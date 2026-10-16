use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Error as AnyhowError, anyhow};
use async_trait::async_trait;
use db::{
    DBService,
    models::{
        execution_process::{
            CreateExecutionProcess, ExecutionContext, ExecutionProcess, ExecutionProcessError,
            ExecutionProcessRunReason, ExecutionProcessStatus,
        },
        execution_process_repo_state::{
            CreateExecutionProcessRepoState, ExecutionProcessRepoState,
        },
        repo::Repo,
        session::{CreateSession, Session, SessionError},
        workspace::{Workspace, WorkspaceError},
        workspace_repo::WorkspaceRepo,
    },
};
#[cfg(not(feature = "qa-mode"))]
use executors::profile::ExecutorConfigs;
use executors::{
    actions::{
        ExecutorAction, ExecutorActionType,
        script::{ScriptContext, ScriptRequest, ScriptRequestLanguage},
    },
    executors::{ExecutorError, StandardCodingAgentExecutor},
    profile::ExecutorProfileId,
};
use futures::{StreamExt, future, stream::BoxStream};
use git::{GitService, GitServiceError};
use json_patch::Patch;
use sqlx::Error as SqlxError;
use thiserror::Error;
use tokio::{sync::RwLock, task::JoinHandle};
use utils::{
    log_msg::LogMsg,
    msg_store::MsgStore,
    text::{git_branch_id, short_uuid},
};
use uuid::Uuid;
use worktree_manager::WorktreeError;

use crate::services::{execution_process, notification::NotificationService};
pub type ContainerRef = String;

fn has_container_ref(workspace: &Workspace) -> bool {
    matches!(
        workspace.container_ref.as_deref(),
        Some(container_ref) if !container_ref.is_empty()
    )
}

