//! The management contract against the actual SQLite migrations and fake ports.
//! No provider executable, process, network request or user directory is touched.
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use db::models::{
    workflow::{NodeExecutionStatus, WorkflowRunStatus},
    workflow_files::WorkflowDeliveryCompleteness,
    workflow_queue::WorkflowQueueEntry,
};
use serde_json::json;
use server::{
    error::ApiError,
    routes::{
        integrations::workflows::WorkflowInteractionResponse,
        workflows::{self, CreateWorkflowRequest, UpdateWorkflowRequest},
    },
    workflow_runtime::{
        arena::{
            ArenaWinnerExecution, ArenaWinnerRequest, NoopWorkflowArenaCreator,
            WorkflowArenaWinnerApplier,
        },
        management::{
            self, AcceptedWorkflowSubmission, PrepareWorkflowMainSessionRequest,
            WorkflowActivePolicy, WorkflowDispatchRequest, WorkflowMainSessionView,
            WorkflowManagementCaller, WorkflowManagementInteractionRequest, WorkflowStopStatus,
            WorkflowSubmission, WorkflowSubmissionAction, WorkflowSubmissionScope,
        },
        runner::{
            self, AgentNodeExecution, AgentNodeRequest, WorkflowAgentExecutor,
            WorkflowRunCanceller, WorkflowWorkspaceRequest, WorkflowWorkspaceResolver,
        },
    },
};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tempfile::TempDir;
use uuid::Uuid;
use workflow::files::{
    DispatchAssignment, DispatchPlan, DispatchStage, SplitTask, SplitTaskManifest, WorkflowHandoff,
    control_file_path, write_json_atomic, write_text_atomic,
};

struct Fixture {
    pool: SqlitePool,
    project_id: Uuid,
    workflow_id: Uuid,
    resolver: FixedWorkspace,
    _directory: TempDir,
}

struct FixedWorkspace {
    workspace_id: Uuid,
    calls: AtomicUsize,
}

#[async_trait]
impl WorkflowWorkspaceResolver for FixedWorkspace {
    async fn create_or_bind_main_workspace(
        &self,
        request: WorkflowWorkspaceRequest,
    ) -> Result<Uuid, ApiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(request.existing_workspace_id.unwrap_or(self.workspace_id))
    }

    async fn cleanup_created_main_workspace(&self, _: Uuid) -> Result<(), ApiError> {
        panic!("management test must not delete workspace files")
    }
}

/// Unlike the old standalone-run fixtures, every returned ID is a real row
/// under the production FK constraints. The runner already owns the Node ID.
struct FakeAgent {
    pool: SqlitePool,
    fail_node: Option<&'static str>,
    omit_handoff_node: Option<&'static str>,
    calls: Mutex<Vec<AgentNodeRequest>>,
}

#[async_trait]
impl WorkflowAgentExecutor for FakeAgent {
    async fn run_agent(&self, request: AgentNodeRequest) -> Result<AgentNodeExecution, ApiError> {
        self.calls.lock().unwrap().push(request.clone());
        if self.fail_node == Some(request.node_id.as_str()) {
            return Err(ApiError::BadRequest("fixture node failure".into()));
        }
        let session_id = request
            .session_id
            .expect("accepted Agent Node has a Session");
        let agent_run_id = Uuid::new_v4();
        sqlx::query("INSERT INTO agent_runs(id,session_id,workspace_id,request_id,idempotency_key,correlation_id,schema_version,payload_version,runtime_profile_id,provider_id,workspace_mode,workspace_path,status,request_envelope) VALUES (?,?,?,?,?,?,1,1,'CODEX:DEFAULT','codex','shared_workspace','fixture','succeeded','{}')")
            .bind(agent_run_id).bind(session_id).bind(request.workspace_id)
            .bind(Uuid::new_v4()).bind(agent_run_id.to_string()).bind(request.run_id)
            .execute(&self.pool).await?;
        if self.omit_handoff_node != Some(request.node_id.as_str()) {
            write_agent_delivery(&self.pool, &request, &format!("{} result", request.node_id))
                .await;
        }
        Ok(AgentNodeExecution::Completed {
            session_id,
            orchestration_node_execution_id: request.orchestration_node_execution_id,
            agent_run_id,
            output_text: format!("{} result", request.node_id),
        })
    }
}

async fn write_agent_delivery(pool: &SqlitePool, request: &AgentNodeRequest, text: &str) {
    let execution_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM node_executions WHERE run_id=? AND node_id=? AND iteration=?",
    )
    .bind(request.run_id)
    .bind(&request.node_id)
    .bind(request.iteration)
    .fetch_one(pool)
    .await
    .unwrap();
    let root: String = sqlx::query_scalar("SELECT container_ref FROM workspaces WHERE id=?")
        .bind(request.workspace_id)
        .fetch_one(pool)
        .await
        .unwrap();
    let root = std::path::Path::new(&root);
    if request.node_id == "split" {
        let directory = format!(
            ".vibe-kanban/workflows/{}/splits/{execution_id}",
            request.run_id
        );
        let mut tasks = Vec::new();
        for (id, depends_on) in [
            ("analysis", Vec::new()),
            ("comparison", Vec::new()),
            (
                "recommendation",
                vec!["analysis".to_string(), "comparison".to_string()],
            ),
        ] {
            let instruction_path = format!("{directory}/tasks/{id}.md");
            let path = control_file_path(root, &instruction_path).await.unwrap();
            write_text_atomic(
                &path,
                &format!("Read the accepted request file. Deliver the {id} report."),
            )
            .await
            .unwrap();
            tasks.push(SplitTask {
                subtask_id: id.into(),
                title: format!("Write {id}"),
                instruction_path,
                depends_on,
            });
        }
        let path = control_file_path(root, &format!("{directory}/tasks.json"))
            .await
            .unwrap();
        write_json_atomic(
            &path,
            &SplitTaskManifest {
                schema_version: 1,
                run_id: request.run_id.to_string(),
                split_execution_id: execution_id.to_string(),
                tasks,
            },
        )
        .await
        .unwrap();
        return;
    }
    let result = format!(
        ".vibe-kanban/workflows/{}/results/{}.md",
        request.run_id, execution_id
    );
    let result_path = control_file_path(root, &result).await.unwrap();
    write_text_atomic(&result_path, text).await.unwrap();
    let index = format!(
        ".vibe-kanban/workflows/{}/handoffs/{}.json",
        request.run_id, execution_id
    );
    let index_path = control_file_path(root, &index).await.unwrap();
    write_json_atomic(
        &index_path,
        &WorkflowHandoff {
            schema_version: 1,
            run_id: request.run_id.to_string(),
            node_execution_id: execution_id.to_string(),
            summary_path: Some(result),
            artifact_paths: Vec::new(),
        },
    )
    .await
    .unwrap();
}

#[derive(Default)]
struct FakeStop(Mutex<Vec<Uuid>>);

#[async_trait]
impl WorkflowRunCanceller for FakeStop {
    async fn cancel_session(&self, id: Uuid) -> Result<(), ApiError> {
        self.0.lock().unwrap().push(id);
        Ok(())
    }

    async fn cancel_orchestration_run(&self, _: &SqlitePool, id: Uuid) -> Result<(), ApiError> {
        self.0.lock().unwrap().push(id);
        Ok(())
    }
}

struct NoArenaWinner;

#[async_trait]
impl WorkflowArenaWinnerApplier for NoArenaWinner {
    async fn apply_winner(&self, _: ArenaWinnerRequest) -> Result<ArenaWinnerExecution, ApiError> {
        panic!("human interaction must not apply an Arena winner")
    }
}

fn graph(human_gate: bool) -> String {
    let mut nodes = vec![
        json!({"id":"start","type":"start","data":{}}),
        json!({"id":"agent-a","type":"agent","data":{"prompt_template":"A: {{input}}"}}),
        json!({"id":"agent-b","type":"agent","data":{"prompt_template":"B: {{upstream}}"}}),
        json!({"id":"end","type":"end","data":{}}),
    ];
    let mut edges = vec![
        json!({"id":"a-b","source":"agent-a","target":"agent-b","type":"default"}),
        json!({"id":"b-end","source":"agent-b","target":"end","type":"default"}),
    ];
    if human_gate {
        nodes.push(json!({"id":"gate","type":"human_gate","data":{"prompt_to_human":"Continue?","required_action":"approve_or_reject"}}));
        edges.push(json!({"id":"start-gate","source":"start","target":"gate","type":"default"}));
        edges.push(json!({"id":"gate-a","source":"gate","target":"agent-a","type":"default"}));
    } else {
        edges.push(json!({"id":"start-a","source":"start","target":"agent-a","type":"default"}));
    }
    json!({"version":1,"nodes":nodes,"edges":edges}).to_string()
}

