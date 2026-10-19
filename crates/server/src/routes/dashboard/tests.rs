use std::time::Instant;

use chrono::{Duration, Utc};
use db::models::{
    agent_run_preview::{AgentRunPreviewRecord, AssistantReplyPreview},
    dashboard::DashboardFacts,
    project::DEFAULT_PROJECT_ID,
};
use serde_json::json;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use uuid::Uuid;

use super::{query, types::*};

async fn pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(":memory:")
                .foreign_keys(true),
        )
        .await
        .unwrap();
    sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
    pool
}

async fn workspace(pool: &SqlitePool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces(id,branch,workspace_kind,container_ref) VALUES (?,'direct-folder','direct_folder','C:/actual')").bind(id).execute(pool).await.unwrap();
    id
}

async fn session(pool: &SqlitePool, workspace: Uuid, title: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO sessions(id,workspace_id,name,executor,agent_working_dir) VALUES (?,?,?,'codex','C:/actual')").bind(id).bind(workspace).bind(title).execute(pool).await.unwrap();
    id
}

async fn run(
    pool: &SqlitePool,
    session: Uuid,
    workspace: Uuid,
    status: &str,
    created_at: chrono::DateTime<Utc>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO agent_runs(id,session_id,workspace_id,request_id,idempotency_key,correlation_id,schema_version,payload_version,runtime_profile_id,provider_id,workspace_mode,workspace_path,status,projection_status,request_envelope,created_at,updated_at) VALUES (?,?,?,?,?,?,1,1,'CODEX:TEST','codex','shared_workspace','C:/actual',?,'current','{}',?,?)")
        .bind(id).bind(session).bind(workspace).bind(Uuid::new_v4()).bind(id.to_string()).bind(Uuid::new_v4()).bind(status).bind(created_at).bind(created_at).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO agent_run_state(agent_run_id,state_schema_version,reducer_version,status,projection_status,state_json,updated_at) VALUES (?,1,1,?,'current',?,?)")
        .bind(id).bind(status).bind(json!({"observed_configuration":{"model_id":"actual-model","reasoning_id":"high"}}).to_string()).bind(created_at).execute(pool).await.unwrap();
    id
}

async fn business_task(pool: &SqlitePool) -> (Uuid, Uuid) {
    let project = Uuid::new_v4();
    let status = Uuid::new_v4();
    let task = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES (?,'Stale SQL name')")
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO local_project_statuses(id,project_id,name,color,sort_order,hidden) VALUES (?,?,'Todo','blue',0,0)").bind(status).bind(project).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO local_issues(id,project_id,issue_number,simple_id,status_id,title,sort_order) VALUES (?,?,1,'LOCAL-1',?,'Stale SQL task',0)").bind(task).bind(project).bind(status).execute(pool).await.unwrap();
    (project, task)
}

async fn execution(
    pool: &SqlitePool,
    project: Uuid,
    task: Uuid,
    parent: Option<Uuid>,
    kind: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tasks(id,project_id,issue_id,parent_task_id,title,execution_kind) VALUES (?,?,?,?,'Actual execution',?)").bind(id).bind(project).bind(task).bind(parent).bind(kind).execute(pool).await.unwrap();
    id
}

async fn workflow(pool: &SqlitePool) -> (Uuid, Uuid, Uuid, Uuid, Uuid) {
    let (project, task) = business_task(pool).await;
    let execution = execution(pool, project, task, None, "workflow").await;
    let definition = Uuid::new_v4();
    let attempt = Uuid::new_v4();
    let run = Uuid::new_v4();
    sqlx::query("INSERT INTO workflows(id,source,project_id,name,graph_json) VALUES (?,'project',?,'Real workflow','{\"nodes\":[],\"edges\":[] }')").bind(definition).bind(project).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO workflow_attempts(id,task_id,workflow_id,status) VALUES (?,?,?,'running')",
    )
    .bind(attempt)
    .bind(execution)
    .bind(definition)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO workflow_runs(id,workflow_id,attempt_id,issue_id,input_text,status) VALUES (?,?,?,?,'input','running')").bind(run).bind(definition).bind(attempt).bind(task).execute(pool).await.unwrap();
    sqlx::query("UPDATE workflow_attempts SET latest_run_id=? WHERE id=?")
        .bind(run)
        .bind(attempt)
        .execute(pool)
        .await
        .unwrap();
    (project, task, execution, attempt, run)
}

