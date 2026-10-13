//! Durable fixed-graph split batches. Files contain the work; these rows only
//! assign identities, stages, independent Sessions and actual execution facts.
use workflow::{
    dispatch::{BatchSubtaskSnapshot, plan_batch, validate_dispatch},
    files::{DispatchPlan, WorkflowFileRef},
};

use super::*;

#[derive(Default)]
pub(super) struct BatchProgress {
    pub waiting: bool,
    pub failed: bool,
    pub changed: bool,
}

/// One tick starts every ready independent item without waiting for a sibling
/// to finish. Completion and stage advancement are driven by canonical facts.
#[allow(clippy::too_many_arguments)]
pub(super) async fn progress<A: WorkflowAgentExecutor, R: WorkflowArenaCreator>(
    pool: &SqlitePool,
    run_id: Uuid,
    graph: &WorkflowGraph,
    issue_id: Uuid,
    workspace_id: Uuid,
    run_input_text: &str,
    agent_executor: &A,
    arena_creator: &R,
) -> Result<BatchProgress, ApiError> {
    synchronize(pool, run_id).await?;
    let batches = sqlx::query("SELECT b.*,n.node_id AS splitter_node_id FROM workflow_split_batches b JOIN node_executions n ON n.id=b.node_execution_id WHERE b.run_id=? ORDER BY b.created_at,b.id")
        .bind(run_id).fetch_all(pool).await?;
    let mut result = BatchProgress::default();
    for batch in batches {
        let batch_id: Uuid = batch.try_get("id")?;
        let state: String = batch.try_get("state")?;
        if matches!(state.as_str(), "failed" | "cancelled") {
            result.failed = true;
            continue;
        }
        if state == "succeeded" {
            continue;
        }
        if matches!(state.as_str(), "pending" | "accepted") {
            result.waiting = true;
            continue;
        }
        let splitter: String = batch.try_get("splitter_node_id")?;
        let scope = dispatch_scope(graph, &splitter).map_err(ApiError::BadRequest)?;
        let partial = batch
            .try_get::<Option<Uuid>, _>("source_batch_id")?
            .is_some()
            && state == "partial";
        if let Err(error) = validate_accepted_plan(pool, workspace_id, graph, batch_id).await {
            fail_batch(pool, batch_id, &error.to_string()).await?;
            result.failed = true;
            result.changed = true;
            continue;
        }
        let member_rows = items(pool, batch_id).await?;
        let snapshots = member_rows
            .iter()
            .map(|row| {
                Ok(BatchSubtaskSnapshot {
                    subtask_id: row.try_get("subtask_id")?,
                    stage_index: row.try_get("stage_index")?,
                    status: subtask_status(&row.try_get::<String, _>("state")?)?,
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        let plan = plan_batch(&snapshots, false);
        for blocked in plan.blocked_subtask_ids.iter().filter(|_| !partial) {
            sqlx::query("UPDATE workflow_split_subtasks SET state='blocked',error_text='An earlier stage did not succeed',updated_at=datetime('now','subsec') WHERE batch_id=? AND subtask_id=? AND state IN ('pending','queued')")
                .bind(batch_id).bind(blocked).execute(pool).await?;
        }
        for id in &scope.candidate_node_ids {
            if !member_rows.iter().any(|row| {
                row.try_get::<Option<String>, _>("node_id")
                    .ok()
                    .flatten()
                    .as_ref()
                    == Some(id)
            }) {
                sqlx::query("INSERT OR IGNORE INTO workflow_node_dispositions(run_id,node_id,disposition,reason) VALUES (?,?,'skipped','No subtask assigned to this fixed executor in the accepted batch')")
                    .bind(run_id).bind(id).execute(pool).await?;
            }
        }
        for id in plan.ready_subtask_ids.iter().filter(|_| !partial) {
            let item = member_rows
                .iter()
                .find(|row| row.try_get::<String, _>("subtask_id").ok().as_ref() == Some(id))
                .ok_or_else(|| ApiError::Conflict("Accepted batch item disappeared".into()))?;
            let node_id: Option<String> = item.try_get("node_id")?;
            let node = node_id
                .as_ref()
                .and_then(|id| graph.nodes.iter().find(|node| &node.id == id))
                .ok_or_else(|| {
                    ApiError::Conflict("Accepted subtask has no fixed executor".into())
                })?;
            if !scope.candidate_node_ids.contains(&node.id) {
                return Err(ApiError::Conflict(
                    "Accepted subtask points outside the frozen scope".into(),
                ));
            }
            let allocated = allocate_subtask(pool, run_id, workspace_id, batch_id, id, node).await;
            let iteration = match allocated {
                Ok(Some(iteration)) => iteration,
                Ok(None) => continue,
                Err(error) => {
                    sqlx::query("UPDATE workflow_split_subtasks SET state='failed',error_text=?,updated_at=datetime('now','subsec') WHERE batch_id=? AND subtask_id=? AND state IN ('pending','queued')")
                        .bind(error.to_string()).bind(batch_id).bind(id).execute(pool).await?;
                    result.changed = true;
                    continue;
                }
            };
            // Each item uses a fresh execution and its own persisted Session.
            // No business Task or project-file task entry is created here.
            let step = execute_ready_node(
                pool,
                run_id,
                graph,
                node,
                iteration,
                issue_id,
                workspace_id,
                run_input_text,
                agent_executor,
                arena_creator,
            )
            .await;
            if let Err(error) = step {
                mark_node_failed(pool, run_id, &node.id, iteration, &error.to_string()).await?;
            }
            result.changed = true;
        }
        synchronize(pool, run_id).await?;
        let latest = items(pool, batch_id).await?;
        let current = latest
            .iter()
            .map(|row| {
                Ok(BatchSubtaskSnapshot {
                    subtask_id: row.try_get("subtask_id")?,
                    stage_index: row.try_get("stage_index")?,
                    status: subtask_status(&row.try_get::<String, _>("state")?)?,
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        let plan = plan_batch(&current, false);
        if partial
            && !current
                .iter()
                .any(|item| item.status == PlannerNodeExecutionStatus::Succeeded)
        {
            fail_batch(
                pool,
                batch_id,
                "Partial delivery has no verified successful source execution",
            )
            .await?;
            result.failed = true;
            result.changed = true;
            continue;
        }
        if partial || plan.summary_ready {
            let node = graph
                .nodes
                .iter()
                .find(|node| node.id == scope.summary_node_id)
                .ok_or_else(|| ApiError::Conflict("Frozen batch summary is missing".into()))?;
            match allocate_summary(pool, run_id, workspace_id, batch_id, node).await {
                Ok(Some(iteration)) => {
                    if let Err(error) = execute_ready_node(
                        pool,
                        run_id,
                        graph,
                        node,
                        iteration,
                        issue_id,
                        workspace_id,
                        run_input_text,
                        agent_executor,
                        arena_creator,
                    )
                    .await
                    {
                        mark_node_failed(pool, run_id, &node.id, iteration, &error.to_string())
                            .await?;
                    }
                    result.changed = true;
                }
                Ok(None) => {}
                Err(error) => {
                    fail_batch(pool, batch_id, &error.to_string()).await?;
                    result.failed = true;
                    result.changed = true;
                }
            }
        } else if plan.failed && !plan.has_live_executions && plan.ready_subtask_ids.is_empty() {
            fail_batch(
                pool,
                batch_id,
                "One or more required subtasks did not succeed; normal summary is blocked",
            )
            .await?;
            result.failed = true;
        }
        synchronize(pool, run_id).await?;
        let state: String =
            sqlx::query_scalar("SELECT state FROM workflow_split_batches WHERE id=?")
                .bind(batch_id)
                .fetch_one(pool)
                .await?;
        result.waiting |= !matches!(state.as_str(), "succeeded" | "failed" | "cancelled");
        result.failed |= matches!(state.as_str(), "failed" | "cancelled");
    }
    Ok(result)
}

async fn items(pool: &SqlitePool, batch_id: Uuid) -> Result<Vec<SqliteRow>, ApiError> {
    Ok(sqlx::query("SELECT s.*,n.iteration,n.status AS execution_status FROM workflow_split_subtasks s LEFT JOIN node_executions n ON n.id=s.node_execution_id WHERE s.batch_id=? ORDER BY s.stage_index,s.subtask_id")
        .bind(batch_id).fetch_all(pool).await?)
}

fn subtask_status(state: &str) -> Result<PlannerNodeExecutionStatus, ApiError> {
    match state {
        "pending" | "queued" => Ok(PlannerNodeExecutionStatus::Pending),
        "blocked" => Ok(PlannerNodeExecutionStatus::Skipped),
        other => Ok(planner_status_from_str(other)?),
    }
}

async fn synchronize(pool: &SqlitePool, run_id: Uuid) -> Result<(), ApiError> {
    sqlx::query("UPDATE workflow_split_subtasks SET state=CASE (SELECT status FROM node_executions n WHERE n.id=workflow_split_subtasks.node_execution_id) WHEN 'pending' THEN 'queued' WHEN 'succeeded' THEN 'succeeded' WHEN 'failed' THEN 'failed' WHEN 'cancelled' THEN 'cancelled' WHEN 'skipped' THEN 'blocked' ELSE 'running' END,error_text=(SELECT error_text FROM node_executions n WHERE n.id=workflow_split_subtasks.node_execution_id),updated_at=datetime('now','subsec') WHERE batch_id IN (SELECT id FROM workflow_split_batches WHERE run_id=?) AND node_execution_id IS NOT NULL AND EXISTS(SELECT 1 FROM node_executions n WHERE n.id=workflow_split_subtasks.node_execution_id)")
        .bind(run_id).execute(pool).await?;
    sqlx::query("UPDATE workflow_split_batches SET state=CASE (SELECT status FROM node_executions n WHERE n.id=workflow_split_batches.summary_node_execution_id) WHEN 'succeeded' THEN 'succeeded' WHEN 'failed' THEN 'failed' ELSE 'cancelled' END,error_text=(SELECT error_text FROM node_executions n WHERE n.id=workflow_split_batches.summary_node_execution_id),updated_at=datetime('now','subsec') WHERE run_id=? AND state IN ('dispatching','running','partial') AND summary_node_execution_id IS NOT NULL AND EXISTS(SELECT 1 FROM node_executions n WHERE n.id=workflow_split_batches.summary_node_execution_id AND n.status IN ('succeeded','failed','cancelled'))")
        .bind(run_id).execute(pool).await?;
    Ok(())
}

async fn fail_batch(pool: &SqlitePool, batch_id: Uuid, message: &str) -> Result<(), ApiError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let run_id: Uuid = sqlx::query_scalar("SELECT run_id FROM workflow_split_batches WHERE id=?")
        .bind(batch_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("UPDATE workflow_split_batches SET state='failed',error_text=?,updated_at=datetime('now','subsec') WHERE id=? AND state NOT IN ('succeeded','cancelled')")
        .bind(message).bind(batch_id).execute(&mut *tx).await?;
    recovery::skip_undispatched_in(&mut tx, run_id, Some(batch_id), message).await?;
    sqlx::query("UPDATE workflow_split_subtasks SET state='blocked',error_text=?,updated_at=datetime('now','subsec') WHERE batch_id=? AND state IN ('pending','queued') AND (node_execution_id IS NULL OR EXISTS(SELECT 1 FROM node_executions n WHERE n.id=workflow_split_subtasks.node_execution_id AND n.status='skipped'))")
        .bind(message).bind(batch_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE workflow_handover_events SET state='cancelled',error_text=?,updated_at=datetime('now','subsec') WHERE batch_id=? AND state='pending'")
        .bind(message).bind(batch_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn validate_accepted_plan(
    pool: &SqlitePool,
    workspace_id: Uuid,
    graph: &WorkflowGraph,
    batch_id: Uuid,
) -> Result<(), ApiError> {
    let row=sqlx::query("SELECT b.*,n.node_id,n.run_id AS splitter_run_id FROM workflow_split_batches b JOIN node_executions n ON n.id=b.node_execution_id WHERE b.id=?")
        .bind(batch_id).fetch_one(pool).await?;
    let root = workflow_control_root(pool, workspace_id).await?;
    let reference = WorkflowFileRef {
        path: row.try_get("manifest_path")?,
        sha256: row.try_get("manifest_sha256")?,
    };
    let manifest: SplitTaskManifest =
        serde_json::from_slice(&file_handoff::read_bound_file(&root, &reference).await?)
            .map_err(|error| file_handoff::file_error(&reference.path, error))?;
    if manifest.run_id != row.try_get::<Uuid, _>("splitter_run_id")?.to_string()
        || manifest.split_execution_id != row.try_get::<Uuid, _>("node_execution_id")?.to_string()
    {
        return Err(ApiError::Conflict(
            "Accepted manifest does not belong to its actual source splitter execution".into(),
        ));
    }
    let reference = WorkflowFileRef {
        path: row
            .try_get::<Option<String>, _>("dispatch_path")?
            .ok_or_else(|| ApiError::Conflict("Accepted dispatch path is missing".into()))?,
        sha256: row
            .try_get::<Option<String>, _>("dispatch_sha256")?
            .ok_or_else(|| ApiError::Conflict("Accepted dispatch hash is missing".into()))?,
    };
    let plan: DispatchPlan =
        serde_json::from_slice(&file_handoff::read_bound_file(&root, &reference).await?)
            .map_err(|error| file_handoff::file_error(&reference.path, error))?;
    let scope = dispatch_scope(graph, &row.try_get::<String, _>("node_id")?)
        .map_err(ApiError::BadRequest)?;
    validate_dispatch(&scope, &manifest, &plan).map_err(ApiError::BadRequest)?;
    if plan.manifest_sha256 != row.try_get::<String, _>("manifest_sha256")? {
        return Err(ApiError::Conflict(
            "Dispatch no longer refers to its accepted manifest".into(),
        ));
    }
    let accepted = items(pool, batch_id).await?;
    if accepted.len() != manifest.tasks.len() {
        return Err(ApiError::Conflict(
            "Accepted split membership changed".into(),
        ));
    }
    for (stage, assignments) in plan.stages.iter().enumerate() {
        for assignment in &assignments.assignments {
            let row = accepted
                .iter()
                .find(|row| {
                    row.try_get::<String, _>("subtask_id").ok().as_ref()
                        == Some(&assignment.subtask_id)
                })
                .ok_or_else(|| ApiError::Conflict("Accepted subtask membership changed".into()))?;
            if row.try_get::<i64, _>("stage_index")? != stage as i64
                || row.try_get::<Option<String>, _>("node_id")?.as_ref()
                    != Some(&assignment.node_id)
            {
                return Err(ApiError::Conflict(
                    "Accepted dispatch assignment changed".into(),
                ));
            }
        }
    }
    for task in &manifest.tasks {
        let item = accepted
            .iter()
            .find(|item| {
                item.try_get::<String, _>("subtask_id").ok().as_ref() == Some(&task.subtask_id)
            })
            .ok_or_else(|| ApiError::Conflict("Accepted subtask disappeared".into()))?;
        let dependencies: Vec<String> =
            serde_json::from_str(&item.try_get::<String, _>("depends_on_json")?).map_err(|_| {
                ApiError::Conflict("Accepted subtask dependencies are invalid".into())
            })?;
        if item.try_get::<String, _>("instruction_path")? != task.instruction_path
            || item.try_get::<String, _>("title")? != task.title
            || dependencies != task.depends_on
        {
            return Err(ApiError::Conflict(
                "Accepted subtask content references changed".into(),
            ));
        }
        let instruction = WorkflowFileRef {
            path: item.try_get("instruction_path")?,
            sha256: item.try_get("instruction_sha256")?,
        };
        file_handoff::read_bound_file(&root, &instruction).await?;
        if item.try_get::<String, _>("state")? == "succeeded" {
            let source: Uuid = item
                .try_get::<Option<Uuid>, _>("node_execution_id")?
                .ok_or_else(|| {
                    ApiError::Conflict("Subtask success has no exact source execution".into())
                })?;
            file_handoff::verified(pool, &root, source).await?;
        }
    }
    Ok(())
}

fn item_identity(batch_id: Uuid, subtask_id: &str, purpose: &str) -> Uuid {
    let mut digest = Sha256::new();
    digest.update(batch_id.as_bytes());
    digest.update(subtask_id.as_bytes());
    digest.update(purpose.as_bytes());
    let bytes: [u8; 16] = digest.finalize()[..16].try_into().expect("SHA-256 prefix");
    Uuid::from_bytes(bytes)
}

async fn allocate_subtask(
    pool: &SqlitePool,
    run_id: Uuid,
    workspace_id: Uuid,
    batch_id: Uuid,
    subtask_id: &str,
    node: &WorkflowNode,
) -> Result<Option<i64>, ApiError> {
    let working_dir = Session::resolve_agent_working_dir(pool, workspace_id).await?;
    let executor = executor_config_from_node(node.data.executor_config.clone())
        .await?
        .executor
        .to_string();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if !accepting_work_in(&mut tx, run_id).await? {
        return Ok(None);
    }
    let row=sqlx::query("SELECT node_execution_id,session_id,title,state,source_subtask_id FROM workflow_split_subtasks WHERE batch_id=? AND subtask_id=?")
        .bind(batch_id).bind(subtask_id).fetch_one(&mut *tx).await?;
    if let Some(execution_id) = row.try_get::<Option<Uuid>, _>("node_execution_id")? {
        let (iteration, status): (i64, String) = sqlx::query_as(
            "SELECT iteration,status FROM node_executions WHERE id=? AND run_id=? AND node_id=?",
        )
        .bind(execution_id)
        .bind(run_id)
        .bind(&node.id)
        .fetch_one(&mut *tx)
        .await?;
        return Ok((status == "pending").then_some(iteration));
    }
    if row.try_get::<String, _>("state")? != "queued" {
        return Ok(None);
    }
    let execution_id = item_identity(batch_id, subtask_id, "execution");
    let original_session = row.try_get::<Option<Uuid>, _>("session_id")?;
    let source_subtask_id = row.try_get::<Option<Uuid>, _>("source_subtask_id")?;
    // Lineage also exists for descendants blocked before their first launch.
    // Only an item with a previous allocation must retain its original Session;
    // follow every clone so a partial-delivery Run cannot hide that allocation.
    let previously_allocated = if original_session.is_none() {
        if let Some(source_id) = source_subtask_id {
            has_original_allocation_in(&mut tx, source_id).await?
        } else {
            false
        }
    } else {
        false
    };
    if previously_allocated {
        return Err(ApiError::Conflict(
            "The subtask's original Session is unavailable; retry cannot replace it".into(),
        ));
    }
    let session_id =
        original_session.unwrap_or_else(|| item_identity(batch_id, subtask_id, "session"));
    if original_session.is_none() {
        sqlx::query("INSERT INTO sessions(id,workspace_id,name,executor,agent_working_dir) VALUES (?,?,?,?,?) ON CONFLICT(id) DO NOTHING")
            .bind(session_id).bind(workspace_id).bind(row.try_get::<String,_>("title")?).bind(executor).bind(working_dir)
            .execute(&mut *tx).await?;
    }
    let valid: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=? AND workspace_id=?)")
            .bind(session_id)
            .bind(workspace_id)
            .fetch_one(&mut *tx)
            .await?;
    if !valid {
        return Err(ApiError::Conflict(
            "Internal subtask Session belongs to another workspace".into(),
        ));
    }
    sqlx::query("UPDATE session_project_memberships SET project_id=(SELECT t.project_id FROM workflow_runs r JOIN workflow_attempts a ON a.id=r.attempt_id JOIN tasks t ON t.id=a.task_id WHERE r.id=?) WHERE session_id=? AND EXISTS(SELECT 1 FROM workflow_runs r JOIN workflow_attempts a ON a.id=r.attempt_id JOIN tasks t ON t.id=a.task_id WHERE r.id=?)")
        .bind(run_id).bind(session_id).bind(run_id).execute(&mut *tx).await?;
    let iteration: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(iteration),-1)+1 FROM node_executions WHERE run_id=? AND node_id=?",
    )
    .bind(run_id)
    .bind(&node.id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO node_executions(id,run_id,node_id,node_type,iteration,status,session_id) VALUES (?,?,?,'agent',?,'pending',?)")
        .bind(execution_id).bind(run_id).bind(&node.id).bind(iteration).bind(session_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE workflow_split_subtasks SET node_execution_id=?,session_id=?,updated_at=datetime('now','subsec') WHERE batch_id=? AND subtask_id=? AND node_execution_id IS NULL AND state='queued'")
        .bind(execution_id).bind(session_id).bind(batch_id).bind(subtask_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE workflow_split_batches SET state='running',updated_at=datetime('now','subsec') WHERE id=? AND state='dispatching'")
        .bind(batch_id).execute(&mut *tx).await?;
    tx.commit().await?;
    canonical_node_identity(pool, run_id, &node.id, iteration).await?;
    Ok(Some(iteration))
}

async fn has_original_allocation_in(
    conn: &mut SqliteConnection,
    source_id: Uuid,
) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("WITH RECURSIVE lineage(id,source_subtask_id,node_execution_id,session_id) AS (SELECT id,source_subtask_id,node_execution_id,session_id FROM workflow_split_subtasks WHERE id=? UNION SELECT s.id,s.source_subtask_id,s.node_execution_id,s.session_id FROM workflow_split_subtasks s JOIN lineage l ON s.id=l.source_subtask_id) SELECT EXISTS(SELECT 1 FROM lineage WHERE node_execution_id IS NOT NULL OR session_id IS NOT NULL)")
        .bind(source_id).fetch_one(conn).await?)
}

async fn allocate_summary(
    pool: &SqlitePool,
    run_id: Uuid,
    workspace_id: Uuid,
    batch_id: Uuid,
    node: &WorkflowNode,
) -> Result<Option<i64>, ApiError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if !accepting_work_in(&mut tx, run_id).await? {
        return Ok(None);
    }
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT summary_node_execution_id FROM workflow_split_batches WHERE id=?",
    )
    .bind(batch_id)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(id) = existing {
        let (iteration, status): (i64, String) = sqlx::query_as(
            "SELECT iteration,status FROM node_executions WHERE id=? AND run_id=? AND node_id=?",
        )
        .bind(id)
        .bind(run_id)
        .bind(&node.id)
        .fetch_one(&mut *tx)
        .await?;
        return Ok((status == "pending").then_some(iteration));
    }
    let execution_id = item_identity(batch_id, "summary", "execution");
    let session_id = if node.kind == WorkflowNodeKind::Agent {
        node_session_id(node).map_err(ApiError::BadRequest)?
    } else {
        None
    };
    if node.kind == WorkflowNodeKind::Agent && session_id.is_none() {
        return Err(ApiError::Conflict(
            "Fixed summary Agent has no configured Session".into(),
        ));
    }
    if let Some(session_id) = session_id {
        let valid: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=? AND workspace_id=?)",
        )
        .bind(session_id)
        .bind(workspace_id)
        .fetch_one(&mut *tx)
        .await?;
        if !valid {
            return Err(ApiError::Conflict(
                "Fixed summary Agent's original Session is unavailable".into(),
            ));
        }
    }
    let iteration: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(iteration),-1)+1 FROM node_executions WHERE run_id=? AND node_id=?",
    )
    .bind(run_id)
    .bind(&node.id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO node_executions(id,run_id,node_id,node_type,iteration,status,session_id) VALUES (?,?,?,?,?,'pending',?)")
        .bind(execution_id).bind(run_id).bind(&node.id).bind(node_kind_value(&node.kind)).bind(iteration).bind(session_id)
        .execute(&mut *tx).await?;
    sqlx::query("UPDATE workflow_split_batches SET summary_node_execution_id=?,updated_at=datetime('now','subsec') WHERE id=? AND summary_node_execution_id IS NULL AND state IN ('dispatching','running','partial')")
        .bind(execution_id).bind(batch_id).execute(&mut *tx).await?;
    tx.commit().await?;
    canonical_node_identity(pool, run_id, &node.id, iteration).await?;
    Ok(Some(iteration))
}

async fn accepting_work_in(conn: &mut SqliteConnection, run_id: Uuid) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_runs r WHERE r.id=? AND r.status IN ('running','awaiting_human','awaiting_arena') AND NOT EXISTS(SELECT 1 FROM workflow_stop_intents s WHERE s.run_id=r.id))")
        .bind(run_id).fetch_one(conn).await?)
}

/// Resolve a particular subtask, not all outputs from a reused candidate Node.
pub(super) async fn execution_context(
    pool: &SqlitePool,
    graph: &WorkflowGraph,
    run_id: Uuid,
    node: &WorkflowNode,
    iteration: i64,
) -> Result<Option<NodeHandlerContext>, ApiError> {
    let row=sqlx::query("SELECT s.*,b.manifest_path,b.manifest_sha256,b.dispatch_path,b.dispatch_sha256,r.workspace_id FROM workflow_split_subtasks s JOIN workflow_split_batches b ON b.id=s.batch_id JOIN workflow_runs r ON r.id=b.run_id JOIN node_executions n ON n.id=s.node_execution_id WHERE n.run_id=? AND n.node_id=? AND n.iteration=? AND b.run_id=n.run_id")
        .bind(run_id).bind(&node.id).bind(iteration).fetch_optional(pool).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let batch_id: Uuid = row.try_get("batch_id")?;
    let workspace_id: Uuid = row.try_get("workspace_id")?;
    validate_accepted_plan(pool, workspace_id, graph, batch_id).await?;
    let root = workflow_control_root(pool, workspace_id).await?;
    let instruction = WorkflowFileRef {
        path: row.try_get("instruction_path")?,
        sha256: row.try_get("instruction_sha256")?,
    };
    file_handoff::read_bound_file(&root, &instruction).await?;
    let request = file_handoff::request_reference(pool, run_id).await?;
    let task_id: String = row.try_get("subtask_id")?;
    let mut upstream = vec![UpstreamOutput {
        node_id: task_id.clone(),
        output_text: format!(
            "Read this subtask's instruction file: `{}` (sha256 {}).\nRead accepted manifest `{}` and dispatch `{}`. Internal subtask ID: {task_id}; batch: {batch_id}. Work only on this assigned item; do not create business Tasks or write project.json.",
            instruction.path,
            instruction.sha256,
            row.try_get::<String, _>("manifest_path")?,
            row.try_get::<String, _>("dispatch_path")?
        ),
    }];
    let dependencies: Vec<String> =
        serde_json::from_str(&row.try_get::<String, _>("depends_on_json")?)
            .map_err(|error| ApiError::Conflict(error.to_string()))?;
    for dependency in dependencies {
        let execution:Option<Uuid>=sqlx::query_scalar("SELECT node_execution_id FROM workflow_split_subtasks WHERE batch_id=? AND subtask_id=? AND state='succeeded'")
            .bind(batch_id).bind(&dependency).fetch_optional(pool).await?.flatten();
        let execution = execution.ok_or_else(|| {
            ApiError::Conflict(format!("Required subtask `{dependency}` has not succeeded"))
        })?;
        let (index, files) = file_handoff::verified(pool, &root, execution).await?;
        upstream.push(UpstreamOutput {
            node_id: dependency,
            output_text: format!(
                "Read exact source execution {execution} handoff `{}` and files: {}",
                index.path,
                files
                    .iter()
                    .map(|file| format!("`{}`", file.path))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }
    Ok(Some(NodeHandlerContext::with_upstream_outputs(
        format!(
            "Read the workflow request `{}` using native file tools.",
            request.path
        ),
        upstream,
    )))
}

pub(super) async fn cancel_pending(pool: &SqlitePool, run_id: Uuid) -> Result<(), ApiError> {
    sqlx::query("UPDATE workflow_split_subtasks SET state='cancelled',error_text='Workflow stop requested',updated_at=datetime('now','subsec') WHERE batch_id IN (SELECT id FROM workflow_split_batches WHERE run_id=?) AND state IN ('pending','queued','blocked') AND node_execution_id IS NULL")
        .bind(run_id).execute(pool).await?;
    sqlx::query("UPDATE workflow_split_batches SET state='cancelled',updated_at=datetime('now','subsec') WHERE run_id=? AND state IN ('pending','accepted','dispatching','running','partial')")
        .bind(run_id).execute(pool).await?;
    synchronize(pool, run_id).await
}

/// A failure outside the internal batch prevents new work while allowing
/// already-running independent children to finish with their real outcomes.
pub(super) async fn abort_unstarted(
    pool: &SqlitePool,
    run_id: Uuid,
    message: &str,
) -> Result<(), ApiError> {
    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM workflow_split_batches WHERE run_id=? AND state IN ('pending','accepted','dispatching','running','partial')")
        .bind(run_id).fetch_all(pool).await?;
    for id in ids {
        fail_batch(pool, id, message).await?;
    }
    synchronize(pool, run_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn lineage_fixture() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("PRAGMA foreign_keys=ON; CREATE TABLE sessions(id BLOB PRIMARY KEY); CREATE TABLE node_executions(id BLOB PRIMARY KEY); CREATE TABLE workflow_split_subtasks(id BLOB PRIMARY KEY,source_subtask_id BLOB REFERENCES workflow_split_subtasks(id) ON DELETE RESTRICT,node_execution_id BLOB REFERENCES node_executions(id) ON DELETE SET NULL,session_id BLOB REFERENCES sessions(id) ON DELETE SET NULL)")
            .execute(&pool).await.unwrap();
        pool
    }

    async fn clone_item(
        pool: &SqlitePool,
        source: Option<Uuid>,
        execution: Option<Uuid>,
        session: Option<Uuid>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO workflow_split_subtasks VALUES (?,?,?,?)")
            .bind(id)
            .bind(source)
            .bind(execution)
            .bind(session)
            .execute(pool)
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn never_allocated_lineage_does_not_require_a_previous_session() {
        let pool = lineage_fixture().await;
        let original = clone_item(&pool, None, None, None).await;
        let partial = clone_item(&pool, Some(original), None, None).await;
        let retry = clone_item(&pool, Some(partial), None, None).await;
        let mut conn = pool.acquire().await.unwrap();
        assert!(!has_original_allocation_in(&mut conn, retry).await.unwrap());
    }

    #[tokio::test]
    async fn deleted_session_remains_required_through_unallocated_retry_clones() {
        let pool = lineage_fixture().await;
        let execution = Uuid::new_v4();
        let session = Uuid::new_v4();
        sqlx::query("INSERT INTO node_executions VALUES (?)")
            .bind(execution)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sessions VALUES (?)")
            .bind(session)
            .execute(&pool)
            .await
            .unwrap();
        let original = clone_item(&pool, None, Some(execution), Some(session)).await;
        let partial = clone_item(&pool, Some(original), None, Some(session)).await;
        let retry = clone_item(&pool, Some(partial), None, None).await;
        sqlx::query("DELETE FROM sessions WHERE id=?")
            .bind(session)
            .execute(&pool)
            .await
            .unwrap();
        let remaining_session: Option<Uuid> =
            sqlx::query_scalar("SELECT session_id FROM workflow_split_subtasks WHERE id=?")
                .bind(original)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(remaining_session.is_none());
        let mut conn = pool.acquire().await.unwrap();
        assert!(has_original_allocation_in(&mut conn, retry).await.unwrap());
    }
}
