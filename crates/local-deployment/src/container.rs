use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::anyhow;
use async_trait::async_trait;
use command_group::AsyncGroupChild;
use db::{
    DBService,
    models::{
        execution_process::{
            ExecutionContext, ExecutionProcess, ExecutionProcessRunReason, ExecutionProcessStatus,
        },
        execution_process_repo_state::ExecutionProcessRepoState,
        repo::Repo,
        scratch::{DraftFollowUpData, Scratch, ScratchType},
        session::{Session, SessionError},
        workspace::{Workspace, WorkspaceKind},
        workspace_repo::WorkspaceRepo,
    },
};
use deployment::DeploymentError;
use executors::{
    actions::{Executable, ExecutorAction},
    env::{ExecutionEnv, RepoContext},
    executors::{
        CancellationToken, ExecutorExitSignal,
        provider_adapter::{DirectProvider, require_capability},
    },
    profile::runtime_profile_ids_match,
    provider_policy::direct_provider_capability_snapshot,
    runtime::{
        AGENT_REQUEST_PAYLOAD_VERSION, AGENT_REQUEST_SCHEMA_VERSION, AgentCapability,
        AgentRunIntent, AgentRunPort, AgentRunPortError, AgentRunRequestEnvelope, AgentRunStatus,
        AgentRuntimeMessageRole, CanonicalMessage, ProviderSessionReference, RunAttemptMode,
        RunAttemptRequest, WorkspaceMode, WorkspaceReference,
    },
};
use futures::{FutureExt, TryStreamExt, stream::select};
use git::GitService;
use services::services::{
    analytics::AnalyticsContext,
    approvals::Approvals,
    config::{Config, DEFAULT_COMMIT_REMINDER_PROMPT},
    container::{ContainerError, ContainerRef, ContainerService},
    diff_stream::{self, DiffStreamHandle},
    execution_process::{ExecutionCompletedEvent, publish_execution_completed},
    file::FileService,
    notification::NotificationService,
    queued_message::QueuedMessageService,
    remote_client::RemoteClient,
};
use sqlx::{SqlitePool, types::Json};
use tokio::{sync::RwLock, task::JoinHandle};
use tokio_util::io::ReaderStream;
use utils::{
    log_msg::LogMsg,
    msg_store::MsgStore,
    text::{git_branch_id, short_uuid},
};
use uuid::Uuid;
use workspace_manager::{RepoWorkspaceInput, WorkspaceError, WorkspaceManager};

use crate::{
    agent_process_registry::{AgentProcessRegistry, RegisteredAgentProcess},
    agent_run_port::{AgentRunTerminalEvent, LocalAgentRunPort},
    agent_runtime_supervisor, command, copy, process_supervisor,
};

const WORKSPACE_TOUCH_DEBOUNCE: Duration = Duration::from_mins(2);

fn trace_launch_diagnostic(
    execution_process_id: Uuid,
    outcome: &agent_runtime_supervisor::AgentRuntimeSupervisorOutcome,
) {
    tracing::warn!(
        execution_process_id = %execution_process_id,
        lifecycle = ?outcome.lifecycle,
        process_status = ?outcome.process_status,
        error_kind = ?outcome.runtime_error.as_ref().map(|error| error.kind),
        launch_phase = ?outcome.runtime_error.as_ref().and_then(|error| error.launch_phase),
        "classified agent runtime launch failure"
    );
}

async fn should_disable_default_commit_for_workspace(
    pool: &SqlitePool,
    workspace_id: Uuid,
) -> Result<bool, sqlx::Error> {
    use db::models::arena_group::{ArenaLifecycleStatus, ArenaMode};

    let group = Workspace::find_arena_group_for_workspace(pool, workspace_id).await?;

    Ok(matches!(
        group.map(|g| (g.mode, g.lifecycle_status)),
        Some((ArenaMode::Design, ArenaLifecycleStatus::Open))
    ))
}

#[derive(Clone)]
pub struct LocalContainerService {
    db: DBService,
    agent_run_port: LocalAgentRunPort,
    workspace_manager: WorkspaceManager,
    child_store: Arc<RwLock<HashMap<Uuid, Arc<RwLock<AsyncGroupChild>>>>>,
    cancellation_tokens: Arc<RwLock<HashMap<Uuid, CancellationToken>>>,
    msg_stores: Arc<RwLock<HashMap<Uuid, Arc<MsgStore>>>>,
    /// Tracks background tasks that stream logs to the database.
    /// When stopping execution, we await these to ensure logs are fully persisted.
    db_stream_handles: Arc<RwLock<HashMap<Uuid, JoinHandle<()>>>>,
    exit_monitor_handles: Arc<RwLock<HashMap<Uuid, JoinHandle<()>>>>,
    agent_process_registry: AgentProcessRegistry,
    workspace_touch_times: Arc<RwLock<HashMap<Uuid, Instant>>>,
    config: Arc<RwLock<Config>>,
    git: GitService,
    file_service: FileService,
    analytics: Option<AnalyticsContext>,
    approvals: Approvals,
    queued_message_service: QueuedMessageService,
    notification_service: NotificationService,
    remote_client: Option<RemoteClient>,
}

