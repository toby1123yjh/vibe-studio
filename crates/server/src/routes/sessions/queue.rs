use axum::{
    Extension, Json, Router, extract::State, middleware::from_fn_with_state,
    response::Json as ResponseJson, routing::get,
};
use db::models::{
    scratch::DraftFollowUpData,
    session::{Session, SessionError},
};
use deployment::Deployment;
use executors::{actions::SelectedSkill, profile::ExecutorConfig};
use serde::Deserialize;
use services::services::queued_message::QueueStatus;
use ts_rs::TS;
use utils::response::ApiResponse;

use crate::{DeploymentImpl, error::ApiError, middleware::load_session_middleware};

/// Request body for queueing a follow-up message
#[derive(Debug, Deserialize, TS)]
struct QueueMessageRequest {
    pub message: String,
    pub executor_config: ExecutorConfig,
    #[serde(default)]
    pub selected_skills: Option<Vec<SelectedSkill>>,
}

/// Queue a follow-up message to be executed when the current execution finishes
async fn queue_message(
    Extension(session): Extension<Session>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<QueueMessageRequest>,
) -> Result<ResponseJson<ApiResponse<QueueStatus>>, ApiError> {
    let queue_guard = deployment
        .queued_message_service()
        .lock_session(session.id)
        .await;
    // Middleware may have loaded the Session before an in-flight deletion.
    let session = Session::find_by_id(&deployment.db().pool, session.id)
        .await?
        .ok_or(SessionError::NotFound)?;
    super::validate_queued_follow_up_profile(
        &deployment.db().pool,
        &session,
        &payload.executor_config,
        payload.selected_skills.as_ref(),
    )
    .await?;

    let data = DraftFollowUpData {
        message: payload.message,
        executor_config: payload.executor_config,
        selected_skills: payload.selected_skills,
    };

    let queued = deployment
        .queued_message_service()
        .queue_message(session.id, data);
    drop(queue_guard);

    deployment
        .track_if_analytics_allowed(
            "follow_up_queued",
            serde_json::json!({
                "session_id": session.id.to_string(),
                "workspace_id": session.workspace_id.to_string(),
            }),
        )
        .await;

    Ok(ResponseJson(ApiResponse::success(QueueStatus::Queued {
        message: queued,
    })))
}

/// Cancel a queued follow-up message
async fn cancel_queued_message(
    Extension(session): Extension<Session>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<QueueStatus>>, ApiError> {
    let guard = deployment
        .queued_message_service()
        .lock_session(session.id)
        .await;
    deployment
        .queued_message_service()
        .cancel_queued(session.id);
    drop(guard);

    deployment
        .track_if_analytics_allowed(
            "follow_up_queue_cancelled",
            serde_json::json!({
                "session_id": session.id.to_string(),
                "workspace_id": session.workspace_id.to_string(),
            }),
        )
        .await;

    Ok(ResponseJson(ApiResponse::success(QueueStatus::Empty)))
}

/// Get the current queue status for a session's workspace
async fn get_queue_status(
    Extension(session): Extension<Session>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<QueueStatus>>, ApiError> {
    let status = deployment.queued_message_service().get_status(session.id);

    Ok(ResponseJson(ApiResponse::success(status)))
}

pub(super) fn router(deployment: &DeploymentImpl) -> Router<DeploymentImpl> {
    Router::new()
        .route(
            "/",
            get(get_queue_status)
                .post(queue_message)
                .delete(cancel_queued_message),
        )
        .layer(from_fn_with_state(
            deployment.clone(),
            load_session_middleware,
        ))
}
