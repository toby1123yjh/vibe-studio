pub(crate) mod agent_run;
pub(crate) mod deletion;
mod executor_config;
mod native_history;
pub mod queue;
pub mod review;
pub mod setup_gate;

use std::path::{Path, PathBuf};

use axum::{
    Extension, Json, Router,
    extract::{Path as AxumPath, Query, State},
    middleware::from_fn_with_state,
    response::Json as ResponseJson,
    routing::{get, post},
};
use chrono::{Duration, Utc};
use db::models::{
    arena_group::{ArenaLifecycleStatus, ArenaMode},
    coding_agent_turn::ResumableAgentSession,
    execution_process::{ExecutionProcess, ExecutionProcessRunReason, ExecutionProcessView},
    requests::UpdateSession,
    scratch::{Scratch, ScratchType},
    session::{CreateSession, Session, SessionError},
    task::{Execution, ExecutionError, ExecutionSummary},
    workspace::{Workspace, WorkspaceError},
    workspace_repo::WorkspaceRepo,
};
use deployment::Deployment;
use executors::{
    actions::SelectedSkill,
    profile::ExecutorConfig,
    runtime::{AgentRunIntent, AgentRunPortSnapshot, RunAttemptMode},
};
pub use native_history::{
    NativeAgentSessionPreview, NativeSessionDiscoveryState, NativeSessionPreviewEntry,
};
use serde::Deserialize;
use services::services::{container::ContainerService, queued_message::QueuedMessageService};
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{
    DeploymentImpl, error::ApiError, middleware::load_session_middleware,
    routes::workspaces::execution::RunScriptError,
};

fn design_arena_prompt(prompt: &str) -> String {
    format!(
        "You are in AI Arena Design Mode.\n\
         Focus on design reasoning, tradeoffs, risks, and decision support.\n\
         Do not create commits, push branches, open PRs, or treat code changes as the final output unless the user explicitly asks to start implementation.\n\n\
         User request:\n{}",
        prompt
    )
}

async fn is_open_design_arena_workspace(
    pool: &sqlx::SqlitePool,
    workspace_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let group = Workspace::find_arena_group_for_workspace(pool, workspace_id).await?;

    Ok(matches!(
        group.map(|g| (g.mode, g.lifecycle_status)),
        Some((ArenaMode::Design, ArenaLifecycleStatus::Open))
    ))
}

#[derive(Debug, Deserialize)]
pub struct SessionQuery {
    pub workspace_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct ResumableAgentSessionQuery {
    pub scope_path: Option<String>,
    pub executor: String,
    pub days: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct NativeAgentSessionPreviewQuery {
    pub scope_path: Option<String>,
    pub executor: String,
    pub session_id: String,
    pub turns: Option<i64>,
}

#[derive(Debug, Deserialize, TS)]
pub struct CreateSessionRequest {
    pub workspace_id: Uuid,
    pub executor: Option<String>,
    pub name: Option<String>,
}

pub async fn get_sessions(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<SessionQuery>,
) -> Result<ResponseJson<ApiResponse<Vec<Session>>>, ApiError> {
    let pool = &deployment.db().pool;
    let sessions = Session::find_by_workspace_id(pool, query.workspace_id).await?;
    Ok(ResponseJson(ApiResponse::success(sessions)))
}

pub async fn get_session(
    Extension(session): Extension<Session>,
) -> Result<ResponseJson<ApiResponse<Session>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(session)))
}

pub async fn delete_session(
    State(deployment): State<DeploymentImpl>,
    Extension(session): Extension<Session>,
    AxumPath(_session_id): AxumPath<Uuid>,
    Query(query): Query<deletion::DeleteSessionQuery>,
) -> Result<ResponseJson<ApiResponse<db::models::requests::SessionDeletionResult>>, ApiError> {
    let _queue_guard =
        lock_session_for_deletion(deployment.queued_message_service(), session.id).await?;
    super::workspaces::managed_directory::preflight_files(
        &deployment,
        session.id,
        query.delete_managed_files,
    )
    .await?;
    deletion::prepare_deletion(&deployment, session.id, None, query.stop_running).await?;
    let result = super::workspaces::managed_directory::delete(
        &deployment,
        session.id,
        None,
        query.delete_managed_files,
    )
    .await?;
    Ok(ResponseJson(ApiResponse::success(result)))
}