async fn preview(pool: &SqlitePool, run_id: Uuid, text: &str) {
    let mut record = AgentRunPreviewRecord::empty(run_id);
    record.preview.0.assistant = Some(AssistantReplyPreview {
        message_id: Uuid::new_v4(),
        run_attempt_id: Uuid::new_v4(),
        run_attempt_number: 1,
        sequence: 1,
        event_id: Uuid::new_v4(),
        timestamp: Utc::now(),
        text: text.into(),
        truncated: false,
    });
    assert!(
        AgentRunPreviewRecord::store_if_unchanged(pool, None, &record)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn dashboard_real_database_preserves_exact_session_runs_and_missing_metadata() {
    let pool = pool().await;
    let ws = workspace(&pool).await;
    let first = session(&pool, ws, "A").await;
    let second = session(&pool, ws, "B").await;
    let now = Utc::now();
    let failed = run(&pool, first, ws, "failed", now - Duration::minutes(2)).await;
    let current = run(&pool, first, ws, "running", now).await;
    let other = run(&pool, second, ws, "awaiting_input", now).await;
    let (project, task) = business_task(&pool).await;
    let exec = execution(&pool, project, task, None, "agent").await;
    sqlx::query("INSERT INTO agent_task_bindings(task_id,session_id) VALUES (?,?)")
        .bind(exec)
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    // A damaged/missing membership must never erase a managed active Session.
    sqlx::query("DELETE FROM session_project_memberships WHERE session_id=?")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    let facts = DashboardFacts::load(&pool).await.unwrap();
    assert_eq!(facts.sessions.len(), 2);
    let page = query::execute(&pool, DashboardQuery::default())
        .await
        .unwrap();
    assert_eq!(page.counts.active_session_count, 2);
    assert_eq!(page.counts.active_run_count, 2);
    assert_eq!(page.counts.work_count, 2);
    let task_row = page.rows.iter().find(|r| r.work.id == exec).unwrap();
    assert_eq!(task_row.raw_status, "running");
    assert_eq!(task_row.agents[0].model_id.as_deref(), Some("actual-model"));
    assert_eq!(task_row.agents[0].configuration_source, "observed");
    assert_eq!(task_row.project.as_ref().unwrap().title, None);
    assert_eq!(task_row.task.as_ref().unwrap().title, None);
    assert!(
        task_row
            .issues
            .iter()
            .any(|i| i.code == "project_metadata_unreadable")
    );
    assert!(
        matches!(task_row.open_target,Some(DashboardOpenTarget::Session{session_id,agent_run_id:Some(id),..}) if session_id==first && id==current)
    );
    let standalone = page.rows.iter().find(|r| r.work.id == second).unwrap();
    assert!(
        matches!(standalone.open_target,Some(DashboardOpenTarget::Session{session_id,agent_run_id:Some(id),..}) if session_id==second && id==other)
    );
    assert!(!page.rows.iter().any(|r|matches!(r.open_target,Some(DashboardOpenTarget::Session{agent_run_id:Some(id),..}) if id==failed)));
    let repeated = query::execute(
        &pool,
        DashboardQuery {
            snapshot_id: page.snapshot_id,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!repeated.has_updates);
    // New standalone sessions may only name their workspace. Match the
    // existing sidebar until a Session-specific title has been assigned.
    sqlx::query("UPDATE workspaces SET name='Named workspace' WHERE id=?")
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET name='   ' WHERE id=?")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    let titled = query::execute(&pool, DashboardQuery::default())
        .await
        .unwrap();
    assert_eq!(
        titled
            .rows
            .iter()
            .find(|r| r.work.id == second)
            .unwrap()
            .title,
        "Named workspace"
    );
    assert_eq!(
        DashboardFacts::load(&pool)
            .await
            .unwrap()
            .sessions
            .iter()
            .find(|s| s.id == first)
            .unwrap()
            .name
            .as_deref(),
        Some("A")
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pragma_foreign_key_check")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn dashboard_current_keeps_waiting_work_and_removes_terminal_work_on_poll() {
    let pool = pool().await;
    let ws = workspace(&pool).await;
    let active = session(&pool, ws, "Still working").await;
    let active_run = run(&pool, active, ws, "running", Utc::now()).await;
    let waiting = session(&pool, ws, "Needs input").await;
    run(&pool, waiting, ws, "awaiting_input", Utc::now()).await;
    for status in ["failed", "succeeded", "cancelled"] {
        let terminal = session(&pool, ws, status).await;
        run(&pool, terminal, ws, status, Utc::now()).await;
    }
    session(&pool, ws, "Never started").await;
    let (_, _, _, attempt, workflow_run) = workflow(&pool).await;
    sqlx::query("UPDATE workflow_runs SET status='awaiting_human' WHERE id=?")
        .bind(workflow_run)
        .execute(&pool)
        .await
        .unwrap();
    let first = query::execute(&pool, DashboardQuery::default())
        .await
        .unwrap();
    assert_eq!(first.rows.len(), 3);
    assert_eq!(first.counts.active_session_count, 2);
    assert!(first.rows.iter().any(|row| row.work.id == active));
    assert!(first.rows.iter().any(|row| row.work.id == waiting));
    let gate = first
        .rows
        .iter()
        .find(|row| row.work.id == attempt)
        .unwrap();
    assert_eq!(gate.status_group, DashboardStatusGroup::Waiting);
    assert_eq!(gate.active_session_count, 0);

    sqlx::query("UPDATE agent_runs SET status='succeeded' WHERE id=?")
        .bind(active_run)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE agent_run_state SET status='succeeded' WHERE agent_run_id=?")
        .bind(active_run)
        .execute(&pool)
        .await
        .unwrap();
    let next = query::execute(
        &pool,
        DashboardQuery {
            snapshot_id: first.snapshot_id,
            visible: first.rows.iter().map(|row| row.work.clone()).collect(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(next.snapshot_id, first.snapshot_id);
    assert_eq!(next.total, 2);
    assert_eq!(next.counts.work_count, 2);
    assert!(!next.rows.iter().any(|row| row.work.id == active));
    assert_eq!(
        next.rows.iter().map(|row| &row.work).collect::<Vec<_>>(),
        first
            .rows
            .iter()
            .filter(|row| row.work.id != active)
            .map(|row| &row.work)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn dashboard_workflow_gate_children_and_old_active_runs_do_not_rewrite_parent() {
    let pool = pool().await;
    let (_, _, _, attempt, current) = workflow(&pool).await;
    let ws = workspace(&pool).await;
    let session_id = session(&pool, ws, "Node").await;
    let agent = run(&pool, session_id, ws, "running", Utc::now()).await;
    let node = Uuid::new_v4();
    sqlx::query("INSERT INTO node_executions(id,run_id,node_id,node_type,status,session_id,agent_run_id) VALUES (?,?,'agent','agent','running',?,?)").bind(node).bind(current).bind(session_id).bind(agent).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO node_executions(id,run_id,node_id,node_type,status) VALUES (?,?,'review','human_gate','awaiting_human')").bind(Uuid::new_v4()).bind(current).execute(&pool).await.unwrap();
    let condition_session = session(&pool, ws, "Branch choice").await;
    let condition_run = run(&pool, condition_session, ws, "succeeded", Utc::now()).await;
    sqlx::query("INSERT INTO node_executions(id,run_id,node_id,node_type,status,session_id,agent_run_id) VALUES (?,?,'condition','condition','awaiting_human',?,?)").bind(Uuid::new_v4()).bind(current).bind(condition_session).bind(condition_run).execute(&pool).await.unwrap();
    let read = query::DashboardRead::load(&pool, false).await.unwrap();
    let work = read
        .works
        .iter()
        .find(|w| w.row.work.id == attempt)
        .unwrap();
    assert_eq!(work.row.raw_status, "running");
    assert_eq!(work.row.active_session_count, 1);
    assert_eq!(work.row.waiting_count, 2);
    assert_eq!(work.children.len(), 3);
    assert!(
        work.children
            .iter()
            .any(|c| c.session_id.is_none()
                && c.raw_status == "awaiting_human"
                && matches!(c.open_target, Some(DashboardOpenTarget::Workflow { attempt_id, .. }) if attempt_id == attempt))
    );
    assert!(
        work.children
            .iter()
            .any(|c| c.agent_run_id == Some(condition_run)
                && c.raw_status == "awaiting_human"
                && c.agent_run_status.as_deref() == Some("succeeded"))
    );
    assert!(!read.works.iter().any(|w| w.row.work.id == session_id));
    let page = query::execute(
        &pool,
        DashboardQuery {
            filters: DashboardFilters {
                status: DashboardStatusFilter::Waiting,
                provider_id: Some("codex".into()),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.counts.active_session_count, 1);
    assert_eq!(page.counts.active_run_count, 1);
    assert_eq!(page.rows[0].matching_session_count, 2);
    // The runtime owns parent status, even while an old Agent is still active.
    sqlx::query("UPDATE workflow_runs SET status='succeeded' WHERE id=?")
        .bind(current)
        .execute(&pool)
        .await
        .unwrap();
    let read = query::DashboardRead::load(&pool, false).await.unwrap();
    let work = read
        .works
        .iter()
        .find(|w| w.row.work.id == attempt)
        .unwrap();
    assert_eq!(work.row.raw_status, "succeeded");
    assert_eq!(work.row.status_group, DashboardStatusGroup::Finished);
    assert_eq!(work.row.active_session_count, 1);
    let ongoing = query::execute(&pool, DashboardQuery::default())
        .await
        .unwrap();
    assert!(ongoing.rows.iter().any(|row| row.work.id == attempt));
}

#[tokio::test]
async fn dashboard_arena_uses_orchestration_link_not_shared_workspace() {
    let pool = pool().await;
    let (project, task, parent, attempt, workflow_run) = workflow(&pool).await;
    let ws = workspace(&pool).await;
    let actual_session = session(&pool, ws, "Candidate").await;
    let unrelated = session(&pool, ws, "Unrelated").await;
    let candidate_run = run(
        &pool,
        actual_session,
        ws,
        "running",
        Utc::now() - Duration::seconds(2),
    )
    .await;
    let other_run = run(&pool, unrelated, ws, "running", Utc::now()).await;
    let exec = execution(&pool, project, task, Some(parent), "arena").await;
    let group = Uuid::new_v4();
    let candidate = Uuid::new_v4();
    sqlx::query("INSERT INTO arena_groups(id,task_id,prompt,base_branch) VALUES (?,?,'p','main')")
        .bind(group)
        .bind(exec)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO arena_candidates(id,arena_group_id,workspace_id,purpose,sort_order) VALUES (?,?,?,'attempt',0)").bind(candidate).bind(group).bind(ws).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO node_executions(id,run_id,node_id,node_type,status,arena_group_id) VALUES (?,?,'compare','arena','awaiting_arena',?)").bind(Uuid::new_v4()).bind(workflow_run).bind(group).execute(&pool).await.unwrap();
    let orchestration = Uuid::new_v4();
    let node = Uuid::new_v4();
    sqlx::query("INSERT INTO orchestration_runs(id,request_id,idempotency_key,correlation_id,product_kind,source_definition_id,source_definition_version,plan_schema_version,plan_snapshot) VALUES (?,?,?,?,'arena',?,'1',1,'{}')").bind(orchestration).bind(Uuid::new_v4()).bind(orchestration.to_string()).bind(Uuid::new_v4()).bind(group).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO orchestration_node_executions(id,orchestration_run_id,node_key,stable_order) VALUES (?,?,'candidate',0)").bind(node).bind(orchestration).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO orchestration_agent_run_links(id,orchestration_run_id,node_execution_id,agent_run_id,dispatch_idempotency_key) VALUES (?,?,?,?,?)").bind(Uuid::new_v4()).bind(orchestration).bind(node).bind(candidate_run).bind(candidate.to_string()).execute(&pool).await.unwrap();
    let facts = DashboardFacts::load(&pool).await.unwrap();
    assert_eq!(facts.arena_runs.len(), 1);
    assert_eq!(facts.arena_runs[0].agent_run_id, candidate_run);
    let read = query::DashboardRead::load(&pool, false).await.unwrap();
    let work = read
        .works
        .iter()
        .find(|w| w.row.work.id == attempt)
        .unwrap();
    assert!(
        work.children
            .iter()
            .any(|c| c.arena_candidate_id == Some(candidate)
                && c.agent_run_id == Some(candidate_run))
    );
    assert!(
        !work
            .children
            .iter()
            .any(|c| c.agent_run_id == Some(other_run))
    );
    assert!(read.works.iter().any(|w| w.row.work.id == unrelated));
}

#[tokio::test]
async fn dashboard_run_replacement_keeps_old_iterations_split_and_arena_out_of_current_reply() {
    let pool = pool().await;
    let (project, task, parent_execution, attempt, old) = workflow(&pool).await;
    let current = Uuid::new_v4();
    sqlx::query("INSERT INTO workflow_runs(id,workflow_id,attempt_id,issue_id,input_text,status) SELECT ?,workflow_id,attempt_id,issue_id,'replacement','succeeded' FROM workflow_runs WHERE id=?").bind(current).bind(old).execute(&pool).await.unwrap();
    sqlx::query("UPDATE workflow_attempts SET latest_run_id=? WHERE id=?")
        .bind(current)
        .bind(attempt)
        .execute(&pool)
        .await
        .unwrap();
    let ws = workspace(&pool).await;
    let mut old_active_runs = Vec::new();
    // Both iterations remain active; neither may be lost to latest-per-Node.
    for iteration in 0..2 {
        let session_id = session(&pool, ws, &format!("Old iteration {iteration}")).await;
        let agent = run(&pool, session_id, ws, "running", Utc::now()).await;
        old_active_runs.push(agent);
        sqlx::query("INSERT INTO node_executions(id,run_id,node_id,node_type,status,iteration,session_id,agent_run_id) VALUES (?,?,'retry','agent','running',?,?,?)").bind(Uuid::new_v4()).bind(old).bind(iteration).bind(session_id).bind(agent).execute(&pool).await.unwrap();
    }
    let split_session = session(&pool, ws, "Old internal work").await;
    let split_run = run(&pool, split_session, ws, "running", Utc::now()).await;
    old_active_runs.push(split_run);
    let split_node = Uuid::new_v4();
    let split_batch = Uuid::new_v4();
    let subtask = Uuid::new_v4();
    sqlx::query("INSERT INTO node_executions(id,run_id,node_id,node_type,status,session_id,agent_run_id) VALUES (?,?,'split_worker','agent','running',?,?)").bind(split_node).bind(old).bind(split_session).bind(split_run).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO workflow_split_batches(id,run_id,node_execution_id,manifest_path,manifest_sha256,state) VALUES (?,?,?,'tasks.json','sha','running')").bind(split_batch).bind(old).bind(split_node).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO workflow_split_subtasks(id,batch_id,subtask_id,title,instruction_path,instruction_sha256,node_execution_id,session_id,state) VALUES (?,?,'internal','Internal work','task.md','sha',?,?,'running')").bind(subtask).bind(split_batch).bind(split_node).bind(split_session).execute(&pool).await.unwrap();

    let arena_exec = execution(&pool, project, task, Some(parent_execution), "arena").await;
    let group = Uuid::new_v4();
    let candidate = Uuid::new_v4();
    let candidate_ws = workspace(&pool).await;
    let candidate_session = session(&pool, candidate_ws, "Old candidate").await;
    let candidate_run = run(
        &pool,
        candidate_session,
        candidate_ws,
        "running",
        Utc::now(),
    )
    .await;
    old_active_runs.push(candidate_run);
    sqlx::query("INSERT INTO arena_groups(id,task_id,prompt,base_branch) VALUES (?,?,'p','main')")
        .bind(group)
        .bind(arena_exec)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO arena_candidates(id,arena_group_id,workspace_id,purpose,sort_order) VALUES (?,?,?,'attempt',0)").bind(candidate).bind(group).bind(candidate_ws).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO node_executions(id,run_id,node_id,node_type,status,arena_group_id) VALUES (?,?,'compare','arena','awaiting_arena',?)").bind(Uuid::new_v4()).bind(old).bind(group).execute(&pool).await.unwrap();
    let orchestration = Uuid::new_v4();
    let candidate_node = Uuid::new_v4();
    sqlx::query("INSERT INTO orchestration_runs(id,request_id,idempotency_key,correlation_id,product_kind,source_definition_id,source_definition_version,plan_schema_version,plan_snapshot) VALUES (?,?,?,?,'arena',?,'1',1,'{}')").bind(orchestration).bind(Uuid::new_v4()).bind(orchestration.to_string()).bind(Uuid::new_v4()).bind(group).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO orchestration_node_executions(id,orchestration_run_id,node_key,stable_order) VALUES (?,?,'candidate',0)").bind(candidate_node).bind(orchestration).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO orchestration_agent_run_links(id,orchestration_run_id,node_execution_id,agent_run_id,dispatch_idempotency_key) VALUES (?,?,?,?,?)").bind(Uuid::new_v4()).bind(orchestration).bind(candidate_node).bind(candidate_run).bind(candidate.to_string()).execute(&pool).await.unwrap();

    // A reused result is explicitly historical even though it references a Run.
    let historical_session = session(&pool, ws, "Reused result").await;
    let historical_run = run(&pool, historical_session, ws, "succeeded", Utc::now()).await;
    let historical_node = Uuid::new_v4();
    sqlx::query("INSERT INTO node_executions(id,run_id,node_id,node_type,status,session_id,agent_run_id) VALUES (?,?,'reuse','agent','succeeded',?,?)").bind(historical_node).bind(old).bind(historical_session).bind(historical_run).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO workflow_result_reuse(run_id,node_id,iteration,source_node_execution_id) VALUES (?,'reuse',0,?)").bind(current).bind(historical_node).execute(&pool).await.unwrap();
    for old_run in old_active_runs.iter().copied().chain(Some(historical_run)) {
        preview(
            &pool,
            old_run,
            "Old output must not become the current reply",
        )
        .await;
    }
    let read = query::DashboardRead::load(&pool, false).await.unwrap();
    let work = read
        .works
        .iter()
        .find(|w| w.row.work.id == attempt)
        .unwrap();
    assert_eq!(work.row.raw_status, "succeeded");
    assert_eq!(work.row.active_run_count, 4);
    assert!(work.row.preview.is_none());
    for old_run in &old_active_runs {
        assert!(
            work.children
                .iter()
                .any(|c| c.agent_run_id == Some(*old_run)
                    && c.workflow_run_id == Some(old)
                    && c.role == "other_active_run")
        );
    }
    assert!(work.children.iter().any(|c| c.subtask_id == Some(subtask)));
    assert!(
        work.children
            .iter()
            .any(|c| c.arena_candidate_id == Some(candidate))
    );
    assert!(
        work.children
            .iter()
            .any(|c| c.reused && c.preview.is_none() && c.preview_status == "historical")
    );
    let invalidated = query::child_page(
        "run-replacement-test",
        Some(work),
        &DashboardExpandedPage {
            work: work.row.work.clone(),
            workflow_run_id: Some(old),
            snapshot_id: None,
            page: 0,
        },
        &DashboardFilters::default(),
    )
    .unwrap();
    assert_eq!(invalidated.invalidation.as_deref(), Some("run_changed"));
    assert!(invalidated.rows.is_empty());
}

#[tokio::test]
async fn dashboard_bounded_fixture_100_works_300_sessions_20_active() {
    let pool = pool().await;
    let ws = workspace(&pool).await;
    let now = Utc::now();
    // Use ordinary sessions for 100 visible roots and 200 old idle sessions;
    // all 300 remain available without inflating the Current filter.
    for i in 0..300 {
        let id = session(&pool, ws, &format!("Session {i}")).await;
        let created = if i < 100 {
            now
        } else {
            now - Duration::days(90)
        };
        run(
            &pool,
            id,
            ws,
            if i < 20 { "running" } else { "succeeded" },
            created,
        )
        .await;
        sqlx::query("UPDATE sessions SET updated_at=? WHERE id=?")
            .bind(created)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let query = DashboardQuery {
        filters: DashboardFilters {
            status: DashboardStatusFilter::All,
            ..Default::default()
        },
        ..Default::default()
    };
    let start = Instant::now();
    let page = query::execute(&pool, query).await.unwrap();
    let elapsed = start.elapsed();
    let bytes = serde_json::to_vec(&page).unwrap().len();
    assert_eq!(page.latest_total, 100);
    assert_eq!(page.rows.len(), 20);
    assert_eq!(page.counts.active_session_count, 20);
    assert_eq!(page.counts.active_run_count, 20);
    assert_eq!(page.projects[0].id, DEFAULT_PROJECT_ID);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert!(
        AgentRunPreviewRecord::find_for_runs(&pool, &[])
            .await
            .unwrap()
            .is_empty()
    );
    eprintln!(
        "Dashboard fixture: 100 matching work roots, 300 managed sessions, 20 active runs; page=20, elapsed={elapsed:?}, response_bytes={bytes}; facts=9 fixed batch SELECTs + preview chunks; canonical history SELECTs=0"
    );
}
