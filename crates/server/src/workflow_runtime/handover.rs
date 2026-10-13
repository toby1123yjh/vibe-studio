//! The only automatic main-Agent turn is a durable accepted-split handover.
//! Notifications, reads, and ordinary node completion never call this path.
use std::{
    collections::HashSet,
    path::Path,
    sync::{Mutex, OnceLock},
};

use db::models::{
    session::Session, workflow_handover::WorkflowHandoverEvent, workspace::Workspace,
};
use deployment::Deployment;
use executors::runtime::{
    AgentRunIntent, AgentRunPort, AgentRunPortCommand, AgentRunPortCommandEnvelope,
    AgentRunRequestEnvelope, AgentRunStatus, AgentRuntimeMessageRole,
    ORCHESTRATION_COMMAND_SCHEMA_VERSION, RunAttemptMode, RunAttemptRequest,
};
use services::services::container::ContainerService;
use sqlx::{FromRow, SqliteConnection, SqlitePool, types::Json};
use uuid::Uuid;
use workflow::{
    WorkflowGraph,
    dispatch::{DispatchScope, dispatch_scope},
    files::{SplitTaskManifest, WorkflowFileRef, validate_manifest},
};

use super::{management::read_workflow_context, runner::file_handoff::read_bound_file};
use crate::{
    DeploymentImpl,
    error::ApiError,
    routes::sessions::agent_run::{
        AgentRunDispatch, AgentRunLaunch, AgentRunOrigin, create_agent_run_under_session_guard,
        direct_provider, latest_provider_session, validate_session_executor,
        validate_session_provider_binding,
    },
};

#[derive(FromRow)]
struct HandoverSource {
    attempt_id: Option<Uuid>,
    workspace_id: Option<Uuid>,
    main_session_id: Option<Uuid>,
    run_status: String,
    batch_state: String,
    source_status: String,
    node_execution_id: Uuid,
    node_id: String,
    graph_snapshot: String,
    manifest_path: String,
    manifest_sha256: String,
    dispatch_sha256: Option<String>,
    failure_requested: bool,
    stop_requested: bool,
    project_slot_held: bool,
}

impl HandoverSource {
    fn accepts_work(&self) -> bool {
        !self.stop_requested
            && !self.failure_requested
            && matches!(
                self.run_status.as_str(),
                "running" | "awaiting_human" | "awaiting_arena"
            )
    }
}

async fn source(
    pool: &SqlitePool,
    event: &WorkflowHandoverEvent,
) -> Result<HandoverSource, ApiError> {
    sqlx::query_as("SELECT r.attempt_id,r.workspace_id,a.main_session_id,r.status AS run_status,b.state AS batch_state,n.status AS source_status,b.node_execution_id,n.node_id,r.graph_snapshot,b.manifest_path,b.manifest_sha256,b.dispatch_sha256,r.error_text IS NOT NULL AS failure_requested,EXISTS(SELECT 1 FROM workflow_stop_intents i WHERE i.run_id=r.id) AS stop_requested,EXISTS(SELECT 1 FROM workflow_project_slots p WHERE p.run_id=r.id) AS project_slot_held FROM workflow_split_batches b JOIN workflow_runs r ON r.id=b.run_id JOIN node_executions n ON n.id=b.node_execution_id AND n.run_id=b.run_id LEFT JOIN workflow_attempts a ON a.id=r.attempt_id WHERE b.id=? AND b.run_id=?")
        .bind(event.batch_id).bind(event.run_id).fetch_optional(pool).await?
        .ok_or_else(|| ApiError::Conflict("Accepted workflow handover source is unavailable".into()))
}

fn in_flight() -> &'static Mutex<HashSet<Uuid>> {
    static EVENTS: OnceLock<Mutex<HashSet<Uuid>>> = OnceLock::new();
    EVENTS.get_or_init(|| Mutex::new(HashSet::new()))
}

struct HandoverGuard(Uuid);
impl Drop for HandoverGuard {
    fn drop(&mut self) {
        in_flight()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.0);
    }
}

/// Native startup must not block reconciliation of other projects. The local
/// guard only suppresses overlapping ticks; SQLite owns durable identities.
pub async fn process_pending(deployment: &DeploymentImpl) -> Result<(), ApiError> {
    for event in WorkflowHandoverEvent::list_unsettled(&deployment.db().pool).await? {
        if !in_flight()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(event.id)
        {
            continue;
        }
        let deployment = deployment.clone();
        tokio::spawn(async move {
            let _guard = HandoverGuard(event.id);
            if let Err(error) = process_one(&deployment, event.id).await {
                let _ = WorkflowHandoverEvent::set_error(
                    &deployment.db().pool,
                    event.id,
                    Some(&error.to_string()),
                )
                .await;
                tracing::warn!(handover_id=%event.id, run_id=%event.run_id, %error, "Workflow main-Agent handover retained for reconciliation");
            }
        });
    }
    Ok(())
}