#[derive(Debug, Error)]
pub enum ContainerError {
    #[error(transparent)]
    GitServiceError(#[from] GitServiceError),
    #[error(transparent)]
    Sqlx(#[from] SqlxError),
    #[error(transparent)]
    ExecutorError(#[from] ExecutorError),
    #[error(transparent)]
    Worktree(#[from] WorktreeError),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    ExecutionProcess(#[from] ExecutionProcessError),
    #[error("Io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to kill process: {0}")]
    KillFailed(std::io::Error),
    #[error(transparent)]
    Other(#[from] AnyhowError), // Catches any unclassified errors
}

#[async_trait]
pub trait ContainerService {
    fn msg_stores(&self) -> &Arc<RwLock<HashMap<Uuid, Arc<MsgStore>>>>;

    fn db(&self) -> &DBService;

    fn git(&self) -> &GitService;

    fn notification_service(&self) -> &NotificationService;

    async fn touch(&self, workspace: &Workspace) -> Result<(), ContainerError>;

    fn workspace_to_current_dir(&self, workspace: &Workspace) -> PathBuf;

    async fn discover_executor_options(
        &self,
        executor_profile_id: ExecutorProfileId,
        session_id: Option<Uuid>,
        workspace_id: Option<Uuid>,
        repo_id: Option<Uuid>,
        refresh: bool,
    ) -> Result<Option<BoxStream<'static, Patch>>, ContainerError> {
        let (workdir, repo_path) = if let Some(session_id) = session_id {
            let session = Session::find_by_id(&self.db().pool, session_id)
                .await?
                .ok_or(SqlxError::RowNotFound)?;

            if let Some(workspace_id) = workspace_id
                && session.workspace_id != workspace_id
            {
                return Err(ContainerError::Other(anyhow!(
                    "Session does not belong to workspace"
                )));
            }

            let workspace = Workspace::find_by_id(&self.db().pool, session.workspace_id)
                .await?
                .ok_or(SqlxError::RowNotFound)?;

            let container_ref = match workspace.container_ref.as_deref() {
                Some(container_ref) if !container_ref.is_empty() => container_ref,
                _ => &self.ensure_container_exists(&workspace).await?,
            };

            if container_ref.is_empty() {
                return Err(ContainerError::Other(anyhow!("Workspace path is empty")));
            }

            let workspace_path = PathBuf::from(container_ref);
            let workdir = match session.agent_working_dir.as_deref() {
                Some(dir) if !dir.is_empty() => Some(workspace_path.join(dir)),
                _ => Some(workspace_path),
            };

            let repos =
                WorkspaceRepo::find_repos_for_workspace(&self.db().pool, session.workspace_id)
                    .await
                    .unwrap_or_default();
            let repo_path = if repos.len() == 1 {
                Some(repos[0].path.clone())
            } else {
                None
            };

            (workdir, repo_path)
        } else if let Some(workspace_id) = workspace_id {
            let workspace = Workspace::find_by_id(&self.db().pool, workspace_id)
                .await?
                .ok_or(SqlxError::RowNotFound)?;

            let ensured_container_ref;
            let container_ref = match workspace.container_ref.as_deref() {
                Some(container_ref) if !container_ref.is_empty() => container_ref,
                _ => {
                    ensured_container_ref = self.ensure_container_exists(&workspace).await?;
                    ensured_container_ref.as_str()
                }
            };

            if container_ref.is_empty() {
                return Err(ContainerError::Other(anyhow!("Workspace path is empty")));
            }

            let repos = WorkspaceRepo::find_repos_for_workspace(&self.db().pool, workspace.id)
                .await
                .unwrap_or_default();
            let repo_path = if repos.len() == 1 {
                Some(repos[0].path.clone())
            } else {
                None
            };

            (Some(PathBuf::from(container_ref)), repo_path)
        } else if let Some(repo_id) = repo_id {
            let repo = Repo::find_by_id(&self.db().pool, repo_id)
                .await
                .ok()
                .flatten()
                .map(|repo| repo.path);
            (None, repo)
        } else {
            (None, None)
        };

        #[cfg(feature = "qa-mode")]
        {
            let _ = executor_profile_id;
            let _ = workdir;
            let _ = repo_path;
            let _ = refresh;
            return Ok(None);
        }
        #[cfg(not(feature = "qa-mode"))]
        {
            let executor = ExecutorConfigs::get_cached()
                .get_coding_agent(&executor_profile_id)
                .ok_or_else(|| {
                    ContainerError::Other(anyhow!("The requested agent profile is unavailable"))
                })?;

            if refresh && let executors::executors::CodingAgent::ClaudeCode(agent) = &executor {
                agent
                    .invalidate_discovery_cache(workdir.as_deref(), repo_path.as_deref())
                    .await?;
            }
            // Codex discovers directly. ACP/OMP re-read native files and the
            // latest scoped runtime observations; never erase their only
            // catalog or create an extra native session merely to refresh.
            // Do not warm DEFAULT in the background for another profile.

            let stream = executor
                .discover_options(workdir.as_deref(), repo_path.as_deref())
                .await?;
            Ok(Some(stream))
        }
    }

    async fn store_db_stream_handle(&self, id: Uuid, handle: JoinHandle<()>);

    async fn take_db_stream_handle(&self, id: &Uuid) -> Option<JoinHandle<()>>;

    async fn create(&self, workspace: &Workspace) -> Result<ContainerRef, ContainerError>;

    async fn kill_all_running_processes(&self) -> Result<(), ContainerError>;

    async fn delete(&self, workspace: &Workspace) -> Result<(), ContainerError>;

    /// A context is finalized when
    /// - Always when the execution process has failed or been killed
    /// - Never when the run reason is DevServer
    /// - Never when a setup script has no next_action (parallel mode)
    /// - The next action is None (no follow-up actions)
    fn should_finalize(&self, ctx: &ExecutionContext) -> bool {
        // Never finalize DevServer processes
        if matches!(
            ctx.execution_process.run_reason,
            ExecutionProcessRunReason::DevServer
        ) {
            return false;
        }

        // Never finalize setup scripts without a next_action (parallel mode).
        // In sequential mode, setup scripts have next_action pointing to coding agent,
        // so they won't finalize anyway (handled by next_action.is_none() check below).
        let action = ctx.execution_process.executor_action().unwrap();
        if matches!(
            ctx.execution_process.run_reason,
            ExecutionProcessRunReason::SetupScript
        ) && action.next_action.is_none()
        {
            return false;
        }

        // Always finalize failed or killed executions, regardless of next action
        if matches!(
            ctx.execution_process.status,
            ExecutionProcessStatus::Failed | ExecutionProcessStatus::Killed
        ) {
            return true;
        }

        // Otherwise, finalize only if no next action
        action.next_action.is_none()
    }

    /// Finalize workspace execution by sending notifications
    async fn finalize_task(&self, ctx: &ExecutionContext) {
        // Skip notification if process was intentionally killed by user
        if matches!(ctx.execution_process.status, ExecutionProcessStatus::Killed) {
            return;
        }

        let workspace_name = ctx
            .workspace
            .name
            .as_deref()
            .unwrap_or(&ctx.workspace.branch);
        let title = format!("Workspace Complete: {}", workspace_name);
        let message = match ctx.execution_process.status {
            ExecutionProcessStatus::Completed => format!(
                "✅ '{}' completed successfully\nBranch: {:?}\nExecutor: {:?}",
                workspace_name, ctx.workspace.branch, ctx.session.executor
            ),
            ExecutionProcessStatus::Failed => format!(
                "❌ '{}' execution failed\nBranch: {:?}\nExecutor: {:?}",
                workspace_name, ctx.workspace.branch, ctx.session.executor
            ),
            _ => {
                tracing::warn!(
                    "Tried to notify workspace completion for {} but process is still running!",
                    ctx.workspace.id
                );
                return;
            }
        };
        self.notification_service()
            .notify(&title, &message, Some(ctx.workspace.id))
            .await;
    }

    /// Cleanup executions marked as running in the db, call at startup
    async fn cleanup_orphan_executions(&self) -> Result<(), ContainerError> {
        let running_processes = ExecutionProcess::find_running(&self.db().pool).await?;
        for process in running_processes {
            tracing::info!(
                "Found orphaned execution process {} for session {}",
                process.id,
                process.session_id
            );
            // Update the execution process status first
            if let Err(e) = ExecutionProcess::update_completion(
                &self.db().pool,
                process.id,
                ExecutionProcessStatus::Failed,
                None, // No exit code for orphaned processes
            )
            .await
            {
                tracing::error!(
                    "Failed to update orphaned execution process {} status: {}",
                    process.id,
                    e
                );
                continue;
            }
            // Capture after-head commit OID per repository
            if let Ok(ctx) = ExecutionProcess::load_context(&self.db().pool, process.id).await
                && let Some(ref container_ref) = ctx.workspace.container_ref
            {
                let workspace_root = PathBuf::from(container_ref);
                for repo in &ctx.repos {
                    let repo_path = workspace_root.join(&repo.name);
                    if let Ok(head) = self.git().get_head_info(&repo_path)
                        && let Err(err) = ExecutionProcessRepoState::update_after_head_commit(
                            &self.db().pool,
                            process.id,
                            repo.id,
                            &head.oid,
                        )
                        .await
                    {
                        tracing::warn!(
                            "Failed to update after_head_commit for repo {} on process {}: {}",
                            repo.id,
                            process.id,
                            err
                        );
                    }
                }
            }
            // Process marked as failed
            tracing::info!("Marked orphaned execution process {} as failed", process.id);
        }
        Ok(())
    }

    /// Backfill repo names that were migrated with a sentinel placeholder.
    /// Also backfills dev_script_working_dir and agent_working_dir for single-repo projects.
    async fn backfill_repo_names(&self) -> Result<(), ContainerError> {
        let pool = &self.db().pool;
        let repos = Repo::list_needing_name_fix(pool).await?;

        if repos.is_empty() {
            return Ok(());
        }

        tracing::info!("Backfilling {} repo names", repos.len());

        for repo in repos {
            let name = repo
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&repo.id.to_string())
                .to_string();

            Repo::update_name(pool, repo.id, &name, &name).await?;
        }

        Ok(())
    }

    fn cleanup_actions_for_repos(&self, repos: &[Repo]) -> Option<ExecutorAction> {
        let repos_with_cleanup: Vec<_> = repos
            .iter()
            .filter(|r| r.cleanup_script.is_some())
            .collect();

        if repos_with_cleanup.is_empty() {
            return None;
        }

        let mut iter = repos_with_cleanup.iter();
        let first = iter.next()?;
        let mut root_action = ExecutorAction::new(
            ExecutorActionType::ScriptRequest(ScriptRequest {
                script: first.cleanup_script.clone().unwrap(),
                language: ScriptRequestLanguage::Bash,
                context: ScriptContext::CleanupScript,
                working_dir: Some(first.name.clone()),
            }),
            None,
        );

        for repo in iter {
            root_action = root_action.append_action(ExecutorAction::new(
                ExecutorActionType::ScriptRequest(ScriptRequest {
                    script: repo.cleanup_script.clone().unwrap(),
                    language: ScriptRequestLanguage::Bash,
                    context: ScriptContext::CleanupScript,
                    working_dir: Some(repo.name.clone()),
                }),
                None,
            ));
        }

        Some(root_action)
    }

    fn archive_actions_for_repos(&self, repos: &[Repo]) -> Option<ExecutorAction> {
        let repos_with_archive: Vec<_> = repos
            .iter()
            .filter(|r| r.archive_script.is_some())
            .collect();

        if repos_with_archive.is_empty() {
            return None;
        }

        let mut iter = repos_with_archive.iter();
        let first = iter.next()?;
        let mut root_action = ExecutorAction::new(
            ExecutorActionType::ScriptRequest(ScriptRequest {
                script: first.archive_script.clone().unwrap(),
                language: ScriptRequestLanguage::Bash,
                context: ScriptContext::ArchiveScript,
                working_dir: Some(first.name.clone()),
            }),
            None,
        );

        for repo in iter {
            root_action = root_action.append_action(ExecutorAction::new(
                ExecutorActionType::ScriptRequest(ScriptRequest {
                    script: repo.archive_script.clone().unwrap(),
                    language: ScriptRequestLanguage::Bash,
                    context: ScriptContext::ArchiveScript,
                    working_dir: Some(repo.name.clone()),
                }),
                None,
            ));
        }

        Some(root_action)
    }

    /// Attempts to run the archive script for a workspace if configured.
    /// Silently returns Ok if no archive script is configured or if conditions aren't met.
    async fn try_run_archive_script(&self, workspace_id: Uuid) -> Result<(), ContainerError> {
        let pool = &self.db().pool;
        let workspace = Workspace::find_by_id(pool, workspace_id)
            .await?
            .ok_or(ContainerError::Other(anyhow!("Workspace not found")))?;
        if workspace.is_direct_folder() {
            return Ok(());
        }
        if ExecutionProcess::has_running_non_dev_server_processes_for_workspace(pool, workspace.id)
            .await
            .unwrap_or(true)
        {
            return Ok(());
        }
        if self.ensure_container_exists(&workspace).await.is_err() {
            return Ok(());
        }
        let repos = WorkspaceRepo::find_repos_for_workspace(pool, workspace.id).await?;
        let Some(action) = self.archive_actions_for_repos(&repos) else {
            return Ok(());
        };
        let session = match Session::find_latest_by_workspace_id(pool, workspace.id).await? {
            Some(s) => s,
            None => {
                Session::create(
                    pool,
                    &CreateSession {
                        executor: None,
                        name: None,
                    },
                    Uuid::new_v4(),
                    workspace.id,
                )
                .await?
            }
        };
        self.start_execution(
            &workspace,
            &session,
            &action,
            &ExecutionProcessRunReason::ArchiveScript,
        )
        .await?;

        Ok(())
    }

    /// Archive a workspace: set archived flag, stop running dev servers, and run archive script.
    async fn archive_workspace(&self, workspace_id: Uuid) -> Result<(), ContainerError> {
        let pool = &self.db().pool;

        Workspace::set_archived(pool, workspace_id, true).await?;

        // Stop running dev servers
        if let Ok(dev_servers) =
            ExecutionProcess::find_running_dev_servers_by_workspace(pool, workspace_id).await
        {
            for dev_server in dev_servers {
                if let Err(e) = self
                    .stop_execution(&dev_server, ExecutionProcessStatus::Killed)
                    .await
                {
                    tracing::error!(
                        "Failed to stop dev server {} for workspace {}: {}",
                        dev_server.id,
                        workspace_id,
                        e
                    );
                }
            }
        }

        // Run archive script (silently skips if not configured)
        if let Err(e) = self.try_run_archive_script(workspace_id).await {
            tracing::error!(
                "Failed to run archive script for workspace {}: {}",
                workspace_id,
                e
            );
        }

        Ok(())
    }

    fn setup_actions_for_repos(&self, repos: &[Repo]) -> Option<ExecutorAction> {
        let repos_with_setup: Vec<_> = repos.iter().filter(|r| r.setup_script.is_some()).collect();

        if repos_with_setup.is_empty() {
            return None;
        }

        let mut iter = repos_with_setup.iter();
        let first = iter.next()?;
        let mut root_action = ExecutorAction::new(
            ExecutorActionType::ScriptRequest(ScriptRequest {
                script: first.setup_script.clone().unwrap(),
                language: ScriptRequestLanguage::Bash,
                context: ScriptContext::SetupScript,
                working_dir: Some(first.name.clone()),
            }),
            None,
        );

        for repo in iter {
            root_action = root_action.append_action(ExecutorAction::new(
                ExecutorActionType::ScriptRequest(ScriptRequest {
                    script: repo.setup_script.clone().unwrap(),
                    language: ScriptRequestLanguage::Bash,
                    context: ScriptContext::SetupScript,
                    working_dir: Some(repo.name.clone()),
                }),
                None,
            ));
        }

        Some(root_action)
    }

    fn setup_action_for_repo(repo: &Repo) -> Option<ExecutorAction> {
        repo.setup_script.as_ref().map(|script| {
            ExecutorAction::new(
                ExecutorActionType::ScriptRequest(ScriptRequest {
                    script: script.clone(),
                    language: ScriptRequestLanguage::Bash,
                    context: ScriptContext::SetupScript,
                    working_dir: Some(repo.name.clone()),
                }),
                None,
            )
        })
    }

    fn build_sequential_setup_chain(
        repos: &[&Repo],
        next_action: ExecutorAction,
    ) -> ExecutorAction {
        let mut chained = next_action;
        for repo in repos.iter().rev() {
            if let Some(script) = &repo.setup_script {
                chained = ExecutorAction::new(
                    ExecutorActionType::ScriptRequest(ScriptRequest {
                        script: script.clone(),
                        language: ScriptRequestLanguage::Bash,
                        context: ScriptContext::SetupScript,
                        working_dir: Some(repo.name.clone()),
                    }),
                    Some(Box::new(chained)),
                );
            }
        }
        chained
    }

    /// Reset a session to a specific process: restore worktrees, stop processes, drop later processes.
    async fn reset_session_to_process(
        &self,
        session_id: Uuid,
        target_process_id: Uuid,
        perform_git_reset: bool,
        force_when_dirty: bool,
    ) -> Result<(), ContainerError> {
        let pool = &self.db().pool;

        let process = ExecutionProcess::find_by_id(pool, target_process_id)
            .await?
            .ok_or_else(|| ContainerError::Other(anyhow!("Process not found")))?;
        if process.session_id != session_id {
            return Err(ContainerError::Other(anyhow!(
                "Process does not belong to this session"
            )));
        }

        let session = Session::find_by_id(pool, session_id)
            .await?
            .ok_or_else(|| ContainerError::Other(anyhow!("Session not found")))?;
        let workspace = Workspace::find_by_id(pool, session.workspace_id)
            .await?
            .ok_or_else(|| ContainerError::Other(anyhow!("Workspace not found")))?;

        let repos = WorkspaceRepo::find_repos_for_workspace(pool, workspace.id).await?;
        let repo_states =
            ExecutionProcessRepoState::find_by_execution_process_id(pool, target_process_id)
                .await?;

        let container_ref = self.ensure_container_exists(&workspace).await?;
        let workspace_dir = std::path::PathBuf::from(container_ref);
        let is_dirty = self
            .is_container_clean(&workspace)
            .await
            .map(|is_clean| !is_clean)
            .unwrap_or(false);

        for repo in &repos {
            let repo_state = repo_states.iter().find(|s| s.repo_id == repo.id);
            let target_oid = match repo_state.and_then(|s| s.before_head_commit.clone()) {
                Some(oid) => Some(oid),
                None => {
                    ExecutionProcess::find_prev_after_head_commit(
                        pool,
                        session_id,
                        target_process_id,
                        repo.id,
                    )
                    .await?
                }
            };

            let worktree_path = workspace_dir.join(&repo.name);
            if let Some(oid) = target_oid {
                self.git().reconcile_worktree_to_commit(
                    &worktree_path,
                    &oid,
                    git::WorktreeResetOptions::new(
                        perform_git_reset,
                        force_when_dirty,
                        is_dirty,
                        perform_git_reset,
                    ),
                );
            }
        }

        self.try_stop(&workspace, false).await;
        ExecutionProcess::drop_at_and_after(pool, session_id, target_process_id).await?;

        Ok(())
    }

    async fn try_stop(&self, workspace: &Workspace, include_dev_server: bool) {
        // stop execution processes for this workspace's sessions
        let sessions = match Session::find_by_workspace_id(&self.db().pool, workspace.id).await {
            Ok(s) => s,
            Err(_) => return,
        };

        for session in sessions {
            if let Ok(processes) =
                ExecutionProcess::find_by_session_id(&self.db().pool, session.id, false).await
            {
                for process in processes {
                    // Skip dev server processes unless explicitly included
                    if !include_dev_server
                        && process.run_reason == ExecutionProcessRunReason::DevServer
                    {
                        continue;
                    }
                    if process.status == ExecutionProcessStatus::Running {
                        self.stop_execution(&process, ExecutionProcessStatus::Killed)
                            .await
                            .unwrap_or_else(|e| {
                                tracing::debug!(
                                    "Failed to stop execution process {} for workspace {}: {}",
                                    process.id,
                                    workspace.id,
                                    e
                                );
                            });
                    }
                }
            }
        }
    }

    async fn ensure_container_exists(
        &self,
        workspace: &Workspace,
    ) -> Result<ContainerRef, ContainerError>;

    async fn is_container_clean(&self, workspace: &Workspace) -> Result<bool, ContainerError>;

    async fn start_execution_inner(
        &self,
        workspace: &Workspace,
        execution_process: &ExecutionProcess,
        executor_action: &ExecutorAction,
    ) -> Result<(), ContainerError>;

    async fn stop_execution(
        &self,
        execution_process: &ExecutionProcess,
        status: ExecutionProcessStatus,
    ) -> Result<(), ContainerError>;

    async fn try_commit_changes(&self, ctx: &ExecutionContext) -> Result<bool, ContainerError>;

    async fn copy_project_files(
        &self,
        source_dir: &Path,
        target_dir: &Path,
        copy_files: &str,
    ) -> Result<(), ContainerError>;

    /// Stream diff updates as LogMsg for WebSocket endpoints.
    async fn stream_diff(
        &self,
        workspace: &Workspace,
        stats_only: bool,
    ) -> Result<futures::stream::BoxStream<'static, Result<LogMsg, std::io::Error>>, ContainerError>;

    /// Fetch the MsgStore for a given execution ID, panicking if missing.
    async fn get_msg_store_by_id(&self, uuid: &Uuid) -> Option<Arc<MsgStore>> {
        let map = self.msg_stores().read().await;
        map.get(uuid).cloned()
    }

    async fn git_branch_prefix(&self) -> String;

    async fn git_branch_from_workspace(&self, workspace_id: &Uuid, task_title: &str) -> String {
        let task_title_id = git_branch_id(task_title);
        let prefix = self.git_branch_prefix().await;

        if prefix.is_empty() {
            format!("{}-{}", short_uuid(workspace_id), task_title_id)
        } else {
            format!("{}/{}-{}", prefix, short_uuid(workspace_id), task_title_id)
        }
    }

    async fn stream_raw_logs(
        &self,
        id: &Uuid,
    ) -> Option<futures::stream::BoxStream<'static, Result<LogMsg, std::io::Error>>> {
        if let Some(store) = self.get_msg_store_by_id(id).await {
            // First try in-memory store
            return Some(
                store
                    .history_plus_stream()
                    .filter(|msg| {
                        future::ready(matches!(
                            msg,
                            Ok(LogMsg::Stdout(..) | LogMsg::Stderr(..) | LogMsg::Finished)
                        ))
                    })
                    .boxed(),
            );
        } else {
            let messages = execution_process::load_raw_log_messages(&self.db().pool, *id).await?;

            let stream = futures::stream::iter(
                messages
                    .into_iter()
                    .filter(|m| matches!(m, LogMsg::Stdout(_) | LogMsg::Stderr(_)))
                    .chain(std::iter::once(LogMsg::Finished))
                    .map(Ok::<_, std::io::Error>),
            )
            .boxed();

            Some(stream)
        }
    }

    async fn stream_normalized_logs(
        &self,
        id: &Uuid,
    ) -> Option<futures::stream::BoxStream<'static, Result<LogMsg, std::io::Error>>> {
        if let Some(store) = self.get_msg_store_by_id(id).await {
            Some(
                store
                    .history_plus_stream()
                    .take_while(|msg| future::ready(!matches!(msg, Ok(LogMsg::Finished))))
                    .filter(|msg| future::ready(matches!(msg, Ok(LogMsg::JsonPatch(..)))))
                    .chain(futures::stream::once(async {
                        Ok::<_, std::io::Error>(LogMsg::Finished)
                    }))
                    .boxed(),
            )
        } else {
            let raw_messages =
                execution_process::load_raw_log_messages(&self.db().pool, *id).await?;

            let patches = raw_messages
                .into_iter()
                .filter_map(|message| match message {
                    LogMsg::JsonPatch(patch) => Some(Ok(LogMsg::JsonPatch(patch))),
                    _ => None,
                });
            return Some(
                futures::stream::iter(patches)
                    .chain(futures::stream::once(async {
                        Ok::<_, std::io::Error>(LogMsg::Finished)
                    }))
                    .boxed(),
            );
        }
    }

    async fn start_execution(
        &self,
        workspace: &Workspace,
        session: &Session,
        executor_action: &ExecutorAction,
        run_reason: &ExecutionProcessRunReason,
    ) -> Result<ExecutionProcess, ContainerError> {
        if !matches!(executor_action.typ(), ExecutorActionType::ScriptRequest(_)) {
            return Err(ContainerError::Other(anyhow!(
                "coding-agent ExecutionProcess actions were removed; use the canonical AgentRun API"
            )));
        }

        // Create new execution process record
        // Capture current HEAD per repository as the "before" commit for this execution
        let repositories =
            WorkspaceRepo::find_repos_for_workspace(&self.db().pool, workspace.id).await?;
        if repositories.is_empty() && !workspace.is_direct_folder() {
            return Err(ContainerError::Other(anyhow!(
                "Workspace has no repositories configured"
            )));
        }

        let mut workspace_for_execution = workspace.clone();
        if !has_container_ref(&workspace_for_execution)
            && let Some(refreshed_workspace) =
                Workspace::find_by_id(&self.db().pool, workspace.id).await?
        {
            workspace_for_execution = refreshed_workspace;
        }
        if !has_container_ref(&workspace_for_execution) {
            workspace_for_execution.container_ref = None;
            let container_ref = self
                .ensure_container_exists(&workspace_for_execution)
                .await?;
            workspace_for_execution.container_ref = Some(container_ref);
        }
        let workspace_root = workspace_for_execution
            .container_ref
            .as_ref()
            .filter(|container_ref| !container_ref.is_empty())
            .map(std::path::PathBuf::from)
            .ok_or_else(|| ContainerError::Other(anyhow!("Container ref not found")))?;

        let mut repo_states = Vec::with_capacity(repositories.len());
        for repo in &repositories {
            let repo_path = workspace_root.join(&repo.name);
            let before_head_commit = self.git().get_head_info(&repo_path).ok().map(|h| h.oid);
            repo_states.push(CreateExecutionProcessRepoState {
                repo_id: repo.id,
                before_head_commit,
                after_head_commit: None,
                merge_commit: None,
            });
        }
        let create_execution_process = CreateExecutionProcess {
            session_id: session.id,
            executor_action: executor_action.clone(),
            run_reason: run_reason.clone(),
        };

        let execution_process = ExecutionProcess::create(
            &self.db().pool,
            &create_execution_process,
            Uuid::new_v4(),
            &repo_states,
        )
        .await?;
        let msg_store = Arc::new(MsgStore::new());
        self.msg_stores()
            .write()
            .await
            .insert(execution_process.id, msg_store.clone());
        if *run_reason != ExecutionProcessRunReason::ArchiveScript
            && let Err(e) = Workspace::set_archived(&self.db().pool, workspace.id, false).await
        {
            self.msg_stores()
                .write()
                .await
                .remove(&execution_process.id);
            return Err(e.into());
        }

        if let Err(start_error) = self
            .start_execution_inner(
                &workspace_for_execution,
                &execution_process,
                executor_action,
            )
            .await
        {
            self.msg_stores()
                .write()
                .await
                .remove(&execution_process.id);
            // Mark process as failed
            if let Err(update_error) = ExecutionProcess::update_completion(
                &self.db().pool,
                execution_process.id,
                ExecutionProcessStatus::Failed,
                None,
            )
            .await
            {
                tracing::error!(
                    "Failed to mark execution process {} as failed after start error: {}",
                    execution_process.id,
                    update_error
                );
            }
            // Emit stderr error message
            let log_message = LogMsg::Stderr(format!("Failed to start execution: {start_error}"));
            if let Err(e) = execution_process::append_log_message(
                session.id,
                execution_process.id,
                &log_message,
            )
            .await
            {
                tracing::error!(
                    "Failed to write error log for execution {}: {}",
                    execution_process.id,
                    e
                );
            }

            return Err(start_error);
        }

        execution_process::spawn_stream_raw_logs_to_storage(
            self.msg_stores().clone(),
            execution_process.id,
            session.id,
        );
        Ok(execution_process)
    }

    async fn try_start_next_action(&self, ctx: &ExecutionContext) -> Result<(), ContainerError> {
        let action = ctx.execution_process.executor_action()?;
        let next_action = if let Some(next_action) = action.next_action() {
            next_action
        } else {
            tracing::debug!("No next action configured");
            return Ok(());
        };

        // ExecutionProcess chaining is limited to standalone scripts. Agent
        // actions are created and resumed through the canonical AgentRun API.
        let next_run_reason = ExecutionProcessRunReason::SetupScript;

        self.start_execution(&ctx.workspace, &ctx.session, next_action, &next_run_reason)
            .await?;

        tracing::debug!("Started next action: {:?}", next_action);
        Ok(())
    }
}
