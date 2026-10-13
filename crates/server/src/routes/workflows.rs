use std::collections::HashMap;

use api_types::{DeleteResponse, MutationResponse};
use axum::{
    BoxError, Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Json as ResponseJson, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use db::models::{
    scratch::DraftWorkspaceRepo,
    session::{CreateSession, Session},
    task::{CreateExecution, Execution, ExecutionKind},
    workflow::{NodeExecutionStatus, WorkflowAttemptStatus, WorkflowRunStatus, WorkflowSource},
    workflow_file_changes::{
        WorkflowFileChangeSummary, WorkflowFileChanges, WorkflowFileCollectionStatus,
    },
    workflow_files::{WorkflowDeliveryView, WorkflowHandoffView, WorkflowSplitBatchView},
    workspace_repo::CreateWorkspaceRepo,
};
use deployment::Deployment;
use executors::{profile::ExecutorConfig, runtime::ProjectionStatus};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, sqlite::SqliteRow};
use thiserror::Error;
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;
use workflow::{
    WorkflowGraph, graph::WorkflowNodeKind, templates::built_in_templates,
    validation::validate_graph,
};

use crate::{
    DeploymentImpl,
    error::ApiError,
    routes::{
        integrations::workflows::WorkflowInteractionResponse,
        workflow_management::WorkflowManagementApiError,
    },
    workflow_runtime::{
        arena::{
            DeploymentWorkflowArenaCreator, DeploymentWorkflowArenaWinnerApplier,
            NoopWorkflowArenaCreator, WorkflowArenaCreator,
        },
        management::{self, WorkflowManagementCaller, WorkflowManagementInteractionRequest},
        runner::{
            DeploymentAgentRunReconciliationBoundary, DeploymentWorkflowAgentExecutor,
            DeploymentWorkflowRunCanceller, WorkflowAgentExecutor, WorkflowWorkspaceRequest,
            WorkflowWorkspaceResolver, get_workflow_run_response,
            reconcile_workflow_run_with_arena_and_boundary, retry_workflow_node_with_arena,
            subscribe_workflow_events, workflow_event_history,
        },
        workspace::{DeploymentWorkflowWorkspaceResolver, main_workflow_branch_name},
    },
};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowTemplateResponse {
    pub id: Uuid,
    pub source: WorkflowSource,
    pub project_id: Option<Uuid>,
    pub name: String,
    pub description: Option<String>,
    pub graph_json: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub external_enabled: bool,
    pub main_agent_config: Option<ExecutorConfig>,
    pub main_agent_prompt: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowTemplateListResponse {
    pub workflows: Vec<WorkflowTemplateResponse>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct CreateWorkflowRequest {
    pub name: String,
    pub description: Option<String>,
    pub graph_json: String,
}

#[derive(Debug, Deserialize, TS)]
pub struct WorkflowExternalAccessRequest {
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct UpdateWorkflowRequest {
    #[ts(type = "number")]
    pub expected_revision: i64,
    pub name: Option<String>,
    pub description: Option<String>,
    pub graph_json: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub main_agent_config: Option<ExecutorConfig>,
    #[serde(default)]
    #[ts(optional)]
    pub main_agent_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowRevisionConflict {
    pub workflow_id: Uuid,
    #[ts(type = "number")]
    pub expected_revision: i64,
    #[ts(type = "number")]
    pub current_revision: i64,
}

#[derive(Debug, Error)]
pub enum WorkflowUpdateError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("workflow {0:?} changed while it was being edited")]
    RevisionConflict(WorkflowRevisionConflict),
}

impl From<WorkflowUpdateError> for ApiError {
    fn from(error: WorkflowUpdateError) -> Self {
        match error {
            WorkflowUpdateError::Api(error) => error,
            WorkflowUpdateError::RevisionConflict(conflict) => ApiError::Conflict(format!(
                "workflow {} revision changed from {} to {}",
                conflict.workflow_id, conflict.expected_revision, conflict.current_revision
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct TriggerWorkflowRequest {
    #[serde(rename = "task_id")]
    #[ts(rename = "task_id")]
    pub issue_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub trigger_source: String,
    pub input_text: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct CreateWorkflowAttemptRequest {
    #[serde(default)]
    #[ts(optional)]
    pub directory_path: Option<String>,
    pub name: Option<String>,
    pub graph_json: String,
    #[serde(default)]
    #[ts(optional)]
    pub repos: Option<Vec<DraftWorkspaceRepo>>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct RunWorkflowAttemptRequest {
    #[serde(default)]
    #[ts(optional)]
    pub directory_path: Option<String>,
    pub workspace_id: Option<Uuid>,
    pub trigger_source: String,
    pub input_text: String,
    #[serde(default)]
    #[ts(optional)]
    pub repos: Option<Vec<DraftWorkspaceRepo>>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct SelectArenaWinnerRequest {
    pub candidate_id: Uuid,
    pub node_execution_id: Uuid,
}

#[derive(Debug, Deserialize, TS)]
pub struct RespondWorkflowNodeRequest {
    pub node_execution_id: Uuid,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct SelectConditionBranchRequest {
    pub node_execution_id: Uuid,
    pub selected_target_node_ids: Vec<String>,
    #[serde(default)]
    #[ts(optional)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowAttemptResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    #[serde(rename = "task_id")]
    #[ts(rename = "task_id")]
    pub issue_id: Uuid,
    pub workflow_id: Uuid,
    pub template_id: Option<Uuid>,
    pub latest_run_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub status: WorkflowAttemptStatus,
    pub main_session_id: Option<Uuid>,
    pub main_session_bound_at: Option<DateTime<Utc>>,
    pub definition_locked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowAttemptListResponse {
    pub attempts: Vec<WorkflowAttemptResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowRunResponse {
    pub id: Uuid,
    pub orchestration_run_id: Option<Uuid>,
    pub workflow_id: Uuid,
    pub attempt_id: Option<Uuid>,
    #[serde(rename = "task_id")]
    #[ts(rename = "task_id")]
    pub issue_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub trigger_source: String,
    pub input_text: String,
    pub output_text: Option<String>,
    pub status: WorkflowRunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error_text: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub nodes: Vec<WorkflowNodeExecutionResponse>,
    #[serde(default)]
    pub split_batches: Vec<WorkflowSplitBatchView>,
    #[serde(default)]
    pub handoffs: Vec<WorkflowHandoffView>,
    pub delivery: Option<WorkflowDeliveryView>,
    pub request_path: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub runtime_view: Option<WorkflowRunRuntimeView>,
    #[serde(default)]
    #[ts(optional)]
    pub queue_phase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowNodeExecutionResponse {
    pub id: Uuid,
    pub run_id: Uuid,
    #[serde(rename = "execution_id")]
    #[ts(rename = "execution_id")]
    pub task_id: Option<Uuid>,
    pub node_id: String,
    pub node_type: String,
    pub iteration: i64,
    pub status: NodeExecutionStatus,
    pub input_text: Option<String>,
    pub output_text: Option<String>,
    pub session_id: Option<Uuid>,
    pub orchestration_node_execution_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
    pub projection_status: Option<ProjectionStatus>,
    pub execution_process_id: Option<Uuid>,
    pub arena_group_id: Option<Uuid>,
    pub tokens_used: Option<i64>,
    pub cost_estimate: Option<f64>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error_text: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum WorkflowRuntimeHealth {
    Ok,
    Starting,
    Slow,
    ProjectionDegraded,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum WorkflowNodeWorkStatus {
    Pending,
    Starting,
    Running,
    AwaitingHuman,
    AwaitingArena,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    Skipped,
    Reused,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowNodeWorkView {
    pub node_id: String,
    pub node_type: String,
    pub iteration: i64,
    pub status: WorkflowNodeWorkStatus,
    pub pending_work_count: i32,
    pub starting_child_count: i32,
    pub active_execution_id: Option<Uuid>,
    pub active_session_id: Option<Uuid>,
    pub orchestration_node_execution_id: Option<Uuid>,
    pub active_agent_run_id: Option<Uuid>,
    pub projection_status: Option<ProjectionStatus>,
    pub active_started_at: Option<DateTime<Utc>>,
    pub active_elapsed_ms: Option<i32>,
    pub active_slow: bool,
    pub active_slow_threshold_ms: i32,
    pub runtime_health: WorkflowRuntimeHealth,
    pub can_open_session: bool,
    pub can_retry: bool,
    pub can_approve: bool,
    pub can_reject: bool,
    pub can_select_arena_winner: bool,
    pub can_select_condition_branch: bool,
    pub can_cancel_node: bool,
    /// Read-only exact source results, never fabricated executions of this Run.
    #[serde(default)]
    pub reused_results: Vec<crate::workflow_runtime::management::WorkflowReuseView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowRunRuntimeView {
    pub run_id: Uuid,
    pub status: WorkflowRunStatus,
    pub active_node_count: i32,
    pub pending_node_count: i32,
    pub waiting_node_count: i32,
    pub failed_node_count: i32,
    pub completed_node_count: i32,
    #[serde(default)]
    pub reused_node_count: i32,
    #[serde(default)]
    pub skipped_node_count: i32,
    pub node_work: Vec<WorkflowNodeWorkView>,
}

pub const WORKFLOW_NODE_ACTIVE_SLOW_THRESHOLD_MS: i32 = 5 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowActionResponse {
    pub run_id: Uuid,
    pub node_id: Option<String>,
    pub status: WorkflowRunStatus,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FallbackWorkflowsQuery {
    pub project_id: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FallbackWorkflowRunsQuery {
    #[serde(rename = "task_id")]
    pub issue_id: Option<Uuid>,
    pub workflow_id: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FallbackNodeExecutionsQuery {
    pub run_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowRunFallbackRow {
    pub id: Uuid,
    pub orchestration_run_id: Option<Uuid>,
    pub workflow_id: Uuid,
    pub attempt_id: Option<Uuid>,
    #[serde(rename = "task_id")]
    pub issue_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub trigger_source: String,
    pub input_text: String,
    pub output_text: Option<String>,
    pub status: WorkflowRunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error_text: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
struct NodeExecutionFallbackRow {
    pub id: Uuid,
    pub run_id: Uuid,
    pub node_id: String,
    pub node_type: String,
    pub iteration: i64,
    pub status: NodeExecutionStatus,
    pub input_text: Option<String>,
    pub output_text: Option<String>,
    pub session_id: Option<Uuid>,
    pub orchestration_node_execution_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
    pub projection_status: Option<ProjectionStatus>,
    pub execution_process_id: Option<Uuid>,
    pub arena_group_id: Option<Uuid>,
    pub tokens_used: Option<i64>,
    pub cost_estimate: Option<f64>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error_text: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Error)]
enum WorkflowRouteError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("{0}")]
    BadRequest(String),
}

impl From<WorkflowRouteError> for ApiError {
    fn from(error: WorkflowRouteError) -> Self {
        match error {
            WorkflowRouteError::Database(err) => ApiError::Database(err),
            WorkflowRouteError::BadRequest(message) => ApiError::BadRequest(message),
        }
    }
}

pub fn router(deployment: &DeploymentImpl) -> Router<DeploymentImpl> {
    Router::new()
        .route(
            "/v1/projects/{project_id}/workflows",
            get(list_workflows).post(create_workflow),
        )
        .route(
            "/v1/projects/{project_id}/workflow-attempts",
            get(list_project_workflow_attempts),
        )
        .route(
            "/v1/projects/{project_id}/tasks/{task_id}/workflow-attempts",
            get(list_issue_workflow_attempts).post(create_workflow_attempt),
        )
        .route(
            "/v1/workflows/{workflow_id}",
            get(get_workflow)
                .put(update_workflow)
                .delete(delete_workflow),
        )
        .route(
            "/v1/workflows/{workflow_id}/attempt",
            get(get_workflow_attempt_by_workflow),
        )
        .route(
            "/v1/workflows/{workflow_id}/trigger",
            post(trigger_workflow),
        )
        .route(
            "/v1/workflows/{workflow_id}/external-access",
            axum::routing::put(set_external_access),
        )
        .route("/v1/workflow-runs/{run_id}", get(get_workflow_run))
        .route(
            "/v1/workflow-runs/{run_id}/file-changes",
            get(get_workflow_file_changes),
        )
        .route(
            "/v1/workflow-attempts/{attempt_id}",
            get(get_workflow_attempt).delete(delete_workflow_attempt),
        )
        .route(
            "/v1/workflow-attempts/{attempt_id}/run",
            post(run_workflow_attempt),
        )
        .route(
            "/v1/workflow-runs/{run_id}/cancel",
            post(cancel_workflow_run),
        )
        .route(
            "/v1/workflow-runs/{run_id}/events",
            get(workflow_run_events),
        )
        .route(
            "/v1/workflow-runs/{run_id}/nodes/{node_id}/retry",
            post(retry_node),
        )
        .route(
            "/v1/workflow-runs/{run_id}/nodes/{node_id}/approve",
            post(approve_node),
        )
        .route(
            "/v1/workflow-runs/{run_id}/nodes/{node_id}/reject",
            post(reject_node),
        )
        .route(
            "/v1/workflow-runs/{run_id}/nodes/{node_id}/arena-winner",
            post(select_arena_winner),
        )
        .route(
            "/v1/workflow-runs/{run_id}/nodes/{node_id}/condition-branch",
            post(select_condition_branch),
        )
        .with_state(deployment.clone())
}

async fn list_workflows(
    State(deployment): State<DeploymentImpl>,
    Path(project_id): Path<Uuid>,
) -> Result<ResponseJson<WorkflowTemplateListResponse>, ApiError> {
    Ok(ResponseJson(WorkflowTemplateListResponse {
        workflows: list_project_workflows(&deployment.db().pool, project_id).await?,
    }))
}

async fn list_project_workflow_attempts(
    State(deployment): State<DeploymentImpl>,
    Path(project_id): Path<Uuid>,
) -> Result<ResponseJson<WorkflowAttemptListResponse>, ApiError> {
    Ok(ResponseJson(WorkflowAttemptListResponse {
        attempts: list_workflow_attempts_for_project(&deployment.db().pool, project_id).await?,
    }))
}

async fn create_workflow(
    State(deployment): State<DeploymentImpl>,
    Path(project_id): Path<Uuid>,
    Json(request): Json<CreateWorkflowRequest>,
) -> Result<ResponseJson<MutationResponse<WorkflowTemplateResponse>>, ApiError> {
    let data = create_project_workflow(&deployment.db().pool, project_id, request).await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn list_issue_workflow_attempts(
    State(deployment): State<DeploymentImpl>,
    Path((project_id, issue_id)): Path<(Uuid, Uuid)>,
) -> Result<ResponseJson<WorkflowAttemptListResponse>, ApiError> {
    Ok(ResponseJson(WorkflowAttemptListResponse {
        attempts: list_workflow_attempts_for_issue(&deployment.db().pool, project_id, issue_id)
            .await?,
    }))
}

async fn create_workflow_attempt(
    State(deployment): State<DeploymentImpl>,
    Path((project_id, issue_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<CreateWorkflowAttemptRequest>,
) -> Result<ResponseJson<MutationResponse<WorkflowAttemptResponse>>, ApiError> {
    super::project_store::require_task(&deployment.db().pool, project_id, issue_id).await?;
    let workspace_resolver = DeploymentWorkflowWorkspaceResolver::new(deployment.clone());
    let data = create_issue_workflow_attempt_with_resources(
        &deployment.db().pool,
        project_id,
        issue_id,
        request,
        &workspace_resolver,
    )
    .await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn get_workflow_attempt(
    State(deployment): State<DeploymentImpl>,
    Path(attempt_id): Path<Uuid>,
) -> Result<ResponseJson<WorkflowAttemptResponse>, ApiError> {
    Ok(ResponseJson(
        workflow_attempt_by_id(&deployment.db().pool, attempt_id)
            .await?
            .ok_or_else(|| ApiError::BadRequest("Workflow attempt not found".to_string()))?,
    ))
}

async fn delete_workflow_attempt(
    State(deployment): State<DeploymentImpl>,
    Path(attempt_id): Path<Uuid>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    delete_issue_workflow_attempt(&deployment.db().pool, attempt_id).await?;
    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn get_workflow_attempt_by_workflow(
    State(deployment): State<DeploymentImpl>,
    Path(workflow_id): Path<Uuid>,
) -> Result<ResponseJson<Option<WorkflowAttemptResponse>>, ApiError> {
    Ok(ResponseJson(
        workflow_attempt_by_workflow_id(&deployment.db().pool, workflow_id).await?,
    ))
}

async fn run_workflow_attempt(
    State(deployment): State<DeploymentImpl>,
    Path(attempt_id): Path<Uuid>,
    Json(request): Json<RunWorkflowAttemptRequest>,
) -> Result<
    (
        StatusCode,
        ResponseJson<MutationResponse<WorkflowRunResponse>>,
    ),
    ApiError,
> {
    let workspace_resolver = DeploymentWorkflowWorkspaceResolver::new(deployment.clone());
    let data = accept_workflow_attempt(
        &deployment.db().pool,
        Uuid::new_v4(),
        attempt_id,
        request,
        &workspace_resolver,
    )
    .await?;

    Ok((
        StatusCode::ACCEPTED,
        ResponseJson(MutationResponse { data, txid: txid() }),
    ))
}

async fn get_workflow(
    State(deployment): State<DeploymentImpl>,
    Path(workflow_id): Path<Uuid>,
) -> Result<ResponseJson<WorkflowTemplateResponse>, ApiError> {
    Ok(ResponseJson(
        get_workflow_template(&deployment.db().pool, workflow_id).await?,
    ))
}

async fn update_workflow(
    State(deployment): State<DeploymentImpl>,
    Path(workflow_id): Path<Uuid>,
    Json(request): Json<UpdateWorkflowRequest>,
) -> Result<Response, ApiError> {
    match update_workflow_template(&deployment.db().pool, workflow_id, request).await {
        Ok(data) => Ok(ResponseJson(MutationResponse { data, txid: txid() }).into_response()),
        Err(WorkflowUpdateError::RevisionConflict(conflict)) => Ok((
            StatusCode::CONFLICT,
            ResponseJson(ApiResponse::<
                MutationResponse<WorkflowTemplateResponse>,
                WorkflowRevisionConflict,
            >::error_with_data(conflict)),
        )
            .into_response()),
        Err(WorkflowUpdateError::Api(error)) => Err(error),
    }
}

async fn delete_workflow(
    State(deployment): State<DeploymentImpl>,
    Path(workflow_id): Path<Uuid>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    delete_workflow_template(&deployment.db().pool, workflow_id).await?;
    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn set_external_access(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
    Json(request): Json<WorkflowExternalAccessRequest>,
) -> Result<ResponseJson<MutationResponse<WorkflowTemplateResponse>>, ApiError> {
    let pool = &deployment.db().pool;
    ensure_system_workflows(pool).await?;
    let changed=sqlx::query("UPDATE workflows SET external_enabled=?, updated_at=datetime('now','subsec') WHERE id=? AND id NOT IN (SELECT workflow_id FROM workflow_attempts)")
        .bind(request.enabled).bind(id).execute(pool).await?.rows_affected();
    if changed == 0 {
        return Err(ApiError::BadRequest(
            "Choose a reusable workflow template".to_string(),
        ));
    }
    Ok(ResponseJson(MutationResponse {
        data: get_workflow_template(pool, id).await?,
        txid: txid(),
    }))
}

pub async fn external_workflow_templates(
    pool: &SqlitePool,
) -> Result<Vec<WorkflowTemplateResponse>, ApiError> {
    ensure_system_workflows(pool).await?;
    let rows=sqlx::query("SELECT id,source,project_id,name,description,graph_json,revision,external_enabled,main_agent_config_json,main_agent_prompt,created_at,updated_at FROM workflows WHERE external_enabled=1 AND id NOT IN (SELECT workflow_id FROM workflow_attempts) ORDER BY name,id")
        .fetch_all(pool).await?;
    let allowed_system = built_in_workflow_ids()?;
    let templates = rows
        .iter()
        .map(workflow_template_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(templates
        .into_iter()
        .filter(|w| w.source != WorkflowSource::System || allowed_system.contains(&w.id))
        .collect())
}

pub async fn list_project_workflows(
    pool: &SqlitePool,
    project_id: Uuid,
) -> Result<Vec<WorkflowTemplateResponse>, ApiError> {
    ensure_system_workflows(pool).await?;
    let active_system_workflow_ids = built_in_workflow_ids()?;

    let mut query = QueryBuilder::<Sqlite>::new(
        r#"
        SELECT id, source, project_id, name, description, graph_json, revision, external_enabled, main_agent_config_json, main_agent_prompt,
               created_at, updated_at
        FROM workflows
        WHERE (
            (source = 'system' AND id IN (
        "#,
    );
    let mut separated = query.separated(", ");
    for workflow_id in &active_system_workflow_ids {
        separated.push_bind(*workflow_id);
    }
    separated.push_unseparated(
        r#"
            ))
            OR project_id =
        "#,
    );
    drop(separated);
    query.push_bind(project_id);
    query.push(
        r#"
        )
          AND id NOT IN (SELECT workflow_id FROM workflow_attempts)
        ORDER BY
            CASE source WHEN 'system' THEN 0 ELSE 1 END,
            name ASC,
            created_at ASC
        "#,
    );

    let rows = query.build().fetch_all(pool).await?;

    Ok(rows
        .iter()
        .map(workflow_template_from_row)
        .collect::<Result<Vec<_>, _>>()?)
}

pub async fn create_project_workflow(
    pool: &SqlitePool,
    project_id: Uuid,
    request: CreateWorkflowRequest,
) -> Result<WorkflowTemplateResponse, ApiError> {
    ensure_project_exists(pool, project_id).await?;
    validate_graph_json(&request.graph_json)?;

    let workflow_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, ?, ?, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(request.name)
    .bind(request.description)
    .bind(request.graph_json)
    .execute(pool)
    .await?;

    workflow_by_id(pool, workflow_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Workflow not found after create".to_string()))
}

pub async fn create_issue_workflow_attempt(
    pool: &SqlitePool,
    project_id: Uuid,
    issue_id: Uuid,
    request: CreateWorkflowAttemptRequest,
) -> Result<WorkflowAttemptResponse, ApiError> {
    ensure_project_exists(pool, project_id).await?;
    ensure_issue_belongs_to_project(pool, project_id, issue_id).await?;
    validate_graph_json(&request.graph_json)?;

    if let Some(id) =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM workflow_attempts WHERE issue_id=?")
            .bind(issue_id)
            .fetch_optional(pool)
            .await?
    {
        return Err(ApiError::Conflict(format!(
            "Task already has workflow instance {id}; use that instance instead of creating another"
        )));
    }

    let name = request
        .name
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Workflow attempt".to_string());

    let attempt_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let mut transaction = pool.begin().await?;

    insert_workflow_attempt(
        &mut transaction,
        attempt_id,
        workflow_id,
        project_id,
        issue_id,
        name,
        request.graph_json,
    )
    .await?;
    transaction.commit().await?;
    workflow_attempt_by_id(pool, attempt_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Workflow attempt not found after create".to_string()))
}

pub async fn insert_workflow_attempt(
    transaction: &mut sqlx::SqliteConnection,
    attempt_id: Uuid,
    workflow_id: Uuid,
    project_id: Uuid,
    issue_id: Uuid,
    name: String,
    graph_json: String,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, ?, ?, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(&name)
    .bind("Task-bound workflow attempt backing graph. Hidden from template lists.")
    .bind(graph_json)
    .execute(&mut *transaction)
    .await?;

    Execution::create(
        &mut *transaction,
        &CreateExecution {
            id: attempt_id,
            project_id,
            issue_id,
            parent_task_id: None,
            title: name,
            execution_kind: ExecutionKind::Workflow,
        },
    )
    .await?;

    sqlx::query(
        r#"
        INSERT INTO workflow_attempts (id, task_id, workflow_id, issue_id, status)
        VALUES (?, ?, ?, ?, 'draft')
        "#,
    )
    .bind(attempt_id)
    .bind(attempt_id)
    .bind(workflow_id)
    .bind(issue_id)
    .execute(&mut *transaction)
    .await?;

    Ok(())
}

pub async fn create_issue_workflow_attempt_with_resources<W>(
    pool: &SqlitePool,
    project_id: Uuid,
    issue_id: Uuid,
    request: CreateWorkflowAttemptRequest,
    workspace_resolver: &W,
) -> Result<WorkflowAttemptResponse, ApiError>
where
    W: WorkflowWorkspaceResolver,
{
    let repo_overrides = workflow_workspace_repo_overrides(request.repos.as_deref().unwrap_or(&[]))
        .map_err(ApiError::BadRequest)?;
    let directory_path =
        workflow_workspace_directory_override(request.directory_path.as_deref(), &repo_overrides)
            .map_err(ApiError::BadRequest)?;
    let attempt = create_issue_workflow_attempt(pool, project_id, issue_id, request).await?;
    let workspace_id = match workspace_resolver
        .create_or_bind_main_workspace(WorkflowWorkspaceRequest {
            issue_id,
            run_id: attempt.id,
            project_id: Some(project_id),
            existing_workspace_id: None,
            directory_path,
            repo_overrides,
            branch_name: main_workflow_branch_name(issue_id, attempt.id),
        })
        .await
    {
        Ok(workspace_id) => workspace_id,
        Err(error) => {
            if let Err(cleanup_error) = delete_issue_workflow_attempt(pool, attempt.id).await {
                tracing::warn!(
                    workflow_attempt_id = %attempt.id,
                    "failed to compensate Workflow attempt after workspace creation failed: {cleanup_error:#}"
                );
            }
            return Err(error);
        }
    };

    let resource_result = async {
        let workflow = get_workflow_template(pool, attempt.workflow_id).await?;
        let mut graph: WorkflowGraph = serde_json::from_str(&workflow.graph_json)
            .map_err(|err| ApiError::BadRequest(format!("Invalid workflow graph JSON: {err}")))?;
        if ensure_agent_node_sessions(pool, workspace_id, &mut graph).await? {
            persist_workflow_graph(pool, attempt.workflow_id, workflow.revision, &graph)
                .await
                .map_err(ApiError::from)?;
        }

        update_workflow_attempt_runtime(
            pool,
            attempt.id,
            None,
            Some(workspace_id),
            WorkflowAttemptStatus::Ready,
        )
        .await?;

        workflow_attempt_by_id(pool, attempt.id)
            .await?
            .ok_or_else(|| {
                ApiError::BadRequest("Workflow attempt not found after resource bind".to_string())
            })
    }
    .await;

    match resource_result {
        Ok(attempt) => Ok(attempt),
        Err(error) => {
            if let Err(cleanup_error) = workspace_resolver
                .cleanup_created_main_workspace(workspace_id)
                .await
            {
                tracing::warn!(
                    workflow_attempt_id = %attempt.id,
                    %workspace_id,
                    "failed to compensate Workflow workspace after resource initialization failed: {cleanup_error:#}"
                );
            }
            if let Err(cleanup_error) = delete_issue_workflow_attempt(pool, attempt.id).await {
                tracing::warn!(
                    workflow_attempt_id = %attempt.id,
                    "failed to compensate Workflow attempt after resource initialization failed: {cleanup_error:#}"
                );
            }
            Err(error)
        }
    }
}

fn workflow_workspace_directory_override(
    directory_path: Option<&str>,
    repos: &[CreateWorkspaceRepo],
) -> Result<Option<String>, String> {
    let Some(path) = directory_path else {
        return Ok(None);
    };
    if path.trim().is_empty() {
        return Err("A directory path is required for direct folder workspaces.".to_string());
    }
    if !repos.is_empty() {
        return Err(
            "Choose either a direct folder or worktree repositories, not both.".to_string(),
        );
    }
    Ok(Some(path.trim().to_string()))
}

fn workflow_workspace_repo_overrides(
    repos: &[DraftWorkspaceRepo],
) -> Result<Vec<CreateWorkspaceRepo>, String> {
    repos
        .iter()
        .map(|repo| {
            let target_branch = repo.target_branch.trim();
            if target_branch.is_empty() {
                return Err(
                    "Every selected workflow repository must include a target branch.".to_string(),
                );
            }

            Ok(CreateWorkspaceRepo {
                repo_id: repo.repo_id,
                target_branch: target_branch.to_string(),
            })
        })
        .collect()
}

pub async fn ensure_agent_node_sessions(
    pool: &SqlitePool,
    workspace_id: Uuid,
    graph: &mut WorkflowGraph,
) -> Result<bool, ApiError> {
    let mut changed = false;
    for node in graph.nodes.iter_mut().filter(|node| {
        matches!(
            node.kind,
            WorkflowNodeKind::Agent | WorkflowNodeKind::TaskSplitter
        )
    }) {
        if node.data.session_id.is_some() {
            continue;
        }

        let display_name = node
            .data
            .display_name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(node.id.as_str());
        let session = Session::create(
            pool,
            &CreateSession {
                executor: None,
                name: Some(format!("Workflow {display_name}")),
            },
            Uuid::new_v4(),
            workspace_id,
        )
        .await?;
        node.data.session_id = Some(session.id.to_string());
        changed = true;
    }

    Ok(changed)
}

pub async fn list_workflow_attempts_for_project(
    pool: &SqlitePool,
    project_id: Uuid,
) -> Result<Vec<WorkflowAttemptResponse>, ApiError> {
    ensure_project_exists(pool, project_id).await?;

    let rows = sqlx::query(
        r#"
        SELECT attempt.id, task.project_id, task.issue_id, attempt.workflow_id,
               (SELECT template_id FROM workflow_attempt_sources WHERE attempt_id=attempt.id) AS template_id,
               attempt.latest_run_id, attempt.workspace_id, task.title AS name,
               attempt.status, attempt.main_session_id, attempt.main_session_bound_at, attempt.definition_locked_at, attempt.created_at, attempt.updated_at
        FROM workflow_attempts attempt
        JOIN tasks task ON task.id = attempt.task_id
        WHERE task.project_id = ?
        ORDER BY attempt.updated_at DESC, attempt.created_at DESC
        "#,
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(workflow_attempt_from_row)
        .collect::<Result<Vec<_>, _>>()?)
}

pub async fn persist_workflow_graph(
    pool: &SqlitePool,
    workflow_id: Uuid,
    expected_revision: i64,
    graph: &WorkflowGraph,
) -> Result<WorkflowTemplateResponse, WorkflowUpdateError> {
    validate_graph(graph)
        .map_err(|err| ApiError::BadRequest(format!("Invalid workflow graph: {err}")))?;
    let graph_json = serde_json::to_string(graph)
        .map_err(|err| ApiError::BadRequest(format!("Invalid workflow graph JSON: {err}")))?;

    let row = sqlx::query(
        r#"
        UPDATE workflows
        SET graph_json = ?,
            revision = revision + 1,
            updated_at = datetime('now', 'subsec')
        WHERE id = ? AND revision = ?
          AND NOT EXISTS(SELECT 1 FROM workflow_attempts a WHERE a.workflow_id=workflows.id AND a.definition_locked_at IS NOT NULL)
        RETURNING id, source, project_id, name, description, graph_json,
                  revision, external_enabled, main_agent_config_json, main_agent_prompt, created_at, updated_at
        "#,
    )
    .bind(graph_json)
    .bind(workflow_id)
    .bind(expected_revision)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;

    match row {
        Some(row) => workflow_template_from_row(&row)
            .map_err(ApiError::from)
            .map_err(WorkflowUpdateError::from),
        None => {
            ensure_workflow_definition_editable(pool, workflow_id).await?;
            let current_revision =
                sqlx::query_scalar::<_, i64>("SELECT revision FROM workflows WHERE id = ?")
                    .bind(workflow_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(ApiError::from)?
                    .ok_or_else(|| ApiError::BadRequest("Workflow not found".to_string()))?;
            Err(WorkflowUpdateError::RevisionConflict(
                WorkflowRevisionConflict {
                    workflow_id,
                    expected_revision,
                    current_revision,
                },
            ))
        }
    }
}

pub async fn list_workflow_attempts_for_issue(
    pool: &SqlitePool,
    project_id: Uuid,
    issue_id: Uuid,
) -> Result<Vec<WorkflowAttemptResponse>, ApiError> {
    ensure_project_exists(pool, project_id).await?;
    ensure_issue_belongs_to_project(pool, project_id, issue_id).await?;

    let rows = sqlx::query(
        r#"
        SELECT attempt.id, task.project_id, task.issue_id, attempt.workflow_id,
               (SELECT template_id FROM workflow_attempt_sources WHERE attempt_id=attempt.id) AS template_id,
               attempt.latest_run_id, attempt.workspace_id, task.title AS name,
               attempt.status, attempt.main_session_id, attempt.main_session_bound_at, attempt.definition_locked_at, attempt.created_at, attempt.updated_at
        FROM workflow_attempts attempt
        JOIN tasks task ON task.id = attempt.task_id
        WHERE task.project_id = ? AND task.issue_id = ?
        ORDER BY attempt.updated_at DESC, attempt.created_at DESC
        "#,
    )
    .bind(project_id)
    .bind(issue_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(workflow_attempt_from_row)
        .collect::<Result<Vec<_>, _>>()?)
}

pub async fn workflow_attempt_by_id(
    pool: &SqlitePool,
    attempt_id: Uuid,
) -> Result<Option<WorkflowAttemptResponse>, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT attempt.id, task.project_id, task.issue_id, attempt.workflow_id,
               (SELECT template_id FROM workflow_attempt_sources WHERE attempt_id=attempt.id) AS template_id,
               attempt.latest_run_id, attempt.workspace_id, task.title AS name,
               attempt.status, attempt.main_session_id, attempt.main_session_bound_at, attempt.definition_locked_at, attempt.created_at, attempt.updated_at
        FROM workflow_attempts attempt
        JOIN tasks task ON task.id = attempt.task_id
        WHERE attempt.id = ?
        "#,
    )
    .bind(attempt_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.as_ref().map(workflow_attempt_from_row).transpose()?)
}

pub async fn workflow_attempt_by_workflow_id(
    pool: &SqlitePool,
    workflow_id: Uuid,
) -> Result<Option<WorkflowAttemptResponse>, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT attempt.id, task.project_id, task.issue_id, attempt.workflow_id,
               (SELECT template_id FROM workflow_attempt_sources WHERE attempt_id=attempt.id) AS template_id,
               attempt.latest_run_id, attempt.workspace_id, task.title AS name,
               attempt.status, attempt.main_session_id, attempt.main_session_bound_at, attempt.definition_locked_at, attempt.created_at, attempt.updated_at
        FROM workflow_attempts attempt
        JOIN tasks task ON task.id = attempt.task_id
        WHERE attempt.workflow_id = ?
        "#,
    )
    .bind(workflow_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.as_ref().map(workflow_attempt_from_row).transpose()?)
}

pub async fn update_workflow_attempt_runtime(
    pool: &SqlitePool,
    attempt_id: Uuid,
    latest_run_id: Option<Uuid>,
    workspace_id: Option<Uuid>,
    status: WorkflowAttemptStatus,
) -> Result<(), ApiError> {
    let result = sqlx::query(
        r#"
        UPDATE workflow_attempts
        SET latest_run_id = COALESCE(?, latest_run_id),
            workspace_id = COALESCE(?, workspace_id),
            status = ?,
            updated_at = datetime('now', 'subsec')
        WHERE id = ? AND (? IS NULL OR latest_run_id IS NULL OR latest_run_id=?)
        "#,
    )
    .bind(latest_run_id)
    .bind(workspace_id)
    .bind(workflow_attempt_status_value(status))
    .bind(attempt_id)
    .bind(latest_run_id)
    .bind(latest_run_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 && workflow_attempt_by_id(pool, attempt_id).await?.is_none() {
        return Err(ApiError::BadRequest(
            "Workflow attempt not found".to_string(),
        ));
    }

    Ok(())
}

pub async fn mark_workflow_attempt_ready(
    pool: &SqlitePool,
    attempt_id: Uuid,
) -> Result<(), ApiError> {
    let result = sqlx::query(
        r#"
        UPDATE workflow_attempts
        SET status = 'ready',
            updated_at = datetime('now', 'subsec')
        WHERE id = ? AND status = 'draft'
        "#,
    )
    .bind(attempt_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 && workflow_attempt_by_id(pool, attempt_id).await?.is_none() {
        return Err(ApiError::BadRequest(
            "Workflow attempt not found".to_string(),
        ));
    }

    Ok(())
}

pub async fn delete_issue_workflow_attempt(
    pool: &SqlitePool,
    attempt_id: Uuid,
) -> Result<(), ApiError> {
    let attempt = workflow_attempt_by_id(pool, attempt_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Workflow attempt not found".to_string()))?;

    if matches!(
        attempt.status,
        WorkflowAttemptStatus::Running
            | WorkflowAttemptStatus::AwaitingHuman
            | WorkflowAttemptStatus::AwaitingArena
    ) {
        return Err(ApiError::BadRequest(
            "Cancel the running workflow attempt before deleting it.".to_string(),
        ));
    }

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let run_ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM workflow_runs WHERE attempt_id=? OR workflow_id=?")
            .bind(attempt.id)
            .bind(attempt.workflow_id)
            .fetch_all(&mut *tx)
            .await?;
    for run_id in run_ids {
        if !crate::workflow_runtime::management::source_boundary_settled_in(&mut tx, run_id).await?
        {
            return Err(ApiError::Conflict("Stop all workflow Runs and wait for their real processes to exit before deleting this instance.".into()));
        }
    }
    // Historical references are restrictive so a single source cannot vanish.
    // Whole-instance deletion removes dependent records before their sources.
    for query in [
        "DELETE FROM workflow_run_deliveries WHERE run_id IN (SELECT id FROM workflow_runs WHERE attempt_id=? OR workflow_id=?)",
        "DELETE FROM workflow_run_submissions WHERE run_id IN (SELECT id FROM workflow_runs WHERE attempt_id=? OR workflow_id=?)",
        "DELETE FROM workflow_result_reuse WHERE run_id IN (SELECT id FROM workflow_runs WHERE attempt_id=? OR workflow_id=?)",
        "UPDATE workflow_split_subtasks SET source_subtask_id=NULL WHERE batch_id IN (SELECT b.id FROM workflow_split_batches b JOIN workflow_runs r ON r.id=b.run_id WHERE r.attempt_id=? OR r.workflow_id=?)",
        "UPDATE workflow_split_batches SET source_batch_id=NULL WHERE run_id IN (SELECT id FROM workflow_runs WHERE attempt_id=? OR workflow_id=?)",
        "DELETE FROM workflow_split_batches WHERE run_id IN (SELECT id FROM workflow_runs WHERE attempt_id=? OR workflow_id=?)",
    ] {
        sqlx::query(query)
            .bind(attempt.id)
            .bind(attempt.workflow_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM workflow_main_session_bindings WHERE workflow_id=?")
        .bind(attempt.workflow_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM workflow_main_session_requests WHERE workflow_id=?")
        .bind(attempt.workflow_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query("UPDATE workflow_attempts SET latest_run_id = NULL WHERE id = ?")
        .bind(attempt.id)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        r#"
        DELETE FROM node_executions
        WHERE run_id IN (
            SELECT id
            FROM workflow_runs
            WHERE attempt_id = ? OR workflow_id = ?
        )
        "#,
    )
    .bind(attempt.id)
    .bind(attempt.workflow_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM workflow_runs WHERE attempt_id = ? OR workflow_id = ?")
        .bind(attempt.id)
        .bind(attempt.workflow_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        r#"
        DELETE FROM tasks
        WHERE id = (SELECT task_id FROM workflow_attempts WHERE id = ?)
        "#,
    )
    .bind(attempt.id)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM workflows WHERE id = ?")
        .bind(attempt.workflow_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(())
}

pub async fn accept_workflow_attempt<W: WorkflowWorkspaceResolver>(
    pool: &SqlitePool,
    run_id: Uuid,
    attempt_id: Uuid,
    request: RunWorkflowAttemptRequest,
    resolver: &W,
) -> Result<WorkflowRunResponse, ApiError> {
    let attempt = workflow_attempt_by_id(pool, attempt_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Workflow attempt not found".to_string()))?;
    let repos = workflow_workspace_repo_overrides(request.repos.as_deref().unwrap_or(&[]))
        .map_err(ApiError::BadRequest)?;
    let directory =
        workflow_workspace_directory_override(request.directory_path.as_deref(), &repos)
            .map_err(ApiError::BadRequest)?;
    use crate::workflow_runtime::management::{
        self, WorkflowActivePolicy, WorkflowManagementCaller, WorkflowSubmission,
        WorkflowSubmissionAction, WorkflowSubmissionScope,
    };
    let integration_id: Option<Uuid> = if request.trigger_source == "external" {
        sqlx::query_scalar("SELECT integration_id FROM external_integration_requests WHERE resource_id=? AND operation='workflow_run'")
            .bind(run_id).fetch_optional(pool).await?
    } else {
        None
    };
    if request.trigger_source == "external" && integration_id.is_none() {
        return Err(ApiError::Forbidden(
            "External acceptance requires its durable integration request".into(),
        ));
    }
    let caller = WorkflowManagementCaller::Instance {
        instance_id: attempt_id,
        namespace: integration_id
            .map(|id| format!("integration:{id}"))
            .unwrap_or_else(|| format!("page-instance:{attempt_id}")),
        integration_id,
    };
    management::verify_management_caller(pool, &caller).await?;
    if let Some(existing) =
        sqlx::query("SELECT input_text,attempt_id,trigger_source FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_optional(pool)
            .await?
    {
        if existing.try_get::<Option<Uuid>, _>("attempt_id")? != Some(attempt_id)
            || existing.try_get::<String, _>("input_text")? != request.input_text
            || existing.try_get::<String, _>("trigger_source")? != request.trigger_source
        {
            return Err(ApiError::Conflict(
                "IDEMPOTENCY_CONFLICT: accepted Run has different parameters".into(),
            ));
        }
        return get_workflow_run_response(pool, run_id).await;
    }
    let workspace_id = if let Some(id) = attempt.workspace_id {
        id
    } else {
        let id = resolver
            .create_or_bind_main_workspace(WorkflowWorkspaceRequest {
                directory_path: directory,
                issue_id: attempt.issue_id,
                run_id: attempt.id,
                project_id: Some(attempt.project_id),
                existing_workspace_id: request.workspace_id,
                repo_overrides: repos,
                branch_name: format!("vk/main-session/{}", attempt.id),
            })
            .await?;
        sqlx::query("UPDATE workflow_attempts SET workspace_id=? WHERE id=? AND workspace_id IS NULL AND definition_locked_at IS NULL")
            .bind(id).bind(attempt.id).execute(pool).await?;
        sqlx::query_scalar("SELECT workspace_id FROM workflow_attempts WHERE id=?")
            .bind(attempt.id)
            .fetch_one(pool)
            .await?
    };
    if request.workspace_id.is_some_and(|id| id != workspace_id) {
        return Err(ApiError::Conflict(
            "Workflow instance workspace is fixed".into(),
        ));
    }
    let accepted = management::submit_workflow(
        pool,
        &caller,
        WorkflowSubmission {
            request_id: run_id.to_string(),
            action: if attempt.latest_run_id.is_some() {
                WorkflowSubmissionAction::Rework
            } else {
                WorkflowSubmissionAction::Start
            },
            input_text: Some(request.input_text),
            material_paths: Vec::new(),
            source_run_id: attempt.latest_run_id,
            source_node_execution_id: None,
            scope: WorkflowSubmissionScope::All,
            active_policy: attempt
                .latest_run_id
                .map(|_| WorkflowActivePolicy::AfterCurrent),
            source_message_id: None,
            delivery_intent: None,
            source_batch_id: None,
        },
        Some(run_id),
        &request.trigger_source,
    )
    .await?;
    let run = get_workflow_run_response(pool, accepted.run_id).await?;
    sync_attempt_from_run(pool, &run).await?;
    Ok(run)
}

pub async fn run_workflow_attempt_runtime<W, A>(
    pool: &SqlitePool,
    attempt_id: Uuid,
    request: RunWorkflowAttemptRequest,
    workspace_resolver: &W,
    agent_executor: &A,
) -> Result<WorkflowRunResponse, ApiError>
where
    W: WorkflowWorkspaceResolver,
    A: WorkflowAgentExecutor,
{
    let arena_creator = NoopWorkflowArenaCreator;
    run_workflow_attempt_runtime_with_arena(
        pool,
        attempt_id,
        request,
        workspace_resolver,
        agent_executor,
        &arena_creator,
    )
    .await
}

pub async fn run_workflow_attempt_runtime_with_arena<W, A, R>(
    pool: &SqlitePool,
    attempt_id: Uuid,
    request: RunWorkflowAttemptRequest,
    workspace_resolver: &W,
    _agent_executor: &A,
    _arena_creator: &R,
) -> Result<WorkflowRunResponse, ApiError>
where
    W: WorkflowWorkspaceResolver,
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
{
    // Scheduled calls and the legacy page helper use the same asynchronous
    // acceptance boundary. Only the dispatcher may claim and start a Run.
    accept_workflow_attempt(
        pool,
        Uuid::new_v4(),
        attempt_id,
        request,
        workspace_resolver,
    )
    .await
}

pub async fn accept_workflow_template_for_issue<W: WorkflowWorkspaceResolver>(
    pool: &SqlitePool,
    workflow_id: Uuid,
    request: TriggerWorkflowRequest,
    resolver: &W,
) -> Result<WorkflowRunResponse, ApiError> {
    let template = get_workflow_template(pool, workflow_id).await?;
    let project_id: Uuid = sqlx::query_scalar("SELECT project_id FROM local_issues WHERE id=?")
        .bind(request.issue_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Task does not exist".into()))?;
    super::project_store::require_task(pool, project_id, request.issue_id).await?;
    if template.project_id.is_some_and(|id| id != project_id) {
        return Err(ApiError::Forbidden(
            "Workflow is not available in this Task's project".into(),
        ));
    }
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let existing=sqlx::query("SELECT a.id,a.workflow_id,s.template_id FROM workflow_attempts a LEFT JOIN workflow_attempt_sources s ON s.attempt_id=a.id WHERE a.issue_id=?")
        .bind(request.issue_id).fetch_optional(&mut *tx).await?;
    let instance_id = if let Some(row) = existing {
        let backing_id: Uuid = row.try_get("workflow_id")?;
        if backing_id != workflow_id
            && row
                .try_get::<Option<Uuid>, _>("template_id")?
                .unwrap_or(backing_id)
                != workflow_id
        {
            return Err(ApiError::Conflict(
                "INSTANCE_BINDING_CONFLICT: Task already has a different workflow publication"
                    .into(),
            ));
        }
        row.try_get("id")?
    } else {
        let instance_id = Uuid::new_v4();
        let backing_id = Uuid::new_v4();
        let mut graph: WorkflowGraph = serde_json::from_str(&template.graph_json)
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;
        for node in &mut graph.nodes {
            node.data.session_id = None;
        }
        insert_workflow_attempt(
            &mut tx,
            instance_id,
            backing_id,
            project_id,
            request.issue_id,
            template.name,
            serde_json::to_string(&graph)
                .map_err(|error| ApiError::BadRequest(error.to_string()))?,
        )
        .await?;
        sqlx::query("INSERT INTO workflow_attempt_sources(attempt_id,template_id,template_revision) VALUES (?,?,?)")
            .bind(instance_id).bind(workflow_id).bind(template.revision).execute(&mut *tx).await?;
        sqlx::query("UPDATE workflows SET main_agent_config_json=?,main_agent_prompt=? WHERE id=?")
            .bind(
                template
                    .main_agent_config
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|error| ApiError::BadRequest(error.to_string()))?,
            )
            .bind(template.main_agent_prompt)
            .bind(backing_id)
            .execute(&mut *tx)
            .await?;
        instance_id
    };
    tx.commit().await?;
    accept_workflow_attempt(
        pool,
        Uuid::new_v4(),
        instance_id,
        RunWorkflowAttemptRequest {
            directory_path: None,
            workspace_id: request.workspace_id,
            trigger_source: request.trigger_source,
            input_text: request.input_text,
            repos: None,
        },
        resolver,
    )
    .await
}

pub async fn sync_attempt_from_run(
    pool: &SqlitePool,
    run: &WorkflowRunResponse,
) -> Result<(), ApiError> {
    if let Some(attempt_id) = run.attempt_id {
        update_workflow_attempt_runtime(
            pool,
            attempt_id,
            Some(run.id),
            run.workspace_id,
            workflow_attempt_status_from_run(run.status),
        )
        .await?;
    }
    Ok(())
}

pub async fn get_workflow_template(
    pool: &SqlitePool,
    workflow_id: Uuid,
) -> Result<WorkflowTemplateResponse, ApiError> {
    ensure_system_workflows(pool).await?;
    workflow_by_id(pool, workflow_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Workflow not found".to_string()))
}

pub async fn update_workflow_template(
    pool: &SqlitePool,
    workflow_id: Uuid,
    request: UpdateWorkflowRequest,
) -> Result<WorkflowTemplateResponse, WorkflowUpdateError> {
    ensure_system_workflows(pool).await?;
    let existing = workflow_by_id(pool, workflow_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Workflow not found".to_string()))?;
    ensure_workflow_definition_editable(pool, workflow_id).await?;

    if existing.source == WorkflowSource::System {
        return Err(
            ApiError::Forbidden("system workflow templates cannot be updated".to_string()).into(),
        );
    }

    let mut pending_sessions = Vec::new();
    let graph_json = if let Some(graph_json) = request.graph_json {
        let graph = parse_graph_json(&graph_json).map_err(ApiError::from)?;
        if let Some(workflow_attempt) = workflow_attempt_by_workflow_id(pool, workflow_id).await? {
            if let Some(workspace_id) = workflow_attempt.workspace_id {
                let mut graph = graph;
                let agent_working_dir = Session::resolve_agent_working_dir(pool, workspace_id)
                    .await
                    .map_err(ApiError::from)?;
                for node in graph
                    .nodes
                    .iter_mut()
                    .filter(|node| node.kind == WorkflowNodeKind::Agent)
                {
                    if node.data.session_id.is_some() {
                        continue;
                    }

                    let session_id = Uuid::new_v4();
                    let display_name = node
                        .data
                        .display_name
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or(node.id.as_str());
                    pending_sessions.push((
                        session_id,
                        workspace_id,
                        format!("Workflow {display_name}"),
                        agent_working_dir.clone(),
                    ));
                    node.data.session_id = Some(session_id.to_string());
                }
                serde_json::to_string(&graph).map_err(|err| {
                    ApiError::BadRequest(format!("Invalid workflow graph JSON: {err}"))
                })?
            } else {
                graph_json
            }
        } else {
            graph_json
        }
    } else {
        existing.graph_json
    };

    let main_agent_config_json = request
        .main_agent_config
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| {
            ApiError::BadRequest(format!("Invalid main Agent configuration: {error}"))
        })?;
    let mut transaction = pool.begin().await.map_err(ApiError::from)?;
    let row = sqlx::query(
        r#"
        UPDATE workflows
        SET name = ?, description = ?, graph_json = ?,
            main_agent_config_json=COALESCE(?,main_agent_config_json),
            main_agent_prompt=COALESCE(?,main_agent_prompt),
            revision = revision + 1,
            updated_at = datetime('now', 'subsec')
        WHERE id = ? AND revision = ?
          AND NOT EXISTS(SELECT 1 FROM workflow_attempts a WHERE a.workflow_id=workflows.id AND a.definition_locked_at IS NOT NULL)
        RETURNING id, source, project_id, name, description, graph_json,
                  revision, external_enabled, main_agent_config_json, main_agent_prompt, created_at, updated_at
        "#,
    )
    .bind(request.name.unwrap_or(existing.name))
    .bind(request.description.or(existing.description))
    .bind(graph_json)
    .bind(main_agent_config_json)
    .bind(request.main_agent_prompt)
    .bind(workflow_id)
    .bind(request.expected_revision)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(ApiError::from)?;

    let updated = match row {
        Some(row) => workflow_template_from_row(&row).map_err(ApiError::from)?,
        None => {
            let locked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_attempts WHERE workflow_id=? AND definition_locked_at IS NOT NULL)")
                .bind(workflow_id).fetch_one(&mut *transaction).await.map_err(ApiError::from)?;
            if locked {
                return Err(ApiError::Conflict("INSTANCE_DEFINITION_LOCKED: This workflow instance definition is permanently locked after acceptance; edit the publication for future instances".into()).into());
            }
            let current_revision =
                sqlx::query_scalar::<_, i64>("SELECT revision FROM workflows WHERE id = ?")
                    .bind(workflow_id)
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(ApiError::from)?
                    .ok_or_else(|| ApiError::BadRequest("Workflow not found".to_string()))?;
            return Err(WorkflowUpdateError::RevisionConflict(
                WorkflowRevisionConflict {
                    workflow_id,
                    expected_revision: request.expected_revision,
                    current_revision,
                },
            ));
        }
    };

    for (session_id, workspace_id, name, agent_working_dir) in pending_sessions {
        sqlx::query(
            r#"
            INSERT INTO sessions (id, workspace_id, name, executor, agent_working_dir)
            VALUES (?, ?, ?, NULL, ?)
            "#,
        )
        .bind(session_id)
        .bind(workspace_id)
        .bind(name)
        .bind(agent_working_dir)
        .execute(&mut *transaction)
        .await
        .map_err(ApiError::from)?;
    }
    transaction.commit().await.map_err(ApiError::from)?;

    if let Some(workflow_attempt) = workflow_attempt_by_workflow_id(pool, workflow_id).await? {
        mark_workflow_attempt_ready(pool, workflow_attempt.id).await?;
    }

    Ok(updated)
}

async fn ensure_workflow_definition_editable(
    pool: &SqlitePool,
    workflow_id: Uuid,
) -> Result<(), ApiError> {
    let locked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_attempts WHERE workflow_id=? AND definition_locked_at IS NOT NULL)")
        .bind(workflow_id).fetch_one(pool).await?;
    if locked {
        return Err(ApiError::Conflict("INSTANCE_DEFINITION_LOCKED: This workflow instance definition is permanently locked after acceptance; edit the publication for future instances".into()));
    }
    Ok(())
}

pub async fn delete_workflow_template(
    pool: &SqlitePool,
    workflow_id: Uuid,
) -> Result<(), ApiError> {
    ensure_system_workflows(pool).await?;
    let existing = workflow_by_id(pool, workflow_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Workflow not found".to_string()))?;

    if existing.source == WorkflowSource::System {
        return Err(ApiError::Forbidden(
            "system workflow templates cannot be deleted".to_string(),
        ));
    }

    sqlx::query("DELETE FROM workflows WHERE id = ?")
        .bind(workflow_id)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn fallback_workflows_payload(
    pool: &SqlitePool,
    project_id: Option<Uuid>,
) -> Result<Value, ApiError> {
    let workflows = match project_id {
        Some(project_id) => list_project_workflows(pool, project_id).await?,
        None => list_all_workflows(pool).await?,
    };

    Ok(json!({ "workflows": workflows }))
}

pub async fn fallback_workflow_runs_payload(
    pool: &SqlitePool,
    issue_id: Option<Uuid>,
    workflow_id: Option<Uuid>,
) -> Result<Value, ApiError> {
    let rows = workflow_run_rows(pool, issue_id, workflow_id).await?;
    Ok(json!({ "workflow_runs": rows }))
}

pub async fn fallback_node_executions_payload(
    pool: &SqlitePool,
    run_id: Option<Uuid>,
) -> Result<Value, ApiError> {
    let rows = node_execution_rows(pool, run_id).await?;
    Ok(json!({ "node_executions": rows }))
}

async fn list_all_workflows(pool: &SqlitePool) -> Result<Vec<WorkflowTemplateResponse>, ApiError> {
    ensure_system_workflows(pool).await?;
    let active_system_workflow_ids = built_in_workflow_ids()?;

    let mut query = QueryBuilder::<Sqlite>::new(
        r#"
        SELECT id, source, project_id, name, description, graph_json, revision, external_enabled, main_agent_config_json, main_agent_prompt,
               created_at, updated_at
        FROM workflows
        WHERE (
            (source = 'system' AND id IN (
        "#,
    );
    let mut separated = query.separated(", ");
    for workflow_id in &active_system_workflow_ids {
        separated.push_bind(*workflow_id);
    }
    separated.push_unseparated(
        r#"
            ))
            OR source != 'system'
        )
          AND id NOT IN (SELECT workflow_id FROM workflow_attempts)
        ORDER BY
            CASE source WHEN 'system' THEN 0 ELSE 1 END,
            name ASC,
            created_at ASC
        "#,
    );
    drop(separated);

    let rows = query.build().fetch_all(pool).await?;

    Ok(rows
        .iter()
        .map(workflow_template_from_row)
        .collect::<Result<Vec<_>, _>>()?)
}

async fn workflow_by_id(
    pool: &SqlitePool,
    workflow_id: Uuid,
) -> Result<Option<WorkflowTemplateResponse>, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT id, source, project_id, name, description, graph_json, revision, external_enabled, main_agent_config_json, main_agent_prompt,
               created_at, updated_at
        FROM workflows
        WHERE id = ?
        "#,
    )
    .bind(workflow_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.as_ref().map(workflow_template_from_row).transpose()?)
}

async fn ensure_system_workflows(pool: &SqlitePool) -> Result<(), ApiError> {
    let templates = built_in_templates();
    let mut active_workflow_ids = Vec::with_capacity(templates.len());

    for template in templates {
        validate_graph(&template.graph).map_err(|err| {
            ApiError::BadRequest(format!("Invalid built-in workflow graph: {err}"))
        })?;
        let workflow_id = parse_system_template_id(template.id)?;
        active_workflow_ids.push(workflow_id);
        let graph_json = serde_json::to_string(&template.graph).map_err(|err| {
            ApiError::BadRequest(format!("Invalid built-in workflow graph JSON: {err}"))
        })?;

        sqlx::query(
            r#"
            INSERT INTO workflows (id, source, project_id, name, description, graph_json)
            VALUES (?, 'system', NULL, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                source = 'system',
                project_id = NULL,
                name = excluded.name,
                description = excluded.description,
                graph_json = excluded.graph_json,
                revision = workflows.revision + 1,
                updated_at = datetime('now', 'subsec')
            WHERE
                workflows.source != excluded.source
                OR workflows.project_id IS NOT NULL
                OR workflows.name != excluded.name
                OR COALESCE(workflows.description, '') != COALESCE(excluded.description, '')
                OR workflows.graph_json != excluded.graph_json
            "#,
        )
        .bind(workflow_id)
        .bind(template.name)
        .bind(template.description)
        .bind(graph_json)
        .execute(pool)
        .await?;
    }

    prune_removed_system_workflows(pool, &active_workflow_ids).await?;

    Ok(())
}

pub(crate) fn built_in_workflow_ids() -> Result<Vec<Uuid>, ApiError> {
    built_in_templates()
        .iter()
        .map(|template| parse_system_template_id(template.id))
        .collect()
}

fn parse_system_template_id(template_id: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(template_id).map_err(|err| {
        ApiError::BadRequest(format!(
            "Invalid built-in workflow id `{template_id}`: {err}",
        ))
    })
}

async fn prune_removed_system_workflows(
    pool: &SqlitePool,
    active_workflow_ids: &[Uuid],
) -> Result<(), ApiError> {
    if active_workflow_ids.is_empty() {
        return Ok(());
    }

    let mut query = QueryBuilder::<Sqlite>::new(
        r#"
        DELETE FROM workflows
        WHERE source = 'system'
          AND id NOT IN (
        "#,
    );
    let mut separated = query.separated(", ");
    for workflow_id in active_workflow_ids {
        separated.push_bind(*workflow_id);
    }
    separated.push_unseparated(
        r#"
          )
          AND id NOT IN (SELECT workflow_id FROM workflow_attempts)
          AND id NOT IN (SELECT workflow_id FROM workflow_runs)
        "#,
    );
    drop(separated);

    query.build().execute(pool).await?;
    Ok(())
}

fn parse_graph_json(graph_json: &str) -> Result<WorkflowGraph, WorkflowRouteError> {
    let graph: WorkflowGraph = serde_json::from_str(graph_json).map_err(|err| {
        WorkflowRouteError::BadRequest(format!("Invalid workflow graph JSON: {err}"))
    })?;
    if graph.version != 1 && graph.version != 2 {
        return Err(WorkflowRouteError::BadRequest(format!(
            "Invalid workflow graph: unsupported workflow graph version {}",
            graph.version
        )));
    }
    Ok(graph)
}

fn validate_graph_json(graph_json: &str) -> Result<(), WorkflowRouteError> {
    let graph = parse_graph_json(graph_json)?;
    validate_graph(&graph)
        .map_err(|err| WorkflowRouteError::BadRequest(format!("Invalid workflow graph: {err}")))?;
    Ok(())
}

async fn ensure_project_exists(pool: &SqlitePool, project_id: Uuid) -> Result<(), ApiError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE id = ?")
        .bind(project_id)
        .fetch_one(pool)
        .await?;
    if count == 0 {
        return Err(ApiError::BadRequest("Project not found".to_string()));
    }

    Ok(())
}

async fn ensure_issue_belongs_to_project(
    pool: &SqlitePool,
    project_id: Uuid,
    issue_id: Uuid,
) -> Result<(), ApiError> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM local_issues WHERE id = ? AND project_id = ?")
            .bind(issue_id)
            .bind(project_id)
            .fetch_one(pool)
            .await?;

    if count == 0 {
        return Err(ApiError::BadRequest(
            "Task not found for project".to_string(),
        ));
    }

    Ok(())
}

fn workflow_template_from_row(
    row: &SqliteRow,
) -> Result<WorkflowTemplateResponse, WorkflowRouteError> {
    Ok(WorkflowTemplateResponse {
        id: row.try_get("id")?,
        source: workflow_source_from_str(&row.try_get::<String, _>("source")?)?,
        project_id: row.try_get("project_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        graph_json: row.try_get("graph_json")?,
        revision: row.try_get("revision")?,
        external_enabled: row.try_get("external_enabled")?,
        main_agent_config: row
            .try_get::<Option<String>, _>("main_agent_config_json")?
            .map(|value| {
                serde_json::from_str(&value).map_err(|error| {
                    WorkflowRouteError::BadRequest(format!(
                        "Invalid main Agent configuration: {error}"
                    ))
                })
            })
            .transpose()?,
        main_agent_prompt: row.try_get("main_agent_prompt")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn workflow_attempt_from_row(
    row: &SqliteRow,
) -> Result<WorkflowAttemptResponse, WorkflowRouteError> {
    Ok(WorkflowAttemptResponse {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        issue_id: row.try_get("issue_id")?,
        workflow_id: row.try_get("workflow_id")?,
        template_id: row.try_get("template_id")?,
        latest_run_id: row.try_get("latest_run_id")?,
        workspace_id: row.try_get("workspace_id")?,
        name: row.try_get("name")?,
        status: workflow_attempt_status_from_str(&row.try_get::<String, _>("status")?)?,
        main_session_id: row.try_get("main_session_id")?,
        main_session_bound_at: row.try_get("main_session_bound_at")?,
        definition_locked_at: row.try_get("definition_locked_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn workflow_run_from_row(row: &SqliteRow) -> Result<WorkflowRunFallbackRow, WorkflowRouteError> {
    Ok(WorkflowRunFallbackRow {
        id: row.try_get("id")?,
        orchestration_run_id: row.try_get("orchestration_run_id")?,
        workflow_id: row.try_get("workflow_id")?,
        attempt_id: row.try_get("attempt_id")?,
        issue_id: row.try_get("issue_id")?,
        workspace_id: row.try_get("workspace_id")?,
        trigger_source: row.try_get("trigger_source")?,
        input_text: row.try_get("input_text")?,
        output_text: row.try_get("output_text")?,
        status: workflow_run_status_from_str(&row.try_get::<String, _>("status")?)?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        error_text: row.try_get("error_text")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn node_execution_from_row(
    row: &SqliteRow,
) -> Result<NodeExecutionFallbackRow, WorkflowRouteError> {
    Ok(NodeExecutionFallbackRow {
        id: row.try_get("id")?,
        run_id: row.try_get("run_id")?,
        node_id: row.try_get("node_id")?,
        node_type: row.try_get("node_type")?,
        iteration: row.try_get("iteration")?,
        status: node_execution_status_from_str(&row.try_get::<String, _>("status")?)?,
        input_text: row.try_get("input_text")?,
        output_text: row.try_get("output_text")?,
        session_id: row.try_get("session_id")?,
        orchestration_node_execution_id: row.try_get("orchestration_node_execution_id")?,
        agent_run_id: row.try_get("agent_run_id")?,
        projection_status: row.try_get("agent_projection_status")?,
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

async fn workflow_run_rows(
    pool: &SqlitePool,
    issue_id: Option<Uuid>,
    workflow_id: Option<Uuid>,
) -> Result<Vec<WorkflowRunFallbackRow>, ApiError> {
    let select = r#"
        SELECT id, orchestration_run_id, workflow_id, attempt_id, issue_id, workspace_id, trigger_source, input_text,
               output_text, status, started_at, finished_at, error_text, created_at, updated_at
        FROM workflow_runs
    "#;

    let rows = match (issue_id, workflow_id) {
        (Some(issue_id), Some(workflow_id)) => {
            sqlx::query(&format!(
                "{select} WHERE issue_id = ? AND workflow_id = ? ORDER BY created_at ASC"
            ))
            .bind(issue_id)
            .bind(workflow_id)
            .fetch_all(pool)
            .await?
        }
        (Some(issue_id), None) => {
            sqlx::query(&format!(
                "{select} WHERE issue_id = ? ORDER BY created_at ASC"
            ))
            .bind(issue_id)
            .fetch_all(pool)
            .await?
        }
        (None, Some(workflow_id)) => {
            sqlx::query(&format!(
                "{select} WHERE workflow_id = ? ORDER BY created_at ASC"
            ))
            .bind(workflow_id)
            .fetch_all(pool)
            .await?
        }
        (None, None) => {
            sqlx::query(&format!("{select} ORDER BY created_at ASC"))
                .fetch_all(pool)
                .await?
        }
    };

    Ok(rows
        .iter()
        .map(workflow_run_from_row)
        .collect::<Result<Vec<_>, _>>()?)
}

async fn node_execution_rows(
    pool: &SqlitePool,
    run_id: Option<Uuid>,
) -> Result<Vec<NodeExecutionFallbackRow>, ApiError> {
    let select = r#"
        SELECT ne.id, ne.run_id, ne.node_id, ne.node_type, ne.iteration, ne.status,
               ne.input_text, ne.output_text, ne.session_id,
               ne.orchestration_node_execution_id, ne.agent_run_id,
               ars.projection_status AS agent_projection_status,
               ne.execution_process_id, ne.arena_group_id, ne.tokens_used, ne.cost_estimate,
               ne.started_at, ne.finished_at, ne.error_text, ne.created_at, ne.updated_at
        FROM node_executions ne
        LEFT JOIN agent_run_state ars ON ars.agent_run_id = ne.agent_run_id
    "#;

    let rows = match run_id {
        Some(run_id) => {
            sqlx::query(&format!(
                "{select} WHERE ne.run_id = ? ORDER BY ne.iteration ASC, ne.created_at ASC"
            ))
            .bind(run_id)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query(&format!("{select} ORDER BY ne.created_at ASC"))
                .fetch_all(pool)
                .await?
        }
    };

    Ok(rows
        .iter()
        .map(node_execution_from_row)
        .collect::<Result<Vec<_>, _>>()?)
}

fn workflow_source_from_str(value: &str) -> Result<WorkflowSource, WorkflowRouteError> {
    match value {
        "system" => Ok(WorkflowSource::System),
        "project" => Ok(WorkflowSource::Project),
        other => Err(WorkflowRouteError::BadRequest(format!(
            "Unknown workflow source `{other}`"
        ))),
    }
}

fn workflow_run_status_from_str(value: &str) -> Result<WorkflowRunStatus, WorkflowRouteError> {
    match value {
        "pending" => Ok(WorkflowRunStatus::Pending),
        "running" => Ok(WorkflowRunStatus::Running),
        "awaiting_human" => Ok(WorkflowRunStatus::AwaitingHuman),
        "awaiting_arena" => Ok(WorkflowRunStatus::AwaitingArena),
        "cancelling" => Ok(WorkflowRunStatus::Cancelling),
        "succeeded" => Ok(WorkflowRunStatus::Succeeded),
        "failed" => Ok(WorkflowRunStatus::Failed),
        "canceled" => Ok(WorkflowRunStatus::Canceled),
        other => Err(WorkflowRouteError::BadRequest(format!(
            "Unknown workflow run status `{other}`"
        ))),
    }
}

fn workflow_attempt_status_from_str(
    value: &str,
) -> Result<WorkflowAttemptStatus, WorkflowRouteError> {
    match value {
        "draft" => Ok(WorkflowAttemptStatus::Draft),
        "ready" => Ok(WorkflowAttemptStatus::Ready),
        "running" => Ok(WorkflowAttemptStatus::Running),
        "awaiting_human" => Ok(WorkflowAttemptStatus::AwaitingHuman),
        "awaiting_arena" => Ok(WorkflowAttemptStatus::AwaitingArena),
        "cancelling" => Ok(WorkflowAttemptStatus::Cancelling),
        "succeeded" => Ok(WorkflowAttemptStatus::Succeeded),
        "failed" => Ok(WorkflowAttemptStatus::Failed),
        "canceled" => Ok(WorkflowAttemptStatus::Canceled),
        other => Err(WorkflowRouteError::BadRequest(format!(
            "Unknown workflow attempt status `{other}`"
        ))),
    }
}

fn workflow_attempt_status_value(status: WorkflowAttemptStatus) -> &'static str {
    match status {
        WorkflowAttemptStatus::Draft => "draft",
        WorkflowAttemptStatus::Ready => "ready",
        WorkflowAttemptStatus::Running => "running",
        WorkflowAttemptStatus::AwaitingHuman => "awaiting_human",
        WorkflowAttemptStatus::AwaitingArena => "awaiting_arena",
        WorkflowAttemptStatus::Cancelling => "cancelling",
        WorkflowAttemptStatus::Succeeded => "succeeded",
        WorkflowAttemptStatus::Failed => "failed",
        WorkflowAttemptStatus::Canceled => "canceled",
    }
}

fn workflow_attempt_status_from_run(status: WorkflowRunStatus) -> WorkflowAttemptStatus {
    match status {
        WorkflowRunStatus::Pending => WorkflowAttemptStatus::Ready,
        WorkflowRunStatus::Running => WorkflowAttemptStatus::Running,
        WorkflowRunStatus::AwaitingHuman => WorkflowAttemptStatus::AwaitingHuman,
        WorkflowRunStatus::AwaitingArena => WorkflowAttemptStatus::AwaitingArena,
        WorkflowRunStatus::Cancelling => WorkflowAttemptStatus::Cancelling,
        WorkflowRunStatus::Succeeded => WorkflowAttemptStatus::Succeeded,
        WorkflowRunStatus::Failed => WorkflowAttemptStatus::Failed,
        WorkflowRunStatus::Canceled => WorkflowAttemptStatus::Canceled,
    }
}

fn node_execution_status_from_str(value: &str) -> Result<NodeExecutionStatus, WorkflowRouteError> {
    match value {
        "pending" => Ok(NodeExecutionStatus::Pending),
        "running" => Ok(NodeExecutionStatus::Running),
        "awaiting_human" => Ok(NodeExecutionStatus::AwaitingHuman),
        "awaiting_arena" => Ok(NodeExecutionStatus::AwaitingArena),
        "cancelling" => Ok(NodeExecutionStatus::Cancelling),
        "succeeded" => Ok(NodeExecutionStatus::Succeeded),
        "failed" => Ok(NodeExecutionStatus::Failed),
        "cancelled" => Ok(NodeExecutionStatus::Cancelled),
        "skipped" => Ok(NodeExecutionStatus::Skipped),
        other => Err(WorkflowRouteError::BadRequest(format!(
            "Unknown node execution status `{other}`"
        ))),
    }
}

pub fn build_workflow_run_runtime_view(
    run_id: Uuid,
    status: WorkflowRunStatus,
    nodes: &[WorkflowNodeExecutionResponse],
    now: DateTime<Utc>,
    active_slow_threshold_ms: i32,
) -> WorkflowRunRuntimeView {
    let mut ordered_node_ids = Vec::new();
    let mut grouped_nodes: HashMap<&str, Vec<&WorkflowNodeExecutionResponse>> = HashMap::new();

    for node in nodes {
        if !grouped_nodes.contains_key(node.node_id.as_str()) {
            ordered_node_ids.push(node.node_id.as_str());
        }
        grouped_nodes
            .entry(node.node_id.as_str())
            .or_default()
            .push(node);
    }

    let node_work = ordered_node_ids
        .into_iter()
        .filter_map(|node_id| {
            grouped_nodes.get(node_id).and_then(|executions| {
                let current = executions
                    .iter()
                    .copied()
                    .max_by(|left, right| compare_node_execution_order(left, right))?;
                Some(build_workflow_node_work_view(
                    current,
                    executions,
                    now,
                    active_slow_threshold_ms,
                ))
            })
        })
        .collect::<Vec<_>>();

    let active_node_count = node_work
        .iter()
        .filter(|work| {
            matches!(
                work.status,
                WorkflowNodeWorkStatus::Starting | WorkflowNodeWorkStatus::Running
            )
        })
        .count() as i32;
    let pending_node_count = node_work
        .iter()
        .filter(|work| work.status == WorkflowNodeWorkStatus::Pending)
        .count() as i32;
    let waiting_node_count = node_work
        .iter()
        .filter(|work| {
            matches!(
                work.status,
                WorkflowNodeWorkStatus::AwaitingHuman | WorkflowNodeWorkStatus::AwaitingArena
            )
        })
        .count() as i32;
    let failed_node_count = node_work
        .iter()
        .filter(|work| work.status == WorkflowNodeWorkStatus::Failed)
        .count() as i32;
    let completed_node_count = node_work
        .iter()
        .filter(|work| work.status == WorkflowNodeWorkStatus::Succeeded)
        .count() as i32;

    WorkflowRunRuntimeView {
        run_id,
        status,
        active_node_count,
        pending_node_count,
        waiting_node_count,
        failed_node_count,
        completed_node_count,
        reused_node_count: 0,
        skipped_node_count: node_work
            .iter()
            .filter(|work| work.status == WorkflowNodeWorkStatus::Skipped)
            .count() as i32,
        node_work,
    }
}

fn compare_node_execution_order(
    left: &WorkflowNodeExecutionResponse,
    right: &WorkflowNodeExecutionResponse,
) -> std::cmp::Ordering {
    left.iteration
        .cmp(&right.iteration)
        .then_with(|| left.updated_at.cmp(&right.updated_at))
}

fn build_workflow_node_work_view(
    current: &WorkflowNodeExecutionResponse,
    executions: &[&WorkflowNodeExecutionResponse],
    now: DateTime<Utc>,
    active_slow_threshold_ms: i32,
) -> WorkflowNodeWorkView {
    let status = workflow_node_work_status(current);
    let is_active = matches!(
        status,
        WorkflowNodeWorkStatus::Starting | WorkflowNodeWorkStatus::Running
    );
    let is_waiting = matches!(
        status,
        WorkflowNodeWorkStatus::AwaitingHuman | WorkflowNodeWorkStatus::AwaitingArena
    );
    let active_elapsed_ms = if is_active {
        current.started_at.map(|started_at| {
            now.signed_duration_since(started_at)
                .num_milliseconds()
                .max(0)
                .min(i64::from(i32::MAX)) as i32
        })
    } else {
        None
    };
    let active_slow = active_elapsed_ms
        .map(|elapsed_ms| elapsed_ms >= active_slow_threshold_ms)
        .unwrap_or(false);
    let runtime_health = workflow_runtime_health(current, status, active_slow);

    WorkflowNodeWorkView {
        node_id: current.node_id.clone(),
        node_type: current.node_type.clone(),
        iteration: current.iteration,
        status,
        pending_work_count: executions
            .iter()
            .filter(|node| node.status == NodeExecutionStatus::Pending)
            .count() as i32,
        starting_child_count: executions
            .iter()
            .filter(|node| {
                node.status == NodeExecutionStatus::Running
                    && matches!(node.node_type.as_str(), "agent" | "condition")
                    && node.agent_run_id.is_none()
            })
            .count() as i32,
        active_execution_id: if is_active || is_waiting {
            Some(current.id)
        } else {
            None
        },
        active_session_id: current.session_id,
        orchestration_node_execution_id: current.orchestration_node_execution_id,
        active_agent_run_id: current.agent_run_id,
        projection_status: current.projection_status,
        active_started_at: if is_active || is_waiting {
            current.started_at
        } else {
            None
        },
        active_elapsed_ms,
        active_slow,
        active_slow_threshold_ms,
        runtime_health,
        can_open_session: current.session_id.is_some()
            && matches!(current.node_type.as_str(), "agent" | "condition"),
        can_retry: current.status == NodeExecutionStatus::Failed
            && matches!(
                current.node_type.as_str(),
                "agent" | "condition" | "transform"
            ),
        can_approve: current.status == NodeExecutionStatus::AwaitingHuman
            && current.node_type == "human_gate",
        can_reject: current.status == NodeExecutionStatus::AwaitingHuman
            && current.node_type == "human_gate",
        can_select_arena_winner: current.status == NodeExecutionStatus::AwaitingArena
            && current.node_type == "arena",
        can_select_condition_branch: current.status == NodeExecutionStatus::AwaitingHuman
            && current.node_type == "condition",
        can_cancel_node: false,
        reused_results: Vec::new(),
    }
}

fn workflow_node_work_status(node: &WorkflowNodeExecutionResponse) -> WorkflowNodeWorkStatus {
    match node.status {
        NodeExecutionStatus::Pending => WorkflowNodeWorkStatus::Pending,
        NodeExecutionStatus::Running
            if matches!(node.node_type.as_str(), "agent" | "condition")
                && node.agent_run_id.is_none() =>
        {
            WorkflowNodeWorkStatus::Starting
        }
        NodeExecutionStatus::Running => WorkflowNodeWorkStatus::Running,
        NodeExecutionStatus::AwaitingHuman => WorkflowNodeWorkStatus::AwaitingHuman,
        NodeExecutionStatus::AwaitingArena => WorkflowNodeWorkStatus::AwaitingArena,
        NodeExecutionStatus::Cancelling => WorkflowNodeWorkStatus::Cancelling,
        NodeExecutionStatus::Succeeded => WorkflowNodeWorkStatus::Succeeded,
        NodeExecutionStatus::Failed => WorkflowNodeWorkStatus::Failed,
        NodeExecutionStatus::Cancelled => WorkflowNodeWorkStatus::Cancelled,
        NodeExecutionStatus::Skipped => WorkflowNodeWorkStatus::Skipped,
    }
}

fn workflow_runtime_health(
    node: &WorkflowNodeExecutionResponse,
    status: WorkflowNodeWorkStatus,
    active_slow: bool,
) -> WorkflowRuntimeHealth {
    if matches!(
        node.projection_status,
        Some(ProjectionStatus::ProjectionDegraded | ProjectionStatus::Rebuilding)
    ) {
        return WorkflowRuntimeHealth::ProjectionDegraded;
    }
    match status {
        WorkflowNodeWorkStatus::Starting if active_slow => WorkflowRuntimeHealth::Unknown,
        WorkflowNodeWorkStatus::Starting => WorkflowRuntimeHealth::Starting,
        WorkflowNodeWorkStatus::Running if active_slow => WorkflowRuntimeHealth::Slow,
        WorkflowNodeWorkStatus::Running
        | WorkflowNodeWorkStatus::AwaitingHuman
        | WorkflowNodeWorkStatus::AwaitingArena
        | WorkflowNodeWorkStatus::Cancelling
        | WorkflowNodeWorkStatus::Succeeded
        | WorkflowNodeWorkStatus::Failed
        | WorkflowNodeWorkStatus::Cancelled
        | WorkflowNodeWorkStatus::Reused
        | WorkflowNodeWorkStatus::Skipped => WorkflowRuntimeHealth::Ok,
        WorkflowNodeWorkStatus::Pending if node.started_at.is_none() => {
            WorkflowRuntimeHealth::Unknown
        }
        WorkflowNodeWorkStatus::Pending => WorkflowRuntimeHealth::Ok,
    }
}

fn txid() -> i64 {
    Utc::now().timestamp_millis()
}

async fn trigger_workflow(
    State(deployment): State<DeploymentImpl>,
    Path(workflow_id): Path<Uuid>,
    Json(request): Json<TriggerWorkflowRequest>,
) -> Result<
    (
        StatusCode,
        ResponseJson<MutationResponse<WorkflowRunResponse>>,
    ),
    WorkflowManagementApiError,
> {
    let workspace_resolver = DeploymentWorkflowWorkspaceResolver::new(deployment.clone());
    let data = accept_workflow_template_for_issue(
        &deployment.db().pool,
        workflow_id,
        request,
        &workspace_resolver,
    )
    .await?;

    Ok((
        StatusCode::ACCEPTED,
        ResponseJson(MutationResponse { data, txid: txid() }),
    ))
}

pub async fn workflow_file_summary(
    pool: &SqlitePool,
    run_id: Uuid,
) -> Result<WorkflowFileChangeSummary, ApiError> {
    let workspace_id: Option<Uuid> =
        sqlx::query_scalar("SELECT workspace_id FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| ApiError::BadRequest("Workflow run not found".into()))?;
    let root = async {
        let workspace_id =
            workspace_id.ok_or_else(|| ApiError::BadRequest("Workspace missing".into()))?;
        let workspace = db::models::workspace::Workspace::find_by_id(pool, workspace_id)
            .await?
            .ok_or_else(|| ApiError::BadRequest("Workspace missing".into()))?;
        let mut root = std::path::PathBuf::from(
            workspace
                .container_ref
                .ok_or_else(|| ApiError::BadRequest("Workspace directory missing".into()))?,
        );
        if let Some(relative) = Session::resolve_agent_working_dir(pool, workspace_id).await? {
            root.push(relative);
        }
        tokio::fs::canonicalize(root)
            .await
            .map_err(|_| ApiError::BadRequest("Workspace directory unavailable".into()))
    }
    .await;
    let reason = match root {
        Ok(root) => match WorkflowFileChanges::project(pool, run_id, &root).await {
            Ok(summary) => return Ok(summary),
            Err(error) => {
                tracing::warn!(%run_id,%error,"Workflow file collection unavailable");
                "file_collection_failed"
            }
        },
        Err(_) => "project_directory_unavailable",
    };
    Ok(WorkflowFileChanges::unavailable(pool, run_id, reason)
        .await
        .unwrap_or_else(|_| WorkflowFileChangeSummary {
            files: Vec::new(),
            collection_status: WorkflowFileCollectionStatus::Unavailable,
            reasons: vec![reason.to_string()],
        }))
}

async fn get_workflow_file_changes(
    State(deployment): State<DeploymentImpl>,
    Path(run_id): Path<Uuid>,
) -> Result<ResponseJson<WorkflowFileChangeSummary>, ApiError> {
    Ok(ResponseJson(
        workflow_file_summary(&deployment.db().pool, run_id).await?,
    ))
}

async fn get_workflow_run(
    State(deployment): State<DeploymentImpl>,
    Path(run_id): Path<Uuid>,
) -> Result<ResponseJson<WorkflowRunResponse>, ApiError> {
    let agent_executor = DeploymentWorkflowAgentExecutor::new(deployment.clone());
    let arena_creator = DeploymentWorkflowArenaCreator::new(deployment.clone());
    let boundary = DeploymentAgentRunReconciliationBoundary::new(deployment.clone());
    let run = reconcile_workflow_run_with_arena_and_boundary(
        &deployment.db().pool,
        run_id,
        &agent_executor,
        &arena_creator,
        &boundary,
    )
    .await?;
    sync_attempt_from_run(&deployment.db().pool, &run).await?;
    Ok(ResponseJson(run))
}

async fn cancel_workflow_run(
    State(deployment): State<DeploymentImpl>,
    Path(run_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<ResponseJson<MutationResponse<WorkflowActionResponse>>, WorkflowManagementApiError> {
    let pool = &deployment.db().pool;
    let caller = page_run_caller(pool, run_id).await?;
    let key = workflow_operation_key(&headers, format!("page-stop:{run_id}"))?;
    let canceller = DeploymentWorkflowRunCanceller::new(deployment.clone());
    management::stop_workflow(pool, &caller, run_id, &key, &canceller).await?;
    let run = get_workflow_run_response(pool, run_id).await?;

    Ok(ResponseJson(MutationResponse {
        data: workflow_action_response(&run, None),
        txid: txid(),
    }))
}

async fn workflow_run_events(
    State(deployment): State<DeploymentImpl>,
    Path(run_id): Path<Uuid>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, BoxError>>>, ApiError> {
    let agent_executor = DeploymentWorkflowAgentExecutor::new(deployment.clone());
    let arena_creator = DeploymentWorkflowArenaCreator::new(deployment.clone());
    let boundary = DeploymentAgentRunReconciliationBoundary::new(deployment.clone());
    let run = reconcile_workflow_run_with_arena_and_boundary(
        &deployment.db().pool,
        run_id,
        &agent_executor,
        &arena_creator,
        &boundary,
    )
    .await?;
    sync_attempt_from_run(&deployment.db().pool, &run).await?;

    let run_id_string = run_id.to_string();
    let receiver = subscribe_workflow_events();
    let history_events = workflow_event_history(run_id);
    let last_history_sequence = history_events
        .iter()
        .map(|event| event.sequence)
        .max()
        .unwrap_or_default();
    let history = stream::iter(
        history_events
            .into_iter()
            .map(|event| Ok::<Event, BoxError>(workflow_event_to_sse_event(event))),
    );
    let live = stream::unfold(
        (run_id_string, receiver, last_history_sequence),
        |(run_id_string, mut receiver, last_history_sequence)| async move {
            loop {
                match receiver.recv().await {
                    Ok(event)
                        if event.run_id == run_id_string
                            && event.sequence > last_history_sequence =>
                    {
                        return Some((
                            Ok::<Event, BoxError>(workflow_event_to_sse_event(event)),
                            (run_id_string, receiver, last_history_sequence),
                        ));
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        },
    );

    Ok(Sse::new(history.chain(live)).keep_alive(KeepAlive::default()))
}

async fn retry_node(
    State(deployment): State<DeploymentImpl>,
    Path((run_id, node_id)): Path<(Uuid, String)>,
) -> Result<ResponseJson<MutationResponse<WorkflowActionResponse>>, ApiError> {
    let agent_executor = DeploymentWorkflowAgentExecutor::new(deployment.clone());
    let arena_creator = DeploymentWorkflowArenaCreator::new(deployment.clone());
    let run = retry_workflow_node_with_arena(
        &deployment.db().pool,
        run_id,
        &node_id,
        &agent_executor,
        &arena_creator,
    )
    .await?;
    sync_attempt_from_run(&deployment.db().pool, &run).await?;

    Ok(ResponseJson(MutationResponse {
        data: workflow_action_response(&run, Some(node_id)),
        txid: txid(),
    }))
}

async fn approve_node(
    State(deployment): State<DeploymentImpl>,
    Path((run_id, node_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
    Json(request): Json<RespondWorkflowNodeRequest>,
) -> Result<ResponseJson<MutationResponse<WorkflowActionResponse>>, WorkflowManagementApiError> {
    let run = respond_page_node(
        &deployment,
        run_id,
        &node_id,
        request.node_execution_id,
        &headers,
        WorkflowInteractionResponse::Approve,
    )
    .await?;

    Ok(ResponseJson(MutationResponse {
        data: workflow_action_response(&run, Some(node_id)),
        txid: txid(),
    }))
}

async fn reject_node(
    State(deployment): State<DeploymentImpl>,
    Path((run_id, node_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
    Json(request): Json<RespondWorkflowNodeRequest>,
) -> Result<ResponseJson<MutationResponse<WorkflowActionResponse>>, WorkflowManagementApiError> {
    let run = respond_page_node(
        &deployment,
        run_id,
        &node_id,
        request.node_execution_id,
        &headers,
        WorkflowInteractionResponse::Reject,
    )
    .await?;

    Ok(ResponseJson(MutationResponse {
        data: workflow_action_response(&run, Some(node_id)),
        txid: txid(),
    }))
}

async fn select_arena_winner(
    State(deployment): State<DeploymentImpl>,
    Path((run_id, node_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
    Json(request): Json<SelectArenaWinnerRequest>,
) -> Result<ResponseJson<MutationResponse<WorkflowActionResponse>>, WorkflowManagementApiError> {
    let run = respond_page_node(
        &deployment,
        run_id,
        &node_id,
        request.node_execution_id,
        &headers,
        WorkflowInteractionResponse::SelectArenaWinner {
            candidate_id: request.candidate_id,
        },
    )
    .await?;

    Ok(ResponseJson(MutationResponse {
        data: workflow_action_response(&run, Some(node_id)),
        txid: txid(),
    }))
}

async fn select_condition_branch(
    State(deployment): State<DeploymentImpl>,
    Path((run_id, node_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
    Json(request): Json<SelectConditionBranchRequest>,
) -> Result<ResponseJson<MutationResponse<WorkflowActionResponse>>, WorkflowManagementApiError> {
    let run = respond_page_node(
        &deployment,
        run_id,
        &node_id,
        request.node_execution_id,
        &headers,
        WorkflowInteractionResponse::SelectBranch {
            selected_target_node_ids: request.selected_target_node_ids,
            reason: request.reason,
        },
    )
    .await?;

    Ok(ResponseJson(MutationResponse {
        data: workflow_action_response(&run, Some(node_id)),
        txid: txid(),
    }))
}

fn workflow_action_response(
    run: &WorkflowRunResponse,
    node_id: Option<String>,
) -> WorkflowActionResponse {
    WorkflowActionResponse {
        run_id: run.id,
        node_id,
        status: run.status,
    }
}

pub(crate) fn workflow_operation_key(
    headers: &HeaderMap,
    fallback: String,
) -> Result<String, ApiError> {
    if headers.contains_key("Idempotency-Key") {
        crate::routes::integrations::request_key(headers)
    } else {
        Ok(fallback)
    }
}

async fn page_run_caller(
    pool: &SqlitePool,
    run_id: Uuid,
) -> Result<WorkflowManagementCaller, ApiError> {
    let instance_id: Option<Uuid> =
        sqlx::query_scalar("SELECT attempt_id FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_optional(pool)
            .await?
            .flatten();
    let instance_id = instance_id
        .ok_or_else(|| ApiError::Conflict("Workflow Run has no bound instance".into()))?;
    Ok(WorkflowManagementCaller::Instance {
        instance_id,
        namespace: format!("page-instance:{instance_id}"),
        integration_id: None,
    })
}

async fn respond_page_node(
    deployment: &DeploymentImpl,
    run_id: Uuid,
    node_id: &str,
    node_execution_id: Uuid,
    headers: &HeaderMap,
    response: WorkflowInteractionResponse,
) -> Result<WorkflowRunResponse, ApiError> {
    let pool = &deployment.db().pool;
    let belongs: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM node_executions WHERE id=? AND run_id=? AND node_id=?)",
    )
    .bind(node_execution_id)
    .bind(run_id)
    .bind(node_id)
    .fetch_one(pool)
    .await?;
    if !belongs {
        return Err(ApiError::Conflict(
            "Workflow interaction does not match this exact Run/Node".into(),
        ));
    }
    let caller = page_run_caller(pool, run_id).await?;
    let request_id = workflow_operation_key(headers, format!("page-response:{node_execution_id}"))?;
    management::respond_to_workflow(
        pool,
        &caller,
        WorkflowManagementInteractionRequest {
            run_id,
            node_execution_id,
            request_id,
            response,
        },
        &DeploymentWorkflowAgentExecutor::new(deployment.clone()),
        &DeploymentWorkflowArenaCreator::new(deployment.clone()),
        &DeploymentWorkflowArenaWinnerApplier::new(deployment.clone()),
    )
    .await
}

fn workflow_event_to_sse_event(event: workflow::WorkflowEvent) -> Event {
    let event_name = serde_json::to_string(&event.kind)
        .ok()
        .and_then(|value| serde_json::from_str::<String>(&value).ok())
        .unwrap_or_else(|| "workflow_event".to_string());
    let id = event.sequence.to_string();
    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());

    Event::default().id(id).event(event_name).data(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_directory_selection_rejects_empty_or_mixed_targets() {
        assert_eq!(
            workflow_workspace_directory_override(Some(" F:/notes "), &[]).unwrap(),
            Some("F:/notes".to_string())
        );
        assert!(workflow_workspace_directory_override(Some(" "), &[]).is_err());
        assert!(
            workflow_workspace_directory_override(
                Some("F:/notes"),
                &[CreateWorkspaceRepo {
                    repo_id: Uuid::new_v4(),
                    target_branch: "main".to_string()
                }]
            )
            .is_err()
        );
        assert_eq!(
            workflow_workspace_directory_override(None, &[]).unwrap(),
            None
        );
    }

    fn ts(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn node(
        id: u128,
        node_id: &str,
        node_type: &str,
        status: NodeExecutionStatus,
        iteration: i64,
        updated_at: &str,
    ) -> WorkflowNodeExecutionResponse {
        WorkflowNodeExecutionResponse {
            id: Uuid::from_u128(id),
            run_id: Uuid::from_u128(100),
            task_id: None,
            node_id: node_id.to_string(),
            node_type: node_type.to_string(),
            iteration,
            status,
            input_text: None,
            output_text: None,
            session_id: None,
            orchestration_node_execution_id: None,
            agent_run_id: None,
            projection_status: None,
            execution_process_id: None,
            arena_group_id: None,
            tokens_used: None,
            cost_estimate: None,
            started_at: None,
            finished_at: None,
            error_text: None,
            created_at: ts("2026-06-25T00:00:00Z"),
            updated_at: ts(updated_at),
        }
    }

    #[test]
    fn runtime_view_marks_running_process_elapsed_and_slow_health() {
        let run_id = Uuid::from_u128(100);
        let mut running = node(
            1,
            "agent-implement",
            "agent",
            NodeExecutionStatus::Running,
            0,
            "2026-06-25T00:05:00Z",
        );
        running.session_id = Some(Uuid::from_u128(2));
        running.orchestration_node_execution_id = Some(Uuid::from_u128(3));
        running.agent_run_id = Some(Uuid::from_u128(4));
        running.projection_status = Some(ProjectionStatus::Current);
        running.started_at = Some(ts("2026-06-25T00:05:00Z"));

        let view = build_workflow_run_runtime_view(
            run_id,
            WorkflowRunStatus::Running,
            &[running],
            ts("2026-06-25T00:10:01Z"),
            300_000,
        );

        assert_eq!(view.active_node_count, 1);
        let work = &view.node_work[0];
        assert_eq!(work.status, WorkflowNodeWorkStatus::Running);
        assert_eq!(work.active_elapsed_ms, Some(301_000));
        assert!(work.active_slow);
        assert_eq!(work.runtime_health, WorkflowRuntimeHealth::Slow);
        assert!(work.can_open_session);
        assert_eq!(work.active_session_id, Some(Uuid::from_u128(2)));
        assert_eq!(
            work.orchestration_node_execution_id,
            Some(Uuid::from_u128(3))
        );
        assert_eq!(work.active_agent_run_id, Some(Uuid::from_u128(4)));
    }

    #[test]
    fn runtime_view_distinguishes_starting_child_from_degraded_projection() {
        let mut starting = node(
            1,
            "agent-starting",
            "agent",
            NodeExecutionStatus::Running,
            0,
            "2026-06-25T00:09:30Z",
        );
        starting.started_at = Some(ts("2026-06-25T00:09:30Z"));
        let mut missing = node(
            2,
            "agent-missing-process",
            "agent",
            NodeExecutionStatus::Running,
            0,
            "2026-06-25T00:00:00Z",
        );
        missing.started_at = Some(ts("2026-06-25T00:00:00Z"));
        missing.agent_run_id = Some(Uuid::from_u128(9));
        missing.projection_status = Some(ProjectionStatus::ProjectionDegraded);

        let view = build_workflow_run_runtime_view(
            Uuid::from_u128(100),
            WorkflowRunStatus::Running,
            &[starting, missing],
            ts("2026-06-25T00:10:00Z"),
            300_000,
        );

        let starting = view
            .node_work
            .iter()
            .find(|work| work.node_id == "agent-starting")
            .unwrap();
        assert_eq!(starting.status, WorkflowNodeWorkStatus::Starting);
        assert_eq!(starting.starting_child_count, 1);
        assert_eq!(starting.runtime_health, WorkflowRuntimeHealth::Starting);
        assert!(!starting.active_slow);

        let missing = view
            .node_work
            .iter()
            .find(|work| work.node_id == "agent-missing-process")
            .unwrap();
        assert_eq!(missing.status, WorkflowNodeWorkStatus::Running);
        assert_eq!(
            missing.runtime_health,
            WorkflowRuntimeHealth::ProjectionDegraded
        );
        assert!(missing.active_slow);
    }

    #[test]
    fn runtime_view_exposes_action_gates_for_waiting_and_failed_work() {
        let mut condition = node(
            2,
            "router",
            "condition",
            NodeExecutionStatus::AwaitingHuman,
            0,
            "2026-06-25T00:00:02Z",
        );
        condition.session_id = Some(Uuid::from_u128(20));
        let nodes = vec![
            node(
                1,
                "approval",
                "human_gate",
                NodeExecutionStatus::AwaitingHuman,
                0,
                "2026-06-25T00:00:01Z",
            ),
            condition,
            node(
                3,
                "arena",
                "arena",
                NodeExecutionStatus::AwaitingArena,
                0,
                "2026-06-25T00:00:03Z",
            ),
            node(
                4,
                "fix",
                "agent",
                NodeExecutionStatus::Failed,
                0,
                "2026-06-25T00:00:04Z",
            ),
        ];

        let view = build_workflow_run_runtime_view(
            Uuid::from_u128(100),
            WorkflowRunStatus::AwaitingHuman,
            &nodes,
            ts("2026-06-25T00:10:00Z"),
            WORKFLOW_NODE_ACTIVE_SLOW_THRESHOLD_MS,
        );

        assert_eq!(view.waiting_node_count, 3);
        assert_eq!(view.failed_node_count, 1);

        let approval = view
            .node_work
            .iter()
            .find(|work| work.node_id == "approval")
            .unwrap();
        assert!(approval.can_approve);
        assert!(approval.can_reject);
        assert!(!approval.can_select_condition_branch);

        let condition = view
            .node_work
            .iter()
            .find(|work| work.node_id == "router")
            .unwrap();
        assert!(condition.can_open_session);
        assert!(condition.can_select_condition_branch);
        assert!(!condition.can_approve);

        let arena = view
            .node_work
            .iter()
            .find(|work| work.node_id == "arena")
            .unwrap();
        assert!(arena.can_select_arena_winner);

        let failed = view
            .node_work
            .iter()
            .find(|work| work.node_id == "fix")
            .unwrap();
        assert!(failed.can_retry);
        assert!(!failed.can_cancel_node);
    }

    #[test]
    fn runtime_view_uses_latest_iteration_as_node_work_state() {
        let nodes = vec![
            node(
                1,
                "fan-in",
                "agent",
                NodeExecutionStatus::Succeeded,
                0,
                "2026-06-25T00:00:01Z",
            ),
            node(
                2,
                "fan-in",
                "agent",
                NodeExecutionStatus::Pending,
                1,
                "2026-06-25T00:00:02Z",
            ),
            node(
                3,
                "end",
                "end",
                NodeExecutionStatus::Skipped,
                0,
                "2026-06-25T00:00:03Z",
            ),
        ];

        let view = build_workflow_run_runtime_view(
            Uuid::from_u128(100),
            WorkflowRunStatus::Running,
            &nodes,
            ts("2026-06-25T00:10:00Z"),
            WORKFLOW_NODE_ACTIVE_SLOW_THRESHOLD_MS,
        );

        let fan_in = view
            .node_work
            .iter()
            .find(|work| work.node_id == "fan-in")
            .unwrap();
        assert_eq!(fan_in.iteration, 1);
        assert_eq!(fan_in.status, WorkflowNodeWorkStatus::Pending);
        assert_eq!(fan_in.pending_work_count, 1);
        assert_eq!(view.pending_node_count, 1);
        assert_eq!(view.completed_node_count, 0);
        assert_eq!(view.skipped_node_count, 1);
        assert_eq!(view.reused_node_count, 0);
    }
}