impl LocalContainerService {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        db: DBService,
        agent_run_port: LocalAgentRunPort,
        workspace_manager: WorkspaceManager,
        msg_stores: Arc<RwLock<HashMap<Uuid, Arc<MsgStore>>>>,
        config: Arc<RwLock<Config>>,
        git: GitService,
        file_service: FileService,
        analytics: Option<AnalyticsContext>,
        approvals: Approvals,
        queued_message_service: QueuedMessageService,
        remote_client: Option<RemoteClient>,
    ) -> Self {
        let child_store = Arc::new(RwLock::new(HashMap::new()));
        let cancellation_tokens = Arc::new(RwLock::new(HashMap::new()));
        let db_stream_handles = Arc::new(RwLock::new(HashMap::new()));
        let exit_monitor_handles = Arc::new(RwLock::new(HashMap::new()));
        let agent_process_registry = AgentProcessRegistry::default();
        let workspace_touch_times = Arc::new(RwLock::new(HashMap::new()));
        let notification_service = NotificationService::new(config.clone());

        let container = LocalContainerService {
            db,
            agent_run_port,
            workspace_manager,
            child_store,
            cancellation_tokens,
            msg_stores,
            db_stream_handles,
            exit_monitor_handles,
            agent_process_registry,
            workspace_touch_times,
            config,
            git,
            file_service,
            analytics,
            approvals,
            queued_message_service,
            notification_service,
            remote_client,
        };

        // A service restart must observe and re-associate persisted processes;
        // it is not an implicit shutdown policy. Only an explicit AgentRun
        // control command may terminate a managed provider process.
        container
            .reconcile_registered_agent_processes("startup")
            .await;
        container.spawn_workspace_cleanup();

        container
    }

    fn map_workspace_manager_error(err: WorkspaceError) -> ContainerError {
        match err {
            WorkspaceError::Database(err) => ContainerError::Sqlx(err),
            WorkspaceError::Worktree(err) => ContainerError::Worktree(err),
            WorkspaceError::GitService(err) => ContainerError::GitServiceError(err),
            WorkspaceError::Io(err) => ContainerError::Io(err),
            WorkspaceError::NoRepositories => {
                ContainerError::Other(anyhow!("No repositories provided"))
            }
            WorkspaceError::Repo(err) => ContainerError::Other(anyhow!(err)),
            WorkspaceError::WorkspaceNotFound => {
                ContainerError::Other(anyhow!("Workspace not found"))
            }
            WorkspaceError::RepoAlreadyAttached => {
                ContainerError::Other(anyhow!("Repository already attached to workspace"))
            }
            WorkspaceError::BranchNotFound { repo_name, branch } => ContainerError::Other(anyhow!(
                "Branch '{}' does not exist in repository '{}'",
                branch,
                repo_name
            )),
            WorkspaceError::PartialCreation(msg) => ContainerError::Other(anyhow!(msg)),
        }
    }

    async fn workspace_repo_inputs(
        &self,
        workspace_id: Uuid,
    ) -> Result<(Vec<Repo>, Vec<RepoWorkspaceInput>), ContainerError> {
        let workspace_repos =
            WorkspaceRepo::find_by_workspace_id(&self.db.pool, workspace_id).await?;
        if workspace_repos.is_empty() {
            return Err(ContainerError::Other(anyhow!(
                "Workspace has no repositories configured"
            )));
        }

        let repositories =
            WorkspaceRepo::find_repos_for_workspace(&self.db.pool, workspace_id).await?;
        let target_branches: HashMap<_, _> = workspace_repos
            .iter()
            .map(|wr| (wr.repo_id, wr.target_branch.clone()))
            .collect();

        let workspace_inputs: Vec<RepoWorkspaceInput> = repositories
            .iter()
            .map(|repo| {
                let target_branch = target_branches.get(&repo.id).cloned().ok_or_else(|| {
                    ContainerError::Other(anyhow!(
                        "Missing target branch mapping for repo {} in workspace {}",
                        repo.id,
                        workspace_id
                    ))
                })?;
                Ok(RepoWorkspaceInput::new(repo.clone(), target_branch))
            })
            .collect::<Result<_, ContainerError>>()?;

        Ok((repositories, workspace_inputs))
    }

    async fn get_child_from_store(&self, id: &Uuid) -> Option<Arc<RwLock<AsyncGroupChild>>> {
        let map = self.child_store.read().await;
        map.get(id).cloned()
    }

    async fn add_child_to_store(&self, id: Uuid, exec: AsyncGroupChild) {
        let mut map = self.child_store.write().await;
        map.insert(id, Arc::new(RwLock::new(exec)));
    }

    async fn remove_child_from_store(&self, id: &Uuid) {
        let mut map = self.child_store.write().await;
        map.remove(id);
    }

    async fn reconcile_registered_agent_processes(&self, reason: &'static str) {
        match self.agent_process_registry.reconcile().await {
            Ok(report) => {
                let mut action_counts = [0usize; 4];
                for observation in &report.observations {
                    // The child store is intentionally empty after a service
                    // restart. An alive process therefore enters read-only
                    // observation until the AgentRun supervisor can attach a
                    // provider transport; it is never treated as completed.
                    let has_live_handle = self
                        .get_child_from_store(&observation.process.runtime_id)
                        .await
                        .is_some();
                    let process_observation = match observation.presence {
                        crate::agent_process_registry::RegisteredProcessPresence::Alive => {
                            process_supervisor::ProcessObservation::Alive
                        }
                        crate::agent_process_registry::RegisteredProcessPresence::Exited => {
                            process_supervisor::ProcessObservation::Exited(None)
                        }
                        crate::agent_process_registry::RegisteredProcessPresence::Unreachable => {
                            process_supervisor::ProcessObservation::TemporarilyUnreachable
                        }
                    };
                    let action = process_supervisor::reconciliation_action(
                        process_observation,
                        has_live_handle,
                    );
                    let action_index = match action {
                        process_supervisor::ReconciliationAction::Attach => 0,
                        process_supervisor::ReconciliationAction::ObserveReadOnly => 1,
                        process_supervisor::ReconciliationAction::ConfirmExit => 2,
                        process_supervisor::ReconciliationAction::PreserveForRetry => 3,
                    };
                    action_counts[action_index] += 1;
                    tracing::debug!(
                        reason,
                        runtime_id = %observation.process.runtime_id,
                        pid = observation.process.pid,
                        presence = ?observation.presence,
                        action = ?action,
                        "observed registered agent process during restart reconciliation"
                    );
                }
                let alive = report.alive().count();
                let exited = report.exited().count();
                let unreachable = report.observations.len().saturating_sub(alive + exited);
                if !report.observations.is_empty() {
                    tracing::info!(
                        reason,
                        observed = report.observations.len(),
                        alive,
                        exited,
                        unreachable,
                        attach = action_counts[0],
                        observe_read_only = action_counts[1],
                        confirm_exit = action_counts[2],
                        preserve_for_retry = action_counts[3],
                        "reconciled registered agent processes without cleanup"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(
                    reason,
                    error = %error,
                    "failed to reconcile registered agent process registry"
                );
            }
        }
    }

    pub(crate) fn spawn_agent_run_terminal_monitor(&self) {
        let mut terminal_events = self.agent_run_port.subscribe_terminal_events();
        let container = self.clone();
        tokio::spawn(async move {
            container.recover_reserved_queued_follow_ups().await;
            let mut retry_tick = tokio::time::interval(Duration::from_secs(2));
            retry_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = retry_tick.tick() => {
                        container.recover_reserved_queued_follow_ups().await;
                        container.reconcile_queued_follow_ups().await;
                    },
                    received = terminal_events.recv() => match received {
                    Ok(event) => container.handle_agent_run_terminal(event).await,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(
                            skipped,
                            "AgentRun terminal monitor lagged; queued follow-ups may require reconciliation"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    },
                }
            }
        });
    }

    async fn recover_reserved_queued_follow_ups(&self) {
        let pending = match reserved_queued_follow_ups(&self.db.pool).await {
            Ok(pending) => pending,
            Err(error) => {
                tracing::error!(%error, "failed to load accepted queued follow-ups for recovery");
                return;
            }
        };
        for (agent_run_id, session_id) in pending {
            let _session_guard = self.queued_message_service.lock_session(session_id).await;
            // Recheck after the Session lock: a user stop or another launch may
            // have settled this reservation since the initial scan.
            if !reserved_queued_follow_ups(&self.db.pool)
                .await
                .is_ok_and(|current| current.contains(&(agent_run_id, session_id)))
            {
                continue;
            }
            let latest: Result<Option<Uuid>, sqlx::Error> = sqlx::query_scalar(
                "SELECT id FROM agent_runs WHERE session_id=? ORDER BY created_at DESC,id DESC LIMIT 1",
            ).bind(session_id).fetch_optional(&self.db.pool).await;
            match latest {
                Ok(Some(id)) if id == agent_run_id => {}
                Ok(Some(_)) => {
                    if let Err(error) = self
                        .agent_run_port
                        .fail_reserved(
                            agent_run_id,
                            "A newer Session Run superseded this queued launch".into(),
                        )
                        .await
                    {
                        tracing::warn!(%agent_run_id, %error, "superseded queued launch could not settle");
                    }
                    continue;
                }
                _ => continue,
            }
            let busy: Result<bool, sqlx::Error> = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM agent_runs r LEFT JOIN agent_run_state s ON s.agent_run_id=r.id WHERE r.session_id=? AND r.id<>? AND (s.agent_run_id IS NULL OR s.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed')))",
            ).bind(session_id).bind(agent_run_id).fetch_one(&self.db.pool).await;
            if !matches!(busy, Ok(false)) {
                continue;
            }
            let attempts: Result<Vec<Uuid>, sqlx::Error> = sqlx::query_scalar(
                "SELECT a.id FROM agent_run_attempts a JOIN agent_runs r ON r.id=a.agent_run_id WHERE r.session_id=? AND r.id<>?",
            ).bind(session_id).bind(agent_run_id).fetch_all(&self.db.pool).await;
            let Ok(attempts) = attempts else {
                continue;
            };
            let mut settled = true;
            for attempt_id in attempts {
                if !matches!(
                    self.agent_run_port
                        .reconcile_terminal_process(attempt_id)
                        .await,
                    Ok(true)
                ) {
                    settled = false;
                    break;
                }
            }
            if !settled {
                continue;
            }
            if let Err(error) = self.agent_run_port.launch_reserved(agent_run_id).await {
                tracing::warn!(%agent_run_id, %error, "accepted queued follow-up could not resume its original launch");
                // A rejected workspace/preflight must become a visible failure,
                // rather than a forever-pending Session or automatic new Run.
                if let Err(failure) = self
                    .agent_run_port
                    .fail_reserved(agent_run_id, error.to_string())
                    .await
                {
                    tracing::warn!(%agent_run_id, %failure, "queued launch outcome requires reconciliation");
                }
            }
        }
    }

    async fn reconcile_queued_follow_ups(&self) {
        for session_id in self.queued_message_service.queued_session_ids() {
            let latest: Result<Option<Uuid>, sqlx::Error> = sqlx::query_scalar(
                "SELECT id FROM agent_runs WHERE session_id=? ORDER BY created_at DESC,id DESC LIMIT 1",
            ).bind(session_id).fetch_optional(&self.db.pool).await;
            let Ok(Some(agent_run_id)) = latest else {
                continue;
            };
            if let Ok(snapshot) = self.agent_run_port.query(agent_run_id).await
                && snapshot.state.status == AgentRunStatus::Succeeded
            {
                self.handle_agent_run_terminal(AgentRunTerminalEvent {
                    agent_run_id,
                    session_id,
                    status: AgentRunStatus::Succeeded,
                })
                .await;
            }
        }
    }

    async fn handle_agent_run_terminal(&self, event: AgentRunTerminalEvent) {
        // Keep deletion excluded while the message moves from the memory
        // queue into its durable AgentRun reservation.
        let _queue_guard = self
            .queued_message_service
            .lock_session(event.session_id)
            .await;
        let Some(queued_message) = self.queued_message_service.get_queued(event.session_id) else {
            return;
        };

        if event.status != AgentRunStatus::Succeeded {
            tracing::info!(
                agent_run_id = %event.agent_run_id,
                session_id = %event.session_id,
                status = ?event.status,
                "retaining queued follow-up after unsuccessful canonical AgentRun"
            );
            return;
        }

        // A terminal outcome may arrive before the provider and host exit.
        // A delayed old event must also never start over a newer user turn.
        let busy: Result<bool,sqlx::Error> = sqlx::query_scalar(
            "SELECT NOT EXISTS(SELECT 1 FROM (SELECT id FROM agent_runs WHERE session_id=?1 ORDER BY created_at DESC,id DESC LIMIT 1) latest WHERE latest.id=?2) OR EXISTS(SELECT 1 FROM agent_runs r LEFT JOIN agent_run_state s ON s.agent_run_id=r.id WHERE r.session_id=?1 AND (s.agent_run_id IS NULL OR s.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed')))",
        ).bind(event.session_id).bind(event.agent_run_id).fetch_one(&self.db.pool).await;
        if !matches!(busy, Ok(false)) {
            return;
        }
        let attempts: Result<Vec<Uuid>,sqlx::Error> = sqlx::query_scalar(
            "SELECT a.id FROM agent_run_attempts a JOIN agent_runs r ON r.id=a.agent_run_id WHERE r.session_id=?",
        ).bind(event.session_id).fetch_all(&self.db.pool).await;
        let Ok(attempts) = attempts else {
            return;
        };
        if attempts.is_empty() {
            return;
        }
        for attempt_id in attempts {
            if !matches!(
                self.agent_run_port
                    .reconcile_terminal_process(attempt_id)
                    .await,
                Ok(true)
            ) {
                return;
            }
        }

        match self
            .start_queued_follow_up(event.session_id, &queued_message.data)
            .await
        {
            Ok(next_agent_run_id) => {
                self.queued_message_service.take_queued(event.session_id);
                if let Err(error) =
                    Scratch::delete(&self.db.pool, event.session_id, &ScratchType::DraftFollowUp)
                        .await
                {
                    tracing::warn!(session_id=%event.session_id, %error, "failed to delete consumed follow-up draft");
                }
                // The message is now a durable canonical Run. A lost launch
                // response must not put it back into the queue with a new ID.
                if let Err(error) = self.agent_run_port.launch_reserved(next_agent_run_id).await {
                    tracing::warn!(agent_run_id=%next_agent_run_id, %error, "queued follow-up was accepted but launch needs reconciliation");
                }
                tracing::info!(
                agent_run_id = %event.agent_run_id,
                next_agent_run_id = %next_agent_run_id,
                session_id = %event.session_id,
                "started queued follow-up after canonical AgentRun completion"
                );
            }
            Err(error) => tracing::error!(
                agent_run_id = %event.agent_run_id,
                session_id = %event.session_id,
                %error,
                "failed to start queued follow-up after canonical AgentRun completion"
            ),
        }
    }

    async fn remove_registered_agent_process(
        &self,
        execution_process_id: Uuid,
        reason: &'static str,
    ) {
        if let Err(error) = self
            .agent_process_registry
            .remove_runtime(execution_process_id)
            .await
        {
            tracing::warn!(
                reason,
                execution_process_id = %execution_process_id,
                error = %error,
                "failed to remove agent process registry entry"
            );
        }
    }

    async fn cleanup_registered_agent_execution(
        &self,
        execution_process_id: Uuid,
        reason: &'static str,
    ) {
        match self
            .agent_process_registry
            .cleanup_runtime(execution_process_id)
            .await
        {
            Ok(report) if !report.is_empty() => {
                tracing::info!(
                    reason,
                    execution_process_id = %execution_process_id,
                    attempted = report.attempted,
                    removed = report.removed,
                    survivors = report.survivors,
                    "cleaned registered agent process tree for execution"
                );
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(
                    reason,
                    execution_process_id = %execution_process_id,
                    error = %error,
                    "failed to clean agent process registry entry"
                );
            }
        }
    }

    async fn add_cancellation_token(&self, id: Uuid, token: CancellationToken) {
        let mut map = self.cancellation_tokens.write().await;
        map.insert(id, token);
    }

    async fn take_cancellation_token(&self, id: &Uuid) -> Option<CancellationToken> {
        let mut map = self.cancellation_tokens.write().await;
        map.remove(id)
    }

    async fn add_db_stream_handle(&self, id: Uuid, handle: JoinHandle<()>) {
        let mut map = self.db_stream_handles.write().await;
        map.insert(id, handle);
    }

    async fn take_db_stream_handle(&self, id: &Uuid) -> Option<JoinHandle<()>> {
        let mut map = self.db_stream_handles.write().await;
        map.remove(id)
    }

    async fn add_exit_monitor_handle(&self, id: Uuid, handle: JoinHandle<()>) {
        let mut map = self.exit_monitor_handles.write().await;
        map.insert(id, handle);
    }

    async fn take_exit_monitor_handle(&self, id: &Uuid) -> Option<JoinHandle<()>> {
        let mut map = self.exit_monitor_handles.write().await;
        map.remove(id)
    }

    async fn cleanup_workspace(&self, workspace: &Workspace) {
        if !workspace.can_delete_container_path() {
            tracing::info!(
                "Skipping filesystem cleanup for external workspace {}",
                workspace.id
            );
            return;
        }

        let Some(container_ref) = &workspace.container_ref else {
            return;
        };
        let workspace_dir = PathBuf::from(container_ref);

        let repositories = WorkspaceRepo::find_repos_for_workspace(&self.db.pool, workspace.id)
            .await
            .unwrap_or_default();

        if repositories.is_empty() {
            tracing::warn!(
                "No repositories found for workspace {}, cleaning up workspace directory only",
                workspace.id
            );
            if workspace_dir.exists()
                && let Err(e) = tokio::fs::remove_dir_all(&workspace_dir).await
            {
                tracing::warn!("Failed to remove workspace directory: {}", e);
            }
        } else {
            WorkspaceManager::cleanup_workspace(&workspace_dir, &repositories)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(
                        "Failed to clean up workspace for workspace {}: {}",
                        workspace.id,
                        e
                    );
                });
        }

        let _ = Workspace::mark_worktree_deleted(&self.db.pool, workspace.id).await;
    }

    async fn cleanup_expired_workspaces(&self) -> Result<(), DeploymentError> {
        if std::env::var("DISABLE_WORKTREE_CLEANUP").is_ok() {
            tracing::info!(
                "Expired workspace cleanup is disabled via DISABLE_WORKTREE_CLEANUP environment variable"
            );
            return Ok(());
        }

        let expired_workspaces = Workspace::find_expired_for_cleanup(&self.db.pool).await?;
        if expired_workspaces.is_empty() {
            tracing::debug!("No expired workspaces found");
            return Ok(());
        }
        tracing::info!(
            "Found {} expired workspaces to clean up",
            expired_workspaces.len()
        );
        for workspace in &expired_workspaces {
            self.cleanup_workspace(workspace).await;
        }
        Ok(())
    }

    fn spawn_workspace_cleanup(&self) {
        let container = self.clone();
        tokio::spawn(async move {
            container
                .workspace_manager
                .cleanup_orphan_workspaces()
                .await;

            let mut cleanup_interval =
                tokio::time::interval(tokio::time::Duration::from_secs(1800)); // 30 minutes
            loop {
                cleanup_interval.tick().await;
                tracing::info!("Starting periodic workspace cleanup...");
                container
                    .cleanup_expired_workspaces()
                    .await
                    .unwrap_or_else(|e| {
                        tracing::error!("Failed to clean up expired workspaces: {}", e)
                    });
            }
        });
    }

    /// Record the current HEAD commit for each repository as the "after" state.
    /// Errors are silently ignored since this runs after the main execution completes
    /// and failure should not block process finalization.
    async fn update_after_head_commits(&self, exec_id: Uuid) {
        if let Ok(ctx) = ExecutionProcess::load_context(&self.db.pool, exec_id).await {
            let workspace_root = self.workspace_to_current_dir(&ctx.workspace);
            for repo in &ctx.repos {
                let repo_path = workspace_root.join(&repo.name);
                if let Ok(head) = self.git().get_head_info(&repo_path) {
                    let _ = ExecutionProcessRepoState::update_after_head_commit(
                        &self.db.pool,
                        exec_id,
                        repo.id,
                        &head.oid,
                    )
                    .await;
                }
            }
        }
    }

    /// Get the commit message based on the execution run reason.
    async fn get_commit_message(&self, ctx: &ExecutionContext) -> String {
        match ctx.execution_process.run_reason {
            ExecutionProcessRunReason::CleanupScript => {
                format!("Cleanup script changes for workspace {}", ctx.workspace.id)
            }
            _ => format!(
                "Changes from execution process {}",
                ctx.execution_process.id
            ),
        }
    }

    /// Check which repos have uncommitted changes. Fails if any repo is inaccessible.
    fn check_repos_for_changes(
        &self,
        workspace_root: &Path,
        repos: &[Repo],
    ) -> Result<Vec<(Repo, PathBuf)>, ContainerError> {
        let git = GitService::new();
        let mut repos_with_changes = Vec::new();

        for repo in repos {
            let worktree_path = workspace_root.join(&repo.name);

            match git.get_worktree_status(&worktree_path) {
                Ok(ws) if !ws.entries.is_empty() => {
                    repos_with_changes.push((repo.clone(), worktree_path));
                }
                Ok(_) => {
                    tracing::debug!("No changes in repo '{}'", repo.name);
                }
                Err(e) => {
                    return Err(ContainerError::Other(anyhow!(
                        "Pre-flight check failed for repo '{}': {}",
                        repo.name,
                        e
                    )));
                }
            }
        }

        Ok(repos_with_changes)
    }

    /// Commit changes to each repo. Logs failures but continues with other repos.
    fn commit_repos(&self, repos_with_changes: Vec<(Repo, PathBuf)>, message: &str) -> bool {
        let mut any_committed = false;

        for (repo, worktree_path) in repos_with_changes {
            tracing::debug!(
                "Committing changes for repo '{}' at {:?}",
                repo.name,
                &worktree_path
            );

            match self.git().commit(&worktree_path, message) {
                Ok(true) => {
                    any_committed = true;
                    tracing::info!("Committed changes in repo '{}'", repo.name);
                }
                Ok(false) => {
                    tracing::warn!("No changes committed in repo '{}' (unexpected)", repo.name);
                }
                Err(e) => {
                    tracing::warn!("Failed to commit in repo '{}': {}", repo.name, e);
                }
            }
        }

        any_committed
    }

    /// Spawn a background task that polls the child process for completion and
    /// cleans up the execution entry when it exits.
    fn spawn_exit_monitor(
        &self,
        exec_id: &Uuid,
        exit_signal: Option<ExecutorExitSignal>,
    ) -> JoinHandle<()> {
        let exec_id = *exec_id;
        let child_store = self.child_store.clone();
        let msg_stores = self.msg_stores.clone();
        let db = self.db.clone();
        let container = self.clone();

        let mut process_exit_rx = self.spawn_os_exit_watcher(exec_id);

        tokio::spawn(async move {
            let mut exit_signal_future = exit_signal
                .map(|rx| rx.boxed()) // wait for result
                .unwrap_or_else(|| std::future::pending().boxed()); // no signal, stall forever

            let terminal_outcome: agent_runtime_supervisor::AgentRuntimeSupervisorOutcome;

            // Wait for process to exit, or exit signal from executor
            tokio::select! {
                // Exit signal with result.
                // Some coding agent processes do not automatically exit after processing the user request; instead the executor
                // signals when processing has finished to gracefully kill the process.
                exit_result = &mut exit_signal_future => {
                    // Executor signaled completion: kill group and use the provided result
                    if let Some(child_lock) = child_store.read().await.get(&exec_id).cloned() {
                        let mut child = child_lock.write().await ;
                        if let Err(err) = command::kill_process_group(&mut child).await {
                            tracing::error!("Failed to kill process group after exit signal: {} {}", exec_id, err);
                        }
                    }

                    terminal_outcome = match exit_result {
                        Ok(result) => agent_runtime_supervisor::classify_executor_exit_result(result),
                        Err(_) => agent_runtime_supervisor::classify_executor_exit_channel_closed(),
                    };
                }
                // Process exit
                exit_status_result = &mut process_exit_rx => {
                    terminal_outcome = match exit_status_result {
                        Ok(Ok(exit_status)) => agent_runtime_supervisor::classify_process_exit(exit_status),
                        Ok(Err(error)) => agent_runtime_supervisor::classify_watcher_error(error.to_string()),
                        Err(error) => agent_runtime_supervisor::classify_watcher_error(error.to_string()),
                    };
                }
            }

            let status = terminal_outcome
                .process_status
                .clone()
                .expect("terminal agent runtime outcome should include process status");
            let exit_code = terminal_outcome.exit_code;
            tracing::debug!(
                execution_process_id = %exec_id,
                lifecycle = ?terminal_outcome.lifecycle,
                process_status = ?status,
                exit_code = ?exit_code,
                error_kind = ?terminal_outcome.runtime_error.as_ref().map(|error| error.kind),
                "classified agent runtime terminal outcome"
            );

            if !ExecutionProcess::was_stopped(&db.pool, exec_id).await
                && let Err(e) =
                    ExecutionProcess::update_completion(&db.pool, exec_id, status, exit_code).await
            {
                tracing::error!("Failed to update execution process completion: {}", e);
            }

            // Notify any subscribers (e.g. workflow runtime) that this execution
            // process has reached a terminal state. This enables event-driven
            // workflow progression without relying on HTTP polling.
            //
            // We publish the event unconditionally here (even if was_stopped was
            // true) because the DB already holds a terminal status in that case
            // and any workflow watcher should still reconcile.  The session_id is
            // looked up from the DB to keep this path self-contained.
            if let Ok(Some(ep)) = ExecutionProcess::find_by_id(&db.pool, exec_id).await {
                publish_execution_completed(ExecutionCompletedEvent {
                    execution_process_id: exec_id,
                    session_id: ep.session_id,
                });
            }

            if let Ok(ctx) = ExecutionProcess::load_context(&db.pool, exec_id).await {
                let success = matches!(
                    ctx.execution_process.status,
                    ExecutionProcessStatus::Completed
                ) && exit_code == Some(0);

                let cleanup_done = matches!(
                    ctx.execution_process.run_reason,
                    ExecutionProcessRunReason::CleanupScript
                ) && !matches!(
                    ctx.execution_process.status,
                    ExecutionProcessStatus::Running
                );

                if success || cleanup_done {
                    // Commit changes (if any) and get feedback about whether changes were made
                    match container.try_commit_changes(&ctx).await {
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!("Failed to commit changes after execution: {}", e);
                            // Treat commit failures as if changes were made to be safe
                        }
                    };

                    // Script actions are the only ExecutionProcess actions. Any
                    // canonical AgentRun is finalized by AgentRunPort and never
                    // enters this legacy process monitor.
                    if let Err(e) = container.try_start_next_action(&ctx).await {
                        tracing::error!("Failed to start next action after completion: {}", e);
                    }
                }

                if container.should_finalize(&ctx) {
                    container.finalize_task(&ctx).await;
                }
            }

            // Now that commit/next-action/finalization steps for this process are complete,
            // capture the HEAD OID as the definitive "after" state (best-effort).
            container.update_after_head_commits(exec_id).await;

            // Wait for DB persistence to complete before cleaning up MsgStore
            let db_stream_handle = container.take_db_stream_handle(&exec_id).await;
            if let Some(msg_arc) = msg_stores.write().await.remove(&exec_id) {
                msg_arc.push_finished();
            }
            if let Some(handle) = db_stream_handle {
                let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
            }

            // SIGKILL any orphaned children (e.g. MCP servers) still in the
            // process group. The executor itself is already done — either it
            // exited naturally or was killed in the exit-signal branch above.
            if let Some(child_lock) = child_store.read().await.get(&exec_id).cloned() {
                let mut child = child_lock.write().await;
                let _ = child.start_kill();
            }
            container
                .remove_registered_agent_process(exec_id, "exit_monitor")
                .await;
            child_store.write().await.remove(&exec_id);
        })
    }

    fn spawn_os_exit_watcher(
        &self,
        exec_id: Uuid,
    ) -> tokio::sync::oneshot::Receiver<std::io::Result<std::process::ExitStatus>> {
        let (tx, rx) = tokio::sync::oneshot::channel::<std::io::Result<std::process::ExitStatus>>();
        let child_store = self.child_store.clone();
        tokio::spawn(async move {
            loop {
                let child_lock = {
                    let map = child_store.read().await;
                    map.get(&exec_id).cloned()
                };
                if let Some(child_lock) = child_lock {
                    let mut child_handler = child_lock.write().await;
                    match child_handler.try_wait() {
                        Ok(Some(status)) => {
                            let _ = tx.send(Ok(status));
                            break;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            let _ = tx.send(Err(e));
                            break;
                        }
                    }
                } else {
                    let _ = tx.send(Err(io::Error::other(format!(
                        "Child handle missing for {exec_id}"
                    ))));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        });
        rx
    }

    fn dir_name_from_workspace(workspace_id: &Uuid, task_title: &str) -> String {
        let task_title_id = git_branch_id(task_title);
        format!("{}-{}", short_uuid(workspace_id), task_title_id)
    }

    async fn track_child_msgs_in_store(
        &self,
        id: Uuid,
        child: &mut AsyncGroupChild,
    ) -> Result<(), ContainerError> {
        let store = self
            .get_msg_store_by_id(&id)
            .await
            .ok_or_else(|| ContainerError::Other(anyhow!("MsgStore not found for execution")))?;
        let out = child.inner().stdout.take().expect("no stdout");
        let err = child.inner().stderr.take().expect("no stderr");

        // Map stdout bytes -> LogMsg::Stdout
        let out = ReaderStream::new(out)
            .map_ok(|chunk| LogMsg::Stdout(String::from_utf8_lossy(&chunk).into_owned()));

        // Map stderr bytes -> LogMsg::Stderr
        let err = ReaderStream::new(err)
            .map_ok(|chunk| LogMsg::Stderr(String::from_utf8_lossy(&chunk).into_owned()));

        // If you have a JSON Patch source, map it to LogMsg::JsonPatch too, then select all three.

        // Merge and forward into the store
        let merged = select(out, err); // Stream<Item = Result<LogMsg, io::Error>>
        store.clone().spawn_forwarder(merged);
        Ok(())
    }

    /// Create a live diff log stream for ongoing attempts for WebSocket
    /// Returns a stream that owns the filesystem watcher - when dropped, watcher is cleaned up
    async fn create_live_diff_stream(
        &self,
        args: diff_stream::DiffStreamArgs,
    ) -> Result<DiffStreamHandle, ContainerError> {
        diff_stream::create(args)
            .await
            .map_err(|e| ContainerError::Other(anyhow!("{e}")))
    }

    /// Copy project files and workspace attachments to the workspace.
    /// Skips files that already exist (fast no-op if all exist).
    async fn copy_files_and_images(
        &self,
        workspace_dir: &Path,
        workspace: &Workspace,
    ) -> Result<(), ContainerError> {
        let repos = WorkspaceRepo::find_repos_with_copy_files(&self.db.pool, workspace.id).await?;

        for repo in &repos {
            if let Some(copy_files) = &repo.copy_files
                && !copy_files.trim().is_empty()
            {
                let worktree_path = workspace_dir.join(&repo.name);
                self.copy_project_files(&repo.path, &worktree_path, copy_files)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!(
                            "Failed to copy project files for repo '{}': {}",
                            repo.name,
                            e
                        );
                    });
            }
        }

        let agent_working_dir = Session::find_latest_by_workspace_id(&self.db.pool, workspace.id)
            .await?
            .and_then(|session| session.agent_working_dir);

        if let Err(e) = self
            .file_service
            .copy_files_by_workspace_to_worktree(
                workspace_dir,
                workspace.id,
                agent_working_dir.as_deref(),
            )
            .await
        {
            tracing::warn!("Failed to copy workspace files to workspace: {}", e);
        }

        Ok(())
    }

    /// Create workspace-level CLAUDE.md and AGENTS.md files that import from each repo.
    /// Uses the @import syntax to reference each repo's config files.
    /// Skips creating files if they already exist or if no repos have the source file.
    async fn create_workspace_config_files(
        workspace_dir: &Path,
        repos: &[Repo],
    ) -> Result<(), ContainerError> {
        const CONFIG_FILES: [&str; 2] = ["CLAUDE.md", "AGENTS.md"];

        for config_file in CONFIG_FILES {
            let workspace_config_path = workspace_dir.join(config_file);

            if workspace_config_path.exists() {
                tracing::trace!(
                    "Workspace config file {} already exists, skipping",
                    config_file
                );
                continue;
            }

            let mut import_lines = Vec::new();
            for repo in repos {
                let repo_config_path = workspace_dir.join(&repo.name).join(config_file);
                if repo_config_path.exists() {
                    import_lines.push(format!("@{}/{}", repo.name, config_file));
                }
            }

            if import_lines.is_empty() {
                tracing::trace!(
                    "No repos have {}, skipping workspace config creation",
                    config_file
                );
                continue;
            }

            let content = import_lines.join("\n") + "\n";
            if let Err(e) = tokio::fs::write(&workspace_config_path, &content).await {
                tracing::warn!(
                    "Failed to create workspace config file {}: {}",
                    config_file,
                    e
                );
                continue;
            }

            tracing::info!(
                "Created workspace {} with {} import(s)",
                config_file,
                import_lines.len()
            );
        }

        Ok(())
    }

    /// Reserve a canonical AgentRun; the consumer moves the queue only after
    /// acceptance and launches this exact identity afterwards.
    async fn start_queued_follow_up(
        &self,
        session_id: Uuid,
        queued_data: &DraftFollowUpData,
    ) -> Result<Uuid, ContainerError> {
        let session = Session::find_by_id(&self.db.pool, session_id)
            .await?
            .ok_or(SessionError::NotFound)?;
        let workspace = Workspace::find_by_id(&self.db.pool, session.workspace_id)
            .await?
            .ok_or(SessionError::WorkspaceNotFound)?;
        let executor_profile_id = queued_data.executor_config.profile_id();

        if let Some(expected) = session.executor.clone() {
            let actual = executor_profile_id.executor.to_string();
            if expected != actual {
                return Err(SessionError::ExecutorMismatch { expected, actual }.into());
            }
        }

        if session.executor.is_none() {
            Session::update_executor(
                &self.db.pool,
                session.id,
                &executor_profile_id.executor.to_string(),
            )
            .await?;
        }

        let provider = DirectProvider::from_base_agent(queued_data.executor_config.executor)
            .ok_or_else(|| {
                ContainerError::Other(anyhow!(
                    "{} is not a V1 direct Agent Runtime provider",
                    queued_data.executor_config.executor
                ))
            })?;
        let runtime_profile_id = executor_profile_id.cache_key();
        let provider_session = sqlx::query_as::<_, (String, Json<ProviderSessionReference>)>(
            r#"
            SELECT runtime_profile_id, session_reference
            FROM agent_provider_sessions
            WHERE session_id = ? AND provider_id = ?
            LIMIT 1
            "#,
        )
        .bind(session.id)
        .bind(provider.id())
        .fetch_optional(&self.db.pool)
        .await?
        .map(|(bound_profile_id, reference)| {
            if !runtime_profile_ids_match(&bound_profile_id, &runtime_profile_id) {
                return Err(ContainerError::Other(anyhow!(
                    "Session is bound to runtime profile {bound_profile_id}; create a new VK session for {runtime_profile_id}"
                )));
            }
            Ok(reference.0)
        })
        .transpose()?;
        let workspace_path = self.ensure_container_exists(&workspace).await?;
        let (request, attempt) = build_queued_agent_run_requests(
            session.id,
            &workspace,
            workspace_path,
            queued_data,
            provider_session,
        )
        .map_err(|error| ContainerError::Other(anyhow!(error)))?;

        self.agent_run_port
            .reserve(request, attempt)
            .await
            .map_err(|error| ContainerError::Other(anyhow!(error)))
    }
}