impl Fixture {
    async fn splitter() -> Self {
        let fixture = Self::new(false).await;
        let graph = json!({"version":2,"nodes":[
            {"id":"start","type":"start","data":{}},
            {"id":"split","type":"task_splitter","data":{"prompt_template":"Split the request from files"}},
            {"id":"worker-a","type":"agent","data":{"prompt_template":"Analyze the assigned files"}},
            {"id":"worker-b","type":"agent","data":{"prompt_template":"Compare the assigned files"}},
            {"id":"summary","type":"agent","data":{"prompt_template":"Read and summarize the deliveries"}},
            {"id":"end","type":"end","data":{}}
        ],"edges":[
            {"id":"s-s","source":"start","target":"split","type":"default"},
            {"id":"s-a","source":"split","target":"worker-a","type":"default"},
            {"id":"s-b","source":"split","target":"worker-b","type":"default"},
            {"id":"a-r","source":"worker-a","target":"summary","type":"default"},
            {"id":"b-r","source":"worker-b","target":"summary","type":"default"},
            {"id":"r-e","source":"summary","target":"end","type":"default"}
        ]}).to_string();
        sqlx::query("UPDATE workflows SET graph_json=? WHERE id=?")
            .bind(graph)
            .bind(fixture.workflow_id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        fixture
    }

    async fn start_splitter(
        &self,
    ) -> (
        WorkflowManagementCaller,
        AcceptedWorkflowSubmission,
        FakeAgent,
    ) {
        let prepared = self.prepare("prepare", None).await;
        let caller = main_caller(prepared.session.id);
        let accepted = start(self, &caller, "start").await;
        let agent = self.agent(None);
        self.dispatch(accepted.run_id, &agent).await;
        let run = runner::get_workflow_run_response(&self.pool, accepted.run_id)
            .await
            .unwrap();
        assert_eq!(run.split_batches.len(), 1);
        assert_eq!(run.split_batches[0].state, "accepted");
        assert_eq!(
            agent
                .calls
                .lock()
                .unwrap()
                .iter()
                .map(|r| r.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["split"]
        );
        (caller, accepted, agent)
    }

    async fn write_dispatch(&self, run_id: Uuid, same_executor: bool) -> WorkflowDispatchRequest {
        let run = runner::get_workflow_run_response(&self.pool, run_id)
            .await
            .unwrap();
        let batch = &run.split_batches[0];
        let relative = format!(
            "{}/dispatch.json",
            batch.manifest_path.rsplit_once('/').unwrap().0
        );
        let path = control_file_path(self._directory.path(), &relative)
            .await
            .unwrap();
        write_json_atomic(
            &path,
            &DispatchPlan {
                schema_version: 1,
                run_id: run_id.to_string(),
                split_execution_id: batch.node_execution_id.to_string(),
                manifest_sha256: batch.manifest_sha256.clone(),
                stages: vec![
                    DispatchStage {
                        assignments: vec![
                            DispatchAssignment {
                                subtask_id: "analysis".into(),
                                node_id: "worker-a".into(),
                            },
                            DispatchAssignment {
                                subtask_id: "comparison".into(),
                                node_id: if same_executor {
                                    "worker-a"
                                } else {
                                    "worker-b"
                                }
                                .into(),
                            },
                        ],
                    },
                    DispatchStage {
                        assignments: vec![DispatchAssignment {
                            subtask_id: "recommendation".into(),
                            node_id: "worker-b".into(),
                        }],
                    },
                ],
            },
        )
        .await
        .unwrap();
        WorkflowDispatchRequest {
            request_id: "dispatch".into(),
            batch_id: batch.id,
            dispatch_path: relative,
        }
    }

    async fn progress(
        &self,
        run_id: Uuid,
        agent: &FakeAgent,
    ) -> server::routes::workflows::WorkflowRunResponse {
        let run = runner::reconcile_workflow_run(&self.pool, run_id, agent)
            .await
            .unwrap();
        workflows::sync_attempt_from_run(&self.pool, &run)
            .await
            .unwrap();
        WorkflowQueueEntry::release_terminal(&self.pool, run_id)
            .await
            .unwrap();
        run
    }

    async fn finish(
        &self,
        run_id: Uuid,
        agent: &FakeAgent,
    ) -> server::routes::workflows::WorkflowRunResponse {
        for _ in 0..8 {
            let run = self.progress(run_id, agent).await;
            if matches!(
                run.status,
                WorkflowRunStatus::Succeeded
                    | WorkflowRunStatus::Failed
                    | WorkflowRunStatus::Canceled
            ) {
                return runner::get_workflow_run_response(&self.pool, run_id)
                    .await
                    .unwrap();
            }
        }
        let run = runner::get_workflow_run_response(&self.pool, run_id)
            .await
            .unwrap();
        panic!("fixture Run did not settle within eight reconciliation passes: {run:?}");
    }

    async fn new(human_gate: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(directory.path().join("management.sqlite"))
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        let project_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();
        sqlx::query("INSERT INTO projects(id,name) VALUES (?,'management fixture')")
            .bind(project_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO scratch(id,scratch_type,payload) VALUES (?,'PROJECT_REPO_DEFAULTS',?)")
            .bind(project_id)
            .bind(json!({"type":"PROJECT_REPO_DEFAULTS","data":{"repos":[],"directory_path":directory.path().to_string_lossy()}}).to_string())
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO local_project_statuses(id,project_id,name,color,sort_order,hidden) VALUES (?,?,'Todo','210 80% 52%',0,0)")
            .bind(Uuid::new_v4()).bind(project_id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO workspaces(id,container_ref,workspace_kind,container_ownership,branch) VALUES (?,?,'direct_folder','external','direct-folder')")
            .bind(workspace_id).bind(directory.path().to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let template = workflows::create_project_workflow(
            &pool,
            project_id,
            CreateWorkflowRequest {
                name: "Captured publication".into(),
                description: None,
                graph_json: graph(human_gate),
            },
        )
        .await
        .unwrap();
        sqlx::query("UPDATE workflows SET main_agent_config_json=?,main_agent_prompt='captured prompt' WHERE id=?")
            .bind(json!({"executor":"CODEX","variant":"DEFAULT","model_id":"captured-model"}).to_string()).bind(template.id).execute(&pool).await.unwrap();
        Self {
            pool,
            project_id,
            workflow_id: template.id,
            resolver: FixedWorkspace {
                workspace_id,
                calls: AtomicUsize::new(0),
            },
            _directory: directory,
        }
    }

    async fn prepare(&self, key: &str, issue_id: Option<Uuid>) -> WorkflowMainSessionView {
        management::prepare_workflow_main_session(
            &self.pool,
            self.prepare_request(key, issue_id),
            &self.resolver,
        )
        .await
        .unwrap()
    }

    fn prepare_request(
        &self,
        key: &str,
        issue_id: Option<Uuid>,
    ) -> PrepareWorkflowMainSessionRequest {
        PrepareWorkflowMainSessionRequest {
            project_id: self.project_id,
            workflow_id: self.workflow_id,
            request_id: key.into(),
            issue_id,
        }
    }

    fn agent(&self, fail_node: Option<&'static str>) -> FakeAgent {
        FakeAgent {
            pool: self.pool.clone(),
            fail_node,
            omit_handoff_node: None,
            calls: Mutex::new(Vec::new()),
        }
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn dispatch(&self, run_id: Uuid, agent: &FakeAgent) {
        let claimed = WorkflowQueueEntry::claim_next(&self.pool)
            .await
            .unwrap()
            .expect("ready queue entry");
        assert_eq!(claimed.run_id, run_id);
        runner::start_accepted_workflow_run(&self.pool, run_id, agent, &NoopWorkflowArenaCreator)
            .await
            .unwrap();
        let run = runner::get_workflow_run_response(&self.pool, run_id)
            .await
            .unwrap();
        workflows::sync_attempt_from_run(&self.pool, &run)
            .await
            .unwrap();
        WorkflowQueueEntry::release_terminal(&self.pool, run_id)
            .await
            .unwrap();
    }
}

fn main_caller(session_id: Uuid) -> WorkflowManagementCaller {
    WorkflowManagementCaller::MainSession {
        session_id,
        agent_run_id: None,
        turn_id: None,
        token_hash: None,
    }
}

fn instance_caller(instance_id: Uuid) -> WorkflowManagementCaller {
    WorkflowManagementCaller::Instance {
        instance_id,
        namespace: format!("page-instance:{instance_id}"),
        integration_id: None,
    }
}

fn submission(
    key: &str,
    action: WorkflowSubmissionAction,
    source: Option<Uuid>,
) -> WorkflowSubmission {
    WorkflowSubmission {
        request_id: key.into(),
        action,
        input_text: Some(format!("requirements for {key}")),
        material_paths: Vec::new(),
        source_run_id: source,
        source_node_execution_id: None,
        scope: WorkflowSubmissionScope::All,
        active_policy: None,
        source_message_id: None,
        delivery_intent: None,
        source_batch_id: None,
    }
}

async fn start(
    fixture: &Fixture,
    caller: &WorkflowManagementCaller,
    key: &str,
) -> AcceptedWorkflowSubmission {
    management::submit_workflow(
        &fixture.pool,
        caller,
        submission(key, WorkflowSubmissionAction::Start, None),
        None,
        "fixture",
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn prepared_main_session_uses_the_runtime_executor_identity() {
    for provider in [
        "CODEX",
        "CLAUDE_CODE",
        "GEMINI",
        "OH_MY_PI",
        "OPENCODE",
        "DEEPSEEK_HARNESS",
    ] {
        let f = Fixture::new(false).await;
        sqlx::query("UPDATE workflows SET main_agent_config_json=? WHERE id=?")
            .bind(json!({"executor": provider, "variant": "DEFAULT"}).to_string())
            .bind(f.workflow_id)
            .execute(&f.pool)
            .await
            .unwrap();
        let prepared = f.prepare("provider-identity", None).await;
        assert_eq!(prepared.session.executor.as_deref(), Some(provider));
        assert_eq!(
            prepared.session.executor.as_deref(),
            Some(
                prepared
                    .context
                    .main_agent_config
                    .executor
                    .to_string()
                    .as_str()
            )
        );
        let replay = f.prepare("provider-identity", None).await;
        assert_eq!(replay.session.id, prepared.session.id);
        assert_eq!(replay.session.executor.as_deref(), Some(provider));
        assert_eq!(f.count("agent_runs").await, 0);
    }
}

#[tokio::test]
async fn preparation_is_only_discussion_and_replays_captured_configuration() {
    let f = Fixture::new(false).await;
    let first = f.prepare("prepare", None).await;
    assert_eq!(f.count("local_issues").await, 0);
    assert_eq!(f.count("tasks").await, 0);
    assert_eq!(f.count("agent_runs").await, 0);
    assert!(first.context.instance_id.is_none());
    let membership: Uuid =
        sqlx::query_scalar("SELECT project_id FROM session_project_memberships WHERE session_id=?")
            .bind(first.session.id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(membership, f.project_id);
    sqlx::query("UPDATE workflows SET main_agent_config_json=?,main_agent_prompt='changed prompt' WHERE id=?")
        .bind(json!({"executor":"CLAUDE_CODE","model_id":"replacement-model"}).to_string()).bind(f.workflow_id).execute(&f.pool).await.unwrap();
    let replay = f.prepare("prepare", None).await;
    assert_eq!(first.session.id, replay.session.id);
    assert_eq!(
        replay.context.main_agent_config.model_id.as_deref(),
        Some("captured-model")
    );
    assert_eq!(replay.context.main_agent_prompt, "captured prompt");
    assert_eq!(f.resolver.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.count("agent_runs").await, 0);
    let accepted = start(&f, &main_caller(first.session.id), "captured-acceptance").await;
    let instance = workflows::workflow_attempt_by_id(&f.pool, accepted.instance_id)
        .await
        .unwrap()
        .unwrap();
    let captured = workflows::get_workflow_template(&f.pool, instance.workflow_id)
        .await
        .unwrap();
    assert_eq!(
        captured.main_agent_config.unwrap().model_id.as_deref(),
        Some("captured-model")
    );
    assert_eq!(
        captured.main_agent_prompt.as_deref(),
        Some("captured prompt")
    );
}

#[tokio::test]
async fn ordinary_nodes_read_real_handoff_without_upstream_chat_fallback() {
    let f = Fixture::new(false).await;
    let prepared = f.prepare("prepare", None).await;
    let accepted = start(&f, &main_caller(prepared.session.id), "start").await;
    let agent = f.agent(None);
    f.dispatch(accepted.run_id, &agent).await;
    let run = runner::get_workflow_run_response(&f.pool, accepted.run_id)
        .await
        .unwrap();
    assert_eq!(run.status, WorkflowRunStatus::Succeeded);
    let source_node = run.nodes.iter().find(|n| n.node_id == "agent-a").unwrap();
    let handoff = run
        .handoffs
        .iter()
        .find(|h| h.node_execution_id == source_node.id)
        .unwrap();
    let result_path = &handoff.files[0].path;
    assert_eq!(
        tokio::fs::read_to_string(f._directory.path().join(result_path))
            .await
            .unwrap(),
        "agent-a result"
    );
    let calls = agent.calls.lock().unwrap();
    let downstream = calls.iter().find(|r| r.node_id == "agent-b").unwrap();
    assert!(downstream.prompt.contains(result_path));
    assert!(!downstream.prompt.contains("agent-a result"));
    assert!(
        downstream
            .prompt
            .contains(run.request_path.as_ref().unwrap())
    );
}

#[tokio::test]
async fn missing_agent_handoff_fails_before_downstream_and_never_uses_chat_result() {
    let f = Fixture::new(false).await;
    let prepared = f.prepare("prepare", None).await;
    let accepted = start(&f, &main_caller(prepared.session.id), "start").await;
    let mut agent = f.agent(None);
    agent.omit_handoff_node = Some("agent-a");
    f.dispatch(accepted.run_id, &agent).await;
    let run = runner::get_workflow_run_response(&f.pool, accepted.run_id)
        .await
        .unwrap();
    assert_eq!(run.status, WorkflowRunStatus::Failed);
    assert_eq!(
        agent
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.node_id.as_str())
            .collect::<Vec<_>>(),
        vec!["agent-a"]
    );
    assert!(
        run.nodes
            .iter()
            .any(|n| n.node_id == "agent-a" && n.status == NodeExecutionStatus::Failed)
    );
}

#[tokio::test]
async fn split_dispatch_replay_binds_file_bytes_and_creates_no_business_subtasks() {
    let f = Fixture::splitter().await;
    let (caller, accepted, _) = f.start_splitter().await;
    let business_count = f.count("tasks").await;
    let project_path = f._directory.path().join(".vibe-kanban/project.json");
    let project_before = tokio::fs::read(&project_path).await.unwrap();
    let request = f.write_dispatch(accepted.run_id, false).await;
    let receipt = management::dispatch_workflow(&f.pool, &caller, request.clone())
        .await
        .unwrap();
    assert_eq!(receipt.assigned_count, 3);
    assert_eq!(receipt.stage_count, 2);
    let replay = management::dispatch_workflow(&f.pool, &caller, request.clone())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(receipt).unwrap(),
        serde_json::to_value(replay).unwrap()
    );
    let changed = f.write_dispatch(accepted.run_id, true).await;
    assert_eq!(request.dispatch_path, changed.dispatch_path);
    assert!(
        matches!(management::dispatch_workflow(&f.pool, &caller, changed).await,
        Err(ApiError::Conflict(message)) if message.contains("IDEMPOTENCY_CONFLICT"))
    );
    assert_eq!(f.count("tasks").await, business_count);
    assert_eq!(f.count("workflow_split_subtasks").await, 3);
    assert_eq!(tokio::fs::read(project_path).await.unwrap(), project_before);
}

#[tokio::test]
async fn same_node_subtasks_have_distinct_sessions_and_one_summary_after_all_stages() {
    let f = Fixture::splitter().await;
    let (caller, accepted, agent) = f.start_splitter().await;
    let task_count = f.count("tasks").await;
    let request = f.write_dispatch(accepted.run_id, true).await;
    management::dispatch_workflow(&f.pool, &caller, request)
        .await
        .unwrap();
    let handover: String =
        sqlx::query_scalar("SELECT state FROM workflow_handover_events WHERE run_id=?")
            .bind(accepted.run_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        handover, "completed",
        "manual dispatch must settle only the unlaunched System handover"
    );
    let completed = f.finish(accepted.run_id, &agent).await;
    assert_eq!(completed.status, WorkflowRunStatus::Succeeded);
    let batch = &completed.split_batches[0];
    assert_eq!(batch.state, "succeeded");
    assert!(batch.subtasks.iter().all(|s| s.state == "succeeded"));
    let analysis = batch
        .subtasks
        .iter()
        .find(|s| s.subtask_id == "analysis")
        .unwrap();
    let comparison = batch
        .subtasks
        .iter()
        .find(|s| s.subtask_id == "comparison")
        .unwrap();
    assert_eq!(analysis.node_id.as_deref(), Some("worker-a"));
    assert_eq!(comparison.node_id.as_deref(), Some("worker-a"));
    assert!(analysis.session_id.is_some() && analysis.node_execution_id.is_some());
    assert_ne!(analysis.session_id, comparison.session_id);
    assert_ne!(analysis.node_execution_id, comparison.node_execution_id);
    assert_ne!(analysis.agent_run_id, comparison.agent_run_id);
    let node_ids: Vec<_> = agent
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.node_id.clone())
        .collect();
    assert_eq!(
        node_ids,
        vec!["split", "worker-a", "worker-a", "worker-b", "summary"]
    );
    assert_eq!(f.count("tasks").await, task_count);
    let execution_count = f.count("node_executions").await;
    f.progress(accepted.run_id, &agent).await;
    assert_eq!(
        f.count("node_executions").await,
        execution_count,
        "replay must not add each-trigger or summary duplicates"
    );
    assert_eq!(agent.calls.lock().unwrap().len(), 5);
}

async fn failed_split_fixture() -> (
    Fixture,
    WorkflowManagementCaller,
    AcceptedWorkflowSubmission,
    server::routes::workflows::WorkflowRunResponse,
) {
    let f = Fixture::splitter().await;
    let (caller, accepted, _) = f.start_splitter().await;
    let request = f.write_dispatch(accepted.run_id, false).await;
    management::dispatch_workflow(&f.pool, &caller, request)
        .await
        .unwrap();
    let agent = f.agent(Some("worker-a"));
    let failed = f.finish(accepted.run_id, &agent).await;
    assert_eq!(failed.status, WorkflowRunStatus::Failed);
    let batch = &failed.split_batches[0];
    assert_eq!(batch.state, "failed");
    assert_eq!(
        batch
            .subtasks
            .iter()
            .find(|s| s.subtask_id == "analysis")
            .unwrap()
            .state,
        "failed"
    );
    assert_eq!(
        batch
            .subtasks
            .iter()
            .find(|s| s.subtask_id == "comparison")
            .unwrap()
            .state,
        "succeeded"
    );
    assert_eq!(
        batch
            .subtasks
            .iter()
            .find(|s| s.subtask_id == "recommendation")
            .unwrap()
            .state,
        "blocked"
    );
    assert!(batch.summary_node_execution_id.is_none());
    assert_eq!(
        agent
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.node_id.as_str())
            .collect::<Vec<_>>(),
        vec!["worker-a", "worker-b"]
    );
    (f, caller, accepted, failed)
}

fn partial_submission(
    key: &str,
    run: &server::routes::workflows::WorkflowRunResponse,
) -> WorkflowSubmission {
    let mut request = submission(key, WorkflowSubmissionAction::Rework, Some(run.id));
    request.input_text =
        Some("Summarize verified successful deliveries and explain missing work.".into());
    request.delivery_intent = Some(WorkflowDeliveryCompleteness::Partial);
    request.source_batch_id = Some(run.split_batches[0].id);
    request.scope = WorkflowSubmissionScope::FromNodes {
        node_ids: vec!["summary".into()],
    };
    request
}

#[tokio::test]
async fn explicit_partial_summary_keeps_failed_source_and_missing_files_visible() {
    let (f, caller, source, failed) = failed_split_fixture().await;
    let history = serde_json::to_value(&failed).unwrap();
    let task_count = f.count("tasks").await;
    let request = partial_submission("partial", &failed);
    let partial = management::submit_workflow(&f.pool, &caller, request.clone(), None, "fixture")
        .await
        .unwrap();
    let replay = management::submit_workflow(&f.pool, &caller, request, None, "fixture")
        .await
        .unwrap();
    assert_eq!(partial.run_id, replay.run_id);
    assert_eq!(partial.instance_id, source.instance_id);
    assert_ne!(partial.run_id, source.run_id);
    let agent = f.agent(None);
    f.dispatch(partial.run_id, &agent).await;
    let completed = f.finish(partial.run_id, &agent).await;
    assert_eq!(completed.status, WorkflowRunStatus::Succeeded);
    let delivery = completed.delivery.as_ref().unwrap();
    assert_eq!(delivery.completeness, WorkflowDeliveryCompleteness::Partial);
    assert_eq!(delivery.source_run_id, Some(source.run_id));
    assert_eq!(delivery.source_batch_id, Some(failed.split_batches[0].id));
    let mut missing = delivery
        .missing_subtasks
        .iter()
        .map(|s| s.subtask_id.as_str())
        .collect::<Vec<_>>();
    missing.sort_unstable();
    assert_eq!(missing, vec!["analysis", "recommendation"]);
    let calls = agent.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].node_id, "summary");
    assert!(calls[0].prompt.contains("partial") || calls[0].prompt.contains("Partial"));
    drop(calls);
    let unchanged = runner::get_workflow_run_response(&f.pool, source.run_id)
        .await
        .unwrap();
    let unchanged = serde_json::to_value(unchanged).unwrap();
    // Available actions follow the latest Run; accepted source outcomes and
    // file identities remain immutable even after another Run is accepted.
    for (field, expected) in history.as_object().unwrap() {
        if field != "runtime_view" {
            assert_eq!(
                &unchanged[field], expected,
                "source history changed: {field}"
            );
        }
    }
    assert_eq!(f.count("tasks").await, task_count);
    let source_task_status: String = sqlx::query_scalar("SELECT s.name FROM local_issues i JOIN local_project_statuses s ON s.id=i.status_id WHERE i.id=?")
        .bind(source.issue_id).fetch_one(&f.pool).await.unwrap();
    assert_eq!(
        source_task_status, "Todo",
        "a successful partial summary must not mark the business task done"
    );
}

#[tokio::test]
async fn partial_delivery_rejects_changed_success_file_without_accepting_a_run() {
    let (f, caller, _, failed) = failed_split_fixture().await;
    let comparison = failed.split_batches[0]
        .subtasks
        .iter()
        .find(|s| s.subtask_id == "comparison")
        .unwrap();
    let handoff = failed
        .handoffs
        .iter()
        .find(|h| Some(h.node_execution_id) == comparison.node_execution_id)
        .unwrap();
    let path = f._directory.path().join(&handoff.files[0].path);
    write_text_atomic(&path, "different unaccepted content")
        .await
        .unwrap();
    let run_count = f.count("workflow_runs").await;
    assert!(
        management::submit_workflow(
            &f.pool,
            &caller,
            partial_submission("changed-partial", &failed),
            None,
            "fixture"
        )
        .await
        .is_err()
    );
    assert_eq!(f.count("workflow_runs").await, run_count);
}

#[tokio::test]
async fn stopping_queued_partial_cancels_internal_batch_but_preserves_source_success() {
    let (f, caller, _, failed) = failed_split_fixture().await;
    let partial = management::submit_workflow(
        &f.pool,
        &caller,
        partial_submission("partial-to-stop", &failed),
        None,
        "fixture",
    )
    .await
    .unwrap();
    let stopped = management::stop_workflow(
        &f.pool,
        &caller,
        partial.run_id,
        "stop-partial",
        &FakeStop::default(),
    )
    .await
    .unwrap();
    assert_eq!(stopped.status, WorkflowRunStatus::Canceled);
    let run = runner::get_workflow_run_response(&f.pool, partial.run_id)
        .await
        .unwrap();
    assert!(run.split_batches.iter().all(|b| b.state == "cancelled"));
    assert!(
        run.split_batches
            .iter()
            .flat_map(|b| &b.subtasks)
            .all(|s| !matches!(s.state.as_str(), "pending" | "queued" | "running"))
    );
    let original = runner::get_workflow_run_response(&f.pool, failed.id)
        .await
        .unwrap();
    assert_eq!(original.status, WorkflowRunStatus::Failed);
    assert_eq!(
        original.split_batches[0]
            .subtasks
            .iter()
            .find(|s| s.subtask_id == "comparison")
            .unwrap()
            .state,
        "succeeded"
    );
}

#[tokio::test]
async fn retry_after_partial_runs_only_exact_failed_work_and_its_dependents() {
    let (f, caller, source, failed) = failed_split_fixture().await;
    let failed_analysis = failed.split_batches[0]
        .subtasks
        .iter()
        .find(|s| s.subtask_id == "analysis")
        .unwrap();
    let comparison = failed.split_batches[0]
        .subtasks
        .iter()
        .find(|s| s.subtask_id == "comparison")
        .unwrap();
    let original_nodes = serde_json::to_value(&failed.nodes).unwrap();
    let task_count = f.count("tasks").await;
    let partial = management::submit_workflow(
        &f.pool,
        &caller,
        partial_submission("partial-before-retry", &failed),
        None,
        "fixture",
    )
    .await
    .unwrap();
    let partial_agent = f.agent(None);
    f.dispatch(partial.run_id, &partial_agent).await;
    let delivered = f.finish(partial.run_id, &partial_agent).await;
    let mut retry = submission(
        "retry-analysis",
        WorkflowSubmissionAction::Retry,
        Some(partial.run_id),
    );
    retry.input_text = None;
    retry.source_batch_id = Some(delivered.split_batches[0].id);
    retry.source_node_execution_id = failed_analysis.node_execution_id;
    let accepted = management::submit_workflow(&f.pool, &caller, retry.clone(), None, "fixture")
        .await
        .unwrap();
    let replay = management::submit_workflow(&f.pool, &caller, retry, None, "fixture")
        .await
        .unwrap();
    assert_eq!(accepted.run_id, replay.run_id);
    assert_eq!(accepted.instance_id, source.instance_id);
    let agent = f.agent(None);
    f.dispatch(accepted.run_id, &agent).await;
    let completed = f.finish(accepted.run_id, &agent).await;
    assert_eq!(
        completed.status,
        WorkflowRunStatus::Succeeded,
        "error={:?}; nodes={:?}; batches={:?}",
        completed.error_text,
        completed
            .nodes
            .iter()
            .map(|n| (&n.node_id, &n.status, &n.error_text))
            .collect::<Vec<_>>(),
        completed
            .split_batches
            .iter()
            .map(|b| (&b.state, &b.error_text))
            .collect::<Vec<_>>()
    );
    assert!(
        completed
            .delivery
            .as_ref()
            .is_none_or(|d| d.completeness == WorkflowDeliveryCompleteness::Full)
    );
    let batch = &completed.split_batches[0];
    assert!(batch.subtasks.iter().all(|s| s.state == "succeeded"));
    let reused = batch
        .subtasks
        .iter()
        .find(|s| s.subtask_id == "comparison")
        .unwrap();
    assert_eq!(reused.node_execution_id, comparison.node_execution_id);
    assert_eq!(reused.agent_run_id, comparison.agent_run_id);
    assert_eq!(reused.session_id, comparison.session_id);
    let retried = batch
        .subtasks
        .iter()
        .find(|s| s.subtask_id == "analysis")
        .unwrap();
    assert_ne!(retried.node_execution_id, failed_analysis.node_execution_id);
    let calls = agent
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.node_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(calls, vec!["worker-a", "worker-b", "summary"]);
    assert_eq!(f.count("tasks").await, task_count);
    let original = runner::get_workflow_run_response(&f.pool, failed.id)
        .await
        .unwrap();
    assert_eq!(original.status, WorkflowRunStatus::Failed);
    assert_eq!(
        serde_json::to_value(original.nodes).unwrap(),
        original_nodes
    );
}

#[tokio::test]
async fn stopping_queued_retry_cancels_unstarted_internal_work_without_launching() {
    let (f, caller, _, failed) = failed_split_fixture().await;
    let target = failed.split_batches[0]
        .subtasks
        .iter()
        .find(|s| s.subtask_id == "analysis")
        .unwrap();
    let mut retry = submission(
        "retry-to-stop",
        WorkflowSubmissionAction::Retry,
        Some(failed.id),
    );
    retry.input_text = None;
    retry.source_node_execution_id = target.node_execution_id;
    let accepted = management::submit_workflow(&f.pool, &caller, retry, None, "fixture")
        .await
        .unwrap();
    let calls = f.count("agent_runs").await;
    let stopped = management::stop_workflow(
        &f.pool,
        &caller,
        accepted.run_id,
        "stop-retry",
        &FakeStop::default(),
    )
    .await
    .unwrap();
    assert_eq!(stopped.status, WorkflowRunStatus::Canceled);
    let run = runner::get_workflow_run_response(&f.pool, accepted.run_id)
        .await
        .unwrap();
    assert_eq!(run.split_batches[0].state, "cancelled");
    assert!(
        run.split_batches[0]
            .subtasks
            .iter()
            .all(|s| !matches!(s.state.as_str(), "pending" | "queued" | "running"))
    );
    assert_eq!(f.count("agent_runs").await, calls);
}

#[tokio::test]
async fn partial_delivery_with_no_successful_files_rejects_before_acceptance() {
    let f = Fixture::splitter().await;
    let (caller, accepted, _) = f.start_splitter().await;
    let request = f.write_dispatch(accepted.run_id, false).await;
    management::dispatch_workflow(&f.pool, &caller, request)
        .await
        .unwrap();
    let mut agent = f.agent(Some("worker-a"));
    agent.omit_handoff_node = Some("worker-b");
    let failed = f.finish(accepted.run_id, &agent).await;
    assert_eq!(failed.status, WorkflowRunStatus::Failed);
    assert!(
        failed.split_batches[0]
            .subtasks
            .iter()
            .all(|s| s.state != "succeeded")
    );
    let runs = f.count("workflow_runs").await;
    let error = management::submit_workflow(
        &f.pool,
        &caller,
        partial_submission("empty-partial", &failed),
        None,
        "fixture",
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("NO_DELIVERABLE_RESULTS"));
    assert_eq!(f.count("workflow_runs").await, runs);
}

#[tokio::test]
async fn dispatch_rejects_a_run_settling_failure_without_consuming_its_pending_batch() {
    let f = Fixture::splitter().await;
    let (caller, accepted, _) = f.start_splitter().await;
    let request = f.write_dispatch(accepted.run_id, false).await;
    sqlx::query("UPDATE workflow_runs SET error_text='failure awaiting peer exit' WHERE id=? AND status='running'")
        .bind(accepted.run_id).execute(&f.pool).await.unwrap();
    assert!(
        management::dispatch_workflow(&f.pool, &caller, request)
            .await
            .is_err()
    );
    let run = runner::get_workflow_run_response(&f.pool, accepted.run_id)
        .await
        .unwrap();
    assert_eq!(run.split_batches[0].state, "accepted");
    assert!(
        run.split_batches[0]
            .subtasks
            .iter()
            .all(|s| s.node_execution_id.is_none())
    );
}

#[tokio::test]
async fn first_acceptance_is_singleton_atomic_locked_and_idempotent() {
    let f = Fixture::new(false).await;
    let prepared = f.prepare("prepare", None).await;
    let caller = main_caller(prepared.session.id);
    let request = submission("accept", WorkflowSubmissionAction::Start, None);
    let (left, right) = tokio::join!(
        management::submit_workflow(&f.pool, &caller, request.clone(), None, "fixture"),
        management::submit_workflow(&f.pool, &caller, request.clone(), None, "fixture")
    );
    let first = left.unwrap();
    assert_eq!(first.run_id, right.unwrap().run_id);
    assert_eq!(f.count("local_issues").await, 1);
    assert_eq!(f.count("workflow_attempts").await, 1);
    assert_eq!(f.count("workflow_run_queue").await, 1);
    assert_eq!(f.count("agent_runs").await, 0);
    let instance = workflows::workflow_attempt_by_id(&f.pool, first.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.main_session_id, Some(prepared.session.id));
    assert!(instance.main_session_bound_at.is_some());
    assert!(instance.definition_locked_at.is_some());
    let canonical: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM tasks WHERE issue_id=? AND execution_kind='workflow'",
    )
    .bind(first.issue_id)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(canonical, 1);
    let locked = workflows::update_workflow_template(
        &f.pool,
        instance.workflow_id,
        UpdateWorkflowRequest {
            expected_revision: 1,
            name: None,
            description: None,
            graph_json: Some(graph(true)),
            main_agent_config: None,
            main_agent_prompt: None,
        },
    )
    .await;
    assert!(
        locked.is_err(),
        "accepted instance definition must be frozen"
    );
    let mut changed = request.clone();
    changed.input_text = Some("different requirements".into());
    assert!(
        matches!(management::submit_workflow(&f.pool,&caller,changed,None,"fixture").await,Err(ApiError::Conflict(message)) if message.contains("IDEMPOTENCY_CONFLICT"))
    );
    let mut later = submission(
        "later",
        WorkflowSubmissionAction::Rework,
        Some(first.run_id),
    );
    later.active_policy = Some(WorkflowActivePolicy::AfterCurrent);
    let next = management::submit_workflow(&f.pool, &caller, later, None, "fixture")
        .await
        .unwrap();
    assert_ne!(first.run_id, next.run_id);
    let replay = management::submit_workflow(&f.pool, &caller, request, None, "fixture")
        .await
        .unwrap();
    assert_eq!(
        replay.run_id, first.run_id,
        "old key replays after latest basis moves"
    );
    assert_eq!(f.count("workflow_runs").await, 2);
}

#[tokio::test]
async fn unavailable_project_file_blocks_new_work_but_not_accepted_history() {
    let f = Fixture::new(false).await;
    let prepared = f.prepare("file-boundary-prepare", None).await;
    let caller = main_caller(prepared.session.id);
    let request = submission("file-boundary-start", WorkflowSubmissionAction::Start, None);
    let accepted = management::submit_workflow(&f.pool, &caller, request.clone(), None, "fixture")
        .await
        .unwrap();
    let path = f._directory.path().join(".vibe-kanban/project.json");
    tokio::fs::write(&path, b"{corrupt").await.unwrap();
    // Idempotent receipt/history remains available for already accepted work.
    let replay = management::submit_workflow(&f.pool, &caller, request, None, "fixture")
        .await
        .unwrap();
    assert_eq!(replay.run_id, accepted.run_id);
    runner::get_workflow_run_response(&f.pool, accepted.run_id)
        .await
        .unwrap();
    let new_request = submission(
        "file-boundary-rework",
        WorkflowSubmissionAction::Rework,
        Some(accepted.run_id),
    );
    let error = management::submit_workflow(&f.pool, &caller, new_request, None, "fixture")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("PROJECT_DATA_INVALID"));
    assert_eq!(f.count("workflow_runs").await, 1);
    assert_eq!(tokio::fs::read(path).await.unwrap(), b"{corrupt");
}

#[tokio::test]
async fn reopening_preserves_original_session_and_deleted_session_cannot_be_replaced() {
    let f = Fixture::new(false).await;
    let prepared = f.prepare("prepare", None).await;
    let caller = main_caller(prepared.session.id);
    let accepted = start(&f, &caller, "start").await;
    let reopened = f.prepare("reopen", Some(accepted.issue_id)).await;
    assert_eq!(reopened.session.id, prepared.session.id);
    assert_eq!(f.resolver.calls.load(Ordering::SeqCst), 1);
    management::stop_workflow(
        &f.pool,
        &caller,
        accepted.run_id,
        "stop",
        &FakeStop::default(),
    )
    .await
    .unwrap();
    sqlx::query("DELETE FROM sessions WHERE id=?")
        .bind(prepared.session.id)
        .execute(&f.pool)
        .await
        .unwrap();
    for request in [
        f.prepare_request("prepare", None),
        f.prepare_request("after-delete", Some(accepted.issue_id)),
    ] {
        assert!(matches!(
            management::prepare_workflow_main_session(&f.pool, request, &f.resolver).await,
            Err(ApiError::Conflict(_))
        ));
    }
    let history = management::get_workflow_instance(
        &f.pool,
        &instance_caller(accepted.instance_id),
        None,
        None,
        20,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(history.instance.main_session_id.is_none());
    assert!(history.instance.main_session_bound_at.is_some());
    assert_eq!(history.runs.len(), 1);
    assert!(!history.notifications.is_empty());
    assert!(
        history
            .notifications
            .iter()
            .all(|n| n.main_session_id.is_none())
    );
    assert_eq!(f.count("agent_runs").await, 0);
}

#[tokio::test]
async fn retry_appends_run_reuses_exact_successes_and_preserves_old_history() {
    let f = Fixture::new(false).await;
    let prepared = f.prepare("prepare", None).await;
    let caller = main_caller(prepared.session.id);
    let first = start(&f, &caller, "start").await;
    let failing = f.agent(Some("agent-b"));
    f.dispatch(first.run_id, &failing).await;
    let original = runner::get_workflow_run_response(&f.pool, first.run_id)
        .await
        .unwrap();
    assert_eq!(original.status, WorkflowRunStatus::Failed);
    let old_b = original
        .nodes
        .iter()
        .find(|n| n.node_id == "agent-b")
        .unwrap();
    assert_eq!(old_b.status, NodeExecutionStatus::Failed);
    let old_json = serde_json::to_value(&original.nodes).unwrap();
    let mut retry = submission("retry", WorkflowSubmissionAction::Retry, Some(first.run_id));
    retry.source_node_execution_id = Some(old_b.id);
    retry.input_text = None;
    let second = management::submit_workflow(&f.pool, &caller, retry, None, "fixture")
        .await
        .unwrap();
    assert_ne!(second.run_id, first.run_id);
    let before = runner::get_workflow_run_response(&f.pool, second.run_id)
        .await
        .unwrap();
    assert_eq!(before.input_text, original.input_text);
    assert!(
        before
            .nodes
            .iter()
            .all(|n| n.node_id == "agent-b" || n.node_id == "end")
    );
    let projected = before.runtime_view.as_ref().unwrap();
    assert_eq!(projected.reused_node_count, 2);
    assert_eq!(projected.completed_node_count, 0);
    let reused_a = projected
        .node_work
        .iter()
        .find(|n| n.node_id == "agent-a")
        .unwrap();
    assert_eq!(
        serde_json::to_value(reused_a.status).unwrap(),
        json!("reused")
    );
    assert!(reused_a.active_execution_id.is_none());
    assert!(reused_a.active_session_id.is_none());
    assert!(!reused_a.can_retry && !reused_a.can_open_session && !reused_a.can_approve);
    assert_eq!(reused_a.reused_results[0].source_run_id, first.run_id);
    let history =
        management::get_workflow_instance(&f.pool, &caller, Some(second.run_id), None, 20)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(history.reuse.len(), 2);
    for reused in &history.reuse {
        assert_eq!(reused.source_run_id, first.run_id);
        assert!(
            original
                .nodes
                .iter()
                .any(|n| n.id == reused.source_node_execution_id
                    && n.node_id == reused.node_id
                    && n.iteration == reused.iteration)
        );
    }
    let success = f.agent(None);
    f.dispatch(second.run_id, &success).await;
    assert_eq!(
        success
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.node_id.as_str())
            .collect::<Vec<_>>(),
        vec!["agent-b"]
    );
    let new_run = runner::get_workflow_run_response(&f.pool, second.run_id)
        .await
        .unwrap();
    assert_eq!(new_run.status, WorkflowRunStatus::Succeeded);
    let new_b = new_run
        .nodes
        .iter()
        .find(|n| n.node_id == "agent-b")
        .unwrap();
    assert_ne!(new_b.id, old_b.id);
    assert_eq!(new_b.task_id, old_b.task_id);
    assert_eq!(new_b.session_id, old_b.session_id);
    let preserved = runner::get_workflow_run_response(&f.pool, first.run_id)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&preserved.nodes).unwrap(), old_json);
    assert_eq!(preserved.error_text, original.error_text);
    assert_eq!(preserved.finished_at, original.finished_at);
    assert_eq!(f.count("workflow_attempts").await, 1);
}

