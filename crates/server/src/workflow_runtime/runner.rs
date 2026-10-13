use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex, Weak},
    time::Duration,
};

use async_trait::async_trait;
use chrono::Utc;
use db::models::{
    orchestration::OrchestrationNodeExecutionRecord,
    session::{CreateSession, Session, SessionError},
    task::{CreateExecution, Execution, ExecutionKind},
    workflow::{NodeExecutionStatus as DbNodeExecutionStatus, WorkflowRunStatus},
    workspace::{Workspace, WorkspaceError, WorkspaceKind},
    workspace_repo::{CreateWorkspaceRepo, WorkspaceRepo},
};
use deployment::Deployment;
use executors::{
    actions::SelectedSkill,
    executors::{BaseCodingAgent, provider_adapter::DirectProvider},
    profile::{ExecutorConfig, ExecutorConfigs},
    provider_policy::direct_provider_capability_snapshot,
    runtime::{
        AGENT_REQUEST_PAYLOAD_VERSION, AGENT_REQUEST_SCHEMA_VERSION, AgentRunIntent,
        AgentRunPortCommand, AgentRunPortCommandEnvelope, AgentRunPortError,
        AgentRunRequestEnvelope, AgentRunStatus, AgentRuntimeMessageRole, CanonicalMessage,
        EachDownstreamExecution, ORCHESTRATION_COMMAND_SCHEMA_VERSION,
        ORCHESTRATION_PLAN_SCHEMA_VERSION, OrchestrationFailurePolicy, OrchestrationJoinPolicy,
        OrchestrationPlanNode, OrchestrationPlanSnapshot, OrchestrationProductKind,
        OrchestrationRetryPolicy, RemainingUpstreamsPolicy, RunAttemptMode, RunAttemptRequest,
        WorkspaceMode, WorkspaceReference,
    },
};
use futures_util::StreamExt;
use git::{GitCli, StatusEntry, WorktreeStatus};
use serde_json::{Value, json};
use services::services::{
    container::ContainerService,
    orchestration::{OrchestrationService, OrchestrationServiceError},
};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection, SqlitePool, sqlite::SqliteRow};
use thiserror::Error;
use tokio::sync::broadcast;
use uuid::Uuid;
use workflow::{
    WorkflowGraph,
    dispatch::{batch_managed_nodes, dispatch_scope},
    events::WorkflowEvent,
    files::{SplitTaskManifest, validate_manifest},
    graph::{WorkflowEdge, WorkflowNode, WorkflowNodeKind},
    handlers::{NodeHandlerContext, NodeHandlerStatus, UpstreamOutput, handle_pure_node},
    planner::{
        NodeExecutionSnapshot, NodeExecutionStatus as PlannerNodeExecutionStatus, RunSnapshot,
        triggered_execution_count,
    },
    runner::WorkflowRunner,
    validation::validate_graph_for_run,
};

#[path = "agent_launch.rs"]
mod agent_launch;
#[path = "batch_runtime.rs"]
mod batch_runtime;
#[path = "file_handoff.rs"]
pub(crate) mod file_handoff;
#[path = "recovery.rs"]
mod recovery;

use recovery::{
    children_settled, recover_undispatched_nodes, settle_workflow_failure, workflow_run_lock,
};

use crate::{
    DeploymentImpl,
    error::ApiError,
    routes::workflows::{
        TriggerWorkflowRequest, WORKFLOW_NODE_ACTIVE_SLOW_THRESHOLD_MS,
        WorkflowNodeExecutionResponse, WorkflowRunResponse, build_workflow_run_runtime_view,
        ensure_agent_node_sessions, get_workflow_template, persist_workflow_graph,
    },
    workflow_runtime::{
        arena::{
            ArenaNodeAttemptRequest, ArenaNodeExecution, ArenaNodeRequest, ArenaWinnerExecution,
            ArenaWinnerRequest, DeploymentWorkflowArenaCreator, NoopWorkflowArenaCreator,
            WorkflowArenaCreator, WorkflowArenaWinnerApplier,
        },
        condition_router::{
            ConditionRouterCompletion, RouterUpstreamNode, build_manual_route, build_router_prompt,
            evaluate_router_output, output_has_router_mutation_warning,
        },
        envelope::{
            WorkflowAgentEnvelope, WorkflowEnvelopeUpstream,
            render_workflow_agent_envelope as render_envelope,
        },
        workspace::{main_workflow_branch_name, short_run_id},
    },
};

#[derive(Debug, Error)]
enum WorkflowRuntimeError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("{0}")]
    BadRequest(String),
}