/// Only an accepted queued User turn with an untouched process reservation is
/// eligible. Setup gates, System handovers, handed-off or unknown hosts and
/// issued MCP credentials belong to their existing recovery owners.
async fn reserved_queued_follow_ups(pool: &SqlitePool) -> Result<Vec<(Uuid, Uuid)>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT r.id, r.session_id
        FROM agent_runs r
        JOIN agent_run_state s ON s.agent_run_id=r.id
        JOIN agent_run_attempts a ON a.agent_run_id=r.id
        JOIN agent_process_registry p ON p.run_attempt_id=a.id
        WHERE s.status='pending' AND a.status='pending'
          AND r.idempotency_key LIKE 'session:%:request:%:queued'
          AND json_extract(r.request_envelope, '$.input.role')='user'
          AND p.registry_status='reserved'
          AND p.host_endpoint IS NULL AND p.host_token IS NULL
          AND p.host_pid IS NULL AND p.pid IS NULL
          AND p.host_instance_id IS NULL AND p.process_started_at IS NULL
          AND NOT EXISTS(SELECT 1 FROM agent_run_launch_gates g WHERE g.agent_run_id=r.id)
          AND NOT EXISTS(SELECT 1 FROM workflow_main_session_bindings b WHERE b.actual_main_agent_run_id=r.id)
        ORDER BY r.created_at,r.id
        "#,
    ).fetch_all(pool).await
}

