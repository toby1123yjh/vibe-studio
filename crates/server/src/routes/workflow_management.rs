//! Local management API and the authenticated, execution-scoped workflow MCP
//! bridge. Model input never supplies project/template/Session authority.
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use deployment::Deployment;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{
    DeploymentImpl,
    error::ApiError,
    workflow_runtime::{
        arena::{DeploymentWorkflowArenaCreator, DeploymentWorkflowArenaWinnerApplier},
        management::{
            self, AcceptedWorkflowSubmission, PrepareWorkflowMainSessionRequest,
            WorkflowContextView, WorkflowDispatchRequest, WorkflowInstanceView,
            WorkflowMainSessionView, WorkflowManagementCaller,
            WorkflowManagementInteractionRequest, WorkflowNotificationPage, WorkflowStopView,
            WorkflowSubmission,
        },
        runner::{DeploymentWorkflowAgentExecutor, DeploymentWorkflowRunCanceller},
        workspace::DeploymentWorkflowWorkspaceResolver,
    },
};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagementPageQuery {
    pub cursor: Option<i64>,
    pub limit: Option<u32>,
    pub run_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct StopWorkflowRequest {
    pub run_id: Uuid,
    pub request_id: String,
}

#[derive(Debug, Serialize, TS)]
pub struct WorkflowManagementError {
    pub code: String,
    pub message: String,
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/workflow-management/prepare-main-session", post(prepare))
        .route(
            "/workflow-management/sessions/{session_id}/context",
            get(context),
        )
        .route(
            "/workflow-management/sessions/{session_id}/notifications",
            get(notifications),
        )
        .route(
            "/workflow-management/instances/{instance_id}",
            get(instance),
        )
        .route(
            "/workflow-management/instances/{instance_id}/submit",
            post(submit),
        )
        .route(
            "/workflow-management/instances/{instance_id}/dispatch",
            post(dispatch),
        )
        .route(
            "/workflow-management/instances/{instance_id}/stop",
            post(stop),
        )
        .route(
            "/workflow-management/instances/{instance_id}/respond",
            post(respond),
        )
        .route("/workflow-management/mcp/{tool}", post(mcp))
}

fn local_caller(instance_id: Uuid) -> WorkflowManagementCaller {
    WorkflowManagementCaller::Instance {
        instance_id,
        namespace: format!("page-instance:{instance_id}"),
        integration_id: None,
    }
}

async fn prepare(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<PrepareWorkflowMainSessionRequest>,
) -> Result<Json<ApiResponse<WorkflowMainSessionView>>, WorkflowManagementApiError> {
    let view = management::prepare_workflow_main_session(
        &deployment.db().pool,
        request,
        &DeploymentWorkflowWorkspaceResolver::new(deployment.clone()),
    )
    .await?;
    Ok(Json(ApiResponse::success(view)))
}
async fn context(
    State(deployment): State<DeploymentImpl>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<ApiResponse<Option<WorkflowContextView>>>, WorkflowManagementApiError> {
    Ok(Json(ApiResponse::success(
        management::read_workflow_context(&deployment.db().pool, session_id).await?,
    )))
}
async fn notifications(
    State(deployment): State<DeploymentImpl>,
    Path(session_id): Path<Uuid>,
    Query(query): Query<ManagementPageQuery>,
) -> Result<Json<ApiResponse<WorkflowNotificationPage>>, WorkflowManagementApiError> {
    Ok(Json(ApiResponse::success(
        management::list_workflow_notifications(
            &deployment.db().pool,
            session_id,
            query.cursor.unwrap_or(0),
            query.limit.unwrap_or(50),
        )
        .await?,
    )))
}
async fn instance(
    State(deployment): State<DeploymentImpl>,
    Path(instance_id): Path<Uuid>,
    Query(query): Query<ManagementPageQuery>,
) -> Result<Json<ApiResponse<Option<WorkflowInstanceView>>>, WorkflowManagementApiError> {
    Ok(Json(ApiResponse::success(
        management::get_workflow_instance(
            &deployment.db().pool,
            &local_caller(instance_id),
            query.run_id,
            query.cursor,
            query.limit.unwrap_or(20),
        )
        .await?,
    )))
}
async fn submit(
    State(deployment): State<DeploymentImpl>,
    Path(instance_id): Path<Uuid>,
    Json(request): Json<WorkflowSubmission>,
) -> Result<(StatusCode, Json<ApiResponse<AcceptedWorkflowSubmission>>), WorkflowManagementApiError>
{
    let view = management::submit_workflow(
        &deployment.db().pool,
        &local_caller(instance_id),
        request,
        None,
        "manual",
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(ApiResponse::success(view))))
}
async fn dispatch(
    State(deployment): State<DeploymentImpl>,
    Path(instance_id): Path<Uuid>,
    Json(request): Json<WorkflowDispatchRequest>,
) -> Result<Json<ApiResponse<management::AcceptedWorkflowDispatch>>, WorkflowManagementApiError> {
    let view =
        management::dispatch_workflow(&deployment.db().pool, &local_caller(instance_id), request)
            .await?;
    Ok(Json(ApiResponse::success(view)))
}
async fn stop(
    State(deployment): State<DeploymentImpl>,
    Path(instance_id): Path<Uuid>,
    Json(request): Json<StopWorkflowRequest>,
) -> Result<Json<ApiResponse<WorkflowStopView>>, WorkflowManagementApiError> {
    let view = management::stop_workflow(
        &deployment.db().pool,
        &local_caller(instance_id),
        request.run_id,
        &request.request_id,
        &DeploymentWorkflowRunCanceller::new(deployment.clone()),
    )
    .await?;
    Ok(Json(ApiResponse::success(view)))
}
async fn respond(
    State(deployment): State<DeploymentImpl>,
    Path(instance_id): Path<Uuid>,
    Json(request): Json<WorkflowManagementInteractionRequest>,
) -> Result<
    Json<ApiResponse<crate::routes::workflows::WorkflowRunResponse>>,
    WorkflowManagementApiError,
> {
    let run = management::respond_to_workflow(
        &deployment.db().pool,
        &local_caller(instance_id),
        request,
        &DeploymentWorkflowAgentExecutor::new(deployment.clone()),
        &DeploymentWorkflowArenaCreator::new(deployment.clone()),
        &DeploymentWorkflowArenaWinnerApplier::new(deployment.clone()),
    )
    .await?;
    Ok(Json(ApiResponse::success(run)))
}

fn mcp_caller(headers: &HeaderMap) -> Result<WorkflowManagementCaller, ApiError> {
    let uuid_header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or(ApiError::Unauthorized)
    };
    let token = headers
        .get("Authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
        .ok_or(ApiError::Unauthorized)?;
    Ok(WorkflowManagementCaller::MainSession {
        session_id: uuid_header("X-Workflow-Session-Id")?,
        agent_run_id: Some(uuid_header("X-Workflow-Agent-Run-Id")?),
        turn_id: Some(uuid_header("X-Workflow-Turn-Id")?),
        token_hash: Some(format!("{:x}", Sha256::digest(token.as_bytes()))),
    })
}

pub struct WorkflowManagementApiError(pub ApiError);
impl From<ApiError> for WorkflowManagementApiError {
    fn from(error: ApiError) -> Self {
        Self(error)
    }
}
impl IntoResponse for WorkflowManagementApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self.0 {
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "workflow_context_unavailable",
                "Workflow launch authority is absent or no longer valid".into(),
            ),
            ApiError::Forbidden(message) => {
                (StatusCode::FORBIDDEN, "out_of_scope", message.clone())
            }
            ApiError::BadRequest(message) => {
                let code = if message.starts_with("INVALID_REWORK_SCOPE") {
                    "invalid_rework_scope"
                } else if message.starts_with("INVALID_RETRY_TARGET") {
                    "invalid_retry_target"
                } else if message.to_ascii_lowercase().contains("interaction") {
                    "interaction_conflict"
                } else {
                    "invalid_workflow_request"
                };
                (StatusCode::BAD_REQUEST, code, message.clone())
            }
            ApiError::Conflict(message) => {
                let code = if message.starts_with("IDEMPOTENCY_CONFLICT") {
                    "idempotency_conflict"
                } else if message.starts_with("STALE_EXECUTION_BASIS") {
                    "stale_execution_basis"
                } else if message.starts_with("REUSE_UNAVAILABLE") {
                    "reuse_unavailable"
                } else if message.starts_with("INVALID_RETRY_TARGET") {
                    "invalid_retry_target"
                } else if message.starts_with("INSTANCE_DEFINITION_LOCKED") {
                    "instance_definition_locked"
                } else if message.starts_with("INSTANCE_BINDING_CONFLICT") {
                    "instance_binding_conflict"
                } else if message.to_ascii_lowercase().contains("interaction") {
                    "interaction_conflict"
                } else if message.contains("Session")
                    && (message.contains("deleted")
                        || message.contains("unavailable")
                        || message.contains("binding"))
                {
                    "workflow_context_unavailable"
                } else {
                    "workflow_conflict"
                };
                (StatusCode::CONFLICT, code, message.clone())
            }
            _ => {
                tracing::warn!(error=%self.0,"Workflow MCP operation failed");
                (StatusCode::INTERNAL_SERVER_ERROR,"workflow_unavailable","Workflow service could not complete this operation; inspect the service log and retry with the same request_id".into())
            }
        };
        (
            status,
            Json(
                ApiResponse::<Value, WorkflowManagementError>::error_with_data(
                    WorkflowManagementError {
                        code: code.into(),
                        message,
                    },
                ),
            ),
        )
            .into_response()
    }
}