impl From<WorkflowRuntimeError> for ApiError {
    fn from(error: WorkflowRuntimeError) -> Self {
        match error {
            WorkflowRuntimeError::Database(err) => ApiError::Database(err),
            WorkflowRuntimeError::BadRequest(message) => ApiError::BadRequest(message),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowWorkspaceRequest {
    pub directory_path: Option<String>,
    pub issue_id: Uuid,
    pub run_id: Uuid,
    pub project_id: Option<Uuid>,
    pub existing_workspace_id: Option<Uuid>,
    pub repo_overrides: Vec<CreateWorkspaceRepo>,
    pub branch_name: String,
}

#[derive(Debug)]
pub struct WorkflowRunStartRequest {
    pub directory_path: Option<String>,
    pub workflow_id: Uuid,
    pub attempt_id: Option<Uuid>,
    pub trigger: TriggerWorkflowRequest,
    pub repo_overrides: Vec<CreateWorkspaceRepo>,
}

#[async_trait]
pub trait WorkflowWorkspaceResolver: Send + Sync {
    async fn create_or_bind_main_workspace(
        &self,
        request: WorkflowWorkspaceRequest,
    ) -> Result<Uuid, ApiError>;

    async fn cleanup_created_main_workspace(&self, workspace_id: Uuid) -> Result<(), ApiError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentNodeRequest {
    pub run_id: Uuid,
    pub orchestration_run_id: Uuid,
    pub orchestration_node_execution_id: Uuid,
    pub iteration: i64,
    pub node_id: String,
    pub session_id: Option<Uuid>,
    pub workspace_id: Uuid,
    pub prompt: String,
    pub selected_skills: Option<Vec<SelectedSkill>>,
    pub executor_config: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentNodeExecution {
    Completed {
        session_id: Uuid,
        orchestration_node_execution_id: Uuid,
        agent_run_id: Uuid,
        output_text: String,
    },
    Started {
        session_id: Uuid,
        orchestration_node_execution_id: Uuid,
        agent_run_id: Uuid,
        output_text: Option<String>,
    },
}

#[async_trait]
pub trait WorkflowAgentExecutor: Send + Sync {
    async fn run_agent(&self, request: AgentNodeRequest) -> Result<AgentNodeExecution, ApiError>;
}

#[async_trait]
pub trait WorkflowRunCanceller: Send + Sync {
    async fn cancel_session(&self, session_id: Uuid) -> Result<(), ApiError>;

    async fn cancel_orchestration_run(
        &self,
        _pool: &SqlitePool,
        _orchestration_run_id: Uuid,
    ) -> Result<(), ApiError> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct DeploymentWorkflowRunCanceller {
    deployment: DeploymentImpl,
}

impl DeploymentWorkflowRunCanceller {
    pub fn new(deployment: DeploymentImpl) -> Self {
        Self { deployment }
    }
}

#[async_trait]
impl WorkflowRunCanceller for DeploymentWorkflowRunCanceller {
    async fn cancel_session(&self, session_id: Uuid) -> Result<(), ApiError> {
        let orchestration_run_ids: Vec<Uuid> = sqlx::query_scalar(
            r#"
            SELECT DISTINCT wr.orchestration_run_id
            FROM node_executions ne
            JOIN workflow_runs wr ON wr.id = ne.run_id
            JOIN orchestration_node_executions orchestration_nodes
              ON orchestration_nodes.id = ne.orchestration_node_execution_id
             AND orchestration_nodes.orchestration_run_id = wr.orchestration_run_id
            JOIN orchestration_agent_run_links links
              ON links.node_execution_id = orchestration_nodes.id
             AND links.orchestration_run_id = wr.orchestration_run_id
            JOIN agent_run_state agent_state
              ON agent_state.agent_run_id = links.agent_run_id
            WHERE ne.session_id = ?
              AND wr.orchestration_run_id IS NOT NULL
              AND (
                  ne.status IN ('running', 'awaiting_human', 'awaiting_arena', 'cancelling')
                  OR agent_state.status IN (
                      'pending', 'starting', 'running',
                      'awaiting_input', 'awaiting_approval', 'cancelling'
                  )
              )
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.deployment.db().pool)
        .await?;
        let mut first_error = None;
        for orchestration_run_id in orchestration_run_ids {
            if let Err(error) = self
                .cancel_orchestration_run(&self.deployment.db().pool, orchestration_run_id)
                .await
            {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn cancel_orchestration_run(
        &self,
        pool: &SqlitePool,
        orchestration_run_id: Uuid,
    ) -> Result<(), ApiError> {
        let mut first_error = crate::workflow_runtime::handover::cancel_for_run(
            &self.deployment,
            orchestration_run_id,
        )
        .await
        .err();
        let service = OrchestrationService::new(
            pool.clone(),
            Arc::new(self.deployment.agent_run_port().clone()),
        );
        if let Err(error) = service
            .cancel(orchestration_run_id, orchestration_run_id)
            .await
            .map_err(orchestration_api_error)
        {
            first_error.get_or_insert(error);
        }
        // A terminal orchestration deliberately retains its outcome. It may
        // still own a parallel child, which needs the same durable cancel key.
        let children: Vec<(Uuid, Uuid)> = sqlx::query_as("SELECT l.node_execution_id,l.agent_run_id FROM orchestration_agent_run_links l LEFT JOIN agent_run_state s ON s.agent_run_id=l.agent_run_id WHERE l.orchestration_run_id=? AND (s.agent_run_id IS NULL OR s.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed'))")
            .bind(orchestration_run_id).fetch_all(pool).await?;
        for (node_execution_id, agent_run_id) in children {
            let mut digest = Sha256::new();
            digest.update(orchestration_run_id.as_bytes());
            digest.update(node_execution_id.as_bytes());
            digest.update(b"cancel");
            let bytes: [u8; 16] = digest.finalize()[..16].try_into().expect("SHA-256 prefix");
            if let Err(error) = service
                .enqueue_command(AgentRunPortCommandEnvelope {
                    schema_version: ORCHESTRATION_COMMAND_SCHEMA_VERSION,
                    command_id: Uuid::from_bytes(bytes),
                    idempotency_key: format!(
                        "orchestration:{orchestration_run_id}:node:{node_execution_id}:cancel"
                    ),
                    agent_run_id,
                    orchestration_run_id: Some(orchestration_run_id),
                    orchestration_node_execution_id: Some(node_execution_id),
                    correlation_id: orchestration_run_id,
                    created_at: Utc::now(),
                    command: AgentRunPortCommand::Cancel {
                        reason: "parent workflow stop requested".into(),
                    },
                })
                .await
                .map_err(orchestration_api_error)
            {
                first_error.get_or_insert(error);
            }
        }
        // A failed delivery is delayed by the outbox. Bound this drain to the
        // accepted commands so one unreachable child cannot starve peer stops
        // or keep this HTTP request retrying forever.
        let deliveries: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM orchestration_outbox WHERE delivery_status IN ('pending','failed')",
        ).fetch_one(pool).await?;
        for _ in 0..deliveries {
            match service
                .deliver_next()
                .await
                .map_err(orchestration_api_error)
            {
                Ok(false) => break,
                Ok(true) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        let terminal_attempts: Vec<(Uuid, Uuid)> = sqlx::query_as("SELECT a.agent_run_id,a.id FROM orchestration_agent_run_links l JOIN agent_run_attempts a ON a.agent_run_id=l.agent_run_id WHERE l.orchestration_run_id=? AND a.status IN ('succeeded','failed','cancelled','crashed','audit_failed')")
            .bind(orchestration_run_id).fetch_all(pool).await?;
        for (agent_run_id, attempt_id) in terminal_attempts {
            let port = self.deployment.agent_run_port();
            match port
                .reconcile_terminal_process(attempt_id)
                .await
                .map_err(orchestration_api_error)
            {
                Ok(true) => {}
                Ok(false) => {
                    if let Err(error) = port
                        .stop_terminal_process_for_deletion(agent_run_id, attempt_id)
                        .await
                        .map_err(orchestration_api_error)
                    {
                        first_error.get_or_insert(error);
                    }
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

/// Canonical startup/restart reconciliation boundary for managed AgentRuns.
///
/// Workflow recovery must not infer a provider run's terminal state from the
/// legacy execution-process projection. Deployments with a provider-backed
/// AgentRun/orchestration implementation should inject it through
/// [`recover_stale_workflow_runs_with_boundary`].
#[async_trait]
pub trait AgentRunReconciliationBoundary: Send + Sync {
    async fn reconcile_workflow_run(
        &self,
        pool: &SqlitePool,
        run_id: Uuid,
    ) -> Result<AgentRunReconciliationResult, ApiError>;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AgentRunReconciliationResult {
    pub completed: bool,
    pub failed: bool,
    pub awaiting_human: bool,
    pub cancelled: bool,
}

/// Explicit adapter seam used until the deployment wires a provider-backed
/// AgentRunPort. Preserving the durable run is the only safe result when no
/// runtime authority is available; this adapter never marks a run succeeded,
/// failed, or cancelled.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnconfiguredAgentRunReconciliationBoundary;

#[async_trait]
impl AgentRunReconciliationBoundary for UnconfiguredAgentRunReconciliationBoundary {
    async fn reconcile_workflow_run(
        &self,
        _pool: &SqlitePool,
        run_id: Uuid,
    ) -> Result<AgentRunReconciliationResult, ApiError> {
        tracing::warn!(
            workflow_run_id = %run_id,
            "AgentRun reconciliation boundary is not configured; preserving durable workflow state"
        );
        Ok(AgentRunReconciliationResult::default())
    }
}

/// Deployment-backed canonical reconciliation for Workflow Agent nodes.
///
/// This boundary deliberately joins Workflow nodes to durable orchestration
/// links and asks the AgentRun port for canonical `RunState`. Provider process
/// ids, legacy turns, and normalized logs are not part of the decision path.
#[derive(Clone)]
pub struct DeploymentAgentRunReconciliationBoundary {
    deployment: DeploymentImpl,
}

impl DeploymentAgentRunReconciliationBoundary {
    pub fn new(deployment: DeploymentImpl) -> Self {
        Self { deployment }
    }
}

#[async_trait]
impl AgentRunReconciliationBoundary for DeploymentAgentRunReconciliationBoundary {
    async fn reconcile_workflow_run(
        &self,
        pool: &SqlitePool,
        run_id: Uuid,
    ) -> Result<AgentRunReconciliationResult, ApiError> {
        let Some(orchestration_run_id) = sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT orchestration_run_id FROM workflow_runs WHERE id = ?",
        )
        .bind(run_id)
        .fetch_optional(pool)
        .await?
        .flatten() else {
            return Ok(AgentRunReconciliationResult::default());
        };

        let service = OrchestrationService::new(
            pool.clone(),
            Arc::new(self.deployment.agent_run_port().clone()),
        );
        service
            .reconcile_run(orchestration_run_id, orchestration_run_id)
            .await
            .map_err(orchestration_api_error)?;
        let arena_runs: Vec<Uuid> = sqlx::query_scalar("SELECT DISTINCT o.id FROM orchestration_runs o JOIN node_executions n ON n.arena_group_id=o.source_definition_id WHERE n.run_id=? AND o.product_kind='arena'")
            .bind(run_id).fetch_all(pool).await?;
        for arena_run in arena_runs {
            service
                .reconcile_run(arena_run, arena_run)
                .await
                .map_err(orchestration_api_error)?;
        }
        let terminal_attempts:Vec<Uuid>=sqlx::query_scalar("WITH owned_runs(id) AS (SELECT ?1 UNION SELECT o.id FROM orchestration_runs o JOIN node_executions n ON n.arena_group_id=o.source_definition_id WHERE n.run_id=?1 AND o.product_kind='arena') SELECT a.id FROM orchestration_agent_run_links l JOIN agent_run_attempts a ON a.agent_run_id=l.agent_run_id WHERE l.orchestration_run_id IN(SELECT id FROM owned_runs) AND a.status IN ('succeeded','failed','cancelled','crashed','audit_failed')")
            .bind(run_id).fetch_all(pool).await?;
        for attempt_id in terminal_attempts {
            // Observe exit independently of the child's immutable outcome. A
            // false observation keeps its slot; only explicit stop sends control.
            let _ = self
                .deployment
                .agent_run_port()
                .reconcile_terminal_process(attempt_id)
                .await
                .map_err(orchestration_api_error)?;
        }

        let runtime_run = load_runtime_run(pool, run_id).await?;
        let rows = sqlx::query(
            r#"
            SELECT ne.id, ne.node_id, ne.node_type, ne.iteration, ne.status,
                   ne.output_text, ne.session_id,
                   orchestration_nodes.id AS orchestration_node_execution_id,
                   links.agent_run_id,
                   agent_state.status AS agent_run_status
            FROM node_executions ne
            JOIN orchestration_node_executions orchestration_nodes
              ON orchestration_nodes.id = ne.orchestration_node_execution_id
            JOIN orchestration_agent_run_links links
              ON links.node_execution_id = orchestration_nodes.id
             AND links.orchestration_run_id = orchestration_nodes.orchestration_run_id
            JOIN agent_run_state agent_state
              ON agent_state.agent_run_id = links.agent_run_id
            WHERE ne.run_id = ?
              AND orchestration_nodes.orchestration_run_id = ?
              AND (
                  ne.status IN ('running', 'awaiting_human', 'awaiting_arena', 'cancelling')
                  OR agent_state.status IN (
                      'pending', 'starting', 'running',
                      'awaiting_input', 'awaiting_approval', 'cancelling'
                  )
              )
            ORDER BY ne.rowid
            "#,
        )
        .bind(run_id)
        .bind(orchestration_run_id)
        .fetch_all(pool)
        .await?;

        // A workflow may have several AgentRuns active at once (for example,
        // an `all`/parallel fan-out). Seeing one child succeed is not enough
        // to advance the parent: downstream joins must wait until every
        // active child reaches a terminal state. Keep this separate from the
        // product-level outcome flags so one success cannot race a sibling.
        let mut outcome = AgentRunReconciliationResult::default();
        let mut all_active_agent_runs_terminal = !rows.is_empty();
        for row in rows {
            let execution_id: Uuid = row.try_get("id")?;
            let node_id: String = row.try_get("node_id")?;
            let node_type: String = row.try_get("node_type")?;
            let iteration: i64 = row.try_get("iteration")?;
            let current_status: String = row.try_get("status")?;
            let started_output_text: Option<String> = row.try_get("output_text")?;
            let session_id: Option<Uuid> = row.try_get("session_id")?;
            let orchestration_node_execution_id: Uuid =
                row.try_get("orchestration_node_execution_id")?;
            let agent_run_id: Uuid = row.try_get("agent_run_id")?;
            let state = match service.query_agent_run(agent_run_id).await {
                Ok(state) => state,
                Err(OrchestrationServiceError::Port(AgentRunPortError::Unavailable(_)))
                | Err(OrchestrationServiceError::Port(AgentRunPortError::NotFound(_))) => {
                    // An unavailable transport is not a terminal fact. Keep
                    // the durable Workflow node associated with the same run.
                    all_active_agent_runs_terminal = false;
                    continue;
                }
                Err(error) => return Err(orchestration_api_error(error)),
            };
            let terminal_output = state
                .terminal_output
                .as_ref()
                .map(|message| message.content.as_str());

            match state.status {
                AgentRunStatus::Succeeded => {
                    if node_type == node_kind_value(&WorkflowNodeKind::Condition) {
                        let completion = complete_condition_router(
                            pool,
                            run_id,
                            &runtime_run.graph,
                            runtime_run.workspace_id,
                            &node_id,
                            iteration,
                            orchestration_node_execution_id,
                            agent_run_id,
                            started_output_text.as_deref(),
                            terminal_output.unwrap_or_default(),
                        )
                        .await?;
                        if completion.should_pause() {
                            outcome.awaiting_human = true;
                        }
                    } else {
                        let delivery =
                            if node_type == node_kind_value(&WorkflowNodeKind::TaskSplitter) {
                                let node = runtime_run
                                    .graph
                                    .nodes
                                    .iter()
                                    .find(|node| node.id == node_id)
                                    .ok_or_else(|| {
                                        ApiError::Conflict("Frozen splitter is missing".into())
                                    })?;
                                accept_split_manifest(
                                    pool,
                                    run_id,
                                    &runtime_run.graph,
                                    node,
                                    iteration,
                                    runtime_run.workspace_id,
                                )
                                .await
                            } else {
                                file_handoff::accept(pool, run_id, execution_id, true)
                                    .await
                                    .map(|_| ())
                            };
                        if let Err(error) = delivery {
                            mark_canonical_node_failed(
                                pool,
                                run_id,
                                &node_id,
                                iteration,
                                session_id,
                                orchestration_node_execution_id,
                                agent_run_id,
                                &error.to_string(),
                            )
                            .await?;
                            outcome.failed = true;
                            continue;
                        }
                        mark_canonical_node_succeeded(
                            pool,
                            run_id,
                            &node_id,
                            iteration,
                            terminal_output,
                            session_id,
                            orchestration_node_execution_id,
                            agent_run_id,
                        )
                        .await?;
                    }
                }
                AgentRunStatus::AwaitingInput | AgentRunStatus::AwaitingApproval => {
                    if current_status != node_status_value(DbNodeExecutionStatus::AwaitingHuman) {
                        update_node_execution(
                            pool,
                            run_id,
                            &node_id,
                            iteration,
                            NodeExecutionUpdate {
                                status: DbNodeExecutionStatus::AwaitingHuman,
                                input_text: None,
                                output_text: None,
                                session_id,
                                orchestration_node_execution_id: Some(
                                    orchestration_node_execution_id,
                                ),
                                agent_run_id: Some(agent_run_id),
                                arena_group_id: None,
                                error_text: None,
                                finished: false,
                            },
                        )
                        .await?;
                    }
                    outcome.awaiting_human = true;
                }
                AgentRunStatus::Failed
                | AgentRunStatus::Cancelled
                | AgentRunStatus::Crashed
                | AgentRunStatus::AuditFailed => {
                    let fallback = match state.status {
                        AgentRunStatus::Cancelled => "Workflow AgentRun was cancelled",
                        AgentRunStatus::Crashed => "Workflow AgentRun crashed",
                        AgentRunStatus::AuditFailed => "Workflow AgentRun audit failed",
                        _ => "Workflow AgentRun failed",
                    };
                    let error_text = state
                        .last_error
                        .as_ref()
                        .map(|error| error.message.as_str())
                        .unwrap_or(fallback);
                    if state.status == AgentRunStatus::Cancelled {
                        update_node_execution(
                            pool,
                            run_id,
                            &node_id,
                            iteration,
                            NodeExecutionUpdate {
                                status: DbNodeExecutionStatus::Cancelled,
                                input_text: None,
                                output_text: None,
                                session_id,
                                orchestration_node_execution_id: Some(
                                    orchestration_node_execution_id,
                                ),
                                agent_run_id: Some(agent_run_id),
                                arena_group_id: None,
                                error_text: Some(error_text),
                                finished: true,
                            },
                        )
                        .await?;
                        outcome.cancelled = true;
                    } else {
                        mark_canonical_node_failed(
                            pool,
                            run_id,
                            &node_id,
                            iteration,
                            session_id,
                            orchestration_node_execution_id,
                            agent_run_id,
                            error_text,
                        )
                        .await?;
                        outcome.failed = true;
                    }
                }
                AgentRunStatus::Pending
                | AgentRunStatus::Starting
                | AgentRunStatus::Running
                | AgentRunStatus::Cancelling => {
                    all_active_agent_runs_terminal = false;
                }
            }
        }

        // Only a fully terminal fan-out may advance the workflow. Failed or
        // cancelled children retain their existing fail/cancel precedence;
        // a paused condition router likewise blocks advancement.
        if all_active_agent_runs_terminal
            && !outcome.awaiting_human
            && !outcome.failed
            && !outcome.cancelled
        {
            outcome.completed = true;
        }

        Ok(outcome)
    }
}

static WORKFLOW_EVENT_HUB: LazyLock<WorkflowRuntimeEventHub> =
    LazyLock::new(WorkflowRuntimeEventHub::new);

const MAX_WORKFLOW_EVENT_HISTORY: usize = 4096;

#[derive(Debug)]
struct WorkflowRuntimeEventHub {
    inner: Mutex<WorkflowRuntimeEventHubInner>,
    sender: broadcast::Sender<WorkflowEvent>,
}

#[derive(Debug, Default)]
struct WorkflowRuntimeEventHubInner {
    next_sequence: u64,
    events: Vec<WorkflowEvent>,
}

impl WorkflowRuntimeEventHub {
    fn new() -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self {
            inner: Mutex::new(WorkflowRuntimeEventHubInner::default()),
            sender,
        }
    }

    fn publish(&self, mut event: WorkflowEvent) -> WorkflowEvent {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.next_sequence += 1;
        event.sequence = inner.next_sequence;
        inner.events.push(event.clone());
        if inner.events.len() > MAX_WORKFLOW_EVENT_HISTORY {
            let excess = inner.events.len() - MAX_WORKFLOW_EVENT_HISTORY;
            inner.events.drain(0..excess);
        }
        let _ = self.sender.send(event.clone());
        event
    }

    fn history(&self, run_id: Uuid) -> Vec<WorkflowEvent> {
        let run_id = run_id.to_string();
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .events
            .iter()
            .filter(|event| event.run_id == run_id)
            .cloned()
            .collect()
    }

    fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.sender.subscribe()
    }
}

pub fn workflow_event_history(run_id: Uuid) -> Vec<WorkflowEvent> {
    WORKFLOW_EVENT_HUB.history(run_id)
}

pub fn subscribe_workflow_events() -> broadcast::Receiver<WorkflowEvent> {
    WORKFLOW_EVENT_HUB.subscribe()
}

#[derive(Clone)]
pub struct DeploymentWorkflowAgentExecutor {
    deployment: DeploymentImpl,
}

impl DeploymentWorkflowAgentExecutor {
    pub fn new(deployment: DeploymentImpl) -> Self {
        Self { deployment }
    }
}

fn workflow_direct_folder_agent_path(root: &str, agent_working_dir: Option<&str>) -> String {
    match agent_working_dir.filter(|directory| !directory.is_empty()) {
        Some(directory) => PathBuf::from(root)
            .join(directory)
            .to_string_lossy()
            .into_owned(),
        None => root.to_string(),
    }
}

#[async_trait]
impl WorkflowAgentExecutor for DeploymentWorkflowAgentExecutor {
    async fn run_agent(&self, request: AgentNodeRequest) -> Result<AgentNodeExecution, ApiError> {
        agent_launch::run(&self.deployment, request).await
    }
}

pub async fn trigger_workflow_run<W, A>(
    pool: &SqlitePool,
    workflow_id: Uuid,
    request: TriggerWorkflowRequest,
    workspace_resolver: &W,
    agent_executor: &A,
) -> Result<WorkflowRunResponse, ApiError>
where
    W: WorkflowWorkspaceResolver,
    A: WorkflowAgentExecutor,
{
    let arena_creator = NoopWorkflowArenaCreator;
    trigger_workflow_run_with_arena(
        pool,
        workflow_id,
        request,
        workspace_resolver,
        agent_executor,
        &arena_creator,
    )
    .await
}

pub async fn trigger_workflow_run_with_arena<W, A, R>(
    pool: &SqlitePool,
    workflow_id: Uuid,
    request: TriggerWorkflowRequest,
    workspace_resolver: &W,
    agent_executor: &A,
    arena_creator: &R,
) -> Result<WorkflowRunResponse, ApiError>
where
    W: WorkflowWorkspaceResolver,
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
{
    trigger_workflow_run_for_attempt_with_arena(
        pool,
        workflow_id,
        None,
        request,
        workspace_resolver,
        agent_executor,
        arena_creator,
    )
    .await
}

pub async fn trigger_workflow_run_for_attempt_with_arena<W, A, R>(
    pool: &SqlitePool,
    workflow_id: Uuid,
    attempt_id: Option<Uuid>,
    request: TriggerWorkflowRequest,
    workspace_resolver: &W,
    agent_executor: &A,
    arena_creator: &R,
) -> Result<WorkflowRunResponse, ApiError>
where
    W: WorkflowWorkspaceResolver,
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
{
    trigger_workflow_run_for_attempt_with_repos(
        pool,
        WorkflowRunStartRequest {
            workflow_id,
            attempt_id,
            trigger: request,
            directory_path: None,
            repo_overrides: Vec::new(),
        },
        workspace_resolver,
        agent_executor,
        arena_creator,
    )
    .await
}

pub async fn trigger_workflow_run_for_attempt_with_repos<W, A, R>(
    pool: &SqlitePool,
    request: WorkflowRunStartRequest,
    workspace_resolver: &W,
    agent_executor: &A,
    arena_creator: &R,
) -> Result<WorkflowRunResponse, ApiError>
where
    W: WorkflowWorkspaceResolver,
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
{
    let WorkflowRunStartRequest {
        workflow_id,
        attempt_id,
        trigger,
        directory_path,
        repo_overrides,
    } = request;
    let workflow = get_workflow_template(pool, workflow_id).await?;
    let mut graph: WorkflowGraph = serde_json::from_str(&workflow.graph_json)
        .map_err(|err| ApiError::BadRequest(format!("Invalid workflow graph JSON: {err}")))?;
    validate_graph_for_run(&graph)
        .map_err(|err| ApiError::BadRequest(format!("Invalid workflow graph: {err}")))?;

    let run_id = Uuid::new_v4();
    let project_id = workflow
        .project_id
        .or(resolve_issue_project_id(pool, trigger.issue_id).await?);
    let workspace_id = workspace_resolver
        .create_or_bind_main_workspace(WorkflowWorkspaceRequest {
            issue_id: trigger.issue_id,
            run_id,
            project_id,
            existing_workspace_id: trigger.workspace_id,
            directory_path,
            repo_overrides,
            branch_name: main_workflow_branch_name(trigger.issue_id, run_id),
        })
        .await?;

    if ensure_agent_node_sessions(pool, workspace_id, &mut graph).await? {
        persist_workflow_graph(pool, workflow_id, workflow.revision, &graph)
            .await
            .map_err(ApiError::from)?;
    }

    initialize_workflow_run(
        pool,
        run_id,
        workflow_id,
        attempt_id,
        workspace_id,
        &trigger,
        &graph,
        None,
    )
    .await?;
    let orchestration_run_id =
        start_workflow_orchestration(pool, run_id, workflow_id, workspace_id, &graph).await?;
    sqlx::query("UPDATE workflow_runs SET orchestration_run_id = ? WHERE id = ?")
        .bind(orchestration_run_id)
        .bind(run_id)
        .execute(pool)
        .await?;
    link_workflow_node_execution_identities(pool, run_id, orchestration_run_id).await?;
    drive_workflow_run(
        pool,
        run_id,
        &graph,
        trigger.issue_id,
        workspace_id,
        &trigger.input_text,
        agent_executor,
        arena_creator,
    )
    .await?;

    get_workflow_run_response(pool, run_id).await
}

async fn start_workflow_orchestration(
    pool: &SqlitePool,
    workflow_run_id: Uuid,
    workflow_id: Uuid,
    workspace_id: Uuid,
    graph: &WorkflowGraph,
) -> Result<Uuid, ApiError> {
    let workspace = Workspace::find_by_id(pool, workspace_id)
        .await?
        .ok_or(ApiError::Workspace(WorkspaceError::WorkspaceNotFound))?;
    let workspace_mode = match workspace.workspace_kind {
        WorkspaceKind::DirectFolder => WorkspaceMode::SharedWorkspace,
        WorkspaceKind::Worktree => WorkspaceMode::IsolatedWorktree,
    };
    let plan = OrchestrationPlanSnapshot {
        schema_version: ORCHESTRATION_PLAN_SCHEMA_VERSION,
        plan_id: stable_workflow_identity(workflow_run_id, workflow_id, 0, "plan"),
        source_definition_id: workflow_id,
        source_definition_version: workflow_graph_snapshot_version(graph)?,
        product_kind: OrchestrationProductKind::Workflow,
        workspace_mode,
        nodes: graph
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let executor_config =
                    node.data.executor_config.as_ref().and_then(|value| {
                        serde_json::from_value::<ExecutorConfig>(value.clone()).ok()
                    });
                OrchestrationPlanNode {
                    node_key: node.id.clone(),
                    stable_order: u32::try_from(index).unwrap_or(u32::MAX),
                    dependencies: graph
                        .edges
                        .iter()
                        .filter(|edge| edge.target == node.id)
                        .map(|edge| edge.source.clone())
                        .collect(),
                    join: OrchestrationJoinPolicy::All,
                    // A failed split item must not cancel its independent
                    // siblings through the canonical orchestration projection.
                    failure_policy: if batch_managed_nodes(graph).contains(&node.id) {
                        OrchestrationFailurePolicy::AllowPartial
                    } else {
                        OrchestrationFailurePolicy::FailFast
                    },
                    remaining_upstreams: RemainingUpstreamsPolicy::Continue,
                    each_downstream_execution: EachDownstreamExecution::Parallel,
                    retry: OrchestrationRetryPolicy::default(),
                    runtime_profile_id: executor_config
                        .as_ref()
                        .map(|config| config.profile_id().cache_key()),
                    provider_id: executor_config
                        .and_then(|config| direct_provider_for_agent(config.executor).ok())
                        .map(|provider| provider.id().to_string()),
                    provider_config: node.data.executor_config.clone(),
                }
            })
            .collect(),
        created_at: Utc::now(),
    };
    let service = OrchestrationService::new(pool.clone(), Arc::new(NoopAgentRunPort));
    service
        .start_run(
            workflow_run_id,
            stable_workflow_identity(workflow_run_id, workflow_id, 0, "start-request"),
            &format!("workflow:{workflow_run_id}:start"),
            workflow_run_id,
            &plan,
        )
        .await
        .map_err(orchestration_api_error)
}

/// Persist the immutable run and queue identity before any Agent is dispatched.
/// The caller may supply an idempotency-reserved ID; retries never allocate a
/// second run. Workspace/session setup is ordinary configuration, not execution.
pub async fn accept_workflow_run<W: WorkflowWorkspaceResolver>(
    pool: &SqlitePool,
    run_id: Uuid,
    request: WorkflowRunStartRequest,
    resolver: &W,
) -> Result<WorkflowRunResponse, ApiError> {
    static ACCEPT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _accept_guard = ACCEPT_LOCK.lock().await;
    if sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_runs WHERE id = ?")
        .bind(run_id)
        .fetch_one(pool)
        .await?
        > 0
    {
        return get_workflow_run_response(pool, run_id).await;
    }
    let project_id = resolve_issue_project_id(pool, request.trigger.issue_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Task not found".to_string()))?;
    let workflow = get_workflow_template(pool, request.workflow_id).await?;
    if workflow.project_id.is_some_and(|id| id != project_id) {
        return Err(ApiError::BadRequest(
            "Workflow belongs to another project".to_string(),
        ));
    }
    let mut graph: WorkflowGraph = serde_json::from_str(&workflow.graph_json)
        .map_err(|e| ApiError::BadRequest(format!("Invalid workflow graph: {e}")))?;
    validate_graph_for_run(&graph).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let workspace_id = resolver
        .create_or_bind_main_workspace(WorkflowWorkspaceRequest {
            issue_id: request.trigger.issue_id,
            run_id,
            project_id: Some(project_id),
            existing_workspace_id: request.trigger.workspace_id,
            directory_path: request.directory_path,
            repo_overrides: request.repo_overrides,
            branch_name: main_workflow_branch_name(request.trigger.issue_id, run_id),
        })
        .await?;
    if ensure_agent_node_sessions(pool, workspace_id, &mut graph).await? {
        persist_workflow_graph(pool, request.workflow_id, workflow.revision, &graph)
            .await
            .map_err(ApiError::from)?;
    }
    initialize_workflow_run(
        pool,
        run_id,
        request.workflow_id,
        request.attempt_id,
        workspace_id,
        &request.trigger,
        &graph,
        Some(project_id),
    )
    .await?;
    let run = get_workflow_run_response(pool, run_id).await?;
    crate::routes::workflows::sync_attempt_from_run(pool, &run).await?;
    Ok(run)
}

/// Continue an already persisted queue item. Only the dispatcher holding its
/// project slot may call this entry. The graph always comes from the snapshot.
pub async fn start_accepted_workflow_run<A: WorkflowAgentExecutor, R: WorkflowArenaCreator>(
    pool: &SqlitePool,
    run_id: Uuid,
    executor: &A,
    arena: &R,
) -> Result<(), ApiError> {
    let changed = sqlx::query("UPDATE workflow_runs SET status='running', started_at=COALESCE(started_at,datetime('now','subsec')), updated_at=datetime('now','subsec') WHERE id=? AND status='pending' AND EXISTS (SELECT 1 FROM workflow_project_slots WHERE run_id=?)")
        .bind(run_id).bind(run_id).execute(pool).await?.rows_affected();
    if changed == 0 {
        return Ok(());
    }
    let runtime = load_runtime_run(pool, run_id).await?;
    let workflow_id: Uuid = sqlx::query_scalar("SELECT workflow_id FROM workflow_runs WHERE id=?")
        .bind(run_id)
        .fetch_one(pool)
        .await?;
    let orchestration_id = start_workflow_orchestration(
        pool,
        run_id,
        workflow_id,
        runtime.workspace_id,
        &runtime.graph,
    )
    .await?;
    sqlx::query("UPDATE workflow_runs SET orchestration_run_id=? WHERE id=?")
        .bind(orchestration_id)
        .bind(run_id)
        .execute(pool)
        .await?;
    link_workflow_node_execution_identities(pool, run_id, orchestration_id).await?;
    sqlx::query("UPDATE workflow_run_queue SET phase='active' WHERE run_id=?")
        .bind(run_id)
        .execute(pool)
        .await?;
    emit_run_status(run_id, WorkflowRunStatus::Running, None, None);
    drive_workflow_run(
        pool,
        run_id,
        &runtime.graph,
        runtime.issue_id,
        runtime.workspace_id,
        &runtime.input_text,
        executor,
        arena,
    )
    .await
}

pub async fn fail_accepted_workflow_run(
    pool: &SqlitePool,
    run_id: Uuid,
    error: &str,
) -> Result<(), ApiError> {
    let lock = workflow_run_lock(run_id);
    let _guard = lock.lock().await;
    settle_workflow_failure(pool, run_id, error).await
}

pub async fn recover_accepted_workflow_start<A: WorkflowAgentExecutor, R: WorkflowArenaCreator>(
    pool: &SqlitePool,
    run_id: Uuid,
    executor: &A,
    arena: &R,
) -> Result<(), ApiError> {
    let failure: Option<String> =
        sqlx::query_scalar("SELECT error_text FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    if let Some(failure) = failure {
        return fail_accepted_workflow_run(pool, run_id, &failure).await;
    }
    link_workflow_node_execution_identities(pool, run_id, run_id).await?;
    // Winner application can write the workspace. Never blindly repeat it
    // after a crash; expose the interrupted decision as a failed run.
    let interrupted = sqlx::query("UPDATE node_executions SET status='failed', error_text='Arena winner application was interrupted; inspect the project files before a new run', finished_at=datetime('now','subsec') WHERE run_id=? AND status='awaiting_arena' AND id IN (SELECT node_execution_id FROM workflow_interaction_responses WHERE json_extract(response_json,'$.action')='arena_winner')")
        .bind(run_id).execute(pool).await?.rows_affected();
    if interrupted > 0 {
        tracing::warn!(%run_id, "Interrupted Arena winner retained as failed; waiting for remaining workflow children");
    }
    let run = load_runtime_run(pool, run_id).await?;
    drive_workflow_run(
        pool,
        run_id,
        &run.graph,
        run.issue_id,
        run.workspace_id,
        &run.input_text,
        executor,
        arena,
    )
    .await?;
    Ok(())
}

async fn link_workflow_node_execution_identities(
    pool: &SqlitePool,
    workflow_run_id: Uuid,
    orchestration_run_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE node_executions
        SET orchestration_node_execution_id = (
            SELECT orchestration_nodes.id
            FROM orchestration_node_executions orchestration_nodes
            WHERE orchestration_nodes.orchestration_run_id = ?
              AND orchestration_nodes.node_key = node_executions.node_id
              AND orchestration_nodes.iteration = node_executions.iteration
        )
        WHERE run_id = ?
        "#,
    )
    .bind(orchestration_run_id)
    .bind(workflow_run_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct NoopAgentRunPort;

#[async_trait]
impl executors::runtime::AgentRunPort for NoopAgentRunPort {
    async fn create(
        &self,
        _request: AgentRunRequestEnvelope,
        _attempt: RunAttemptRequest,
    ) -> Result<Uuid, executors::runtime::AgentRunPortError> {
        Err(executors::runtime::AgentRunPortError::Unavailable(
            "plan compiler does not dispatch AgentRuns".to_string(),
        ))
    }

    async fn query(
        &self,
        agent_run_id: Uuid,
    ) -> Result<executors::runtime::AgentRunPortSnapshot, executors::runtime::AgentRunPortError>
    {
        Err(executors::runtime::AgentRunPortError::NotFound(
            agent_run_id,
        ))
    }

    async fn control(
        &self,
        _command: AgentRunPortCommandEnvelope,
    ) -> Result<(), executors::runtime::AgentRunPortError> {
        Err(executors::runtime::AgentRunPortError::Unavailable(
            "plan compiler does not control AgentRuns".to_string(),
        ))
    }

    async fn subscribe(
        &self,
        agent_run_id: Uuid,
    ) -> Result<executors::runtime::AgentEventStream, executors::runtime::AgentRunPortError> {
        Err(executors::runtime::AgentRunPortError::NotFound(
            agent_run_id,
        ))
    }
}

fn workflow_graph_snapshot_version(graph: &WorkflowGraph) -> Result<String, ApiError> {
    let bytes = serde_json::to_vec(graph).map_err(|error| {
        ApiError::BadRequest(format!("Cannot snapshot workflow graph: {error}"))
    })?;
    let digest = Sha256::digest(bytes);
    Ok(format!("sha256:{digest:x}"))
}

fn stable_workflow_identity(
    orchestration_run_id: Uuid,
    node_execution_id: Uuid,
    iteration: i64,
    purpose: &str,
) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(orchestration_run_id.as_bytes());
    hasher.update(node_execution_id.as_bytes());
    hasher.update(iteration.to_le_bytes());
    hasher.update(purpose.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

fn stable_orchestration_node_execution_identity(
    orchestration_run_id: Uuid,
    node_id: &str,
    iteration: u32,
) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(orchestration_run_id.as_bytes());
    hasher.update(node_id.as_bytes());
    hasher.update(iteration.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

async fn canonical_node_identity(
    pool: &SqlitePool,
    workflow_run_id: Uuid,
    node_id: &str,
    iteration: i64,
) -> Result<(Uuid, Uuid), ApiError> {
    let orchestration_run_id: Uuid =
        sqlx::query_scalar("SELECT orchestration_run_id FROM workflow_runs WHERE id = ?")
            .bind(workflow_run_id)
            .fetch_optional(pool)
            .await?
            .flatten()
            .ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "Workflow run {workflow_run_id} has no canonical orchestration identity"
                ))
            })?;
    let iteration = u32::try_from(iteration).map_err(|_| {
        ApiError::BadRequest(format!(
            "Workflow node `{node_id}` has invalid iteration {iteration}"
        ))
    })?;
    let stable_order: i64 = sqlx::query_scalar(
        r#"
        SELECT stable_order
        FROM orchestration_node_executions
        WHERE orchestration_run_id = ? AND node_key = ?
        ORDER BY iteration
        LIMIT 1
        "#,
    )
    .bind(orchestration_run_id)
    .bind(node_id)
    .fetch_one(pool)
    .await?;
    let stable_order = u32::try_from(stable_order).map_err(|_| {
        ApiError::BadRequest(format!(
            "Workflow node `{node_id}` has invalid canonical stable order {stable_order}"
        ))
    })?;
    let proposed_id =
        stable_orchestration_node_execution_identity(orchestration_run_id, node_id, iteration);
    let orchestration_node_execution_id =
        OrchestrationNodeExecutionRecord::persist_identity_before_dispatch(
            pool,
            proposed_id,
            orchestration_run_id,
            node_id,
            iteration,
            stable_order,
        )
        .await
        .map_err(orchestration_api_error)?;
    sqlx::query(
        r#"
        UPDATE node_executions
        SET orchestration_node_execution_id = ?,
            updated_at = datetime('now', 'subsec')
        WHERE run_id = ? AND node_id = ? AND iteration = ?
        "#,
    )
    .bind(orchestration_node_execution_id)
    .bind(workflow_run_id)
    .bind(node_id)
    .bind(i64::from(iteration))
    .execute(pool)
    .await?;

    Ok((orchestration_run_id, orchestration_node_execution_id))
}

fn direct_provider_for_agent(agent: BaseCodingAgent) -> Result<DirectProvider, ApiError> {
    match agent {
        BaseCodingAgent::Gemini => Ok(DirectProvider::Gemini),
        BaseCodingAgent::Codex => Ok(DirectProvider::Codex),
        BaseCodingAgent::ClaudeCode => Ok(DirectProvider::ClaudeCode),
        BaseCodingAgent::OhMyPi => Ok(DirectProvider::OhMyPi),
        BaseCodingAgent::Opencode => Ok(DirectProvider::Opencode),
        BaseCodingAgent::DeepseekHarness => Ok(DirectProvider::DeepseekHarness),
        #[cfg(feature = "qa-mode")]
        BaseCodingAgent::QaMock => Err(ApiError::BadRequest(
            "QA mock is not a production Agent Runtime provider".to_string(),
        )),
    }
}

fn orchestration_api_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::BadRequest(format!(
        "Canonical orchestration rejected the operation: {error}"
    ))
}

async fn resolve_issue_project_id(
    pool: &SqlitePool,
    issue_id: Uuid,
) -> Result<Option<Uuid>, ApiError> {
    let has_local_issues: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'local_issues'",
    )
    .fetch_one(pool)
    .await?;

    if has_local_issues == 0 {
        return Ok(None);
    }

    Ok(
        sqlx::query_scalar("SELECT project_id FROM local_issues WHERE id = ?")
            .bind(issue_id)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn get_workflow_run_response(
    pool: &SqlitePool,
    run_id: Uuid,
) -> Result<WorkflowRunResponse, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT id, orchestration_run_id, workflow_id, attempt_id, issue_id, workspace_id, trigger_source, input_text,
               output_text, status, started_at, finished_at, error_text, created_at, updated_at
        FROM workflow_runs
        WHERE id = ?
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::BadRequest("Workflow run not found".to_string()))?;

    let nodes = node_execution_responses(pool, run_id).await?;
    let status = workflow_run_status_from_str(&row.try_get::<String, _>("status")?)?;
    let mut runtime_view = build_workflow_run_runtime_view(
        run_id,
        status,
        &nodes,
        chrono::Utc::now(),
        WORKFLOW_NODE_ACTIVE_SLOW_THRESHOLD_MS,
    );
    project_reused_work(pool, run_id, &mut runtime_view).await?;
    let files = db::models::workflow_files::WorkflowFileRunView::load(pool, run_id).await?;

    Ok(WorkflowRunResponse {
        id: row.try_get("id")?,
        orchestration_run_id: row.try_get("orchestration_run_id")?,
        workflow_id: row.try_get("workflow_id")?,
        attempt_id: row.try_get("attempt_id")?,
        issue_id: row.try_get("issue_id")?,
        workspace_id: row.try_get("workspace_id")?,
        trigger_source: row.try_get("trigger_source")?,
        input_text: row.try_get("input_text")?,
        output_text: row.try_get("output_text")?,
        status,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        error_text: row.try_get("error_text")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        nodes,
        split_batches: files.split_batches,
        handoffs: files.handoffs,
        delivery: files.delivery,
        request_path: Some(format!(".vibe-kanban/workflows/{run_id}/request.md")),
        runtime_view: Some(runtime_view),
        queue_phase: sqlx::query_scalar("SELECT phase FROM workflow_run_queue WHERE run_id=?")
            .bind(run_id)
            .fetch_optional(pool)
            .await?,
    })
}

/// Reuse and an unselected branch are plan facts, not fresh executions. Keep
/// every exact source iteration and leave all write/session identities empty.
async fn project_reused_work(
    pool: &SqlitePool,
    run_id: Uuid,
    view: &mut crate::routes::workflows::WorkflowRunRuntimeView,
) -> Result<(), ApiError> {
    use super::management::WorkflowReuseView;
    use crate::routes::workflows::{
        WorkflowNodeWorkStatus, WorkflowNodeWorkView, WorkflowRuntimeHealth,
    };
    let reused=sqlx::query("SELECT u.node_id,u.iteration,u.source_node_execution_id,n.run_id AS source_run_id,n.output_text FROM workflow_result_reuse u JOIN node_executions n ON n.id=u.source_node_execution_id WHERE u.run_id=? ORDER BY u.node_id,u.iteration")
        .bind(run_id).fetch_all(pool).await?;
    let skipped:Vec<String>=sqlx::query_scalar("SELECT node_id FROM workflow_node_dispositions WHERE run_id=? AND disposition='skipped' ORDER BY node_id")
        .bind(run_id).fetch_all(pool).await?;
    if reused.is_empty() && skipped.is_empty() {
        return Ok(());
    }
    let graph_json:String=sqlx::query_scalar("SELECT COALESCE(r.graph_snapshot,w.graph_json) FROM workflow_runs r JOIN workflows w ON w.id=r.workflow_id WHERE r.id=?")
        .bind(run_id).fetch_one(pool).await?;
    let graph: WorkflowGraph = serde_json::from_str(&graph_json)
        .map_err(|_| ApiError::Conflict("Frozen workflow cannot project result lineage".into()))?;
    let mut grouped: HashMap<String, Vec<WorkflowReuseView>> = HashMap::new();
    for row in reused {
        let result = WorkflowReuseView {
            node_id: row.try_get("node_id")?,
            iteration: row.try_get("iteration")?,
            source_node_execution_id: row.try_get("source_node_execution_id")?,
            source_run_id: row.try_get("source_run_id")?,
            output_text: row.try_get("output_text")?,
        };
        grouped
            .entry(result.node_id.clone())
            .or_default()
            .push(result);
    }
    for node in &graph.nodes {
        let results = grouped.remove(&node.id).unwrap_or_default();
        let is_skipped = skipped.contains(&node.id);
        if results.is_empty() && !is_skipped {
            continue;
        }
        if let Some(actual) = view
            .node_work
            .iter_mut()
            .find(|work| work.node_id == node.id)
        {
            actual.reused_results = results;
            continue;
        }
        view.node_work.push(WorkflowNodeWorkView {
            node_id: node.id.clone(),
            node_type: node_kind_value(&node.kind).into(),
            iteration: results
                .iter()
                .map(|result| result.iteration)
                .max()
                .unwrap_or(0),
            status: if results.is_empty() {
                WorkflowNodeWorkStatus::Skipped
            } else {
                WorkflowNodeWorkStatus::Reused
            },
            pending_work_count: 0,
            starting_child_count: 0,
            active_execution_id: None,
            active_session_id: None,
            orchestration_node_execution_id: None,
            active_agent_run_id: None,
            projection_status: None,
            active_started_at: None,
            active_elapsed_ms: None,
            active_slow: false,
            active_slow_threshold_ms: WORKFLOW_NODE_ACTIVE_SLOW_THRESHOLD_MS,
            runtime_health: WorkflowRuntimeHealth::Ok,
            can_open_session: false,
            can_retry: false,
            can_approve: false,
            can_reject: false,
            can_select_arena_winner: false,
            can_select_condition_branch: false,
            can_cancel_node: false,
            reused_results: results,
        });
    }
    view.node_work.sort_by_key(|work| {
        graph
            .nodes
            .iter()
            .position(|node| node.id == work.node_id)
            .unwrap_or(usize::MAX)
    });
    view.reused_node_count = view
        .node_work
        .iter()
        .filter(|work| work.status == WorkflowNodeWorkStatus::Reused)
        .count() as i32;
    view.skipped_node_count = view
        .node_work
        .iter()
        .filter(|work| work.status == WorkflowNodeWorkStatus::Skipped)
        .count() as i32;
    Ok(())
}

pub async fn approve_human_node<A>(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    agent_executor: &A,
) -> Result<WorkflowRunResponse, ApiError>
where
    A: WorkflowAgentExecutor,
{
    let arena_creator = NoopWorkflowArenaCreator;
    approve_human_node_with_arena(pool, run_id, node_id, agent_executor, &arena_creator).await
}

pub async fn approve_human_node_with_arena<A, R>(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    agent_executor: &A,
    arena_creator: &R,
) -> Result<WorkflowRunResponse, ApiError>
where
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
{
    approve_human_node_at(pool, run_id, node_id, None, agent_executor, arena_creator).await
}

pub async fn approve_human_node_at<A: WorkflowAgentExecutor, R: WorkflowArenaCreator>(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    interaction_id: Option<Uuid>,
    agent_executor: &A,
    arena_creator: &R,
) -> Result<WorkflowRunResponse, ApiError> {
    let lock = workflow_run_lock(run_id);
    let guard = lock.lock().await;
    let run = load_runtime_run(pool, run_id).await?;
    let node = run
        .graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .ok_or_else(|| ApiError::BadRequest(format!("Workflow node `{node_id}` not found")))?;
    if node.kind != WorkflowNodeKind::HumanGate {
        return Err(ApiError::BadRequest(format!(
            "Workflow node `{node_id}` is not a human gate"
        )));
    }
    let iteration =
        interaction_iteration(pool, run_id, node_id, interaction_id, "awaiting_human").await?;
    let context = node_context(pool, &run.graph, run_id, node, iteration, &run.input_text).await?;
    let approval_output = context.upstream_text();
    // Prepare the verified handoff before the transactional success decision.
    // A crash after that decision can safely continue downstream from files.
    file_handoff::forward(pool, &run.graph, run_id, node, iteration).await?;
    let claimed_iteration = claim_interaction(
        pool,
        run_id,
        node_id,
        interaction_id,
        "awaiting_human",
        serde_json::json!({"action":"approve", "output_text":approval_output}),
    )
    .await?;
    if claimed_iteration != iteration {
        return Err(ApiError::Conflict("Workflow interaction changed".into()));
    }
    mark_node_succeeded(
        pool,
        run_id,
        node_id,
        iteration,
        Some(&approval_output),
        None,
    )
    .await?;
    update_run_status(pool, run_id, WorkflowRunStatus::Running, None, None, false).await?;
    drop(guard);
    drive_workflow_run(
        pool,
        run_id,
        &run.graph,
        run.issue_id,
        run.workspace_id,
        &run.input_text,
        agent_executor,
        arena_creator,
    )
    .await?;

    get_workflow_run_response(pool, run_id).await
}

pub async fn select_condition_branch_with_arena<A, R>(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    selected_target_node_ids: Vec<String>,
    reason: Option<String>,
    agent_executor: &A,
    arena_creator: &R,
) -> Result<WorkflowRunResponse, ApiError>
where
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
{
    select_condition_branch_at(
        pool,
        run_id,
        node_id,
        None,
        selected_target_node_ids,
        reason,
        agent_executor,
        arena_creator,
    )
    .await
}

pub async fn select_condition_branch_at<A: WorkflowAgentExecutor, R: WorkflowArenaCreator>(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    interaction_id: Option<Uuid>,
    selected_target_node_ids: Vec<String>,
    reason: Option<String>,
    agent_executor: &A,
    arena_creator: &R,
) -> Result<WorkflowRunResponse, ApiError> {
    let lock = workflow_run_lock(run_id);
    let guard = lock.lock().await;
    let run = load_runtime_run(pool, run_id).await?;
    let node = run
        .graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .ok_or_else(|| ApiError::BadRequest(format!("Workflow node `{node_id}` not found")))?;
    if node.kind != WorkflowNodeKind::Condition {
        return Err(ApiError::BadRequest(format!(
            "Workflow node `{node_id}` is not a condition"
        )));
    }
    let iteration =
        interaction_iteration(pool, run_id, node_id, interaction_id, "awaiting_human").await?;

    let existing_output = node_execution_output_text(pool, run_id, node_id, iteration).await?;
    let manual_route = build_manual_route(
        &run.graph,
        node,
        &selected_target_node_ids,
        reason.as_deref(),
        output_has_router_mutation_warning(existing_output.as_deref()),
    )
    .map_err(|err| ApiError::BadRequest(err.to_string()))?;

    file_handoff::forward(pool, &run.graph, run_id, node, iteration).await?;
    let claimed_iteration = claim_interaction(
        pool,
        run_id,
        node_id,
        interaction_id,
        "awaiting_human",
        serde_json::json!({"action":"branch", "output_text":manual_route.output_text, "skipped":manual_route.skipped_target_node_ids}),
    )
    .await?;
    if claimed_iteration != iteration {
        return Err(ApiError::Conflict(
            "Workflow interaction changed".to_string(),
        ));
    }
    mark_node_succeeded(
        pool,
        run_id,
        node_id,
        iteration,
        Some(&manual_route.output_text),
        None,
    )
    .await?;
    update_run_status(pool, run_id, WorkflowRunStatus::Running, None, None, false).await?;
    drop(guard);
    drive_workflow_run(
        pool,
        run_id,
        &run.graph,
        run.issue_id,
        run.workspace_id,
        &run.input_text,
        agent_executor,
        arena_creator,
    )
    .await?;

    get_workflow_run_response(pool, run_id).await
}

pub async fn select_arena_winner_with_arena<A, R, W>(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    candidate_id: Uuid,
    agent_executor: &A,
    arena_creator: &R,
    winner_applier: &W,
) -> Result<WorkflowRunResponse, ApiError>
where
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
    W: WorkflowArenaWinnerApplier,
{
    select_arena_winner_at(
        pool,
        run_id,
        node_id,
        None,
        candidate_id,
        agent_executor,
        arena_creator,
        winner_applier,
    )
    .await
}

pub async fn select_arena_winner_at<
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
    W: WorkflowArenaWinnerApplier,
>(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    interaction_id: Option<Uuid>,
    candidate_id: Uuid,
    agent_executor: &A,
    arena_creator: &R,
    winner_applier: &W,
) -> Result<WorkflowRunResponse, ApiError> {
    let lock = workflow_run_lock(run_id);
    let guard = lock.lock().await;
    let run = load_runtime_run(pool, run_id).await?;
    let node = run
        .graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .ok_or_else(|| ApiError::BadRequest(format!("Workflow node `{node_id}` not found")))?;
    if node.kind != WorkflowNodeKind::Arena {
        return Err(ApiError::BadRequest(format!(
            "Workflow node `{node_id}` is not an arena node"
        )));
    }
    let arena_group_id: Uuid = sqlx::query_scalar::<_, Option<Uuid>>("SELECT arena_group_id FROM node_executions WHERE run_id=? AND node_id=? AND status='awaiting_arena' AND (? IS NULL OR id=?) ORDER BY iteration DESC LIMIT 1")
        .bind(run_id).bind(node_id).bind(interaction_id).bind(interaction_id)
        .fetch_optional(pool).await?.flatten().ok_or_else(|| ApiError::Conflict("Workflow Arena interaction is stale or unavailable".into()))?;
    let iteration = claim_interaction(
        pool,
        run_id,
        node_id,
        interaction_id,
        "awaiting_arena",
        serde_json::json!({"action":"arena_winner", "candidate_id":candidate_id}),
    )
    .await?;

    let result = async {
        let ArenaWinnerExecution {
            output_text,
            artifact_paths,
        } = winner_applier
            .apply_winner(ArenaWinnerRequest {
                run_id,
                node_id: node_id.to_string(),
                arena_group_id,
                main_workspace_id: run.workspace_id,
                candidate_id,
            })
            .await?;
        let execution_id = file_handoff::execution_id(pool, run_id, node_id, iteration).await?;
        let root = workflow_control_root(pool, run.workspace_id).await?;
        let note = format!(".vibe-kanban/workflows/{run_id}/results/{execution_id}.md");
        file_handoff::write_result_note(pool, execution_id, &root, &note, &output_text).await?;
        file_handoff::publish(pool, run_id, execution_id, Some(note), artifact_paths).await?;
        mark_node_succeeded(pool, run_id, node_id, iteration, Some(&output_text), None).await?;
        Ok::<(), ApiError>(())
    }
    .await;
    if let Err(err) = result {
        let message = format!("{err}; winner candidate: {candidate_id}");
        mark_node_failed(pool, run_id, node_id, iteration, &message).await?;
        settle_workflow_failure(pool, run_id, &message).await?;
        return get_workflow_run_response(pool, run_id).await;
    }
    update_run_status(pool, run_id, WorkflowRunStatus::Running, None, None, false).await?;
    drop(guard);
    drive_workflow_run(
        pool,
        run_id,
        &run.graph,
        run.issue_id,
        run.workspace_id,
        &run.input_text,
        agent_executor,
        arena_creator,
    )
    .await?;

    get_workflow_run_response(pool, run_id).await
}

pub async fn reject_human_node(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
) -> Result<WorkflowRunResponse, ApiError> {
    reject_human_node_at(pool, run_id, node_id, None).await
}

pub async fn reject_human_node_at(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    interaction_id: Option<Uuid>,
) -> Result<WorkflowRunResponse, ApiError> {
    let lock = workflow_run_lock(run_id);
    let _guard = lock.lock().await;
    let run = load_runtime_run(pool, run_id).await?;
    if !run
        .graph
        .nodes
        .iter()
        .any(|node| node.id == node_id && node.kind == WorkflowNodeKind::HumanGate)
    {
        return Err(ApiError::BadRequest(
            "Only a Human Gate can accept a workflow rejection".into(),
        ));
    }
    let message = "Human gate rejected";
    let iteration = claim_interaction(
        pool,
        run_id,
        node_id,
        interaction_id,
        "awaiting_human",
        serde_json::json!({"action":"reject", "error":message}),
    )
    .await?;
    mark_node_failed(pool, run_id, node_id, iteration, message).await?;
    settle_workflow_failure(pool, run_id, message).await?;

    get_workflow_run_response(pool, run_id).await
}

async fn claim_interaction(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    expected: Option<Uuid>,
    status: &str,
    response: serde_json::Value,
) -> Result<i64, ApiError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO workflow_interaction_responses(node_execution_id,response_json)
         SELECT n.id,? FROM node_executions n JOIN workflow_runs r ON r.id=n.run_id
         WHERE n.run_id=? AND n.node_id=? AND n.status=? AND (? IS NULL OR n.id=?)
           AND r.status IN ('running','awaiting_human','awaiting_arena')
           AND r.error_text IS NULL
         ORDER BY n.iteration DESC LIMIT 1
         ON CONFLICT(node_execution_id) DO NOTHING RETURNING node_execution_id",
    )
    .bind(response.to_string())
    .bind(run_id)
    .bind(node_id)
    .bind(status)
    .bind(expected)
    .bind(expected)
    .fetch_optional(&mut *tx)
    .await?;
    let id = claimed.ok_or_else(|| {
        ApiError::Conflict("Workflow interaction is stale or already answered".to_string())
    })?;
    let iteration = sqlx::query_scalar("SELECT iteration FROM node_executions WHERE id=?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    // Persist the decision and its state transition together. Restart can
    // resume downstream dispatch without losing an already accepted answer.
    let action = response["action"].as_str().unwrap_or_default();
    if action != "arena_winner" {
        let rejected = action == "reject";
        sqlx::query("UPDATE node_executions SET status=?, output_text=?, error_text=?, finished_at=datetime('now','subsec'), updated_at=datetime('now','subsec') WHERE id=?")
            .bind(if rejected { "failed" } else { "succeeded" })
            .bind(response["output_text"].as_str()).bind(response["error"].as_str())
            .bind(id).execute(&mut *tx).await?;
        if let Some(skipped) = response["skipped"].as_array() {
            for target in skipped.iter().filter_map(serde_json::Value::as_str) {
                sqlx::query("UPDATE node_executions SET status='skipped', finished_at=datetime('now','subsec') WHERE id=(SELECT id FROM node_executions WHERE run_id=? AND node_id=? AND status='pending' ORDER BY iteration ASC LIMIT 1)")
                    .bind(run_id).bind(target).execute(&mut *tx).await?;
            }
        }
        // Rejection is a node fact. The parent remains active until every
        // real child, system turn and durable command has settled.
        sqlx::query("UPDATE workflow_runs SET status='running', error_text=?, updated_at=datetime('now','subsec') WHERE id=?")
            .bind(response["error"].as_str()).bind(run_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    if action != "arena_winner" {
        // The transaction already made this execution terminal. The ordinary
        // update helper correctly refuses to change it, so publish the actual
        // committed decision here instead of waiting for a page refresh.
        emit_node_update(
            run_id,
            node_id,
            iteration,
            NodeExecutionUpdate {
                status: if action == "reject" {
                    DbNodeExecutionStatus::Failed
                } else {
                    DbNodeExecutionStatus::Succeeded
                },
                input_text: None,
                output_text: response["output_text"].as_str(),
                session_id: None,
                orchestration_node_execution_id: None,
                agent_run_id: None,
                arena_group_id: None,
                error_text: response["error"].as_str(),
                finished: true,
            },
        );
    }
    Ok(iteration)
}

async fn interaction_iteration(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    expected: Option<Uuid>,
    status: &str,
) -> Result<i64, ApiError> {
    sqlx::query_scalar("SELECT iteration FROM node_executions WHERE run_id=? AND node_id=? AND status=? AND (? IS NULL OR id=?) AND NOT EXISTS(SELECT 1 FROM workflow_interaction_responses WHERE node_execution_id=node_executions.id) ORDER BY iteration DESC LIMIT 1")
        .bind(run_id).bind(node_id).bind(status).bind(expected).bind(expected)
        .fetch_optional(pool).await?
        .ok_or_else(|| ApiError::Conflict("Workflow interaction is stale or already answered".into()))
}

pub async fn cancel_workflow_run_runtime<C>(
    pool: &SqlitePool,
    run_id: Uuid,
    canceller: &C,
) -> Result<WorkflowRunResponse, ApiError>
where
    C: WorkflowRunCanceller,
{
    let lock = workflow_run_lock(run_id);
    let guard = lock.lock().await;
    ensure_run_exists(pool, run_id).await?;
    if db::models::workflow_queue::WorkflowQueueEntry::cancel_queued(pool, run_id).await? {
        sqlx::query("UPDATE node_executions SET status='cancelled', finished_at=datetime('now','subsec') WHERE run_id=? AND status='pending'")
            .bind(run_id).execute(pool).await?;
        return get_workflow_run_response(pool, run_id).await;
    }
    let status: WorkflowRunStatus =
        sqlx::query_scalar("SELECT status FROM workflow_runs WHERE id = ?")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .map_err(ApiError::Database)?;
    if matches!(
        status,
        WorkflowRunStatus::Canceled | WorkflowRunStatus::Failed | WorkflowRunStatus::Succeeded
    ) {
        let mut conn = pool.acquire().await?;
        let settled = super::management::source_boundary_settled_in(&mut conn, run_id).await?;
        drop(conn);
        if settled {
            return get_workflow_run_response(pool, run_id).await;
        }
        // A failed parent may still have a running parallel child. Keep the
        // explicit cancel usable until real child/process shutdown is confirmed.
    }
    update_run_status(
        pool,
        run_id,
        WorkflowRunStatus::Cancelling,
        None,
        Some("Workflow run cancellation requested"),
        false,
    )
    .await?;
    project_workflow_nodes_cancelling(pool, run_id).await?;
    // System-turn cancellation takes the main Session lock. Release the Run
    // lock first, after persisting the stop and observing durable child IDs.
    drop(guard);
    let mut first_error = None;
    if let Some(orchestration_run_id) = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT orchestration_run_id FROM workflow_runs WHERE id = ?",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?
    {
        if let Err(error) = canceller
            .cancel_orchestration_run(pool, orchestration_run_id)
            .await
        {
            first_error.get_or_insert(error);
        }
    } else {
        for session_id in running_node_session_ids(pool, run_id).await? {
            if let Err(error) = canceller.cancel_session(session_id).await {
                first_error.get_or_insert(error);
            }
        }
    }
    let arena_runs: Vec<Uuid> = sqlx::query_scalar("SELECT DISTINCT o.id FROM orchestration_runs o JOIN node_executions n ON n.arena_group_id=o.source_definition_id WHERE n.run_id=? AND o.product_kind='arena'")
        .bind(run_id).fetch_all(pool).await?;
    for arena_run in arena_runs {
        if let Err(error) = canceller.cancel_orchestration_run(pool, arena_run).await {
            first_error.get_or_insert(error);
        }
    }
    reconcile_cancelling_workflow_run(pool, run_id).await?;

    if let Some(error) = first_error {
        return Err(error);
    }

    get_workflow_run_response(pool, run_id).await
}

async fn project_workflow_nodes_cancelling(
    pool: &SqlitePool,
    run_id: Uuid,
) -> Result<(), ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT node_id, iteration, status, session_id, node_type,
               orchestration_node_execution_id, agent_run_id,
               (agent_run_id IS NOT NULL OR arena_group_id IS NOT NULL
                OR EXISTS(SELECT 1 FROM orchestration_agent_run_links l
                  WHERE l.node_execution_id=node_executions.orchestration_node_execution_id)
                OR EXISTS(SELECT 1 FROM orchestration_outbox o
                  WHERE o.node_execution_id=node_executions.orchestration_node_execution_id)) AS has_launch
        FROM node_executions
        WHERE run_id = ?
          AND status IN ('pending', 'running', 'awaiting_human', 'awaiting_arena')
        ORDER BY rowid
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;

    for row in rows {
        let current_status: String = row.try_get("status")?;
        let undispatched = !row.try_get::<bool, _>("has_launch")?
            || (current_status == "awaiting_human"
                && row.try_get::<String, _>("node_type")? == "human_gate");
        let node_id: String = row.try_get("node_id")?;
        update_node_execution(
            pool,
            run_id,
            &node_id,
            row.try_get("iteration")?,
            NodeExecutionUpdate {
                status: if undispatched {
                    DbNodeExecutionStatus::Cancelled
                } else {
                    DbNodeExecutionStatus::Cancelling
                },
                input_text: None,
                output_text: None,
                session_id: row.try_get("session_id")?,
                orchestration_node_execution_id: row.try_get("orchestration_node_execution_id")?,
                agent_run_id: row.try_get("agent_run_id")?,
                arena_group_id: None,
                error_text: None,
                finished: undispatched,
            },
        )
        .await?;
    }

    Ok(())
}

async fn reconcile_cancelling_workflow_run(
    pool: &SqlitePool,
    run_id: Uuid,
) -> Result<(), ApiError> {
    batch_runtime::cancel_pending(pool, run_id).await?;
    sqlx::query("UPDATE node_executions SET status='cancelled',finished_at=datetime('now','subsec') WHERE run_id=? AND status='cancelling' AND arena_group_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM orchestration_runs o JOIN orchestration_agent_run_links l ON l.orchestration_run_id=o.id LEFT JOIN agent_run_state s ON s.agent_run_id=l.agent_run_id WHERE o.product_kind='arena' AND o.source_definition_id=node_executions.arena_group_id AND (s.agent_run_id IS NULL OR s.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed')))")
        .bind(run_id).execute(pool).await?;
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM node_executions WHERE run_id = ? AND status IN ('running','awaiting_human','awaiting_arena','cancelling')",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    if active == 0 && children_settled(pool, run_id).await? {
        sqlx::query(
            "UPDATE node_executions SET status = 'cancelled', finished_at = datetime('now','subsec'), updated_at = datetime('now','subsec') WHERE run_id = ? AND status = 'pending'",
        )
        .bind(run_id)
        .execute(pool)
        .await?;
        update_run_status(
            pool,
            run_id,
            WorkflowRunStatus::Canceled,
            None,
            Some("Workflow run canceled"),
            true,
        )
        .await?;
    }
    Ok(())
}

pub async fn retry_workflow_node<A>(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    agent_executor: &A,
) -> Result<WorkflowRunResponse, ApiError>
where
    A: WorkflowAgentExecutor,
{
    let arena_creator = NoopWorkflowArenaCreator;
    retry_workflow_node_with_arena(pool, run_id, node_id, agent_executor, &arena_creator).await
}

pub async fn retry_workflow_node_with_arena<A, R>(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    _agent_executor: &A,
    _arena_creator: &R,
) -> Result<WorkflowRunResponse, ApiError>
where
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
{
    let run = load_runtime_run(pool, run_id).await?;
    let node = run
        .graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .ok_or_else(|| ApiError::BadRequest(format!("Workflow node `{node_id}` not found")))?;
    if !matches!(
        node.kind,
        WorkflowNodeKind::Agent | WorkflowNodeKind::Condition | WorkflowNodeKind::Transform
    ) {
        return Err(ApiError::BadRequest(format!(
            "Workflow node `{node_id}` cannot be retried"
        )));
    }
    let instance_id: Option<Uuid> =
        sqlx::query_scalar("SELECT attempt_id FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    let instance_id = instance_id.ok_or_else(|| {
        ApiError::Conflict(
            "Retry requires a stable Workflow instance; historical standalone Runs are immutable"
                .into(),
        )
    })?;
    let failed: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM node_executions WHERE run_id=? AND node_id=? AND status='failed' ORDER BY iteration")
        .bind(run_id).bind(node_id).fetch_all(pool).await?;
    if failed.len() != 1 {
        return Err(ApiError::Conflict("Retry must name one exact failed NodeExecution; use workflow-management/submit when several iterations failed".into()));
    }
    let caller = super::management::WorkflowManagementCaller::Instance {
        instance_id,
        namespace: format!("page-instance:{instance_id}"),
        integration_id: None,
    };
    let accepted = super::management::submit_workflow(
        pool,
        &caller,
        super::management::WorkflowSubmission {
            request_id: Uuid::new_v4().to_string(),
            action: super::management::WorkflowSubmissionAction::Retry,
            input_text: None,
            material_paths: Vec::new(),
            source_run_id: Some(run_id),
            source_node_execution_id: Some(failed[0]),
            scope: super::management::WorkflowSubmissionScope::All,
            active_policy: None,
            source_message_id: None,
            delivery_intent: None,
            source_batch_id: None,
        },
        None,
        "manual",
    )
    .await?;
    get_workflow_run_response(pool, accepted.run_id).await
}

pub async fn reconcile_workflow_run<A>(
    pool: &SqlitePool,
    run_id: Uuid,
    agent_executor: &A,
) -> Result<WorkflowRunResponse, ApiError>
where
    A: WorkflowAgentExecutor,
{
    let arena_creator = NoopWorkflowArenaCreator;
    reconcile_workflow_run_with_arena(pool, run_id, agent_executor, &arena_creator).await
}

pub async fn reconcile_workflow_run_with_arena<A, R>(
    pool: &SqlitePool,
    run_id: Uuid,
    agent_executor: &A,
    arena_creator: &R,
) -> Result<WorkflowRunResponse, ApiError>
where
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
{
    reconcile_workflow_run_with_arena_and_boundary(
        pool,
        run_id,
        agent_executor,
        arena_creator,
        &UnconfiguredAgentRunReconciliationBoundary,
    )
    .await
}

pub async fn reconcile_workflow_run_with_arena_and_boundary<A, R, B>(
    pool: &SqlitePool,
    run_id: Uuid,
    agent_executor: &A,
    arena_creator: &R,
    boundary: &B,
) -> Result<WorkflowRunResponse, ApiError>
where
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
    B: AgentRunReconciliationBoundary,
{
    let current = get_workflow_run_response(pool, run_id).await?;
    if current.queue_phase.as_deref().is_some_and(|phase| {
        matches!(
            phase,
            "queued" | "waiting_for_source" | "stopping_source" | "starting"
        )
    }) {
        return Ok(current);
    }
    if !matches!(
        current.status,
        WorkflowRunStatus::Pending
            | WorkflowRunStatus::Running
            | WorkflowRunStatus::AwaitingHuman
            | WorkflowRunStatus::AwaitingArena
            | WorkflowRunStatus::Cancelling
    ) {
        return Ok(current);
    }

    let outcome = boundary.reconcile_workflow_run(pool, run_id).await?;

    if current.status == WorkflowRunStatus::Cancelling {
        reconcile_cancelling_workflow_run(pool, run_id).await?;
        return get_workflow_run_response(pool, run_id).await;
    }
    // Startup/review failure is durable independently of any Node result.
    // A later peer success or input request cannot erase it or restart work.
    let failure: Option<String> =
        sqlx::query_scalar("SELECT error_text FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    if let Some(failure) = failure {
        fail_accepted_workflow_run(pool, run_id, &failure).await?;
        return get_workflow_run_response(pool, run_id).await;
    }
    let run = load_runtime_run(pool, run_id).await?;
    if outcome.awaiting_human {
        update_run_status(
            pool,
            run_id,
            WorkflowRunStatus::AwaitingHuman,
            None,
            None,
            false,
        )
        .await?;
    } else if outcome.completed {
        update_run_status(pool, run_id, WorkflowRunStatus::Running, None, None, false).await?;
    }
    // Dispatch acceptance and a single sibling completion also need a tick.
    // The driver handles batch barriers and does not terminate live siblings.
    drive_workflow_run(
        pool,
        run_id,
        &run.graph,
        run.issue_id,
        run.workspace_id,
        &run.input_text,
        agent_executor,
        arena_creator,
    )
    .await?;

    get_workflow_run_response(pool, run_id).await
}

pub async fn recover_stale_workflow_runs(pool: &SqlitePool) -> Result<u64, ApiError> {
    recover_stale_workflow_runs_with_boundary(pool, &UnconfiguredAgentRunReconciliationBoundary)
        .await
}

/// Reconcile active workflow runs through the canonical AgentRun boundary.
///
/// This function intentionally does not join or inspect `execution_processes`.
/// The legacy process table remains a local supervisor detail and cannot be a
/// source of workflow terminal decisions after restart.
pub async fn recover_stale_workflow_runs_with_boundary<B>(
    pool: &SqlitePool,
    boundary: &B,
) -> Result<u64, ApiError>
where
    B: AgentRunReconciliationBoundary,
{
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT workflow_runs.id AS run_id
        FROM workflow_runs
        JOIN orchestration_runs
          ON orchestration_runs.id = workflow_runs.orchestration_run_id
        LEFT JOIN orchestration_node_executions orchestration_nodes
          ON orchestration_nodes.orchestration_run_id = orchestration_runs.id
        LEFT JOIN orchestration_agent_run_links links
          ON links.orchestration_run_id = orchestration_runs.id
         AND links.node_execution_id = orchestration_nodes.id
        LEFT JOIN agent_run_state agent_state
          ON agent_state.agent_run_id = links.agent_run_id
        LEFT JOIN node_executions nodes
          ON nodes.run_id = workflow_runs.id
         AND nodes.orchestration_node_execution_id = orchestration_nodes.id
        WHERE orchestration_runs.status NOT IN ('succeeded', 'failed', 'cancelled')
           OR nodes.status IN ('running', 'awaiting_human', 'awaiting_arena', 'cancelling')
           OR agent_state.status IN (
               'pending', 'starting', 'running',
               'awaiting_input', 'awaiting_approval', 'cancelling'
           )
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut observed = 0;
    for row in rows {
        let run_id: Uuid = row.try_get("run_id")?;
        let _ = boundary.reconcile_workflow_run(pool, run_id).await?;
        observed += 1;
    }

    Ok(observed)
}

#[derive(Debug)]
struct RuntimeRun {
    graph: WorkflowGraph,
    issue_id: Uuid,
    workspace_id: Uuid,
    input_text: String,
}

async fn load_runtime_run(pool: &SqlitePool, run_id: Uuid) -> Result<RuntimeRun, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT issue_id, workspace_id, input_text, graph_snapshot
        FROM workflow_runs
        WHERE id = ?
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::BadRequest("Workflow run not found".to_string()))?;

    let graph_json = row
        .try_get::<Option<String>, _>("graph_snapshot")?
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "Workflow run {run_id} has no immutable graph snapshot"
            ))
        })?;
    let graph: WorkflowGraph = serde_json::from_str(&graph_json)
        .map_err(|err| ApiError::BadRequest(format!("Invalid workflow graph JSON: {err}")))?;
    validate_graph_for_run(&graph)
        .map_err(|err| ApiError::BadRequest(format!("Invalid workflow graph: {err}")))?;

    Ok(RuntimeRun {
        graph,
        issue_id: row.try_get("issue_id")?,
        workspace_id: row
            .try_get::<Option<Uuid>, _>("workspace_id")?
            .ok_or_else(|| ApiError::BadRequest("Workflow run has no workspace".to_string()))?,
        input_text: row.try_get("input_text")?,
    })
}

#[derive(Debug, Clone, Copy)]
pub(super) struct WorkflowTaskParent {
    task_id: Uuid,
    project_id: Uuid,
    issue_id: Uuid,
}

async fn initialize_workflow_run(
    pool: &SqlitePool,
    run_id: Uuid,
    workflow_id: Uuid,
    attempt_id: Option<Uuid>,
    workspace_id: Uuid,
    request: &TriggerWorkflowRequest,
    graph: &WorkflowGraph,
    queue_project_id: Option<Uuid>,
) -> Result<(), ApiError> {
    let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
    initialize_workflow_run_in(
        &mut transaction,
        run_id,
        workflow_id,
        attempt_id,
        workspace_id,
        request,
        graph,
        queue_project_id,
        "queued",
        None,
    )
    .await?;
    transaction.commit().await?;
    emit_run_status(
        run_id,
        if queue_project_id.is_some() {
            WorkflowRunStatus::Pending
        } else {
            WorkflowRunStatus::Running
        },
        None,
        None,
    );
    Ok(())
}

/// Shared acceptance primitive. Request/dependency/stop intent and new Run are
/// committed by the caller in one transaction, before provider I/O.
#[allow(clippy::too_many_arguments)]
pub(super) async fn initialize_workflow_run_in(
    transaction: &mut SqliteConnection,
    run_id: Uuid,
    workflow_id: Uuid,
    attempt_id: Option<Uuid>,
    workspace_id: Uuid,
    request: &TriggerWorkflowRequest,
    graph: &WorkflowGraph,
    queue_project_id: Option<Uuid>,
    queue_phase: &str,
    affected_node_ids: Option<&HashSet<String>>,
) -> Result<(), ApiError> {
    let graph_snapshot = serde_json::to_string(graph).map_err(|error| {
        ApiError::BadRequest(format!("Cannot snapshot workflow graph: {error}"))
    })?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_runs WHERE id=?)")
        .bind(run_id)
        .fetch_one(&mut *transaction)
        .await?;
    if exists {
        return Ok(());
    }
    if let Some(attempt_id) = attempt_id {
        let current: (String,Option<String>)=sqlx::query_as("SELECT w.graph_json,a.frozen_graph_json FROM workflow_attempts a JOIN workflows w ON w.id=a.workflow_id WHERE a.id=? AND a.issue_id=?")
            .bind(attempt_id).bind(request.issue_id).fetch_optional(&mut *transaction).await?
            .ok_or_else(||ApiError::Conflict("Workflow instance no longer belongs to this Task".into()))?;
        let expected_graph: WorkflowGraph =
            serde_json::from_str(current.1.as_deref().unwrap_or(&current.0))
                .map_err(|_| ApiError::Conflict("Workflow definition is invalid".into()))?;
        if &expected_graph != graph {
            return Err(ApiError::Conflict(
                "Workflow definition changed before acceptance; reload the instance".into(),
            ));
        }
    }
    let inserted = sqlx::query(
        r#"
        INSERT INTO workflow_runs
            (id, workflow_id, attempt_id, issue_id, workspace_id, trigger_source, input_text, graph_snapshot, status, started_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, CASE WHEN ? THEN NULL ELSE datetime('now', 'subsec') END)
        ON CONFLICT(id) DO NOTHING
        "#,
    )
    .bind(run_id)
    .bind(workflow_id)
    .bind(attempt_id)
    .bind(request.issue_id)
    .bind(workspace_id)
    .bind(&request.trigger_source)
    .bind(&request.input_text)
    .bind(&graph_snapshot)
    .bind(if queue_project_id.is_some() { "pending" } else { "running" })
    .bind(queue_project_id.is_some())
    .execute(&mut *transaction)
    .await?;