fn build_queued_agent_run_requests(
    session_id: Uuid,
    workspace: &Workspace,
    workspace_path: String,
    queued_data: &DraftFollowUpData,
    provider_session: Option<ProviderSessionReference>,
) -> Result<(AgentRunRequestEnvelope, RunAttemptRequest), AgentRunPortError> {
    let provider = DirectProvider::from_base_agent(queued_data.executor_config.executor)
        .ok_or_else(|| {
            AgentRunPortError::Rejected(format!(
                "{} is not a V1 direct Agent Runtime provider",
                queued_data.executor_config.executor
            ))
        })?;
    let runtime_profile_id = queued_data.executor_config.profile_id().cache_key();
    let capability_snapshot =
        direct_provider_capability_snapshot(provider, runtime_profile_id.clone());
    let intent = if provider_session.is_some() {
        require_capability(
            provider,
            &capability_snapshot,
            AgentCapability::SessionResume,
            false,
        )
        .map_err(|error| AgentRunPortError::Rejected(error.to_string()))?;
        AgentRunIntent::FollowUp
    } else {
        AgentRunIntent::Initial
    };
    let request_id = Uuid::new_v4();
    let agent_run_id = Uuid::new_v4();
    let turn_id = Uuid::new_v4();
    let correlation_id = Uuid::new_v4();
    let created_at = chrono::Utc::now();
    let idempotency_key = format!("session:{session_id}:request:{request_id}:queued");
    let workspace = WorkspaceReference {
        workspace_id: workspace.id,
        mode: match workspace.workspace_kind {
            WorkspaceKind::DirectFolder => WorkspaceMode::SharedWorkspace,
            WorkspaceKind::Worktree => WorkspaceMode::IsolatedWorktree,
        },
        path: workspace_path,
    };
    let request = AgentRunRequestEnvelope {
        schema_version: AGENT_REQUEST_SCHEMA_VERSION,
        payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
        request_id,
        idempotency_key: idempotency_key.clone(),
        session_id,
        agent_run_id,
        turn_id,
        correlation_id,
        intent,
        runtime_profile_id: runtime_profile_id.clone(),
        provider_id: provider.id().to_string(),
        workspace: workspace.clone(),
        input: CanonicalMessage {
            message_id: Uuid::new_v4(),
            role: AgentRuntimeMessageRole::User,
            content: queued_data.message.clone(),
        },
        created_at,
    };
    let attempt = RunAttemptRequest {
        schema_version: AGENT_REQUEST_SCHEMA_VERSION,
        payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
        request_id,
        idempotency_key: format!("{idempotency_key}:attempt:1"),
        session_id,
        agent_run_id,
        turn_id,
        run_attempt_id: Uuid::new_v4(),
        attempt_number: 1,
        correlation_id,
        mode: RunAttemptMode::Launch,
        transport: provider.transport(),
        runtime_profile_id,
        provider_id: provider.id().to_string(),
        workspace,
        capability_snapshot,
        executor_config: queued_data.executor_config.clone(),
        selected_skills: queued_data.selected_skills.clone(),
        reset_to_message_id: None,
        provider_session,
        created_at,
    };

    Ok((request, attempt))
}