async fn mcp(
    State(deployment): State<DeploymentImpl>,
    Path(tool): Path<String>,
    headers: HeaderMap,
    Json(params): Json<Value>,
) -> Result<Json<ApiResponse<Value>>, WorkflowManagementApiError> {
    let caller = mcp_caller(&headers)?;
    let pool = &deployment.db().pool;
    management::verify_management_caller(pool, &caller).await?;
    let parse =
        |value: Value| ApiError::BadRequest(format!("Invalid workflow tool parameters: {value}"));
    let encode = |value: Result<Value, serde_json::Error>| {
        value.map_err(|_| ApiError::Conflict("Workflow response could not be encoded".into()))
    };
    let result =
        match tool.as_str() {
            "workflow_context" => {
                if !params.as_object().is_some_and(|object| object.is_empty()) {
                    return Err(ApiError::BadRequest(
                        "workflow_context has no identity parameters".into(),
                    )
                    .into());
                }
                let WorkflowManagementCaller::MainSession { session_id, .. } = &caller else {
                    return Err(ApiError::Unauthorized.into());
                };
                encode(serde_json::to_value(
                    management::read_workflow_context(pool, *session_id).await?,
                ))?
            }
            "workflow_list" => {
                let query: ManagementPageQuery = serde_json::from_value(params)
                    .map_err(|error| parse(Value::String(error.to_string())))?;
                if query.run_id.is_some() {
                    return Err(ApiError::BadRequest(
                        "workflow_list does not accept a Run identity".into(),
                    )
                    .into());
                }
                encode(serde_json::to_value(
                    management::list_callable_workflows(
                        pool,
                        &caller,
                        query.cursor.unwrap_or(0),
                        query.limit.unwrap_or(20),
                    )
                    .await?,
                ))?
            }
            "workflow_get" => {
                let query: ManagementPageQuery = serde_json::from_value(params)
                    .map_err(|error| parse(Value::String(error.to_string())))?;
                encode(serde_json::to_value(
                    management::get_workflow_instance(
                        pool,
                        &caller,
                        query.run_id,
                        query.cursor,
                        query.limit.unwrap_or(20),
                    )
                    .await?,
                ))?
            }
            "workflow_submit" => {
                let request: WorkflowSubmission = serde_json::from_value(params)
                    .map_err(|error| parse(Value::String(error.to_string())))?;
                encode(serde_json::to_value(
                    management::submit_workflow(pool, &caller, request, None, "main_agent").await?,
                ))?
            }
            "workflow_dispatch" => {
                let request: WorkflowDispatchRequest = serde_json::from_value(params)
                    .map_err(|error| parse(Value::String(error.to_string())))?;
                encode(serde_json::to_value(
                    management::dispatch_workflow(pool, &caller, request).await?,
                ))?
            }
            "workflow_stop" => {
                let request: StopWorkflowRequest = serde_json::from_value(params)
                    .map_err(|error| parse(Value::String(error.to_string())))?;
                encode(serde_json::to_value(
                    management::stop_workflow(
                        pool,
                        &caller,
                        request.run_id,
                        &request.request_id,
                        &DeploymentWorkflowRunCanceller::new(deployment.clone()),
                    )
                    .await?,
                ))?
            }
            "workflow_respond" => {
                let request: WorkflowManagementInteractionRequest = serde_json::from_value(params)
                    .map_err(|error| parse(Value::String(error.to_string())))?;
                encode(serde_json::to_value(
                    management::respond_to_workflow(
                        pool,
                        &caller,
                        request,
                        &DeploymentWorkflowAgentExecutor::new(deployment.clone()),
                        &DeploymentWorkflowArenaCreator::new(deployment.clone()),
                        &DeploymentWorkflowArenaWinnerApplier::new(deployment.clone()),
                    )
                    .await?,
                ))?
            }
            _ => return Err(ApiError::BadRequest(
                "Unknown workflow tool; arbitrary Session dispatch is unavailable in workflow mode"
                    .into(),
            )
            .into()),
        };
    Ok(Json(ApiResponse::success(result)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_page_and_mcp_errors_keep_contract_codes() {
        for (error, status, code) in [
            (
                ApiError::Unauthorized,
                StatusCode::UNAUTHORIZED,
                "workflow_context_unavailable",
            ),
            (
                ApiError::Conflict("STALE_EXECUTION_BASIS: source changed".into()),
                StatusCode::CONFLICT,
                "stale_execution_basis",
            ),
            (
                ApiError::Conflict("REUSE_UNAVAILABLE: no upstream result".into()),
                StatusCode::CONFLICT,
                "reuse_unavailable",
            ),
            (
                ApiError::BadRequest("INVALID_REWORK_SCOPE: unknown frozen Node".into()),
                StatusCode::BAD_REQUEST,
                "invalid_rework_scope",
            ),
            (
                ApiError::Conflict("INVALID_RETRY_TARGET: source is active".into()),
                StatusCode::CONFLICT,
                "invalid_retry_target",
            ),
            (
                ApiError::Conflict("INSTANCE_BINDING_CONFLICT: publication changed".into()),
                StatusCode::CONFLICT,
                "instance_binding_conflict",
            ),
            (
                ApiError::Conflict("INSTANCE_DEFINITION_LOCKED: accepted definition".into()),
                StatusCode::CONFLICT,
                "instance_definition_locked",
            ),
            (
                ApiError::Conflict("Workflow interaction is stale".into()),
                StatusCode::CONFLICT,
                "interaction_conflict",
            ),
            (
                ApiError::Conflict("IDEMPOTENCY_CONFLICT: changed payload".into()),
                StatusCode::CONFLICT,
                "idempotency_conflict",
            ),
        ] {
            let response = WorkflowManagementApiError(error).into_response();
            assert_eq!(response.status(), status);
            let bytes = axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["error_data"]["code"], code);
            assert!(body["error_data"]["message"].is_string());
        }
    }
}