    if inserted.rows_affected() == 0 {
        return Ok(());
    }
    if let Some(attempt_id) = attempt_id {
        sqlx::query("UPDATE workflow_attempts SET definition_locked_at=COALESCE(definition_locked_at,datetime('now','subsec')),frozen_graph_json=COALESCE(frozen_graph_json,?),latest_run_id=?,workspace_id=COALESCE(workspace_id,?),status='ready',updated_at=datetime('now','subsec') WHERE id=?")
            .bind(&graph_snapshot).bind(run_id).bind(workspace_id).bind(attempt_id).execute(&mut *transaction).await?;
    }

    if request.trigger_source == "external" {
        let authorized: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM external_integration_requests r JOIN external_integrations i ON i.id=r.integration_id JOIN external_integration_projects p ON p.integration_id=i.id JOIN local_issues issue ON issue.project_id=p.project_id WHERE r.resource_id=? AND r.operation='workflow_run' AND i.enabled=1 AND issue.id=?)")
            .bind(run_id).bind(request.issue_id).fetch_one(&mut *transaction).await?;
        if !authorized {
            return Err(ApiError::Forbidden(
                "External integration project access was revoked".into(),
            ));
        }
        let enabled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_attempt_sources s JOIN workflows w ON w.id=s.template_id WHERE s.attempt_id=? AND w.external_enabled=1)")
            .bind(attempt_id).fetch_one(&mut *transaction).await?;
        if !enabled {
            return Err(ApiError::Conflict(
                "Workflow template is no longer enabled for external calls".into(),
            ));
        }
        sqlx::query("UPDATE external_integration_requests SET state='complete' WHERE resource_id=? AND operation='workflow_run'")
            .bind(run_id).execute(&mut *transaction).await?;
    }

    let parent = workflow_task_parent(&mut *transaction, attempt_id, request.issue_id).await?;
    for node in &graph.nodes {
        if affected_node_ids.is_none_or(|ids| ids.contains(&node.id))
            && !batch_managed_nodes(graph).contains(&node.id)
        {
            materialize_node_execution(&mut *transaction, run_id, workspace_id, parent, node, 0)
                .await?;
        }
    }

    if let Some(project_id) = queue_project_id {
        sqlx::query("INSERT INTO workflow_run_queue(run_id,project_id,phase) VALUES (?,?,?)")
            .bind(run_id)
            .bind(project_id)
            .bind(queue_phase)
            .execute(&mut *transaction)
            .await?;
    }

    Ok(())
}