#[tokio::test]
async fn partial_rework_projects_every_real_reused_iteration_without_new_executions() {
    let f = Fixture::new(false).await;
    let fan_in = json!({"version":2,"nodes":[
        {"id":"start","type":"start","data":{}},
        {"id":"left","type":"transform","data":{"mode":"template","template":"left {{input}}"}},
        {"id":"right","type":"transform","data":{"mode":"template","template":"right {{input}}"}},
        {"id":"review","type":"agent","data":{"prompt_template":"Review {{upstream}}"}},
        {"id":"end","type":"end","data":{}}
    ],"edges":[
        {"id":"sl","source":"start","target":"left","type":"default"},
        {"id":"sr","source":"start","target":"right","type":"default"},
        {"id":"lr","source":"left","target":"review","type":"default"},
        {"id":"rr","source":"right","target":"review","type":"default"},
        {"id":"re","source":"review","target":"end","type":"default"}
    ]})
    .to_string();
    sqlx::query("UPDATE workflows SET graph_json=? WHERE id=?")
        .bind(fan_in)
        .bind(f.workflow_id)
        .execute(&f.pool)
        .await
        .unwrap();
    let prepared = f.prepare("prepare", None).await;
    let caller = main_caller(prepared.session.id);
    let source = start(&f, &caller, "start").await;
    let agent = f.agent(None);
    f.dispatch(source.run_id, &agent).await;
    let completed = runner::get_workflow_run_response(&f.pool, source.run_id)
        .await
        .unwrap();
    assert_eq!(completed.status, WorkflowRunStatus::Succeeded);
    let source_review: Vec<_> = completed
        .nodes
        .iter()
        .filter(|n| n.node_id == "review")
        .collect();
    assert_eq!(source_review.len(), 2);
    let mut request = submission(
        "end-only",
        WorkflowSubmissionAction::Rework,
        Some(source.run_id),
    );
    request.scope = WorkflowSubmissionScope::FromNodes {
        node_ids: vec!["end".into()],
    };
    let accepted = management::submit_workflow(&f.pool, &caller, request, None, "fixture")
        .await
        .unwrap();
    let projected = runner::get_workflow_run_response(&f.pool, accepted.run_id)
        .await
        .unwrap();
    assert!(projected.nodes.iter().all(|n| n.node_id == "end"));
    let view = projected.runtime_view.unwrap();
    assert_eq!(view.reused_node_count, 4);
    assert_eq!(view.completed_node_count, 0);
    let review = view
        .node_work
        .iter()
        .find(|n| n.node_id == "review")
        .unwrap();
    assert_eq!(review.reused_results.len(), 2);
    assert_eq!(review.iteration, 1);
    for result in &review.reused_results {
        assert_eq!(result.source_run_id, source.run_id);
        assert!(source_review.iter().any(|n|n.id==result.source_node_execution_id && n.iteration==result.iteration));
    }
    assert!(
        review.active_execution_id.is_none()
            && review.orchestration_node_execution_id.is_none()
            && review.active_agent_run_id.is_none()
    );
    assert!(!review.can_retry && !review.can_open_session && !review.can_cancel_node);
    assert_eq!(agent.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn known_missing_reuse_rejects_before_acceptance_and_does_not_consume_key() {
    let f = Fixture::new(false).await;
    let prepared = f.prepare("prepare", None).await;
    let caller = main_caller(prepared.session.id);
    let first = start(&f, &caller, "start").await;
    management::stop_workflow(&f.pool, &caller, first.run_id, "stop", &FakeStop::default())
        .await
        .unwrap();
    let mut request = submission(
        "partial",
        WorkflowSubmissionAction::Rework,
        Some(first.run_id),
    );
    request.scope = WorkflowSubmissionScope::FromNodes {
        node_ids: vec!["agent-b".into()],
    };
    assert!(
        matches!(management::submit_workflow(&f.pool,&caller,request.clone(),None,"fixture").await,Err(ApiError::Conflict(message)) if message.contains("REUSE_UNAVAILABLE"))
    );
    assert_eq!(f.count("workflow_runs").await, 1);
    assert_eq!(f.count("workflow_run_submissions").await, 1);
    let instance = workflows::workflow_attempt_by_id(&f.pool, first.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.latest_run_id, Some(first.run_id));
    request.scope = WorkflowSubmissionScope::All;
    let fixed = management::submit_workflow(&f.pool, &caller, request, None, "fixture")
        .await
        .unwrap();
    assert_eq!(fixed.phase, "queued");
    assert_eq!(f.count("workflow_runs").await, 2);
}

#[tokio::test]
async fn stop_intent_retries_until_the_outbox_and_project_boundary_settle() {
    let f = Fixture::new(false).await;
    let prepared = f.prepare("prepare", None).await;
    let caller = main_caller(prepared.session.id);
    let accepted = start(&f, &caller, "start").await;
    f.dispatch(accepted.run_id, &f.agent(None)).await;
    let completed = runner::get_workflow_run_response(&f.pool, accepted.run_id)
        .await
        .unwrap();
    assert_eq!(completed.status, WorkflowRunStatus::Succeeded);

    // Model a natural terminal outcome with a delayed, failed control delivery.
    // The fake canceller acknowledges a drain pass before its backoff expires;
    // it does not prove that the command was delivered or the slot settled.
    let node = completed
        .nodes
        .iter()
        .find(|n| n.node_id == "agent-a")
        .unwrap();
    let command = executors::runtime::AgentRunPortCommandEnvelope {
        schema_version: executors::runtime::ORCHESTRATION_COMMAND_SCHEMA_VERSION,
        command_id: Uuid::new_v4(),
        idempotency_key: "delayed-stop-control".into(),
        agent_run_id: node.agent_run_id.unwrap(),
        orchestration_run_id: Some(accepted.run_id),
        orchestration_node_execution_id: node.orchestration_node_execution_id,
        correlation_id: accepted.run_id,
        created_at: chrono::Utc::now(),
        command: executors::runtime::AgentRunPortCommand::Cancel {
            reason: "fixture delayed control".into(),
        },
    };
    sqlx::query("INSERT INTO orchestration_outbox(id,orchestration_run_id,node_execution_id,command_id,idempotency_key,command_schema_version,command_envelope,delivery_status,available_at,last_error) VALUES (?,?,?,?,?,1,?,'failed','9999-12-31T23:59:59Z','fixture audit failure')")
        .bind(Uuid::new_v4()).bind(accepted.run_id)
        .bind(node.orchestration_node_execution_id).bind(command.command_id)
        .bind(&command.idempotency_key).bind(serde_json::to_string(&command).unwrap())
        .execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO workflow_project_slots(project_id,run_id) VALUES (?,?)")
        .bind(f.project_id)
        .bind(accepted.run_id)
        .execute(&f.pool)
        .await
        .unwrap();
    let canceller = FakeStop::default();
    let stopped = management::stop_workflow(&f.pool, &caller, accepted.run_id, "stop", &canceller)
        .await
        .unwrap();
    assert_eq!(stopped.status, WorkflowRunStatus::Succeeded);
    assert!(matches!(stopped.stop_status, WorkflowStopStatus::Requested));
    assert_eq!(f.count("workflow_project_slots").await, 1);
    let intent_state = || async {
        sqlx::query_scalar::<_, String>("SELECT state FROM workflow_stop_intents WHERE run_id=?")
            .bind(accepted.run_id)
            .fetch_one(&f.pool)
            .await
            .unwrap()
    };
    assert_eq!(intent_state().await, "pending");

    // A background pass and same-request replay must retain the exact intent
    // and command, so a transient control failure cannot strand cancellation.
    management::deliver_stop_intents(&f.pool, &canceller)
        .await
        .unwrap();
    let replay = management::stop_workflow(&f.pool, &caller, accepted.run_id, "stop", &canceller)
        .await
        .unwrap();
    assert!(matches!(replay.stop_status, WorkflowStopStatus::Requested));
    assert_eq!(intent_state().await, "pending");
    assert_eq!(canceller.0.lock().unwrap().len(), 3);
    assert_eq!(f.count("workflow_stop_intents").await, 1);
    assert_eq!(f.count("orchestration_outbox").await, 1);
    assert_eq!(f.count("workflow_runs").await, 1);

    // The independent control dispatcher now confirms delivery. Only this
    // fact permits slot release and completion of the original stop intent.
    sqlx::query("UPDATE orchestration_outbox SET delivery_status='delivered' WHERE command_id=?")
        .bind(command.command_id)
        .execute(&f.pool)
        .await
        .unwrap();
    management::deliver_stop_intents(&f.pool, &canceller)
        .await
        .unwrap();
    assert_eq!(intent_state().await, "delivered");
    assert_eq!(f.count("workflow_project_slots").await, 0);
    let confirmed =
        management::stop_workflow(&f.pool, &caller, accepted.run_id, "stop", &canceller)
            .await
            .unwrap();
    assert!(matches!(
        confirmed.stop_status,
        WorkflowStopStatus::Confirmed
    ));
    let final_run = runner::get_workflow_run_response(&f.pool, accepted.run_id)
        .await
        .unwrap();
    assert_eq!(final_run.status, completed.status);
    assert_eq!(final_run.output_text, completed.output_text);
    assert_eq!(final_run.finished_at, completed.finished_at);
}

#[tokio::test]
async fn user_stop_cancels_waiting_dependents_without_canceling_another_issue() {
    let f = Fixture::new(false).await;
    let prepared = f.prepare("prepare", None).await;
    let caller = main_caller(prepared.session.id);
    let first = start(&f, &caller, "start").await;
    WorkflowQueueEntry::claim_next(&f.pool)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE workflow_runs SET status='running' WHERE id=?")
        .bind(first.run_id)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE workflow_run_queue SET phase='active' WHERE run_id=?")
        .bind(first.run_id)
        .execute(&f.pool)
        .await
        .unwrap();
    let mut next = submission("next", WorkflowSubmissionAction::Rework, Some(first.run_id));
    next.active_policy = Some(WorkflowActivePolicy::AfterCurrent);
    let dependent = management::submit_workflow(&f.pool, &caller, next, None, "fixture")
        .await
        .unwrap();
    assert_eq!(dependent.phase, "waiting_for_source");
    let dependent_nodes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM node_executions WHERE run_id=?")
            .bind(dependent.run_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(dependent_nodes, 0);
    let unrelated = f.prepare("other-issue", None).await;
    let other = start(&f, &main_caller(unrelated.session.id), "other-start").await;
    let stopped =
        management::stop_workflow(&f.pool, &caller, first.run_id, "stop", &FakeStop::default())
            .await
            .unwrap();
    assert!(matches!(stopped.stop_status, WorkflowStopStatus::Confirmed));
    assert_eq!(
        stopped.affected_run_ids,
        vec![first.run_id, dependent.run_id]
    );
    assert!(stopped.unresolved_source_run_ids.is_empty());
    for _ in 0..2 {
        management::resolve_waiting_submissions(&f.pool)
            .await
            .unwrap();
    }
    let pending = runner::get_workflow_run_response(&f.pool, dependent.run_id)
        .await
        .unwrap();
    assert_eq!(pending.status, WorkflowRunStatus::Canceled);
    assert_eq!(pending.queue_phase.as_deref(), Some("finished"));
    let claimed = WorkflowQueueEntry::claim_next(&f.pool)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.run_id, other.run_id);
    assert_eq!(f.count("agent_runs").await, 0);
}

#[tokio::test]
async fn internal_replacement_stop_preserves_successor_fifo_and_never_launches_source() {
    let f = Fixture::new(false).await;
    let prepared = f.prepare("prepare", None).await;
    let caller = main_caller(prepared.session.id);
    let first = start(&f, &caller, "start").await;
    let mut request = submission(
        "replace",
        WorkflowSubmissionAction::Rework,
        Some(first.run_id),
    );
    request.active_policy = Some(WorkflowActivePolicy::StopThenRun);
    let replacement = management::submit_workflow(&f.pool, &caller, request, None, "fixture")
        .await
        .unwrap();
    assert_eq!(replacement.phase, "stopping_source");
    let sequence: i64 =
        sqlx::query_scalar("SELECT sequence FROM workflow_run_queue WHERE run_id=?")
            .bind(replacement.run_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    management::deliver_stop_intents(&f.pool, &FakeStop::default())
        .await
        .unwrap();
    management::resolve_waiting_submissions(&f.pool)
        .await
        .unwrap();
    let ready = runner::get_workflow_run_response(&f.pool, replacement.run_id)
        .await
        .unwrap();
    assert_eq!(ready.status, WorkflowRunStatus::Pending);
    assert_eq!(ready.queue_phase.as_deref(), Some("queued"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT sequence FROM workflow_run_queue WHERE run_id=?")
            .bind(replacement.run_id)
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        sequence
    );
    assert_eq!(
        runner::get_workflow_run_response(&f.pool, first.run_id)
            .await
            .unwrap()
            .status,
        WorkflowRunStatus::Canceled
    );
    assert_eq!(
        WorkflowQueueEntry::claim_next(&f.pool)
            .await
            .unwrap()
            .unwrap()
            .run_id,
        replacement.run_id
    );
    assert_eq!(f.count("agent_runs").await, 0);
}

#[tokio::test]
async fn dependency_plan_failure_is_retained_as_a_terminal_accepted_run() {
    let f = Fixture::new(false).await;
    let prepared = f.prepare("prepare", None).await;
    let caller = main_caller(prepared.session.id);
    let first = start(&f, &caller, "start").await;
    let mut partial = submission(
        "partial",
        WorkflowSubmissionAction::Rework,
        Some(first.run_id),
    );
    partial.scope = WorkflowSubmissionScope::FromNodes {
        node_ids: vec!["agent-b".into()],
    };
    partial.active_policy = Some(WorkflowActivePolicy::AfterCurrent);
    let accepted = management::submit_workflow(&f.pool, &caller, partial, None, "fixture")
        .await
        .unwrap();
    assert_eq!(accepted.phase, "waiting_for_source");
    // Model a recovered source that failed before producing reusable A output.
    runner::fail_accepted_workflow_run(&f.pool, first.run_id, "fixture source failure")
        .await
        .unwrap();
    WorkflowQueueEntry::release_terminal(&f.pool, first.run_id)
        .await
        .unwrap();
    management::resolve_waiting_submissions(&f.pool)
        .await
        .unwrap();
    let failed = runner::get_workflow_run_response(&f.pool, accepted.run_id)
        .await
        .unwrap();
    assert_eq!(failed.status, WorkflowRunStatus::Failed);
    assert_eq!(failed.queue_phase.as_deref(), Some("finished"));
    assert!(
        failed
            .error_text
            .as_deref()
            .unwrap()
            .contains("WORKFLOW_PLANNING_FAILED")
    );
    assert!(failed.nodes.is_empty());
    let instance = workflows::workflow_attempt_by_id(&f.pool, accepted.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.latest_run_id, Some(accepted.run_id));
    assert_eq!(
        serde_json::to_value(instance.status).unwrap(),
        json!("failed")
    );
    let notifications =
        management::list_workflow_notifications(&f.pool, prepared.session.id, 0, 100)
            .await
            .unwrap();
    assert!(
        notifications
            .notifications
            .iter()
            .any(|n| n.run_id == accepted.run_id && n.kind == "run_terminal")
    );
    assert_eq!(f.count("agent_runs").await, 0);
}

#[tokio::test]
async fn outcome_and_notifications_are_atomic_deduplicated_and_cleanup_preserves_result() {
    let f = Fixture::new(false).await;
    let prepared = f.prepare("prepare", None).await;
    let caller = main_caller(prepared.session.id);
    let accepted = start(&f, &caller, "start").await;
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query("UPDATE workflow_runs SET status='failed',error_text='rolled back',finished_at=datetime('now','subsec') WHERE id=?")
        .bind(accepted.run_id).execute(&mut *tx).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workflow_notifications")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        1
    );
    tx.rollback().await.unwrap();
    assert_eq!(f.count("workflow_notifications").await, 0);
    assert_eq!(
        runner::get_workflow_run_response(&f.pool, accepted.run_id)
            .await
            .unwrap()
            .status,
        WorkflowRunStatus::Pending
    );
    let agent = f.agent(None);
    f.dispatch(accepted.run_id, &agent).await;
    let natural = runner::get_workflow_run_response(&f.pool, accepted.run_id)
        .await
        .unwrap();
    assert_eq!(natural.status, WorkflowRunStatus::Succeeded);
    let count = f.count("agent_runs").await;
    management::stop_workflow(
        &f.pool,
        &caller,
        accepted.run_id,
        "cleanup",
        &FakeStop::default(),
    )
    .await
    .unwrap();
    management::stop_workflow(
        &f.pool,
        &caller,
        accepted.run_id,
        "cleanup",
        &FakeStop::default(),
    )
    .await
    .unwrap();
    let preserved = runner::get_workflow_run_response(&f.pool, accepted.run_id)
        .await
        .unwrap();
    assert_eq!(preserved.status, natural.status);
    assert_eq!(preserved.output_text, natural.output_text);
    assert_eq!(preserved.error_text, natural.error_text);
    assert_eq!(preserved.finished_at, natural.finished_at);
    let page = management::list_workflow_notifications(&f.pool, prepared.session.id, 0, 100)
        .await
        .unwrap();
    assert_eq!(
        page.notifications
            .iter()
            .filter(|n| n.kind == "run_terminal")
            .count(),
        1
    );
    management::read_workflow_context(&f.pool, prepared.session.id)
        .await
        .unwrap();
    management::get_workflow_instance(&f.pool, &caller, None, None, 20)
        .await
        .unwrap();
    assert_eq!(
        f.count("agent_runs").await,
        count,
        "read/cleanup never wake the main Agent"
    );
}

#[tokio::test]
async fn interaction_is_exact_id_cas_and_notification_resolves_without_fake_chat() {
    let f = Fixture::new(true).await;
    let prepared = f.prepare("prepare", None).await;
    let caller = main_caller(prepared.session.id);
    let accepted = start(&f, &caller, "start").await;
    let agent = f.agent(None);
    f.dispatch(accepted.run_id, &agent).await;
    assert!(agent.calls.lock().unwrap().is_empty());
    let waiting = management::list_run_interactions(&f.pool, accepted.run_id)
        .await
        .unwrap();
    assert_eq!(waiting.len(), 1);
    let gate_id = waiting[0].id;
    let events = management::list_workflow_notifications(&f.pool, prepared.session.id, 0, 100)
        .await
        .unwrap();
    assert!(
        events
            .notifications
            .iter()
            .any(|n| n.interaction_id == Some(gate_id) && !n.is_resolved)
    );
    let wrong = WorkflowManagementInteractionRequest {
        run_id: accepted.run_id,
        node_execution_id: gate_id,
        request_id: "wrong-kind".into(),
        response: WorkflowInteractionResponse::SelectBranch {
            selected_target_node_ids: vec!["agent-a".into()],
            reason: None,
        },
    };
    assert!(matches!(
        management::respond_to_workflow(
            &f.pool,
            &caller,
            wrong,
            &agent,
            &NoopWorkflowArenaCreator,
            &NoArenaWinner
        )
        .await,
        Err(ApiError::BadRequest(_))
    ));
    assert_eq!(f.count("workflow_operation_requests").await, 0);
    let approve = WorkflowManagementInteractionRequest {
        run_id: accepted.run_id,
        node_execution_id: gate_id,
        request_id: "approve".into(),
        response: WorkflowInteractionResponse::Approve,
    };
    let complete = management::respond_to_workflow(
        &f.pool,
        &caller,
        approve.clone(),
        &agent,
        &NoopWorkflowArenaCreator,
        &NoArenaWinner,
    )
    .await
    .unwrap();
    assert_eq!(complete.status, WorkflowRunStatus::Succeeded);
    let calls = agent.calls.lock().unwrap().len();
    management::respond_to_workflow(
        &f.pool,
        &caller,
        approve.clone(),
        &agent,
        &NoopWorkflowArenaCreator,
        &NoArenaWinner,
    )
    .await
    .unwrap();
    assert_eq!(agent.calls.lock().unwrap().len(), calls);
    let mut changed = approve;
    changed.response = WorkflowInteractionResponse::Reject;
    assert!(
        matches!(management::respond_to_workflow(&f.pool,&caller,changed,&agent,&NoopWorkflowArenaCreator,&NoArenaWinner).await,Err(ApiError::Conflict(message)) if message.contains("IDEMPOTENCY_CONFLICT"))
    );
    let events = management::list_workflow_notifications(&f.pool, prepared.session.id, 0, 100)
        .await
        .unwrap();
    assert!(
        events
            .notifications
            .iter()
            .filter(|n| n.interaction_id == Some(gate_id))
            .all(|n| n.is_resolved)
    );
    assert_eq!(f.count("workflow_interaction_responses").await, 1);
    let roles: Vec<String> = sqlx::query("SELECT input_message FROM agent_turns")
        .fetch_all(&f.pool)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get("input_message"))
        .collect();
    assert!(
        roles.is_empty(),
        "system notifications never fabricate Assistant turns"
    );
}