pub(crate) async fn lock_session_for_deletion(
    queue: &QueuedMessageService,
    session_id: Uuid,
) -> Result<tokio::sync::OwnedMutexGuard<()>, ApiError> {
    let guard = queue.lock_session(session_id).await;
    if queue.has_queued(session_id) {
        return Err(ApiError::Conflict(
            "Session has a queued follow-up. Cancel it before deleting this session.".to_string(),
        ));
    }
    Ok(guard)
}

pub async fn get_session_task(
    State(deployment): State<DeploymentImpl>,
    Extension(session): Extension<Session>,
) -> Result<ResponseJson<ApiResponse<Option<ExecutionSummary>>>, ApiError> {
    let pool = &deployment.db().pool;
    let task = match Execution::find_agent_by_session_id(pool, session.id).await? {
        Some(task) => Some(
            Execution::summary_by_id(pool, task.id)
                .await?
                .ok_or(ExecutionError::NotFound { task_id: task.id })?,
        ),
        None => None,
    };
    Ok(ResponseJson(ApiResponse::success(task)))
}

pub async fn get_resumable_agent_sessions(
    Query(query): Query<ResumableAgentSessionQuery>,
) -> Result<ResponseJson<ApiResponse<Vec<ResumableAgentSession>>>, ApiError> {
    let days = query.days.unwrap_or(3).clamp(1, 30);
    let limit = query.limit.unwrap_or(10).clamp(1, 50);
    let since = Utc::now() - Duration::days(days);
    let executor = query.executor.trim();
    let Some(scope_path) = native_history_scope_path(query.scope_path.as_deref()) else {
        return Ok(ResponseJson(ApiResponse::success(Vec::new())));
    };

    // The resume picker intentionally reflects the agent's native history only.
    // The selected agent and its exact working directory are the complete
    // lookup context; Vibe workspace/session state must not affect the result.
    let sessions = native_history::list_native_resumable_agent_sessions(
        executor,
        since,
        limit as usize,
        &[scope_path],
        false,
    );

    Ok(ResponseJson(ApiResponse::success(sessions)))
}

pub async fn get_native_session_discovery_state(
    Query(query): Query<ResumableAgentSessionQuery>,
) -> Result<ResponseJson<ApiResponse<NativeSessionDiscoveryState>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        native_history::native_session_discovery_state(query.executor.trim()),
    )))
}

pub async fn get_native_agent_session_preview(
    Query(query): Query<NativeAgentSessionPreviewQuery>,
) -> Result<ResponseJson<ApiResponse<Option<NativeAgentSessionPreview>>>, ApiError> {
    let turn_limit = query
        .turns
        .unwrap_or(native_history::DEFAULT_NATIVE_SESSION_PREVIEW_TURNS as i64);
    let turn_limit = turn_limit.clamp(1, native_history::MAX_NATIVE_SESSION_PREVIEW_TURNS as i64);
    let Some(scope_path) = native_history_scope_path(query.scope_path.as_deref()) else {
        return Ok(ResponseJson(ApiResponse::success(None)));
    };

    let preview = native_history::get_native_agent_session_preview(
        query.executor.trim(),
        &query.session_id,
        turn_limit as usize,
        &[scope_path],
        false,
    );

    Ok(ResponseJson(ApiResponse::success(preview)))
}