pub(super) async fn workflow_task_parent(
    connection: &mut SqliteConnection,
    attempt_id: Option<Uuid>,
    issue_id: Uuid,
) -> Result<Option<WorkflowTaskParent>, ApiError> {
    let Some(attempt_id) = attempt_id else {
        return Ok(None);
    };
    let parent = sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
        r#"
        SELECT task.id, task.project_id, task.issue_id
        FROM workflow_attempts attempt
        JOIN tasks task ON task.id = attempt.task_id
        WHERE attempt.id = ? AND task.issue_id = ?
        "#,
    )
    .bind(attempt_id)
    .bind(issue_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| {
        ApiError::BadRequest(
            "Workflow attempt is missing its canonical Execution or belongs to another Task"
                .to_string(),
        )
    })?;
    Ok(Some(WorkflowTaskParent {
        task_id: parent.0,
        project_id: parent.1,
        issue_id: parent.2,
    }))
}

pub(super) async fn materialize_node_execution(
    connection: &mut SqliteConnection,
    run_id: Uuid,
    workspace_id: Uuid,
    parent: Option<WorkflowTaskParent>,
    node: &WorkflowNode,
    iteration: i64,
) -> Result<(), ApiError> {
    let execution_id = Uuid::new_v4();
    let source_session_id = if matches!(
        node.kind,
        WorkflowNodeKind::Agent | WorkflowNodeKind::TaskSplitter
    ) {
        node_session_id(node).map_err(ApiError::BadRequest)?
    } else {
        None
    };
    let (task_id, runtime_session_id) = match (node.kind.clone(), parent) {
        (WorkflowNodeKind::Agent, Some(parent)) => {
            let source_session_id = source_session_id.ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "Workflow Agent node `{}` has no source Session",
                    node.id
                ))
            })?;
            let valid: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE id=? AND workspace_id=?")
                    .bind(source_session_id)
                    .bind(workspace_id)
                    .fetch_one(&mut *connection)
                    .await?;
            if valid != 1 {
                return Err(ApiError::BadRequest(format!(
                    "Workflow Agent node `{}` references a missing or foreign Session",
                    node.id
                )));
            }
            let bound:Option<(Uuid,Option<Uuid>)>=sqlx::query_as("SELECT t.id,t.parent_task_id FROM tasks t JOIN agent_task_bindings b ON b.task_id=t.id WHERE b.session_id=?")
                .bind(source_session_id).fetch_optional(&mut *connection).await?;
            let task_id = if let Some((task_id, parent_id)) = bound {
                if parent_id != Some(parent.task_id) {
                    return Err(ApiError::Conflict(
                        "Workflow Node Session belongs to another Execution".to_string(),
                    ));
                }
                task_id
            } else {
                Execution::create(
                    connection,
                    &CreateExecution {
                        id: execution_id,
                        project_id: parent.project_id,
                        issue_id: parent.issue_id,
                        parent_task_id: Some(parent.task_id),
                        title: workflow_node_task_title(node),
                        execution_kind: ExecutionKind::Agent,
                    },
                )
                .await
                .map_err(|error| ApiError::Session(SessionError::Execution(error)))?;
                Execution::bind_agent_session(connection, execution_id, source_session_id)
                    .await
                    .map_err(|error| ApiError::Session(SessionError::Execution(error)))?;
                execution_id
            };
            (Some(task_id), Some(source_session_id))
        }
        (WorkflowNodeKind::Arena, Some(parent)) => {
            Execution::create(
                connection,
                &CreateExecution {
                    id: execution_id,
                    project_id: parent.project_id,
                    issue_id: parent.issue_id,
                    parent_task_id: Some(parent.task_id),
                    title: workflow_node_task_title(node),
                    execution_kind: ExecutionKind::Arena,
                },
            )
            .await
            .map_err(|error| ApiError::Session(SessionError::Execution(error)))?;
            (Some(execution_id), None)
        }
        (WorkflowNodeKind::TaskSplitter, _) => {
            // The splitter's fixed Agent is distinct from the main manager.
            // It has its own stable Session and creates no business Task.
            let source_session_id = source_session_id.ok_or_else(|| {
                ApiError::Conflict("Task splitter requires its configured Node Session".into())
            })?;
            let valid: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE id=? AND workspace_id=?")
                    .bind(source_session_id)
                    .bind(workspace_id)
                    .fetch_one(&mut *connection)
                    .await?;
            if valid != 1 {
                return Err(ApiError::BadRequest(format!(
                    "Workflow task splitter `{}` references a missing or foreign Node Session",
                    node.id
                )));
            }
            (None, Some(source_session_id))
        }
        (WorkflowNodeKind::Agent, None) => (None, source_session_id),
        _ => (None, None),
    };

    sqlx::query(
        r#"
        INSERT INTO node_executions (
            id, run_id, task_id, node_id, node_type, iteration, status, session_id
        )
        VALUES (?, ?, ?, ?, ?, ?, 'pending', ?)
        "#,
    )
    .bind(execution_id)
    .bind(run_id)
    .bind(task_id)
    .bind(&node.id)
    .bind(node_kind_value(&node.kind))
    .bind(iteration)
    .bind(runtime_session_id)
    .execute(connection)
    .await?;

    Ok(())
}