/// A finished reply is insufficient: all attempts and their actual processes
/// must settle before releasing the workflow's project slot.
pub async fn has_unsettled(pool: &SqlitePool, run_id: Uuid) -> Result<bool, ApiError> {
    let mut conn = pool.acquire().await?;
    has_unsettled_in(&mut conn, run_id).await
}

pub(crate) async fn has_unsettled_in(
    conn: &mut SqliteConnection,
    run_id: Uuid,
) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_handover_events h WHERE h.run_id=? AND (h.state IN ('pending','claimed') OR EXISTS(SELECT 1 FROM agent_runs r LEFT JOIN agent_run_attempts a ON a.agent_run_id=r.id LEFT JOIN agent_process_registry p ON p.run_attempt_id=a.id WHERE r.id=h.agent_run_id AND (a.id IS NULL OR a.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed') OR p.run_attempt_id IS NULL OR p.registry_status<>'exited'))))")
        .bind(run_id).fetch_one(conn).await?)
}

async fn process_one(deployment: &DeploymentImpl, event_id: Uuid) -> Result<(), ApiError> {
    let pool = &deployment.db().pool;
    let Some(initial) = WorkflowHandoverEvent::find_by_id(pool, event_id).await? else {
        return Ok(());
    };
    // Queue insertion/consumption, direct user turns, deletion and automatic
    // turns all share this gate. Never acquire it while holding a run lock.
    let guard = deployment
        .queued_message_service()
        .lock_session(initial.session_id)
        .await;
    let Some(mut event) = WorkflowHandoverEvent::find_by_id(pool, event_id).await? else {
        return Ok(());
    };
    let origin = source(pool, &event).await?;
    if !origin.accepts_work() {
        return cancel_one(deployment, &event).await;
    }
    let existing = owned_turn(pool, &event).await?;
    if let Some((request, attempt, status)) = existing.as_ref() {
        if status.is_terminal() {
            finish_terminal(pool, &event, *status).await?;
            deployment
                .agent_run_port()
                .reconcile_terminal_process(attempt.run_attempt_id)
                .await
                .map_err(port_error)?;
            return Ok(());
        }
        if event.state != "claimed" {
            // An event's business outcome cannot authorize another model turn.
            // The still-running original turn remains an occupancy barrier.
            return Ok(());
        }
        if *status != AgentRunStatus::Pending {
            WorkflowHandoverEvent::set_error(pool, event.id, None).await?;
            return Ok(());
        }
        let proof = launch_proof(pool, &event, attempt.run_attempt_id).await?;
        if !proof.can_launch() {
            WorkflowHandoverEvent::set_error(pool, event.id, Some("Main Agent launch was already handed off; waiting for its original process, without starting a replacement")).await?;
            return Ok(());
        }
        if request.workspace.workspace_id != origin.workspace_id.unwrap_or(Uuid::nil()) {
            return Err(ApiError::Conflict(
                "Handover workspace no longer matches its canonical turn".into(),
            ));
        }
    } else if !matches!(event.state.as_str(), "pending" | "claimed") {
        return Ok(());
    }
    // A user may already have submitted a valid dispatch while this event was
    // waiting. No automatic Agent is needed, even across service restart.
    if origin.dispatch_sha256.is_some() {
        if existing.is_none() {
            WorkflowHandoverEvent::finish(pool, event.id, "completed", None).await?;
        } else {
            // A reserved but never-launched system turn is retired, not run
            // unnecessarily after a manually accepted dispatch.
            deployment
                .agent_run_port()
                .fail_reserved(
                    event.agent_run_id.unwrap(),
                    "Dispatch already accepted before automatic launch".into(),
                )
                .await
                .map_err(port_error)?;
            WorkflowHandoverEvent::finish(pool, event.id, "completed", None).await?;
        }
        return Ok(());
    }
    if origin.batch_state != "accepted" {
        return fail_handover(pool, &event, "Split batch is no longer awaiting dispatch").await;
    }
    if origin.source_status != "succeeded" {
        return WorkflowHandoverEvent::set_error(
            pool,
            event.id,
            Some("Waiting for the splitter's real successful completion"),
        )
        .await
        .map_err(Into::into);
    }
    if !origin.project_slot_held {
        return WorkflowHandoverEvent::set_error(
            pool,
            event.id,
            Some("Waiting for the workflow Run's project occupancy before dispatch"),
        )
        .await
        .map_err(Into::into);
    }
    if deployment
        .queued_message_service()
        .has_queued(event.session_id)
        || session_has_other_active_run(pool, &event).await?
    {
        return WorkflowHandoverEvent::set_error(
            pool,
            event.id,
            Some("Waiting for the user's active or queued main-Session input"),
        )
        .await
        .map_err(Into::into);
    }
    if !previous_processes_settled(deployment, &event).await? {
        return WorkflowHandoverEvent::set_error(
            pool,
            event.id,
            Some("Waiting for the previous main Agent process to exit"),
        )
        .await
        .map_err(Into::into);
    }
    let Some(session) = Session::find_by_id(pool, event.session_id).await? else {
        return WorkflowHandoverEvent::set_error(
            pool,
            event.id,
            Some("Original main Session was deleted; no replacement Session will be created"),
        )
        .await
        .map_err(Into::into);
    };
    let context = read_workflow_context(pool, event.session_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Original main Agent binding is unavailable".into()))?;
    if origin.main_session_id != Some(event.session_id)
        || context.instance_id != origin.attempt_id
        || origin.workspace_id != Some(context.workspace_id)
        || session.workspace_id != context.workspace_id
    {
        return Err(ApiError::Forbidden(
            "Original main Session no longer belongs to this workflow Run and workspace".into(),
        ));
    }
    let workspace = Workspace::find_by_id(pool, context.workspace_id)
        .await?
        .ok_or_else(|| ApiError::Conflict("Original main workspace is unavailable".into()))?;
    let workspace_path = deployment
        .container()
        .ensure_container_exists(&workspace)
        .await?;
    let scope = match verify_files(pool, Path::new(&workspace_path), &event, &origin).await {
        Ok(scope) => scope,
        Err(error) => {
            if let Some(id) = event.agent_run_id {
                if existing.is_some() {
                    deployment
                        .agent_run_port()
                        .fail_reserved(id, error.to_string())
                        .await
                        .map_err(port_error)?;
                }
            }
            return fail_handover(pool, &event, &error.to_string()).await;
        }
    };
    if existing.is_none() {
        validate_session_executor(pool, &session, &context.main_agent_config).await?;
        super::mcp_launch::validate_main_agent_launch(
            deployment,
            session.id,
            workspace.id,
            &context.main_agent_config,
        )
        .await?;
        let provider = direct_provider(&context.main_agent_config)?;
        let runtime_profile_id = context.main_agent_config.profile_id().cache_key();
        let provider_session =
            latest_provider_session(pool, session.id, provider, &runtime_profile_id).await?;
        validate_session_provider_binding(
            pool,
            session.id,
            provider,
            &runtime_profile_id,
            provider_session.as_ref(),
            &context.main_agent_config,
            None,
            Some(Path::new(&workspace_path)),
        )
        .await?;
        if event.state == "pending" {
            if !WorkflowHandoverEvent::claim(
                pool,
                event.id,
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
            )
            .await?
            {
                return Ok(());
            }
            event = WorkflowHandoverEvent::find_by_id(pool, event.id)
                .await?
                .ok_or_else(|| {
                    ApiError::Conflict("Workflow handover disappeared during claim".into())
                })?;
        }
        let (agent_run_id, run_attempt_id, turn_id) = claimed_identity(&event)?;
        create_agent_run_under_session_guard(
            deployment,
            &session,
            &workspace,
            workspace_path,
            AgentRunLaunch {
                intent: if provider_session.is_some() {
                    AgentRunIntent::FollowUp
                } else {
                    AgentRunIntent::Initial
                },
                mode: if provider_session.is_some() {
                    RunAttemptMode::Resume
                } else {
                    RunAttemptMode::Launch
                },
                prompt: handover_prompt(&event, &origin, &scope),
                selected_skills: None,
                executor_config: context.main_agent_config,
                provider_session,
            },
            AgentRunOrigin {
                dispatch: AgentRunDispatch::Reserved,
                request_id: event.id,
                agent_run_id,
                run_attempt_id,
                turn_id,
                correlation_id: event.id,
                message_id: event.id,
                role: AgentRuntimeMessageRole::System,
                created_at: event.created_at,
            },
            &guard,
        )
        .await?;
    } else {
        let (_, attempt, _) = existing.as_ref().unwrap();
        if !executors::workflow_mcp::matches_snapshot(
            &context.main_agent_config,
            &attempt.executor_config,
        ) {
            return Err(ApiError::Conflict(
                "Captured main Agent configuration differs from the reserved handover".into(),
            ));
        }
    }
    // Claim/reservation precede launch, and both recovery and first launch
    // recheck durable stop/dispatch boundaries. Token issuance repeats these
    // checks transactionally inside the actual local AgentRunPort.
    let current = source(pool, &event).await?;
    if !current.accepts_work() {
        return cancel_one(deployment, &event).await;
    }
    if current.dispatch_sha256.is_some() {
        deployment
            .agent_run_port()
            .fail_reserved(
                event.agent_run_id.unwrap(),
                "Dispatch already accepted before automatic launch".into(),
            )
            .await
            .map_err(port_error)?;
        WorkflowHandoverEvent::finish(pool, event.id, "completed", None).await?;
        return Ok(());
    }
    if !current.project_slot_held {
        return WorkflowHandoverEvent::set_error(
            pool,
            event.id,
            Some("Workflow Run no longer owns its project slot; automatic launch is held"),
        )
        .await
        .map_err(Into::into);
    }
    let (agent_run_id, _, _) = claimed_identity(&event)?;
    let snapshot = deployment
        .agent_run_port()
        .launch_reserved(agent_run_id)
        .await
        .map_err(port_error)?;
    if snapshot.state.status.is_terminal() {
        finish_terminal(pool, &event, snapshot.state.status).await?;
    } else {
        WorkflowHandoverEvent::set_error(pool, event.id, None).await?;
    }
    Ok(())
}

fn claimed_identity(event: &WorkflowHandoverEvent) -> Result<(Uuid, Uuid, Uuid), ApiError> {
    match (event.agent_run_id, event.run_attempt_id, event.turn_id) {
        (Some(run), Some(attempt), Some(turn)) => Ok((run, attempt, turn)),
        _ => Err(ApiError::Conflict("Claimed handover has an incomplete canonical identity; replacement launch is forbidden".into())),
    }
}

/// Query canonical authority, never the legacy execution-process projection.
async fn owned_turn(
    pool: &SqlitePool,
    event: &WorkflowHandoverEvent,
) -> Result<Option<(AgentRunRequestEnvelope, RunAttemptRequest, AgentRunStatus)>, ApiError> {
    let Some(run_id) = event.agent_run_id else {
        return Ok(None);
    };
    let row: Option<(Json<AgentRunRequestEnvelope>, Json<RunAttemptRequest>, AgentRunStatus)> = sqlx::query_as("SELECT r.request_envelope,a.request_envelope,r.status FROM agent_runs r JOIN agent_run_attempts a ON a.agent_run_id=r.id WHERE r.id=? ORDER BY a.attempt_number DESC LIMIT 1")
        .bind(run_id).fetch_optional(pool).await?;
    let Some((request, attempt, status)) = row else {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM agent_runs WHERE id=?)")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
        if exists {
            return Err(ApiError::Conflict(
                "Handover canonical attempt is missing; replacement launch is forbidden".into(),
            ));
        }
        return Ok(None);
    };
    let (_, attempt_id, turn_id) = claimed_identity(event)?;
    if request.0.agent_run_id != run_id
        || request.0.session_id != event.session_id
        || request.0.turn_id != turn_id
        || request.0.request_id != event.id
        || request.0.correlation_id != event.id
        || request.0.input.role != AgentRuntimeMessageRole::System
        || attempt.0.run_attempt_id != attempt_id
        || attempt.0.turn_id != turn_id
        || attempt.0.agent_run_id != run_id
        || attempt.0.session_id != event.session_id
    {
        return Err(ApiError::Forbidden(
            "Handover canonical identity does not match its original system turn".into(),
        ));
    }
    Ok(Some((request.0, attempt.0, status)))
}

#[derive(Default, FromRow)]
struct LaunchProof {
    untouched_registry: bool,
    credential_issued: bool,
}
impl LaunchProof {
    fn can_launch(&self) -> bool {
        self.untouched_registry && !self.credential_issued
    }
}
async fn launch_proof(
    pool: &SqlitePool,
    event: &WorkflowHandoverEvent,
    attempt_id: Uuid,
) -> Result<LaunchProof, ApiError> {
    Ok(sqlx::query_as("SELECT EXISTS(SELECT 1 FROM agent_process_registry WHERE run_attempt_id=? AND registry_status='reserved' AND pid IS NULL AND host_pid IS NULL AND host_endpoint IS NULL AND host_token IS NULL AND process_started_at IS NULL) AS untouched_registry,EXISTS(SELECT 1 FROM workflow_main_session_bindings WHERE session_id=? AND actual_main_run_attempt_id=?) AS credential_issued")
        .bind(attempt_id).bind(event.session_id).bind(attempt_id).fetch_one(pool).await?)
}

async fn session_has_other_active_run(
    pool: &SqlitePool,
    event: &WorkflowHandoverEvent,
) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM agent_runs WHERE session_id=? AND id IS NOT ? AND status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed'))")
        .bind(event.session_id).bind(event.agent_run_id).fetch_one(pool).await?)
}
async fn previous_processes_settled(
    deployment: &DeploymentImpl,
    event: &WorkflowHandoverEvent,
) -> Result<bool, ApiError> {
    let attempts: Vec<Uuid> = sqlx::query_scalar("SELECT a.id FROM agent_runs r JOIN agent_run_attempts a ON a.agent_run_id=r.id WHERE r.session_id=? AND r.id IS NOT ?")
        .bind(event.session_id).bind(event.agent_run_id).fetch_all(&deployment.db().pool).await?;
    for id in attempts {
        if !deployment
            .agent_run_port()
            .reconcile_terminal_process(id)
            .await
            .map_err(port_error)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn verify_files(
    pool: &SqlitePool,
    root: &Path,
    event: &WorkflowHandoverEvent,
    source: &HandoverSource,
) -> Result<DispatchScope, ApiError> {
    let reference = WorkflowFileRef {
        path: source.manifest_path.clone(),
        sha256: source.manifest_sha256.clone(),
    };
    let bytes = read_bound_file(root, &reference).await?;
    let manifest: SplitTaskManifest = serde_json::from_slice(&bytes).map_err(|error| {
        ApiError::BadRequest(format!("Accepted split manifest is invalid: {error}"))
    })?;
    validate_manifest(&manifest).map_err(ApiError::BadRequest)?;
    if manifest.run_id != event.run_id.to_string()
        || manifest.split_execution_id != source.node_execution_id.to_string()
    {
        return Err(ApiError::Conflict(
            "Accepted split manifest has a different execution identity".into(),
        ));
    }
    let bindings: Vec<(String, String, String)> = sqlx::query_as("SELECT subtask_id,instruction_path,instruction_sha256 FROM workflow_split_subtasks WHERE batch_id=?")
        .bind(event.batch_id).fetch_all(pool).await?;
    if bindings.len() != manifest.tasks.len() {
        return Err(ApiError::Conflict(
            "Accepted split membership is inconsistent".into(),
        ));
    }
    for task in manifest.tasks {
        let (_, path, hash) = bindings
            .iter()
            .find(|(id, _, _)| *id == task.subtask_id)
            .ok_or_else(|| {
                ApiError::Conflict("Accepted split instruction binding is missing".into())
            })?;
        if *path != task.instruction_path {
            return Err(ApiError::Conflict(
                "Accepted split instruction path changed".into(),
            ));
        }
        read_bound_file(
            root,
            &WorkflowFileRef {
                path: path.clone(),
                sha256: hash.clone(),
            },
        )
        .await?;
    }
    let graph: WorkflowGraph = serde_json::from_str(&source.graph_snapshot).map_err(|error| {
        ApiError::Conflict(format!("Frozen workflow graph is invalid: {error}"))
    })?;
    dispatch_scope(&graph, &source.node_id).map_err(ApiError::BadRequest)
}

fn handover_prompt(
    event: &WorkflowHandoverEvent,
    source: &HandoverSource,
    scope: &DispatchScope,
) -> String {
    let candidates = serde_json::to_string(&scope.candidate_node_ids).expect("Node IDs serialize");
    let dispatch_path = format!(
        ".vibe-kanban/workflows/{}/splits/{}/dispatch.json",
        event.run_id, source.node_execution_id
    );
    format!(
        "System workflow dispatch handover, not a user message.\nRun: {}\nBatch: {}\nSplit execution: {}\nAccepted manifest: {}\nManifest SHA-256: {}\nFixed candidate Node IDs: {candidates}\nFixed summary exit: {}\n\nCall workflow_get for this Run to inspect its frozen nodes and their responsibilities. Actually read the manifest, each instruction_path and the necessary referenced material files from the project directory. Decide task assignments and serial/parallel stages within the fixed candidate nodes. Do not copy task bodies or upstream results into tool parameters.\n\nWrite {dispatch_path} as JSON: schema_version=1, run_id and split_execution_id exactly as above, manifest_sha256 as above, stages=[{{\"assignments\":[{{\"subtask_id\":\"manifest-task-id\",\"node_id\":\"fixed-candidate-id\"}}]}}]. Stages execute serially; assignments inside a stage execute in parallel. Assign each task exactly once and put dependent tasks in later stages.\n\nExplicitly call workflow_dispatch with batch_id={}, request_id=\"handover:{}\", and dispatch_path=\"{dispatch_path}\". Check its accepted result. Writing the file alone is not submission. Dispatch advances this existing Run; do not call workflow_submit to create another Run, create business Tasks, add Nodes, change the graph, or launch Agents directly. If dispatch cannot be accepted, report the actual reason and leave the failed work visible.",
        event.run_id,
        event.batch_id,
        source.node_execution_id,
        source.manifest_path,
        source.manifest_sha256,
        scope.summary_node_id,
        event.batch_id,
        event.id
    )
}

async fn finish_terminal(
    pool: &SqlitePool,
    event: &WorkflowHandoverEvent,
    status: AgentRunStatus,
) -> Result<(), ApiError> {
    fail_handover(pool, event, &format!("Main Agent dispatch turn ended ({status:?}) without an accepted workflow_dispatch; dispatch was not completed")).await
}

/// Dispatch acceptance and terminal failure race in one write transaction.
/// Preserve a valid late acceptance, never erase already-dispatched work.
async fn fail_handover(
    pool: &SqlitePool,
    event: &WorkflowHandoverEvent,
    reason: &str,
) -> Result<(), ApiError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let (dispatched, stopped): (bool, bool) = sqlx::query_as("SELECT b.dispatch_sha256 IS NOT NULL,r.status IN ('cancelling','canceled') OR EXISTS(SELECT 1 FROM workflow_stop_intents i WHERE i.run_id=r.id) FROM workflow_split_batches b JOIN workflow_runs r ON r.id=b.run_id WHERE b.id=?")
        .bind(event.batch_id).fetch_one(&mut *tx).await?;
    sqlx::query("UPDATE workflow_handover_events SET state=?,error_text=?,updated_at=datetime('now','subsec') WHERE id=? AND state IN ('pending','claimed')")
        .bind(if dispatched { "completed" } else if stopped { "cancelled" } else { "failed" }).bind(if dispatched || stopped { None } else { Some(reason) }).bind(event.id).execute(&mut *tx).await?;
    if !dispatched {
        let changed = sqlx::query("UPDATE workflow_split_batches SET state=?,error_text=?,updated_at=datetime('now','subsec') WHERE id=? AND state='accepted' AND dispatch_sha256 IS NULL")
            .bind(if stopped { "cancelled" } else { "failed" }).bind(if stopped { None } else { Some(reason) }).bind(event.batch_id).execute(&mut *tx).await?.rows_affected();
        if changed == 1 {
            sqlx::query("UPDATE workflow_split_subtasks SET state=?,error_text=?,updated_at=datetime('now','subsec') WHERE batch_id=? AND state='pending'")
                .bind(if stopped { "cancelled" } else { "blocked" }).bind(if stopped { None } else { Some(reason) }).bind(event.batch_id).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Cancel only the event-owned system turn, even if the Session now contains
/// another user turn. Call outside the workflow run lock.
pub async fn cancel_for_run(deployment: &DeploymentImpl, run_id: Uuid) -> Result<(), ApiError> {
    let pool = &deployment.db().pool;
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM workflow_handover_events WHERE run_id=?")
            .bind(run_id)
            .fetch_all(pool)
            .await?;
    let mut first_error = None;
    for id in ids {
        let result: Result<(), ApiError> = async {
            let Some(initial) = WorkflowHandoverEvent::find_by_id(pool, id).await? else {
                return Ok(());
            };
            let _guard = deployment
                .queued_message_service()
                .lock_session(initial.session_id)
                .await;
            if let Some(event) = WorkflowHandoverEvent::find_by_id(pool, id).await? {
                cancel_one(deployment, &event).await?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            // One unreachable manager cannot prevent stopping the other owned
            // turns. Retain the failure so the parent still holds occupancy.
            let _ = WorkflowHandoverEvent::set_error(pool, id, Some(&error.to_string())).await;
            tracing::warn!(handover_id=%id, %run_id, %error, "Workflow handover stop is not confirmed; other owned turns continue stopping");
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

async fn cancel_one(
    deployment: &DeploymentImpl,
    event: &WorkflowHandoverEvent,
) -> Result<(), ApiError> {
    let pool = &deployment.db().pool;
    let Some((request, attempt, status)) = owned_turn(pool, event).await? else {
        WorkflowHandoverEvent::finish(pool, event.id, "cancelled", None).await?;
        return Ok(());
    };
    let port = deployment.agent_run_port();
    if status.is_terminal() {
        if !port
            .reconcile_terminal_process(attempt.run_attempt_id)
            .await
            .map_err(port_error)?
        {
            port.stop_terminal_process_for_deletion(request.agent_run_id, attempt.run_attempt_id)
                .await
                .map_err(port_error)?;
        }
    } else {
        port.control(AgentRunPortCommandEnvelope {
            schema_version: ORCHESTRATION_COMMAND_SCHEMA_VERSION,
            command_id: event.id,
            idempotency_key: format!("workflow-handover:{}:cancel", event.id),
            agent_run_id: request.agent_run_id,
            orchestration_run_id: None,
            orchestration_node_execution_id: None,
            correlation_id: event.id,
            created_at: event.created_at,
            command: AgentRunPortCommand::Cancel {
                reason: "Parent workflow stop requested".into(),
            },
        })
        .await
        .map_err(port_error)?;
        let snapshot = port.query(request.agent_run_id).await.map_err(port_error)?;
        if !snapshot.state.status.is_terminal()
            || !port
                .reconcile_terminal_process(attempt.run_attempt_id)
                .await
                .map_err(port_error)?
        {
            WorkflowHandoverEvent::set_error(pool, event.id, Some("Workflow-owned main Agent turn is stopping; waiting for its actual process exit")).await?;
            return Ok(());
        }
    }
    // A dispatch that was accepted before stopping remains a historical fact.
    let dispatched: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_split_batches WHERE id=? AND dispatch_sha256 IS NOT NULL)").bind(event.batch_id).fetch_one(pool).await?;
    WorkflowHandoverEvent::finish(
        pool,
        event.id,
        if dispatched { "completed" } else { "cancelled" },
        None,
    )
    .await?;
    Ok(())
}

fn port_error(error: executors::runtime::AgentRunPortError) -> ApiError {
    ApiError::BadGateway(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE workflow_runs(id BLOB PRIMARY KEY,status TEXT NOT NULL); CREATE TABLE workflow_split_batches(id BLOB PRIMARY KEY,run_id BLOB,dispatch_sha256 TEXT,state TEXT,error_text TEXT,updated_at TEXT); CREATE TABLE workflow_handover_events(id BLOB PRIMARY KEY,run_id BLOB,session_id BLOB,agent_run_id BLOB,run_attempt_id BLOB,turn_id BLOB,state TEXT,error_text TEXT,updated_at TEXT); CREATE TABLE workflow_split_subtasks(batch_id BLOB,state TEXT,error_text TEXT,updated_at TEXT); CREATE TABLE workflow_stop_intents(run_id BLOB); CREATE TABLE agent_runs(id BLOB PRIMARY KEY,session_id BLOB,status TEXT,request_envelope TEXT); CREATE TABLE agent_run_attempts(id BLOB PRIMARY KEY,agent_run_id BLOB,attempt_number INTEGER,status TEXT,request_envelope TEXT); CREATE TABLE agent_process_registry(run_attempt_id BLOB PRIMARY KEY,registry_status TEXT,pid INTEGER,host_pid INTEGER,host_endpoint TEXT,host_token TEXT,process_started_at TEXT); CREATE TABLE workflow_main_session_bindings(session_id BLOB PRIMARY KEY,actual_main_run_attempt_id BLOB);")
            .execute(&pool).await.unwrap();
        pool
    }

    fn event() -> WorkflowHandoverEvent {
        WorkflowHandoverEvent {
            id: Uuid::new_v4(),
            batch_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            state: "claimed".into(),
            agent_run_id: Some(Uuid::new_v4()),
            run_attempt_id: Some(Uuid::new_v4()),
            turn_id: Some(Uuid::new_v4()),
            error_text: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    async fn seed(pool: &SqlitePool, event: &WorkflowHandoverEvent) {
        sqlx::query("INSERT INTO workflow_runs VALUES (?,'running')")
            .bind(event.run_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_split_batches(id,run_id,state) VALUES (?,?,'accepted')")
            .bind(event.batch_id)
            .bind(event.run_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_handover_events(id,run_id,session_id,agent_run_id,run_attempt_id,turn_id,state) VALUES (?,?,?,?,?,?,'claimed')")
            .bind(event.id).bind(event.run_id).bind(event.session_id).bind(event.agent_run_id).bind(event.run_attempt_id).bind(event.turn_id).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO workflow_split_subtasks(batch_id,state) VALUES (?,'pending')")
            .bind(event.batch_id)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn terminal_main_turn_without_dispatch_fails_only_the_undispatched_batch() {
        let pool = pool().await;
        let event = event();
        seed(&pool, &event).await;
        finish_terminal(&pool, &event, AgentRunStatus::Succeeded)
            .await
            .unwrap();
        let (state, error): (String, String) =
            sqlx::query_as("SELECT state,error_text FROM workflow_split_batches WHERE id=?")
                .bind(event.batch_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "failed");
        assert!(error.contains("without an accepted workflow_dispatch"));
        let state: String =
            sqlx::query_scalar("SELECT state FROM workflow_split_subtasks WHERE batch_id=?")
                .bind(event.batch_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "blocked");
        // Replay preserves the original failure, rather than scheduling a new turn.
        finish_terminal(&pool, &event, AgentRunStatus::Failed)
            .await
            .unwrap();
        let replay: String =
            sqlx::query_scalar("SELECT error_text FROM workflow_split_batches WHERE id=?")
                .bind(event.batch_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(replay, error);
    }

    #[tokio::test]
    async fn accepted_dispatch_wins_the_terminal_failure_race_and_preserves_work() {
        let pool = pool().await;
        let event = event();
        seed(&pool, &event).await;
        sqlx::query("UPDATE workflow_split_batches SET state='dispatching',dispatch_sha256='accepted-hash' WHERE id=?").bind(event.batch_id).execute(&pool).await.unwrap();
        sqlx::query("UPDATE workflow_split_subtasks SET state='queued' WHERE batch_id=?")
            .bind(event.batch_id)
            .execute(&pool)
            .await
            .unwrap();
        finish_terminal(&pool, &event, AgentRunStatus::Failed)
            .await
            .unwrap();
        let state: String =
            sqlx::query_scalar("SELECT state FROM workflow_handover_events WHERE id=?")
                .bind(event.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "completed");
        let state: String =
            sqlx::query_scalar("SELECT state FROM workflow_split_subtasks WHERE batch_id=?")
                .bind(event.batch_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "queued");
    }

    #[tokio::test]
    async fn stop_intent_wins_without_misreporting_dispatch_failure() {
        let pool = pool().await;
        let event = event();
        seed(&pool, &event).await;
        sqlx::query("INSERT INTO workflow_stop_intents VALUES (?)")
            .bind(event.run_id)
            .execute(&pool)
            .await
            .unwrap();
        finish_terminal(&pool, &event, AgentRunStatus::Cancelled)
            .await
            .unwrap();
        let state: String =
            sqlx::query_scalar("SELECT state FROM workflow_handover_events WHERE id=?")
                .bind(event.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "cancelled");
        let state: String =
            sqlx::query_scalar("SELECT state FROM workflow_split_subtasks WHERE batch_id=?")
                .bind(event.batch_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "cancelled");
    }

    #[tokio::test]
    async fn launch_recovery_needs_both_untouched_registry_and_unissued_credential() {
        let pool = pool().await;
        let event = event();
        let attempt_id = event.run_attempt_id.unwrap();
        assert!(
            !launch_proof(&pool, &event, attempt_id)
                .await
                .unwrap()
                .can_launch()
        );
        sqlx::query("INSERT INTO agent_process_registry(run_attempt_id,registry_status) VALUES (?,'reserved')").bind(attempt_id).execute(&pool).await.unwrap();
        assert!(
            launch_proof(&pool, &event, attempt_id)
                .await
                .unwrap()
                .can_launch()
        );
        sqlx::query("INSERT INTO workflow_main_session_bindings VALUES (?,?)")
            .bind(event.session_id)
            .bind(attempt_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !launch_proof(&pool, &event, attempt_id)
                .await
                .unwrap()
                .can_launch()
        );
        sqlx::query("DELETE FROM workflow_main_session_bindings")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE agent_process_registry SET host_endpoint='already-handed-off' WHERE run_attempt_id=?").bind(attempt_id).execute(&pool).await.unwrap();
        assert!(
            !launch_proof(&pool, &event, attempt_id)
                .await
                .unwrap()
                .can_launch()
        );
    }

    #[tokio::test]
    async fn completion_never_releases_a_live_unreachable_or_missing_process() {
        let pool = pool().await;
        let event = event();
        seed(&pool, &event).await;
        sqlx::query("UPDATE workflow_handover_events SET state='completed' WHERE id=?")
            .bind(event.id)
            .execute(&pool)
            .await
            .unwrap();
        let run_id = event.agent_run_id.unwrap();
        let attempt_id = event.run_attempt_id.unwrap();
        sqlx::query("INSERT INTO agent_runs(id,session_id,status) VALUES (?,?,'succeeded')")
            .bind(run_id)
            .bind(event.session_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO agent_run_attempts(id,agent_run_id,status) VALUES (?,?,'succeeded')",
        )
        .bind(attempt_id)
        .bind(run_id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(has_unsettled(&pool, event.run_id).await.unwrap());
        sqlx::query("INSERT INTO agent_process_registry(run_attempt_id,registry_status) VALUES (?,'running')").bind(attempt_id).execute(&pool).await.unwrap();
        assert!(has_unsettled(&pool, event.run_id).await.unwrap());
        sqlx::query("UPDATE agent_process_registry SET registry_status='unreachable'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(has_unsettled(&pool, event.run_id).await.unwrap());
        sqlx::query("UPDATE agent_process_registry SET registry_status='exited'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!has_unsettled(&pool, event.run_id).await.unwrap());
    }

    #[tokio::test]
    async fn unrelated_user_turn_is_busy_but_is_never_owned_by_the_handover() {
        let pool = pool().await;
        let event = event();
        seed(&pool, &event).await;
        let user_run = Uuid::new_v4();
        sqlx::query("INSERT INTO agent_runs(id,session_id,status) VALUES (?,?,'running')")
            .bind(user_run)
            .bind(event.session_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(session_has_other_active_run(&pool, &event).await.unwrap());
        assert!(owned_turn(&pool, &event).await.unwrap().is_none());
        sqlx::query("UPDATE agent_runs SET status='succeeded' WHERE id=?")
            .bind(user_run)
            .execute(&pool)
            .await
            .unwrap();
        assert!(!session_has_other_active_run(&pool, &event).await.unwrap());
    }

    #[test]
    fn automatic_prompt_carries_file_identities_and_a_fixed_dispatch_scope() {
        let event = event();
        let source = HandoverSource {
            attempt_id: Some(Uuid::new_v4()),
            workspace_id: Some(Uuid::new_v4()),
            main_session_id: Some(event.session_id),
            run_status: "running".into(),
            batch_state: "accepted".into(),
            source_status: "succeeded".into(),
            node_execution_id: Uuid::new_v4(),
            node_id: "split".into(),
            graph_snapshot: "{}".into(),
            manifest_path: "materials/tasks.json".into(),
            manifest_sha256: "f".repeat(64),
            dispatch_sha256: None,
            failure_requested: false,
            stop_requested: false,
            project_slot_held: true,
        };
        let scope = DispatchScope {
            splitter_node_id: "split".into(),
            candidate_node_ids: vec!["research".into(), "review".into()],
            summary_node_id: "summary".into(),
        };
        let prompt = handover_prompt(&event, &source, &scope);
        assert!(prompt.contains("System workflow dispatch handover, not a user message"));
        assert!(prompt.contains("Actually read the manifest"));
        assert!(prompt.contains("materials/tasks.json"));
        assert!(prompt.contains("[\"research\",\"review\"]"));
        assert!(prompt.contains(&format!("request_id=\"handover:{}\"", event.id)));
        assert!(prompt.contains("do not call workflow_submit"));
    }
}