#[async_trait]
impl ContainerService for LocalContainerService {
    fn msg_stores(&self) -> &Arc<RwLock<HashMap<Uuid, Arc<MsgStore>>>> {
        &self.msg_stores
    }

    fn db(&self) -> &DBService {
        &self.db
    }

    fn git(&self) -> &GitService {
        &self.git
    }

    fn notification_service(&self) -> &NotificationService {
        &self.notification_service
    }

    async fn touch(&self, workspace: &Workspace) -> Result<(), ContainerError> {
        let now = Instant::now();

        // We debounce touches to avoid excessive database writes, which in SQLites causes DB locks
        let should_debounce = |last_touch: &Instant| -> bool {
            now.duration_since(*last_touch) < WORKSPACE_TOUCH_DEBOUNCE
        };

        // Quick check with read lock
        if self
            .workspace_touch_times
            .read()
            .await
            .get(&workspace.id)
            .is_some_and(should_debounce)
        {
            return Ok(());
        }

        let mut map = self.workspace_touch_times.write().await;
        // Clean up stale entries older than the debounce window, reduce memory usage over time
        map.retain(|_, time| should_debounce(time));
        // check in case another thread has touched already
        if map.get(&workspace.id).is_some_and(should_debounce) {
            return Ok(());
        }
        map.insert(workspace.id, now);
        drop(map);

        Workspace::touch(&self.db.pool, workspace.id).await?;
        Ok(())
    }

    async fn store_db_stream_handle(&self, id: Uuid, handle: JoinHandle<()>) {
        self.add_db_stream_handle(id, handle).await;
    }