fn workflow_node_task_title(node: &WorkflowNode) -> String {
    node.data
        .display_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(node.id.as_str())
        .trim()
        .to_string()
}

async fn ensure_triggered_node_iterations(
    pool: &SqlitePool,
    run_id: Uuid,
    graph: &WorkflowGraph,
) -> Result<(), ApiError> {
    let snapshot = load_run_snapshot(pool, run_id).await?;
    let existing_counts = existing_execution_counts(pool, run_id).await?;
    let mut max_iterations = max_execution_iterations(pool, run_id).await?;
    let affected_json: Option<String> = sqlx::query_scalar(
        "SELECT affected_nodes_json FROM workflow_run_submissions WHERE run_id=?",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    let affected: Option<HashSet<String>> = affected_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| ApiError::Conflict("Accepted workflow scope is invalid".into()))?;

    for node in graph.nodes.iter().filter(|node| {
        node.kind != WorkflowNodeKind::Start
            && affected.as_ref().is_none_or(|ids| ids.contains(&node.id))
    }) {
        let desired_count = triggered_execution_count(graph, &snapshot, &node.id);
        let existing_count = existing_counts
            .get(node.id.as_str())
            .copied()
            .unwrap_or_default();

        for _ in existing_count..desired_count {
            let next_iteration = max_iterations
                .entry(node.id.clone())
                .and_modify(|iteration| *iteration += 1)
                .or_insert(0);
            insert_node_execution(pool, run_id, node, *next_iteration).await?;
        }
    }

    Ok(())
}

async fn existing_execution_counts(
    pool: &SqlitePool,
    run_id: Uuid,
) -> Result<HashMap<String, i64>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT node_id, COUNT(*) AS count
        FROM workflow_effective_node_executions
        WHERE run_id = ? AND status != 'skipped'
        GROUP BY node_id
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|row| Ok((row.try_get("node_id")?, row.try_get("count")?)))
        .collect::<Result<HashMap<_, _>, ApiError>>()
}

async fn max_execution_iterations(
    pool: &SqlitePool,
    run_id: Uuid,
) -> Result<HashMap<String, i64>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT node_id, MAX(iteration) AS max_iteration
        FROM workflow_effective_node_executions
        WHERE run_id = ?
        GROUP BY node_id
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|row| {
            Ok((
                row.try_get("node_id")?,
                row.try_get::<Option<i64>, _>("max_iteration")?
                    .unwrap_or_default(),
            ))
        })
        .collect::<Result<HashMap<_, _>, ApiError>>()
}