fn native_history_scope_path(scope_path: Option<&str>) -> Option<PathBuf> {
    scope_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

pub async fn create_session(
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<ResponseJson<ApiResponse<Session>>, ApiError> {
    let pool = &deployment.db().pool;

    // Verify workspace exists
    let _workspace = Workspace::find_by_id(pool, payload.workspace_id)
        .await?
        .ok_or(ApiError::Workspace(WorkspaceError::ValidationError(
            "Workspace not found".to_string(),
        )))?;

    let session = Session::create(
        pool,
        &CreateSession {
            executor: payload.executor,
            name: payload.name,
        },
        Uuid::new_v4(),
        payload.workspace_id,
    )
    .await?;

    Ok(ResponseJson(ApiResponse::success(session)))
}

pub async fn update_session(
    Extension(session): Extension<Session>,
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<UpdateSession>,
) -> Result<ResponseJson<ApiResponse<Session>>, ApiError> {
    let pool = &deployment.db().pool;

    Session::update(pool, session.id, request.name.as_deref()).await?;

    let updated = Session::find_by_id(pool, session.id)
        .await?
        .ok_or(ApiError::Session(SessionError::NotFound))?;

    Ok(ResponseJson(ApiResponse::success(updated)))
}

#[derive(Debug, Deserialize, TS)]
pub struct CreateFollowUpAttempt {
    pub prompt: String,
    #[serde(default)]
    #[ts(optional)]
    pub selected_skills: Option<Vec<SelectedSkill>>,
    pub executor_config: ExecutorConfig,
    #[serde(default)]
    #[ts(optional)]
    pub resume_session_id: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub resume_scope_path: Option<String>,
}

pub(crate) async fn validate_queued_follow_up_profile(
    pool: &sqlx::SqlitePool,
    session: &Session,
    executor_config: &ExecutorConfig,
    selected_skills: Option<&Vec<SelectedSkill>>,
) -> Result<(), ApiError> {
    agent_run::validate_session_executor(pool, session, executor_config).await?;
    let provider = agent_run::direct_provider(executor_config)?;
    let runtime_profile_id = executor_config.profile_id().cache_key();
    agent_run::validate_session_provider_binding(
        pool,
        session.id,
        provider,
        &runtime_profile_id,
        None,
        executor_config,
        selected_skills,
        None,
    )
    .await
}

pub async fn follow_up(
    Extension(session): Extension<Session>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<CreateFollowUpAttempt>,
) -> Result<ResponseJson<ApiResponse<AgentRunPortSnapshot>>, ApiError> {
    let agent_run = start_coding_agent_execution_for_session(
        &deployment,
        session,
        payload.prompt,
        payload.selected_skills,
        payload.executor_config,
        payload.resume_session_id,
        payload.resume_scope_path,
    )
    .await?;

    Ok(ResponseJson(ApiResponse::success(agent_run)))
}

pub async fn start_coding_agent_execution_for_session(
    deployment: &DeploymentImpl,
    session: Session,
    prompt: String,
    selected_skills: Option<Vec<SelectedSkill>>,
    executor_config: ExecutorConfig,
    resume_session_id: Option<String>,
    resume_scope_path: Option<String>,
) -> Result<AgentRunPortSnapshot, ApiError> {
    prepare_coding_agent_execution_for_session(
        deployment,
        session,
        prompt,
        selected_skills,
        executor_config,
        resume_session_id,
        resume_scope_path,
        agent_run::AgentRunDispatch::Immediate,
    )
    .await
}

pub(crate) async fn reserve_coding_agent_execution_for_session(
    deployment: &DeploymentImpl,
    session: Session,
    prompt: String,
    selected_skills: Option<Vec<SelectedSkill>>,
    executor_config: ExecutorConfig,
    resume_session_id: Option<String>,
    resume_scope_path: Option<String>,
) -> Result<AgentRunPortSnapshot, ApiError> {
    prepare_coding_agent_execution_for_session(
        deployment,
        session,
        prompt,
        selected_skills,
        executor_config,
        resume_session_id,
        resume_scope_path,
        agent_run::AgentRunDispatch::Reserved,
    )
    .await
}

async fn prepare_coding_agent_execution_for_session(
    deployment: &DeploymentImpl,
    session: Session,
    prompt: String,
    selected_skills: Option<Vec<SelectedSkill>>,
    executor_config: ExecutorConfig,
    resume_session_id: Option<String>,
    resume_scope_path: Option<String>,
    dispatch: agent_run::AgentRunDispatch,
) -> Result<AgentRunPortSnapshot, ApiError> {
    let pool = &deployment.db().pool;

    let workspace = Workspace::find_by_id(pool, session.workspace_id)
        .await?
        .ok_or(ApiError::Workspace(WorkspaceError::ValidationError(
            "Workspace not found".to_string(),
        )))?;

    tracing::info!("{:?}", workspace);

    if agent_run::has_active_agent_run_for_session(pool, session.id).await? {
        return Err(ApiError::BadRequest(
            "A coding agent is already running for this session. Queue the follow-up instead."
                .to_string(),
        ));
    }

    let workspace_path = deployment
        .container()
        .ensure_container_exists(&workspace)
        .await?;

    agent_run::validate_session_executor(pool, &session, &executor_config).await?;
    let provider = agent_run::direct_provider(&executor_config)?;
    let runtime_profile_id = executor_config.profile_id().cache_key();
    let resume_scope_path = resume_scope_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let has_explicit_resume = resume_session_id
        .as_deref()
        .is_some_and(|session_id| !session_id.trim().is_empty());
    if has_explicit_resume && resume_scope_path.is_none() {
        return Err(ApiError::BadRequest(
            "Native session adoption requires the exact working directory scope".to_string(),
        ));
    }
    if let (Some(session_id), Some(scope_path)) =
        (resume_session_id.as_deref(), resume_scope_path.as_deref())
    {
        agent_run::validate_native_resume_identity(provider, session_id, scope_path)?;
    }
    let explicit_provider_session = agent_run::explicit_provider_session(
        provider,
        &runtime_profile_id,
        resume_session_id.as_deref(),
        Utc::now(),
    )
    .map(|reference| {
        agent_run::native_adoption_reference(
            reference,
            &executor_config,
            selected_skills.as_ref(),
            Path::new(&workspace_path),
            resume_scope_path.as_deref(),
        )
    });
    agent_run::validate_session_provider_binding(
        pool,
        session.id,
        provider,
        &runtime_profile_id,
        explicit_provider_session.as_ref(),
        &executor_config,
        selected_skills.as_ref(),
        Some(Path::new(&workspace_path)),
    )
    .await?;
    let provider_session = match explicit_provider_session {
        Some(reference) => Some(reference),
        None => {
            agent_run::latest_provider_session(pool, session.id, provider, &runtime_profile_id)
                .await?
        }
    };

    if has_explicit_resume {
        if let Some(reference) = provider_session.as_ref() {
            agent_run::bind_provider_session(pool, session.id, reference).await?;
        }
    }

    let mut prompt = prompt;
    if is_open_design_arena_workspace(pool, workspace.id).await? {
        prompt = design_arena_prompt(&prompt);
    }

    let (intent, mode) = if provider_session.is_some() {
        (
            AgentRunIntent::FollowUp,
            if resume_session_id
                .as_deref()
                .is_some_and(|session_id| !session_id.trim().is_empty())
            {
                RunAttemptMode::Resume
            } else {
                RunAttemptMode::Launch
            },
        )
    } else {
        (AgentRunIntent::Initial, RunAttemptMode::Launch)
    };
    let agent_run = agent_run::create_agent_run(
        deployment,
        &session,
        &workspace,
        workspace_path,
        agent_run::AgentRunLaunch {
            intent,
            mode,
            prompt,
            selected_skills,
            executor_config,
            provider_session,
        },
        dispatch,
    )
    .await?;

    if let Err(e) = Scratch::delete(pool, session.id, &ScratchType::DraftFollowUp).await {
        tracing::debug!(
            "Failed to delete draft follow-up scratch for session {}: {}",
            session.id,
            e
        );
    }

    Ok(agent_run)
}

pub(crate) async fn launch_reserved_coding_agent_execution(
    deployment: &DeploymentImpl,
    agent_run_id: Uuid,
) -> Result<AgentRunPortSnapshot, ApiError> {
    deployment
        .agent_run_port()
        .launch_reserved(agent_run_id)
        .await
        .map_err(agent_run::agent_run_port_error)
}

pub(crate) async fn fail_reserved_coding_agent_execution(
    deployment: &DeploymentImpl,
    agent_run_id: Uuid,
    message: String,
) -> Result<AgentRunPortSnapshot, ApiError> {
    deployment
        .agent_run_port()
        .fail_reserved(agent_run_id, message)
        .await
        .map_err(agent_run::agent_run_port_error)
}

#[cfg(test)]
mod tests {
    use db::models::scratch::DraftFollowUpData;
    use executors::{executors::BaseCodingAgent, profile::ExecutorConfig};
    use services::services::queued_message::QueuedMessageService;
    use uuid::Uuid;

    use super::{lock_session_for_deletion, native_history_scope_path};
    use crate::error::ApiError;

    #[tokio::test]
    async fn deletion_rejects_queued_follow_up_without_consuming_it() {
        let queue = QueuedMessageService::new();
        let session_id = Uuid::new_v4();
        queue.queue_message(
            session_id,
            DraftFollowUpData {
                message: "Keep this queued message".to_string(),
                executor_config: ExecutorConfig::new(BaseCodingAgent::Codex),
                selected_skills: None,
            },
        );
        assert!(matches!(
            lock_session_for_deletion(&queue, session_id).await,
            Err(ApiError::Conflict(message)) if message.contains("queued follow-up")
        ));
        assert_eq!(
            queue.get_queued(session_id).unwrap().data.message,
            "Keep this queued message"
        );
        queue.cancel_queued(session_id);
        assert!(lock_session_for_deletion(&queue, session_id).await.is_ok());
    }

    #[test]
    fn native_history_scope_uses_only_the_explicit_working_directory() {
        let path = native_history_scope_path(Some("  F:/repo  ")).expect("scope path");

        assert_eq!(path.to_string_lossy(), "F:/repo");
    }

    #[test]
    fn native_history_scope_rejects_missing_or_blank_working_directory() {
        assert!(native_history_scope_path(None).is_none());
        assert!(native_history_scope_path(Some("  ")).is_none());
    }
}

pub async fn run_setup_script(
    Extension(session): Extension<Session>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<ExecutionProcessView, RunScriptError>>, ApiError> {
    let pool = &deployment.db().pool;

    let workspace = Workspace::find_by_id(pool, session.workspace_id)
        .await?
        .ok_or(ApiError::Workspace(WorkspaceError::ValidationError(
            "Workspace not found".to_string(),
        )))?;

    if ExecutionProcess::has_running_non_dev_server_processes_for_workspace(pool, workspace.id)
        .await?
    {
        return Ok(ResponseJson(ApiResponse::error_with_data(
            RunScriptError::ProcessAlreadyRunning,
        )));
    }

    deployment
        .container()
        .ensure_container_exists(&workspace)
        .await?;

    let repos = WorkspaceRepo::find_repos_for_workspace(pool, workspace.id).await?;
    let executor_action = match deployment.container().setup_actions_for_repos(&repos) {
        Some(action) => action,
        None => {
            return Ok(ResponseJson(ApiResponse::error_with_data(
                RunScriptError::NoScriptConfigured,
            )));
        }
    };

    let execution_process = deployment
        .container()
        .start_execution(
            &workspace,
            &session,
            &executor_action,
            &ExecutionProcessRunReason::SetupScript,
        )
        .await?;

    deployment
        .track_if_analytics_allowed(
            "setup_script_executed",
            serde_json::json!({
                "workspace_id": workspace.id.to_string(),
            }),
        )
        .await;

    Ok(ResponseJson(ApiResponse::success(
        ExecutionProcessView::from_process(execution_process),
    )))
}

pub fn router(deployment: &DeploymentImpl) -> Router<DeploymentImpl> {
    let session_id_router = Router::new()
        .route(
            "/",
            get(get_session).put(update_session).delete(delete_session),
        )
        .route("/follow-up", post(follow_up))
        .route("/execution", get(get_session_task))
        .route(
            "/deletion-info",
            get(super::workspaces::managed_directory::deletion_info),
        )
        .route(
            "/executor-config",
            get(executor_config::get_executor_config),
        )
        .route("/setup", post(run_setup_script))
        .route("/review", post(review::start_review))
        .layer(from_fn_with_state(
            deployment.clone(),
            load_session_middleware,
        ));

    let sessions_router = Router::new()
        .route("/", get(get_sessions).post(create_session))
        .route("/resumable", get(get_resumable_agent_sessions))
        .route("/resumable-status", get(get_native_session_discovery_state))
        .route("/native-preview", get(get_native_agent_session_preview))
        .nest("/{session_id}", session_id_router)
        .nest("/{session_id}/queue", queue::router(deployment));

    Router::new().nest("/sessions", sessions_router)
}