    async fn take_db_stream_handle(&self, id: &Uuid) -> Option<JoinHandle<()>> {
        LocalContainerService::take_db_stream_handle(self, id).await
    }

    async fn git_branch_prefix(&self) -> String {
        self.config.read().await.git_branch_prefix.clone()
    }

    fn workspace_to_current_dir(&self, workspace: &Workspace) -> PathBuf {
        PathBuf::from(workspace.container_ref.clone().unwrap_or_default())
    }

    async fn create(&self, workspace: &Workspace) -> Result<ContainerRef, ContainerError> {
        if workspace.is_direct_folder() {
            let container_ref = workspace
                .container_ref
                .as_deref()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ContainerError::Other(anyhow!(
                        "Direct folder workspace is missing a container path"
                    ))
                })?;
            let container_path = PathBuf::from(container_ref);
            let metadata = tokio::fs::metadata(&container_path).await?;
            if !metadata.is_dir() {
                return Err(ContainerError::Other(anyhow!(
                    "Direct folder workspace path is not a directory: {}",
                    container_path.display()
                )));
            }
            return Ok(container_ref.to_string());
        }

        let label = workspace.name.as_deref().unwrap_or("workspace");
        let workspace_dir_name =
            LocalContainerService::dir_name_from_workspace(&workspace.id, label);
        let workspace_dir = WorkspaceManager::get_workspace_base_dir().join(&workspace_dir_name);

        let (repositories, workspace_inputs) = self.workspace_repo_inputs(workspace.id).await?;

        let created_workspace = WorkspaceManager::create_workspace(
            &workspace_dir,
            &workspace_inputs,
            &workspace.branch,
        )
        .await
        .map_err(Self::map_workspace_manager_error)?;

        // Copy project files and images to workspace
        self.copy_files_and_images(&created_workspace.workspace_dir, workspace)
            .await?;

        Self::create_workspace_config_files(&created_workspace.workspace_dir, &repositories)
            .await?;

        Workspace::update_container_ref(
            &self.db.pool,
            workspace.id,
            &created_workspace.workspace_dir.to_string_lossy(),
        )
        .await?;

        Ok(created_workspace
            .workspace_dir
            .to_string_lossy()
            .to_string())
    }

    async fn delete(&self, workspace: &Workspace) -> Result<(), ContainerError> {
        self.try_stop(workspace, true).await;
        if workspace.can_delete_container_path() {
            self.cleanup_workspace(workspace).await;
        }
        Ok(())
    }

    async fn ensure_container_exists(
        &self,
        workspace: &Workspace,
    ) -> Result<ContainerRef, ContainerError> {
        self.touch(workspace).await?;
        if workspace.is_direct_folder() {
            let container_ref = workspace
                .container_ref
                .as_deref()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ContainerError::Other(anyhow!(
                        "Direct folder workspace is missing a container path"
                    ))
                })?;
            let container_path = PathBuf::from(container_ref);
            let metadata = tokio::fs::metadata(&container_path).await?;
            if !metadata.is_dir() {
                return Err(ContainerError::Other(anyhow!(
                    "Direct folder workspace path is not a directory: {}",
                    container_path.display()
                )));
            }
            if workspace.worktree_deleted {
                Workspace::clear_worktree_deleted(&self.db.pool, workspace.id).await?;
            }
            return Ok(container_ref.to_string());
        }

        let (repositories, workspace_inputs) = self.workspace_repo_inputs(workspace.id).await?;

        let workspace_dir = if let Some(container_ref) = &workspace.container_ref {
            PathBuf::from(container_ref)
        } else {
            let label = workspace.name.as_deref().unwrap_or("workspace");
            let workspace_dir_name =
                LocalContainerService::dir_name_from_workspace(&workspace.id, label);
            WorkspaceManager::get_workspace_base_dir().join(&workspace_dir_name)
        };

        WorkspaceManager::ensure_workspace_exists(
            &workspace_dir,
            &workspace_inputs,
            &workspace.branch,
        )
        .await
        .map_err(Self::map_workspace_manager_error)?;

        if workspace.container_ref.is_none() {
            Workspace::update_container_ref(
                &self.db.pool,
                workspace.id,
                &workspace_dir.to_string_lossy(),
            )
            .await?;
        }

        if workspace.worktree_deleted {
            Workspace::clear_worktree_deleted(&self.db.pool, workspace.id).await?;
        }

        // Copy project files and images (fast no-op if already exist)
        self.copy_files_and_images(&workspace_dir, workspace)
            .await?;

        Self::create_workspace_config_files(&workspace_dir, &repositories).await?;

        Ok(workspace_dir.to_string_lossy().to_string())
    }

    async fn is_container_clean(&self, workspace: &Workspace) -> Result<bool, ContainerError> {
        let Some(container_ref) = &workspace.container_ref else {
            return Ok(true);
        };

        let workspace_dir = PathBuf::from(container_ref);
        if !workspace_dir.exists() {
            return Ok(true);
        }

        let repositories =
            WorkspaceRepo::find_repos_for_workspace(&self.db.pool, workspace.id).await?;

        for repo in &repositories {
            let worktree_path = workspace_dir.join(&repo.name);
            if worktree_path.exists() {
                let (uncommitted, untracked) =
                    self.git().get_worktree_change_counts(&worktree_path)?;
                if uncommitted > 0 || untracked > 0 {
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }

    async fn start_execution_inner(
        &self,
        workspace: &Workspace,
        execution_process: &ExecutionProcess,
        executor_action: &ExecutorAction,
    ) -> Result<(), ContainerError> {
        if !matches!(
            executor_action.typ(),
            executors::actions::ExecutorActionType::ScriptRequest(_)
        ) {
            return Err(ContainerError::Other(anyhow!(
                "coding-agent ExecutionProcess actions were removed; use the canonical AgentRun API"
            )));
        }

        // Get the worktree path
        let container_ref = workspace
            .container_ref
            .as_ref()
            .ok_or(ContainerError::Other(anyhow!(
                "Container ref not found for workspace"
            )))?;
        let current_dir = PathBuf::from(container_ref);
        let starting = agent_runtime_supervisor::classify_starting();
        tracing::debug!(
            execution_process_id = %execution_process.id,
            lifecycle = ?starting.lifecycle,
            "classified agent runtime start outcome"
        );

        let repos = WorkspaceRepo::find_repos_for_workspace(&self.db.pool, workspace.id).await?;
        let repo_names: Vec<String> = repos.iter().map(|r| r.name.clone()).collect();
        let repo_context = RepoContext::new(current_dir.clone(), repo_names);

        let design_arena_no_commit =
            should_disable_default_commit_for_workspace(&self.db.pool, workspace.id).await?;

        let config = self.config.read().await;
        let commit_reminder_enabled = config.commit_reminder_enabled && !design_arena_no_commit;
        let commit_reminder_prompt = config
            .commit_reminder_prompt
            .clone()
            .unwrap_or_else(|| DEFAULT_COMMIT_REMINDER_PROMPT.to_string());
        drop(config);
        let mut env = ExecutionEnv::new(
            repo_context,
            commit_reminder_enabled,
            commit_reminder_prompt,
        );

        // Always inject workspace/session context
        env.insert("VK_WORKSPACE_ID", workspace.id.to_string());
        env.insert("VK_WORKSPACE_BRANCH", &workspace.branch);

        // Create the child and stream, add to execution tracker with timeout
        let spawn_result = tokio::time::timeout(
            Duration::from_secs(30),
            executor_action.spawn(
                &current_dir,
                Arc::new(executors::approvals::NoopExecutorApprovalService {}),
                &env,
            ),
        )
        .await;

        let mut spawned = match spawn_result {
            Ok(Ok(spawned)) => spawned,
            Ok(Err(error)) => {
                let outcome =
                    agent_runtime_supervisor::classify_launch_executor_error(&error, None);
                trace_launch_diagnostic(execution_process.id, &outcome);
                return Err(ContainerError::ExecutorError(error));
            }
            Err(_) => {
                let outcome = agent_runtime_supervisor::classify_launch_timeout();
                trace_launch_diagnostic(execution_process.id, &outcome);
                return Err(ContainerError::Other(anyhow!(
                    "Timeout: process took more than 30 seconds to start"
                )));
            }
        };

        let pid = match spawned.child.inner().id() {
            Some(pid) => pid,
            None => {
                let _ = command::kill_process_group(&mut spawned.child).await;
                return Err(ContainerError::Other(anyhow!(
                    "Spawned execution process has no OS pid"
                )));
            }
        };
        let registered_process = RegisteredAgentProcess::new(
            execution_process.id,
            Some(execution_process.session_id),
            Some(workspace.id),
            None,
            pid,
            Some(pid),
            None,
        );
        if let Err(error) = self
            .agent_process_registry
            .register(registered_process)
            .await
        {
            tracing::warn!(
                execution_process_id = %execution_process.id,
                session_id = %execution_process.session_id,
                workspace_id = %workspace.id,
                pid,
                error = %error,
                "failed to register spawned agent process; killing child"
            );
            let _ = command::kill_process_group(&mut spawned.child).await;
            return Err(ContainerError::Io(error));
        }

        if let Err(e) = self
            .track_child_msgs_in_store(execution_process.id, &mut spawned.child)
            .await
        {
            let _ = command::kill_process_group(&mut spawned.child).await;
            self.remove_registered_agent_process(execution_process.id, "spawn_tracking_failed")
                .await;
            return Err(e);
        }

        self.add_child_to_store(execution_process.id, spawned.child)
            .await;
        let running = agent_runtime_supervisor::classify_running();
        tracing::debug!(
            execution_process_id = %execution_process.id,
            lifecycle = ?running.lifecycle,
            "classified agent runtime running outcome"
        );

        // Store cancellation token for graceful shutdown
        if let Some(cancel) = spawned.cancel {
            self.add_cancellation_token(execution_process.id, cancel)
                .await;
        }

        // Spawn unified exit monitor: watches OS exit and optional executor signal
        let hn = self.spawn_exit_monitor(&execution_process.id, spawned.exit_signal);
        self.add_exit_monitor_handle(execution_process.id, hn).await;

        Ok(())
    }

    async fn stop_execution(
        &self,
        execution_process: &ExecutionProcess,
        status: ExecutionProcessStatus,
    ) -> Result<(), ContainerError> {
        let child = self.get_child_from_store(&execution_process.id).await;
        let exit_code = if status == ExecutionProcessStatus::Completed {
            Some(0)
        } else {
            None
        };
        let cancelling = agent_runtime_supervisor::classify_cancellation_requested();
        tracing::debug!(
            execution_process_id = %execution_process.id,
            lifecycle = ?cancelling.lifecycle,
            requested_status = ?status,
            "classified agent runtime cancellation request"
        );

        ExecutionProcess::update_completion(
            &self.db.pool,
            execution_process.id,
            status.clone(),
            exit_code,
        )
        .await?;

        // Try graceful cancellation first, then force kill
        if let Some(cancel) = self.take_cancellation_token(&execution_process.id).await {
            cancel.cancel();

            // Wait for exit monitor to finish gracefully
            if let Some(monitor_handle) = self.take_exit_monitor_handle(&execution_process.id).await
            {
                match tokio::time::timeout(Duration::from_secs(5), monitor_handle).await {
                    Ok(_) => {
                        tracing::debug!("Process {} exited gracefully", execution_process.id);
                    }
                    Err(_) => {
                        tracing::debug!(
                            "Graceful shutdown timed out for process {}, force killing",
                            execution_process.id
                        );
                    }
                }
            }
        }

        if let Some(child) = child {
            {
                let mut child_guard = child.write().await;
                if let Err(e) = command::kill_process_group(&mut child_guard).await {
                    tracing::error!(
                        "Failed to stop execution process {}: {}",
                        execution_process.id,
                        e
                    );
                    return Err(e);
                }
            }
            self.remove_child_from_store(&execution_process.id).await;
            self.remove_registered_agent_process(execution_process.id, "explicit_stop")
                .await;
        } else {
            tracing::warn!(
                execution_process_id = %execution_process.id,
                session_id = %execution_process.session_id,
                requested_status = ?status,
                "execution process has no in-memory child handle; cleaning stale registry entry"
            );
            self.cleanup_registered_agent_execution(execution_process.id, "missing_child_stop")
                .await;
        }

        // Mark the process finished in the MsgStore and wait for DB persistence
        let db_stream_handle = self.take_db_stream_handle(&execution_process.id).await;
        if let Some(msg) = self.msg_stores.write().await.remove(&execution_process.id) {
            msg.push_finished();
        }
        if let Some(handle) = db_stream_handle {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }

        tracing::debug!(
            "Execution process {} stopped successfully",
            execution_process.id
        );

        // Record after-head commit OID (best-effort)
        self.update_after_head_commits(execution_process.id).await;

        Ok(())
    }

    async fn stream_diff(
        &self,
        workspace: &Workspace,
        stats_only: bool,
    ) -> Result<futures::stream::BoxStream<'static, Result<LogMsg, std::io::Error>>, ContainerError>
    {
        let workspace_repos =
            WorkspaceRepo::find_by_workspace_id(&self.db.pool, workspace.id).await?;
        let target_branches: HashMap<_, _> = workspace_repos
            .iter()
            .map(|wr| (wr.repo_id, wr.target_branch.clone()))
            .collect();

        let repositories =
            WorkspaceRepo::find_repos_for_workspace(&self.db.pool, workspace.id).await?;

        let mut streams = Vec::new();

        let container_ref = self.ensure_container_exists(workspace).await?;
        let workspace_root = PathBuf::from(container_ref);

        for repo in repositories {
            let worktree_path = workspace_root.join(&repo.name);
            let branch = &workspace.branch;

            let Some(target_branch) = target_branches.get(&repo.id) else {
                tracing::warn!(
                    "Skipping diff stream for repo {}: no target branch configured",
                    repo.name
                );
                continue;
            };

            let base_commit = match self
                .git()
                .get_base_commit(&repo.path, branch, target_branch)
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        "Skipping diff stream for repo {}: failed to get base commit: {}",
                        repo.name,
                        e
                    );
                    continue;
                }
            };

            let stream = self
                .create_live_diff_stream(diff_stream::DiffStreamArgs {
                    git_service: self.git().clone(),
                    db: self.db().clone(),
                    workspace_id: workspace.id,
                    repo_id: repo.id,
                    repo_path: repo.path.clone(),
                    worktree_path: worktree_path.clone(),
                    branch: branch.to_string(),
                    target_branch: target_branch.clone(),
                    base_commit: base_commit.clone(),
                    stats_only,
                    path_prefix: Some(repo.name.clone()),
                })
                .await?;

            streams.push(Box::pin(stream));
        }

        if streams.is_empty() {
            return Ok(Box::pin(futures::stream::empty()));
        }

        // Merge all streams into one
        Ok(Box::pin(futures::stream::select_all(streams)))
    }

    async fn try_commit_changes(&self, ctx: &ExecutionContext) -> Result<bool, ContainerError> {
        if !matches!(
            ctx.execution_process.run_reason,
            ExecutionProcessRunReason::CleanupScript,
        ) {
            return Ok(false);
        }

        if should_disable_default_commit_for_workspace(&self.db.pool, ctx.workspace.id).await? {
            tracing::info!(
                workspace_id = %ctx.workspace.id,
                "Skipping automatic commit for open Design Arena workspace"
            );
            return Ok(false);
        }

        let message = self.get_commit_message(ctx).await;

        let container_ref = ctx
            .workspace
            .container_ref
            .as_ref()
            .ok_or_else(|| ContainerError::Other(anyhow!("Container reference not found")))?;
        let workspace_root = PathBuf::from(container_ref);

        let repos_with_changes = self.check_repos_for_changes(&workspace_root, &ctx.repos)?;
        if repos_with_changes.is_empty() {
            tracing::debug!("No changes to commit in any repository");
            return Ok(false);
        }

        Ok(self.commit_repos(repos_with_changes, &message))
    }

    /// Copy files from the original project directory to the worktree.
    /// Skips files that already exist at target with same size.
    async fn copy_project_files(
        &self,
        source_dir: &Path,
        target_dir: &Path,
        copy_files: &str,
    ) -> Result<(), ContainerError> {
        let source_dir = source_dir.to_path_buf();
        let target_dir = target_dir.to_path_buf();
        let copy_files = copy_files.to_string();

        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            tokio::task::spawn_blocking(move || {
                copy::copy_project_files_impl(&source_dir, &target_dir, &copy_files)
            }),
        )
        .await
        .map_err(|_| ContainerError::Other(anyhow!("Copy project files timed out after 30s")))?
        .map_err(|e| ContainerError::Other(anyhow!("Copy files task failed: {e}")))?
    }

    async fn kill_all_running_processes(&self) -> Result<(), ContainerError> {
        tracing::info!("Killing all running processes");
        let running_processes = ExecutionProcess::find_running(&self.db.pool).await?;

        tracing::info!(
            "Found {} running processes to kill",
            running_processes.len()
        );

        for process in running_processes {
            tracing::info!(
                "Killing process: id={}, run_reason={:?}",
                process.id,
                process.run_reason
            );
            if let Err(error) = self
                .stop_execution(&process, ExecutionProcessStatus::Killed)
                .await
            {
                tracing::error!(
                    "Failed to cleanly kill running execution process {:?}: {:?}",
                    process,
                    error
                );
            } else {
                tracing::info!("Successfully killed process: id={}", process.id);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use db::models::{
        arena_group::{ArenaLifecycleStatus, ArenaMode},
        workspace::ContainerOwnership,
    };
    use executors::{
        executors::BaseCodingAgent, profile::ExecutorConfig,
        runtime::PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION,
    };
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    async fn setup_container_policy_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");

        for statement in [
            r#"
            CREATE TABLE tasks (
                id BLOB PRIMARY KEY
            )
            "#,
            r#"
            CREATE TABLE arena_groups (
                id BLOB PRIMARY KEY,
                task_id BLOB NOT NULL UNIQUE,
                prompt TEXT NOT NULL,
                base_branch TEXT NOT NULL,
                mode TEXT NOT NULL,
                lifecycle_status TEXT NOT NULL,
                winner_candidate_id BLOB,
                promoted_at TEXT,
                closed_at TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
            )
            "#,
            r#"
            CREATE TABLE workspaces (
                id BLOB PRIMARY KEY
            )
            "#,
            r#"
            CREATE TABLE arena_candidates (
                id BLOB PRIMARY KEY,
                arena_group_id BLOB NOT NULL,
                workspace_id BLOB NOT NULL UNIQUE,
                purpose TEXT NOT NULL,
                sort_order INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
            )
            "#,
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("create schema");
        }

        pool
    }

    async fn insert_workspace_in_arena(
        pool: &SqlitePool,
        mode: ArenaMode,
        lifecycle_status: ArenaLifecycleStatus,
    ) -> Uuid {
        let group_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();

        sqlx::query("INSERT INTO tasks (id) VALUES (?)")
            .bind(task_id)
            .execute(pool)
            .await
            .expect("insert Arena Task");

        sqlx::query(
            r#"INSERT INTO arena_groups
                  (id, task_id, prompt, base_branch, mode, lifecycle_status)
               VALUES (?, ?, ?, ?, ?, ?)"#,
        )
        .bind(group_id)
        .bind(task_id)
        .bind("Compare designs")
        .bind("main")
        .bind(mode)
        .bind(lifecycle_status)
        .execute(pool)
        .await
        .expect("insert arena group");

        sqlx::query("INSERT INTO workspaces (id) VALUES (?)")
            .bind(workspace_id)
            .execute(pool)
            .await
            .expect("insert workspace");

        sqlx::query(
            r#"
            INSERT INTO arena_candidates
                (id, arena_group_id, workspace_id, purpose, sort_order)
            VALUES (?, ?, ?, 'attempt', 0)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(group_id)
        .bind(workspace_id)
        .execute(pool)
        .await
        .expect("insert Arena candidate");

        workspace_id
    }

    fn runtime_workspace(workspace_kind: WorkspaceKind) -> Workspace {
        let now = Utc::now();
        Workspace {
            id: Uuid::new_v4(),
            container_ref: Some("C:\\runtime-workspace".to_string()),
            workspace_kind,
            container_ownership: ContainerOwnership::Managed,
            branch: "main".to_string(),
            setup_completed_at: None,
            created_at: now,
            updated_at: now,
            archived: false,
            pinned: false,
            name: None,
            worktree_deleted: false,
        }
    }

    fn queued_follow_up_data() -> DraftFollowUpData {
        let mut executor_config = ExecutorConfig::new(BaseCodingAgent::Codex);
        executor_config.variant = Some("queued-profile".to_string());
        executor_config.model_id = Some("gpt-5.4".to_string());
        executor_config.agent_id = Some("default".to_string());
        executor_config.reasoning_id = Some("high".to_string());
        DraftFollowUpData {
            message: "Continue the canonical task".to_string(),
            executor_config,
            selected_skills: None,
        }
    }

    #[test]
    fn queued_request_without_provider_session_is_canonical_initial() {
        let session_id = Uuid::new_v4();
        let workspace = runtime_workspace(WorkspaceKind::Worktree);
        let queued_data = queued_follow_up_data();

        let (request, attempt) = build_queued_agent_run_requests(
            session_id,
            &workspace,
            "C:\\runtime-workspace".to_string(),
            &queued_data,
            None,
        )
        .expect("build queued AgentRun request");

        request.validate_current().expect("valid request envelope");
        attempt
            .validate_for_run(&request)
            .expect("valid attempt envelope");
        assert_eq!(request.intent, AgentRunIntent::Initial);
        assert_eq!(request.input.content, queued_data.message);
        assert_eq!(attempt.executor_config, queued_data.executor_config);
        assert_eq!(attempt.selected_skills, None);
        assert_eq!(attempt.provider_session, None);
        assert_eq!(request.workspace.mode, WorkspaceMode::IsolatedWorktree);
        assert_eq!(request.workspace, attempt.workspace);
    }

    #[test]
    fn queued_request_with_provider_session_is_native_follow_up() {
        let session_id = Uuid::new_v4();
        let workspace = runtime_workspace(WorkspaceKind::DirectFolder);
        let queued_data = queued_follow_up_data();
        let runtime_profile_id = queued_data.executor_config.profile_id().cache_key();
        let provider_session = ProviderSessionReference {
            schema_version: PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION,
            provider_id: DirectProvider::Codex.id().to_string(),
            runtime_profile_id,
            provider_session_id: "native-session-1".to_string(),
            observed_at: Utc::now(),
            metadata: Some(serde_json::json!({ "source": "test" })),
        };

        let (request, attempt) = build_queued_agent_run_requests(
            session_id,
            &workspace,
            "C:\\shared-workspace".to_string(),
            &queued_data,
            Some(provider_session.clone()),
        )
        .expect("build queued follow-up request");

        request.validate_current().expect("valid request envelope");
        attempt
            .validate_for_run(&request)
            .expect("valid attempt envelope");
        assert_eq!(request.intent, AgentRunIntent::FollowUp);
        assert_eq!(attempt.provider_session, Some(provider_session));
        assert_eq!(attempt.executor_config, queued_data.executor_config);
        assert_eq!(attempt.selected_skills, None);
        assert_eq!(request.workspace.mode, WorkspaceMode::SharedWorkspace);
    }

    #[test]
    fn queued_request_maps_both_workspace_collaboration_modes() {
        let queued_data = queued_follow_up_data();
        let cases = [
            (WorkspaceKind::DirectFolder, WorkspaceMode::SharedWorkspace),
            (WorkspaceKind::Worktree, WorkspaceMode::IsolatedWorktree),
        ];

        for (workspace_kind, expected_mode) in cases {
            let workspace = runtime_workspace(workspace_kind);
            let (request, attempt) = build_queued_agent_run_requests(
                Uuid::new_v4(),
                &workspace,
                "C:\\workspace".to_string(),
                &queued_data,
                None,
            )
            .expect("build workspace-mode request");

            assert_eq!(request.workspace.mode, expected_mode);
            assert_eq!(attempt.workspace.mode, expected_mode);
        }
    }

    #[tokio::test]
    async fn queued_reservation_recovery_keeps_original_identity_and_excludes_other_launch_owners()
    {
        use db::models::agent_runtime::AgentRunRecord;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        let workspace = runtime_workspace(WorkspaceKind::DirectFolder);
        let session_id = Uuid::new_v4();
        sqlx::query("INSERT INTO workspaces(id,container_ref,workspace_kind,container_ownership,branch) VALUES (?,'fixture','direct_folder','external','direct-folder')")
            .bind(workspace.id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO sessions(id,workspace_id,executor) VALUES (?,?,'CODEX')")
            .bind(session_id)
            .bind(workspace.id)
            .execute(&pool)
            .await
            .unwrap();
        let (request, attempt) = build_queued_agent_run_requests(
            session_id,
            &workspace,
            "fixture".into(),
            &queued_follow_up_data(),
            None,
        )
        .unwrap();
        AgentRunRecord::persist_identity_before_launch(&pool, &request, &attempt)
            .await
            .unwrap();
        let expected = vec![(request.agent_run_id, session_id)];
        assert_eq!(reserved_queued_follow_ups(&pool).await.unwrap(), expected);
        assert_eq!(
            reserved_queued_follow_ups(&pool).await.unwrap(),
            expected,
            "scans cannot allocate another identity"
        );
        sqlx::query(
            "INSERT INTO agent_run_launch_gates(agent_run_id,setup_action) VALUES (?,'{}')",
        )
        .bind(request.agent_run_id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            reserved_queued_follow_ups(&pool).await.unwrap().is_empty(),
            "setup gates have a separate recovery owner"
        );
        sqlx::query("DELETE FROM agent_run_launch_gates WHERE agent_run_id=?")
            .bind(request.agent_run_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE agent_process_registry SET host_endpoint='endpoint',host_token='token' WHERE run_attempt_id=?")
            .bind(attempt.run_attempt_id).execute(&pool).await.unwrap();
        assert!(
            reserved_queued_follow_ups(&pool).await.unwrap().is_empty(),
            "an accepted host handoff must attach, never respawn"
        );
        sqlx::query("UPDATE agent_process_registry SET host_endpoint=NULL,host_token=NULL WHERE run_attempt_id=?")
            .bind(attempt.run_attempt_id).execute(&pool).await.unwrap();
        sqlx::query("UPDATE agent_runs SET request_envelope=json_set(request_envelope,'$.input.role','system') WHERE id=?")
            .bind(request.agent_run_id).execute(&pool).await.unwrap();
        assert!(
            reserved_queued_follow_ups(&pool).await.unwrap().is_empty(),
            "System handover has its own durable event"
        );
        sqlx::query("UPDATE agent_runs SET request_envelope=json_set(request_envelope,'$.input.role','user'),idempotency_key='ordinary-user-run' WHERE id=?")
            .bind(request.agent_run_id).execute(&pool).await.unwrap();
        assert!(
            reserved_queued_follow_ups(&pool).await.unwrap().is_empty(),
            "a pending state alone does not authorize recovery"
        );
        sqlx::query("UPDATE agent_runs SET idempotency_key=? WHERE id=?")
            .bind(&request.idempotency_key)
            .bind(request.agent_run_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE agent_run_state SET status='cancelled' WHERE agent_run_id=?")
            .bind(request.agent_run_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            reserved_queued_follow_ups(&pool).await.unwrap().is_empty(),
            "stopped reservations cannot be revived"
        );
        let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_runs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(runs, 1);
    }

    #[tokio::test]
    async fn open_design_arena_workspace_disables_default_commit() {
        let pool = setup_container_policy_pool().await;
        let workspace_id =
            insert_workspace_in_arena(&pool, ArenaMode::Design, ArenaLifecycleStatus::Open).await;

        let disabled = should_disable_default_commit_for_workspace(&pool, workspace_id)
            .await
            .expect("policy lookup");

        assert!(disabled);
    }

    #[tokio::test]
    async fn implementation_started_workspace_allows_normal_commit_policy() {
        let pool = setup_container_policy_pool().await;
        let workspace_id = insert_workspace_in_arena(
            &pool,
            ArenaMode::Design,
            ArenaLifecycleStatus::ImplementationStarted,
        )
        .await;

        let disabled = should_disable_default_commit_for_workspace(&pool, workspace_id)
            .await
            .expect("policy lookup");

        assert!(!disabled);
    }
}