async fn insert_node_execution(
    pool: &SqlitePool,
    run_id: Uuid,
    node: &WorkflowNode,
    iteration: i64,
) -> Result<(), ApiError> {
    let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
    let (attempt_id, issue_id, workspace_id) =
        sqlx::query_as::<_, (Option<Uuid>, Uuid, Option<Uuid>)>(
            r#"
        SELECT attempt_id, issue_id, workspace_id
        FROM workflow_runs
        WHERE id = ?
        "#,
        )
        .bind(run_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Workflow run not found".to_string()))?;
    let workspace_id = workspace_id
        .ok_or_else(|| ApiError::BadRequest("Workflow run has no workspace".to_string()))?;
    let parent = workflow_task_parent(&mut transaction, attempt_id, issue_id).await?;
    materialize_node_execution(
        &mut transaction,
        run_id,
        workspace_id,
        parent,
        node,
        iteration,
    )
    .await?;
    transaction.commit().await?;

    // Every dynamically-created iteration gets its canonical orchestration
    // identity before it can be dispatched or observed.
    let _ = canonical_node_identity(pool, run_id, &node.id, iteration).await?;

    emit_node_status(
        run_id,
        &node.id,
        DbNodeExecutionStatus::Pending,
        json!({ "status": "pending", "iteration": iteration }),
    );

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn drive_workflow_run<A, R>(
    pool: &SqlitePool,
    run_id: Uuid,
    graph: &WorkflowGraph,
    issue_id: Uuid,
    workspace_id: Uuid,
    run_input_text: &str,
    agent_executor: &A,
    arena_creator: &R,
) -> Result<(), ApiError>
where
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
{
    let lock = workflow_run_lock(run_id);
    let _guard = lock.lock().await;
    let runner = WorkflowRunner::from_graph(graph.clone());
    let managed = batch_managed_nodes(graph);

    loop {
        let (status, failure): (String, Option<String>) =
            sqlx::query_as("SELECT status,error_text FROM workflow_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(pool)
                .await?;
        if matches!(
            status.as_str(),
            "cancelling" | "canceled" | "failed" | "succeeded"
        ) {
            return Ok(());
        }
        if let Some(failure) = failure {
            settle_workflow_failure(pool, run_id, &failure).await?;
            return Ok(());
        }
        let before = load_run_snapshot(pool, run_id).await?;
        if before.nodes.iter().any(|node| {
            !managed.contains(&node.node_id)
                && matches!(
                    node.status,
                    PlannerNodeExecutionStatus::Failed | PlannerNodeExecutionStatus::Cancelled
                )
        }) {
            settle_workflow_failure(
                pool,
                run_id,
                "Required workflow work failed before this batch could continue",
            )
            .await?;
            return Ok(());
        }
        recover_undispatched_nodes(
            pool,
            run_id,
            graph,
            issue_id,
            workspace_id,
            run_input_text,
            agent_executor,
            arena_creator,
        )
        .await?;
        let batches = batch_runtime::progress(
            pool,
            run_id,
            graph,
            issue_id,
            workspace_id,
            run_input_text,
            agent_executor,
            arena_creator,
        )
        .await?;
        ensure_triggered_node_iterations(pool, run_id, graph).await?;
        let snapshot = load_run_snapshot(pool, run_id).await?;
        let failed = batches.failed
            || snapshot.nodes.iter().any(|node| {
                matches!(
                    node.status,
                    PlannerNodeExecutionStatus::Failed | PlannerNodeExecutionStatus::Cancelled
                )
            });
        if failed {
            if batches.waiting {
                return Ok(());
            }
            settle_workflow_failure(pool,run_id,
                "Required workflow work or file handoff did not succeed; successful sibling results were retained").await?;
            return Ok(());
        }
        if !batches.waiting
            && all_nodes_terminal(&snapshot)
            && children_settled(pool, run_id).await?
        {
            let output_text = final_run_output(pool, run_id, graph).await?;
            update_run_status(
                pool,
                run_id,
                WorkflowRunStatus::Succeeded,
                output_text,
                None,
                true,
            )
            .await?;
            return Ok(());
        }

        let ready_plan = runner.ready_plan(&snapshot);
        if ready_plan.ready_nodes.is_empty() {
            if batches.changed {
                continue;
            }
            return Ok(());
        }

        let mut should_pause = false;
        for ready_node in ready_plan.ready_nodes {
            let status: String = sqlx::query_scalar("SELECT status FROM workflow_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(pool)
                .await?;
            if matches!(
                status.as_str(),
                "cancelling" | "canceled" | "failed" | "succeeded"
            ) {
                return Ok(());
            }
            let Some(node) = graph
                .nodes
                .iter()
                .find(|node| node.id == ready_node.node_id)
            else {
                continue;
            };
            let step = execute_ready_node(
                pool,
                run_id,
                graph,
                node,
                ready_node.iteration,
                issue_id,
                workspace_id,
                run_input_text,
                agent_executor,
                arena_creator,
            )
            .await;
            let step = match step {
                Ok(step) => step,
                Err(error) => {
                    mark_node_failed(
                        pool,
                        run_id,
                        &node.id,
                        ready_node.iteration,
                        &error.to_string(),
                    )
                    .await?;
                    RunStep::Continue
                }
            };
            match step {
                RunStep::Continue => {}
                RunStep::Pause => should_pause = true,
            }
        }

        if should_pause {
            return Ok(());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunStep {
    Continue,
    Pause,
}

#[allow(clippy::too_many_arguments)]
async fn execute_ready_node<A, R>(
    pool: &SqlitePool,
    run_id: Uuid,
    graph: &WorkflowGraph,
    node: &WorkflowNode,
    iteration: i64,
    issue_id: Uuid,
    workspace_id: Uuid,
    run_input_text: &str,
    agent_executor: &A,
    arena_creator: &R,
) -> Result<RunStep, ApiError>
where
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
{
    let context = match batch_runtime::execution_context(pool, graph, run_id, node, iteration).await
    {
        Ok(Some(context)) => context,
        Ok(None) => {
            match node_context(pool, graph, run_id, node, iteration, run_input_text).await {
                Ok(context) => context,
                Err(error) => {
                    mark_node_failed(pool, run_id, &node.id, iteration, &error.to_string()).await?;
                    return Ok(RunStep::Continue);
                }
            }
        }
        Err(error) => {
            mark_node_failed(pool, run_id, &node.id, iteration, &error.to_string()).await?;
            return Ok(RunStep::Continue);
        }
    };
    match node.kind {
        WorkflowNodeKind::TaskSplitter => {
            execute_task_splitter(
                pool,
                run_id,
                graph,
                node,
                iteration,
                workspace_id,
                context,
                agent_executor,
            )
            .await
        }
        WorkflowNodeKind::Agent => {
            let prompt = format!(
                "{}{}",
                render_agent_prompt(node, &context),
                file_handoff::output_contract(pool, run_id, &node.id, iteration).await?
            );
            let session_id = node_execution_session_id(pool, run_id, &node.id, iteration).await?;
            let (orchestration_run_id, orchestration_node_execution_id) =
                canonical_node_identity(pool, run_id, &node.id, iteration).await?;
            mark_node_running(pool, run_id, &node.id, iteration, Some(&prompt)).await?;
            match agent_executor
                .run_agent(AgentNodeRequest {
                    run_id,
                    orchestration_run_id,
                    orchestration_node_execution_id,
                    iteration,
                    node_id: node.id.clone(),
                    session_id,
                    workspace_id,
                    prompt,
                    // Workflow Skills are resolved from Agent Center/session capabilities,
                    // not persisted on Node authoring data.
                    selected_skills: None,
                    executor_config: node.data.executor_config.clone(),
                })
                .await
            {
                Ok(AgentNodeExecution::Completed {
                    session_id,
                    orchestration_node_execution_id,
                    agent_run_id,
                    output_text,
                }) => {
                    let execution_id =
                        file_handoff::execution_id(pool, run_id, &node.id, iteration).await?;
                    if let Err(error) = file_handoff::accept(pool, run_id, execution_id, true).await
                    {
                        mark_canonical_node_failed(
                            pool,
                            run_id,
                            &node.id,
                            iteration,
                            Some(session_id),
                            orchestration_node_execution_id,
                            agent_run_id,
                            &error.to_string(),
                        )
                        .await?;
                        return Ok(RunStep::Continue);
                    }
                    mark_canonical_node_succeeded(
                        pool,
                        run_id,
                        &node.id,
                        iteration,
                        Some(&output_text),
                        Some(session_id),
                        orchestration_node_execution_id,
                        agent_run_id,
                    )
                    .await?;
                    Ok(RunStep::Continue)
                }
                Ok(AgentNodeExecution::Started {
                    session_id,
                    orchestration_node_execution_id,
                    agent_run_id,
                    output_text,
                }) => {
                    update_node_execution(
                        pool,
                        run_id,
                        &node.id,
                        iteration,
                        NodeExecutionUpdate {
                            status: DbNodeExecutionStatus::Running,
                            input_text: None,
                            output_text: output_text.as_deref(),
                            session_id: Some(session_id),
                            orchestration_node_execution_id: Some(orchestration_node_execution_id),
                            agent_run_id: Some(agent_run_id),
                            arena_group_id: None,
                            error_text: None,
                            finished: false,
                        },
                    )
                    .await?;
                    Ok(RunStep::Pause)
                }
                Err(err) => {
                    let message = err.to_string();
                    mark_node_failed(pool, run_id, &node.id, iteration, &message).await?;
                    Ok(RunStep::Continue)
                }
            }
        }
        WorkflowNodeKind::Condition => {
            let pre_worktree_snapshot = workflow_worktree_snapshot(pool, workspace_id).await?;
            let upstream_nodes = router_upstream_nodes(pool, graph, run_id, node).await?;
            let prompt = build_router_prompt(
                &run_id.to_string(),
                graph,
                node,
                &context.run_input_text,
                &upstream_nodes,
                pre_worktree_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.summary.as_str()),
            )
            .map_err(|err| ApiError::BadRequest(err.to_string()))?;
            let router_session_id =
                router_session_id_for_run(pool, run_id, workspace_id, graph).await?;
            let (orchestration_run_id, orchestration_node_execution_id) =
                canonical_node_identity(pool, run_id, &node.id, iteration).await?;
            let router_executor_config = graph.router_executor_config.clone().ok_or_else(|| {
                ApiError::BadRequest(
                    "Workflow with Condition nodes requires router executor config".to_string(),
                )
            })?;
            let initial_agent_run_id = stable_workflow_identity(
                orchestration_run_id,
                orchestration_node_execution_id,
                iteration,
                "agent-run",
            );
            let started_output =
                router_started_output_payload(initial_agent_run_id, pre_worktree_snapshot.as_ref());
            update_node_execution(
                pool,
                run_id,
                &node.id,
                iteration,
                NodeExecutionUpdate {
                    status: DbNodeExecutionStatus::Running,
                    input_text: Some(&prompt),
                    output_text: Some(&started_output),
                    session_id: Some(router_session_id),
                    orchestration_node_execution_id: Some(orchestration_node_execution_id),
                    agent_run_id: None,
                    arena_group_id: None,
                    error_text: None,
                    finished: false,
                },
            )
            .await?;
            match agent_executor
                .run_agent(AgentNodeRequest {
                    run_id,
                    orchestration_run_id,
                    orchestration_node_execution_id,
                    iteration,
                    node_id: node.id.clone(),
                    session_id: Some(router_session_id),
                    workspace_id,
                    prompt,
                    selected_skills: None,
                    executor_config: Some(router_executor_config),
                })
                .await
            {
                Ok(AgentNodeExecution::Completed {
                    session_id,
                    orchestration_node_execution_id,
                    agent_run_id,
                    output_text,
                }) => {
                    let started_output =
                        router_started_output_payload(agent_run_id, pre_worktree_snapshot.as_ref());
                    update_node_execution(
                        pool,
                        run_id,
                        &node.id,
                        iteration,
                        NodeExecutionUpdate {
                            status: DbNodeExecutionStatus::Running,
                            input_text: None,
                            output_text: Some(&started_output),
                            session_id: Some(session_id),
                            orchestration_node_execution_id: Some(orchestration_node_execution_id),
                            agent_run_id: Some(agent_run_id),
                            arena_group_id: None,
                            error_text: None,
                            finished: false,
                        },
                    )
                    .await?;
                    let completion = complete_condition_router(
                        pool,
                        run_id,
                        graph,
                        workspace_id,
                        &node.id,
                        iteration,
                        orchestration_node_execution_id,
                        agent_run_id,
                        Some(&started_output),
                        &output_text,
                    )
                    .await?;
                    if completion.should_pause() {
                        Ok(RunStep::Pause)
                    } else {
                        Ok(RunStep::Continue)
                    }
                }
                Ok(AgentNodeExecution::Started {
                    session_id,
                    orchestration_node_execution_id,
                    agent_run_id,
                    output_text: _,
                }) => {
                    let output_text =
                        router_started_output_payload(agent_run_id, pre_worktree_snapshot.as_ref());
                    update_node_execution(
                        pool,
                        run_id,
                        &node.id,
                        iteration,
                        NodeExecutionUpdate {
                            status: DbNodeExecutionStatus::Running,
                            input_text: None,
                            output_text: Some(&output_text),
                            session_id: Some(session_id),
                            orchestration_node_execution_id: Some(orchestration_node_execution_id),
                            agent_run_id: Some(agent_run_id),
                            arena_group_id: None,
                            error_text: None,
                            finished: false,
                        },
                    )
                    .await?;
                    Ok(RunStep::Pause)
                }
                Err(err) => {
                    let message = err.to_string();
                    mark_node_failed(pool, run_id, &node.id, iteration, &message).await?;
                    Ok(RunStep::Continue)
                }
            }
        }
        WorkflowNodeKind::Arena => {
            let prompt = render_arena_prompt(node, &context);
            mark_node_running(pool, run_id, &node.id, iteration, Some(&prompt)).await?;
            match arena_creator
                .create_arena(ArenaNodeRequest {
                    run_id,
                    node_id: node.id.clone(),
                    iteration,
                    issue_id,
                    main_workspace_id: workspace_id,
                    prompt: prompt.clone(),
                    attempts: arena_attempt_requests(issue_id, run_id, node, &context, &prompt),
                })
                .await
            {
                Ok(ArenaNodeExecution { arena_group_id }) => {
                    update_node_execution(
                        pool,
                        run_id,
                        &node.id,
                        iteration,
                        NodeExecutionUpdate {
                            status: DbNodeExecutionStatus::AwaitingArena,
                            input_text: Some(&prompt),
                            output_text: None,
                            session_id: None,
                            orchestration_node_execution_id: None,
                            agent_run_id: None,
                            arena_group_id: Some(arena_group_id),
                            error_text: None,
                            finished: false,
                        },
                    )
                    .await?;
                    update_run_status(
                        pool,
                        run_id,
                        WorkflowRunStatus::AwaitingArena,
                        None,
                        None,
                        false,
                    )
                    .await?;
                    Ok(RunStep::Pause)
                }
                Err(err) => {
                    let message = err.to_string();
                    mark_node_failed(pool, run_id, &node.id, iteration, &message).await?;
                    Ok(RunStep::Continue)
                }
            }
        }
        WorkflowNodeKind::Transform => {
            mark_node_running(
                pool,
                run_id,
                &node.id,
                iteration,
                Some(&context.upstream_text()),
            )
            .await?;
            match file_handoff::transform(pool, graph, run_id, node, iteration).await {
                Ok(path) => {
                    mark_node_succeeded(
                        pool,
                        run_id,
                        &node.id,
                        iteration,
                        Some(&format!("Read workflow handoff: `{path}`")),
                        None,
                    )
                    .await?
                }
                Err(error) => {
                    mark_node_failed(pool, run_id, &node.id, iteration, &error.to_string()).await?
                }
            }
            Ok(RunStep::Continue)
        }
        _ => {
            let input_text = context.upstream_text();
            mark_node_running(pool, run_id, &node.id, iteration, Some(&input_text)).await?;
            let outgoing_edges = outgoing_edges(graph, &node.id);
            match handle_pure_node(node, &outgoing_edges, &context) {
                Ok(outcome) => match outcome.status {
                    NodeHandlerStatus::Succeeded => {
                        let handoff =
                            file_handoff::forward(pool, graph, run_id, node, iteration).await;
                        let handoff = match handoff {
                            Ok(path) => path,
                            Err(error) => {
                                mark_node_failed(
                                    pool,
                                    run_id,
                                    &node.id,
                                    iteration,
                                    &error.to_string(),
                                )
                                .await?;
                                return Ok(RunStep::Continue);
                            }
                        };
                        mark_node_succeeded(
                            pool,
                            run_id,
                            &node.id,
                            iteration,
                            Some(&format!("Read workflow handoff: `{handoff}`")),
                            None,
                        )
                        .await?;
                        mark_skipped_targets(pool, run_id, &outcome.skipped_target_node_ids)
                            .await?;
                        Ok(RunStep::Continue)
                    }
                    NodeHandlerStatus::AwaitingHuman => {
                        let review_prompt = format!(
                            "{}\n\n{}",
                            outcome.prompt.as_deref().unwrap_or("Review workflow files"),
                            context.upstream_text()
                        );
                        update_node_execution(
                            pool,
                            run_id,
                            &node.id,
                            iteration,
                            NodeExecutionUpdate {
                                status: DbNodeExecutionStatus::AwaitingHuman,
                                input_text: Some(&review_prompt),
                                output_text: None,
                                session_id: None,
                                orchestration_node_execution_id: None,
                                agent_run_id: None,
                                arena_group_id: None,
                                error_text: None,
                                finished: false,
                            },
                        )
                        .await?;
                        update_run_status(
                            pool,
                            run_id,
                            WorkflowRunStatus::AwaitingHuman,
                            None,
                            None,
                            false,
                        )
                        .await?;
                        Ok(RunStep::Pause)
                    }
                    NodeHandlerStatus::AwaitingArena => {
                        update_node_execution(
                            pool,
                            run_id,
                            &node.id,
                            iteration,
                            NodeExecutionUpdate {
                                status: DbNodeExecutionStatus::AwaitingArena,
                                input_text: outcome.prompt.as_deref(),
                                output_text: None,
                                session_id: None,
                                orchestration_node_execution_id: None,
                                agent_run_id: None,
                                arena_group_id: None,
                                error_text: None,
                                finished: false,
                            },
                        )
                        .await?;
                        update_run_status(
                            pool,
                            run_id,
                            WorkflowRunStatus::AwaitingArena,
                            None,
                            None,
                            false,
                        )
                        .await?;
                        Ok(RunStep::Pause)
                    }
                },
                Err(err) => {
                    let message = err.to_string();
                    mark_node_failed(pool, run_id, &node.id, iteration, &message).await?;
                    Ok(RunStep::Continue)
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_task_splitter<A>(
    pool: &SqlitePool,
    run_id: Uuid,
    graph: &WorkflowGraph,
    node: &WorkflowNode,
    iteration: i64,
    workspace_id: Uuid,
    context: NodeHandlerContext,
    agent_executor: &A,
) -> Result<RunStep, ApiError>
where
    A: WorkflowAgentExecutor,
{
    let execution_id = file_handoff::execution_id(pool, run_id, &node.id, iteration).await?;
    let prompt = render_task_splitter_prompt(node, &context, run_id, execution_id);
    let session_id = node_execution_session_id(pool, run_id, &node.id, iteration).await?;
    let (orchestration_run_id, orchestration_node_execution_id) =
        canonical_node_identity(pool, run_id, &node.id, iteration).await?;
    mark_node_running(pool, run_id, &node.id, iteration, Some(&prompt)).await?;
    let result = agent_executor
        .run_agent(AgentNodeRequest {
            run_id,
            orchestration_run_id,
            orchestration_node_execution_id,
            iteration,
            node_id: node.id.clone(),
            session_id,
            workspace_id,
            prompt,
            selected_skills: None,
            executor_config: node.data.executor_config.clone(),
        })
        .await;
    match result {
        Ok(AgentNodeExecution::Started {
            session_id,
            orchestration_node_execution_id,
            agent_run_id,
            output_text,
        }) => {
            update_node_execution(
                pool,
                run_id,
                &node.id,
                iteration,
                NodeExecutionUpdate {
                    status: DbNodeExecutionStatus::Running,
                    input_text: None,
                    output_text: output_text.as_deref(),
                    session_id: Some(session_id),
                    orchestration_node_execution_id: Some(orchestration_node_execution_id),
                    agent_run_id: Some(agent_run_id),
                    arena_group_id: None,
                    error_text: None,
                    finished: false,
                },
            )
            .await?;
            Ok(RunStep::Pause)
        }
        Ok(AgentNodeExecution::Completed {
            session_id,
            orchestration_node_execution_id,
            agent_run_id,
            output_text,
        }) => {
            if let Err(error) =
                accept_split_manifest(pool, run_id, graph, node, iteration, workspace_id).await
            {
                let message = error.to_string();
                mark_canonical_node_failed(
                    pool,
                    run_id,
                    &node.id,
                    iteration,
                    Some(session_id),
                    orchestration_node_execution_id,
                    agent_run_id,
                    &message,
                )
                .await?;
                return Ok(RunStep::Continue);
            }
            mark_canonical_node_succeeded(
                pool,
                run_id,
                &node.id,
                iteration,
                Some(&output_text),
                Some(session_id),
                orchestration_node_execution_id,
                agent_run_id,
            )
            .await?;
            Ok(RunStep::Continue)
        }
        Err(error) => {
            let message = error.to_string();
            mark_node_failed(pool, run_id, &node.id, iteration, &message).await?;
            Ok(RunStep::Continue)
        }
    }
}

fn render_task_splitter_prompt(
    node: &WorkflowNode,
    context: &NodeHandlerContext,
    run_id: Uuid,
    execution_id: Uuid,
) -> String {
    let requested = render_prompt_template(
        node.data.prompt_template.as_deref().unwrap_or_default(),
        context,
    );
    let directory = format!(".vibe-kanban/workflows/{run_id}/splits/{execution_id}");
    let control = format!("{directory}/tasks.json");
    let example = json!({"schema_version":1,"run_id":run_id,"split_execution_id":execution_id,
        "tasks":[{"subtask_id":"analysis","title":"Analyze the materials",
            "instruction_path":format!("{directory}/tasks/analysis.md"),"depends_on":[]}]})
    .to_string();
    let contract = format!(
        "Read the referenced files with native file tools. Write a complete UTF-8 instruction file per internal subtask, describing material paths, the work and required deliverables. Each instruction must require publishing the handoff using the executor's own Required file delivery Runtime envelope. Do not invent a handoff schema, completion status, hashes or execution IDs: downstream execution identities and the exact handoff contract are provided by Runtime when that subtask launches. Atomically publish `{control}` with this schema and these exact source identities:\n{example}\nUse stable ASCII subtask_id values and only project-relative file paths. depends_on refers to earlier required subtask IDs; it must be acyclic. Do not create VB business Tasks, invoke project task creation tools, or edit project.json. Do not assign Agents or start downstream executions: the main Agent submits the dispatch plan after runtime validation. A chat reply is not a split result."
    );
    render_workflow_agent_envelope(
        "Task splitter",
        node,
        &format!("{requested}\n\n{contract}"),
        context,
    )
}

pub(crate) async fn workflow_control_root(
    pool: &SqlitePool,
    workspace_id: Uuid,
) -> Result<PathBuf, ApiError> {
    let mut conn = pool.acquire().await?;
    workflow_control_root_in(&mut conn, workspace_id).await
}

pub(crate) async fn workflow_control_root_in(
    conn: &mut SqliteConnection,
    workspace_id: Uuid,
) -> Result<PathBuf, ApiError> {
    let workspace: Option<(Option<String>, String)> =
        sqlx::query_as("SELECT container_ref,workspace_kind FROM workspaces WHERE id=?")
            .bind(workspace_id)
            .fetch_optional(&mut *conn)
            .await?;
    let (root, kind) =
        workspace.ok_or_else(|| ApiError::Conflict("Workflow workspace is unavailable".into()))?;
    let root = root
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            ApiError::Conflict("Workflow workspace has no local control-file root".into())
        })?;
    // Git DirectFolder containers point to the parent of the chosen repo.
    // Match Session::resolve_agent_working_dir, using this transaction's view.
    let repos: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT r.name,r.default_working_dir FROM workspace_repos w JOIN repos r ON r.id=w.repo_id WHERE w.workspace_id=? ORDER BY r.display_name",
    )
        .bind(workspace_id).fetch_all(&mut *conn).await?;
    let path = if let [(name, subdirectory)] = repos.as_slice() {
        let path = root.join(name);
        if kind == "direct_folder" {
            path
        } else {
            subdirectory
                .as_deref()
                .filter(|dir| !dir.is_empty())
                .map_or(path.clone(), |dir| path.join(dir))
        }
    } else {
        root.clone()
    };
    let unavailable = || ApiError::Conflict("Workflow working directory is unavailable".into());
    let canonical_root = tokio::fs::canonicalize(&root)
        .await
        .map_err(|_| unavailable())?;
    let canonical = tokio::fs::canonicalize(path)
        .await
        .map_err(|_| unavailable())?;
    if !canonical.starts_with(&canonical_root)
        || !tokio::fs::metadata(&canonical)
            .await
            .map_err(|_| unavailable())?
            .is_dir()
    {
        return Err(ApiError::Conflict(
            "Workflow working directory leaves its workspace".into(),
        ));
    }
    Ok(canonical)
}

async fn accept_split_manifest(
    pool: &SqlitePool,
    run_id: Uuid,
    graph: &WorkflowGraph,
    node: &WorkflowNode,
    iteration: i64,
    workspace_id: Uuid,
) -> Result<(), ApiError> {
    let node_execution_id = file_handoff::execution_id(pool, run_id, &node.id, iteration).await?;
    let root = workflow_control_root(pool, workspace_id).await?;
    let manifest_path =
        format!(".vibe-kanban/workflows/{run_id}/splits/{node_execution_id}/tasks.json");
    let reference = file_handoff::bind_file(&root, &manifest_path).await?;
    let bytes = file_handoff::read_bound_file(&root, &reference).await?;
    let manifest: SplitTaskManifest = serde_json::from_slice(&bytes)
        .map_err(|error| file_handoff::file_error(&manifest_path, error))?;
    validate_manifest(&manifest).map_err(ApiError::BadRequest)?;
    if manifest.run_id != run_id.to_string()
        || manifest.split_execution_id != node_execution_id.to_string()
    {
        return Err(ApiError::BadRequest(
            "Splitter manifest identity does not match this Run and NodeExecution".into(),
        ));
    }
    dispatch_scope(graph, &node.id).map_err(ApiError::BadRequest)?;
    let mut instructions = Vec::new();
    for task in &manifest.tasks {
        let instruction = file_handoff::bind_file(&root, &task.instruction_path).await?;
        let bytes = file_handoff::read_bound_file(&root, &instruction).await?;
        let text = std::str::from_utf8(&bytes).map_err(|_| {
            ApiError::BadRequest(format!(
                "Subtask `{}` instruction must be UTF-8 text",
                task.subtask_id
            ))
        })?;
        if text.trim().is_empty() {
            return Err(ApiError::BadRequest(format!(
                "Subtask `{}` instruction is empty",
                task.subtask_id
            )));
        }
        instructions.push(instruction);
    }
    let mut artifacts = vec![manifest_path.clone()];
    artifacts.extend(
        instructions
            .iter()
            .map(|instruction| instruction.path.clone()),
    );
    file_handoff::publish(pool, run_id, node_execution_id, None, artifacts).await?;
    let batch_id = node_execution_id;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_runs r WHERE r.id=? AND r.status IN ('running','awaiting_human','awaiting_arena') AND NOT EXISTS(SELECT 1 FROM workflow_stop_intents s WHERE s.run_id=r.id))")
        .bind(run_id).fetch_one(&mut *tx).await?;
    if !active {
        // Keep a natural successful child outcome in a stop race, but never
        // accept new dispatch work or authorize a main-Agent turn after stop.
        return Ok(());
    }
    let existing: Option<(String,String)> = sqlx::query_as("SELECT manifest_path,manifest_sha256 FROM workflow_split_batches WHERE run_id=? AND node_execution_id=?")
        .bind(run_id).bind(node_execution_id).fetch_optional(&mut *tx).await?;
    if let Some((path, sha)) = existing {
        if path != manifest_path || sha != reference.sha256 {
            return Err(ApiError::Conflict(
                "Split manifest changed after acceptance".into(),
            ));
        }
        for (task, instruction) in manifest.tasks.iter().zip(&instructions) {
            let bound: Option<(String,String)> = sqlx::query_as("SELECT instruction_path,instruction_sha256 FROM workflow_split_subtasks WHERE batch_id=? AND subtask_id=?")
                .bind(batch_id).bind(&task.subtask_id).fetch_optional(&mut *tx).await?;
            if bound != Some((task.instruction_path.clone(), instruction.sha256.clone())) {
                return Err(ApiError::Conflict(
                    "Split task instruction changed after acceptance".into(),
                ));
            }
        }
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO workflow_split_batches (id,run_id,node_execution_id,manifest_path,manifest_sha256,state,accepted_at) VALUES (?,?,?,?,?,'accepted',datetime('now','subsec'))",
    )
    .bind(batch_id)
    .bind(run_id)
    .bind(node_execution_id)
    .bind(&manifest_path)
    .bind(&reference.sha256)
    .execute(&mut *tx)
    .await?;
    for (task, instruction) in manifest.tasks.iter().zip(&instructions) {
        sqlx::query(
            "INSERT INTO workflow_split_subtasks (id,batch_id,subtask_id,title,instruction_path,instruction_sha256,depends_on_json) VALUES (?,?,?,?,?,?,?)",
        )
        .bind(Uuid::new_v4())
        .bind(batch_id)
        .bind(&task.subtask_id)
        .bind(&task.title)
        .bind(&task.instruction_path)
        .bind(&instruction.sha256)
        .bind(serde_json::to_string(&task.depends_on).map_err(|error| ApiError::BadRequest(error.to_string()))?)
        .execute(&mut *tx)
        .await?;
    }
    let main_session_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT a.main_session_id FROM workflow_runs r JOIN workflow_attempts a ON a.id=r.attempt_id WHERE r.id=?",
    )
    .bind(run_id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    if let Some(session_id) = main_session_id {
        // Event and accepted tasks commit together: restart cannot lose the
        // authorized handover between split acceptance and event publication.
        sqlx::query("INSERT INTO workflow_handover_events(id,batch_id,run_id,session_id,state) VALUES (?,?,?,?,'pending')")
            .bind(batch_id).bind(batch_id).bind(run_id).bind(session_id).execute(&mut *tx).await?;
    } else {
        return Err(ApiError::Conflict(
            "Task splitter has no original main Session binding".into(),
        ));
    }
    tx.commit().await?;
    Ok(())
}

async fn node_execution_session_id(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    iteration: i64,
) -> Result<Option<Uuid>, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT session_id
        FROM node_executions
        WHERE run_id = ? AND node_id = ? AND iteration = ?
        "#,
    )
    .bind(run_id)
    .bind(node_id)
    .bind(iteration)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        ApiError::BadRequest(format!(
            "Workflow node execution `{node_id}` iteration {iteration} was not found"
        ))
    })
}

async fn node_context(
    pool: &SqlitePool,
    graph: &WorkflowGraph,
    run_id: Uuid,
    node: &WorkflowNode,
    iteration: i64,
    _run_input_text: &str,
) -> Result<NodeHandlerContext, ApiError> {
    file_handoff::context(pool, graph, run_id, node, iteration).await
}

fn render_agent_prompt(node: &WorkflowNode, context: &NodeHandlerContext) -> String {
    let node_prompt = render_prompt_template(
        node.data.prompt_template.as_deref().unwrap_or_default(),
        context,
    );
    if node.data.include_workflow_context.unwrap_or(true) {
        render_workflow_agent_envelope("Agent step", node, &node_prompt, context)
    } else {
        format!(
            "{node_prompt}\n\n{}\n{}",
            context.run_input_text,
            context.upstream_text()
        )
    }
}

fn render_arena_prompt(node: &WorkflowNode, context: &NodeHandlerContext) -> String {
    let node_prompt = render_prompt_template(
        node.data.prompt_template.as_deref().unwrap_or_default(),
        context,
    );
    render_workflow_agent_envelope("Arena step", node, &node_prompt, context)
}

fn node_session_id(node: &WorkflowNode) -> Result<Option<Uuid>, String> {
    node.data
        .session_id
        .as_deref()
        .map(|session_id| {
            Uuid::parse_str(session_id).map_err(|err| {
                format!(
                    "Workflow node `{}` has invalid session_id `{session_id}`: {err}",
                    node.id
                )
            })
        })
        .transpose()
}

fn render_prompt_template(template: &str, context: &NodeHandlerContext) -> String {
    let upstream = context.upstream_text();
    template
        .replace("{{input}}", &context.run_input_text)
        .replace("{{run_input}}", &context.run_input_text)
        .replace("{{upstream}}", &upstream)
}

fn render_workflow_agent_envelope(
    node_type_label: &str,
    node: &WorkflowNode,
    node_prompt: &str,
    context: &NodeHandlerContext,
) -> String {
    let upstream_handoff = context
        .upstream_outputs
        .iter()
        .map(|upstream| WorkflowEnvelopeUpstream {
            heading: Cow::Borrowed(upstream.node_id.as_str()),
            body: Cow::Borrowed(upstream.output_text.as_str()),
        })
        .collect::<Vec<_>>();
    let node_name = node_label_for_workflow_envelope(node);
    let handoff_contract = "- Work in the shared workflow workspace/worktree for this run.\n\
- Treat direct upstream handoff as prior workflow context, not as a new user request.\n\
- Questions are allowed when needed; make the question specific and explain what decision or missing input blocks progress.\n\
- When you finish, include a concise handoff for downstream nodes: what changed or was decided, important files/areas, risks, and recommended next step.";

    render_envelope(WorkflowAgentEnvelope {
        node_type_label,
        node_name: &node_name,
        node_id: &node.id,
        workflow_input: &context.run_input_text,
        upstream_handoff: &upstream_handoff,
        node_task: node_prompt,
        handoff_contract,
    })
}

fn node_label_for_workflow_envelope(node: &WorkflowNode) -> String {
    node.data
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(&node.id)
        .to_string()
}

fn arena_attempt_requests(
    issue_id: Uuid,
    run_id: Uuid,
    node: &WorkflowNode,
    context: &NodeHandlerContext,
    fallback_prompt: &str,
) -> Vec<ArenaNodeAttemptRequest> {
    let attempts = node
        .data
        .attempts
        .clone()
        .filter(|attempts| !attempts.is_empty())
        .unwrap_or_else(|| vec![Default::default(), Default::default()]);

    attempts
        .into_iter()
        .enumerate()
        .map(|(idx, attempt)| {
            let prompt = attempt
                .prompt_template
                .as_deref()
                .map(|template| {
                    let node_prompt = render_prompt_template(template, context);
                    render_workflow_agent_envelope("Arena candidate", node, &node_prompt, context)
                })
                .unwrap_or_else(|| fallback_prompt.to_string());

            ArenaNodeAttemptRequest {
                attempt_id: attempt.id.clone(),
                display_name: attempt.display_name.clone(),
                branch_name: workflow_arena_branch_name(issue_id, run_id, idx + 1),
                prompt,
                executor_config: attempt.executor_config.clone(),
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorktreeSnapshot {
    summary: String,
    fingerprint: String,
}

const MAX_WORKTREE_STATUS_SUMMARY_ENTRIES_PER_REPO: usize = 120;

async fn router_session_id_for_run(
    pool: &SqlitePool,
    run_id: Uuid,
    workspace_id: Uuid,
    graph: &WorkflowGraph,
) -> Result<Uuid, ApiError> {
    if let Some(session_id) = sqlx::query_scalar::<_, Option<Uuid>>(
        r#"
        SELECT session_id
        FROM node_executions
        WHERE run_id = ?
          AND node_type = 'condition'
          AND session_id IS NOT NULL
        ORDER BY rowid ASC
        LIMIT 1
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    .flatten()
    {
        return Ok(session_id);
    }

    let router_executor_config = graph.router_executor_config.clone().ok_or_else(|| {
        ApiError::BadRequest(
            "Workflow with Condition nodes requires router executor config".to_string(),
        )
    })?;
    let executor_config = executor_config_from_node(Some(router_executor_config)).await?;
    let session = Session::create(
        pool,
        &CreateSession {
            executor: Some(executor_config.profile_id().executor.to_string()),
            name: Some("Workflow Router".to_string()),
        },
        Uuid::new_v4(),
        workspace_id,
    )
    .await?;

    Ok(session.id)
}

async fn router_upstream_nodes(
    pool: &SqlitePool,
    graph: &WorkflowGraph,
    run_id: Uuid,
    node: &WorkflowNode,
) -> Result<Vec<RouterUpstreamNode>, ApiError> {
    let mut upstream_nodes = Vec::new();
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    let root = workflow_control_root(pool, workspace_id).await?;
    for edge in graph.edges.iter().filter(|edge| edge.target == node.id) {
        let source_node = graph
            .nodes
            .iter()
            .find(|candidate| candidate.id == edge.source);
        let rows = sqlx::query(
            r#"
            SELECT source_node_execution_id, status, error_text
            FROM workflow_effective_node_executions
            WHERE run_id = ? AND node_id = ?
            ORDER BY iteration ASC
            "#,
        )
        .bind(run_id)
        .bind(&edge.source)
        .fetch_all(pool)
        .await?;
        for row in &rows {
            let status: String = row.try_get("status")?;
            let output_text = if status == "succeeded" {
                let execution_id: Uuid = row.try_get("source_node_execution_id")?;
                let (index, files) = file_handoff::verified(pool, &root, execution_id).await?;
                Some(format!(
                    "Read source execution {execution_id} handoff `{}` and referenced files: {}",
                    index.path,
                    files
                        .iter()
                        .map(|file| format!("`{}`", file.path))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            } else {
                None
            };
            upstream_nodes.push(RouterUpstreamNode {
                node_id: edge.source.clone(),
                node_type: source_node
                    .map(|source| node_kind_value(&source.kind).to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                status,
                output_text,
                error_text: row.try_get("error_text")?,
            });
        }
        if rows.is_empty() {
            let skipped:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_node_dispositions WHERE run_id=? AND node_id=?)")
                .bind(run_id).bind(&edge.source).fetch_one(pool).await?;
            upstream_nodes.push(RouterUpstreamNode {
                node_id: edge.source.clone(),
                node_type: source_node
                    .map(|source| node_kind_value(&source.kind).to_string())
                    .unwrap_or_else(|| "unknown".into()),
                status: if skipped { "skipped" } else { "pending" }.into(),
                output_text: None,
                error_text: None,
            });
        }
    }

    Ok(upstream_nodes)
}

async fn workflow_worktree_snapshot(
    pool: &SqlitePool,
    workspace_id: Uuid,
) -> Result<Option<WorktreeSnapshot>, ApiError> {
    let workspace = Workspace::find_by_id(pool, workspace_id)
        .await?
        .ok_or(ApiError::Workspace(WorkspaceError::WorkspaceNotFound))?;
    let Some(container_ref) = workspace
        .container_ref
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let repos =
        WorkspaceRepo::find_repos_with_target_branch_for_workspace(pool, workspace_id).await?;
    if repos.is_empty() {
        return Ok(None);
    }

    let workspace_root = PathBuf::from(container_ref);
    let git = GitCli::new();
    let mut summary_lines = Vec::new();
    let mut fingerprint_lines = Vec::new();

    for repo in repos {
        let repo_path = workspace_root.join(&repo.repo.name);
        match git.get_worktree_status(&repo_path) {
            Ok(status) => {
                summary_lines.push(render_worktree_status_summary(&repo.repo.name, &status));
                fingerprint_lines.push(format!(
                    "repo={};tracked={};untracked={}",
                    repo.repo.name, status.uncommitted_tracked, status.untracked
                ));
                for entry in status.entries {
                    let path = String::from_utf8_lossy(&entry.path);
                    let orig_path = entry
                        .orig_path
                        .as_ref()
                        .map(|path| String::from_utf8_lossy(path).to_string())
                        .unwrap_or_default();
                    fingerprint_lines.push(format!(
                        "{}{}:{}:{}",
                        entry.staged, entry.unstaged, path, orig_path
                    ));
                }
            }
            Err(err) => {
                summary_lines.push(format!("{}: status unavailable ({err})", repo.repo.name));
                fingerprint_lines.push(format!("repo={};status=unavailable:{err}", repo.repo.name));
            }
        }
    }

    Ok(Some(WorktreeSnapshot {
        summary: summary_lines.join("\n"),
        fingerprint: fingerprint_lines.join("\n"),
    }))
}

fn render_worktree_status_summary(repo_name: &str, status: &WorktreeStatus) -> String {
    let mut lines = Vec::new();
    if status.entries.is_empty() {
        lines.push(format!("{repo_name}: clean"));
        return lines.join("\n");
    }

    lines.push(format!(
        "{repo_name}: {} tracked change(s), {} untracked/generated file(s)",
        status.uncommitted_tracked, status.untracked
    ));
    lines.push("  changed/generated files:".to_string());

    for entry in status
        .entries
        .iter()
        .take(MAX_WORKTREE_STATUS_SUMMARY_ENTRIES_PER_REPO)
    {
        lines.push(format!("  - {}", render_status_entry(entry)));
    }
    let omitted = status
        .entries
        .len()
        .saturating_sub(MAX_WORKTREE_STATUS_SUMMARY_ENTRIES_PER_REPO);
    if omitted > 0 {
        lines.push(format!("  - [{omitted} more file(s) omitted]"));
    }

    lines.join("\n")
}

fn render_status_entry(entry: &StatusEntry) -> String {
    let code = status_entry_code(entry);
    let label = status_entry_label(entry);
    let path = String::from_utf8_lossy(&entry.path);
    if let Some(orig_path) = entry.orig_path.as_ref() {
        let orig_path = String::from_utf8_lossy(orig_path);
        format!("{code} {label}: {orig_path} -> {path}")
    } else {
        format!("{code} {label}: {path}")
    }
}

fn status_entry_code(entry: &StatusEntry) -> String {
    if entry.is_untracked {
        "??".to_string()
    } else {
        format!("{}{}", entry.staged, entry.unstaged).replace(' ', ".")
    }
}

fn status_entry_label(entry: &StatusEntry) -> &'static str {
    if entry.is_untracked {
        return "untracked/generated";
    }

    let statuses = [entry.staged, entry.unstaged];
    if statuses.contains(&'R') {
        "renamed"
    } else if statuses.contains(&'C') {
        "copied"
    } else if statuses.contains(&'A') {
        "added"
    } else if statuses.contains(&'D') {
        "deleted"
    } else if statuses.contains(&'M') {
        "modified"
    } else {
        "changed"
    }
}

// ---------------------------------------------------------------------------
// Event-driven workflow completion watcher
//
// The watcher subscribes to canonical AgentRun events. The AgentRun port is
// the lifecycle authority; the durable orchestration inbox records each
// event before the Workflow/Arena product projection is reconciled. This
// keeps background completion, restart replay, and HTTP-triggered reads on
// the same event path. The old execution-process completion hub is not part
// of this lifecycle.
//
// For each event we resolve the owning workflow run through the canonical
// orchestration link and spawn a small task that drains the inbox and
// small task that calls `reconcile_workflow_run_with_arena` — exactly what the
// HTTP handlers do, but driven by the real event rather than polling.
// ---------------------------------------------------------------------------

/// Spawn a background task that advances workflow runs as soon as their
/// canonical AgentRuns emit lifecycle events.
///
/// The task loops forever (until the process exits) consuming canonical
/// AgentRun events. For each subscribed AgentRun it resolves the owning
/// workflow run through the canonical orchestration link, drains durable join
/// facts, then reconciles the parent product run.
///
/// Concurrent reconcile calls for the same run are safe because
/// `reconcile_workflow_run_with_arena` is idempotent — it re-reads state from
/// the DB each time and takes no action if the run is already terminal.
pub fn spawn_workflow_completion_watcher(deployment: DeploymentImpl) {
    tokio::spawn(async move {
        let mut watched_agent_runs = HashSet::new();
        loop {
            let active: Vec<(Uuid, Uuid)> = match sqlx::query_as(
                r#"
                SELECT DISTINCT ne.run_id, links.agent_run_id
                FROM node_executions ne
                JOIN workflow_runs wr ON wr.id = ne.run_id
                JOIN orchestration_node_executions orchestration_nodes
                  ON orchestration_nodes.id = ne.orchestration_node_execution_id
                 AND orchestration_nodes.orchestration_run_id = wr.orchestration_run_id
                JOIN orchestration_agent_run_links links
                  ON links.node_execution_id = orchestration_nodes.id
                 AND links.orchestration_run_id = wr.orchestration_run_id
                JOIN agent_run_state agent_state
                  ON agent_state.agent_run_id = links.agent_run_id
                WHERE (
                    ne.status IN ('running', 'awaiting_human', 'awaiting_arena', 'cancelling')
                    OR agent_state.status IN (
                        'pending', 'starting', 'running',
                        'awaiting_input', 'awaiting_approval', 'cancelling'
                    )
                )
                "#,
            )
            .fetch_all(&deployment.db().pool)
            .await
            {
                Ok(rows) => rows,
                Err(error) => {
                    tracing::warn!("workflow watcher: canonical link lookup failed: {error}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };

            for (run_id, agent_run_id) in active {
                if !watched_agent_runs.insert(agent_run_id) {
                    continue;
                }
                let deployment = deployment.clone();
                tokio::spawn(async move {
                    let pool = deployment.db().pool.clone();
                    let service = OrchestrationService::new(
                        pool.clone(),
                        Arc::new(deployment.agent_run_port().clone()),
                    );
                    let mut events = match service.subscribe_agent_run(agent_run_id).await {
                        Ok(events) => events,
                        Err(error) => {
                            tracing::warn!(
                                %run_id,
                                %agent_run_id,
                                "workflow watcher: cannot subscribe to canonical AgentRun: {error}"
                            );
                            return;
                        }
                    };
                    let boundary =
                        DeploymentAgentRunReconciliationBoundary::new(deployment.clone());
                    while let Some(event) = events.next().await {
                        // Persist each canonical AgentEvent into the
                        // orchestration inbox before reconciling the parent.
                        // The AgentRun port is the source of truth; without
                        // this bridge, join consumption has no durable source
                        // facts after a restart (and no live fan-in at all).
                        if let Some(orchestration_run_id) = event.orchestration_run_id {
                            if let Err(error) = service
                                .ingest_agent_event(orchestration_run_id, &event)
                                .await
                            {
                                tracing::warn!(
                                    %run_id,
                                    %agent_run_id,
                                    %orchestration_run_id,
                                    "workflow watcher: canonical event ingestion failed: {error}"
                                );
                                continue;
                            }
                        }
                        if let Some(orchestration_run_id) = event.orchestration_run_id {
                            // Reconcile the canonical child projection before
                            // consuming the inbox. The durable join consumer
                            // must only create a handoff from a confirmed
                            // successful AgentRun, never from a provider event
                            // that raced its node projection.
                            if let Err(error) = service
                                .reconcile_run(orchestration_run_id, event.correlation_id)
                                .await
                            {
                                tracing::warn!(
                                    %run_id,
                                    %agent_run_id,
                                    %orchestration_run_id,
                                    "workflow watcher: canonical child reconcile before join failed: {error}"
                                );
                            } else if let Err(error) =
                                service.drain_inbox_for_run(orchestration_run_id).await
                            {
                                tracing::warn!(
                                    %run_id,
                                    %agent_run_id,
                                    %orchestration_run_id,
                                    "workflow watcher: durable orchestration inbox drain failed: {error}"
                                );
                            }
                        }
                        let agent_executor =
                            DeploymentWorkflowAgentExecutor::new(deployment.clone());
                        let arena_creator = DeploymentWorkflowArenaCreator::new(deployment.clone());
                        if let Err(error) = reconcile_workflow_run_with_arena_and_boundary(
                            &pool,
                            run_id,
                            &agent_executor,
                            &arena_creator,
                            &boundary,
                        )
                        .await
                        {
                            tracing::warn!(
                                %run_id,
                                %agent_run_id,
                                "workflow watcher: canonical reconcile failed: {error}"
                            );
                        }
                    }
                });
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

fn router_started_output_payload(
    agent_run_id: Uuid,
    pre_worktree_snapshot: Option<&WorktreeSnapshot>,
) -> String {
    serde_json::to_string(&json!({
        "type": "condition_router_run",
        "source": "router",
        "status": "running",
        "schema_version": 1,
        "agent_run_id": agent_run_id,
        "pre_worktree_summary": pre_worktree_snapshot.map(|snapshot| snapshot.summary.as_str()),
        "pre_worktree_fingerprint": pre_worktree_snapshot.map(|snapshot| snapshot.fingerprint.as_str())
    }))
    .unwrap_or_else(|_| "{\"type\":\"condition_router_run\",\"status\":\"running\"}".to_string())
}

fn router_pre_worktree_fingerprint(output_text: Option<&str>) -> Option<String> {
    output_text
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .and_then(|value| {
            value
                .get("pre_worktree_fingerprint")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

async fn router_mutation_warning(
    pool: &SqlitePool,
    workspace_id: Uuid,
    started_output_text: Option<&str>,
) -> Result<Option<String>, ApiError> {
    let Some(before) = router_pre_worktree_fingerprint(started_output_text) else {
        return Ok(None);
    };
    let Some(after) = workflow_worktree_snapshot(pool, workspace_id).await? else {
        return Ok(None);
    };
    if before == after.fingerprint {
        return Ok(None);
    }

    Ok(Some(
        "Worktree status changed while the Condition router was running".to_string(),
    ))
}

#[allow(clippy::too_many_arguments)]
async fn complete_condition_router(
    pool: &SqlitePool,
    run_id: Uuid,
    graph: &WorkflowGraph,
    workspace_id: Uuid,
    node_id: &str,
    iteration: i64,
    orchestration_node_execution_id: Uuid,
    agent_run_id: Uuid,
    started_output_text: Option<&str>,
    raw_output: &str,
) -> Result<ConditionRouterCompletion, ApiError> {
    let condition_node = graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .ok_or_else(|| ApiError::BadRequest(format!("Workflow node `{node_id}` not found")))?;
    if condition_node.kind != WorkflowNodeKind::Condition {
        return Err(ApiError::BadRequest(format!(
            "Workflow node `{node_id}` is not a condition"
        )));
    }

    let mutation_warning = router_mutation_warning(pool, workspace_id, started_output_text).await?;
    let completion = evaluate_router_output(raw_output, graph, condition_node, mutation_warning);
    if completion.should_pause() {
        update_node_execution(
            pool,
            run_id,
            node_id,
            iteration,
            NodeExecutionUpdate {
                status: DbNodeExecutionStatus::AwaitingHuman,
                input_text: completion.pause_prompt.as_deref(),
                output_text: Some(&completion.output_text),
                session_id: None,
                orchestration_node_execution_id: Some(orchestration_node_execution_id),
                agent_run_id: Some(agent_run_id),
                arena_group_id: None,
                error_text: None,
                finished: false,
            },
        )
        .await?;
        update_run_status(
            pool,
            run_id,
            WorkflowRunStatus::AwaitingHuman,
            None,
            None,
            false,
        )
        .await?;
    } else {
        if let Err(error) =
            file_handoff::forward(pool, graph, run_id, condition_node, iteration).await
        {
            mark_canonical_node_failed(
                pool,
                run_id,
                node_id,
                iteration,
                None,
                orchestration_node_execution_id,
                agent_run_id,
                &error.to_string(),
            )
            .await?;
            return Err(error);
        }
        mark_canonical_node_succeeded(
            pool,
            run_id,
            node_id,
            iteration,
            Some(&completion.output_text),
            None,
            orchestration_node_execution_id,
            agent_run_id,
        )
        .await?;
        mark_skipped_targets(pool, run_id, &completion.skipped_target_node_ids).await?;
    }

    Ok(completion)
}

fn workflow_arena_branch_name(issue_id: Uuid, run_id: Uuid, attempt_index: usize) -> String {
    format!(
        "vk/{issue_id}-wf-{}-arena-{attempt_index}",
        short_run_id(run_id)
    )
}

fn outgoing_edges(graph: &WorkflowGraph, node_id: &str) -> Vec<WorkflowEdge> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.source == node_id)
        .cloned()
        .collect()
}

async fn load_run_snapshot(pool: &SqlitePool, run_id: Uuid) -> Result<RunSnapshot, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT node_id, iteration, status, output_text, error_text
        FROM workflow_effective_node_executions
        WHERE run_id = ?
        UNION ALL
        SELECT node_id,0,'skipped',NULL,NULL
        FROM workflow_node_dispositions WHERE run_id = ?
        ORDER BY node_id,iteration
        "#,
    )
    .bind(run_id)
    .bind(run_id)
    .fetch_all(pool)
    .await?;

    let nodes = rows
        .iter()
        .map(|row| {
            Ok(NodeExecutionSnapshot {
                node_id: row.try_get("node_id")?,
                iteration: row.try_get("iteration")?,
                status: planner_status_from_str(&row.try_get::<String, _>("status")?)?,
                output_text: row.try_get("output_text")?,
                error_text: row.try_get("error_text")?,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;

    Ok(RunSnapshot {
        run_id: run_id.to_string(),
        nodes,
    })
}

fn all_nodes_terminal(snapshot: &RunSnapshot) -> bool {
    !snapshot.nodes.is_empty()
        && snapshot.nodes.iter().all(|node| {
            matches!(
                node.status,
                PlannerNodeExecutionStatus::Succeeded
                    | PlannerNodeExecutionStatus::Cancelled
                    | PlannerNodeExecutionStatus::Skipped
            )
        })
}

async fn final_run_output(
    pool: &SqlitePool,
    run_id: Uuid,
    graph: &WorkflowGraph,
) -> Result<Option<String>, ApiError> {
    for node in graph
        .nodes
        .iter()
        .filter(|node| node.kind == WorkflowNodeKind::End)
    {
        let output = sqlx::query_scalar::<_, Option<String>>(
            r#"
            SELECT output_text
            FROM workflow_effective_node_executions
            WHERE run_id = ? AND node_id = ? AND status = 'succeeded'
            ORDER BY iteration ASC
            LIMIT 1
            "#,
        )
        .bind(run_id)
        .bind(&node.id)
        .fetch_optional(pool)
        .await?
        .flatten();
        if output.is_some() {
            return Ok(output);
        }
    }

    Ok(sqlx::query_scalar::<_, Option<String>>(
        r#"
        SELECT output_text
        FROM workflow_effective_node_executions
        WHERE run_id = ? AND status = 'succeeded'
        ORDER BY iteration DESC,node_id DESC
        LIMIT 1
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    .flatten())
}

async fn mark_node_running(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    iteration: i64,
    input_text: Option<&str>,
) -> Result<(), ApiError> {
    update_node_execution(
        pool,
        run_id,
        node_id,
        iteration,
        NodeExecutionUpdate {
            status: DbNodeExecutionStatus::Running,
            input_text,
            output_text: None,
            session_id: None,
            orchestration_node_execution_id: None,
            agent_run_id: None,
            arena_group_id: None,
            error_text: None,
            finished: false,
        },
    )
    .await
}

async fn mark_node_succeeded(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    iteration: i64,
    output_text: Option<&str>,
    session_id: Option<Uuid>,
) -> Result<(), ApiError> {
    update_node_execution(
        pool,
        run_id,
        node_id,
        iteration,
        NodeExecutionUpdate {
            status: DbNodeExecutionStatus::Succeeded,
            input_text: None,
            output_text,
            session_id,
            orchestration_node_execution_id: None,
            agent_run_id: None,
            arena_group_id: None,
            error_text: None,
            finished: true,
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn mark_canonical_node_succeeded(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    iteration: i64,
    output_text: Option<&str>,
    session_id: Option<Uuid>,
    orchestration_node_execution_id: Uuid,
    agent_run_id: Uuid,
) -> Result<(), ApiError> {
    update_node_execution(
        pool,
        run_id,
        node_id,
        iteration,
        NodeExecutionUpdate {
            status: DbNodeExecutionStatus::Succeeded,
            input_text: None,
            output_text,
            session_id,
            orchestration_node_execution_id: Some(orchestration_node_execution_id),
            agent_run_id: Some(agent_run_id),
            arena_group_id: None,
            error_text: None,
            finished: true,
        },
    )
    .await
}

async fn mark_node_failed(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    iteration: i64,
    error_text: &str,
) -> Result<(), ApiError> {
    update_node_execution(
        pool,
        run_id,
        node_id,
        iteration,
        NodeExecutionUpdate {
            status: DbNodeExecutionStatus::Failed,
            input_text: None,
            output_text: None,
            session_id: None,
            orchestration_node_execution_id: None,
            agent_run_id: None,
            arena_group_id: None,
            error_text: Some(error_text),
            finished: true,
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn mark_canonical_node_failed(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    iteration: i64,
    session_id: Option<Uuid>,
    orchestration_node_execution_id: Uuid,
    agent_run_id: Uuid,
    error_text: &str,
) -> Result<(), ApiError> {
    update_node_execution(
        pool,
        run_id,
        node_id,
        iteration,
        NodeExecutionUpdate {
            status: DbNodeExecutionStatus::Failed,
            input_text: None,
            output_text: None,
            session_id,
            orchestration_node_execution_id: Some(orchestration_node_execution_id),
            agent_run_id: Some(agent_run_id),
            arena_group_id: None,
            error_text: Some(error_text),
            finished: true,
        },
    )
    .await
}

async fn mark_skipped_targets(
    pool: &SqlitePool,
    run_id: Uuid,
    node_ids: &[String],
) -> Result<(), ApiError> {
    for node_id in node_ids {
        let row = sqlx::query(
            r#"
            SELECT id, iteration
            FROM node_executions
            WHERE run_id = ? AND node_id = ? AND status = 'pending'
            ORDER BY iteration ASC
            LIMIT 1
            "#,
        )
        .bind(run_id)
        .bind(node_id)
        .fetch_optional(pool)
        .await?;

        let Some(row) = row else {
            continue;
        };
        let execution_id: Uuid = row.try_get("id")?;
        let iteration: i64 = row.try_get("iteration")?;
        let result = sqlx::query(
            r#"
            UPDATE node_executions
            SET status = 'skipped',
                finished_at = datetime('now', 'subsec'),
                updated_at = datetime('now', 'subsec')
            WHERE id = ? AND status = 'pending'
            "#,
        )
        .bind(execution_id)
        .execute(pool)
        .await?;
        if result.rows_affected() > 0 {
            emit_node_status(
                run_id,
                node_id,
                DbNodeExecutionStatus::Skipped,
                json!({ "status": "skipped", "iteration": iteration }),
            );
        }
    }

    Ok(())
}

async fn ensure_run_exists(pool: &SqlitePool, run_id: Uuid) -> Result<(), ApiError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_runs WHERE id = ?")
        .bind(run_id)
        .fetch_one(pool)
        .await?;
    if count == 0 {
        return Err(ApiError::BadRequest("Workflow run not found".to_string()));
    }

    Ok(())
}

async fn node_execution_output_text(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    iteration: i64,
) -> Result<Option<String>, ApiError> {
    Ok(sqlx::query_scalar::<_, Option<String>>(
        r#"
        SELECT output_text
        FROM node_executions
        WHERE run_id = ? AND node_id = ? AND iteration = ?
        "#,
    )
    .bind(run_id)
    .bind(node_id)
    .bind(iteration)
    .fetch_optional(pool)
    .await?
    .flatten())
}

async fn running_node_session_ids(pool: &SqlitePool, run_id: Uuid) -> Result<Vec<Uuid>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT session_id
        FROM node_executions
        WHERE run_id = ? AND status = 'running' AND session_id IS NOT NULL
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| row.try_get("session_id"))
        .collect::<Result<Vec<_>, _>>()?)
}

async fn mark_pending_nodes_skipped(pool: &SqlitePool, run_id: Uuid) -> Result<(), ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT id, node_id, iteration
        FROM node_executions
        WHERE run_id = ? AND status = 'pending'
        ORDER BY rowid ASC
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;

    for row in rows {
        let execution_id: Uuid = row.try_get("id")?;
        let node_id: String = row.try_get("node_id")?;
        let iteration: i64 = row.try_get("iteration")?;
        let result = sqlx::query(
            r#"
            UPDATE node_executions
            SET status = 'skipped',
                finished_at = datetime('now', 'subsec'),
                updated_at = datetime('now', 'subsec')
            WHERE id = ? AND status = 'pending'
            "#,
        )
        .bind(execution_id)
        .execute(pool)
        .await?;
        if result.rows_affected() > 0 {
            emit_node_status(
                run_id,
                &node_id,
                DbNodeExecutionStatus::Skipped,
                json!({ "status": "skipped", "iteration": iteration }),
            );
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct NodeExecutionUpdate<'a> {
    status: DbNodeExecutionStatus,
    input_text: Option<&'a str>,
    output_text: Option<&'a str>,
    session_id: Option<Uuid>,
    orchestration_node_execution_id: Option<Uuid>,
    agent_run_id: Option<Uuid>,
    arena_group_id: Option<Uuid>,
    error_text: Option<&'a str>,
    finished: bool,
}

async fn update_node_execution(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    iteration: i64,
    update: NodeExecutionUpdate<'_>,
) -> Result<(), ApiError> {
    let result = sqlx::query(
        r#"
        UPDATE node_executions
        SET status = ?,
            input_text = COALESCE(?, input_text),
            output_text = COALESCE(?, output_text),
            session_id = COALESCE(?, session_id),
            orchestration_node_execution_id = COALESCE(?, orchestration_node_execution_id),
            agent_run_id = COALESCE(?, agent_run_id),
            arena_group_id = COALESCE(?, arena_group_id),
            error_text = ?,
            started_at = COALESCE(started_at, datetime('now', 'subsec')),
            finished_at = CASE WHEN ? THEN datetime('now', 'subsec') ELSE finished_at END,
            updated_at = datetime('now', 'subsec')
        WHERE run_id = ? AND node_id = ? AND iteration = ?
          AND status NOT IN ('succeeded','failed','cancelled','skipped')
          AND (status<>'cancelling' OR ? IN ('cancelling','succeeded','failed','cancelled','skipped'))
        "#,
    )
    .bind(node_status_value(update.status))
    .bind(update.input_text)
    .bind(update.output_text)
    .bind(update.session_id)
    .bind(update.orchestration_node_execution_id)
    .bind(update.agent_run_id)
    .bind(update.arena_group_id)
    .bind(update.error_text)
    .bind(update.finished)
    .bind(run_id)
    .bind(node_id)
    .bind(iteration)
    .bind(node_status_value(update.status))
    .execute(pool)
    .await?;

    if result.rows_affected() > 0 {
        emit_node_update(run_id, node_id, iteration, update);
    }

    Ok(())
}

async fn update_run_status(
    pool: &SqlitePool,
    run_id: Uuid,
    status: WorkflowRunStatus,
    output_text: Option<String>,
    error_text: Option<&str>,
    finished: bool,
) -> Result<(), ApiError> {
    let result = sqlx::query(
        r#"
        UPDATE workflow_runs
        SET status = ?,
            output_text = ?,
            error_text = COALESCE(error_text, ?),
            finished_at = CASE WHEN ? THEN datetime('now', 'subsec') ELSE NULL END,
            updated_at = datetime('now', 'subsec')
        WHERE id = ?
          AND status NOT IN ('succeeded','failed','canceled')
          AND (status NOT IN ('cancelling','canceled') OR ? IN ('cancelling','canceled'))
          AND (error_text IS NULL OR ? IN ('failed','cancelling','canceled'))
        "#,
    )
    .bind(run_status_value(status))
    .bind(output_text.as_deref())
    .bind(error_text)
    .bind(finished)
    .bind(run_id)
    .bind(run_status_value(status))
    .bind(run_status_value(status))
    .execute(pool)
    .await?;

    if result.rows_affected() > 0 {
        emit_run_status(run_id, status, output_text.as_deref(), error_text);
    }

    Ok(())
}

fn emit_run_status(
    run_id: Uuid,
    status: WorkflowRunStatus,
    output_text: Option<&str>,
    error_text: Option<&str>,
) {
    let status_value = run_status_value(status);
    WORKFLOW_EVENT_HUB.publish(WorkflowEvent::run_status(
        run_id.to_string(),
        status_value,
        json!({
            "status": status_value,
            "output_text": output_text,
            "error_text": error_text,
        }),
    ));
}

fn emit_node_update(run_id: Uuid, node_id: &str, iteration: i64, update: NodeExecutionUpdate<'_>) {
    let status_value = node_status_value(update.status);
    emit_node_status(
        run_id,
        node_id,
        update.status,
        json!({
            "status": status_value,
            "iteration": iteration,
            "input_text": update.input_text,
            "output_text": update.output_text,
            "session_id": update.session_id,
            "orchestration_node_execution_id": update.orchestration_node_execution_id,
            "agent_run_id": update.agent_run_id,
            "arena_group_id": update.arena_group_id,
            "error_text": update.error_text,
        }),
    );

    if let Some(output_text) = update.output_text {
        WORKFLOW_EVENT_HUB.publish(WorkflowEvent::node_output(
            run_id.to_string(),
            node_id.to_string(),
            json!({ "output_text": output_text }),
        ));
    }
    if let Some(error_text) = update.error_text {
        WORKFLOW_EVENT_HUB.publish(WorkflowEvent::node_error(
            run_id.to_string(),
            node_id.to_string(),
            json!({ "error_text": error_text }),
        ));
    }
    if update.status == DbNodeExecutionStatus::AwaitingHuman {
        WORKFLOW_EVENT_HUB.publish(WorkflowEvent::node_waiting_human(
            run_id.to_string(),
            node_id.to_string(),
            json!({ "prompt": update.input_text }),
        ));
    }
    if update.status == DbNodeExecutionStatus::AwaitingArena {
        WORKFLOW_EVENT_HUB.publish(WorkflowEvent::node_waiting_arena(
            run_id.to_string(),
            node_id.to_string(),
            json!({
                "prompt": update.input_text,
                "arena_group_id": update.arena_group_id,
            }),
        ));
    }
}

fn emit_node_status(run_id: Uuid, node_id: &str, status: DbNodeExecutionStatus, payload: Value) {
    WORKFLOW_EVENT_HUB.publish(WorkflowEvent::node_status(
        run_id.to_string(),
        node_id.to_string(),
        node_status_value(status),
        payload,
    ));
}

async fn node_execution_responses(
    pool: &SqlitePool,
    run_id: Uuid,
) -> Result<Vec<WorkflowNodeExecutionResponse>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT ne.id, ne.run_id, ne.task_id, ne.node_id, ne.node_type, ne.iteration, ne.status, ne.input_text, ne.output_text,
               ne.session_id, ne.orchestration_node_execution_id, ne.agent_run_id,
               ars.projection_status AS projection_status,
               ne.execution_process_id, ne.arena_group_id, ne.tokens_used, ne.cost_estimate,
               ne.started_at, ne.finished_at, ne.error_text, ne.created_at, ne.updated_at
        FROM node_executions ne
        LEFT JOIN agent_run_state ars ON ars.agent_run_id = ne.agent_run_id
        WHERE ne.run_id = ?
        ORDER BY ne.rowid ASC
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(node_execution_response_from_row)
        .collect::<Result<Vec<_>, _>>()?)
}

fn node_execution_response_from_row(
    row: &SqliteRow,
) -> Result<WorkflowNodeExecutionResponse, WorkflowRuntimeError> {
    Ok(WorkflowNodeExecutionResponse {
        id: row.try_get("id")?,
        run_id: row.try_get("run_id")?,
        task_id: row.try_get("task_id")?,
        node_id: row.try_get("node_id")?,
        node_type: row.try_get("node_type")?,
        iteration: row.try_get("iteration")?,
        status: node_execution_status_from_str(&row.try_get::<String, _>("status")?)?,
        input_text: row.try_get("input_text")?,
        output_text: row.try_get("output_text")?,
        session_id: row.try_get("session_id")?,
        orchestration_node_execution_id: row.try_get("orchestration_node_execution_id")?,
        agent_run_id: row.try_get("agent_run_id")?,
        projection_status: row.try_get("projection_status")?,
        execution_process_id: row.try_get("execution_process_id")?,
        arena_group_id: row.try_get("arena_group_id")?,
        tokens_used: row.try_get("tokens_used")?,
        cost_estimate: row.try_get("cost_estimate")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        error_text: row.try_get("error_text")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn executor_config_from_node(value: Option<Value>) -> Result<ExecutorConfig, ApiError> {
    if let Some(value) = value {
        return serde_json::from_value(value).map_err(|err| {
            ApiError::BadRequest(format!("Invalid workflow agent executor config: {err}"))
        });
    }

    let profile_id = ExecutorConfigs::get_cached()
        .get_recommended_executor_profile()
        .await
        .map_err(|err| {
            ApiError::BadRequest(format!(
                "No available executor profile for workflow agent: {err}"
            ))
        })?;
    Ok(ExecutorConfig::from(profile_id))
}

fn workflow_run_status_from_str(value: &str) -> Result<WorkflowRunStatus, WorkflowRuntimeError> {
    match value {
        "pending" => Ok(WorkflowRunStatus::Pending),
        "running" => Ok(WorkflowRunStatus::Running),
        "awaiting_human" => Ok(WorkflowRunStatus::AwaitingHuman),
        "awaiting_arena" => Ok(WorkflowRunStatus::AwaitingArena),
        "cancelling" => Ok(WorkflowRunStatus::Cancelling),
        "succeeded" => Ok(WorkflowRunStatus::Succeeded),
        "failed" => Ok(WorkflowRunStatus::Failed),
        "canceled" => Ok(WorkflowRunStatus::Canceled),
        other => Err(WorkflowRuntimeError::BadRequest(format!(
            "Unknown workflow run status `{other}`"
        ))),
    }
}

fn node_execution_status_from_str(
    value: &str,
) -> Result<DbNodeExecutionStatus, WorkflowRuntimeError> {
    match value {
        "pending" => Ok(DbNodeExecutionStatus::Pending),
        "running" => Ok(DbNodeExecutionStatus::Running),
        "awaiting_human" => Ok(DbNodeExecutionStatus::AwaitingHuman),
        "awaiting_arena" => Ok(DbNodeExecutionStatus::AwaitingArena),
        "cancelling" => Ok(DbNodeExecutionStatus::Cancelling),
        "succeeded" => Ok(DbNodeExecutionStatus::Succeeded),
        "failed" => Ok(DbNodeExecutionStatus::Failed),
        "cancelled" => Ok(DbNodeExecutionStatus::Cancelled),
        "skipped" => Ok(DbNodeExecutionStatus::Skipped),
        other => Err(WorkflowRuntimeError::BadRequest(format!(
            "Unknown node execution status `{other}`"
        ))),
    }
}

fn planner_status_from_str(
    value: &str,
) -> Result<PlannerNodeExecutionStatus, WorkflowRuntimeError> {
    Ok(match node_execution_status_from_str(value)? {
        DbNodeExecutionStatus::Pending => PlannerNodeExecutionStatus::Pending,
        DbNodeExecutionStatus::Running => PlannerNodeExecutionStatus::Running,
        DbNodeExecutionStatus::AwaitingHuman => PlannerNodeExecutionStatus::AwaitingHuman,
        DbNodeExecutionStatus::AwaitingArena => PlannerNodeExecutionStatus::AwaitingArena,
        DbNodeExecutionStatus::Cancelling => PlannerNodeExecutionStatus::Cancelling,
        DbNodeExecutionStatus::Succeeded => PlannerNodeExecutionStatus::Succeeded,
        DbNodeExecutionStatus::Failed => PlannerNodeExecutionStatus::Failed,
        DbNodeExecutionStatus::Cancelled => PlannerNodeExecutionStatus::Cancelled,
        DbNodeExecutionStatus::Skipped => PlannerNodeExecutionStatus::Skipped,
    })
}

fn run_status_value(status: WorkflowRunStatus) -> &'static str {
    match status {
        WorkflowRunStatus::Pending => "pending",
        WorkflowRunStatus::Running => "running",
        WorkflowRunStatus::AwaitingHuman => "awaiting_human",
        WorkflowRunStatus::AwaitingArena => "awaiting_arena",
        WorkflowRunStatus::Cancelling => "cancelling",
        WorkflowRunStatus::Succeeded => "succeeded",
        WorkflowRunStatus::Failed => "failed",
        WorkflowRunStatus::Canceled => "canceled",
    }
}

fn node_status_value(status: DbNodeExecutionStatus) -> &'static str {
    match status {
        DbNodeExecutionStatus::Pending => "pending",
        DbNodeExecutionStatus::Running => "running",
        DbNodeExecutionStatus::AwaitingHuman => "awaiting_human",
        DbNodeExecutionStatus::AwaitingArena => "awaiting_arena",
        DbNodeExecutionStatus::Cancelling => "cancelling",
        DbNodeExecutionStatus::Succeeded => "succeeded",
        DbNodeExecutionStatus::Failed => "failed",
        DbNodeExecutionStatus::Cancelled => "cancelled",
        DbNodeExecutionStatus::Skipped => "skipped",
    }
}

fn node_kind_value(kind: &WorkflowNodeKind) -> &'static str {
    match kind {
        WorkflowNodeKind::Start => "start",
        WorkflowNodeKind::End => "end",
        WorkflowNodeKind::Agent => "agent",
        WorkflowNodeKind::TaskSplitter => "task_splitter",
        WorkflowNodeKind::Condition => "condition",
        WorkflowNodeKind::HumanGate => "human_gate",
        WorkflowNodeKind::Transform => "transform",
        WorkflowNodeKind::Arena => "arena",
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn interaction_identity_consumes_one_iteration_and_persists_decision_atomically() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE workflow_runs(id BLOB PRIMARY KEY,status TEXT,error_text TEXT,updated_at TEXT,finished_at TEXT);
            CREATE TABLE node_executions(id BLOB PRIMARY KEY,run_id BLOB,node_id TEXT,iteration INTEGER,status TEXT,output_text TEXT,error_text TEXT,finished_at TEXT,updated_at TEXT);
            CREATE TABLE workflow_interaction_responses(node_execution_id BLOB PRIMARY KEY,response_json TEXT);")
            .execute(&pool).await.unwrap();
        let run = Uuid::new_v4();
        let stale = Uuid::new_v4();
        let waiting = Uuid::new_v4();
        sqlx::query("INSERT INTO workflow_runs(id,status) VALUES (?,'awaiting_human')")
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        for (id, iteration, status) in [(stale, 0, "succeeded"), (waiting, 4, "awaiting_human")] {
            sqlx::query("INSERT INTO node_executions(id,run_id,node_id,iteration,status) VALUES (?,?,'gate',?,?)")
                .bind(id).bind(run).bind(iteration).bind(status).execute(&pool).await.unwrap();
        }
        let response = serde_json::json!({"action":"approve","output_text":"accepted output"});
        assert!(matches!(
            claim_interaction(
                &pool,
                run,
                "gate",
                Some(stale),
                "awaiting_human",
                response.clone()
            )
            .await,
            Err(ApiError::Conflict(_))
        ));
        assert_eq!(
            claim_interaction(
                &pool,
                run,
                "gate",
                Some(waiting),
                "awaiting_human",
                response.clone()
            )
            .await
            .unwrap(),
            4
        );
        assert!(matches!(
            claim_interaction(
                &pool,
                run,
                "gate",
                Some(waiting),
                "awaiting_human",
                response
            )
            .await,
            Err(ApiError::Conflict(_))
        ));
        let state: (String,String,String) = sqlx::query_as("SELECT n.status,n.output_text,r.status FROM node_executions n JOIN workflow_runs r ON r.id=n.run_id WHERE n.id=?")
            .bind(waiting).fetch_one(&pool).await.unwrap();
        assert_eq!(
            state,
            (
                "succeeded".into(),
                "accepted output".into(),
                "running".into()
            )
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_interaction_responses")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }

    #[test]
    fn workflow_direct_folder_cwd_preserves_plain_path_and_selected_git_root() {
        assert_eq!(
            super::workflow_direct_folder_agent_path("F:/notes", None),
            "F:/notes"
        );
        assert_eq!(
            super::workflow_direct_folder_agent_path("F:/projects", Some("repo")),
            std::path::Path::new("F:/projects")
                .join("repo")
                .to_string_lossy()
        );
    }
    use workflow::graph::{ArenaAttemptConfig, WorkflowNodeData};

    use super::*;

    fn workflow_node(id: &str, kind: WorkflowNodeKind, prompt_template: &str) -> WorkflowNode {
        WorkflowNode {
            id: id.to_string(),
            kind,
            data: WorkflowNodeData {
                display_name: Some("Implementation".to_string()),
                prompt_template: Some(prompt_template.to_string()),
                ..WorkflowNodeData::default()
            },
            position: None,
        }
    }

    fn upstream(node_id: &str, output_text: &str) -> UpstreamOutput {
        UpstreamOutput {
            node_id: node_id.to_string(),
            output_text: output_text.to_string(),
        }
    }

    #[test]
    fn worktree_status_summary_lists_changed_and_generated_files() {
        let summary = render_worktree_status_summary(
            "todo-app",
            &WorktreeStatus {
                uncommitted_tracked: 2,
                untracked: 1,
                entries: vec![
                    StatusEntry {
                        staged: ' ',
                        unstaged: 'M',
                        path: b"src/main.tsx".to_vec(),
                        orig_path: None,
                        is_untracked: false,
                    },
                    StatusEntry {
                        staged: '?',
                        unstaged: '?',
                        path: b"docs/todo-design.md".to_vec(),
                        orig_path: None,
                        is_untracked: true,
                    },
                    StatusEntry {
                        staged: 'R',
                        unstaged: ' ',
                        path: b"src/new-name.ts".to_vec(),
                        orig_path: Some(b"src/old-name.ts".to_vec()),
                        is_untracked: false,
                    },
                ],
            },
        );

        assert!(summary.contains("todo-app: 2 tracked change(s), 1 untracked/generated file(s)"));
        assert!(summary.contains(".M modified: src/main.tsx"));
        assert!(summary.contains("?? untracked/generated: docs/todo-design.md"));
        assert!(summary.contains("R. renamed: src/old-name.ts -> src/new-name.ts"));
    }

    #[test]
    fn agent_prompt_is_explicit_workflow_envelope_with_handoff() {
        let node = workflow_node(
            "agent-implement",
            WorkflowNodeKind::Agent,
            "Implement the requested change.",
        );
        let context = NodeHandlerContext::with_upstream_outputs(
            "Build a todo workflow",
            vec![upstream("agent-plan", "Plan says implement the UI first.")],
        );

        let prompt = render_agent_prompt(&node, &context);

        assert!(prompt.contains("# Workflow Agent Envelope"));
        assert!(prompt.contains("## Current Node"));
        assert!(prompt.contains("- Type: Agent step"));
        assert!(prompt.contains("- Name: Implementation"));
        assert!(prompt.contains("- ID: agent-implement"));
        assert!(prompt.contains("## Workflow Input"));
        assert!(prompt.contains("Build a todo workflow"));
        assert!(prompt.contains("## Direct Upstream Handoff"));
        assert!(prompt.contains("### agent-plan"));
        assert!(prompt.contains("Plan says implement the UI first."));
        assert!(prompt.contains("## Node Task"));
        assert!(prompt.contains("Implement the requested change."));
        assert!(prompt.contains("Questions are allowed when needed"));
        assert!(prompt.contains("concise handoff for downstream nodes"));
    }

    #[test]
    fn agent_envelope_keeps_template_rendering_semantics() {
        let node = workflow_node(
            "agent-review",
            WorkflowNodeKind::Agent,
            "Review {{input}} using this plan: {{upstream}}",
        );
        let context = NodeHandlerContext::with_upstream_outputs(
            "Build a todo workflow",
            vec![upstream("agent-plan", "Use React state for todo items.")],
        );

        let prompt = render_agent_prompt(&node, &context);

        assert!(prompt.contains(
            "Review Build a todo workflow using this plan: Use React state for todo items."
        ));
        assert!(prompt.contains("## Direct Upstream Handoff"));
    }

    #[test]
    fn agent_prompt_can_skip_workflow_envelope() {
        let mut node = workflow_node(
            "agent-review",
            WorkflowNodeKind::Agent,
            "Review the accepted plan using file tools.",
        );
        node.data.include_workflow_context = Some(false);
        let context = NodeHandlerContext::with_upstream_outputs(
            "Read accepted request from `.vibe-kanban/workflows/run/request.md`.",
            vec![upstream(
                "agent-plan",
                "Read delivery file: `artifacts/plan.md`",
            )],
        );

        let prompt = render_agent_prompt(&node, &context);

        assert_eq!(
            prompt,
            "Review the accepted plan using file tools.\n\nRead accepted request from `.vibe-kanban/workflows/run/request.md`.\nRead delivery file: `artifacts/plan.md`"
        );
        assert!(!prompt.contains("# Workflow Agent Envelope"));
        assert!(!prompt.contains("## Direct Upstream Handoff"));
    }

    #[test]
    fn arena_attempt_prompt_wraps_attempt_template_as_envelope() {
        let mut node = workflow_node("arena", WorkflowNodeKind::Arena, "Compare approaches.");
        node.data.attempts = Some(vec![ArenaAttemptConfig {
            id: Some("candidate-a".to_string()),
            prompt_template: Some("Implement candidate from {{upstream}}".to_string()),
            ..ArenaAttemptConfig::default()
        }]);
        let context = NodeHandlerContext::with_upstream_outputs(
            "Build a todo workflow",
            vec![upstream("agent-plan", "Plan says keep the UI simple.")],
        );

        let requests =
            arena_attempt_requests(Uuid::nil(), Uuid::nil(), &node, &context, "fallback");

        assert_eq!(requests.len(), 1);
        assert!(requests[0].prompt.contains("# Workflow Agent Envelope"));
        assert!(requests[0].prompt.contains("- Type: Arena candidate"));
        assert!(
            requests[0]
                .prompt
                .contains("Implement candidate from Plan says keep the UI simple.")
        );
        assert!(requests[0].prompt.contains("## Direct Upstream Handoff"));
    }
}
