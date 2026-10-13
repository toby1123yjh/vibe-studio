//! Durable project-wide scheduling shared by the UI and integration routes.
use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use db::models::workflow_queue::WorkflowQueueEntry;
use deployment::Deployment;
use uuid::Uuid;

use super::{
    arena::DeploymentWorkflowArenaCreator,
    handover,
    management::{deliver_stop_intents, resolve_waiting_submissions},
    runner::{
        AgentRunReconciliationBoundary, DeploymentAgentRunReconciliationBoundary,
        DeploymentWorkflowAgentExecutor, DeploymentWorkflowRunCanceller,
        fail_accepted_workflow_run, reconcile_workflow_run_with_arena_and_boundary,
        start_accepted_workflow_run,
    },
};
use crate::{DeploymentImpl, error::ApiError, routes::workflows::sync_attempt_from_run};

pub fn spawn_dispatcher(deployment: DeploymentImpl) {
    tokio::spawn(async move {
        let mut recover_active = true;
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            match recover_starting(&deployment, recover_active).await {
                Ok(()) => recover_active = false,
                Err(error) => tracing::warn!(%error,"Workflow queue recovery will retry"),
            }
            if let Err(error) = tick(&deployment).await {
                tracing::warn!(%error,"Workflow queue reconciliation failed; retaining project slots");
            }
        }
    });
}

fn in_flight() -> &'static Mutex<HashSet<Uuid>> {
    static RUNS: OnceLock<Mutex<HashSet<Uuid>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashSet::new()))
}

pub(super) fn is_starting(run_id: Uuid) -> bool {
    in_flight()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&run_id)
}

struct StartingGuard(Uuid);
impl Drop for StartingGuard {
    fn drop(&mut self) {
        in_flight()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

fn schedule(deployment: &DeploymentImpl, id: Uuid, recover: bool) {
    if !in_flight()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id)
    {
        return;
    }
    let deployment = deployment.clone();
    tokio::spawn(async move {
        let _guard = StartingGuard(id);
        let result = if recover {
            recover_one(&deployment, id).await
        } else {
            start_one(&deployment, id).await
        };
        if let Err(error) = result {
            tracing::warn!(%id,%error,"Workflow startup recovery will retry; slot retained");
        }
    });
}

async fn recover_starting(
    deployment: &DeploymentImpl,
    include_active: bool,
) -> Result<(), ApiError> {
    let pool = &deployment.db().pool;
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT q.run_id FROM workflow_run_queue q JOIN workflow_runs r ON r.id=q.run_id WHERE (q.phase='starting' OR (? AND q.phase='active')) AND r.status IN ('pending','running','awaiting_human','awaiting_arena')")
        .bind(include_active).fetch_all(pool).await?;
    for id in ids {
        schedule(deployment, id, true);
    }
    Ok(())
}

async fn recover_one(deployment: &DeploymentImpl, id: Uuid) -> Result<(), ApiError> {
    let pool = &deployment.db().pool;
    sqlx::query("UPDATE workflow_run_queue SET phase='starting' WHERE run_id=? AND phase='active'")
        .bind(id)
        .execute(pool)
        .await?;
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM orchestration_runs WHERE id=?")
        .bind(id)
        .fetch_one(pool)
        .await?;
    if exists == 0 {
        // No orchestration identity means no child command could have been
        // dispatched. Keep the same run/slot and safely continue startup.
        sqlx::query("UPDATE workflow_runs SET status='pending' WHERE id=? AND status='running'")
            .bind(id)
            .execute(pool)
            .await?;
        start_one(deployment, id).await?;
    } else {
        // Do not re-launch when dispatch may already have happened. Repair
        // the reference and reconcile the durable canonical authority.
        sqlx::query("UPDATE workflow_runs SET orchestration_run_id=? WHERE id=? AND orchestration_run_id IS NULL")
                .bind(id).bind(id).execute(pool).await?;
        super::runner::recover_accepted_workflow_start(
            pool,
            id,
            &DeploymentWorkflowAgentExecutor::new(deployment.clone()),
            &DeploymentWorkflowArenaCreator::new(deployment.clone()),
        )
        .await?;
        sqlx::query("UPDATE workflow_run_queue SET phase='active' WHERE run_id=? AND phase='starting' AND EXISTS(SELECT 1 FROM workflow_runs WHERE id=? AND status IN ('running','awaiting_human','awaiting_arena','cancelling'))")
            .bind(id).bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn start_one(deployment: &DeploymentImpl, run_id: Uuid) -> Result<(), ApiError> {
    let pool = &deployment.db().pool;
    let executor = DeploymentWorkflowAgentExecutor::new(deployment.clone());
    let arena = DeploymentWorkflowArenaCreator::new(deployment.clone());
    if let Err(error) = start_accepted_workflow_run(pool, run_id, &executor, &arena).await {
        fail_accepted_workflow_run(pool, run_id, &error.to_string()).await?;
    }
    Ok(())
}

pub async fn tick(deployment: &DeploymentImpl) -> Result<(), ApiError> {
    let pool = &deployment.db().pool;
    let executor = DeploymentWorkflowAgentExecutor::new(deployment.clone());
    let arena = DeploymentWorkflowArenaCreator::new(deployment.clone());
    let boundary = DeploymentAgentRunReconciliationBoundary::new(deployment.clone());
    // Schedule handover cleanup even if another child stop is unreachable.
    // Workers consult the durable stop fence before doing any new work.
    handover::process_pending(deployment).await?;
    deliver_stop_intents(
        pool,
        &DeploymentWorkflowRunCanceller::new(deployment.clone()),
    )
    .await?;
    let active: Vec<Uuid> = sqlx::query_scalar("SELECT run_id FROM workflow_project_slots")
        .fetch_all(pool)
        .await?;
    for id in active {
        if in_flight()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&id)
        {
            continue;
        }
        // Even a terminal parent must consult canonical children before release.
        if let Err(error) = boundary.reconcile_workflow_run(pool, id).await {
            tracing::warn!(run_id=%id,%error,"Cannot confirm workflow child state; project stays occupied");
            continue;
        }
        let result: Result<(), ApiError> = async {
            let run = reconcile_workflow_run_with_arena_and_boundary(
                pool, id, &executor, &arena, &boundary,
            )
            .await?;
            sync_attempt_from_run(pool, &run).await?;
            if !handover::has_unsettled(pool, id).await? {
                WorkflowQueueEntry::release_terminal(pool, id).await?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(%id,%error,"Workflow reconciliation failed; other projects continue");
        }
    }
    // Dependency plans become ready only after source reconciliation and slot
    // release, retaining their original FIFO sequence.
    resolve_waiting_submissions(pool).await?;
    while let Some(entry) = WorkflowQueueEntry::claim_next(pool).await? {
        schedule(deployment, entry.run_id, false);
    }
    Ok(())
}
