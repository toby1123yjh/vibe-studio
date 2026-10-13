//! External workflow calls select prepared templates; arbitrary graphs and host
//! paths are deliberately absent from these request types.
use std::path::{Path as FsPath, PathBuf};

use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use db::models::{
    integration::IntegrationRequest, session::Session, workflow::WorkflowAttemptStatus,
    workflow_file_changes::WorkflowFileChangeSummary, workspace::Workspace,
};
use deployment::Deployment;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;
use workflow::WorkflowGraph;

use super::{IntegrationCaller, authorize_project, project_root, request_hash, request_key};
use crate::{
    DeploymentImpl,
    error::ApiError,
    routes::{
        workflow_management::WorkflowManagementApiError,
        workflows::{self, WorkflowAttemptResponse, WorkflowRunResponse},
    },
    workflow_runtime::{
        arena::{DeploymentWorkflowArenaCreator, DeploymentWorkflowArenaWinnerApplier},
        management::{
            self, WorkflowActivePolicy, WorkflowManagementCaller, WorkflowSubmission,
            WorkflowSubmissionAction, WorkflowSubmissionScope,
        },
        runner::{
            self, DeploymentAgentRunReconciliationBoundary, DeploymentWorkflowAgentExecutor,
            DeploymentWorkflowRunCanceller, WorkflowWorkspaceRequest, WorkflowWorkspaceResolver,
        },
        workspace::DeploymentWorkflowWorkspaceResolver,
    },
};

#[derive(Debug, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CreateExternalWorkflowAttempt {
    pub template_id: Uuid,
    pub name: Option<String>,
}

/// Only selection metadata crosses the integration boundary. Template graphs
/// can contain source Session identities and local executor overrides.
#[derive(Debug, Serialize, TS)]
pub struct ExternalWorkflowTemplate {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    #[ts(type = "number")]
    pub revision: i64,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RunExternalWorkflow {
    pub input_text: String,
    #[serde(default)]
    pub material_paths: Vec<String>,
}

#[derive(Debug, Serialize, TS)]
pub struct ExternalWorkflowRun {
    pub project_id: Uuid,
    pub run: WorkflowRunResponse,
    pub file_changes: WorkflowFileChangeSummary,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WorkflowInteraction {
    pub id: Uuid,
    pub run_id: Uuid,
    pub node_id: String,
    pub iteration: i64,
    pub node_type: String,
    pub output_text: Option<String>,
    pub branch_targets: Vec<String>,
    pub arena_candidate_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowInteractionResponse {
    Approve,
    Reject,
    SelectBranch {
        selected_target_node_ids: Vec<String>,
        reason: Option<String>,
    },
    SelectArenaWinner {
        candidate_id: Uuid,
    },
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/projects/{project_id}/workflows",get(list_templates))
        .route("/projects/{project_id}/tasks/{task_id}/workflow-attempts",post(create_attempt))
        .route("/projects/{project_id}/tasks/{task_id}/workflow-attempts/{attempt_id}/run",post(submit_run))
        .route("/projects/{project_id}/tasks/{task_id}/workflow-runs/{run_id}",get(get_run))
        .route("/projects/{project_id}/tasks/{task_id}/workflow-runs/{run_id}/cancel",post(cancel_run))
        .route("/projects/{project_id}/tasks/{task_id}/workflow-runs/{run_id}/interactions",get(interactions))
        .route("/projects/{project_id}/tasks/{task_id}/workflow-runs/{run_id}/interactions/{interaction_id}/response",post(respond))
}

async fn list_templates(
    State(deployment): State<DeploymentImpl>,
    Extension(caller): Extension<IntegrationCaller>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<ApiResponse<Vec<ExternalWorkflowTemplate>>>, ApiError> {
    authorize_project(&deployment.db().pool, &caller, project_id).await?;
    Ok(Json(ApiResponse::success(
        workflows::external_workflow_templates(&deployment.db().pool)
            .await?
            .into_iter()
            .map(|template| ExternalWorkflowTemplate {
                id: template.id,
                name: template.name,
                description: template.description,
                revision: template.revision,
            })
            .collect(),
    )))
}

async fn ensure_issue(
    pool: &SqlitePool,
    caller: &IntegrationCaller,
    project_id: Uuid,
    issue_id: Uuid,
) -> Result<(), ApiError> {
    authorize_project(pool, caller, project_id).await?;
    let valid: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_issues WHERE id=? AND project_id=?)")
            .bind(issue_id)
            .bind(project_id)
            .fetch_one(pool)
            .await?;
    if !valid {
        return Err(ApiError::Forbidden(
            "Task is not available in this project".into(),
        ));
    }
    Ok(())
}

async fn ensure_run(
    pool: &SqlitePool,
    caller: &IntegrationCaller,
    project_id: Uuid,
    issue_id: Uuid,
    run_id: Uuid,
) -> Result<(), ApiError> {
    ensure_issue(pool, caller, project_id, issue_id).await?;
    let valid: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_runs WHERE id=? AND issue_id=?)")
            .bind(run_id)
            .bind(issue_id)
            .fetch_one(pool)
            .await?;
    if !valid {
        return Err(ApiError::Forbidden(
            "Run is not available in this Task".into(),
        ));
    }
    Ok(())
}

async fn create_attempt(
    State(deployment): State<DeploymentImpl>,
    Extension(caller): Extension<IntegrationCaller>,
    Path((project_id, issue_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(request): Json<CreateExternalWorkflowAttempt>,
) -> Result<Json<ApiResponse<WorkflowAttemptResponse>>, ApiError> {
    let pool = &deployment.db().pool;
    ensure_issue(pool, &caller, project_id, issue_id).await?;
    crate::routes::project_store::require_task(pool, project_id, issue_id).await?;
    let root = project_root(&deployment, project_id).await?;
    let key = request_key(&headers)?;
    let hash = request_hash(&request)?;
    let scope = format!("{project_id}:{issue_id}");
    // Reservation and business identities are one transaction. Filesystem setup
    // may be retried without re-creating the Task Task or copying old sessions.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let mut reserved = IntegrationRequest::reserve(
        &mut tx,
        caller.id,
        "workflow_attempt",
        &scope,
        &key,
        &hash,
        None,
    )
    .await?;
    if reserved.request_hash != hash {
        return Err(ApiError::Conflict(
            "IDEMPOTENCY_CONFLICT: Idempotency-Key has different parameters".into(),
        ));
    }
    if reserved.state != "complete" {
        let authorized: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM external_integrations i JOIN external_integration_projects p ON p.integration_id=i.id WHERE i.id=? AND i.enabled=1 AND p.project_id=?)")
            .bind(caller.id).bind(project_id).fetch_one(&mut *tx).await?;
        if !authorized {
            return Err(ApiError::Forbidden(
                "External integration project access was revoked".into(),
            ));
        }
        let template=sqlx::query("SELECT graph_json,name,revision,source,main_agent_config_json,main_agent_prompt FROM workflows WHERE id=? AND (project_id IS NULL OR project_id=?) AND external_enabled=1 AND id NOT IN(SELECT workflow_id FROM workflow_attempts)")
            .bind(request.template_id).bind(project_id).fetch_optional(&mut *tx).await?
            .ok_or_else(||ApiError::BadRequest("Workflow is not enabled for external calls".into()))?;
        if template.try_get::<String, _>("source")? == "system"
            && !workflows::built_in_workflow_ids()?.contains(&request.template_id)
        {
            return Err(ApiError::BadRequest(
                "Workflow template is no longer available".into(),
            ));
        }
        if let Some(existing)=sqlx::query("SELECT a.id,a.workflow_id,s.template_id FROM workflow_attempts a LEFT JOIN workflow_attempt_sources s ON s.attempt_id=a.id WHERE a.issue_id=?")
            .bind(issue_id).fetch_optional(&mut *tx).await? {
            if existing.try_get::<Option<Uuid>,_>("template_id")?.unwrap_or(existing.try_get("workflow_id")?)!=request.template_id {
                return Err(ApiError::Conflict("INSTANCE_BINDING_CONFLICT: Task already has a different workflow instance".into()));
            }
            reserved.resource_id=existing.try_get("id")?;
            sqlx::query("UPDATE external_integration_requests SET resource_id=? WHERE integration_id=? AND operation='workflow_attempt' AND scope=? AND request_key=?")
                .bind(reserved.resource_id).bind(caller.id).bind(&scope).bind(&key).execute(&mut *tx).await?;
            IntegrationRequest::complete(&mut tx,caller.id,"workflow_attempt",&scope,&key).await?;
            tx.commit().await?;
            return Ok(Json(ApiResponse::success(bind_attempt_space(&deployment,reserved.resource_id,&root).await?)));
        }
        let mut graph: WorkflowGraph =
            serde_json::from_str(&template.try_get::<String, _>("graph_json")?)
                .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        for node in &mut graph.nodes {
            node.data.session_id = None;
        }
        workflow::validation::validate_graph_for_run(&graph)
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        let graph_json =
            serde_json::to_string(&graph).map_err(|e| ApiError::BadRequest(e.to_string()))?;
        let name = request
            .name
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or(template.try_get("name")?);
        let backing_id = Uuid::new_v4();
        workflows::insert_workflow_attempt(
            &mut tx,
            reserved.resource_id,
            backing_id,
            project_id,
            issue_id,
            name,
            graph_json,
        )
        .await?;
        sqlx::query("UPDATE workflows SET main_agent_config_json=?,main_agent_prompt=? WHERE id=?")
            .bind(template.try_get::<Option<String>, _>("main_agent_config_json")?)
            .bind(template.try_get::<Option<String>, _>("main_agent_prompt")?)
            .bind(backing_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO workflow_attempt_sources(attempt_id,template_id,template_revision) VALUES(?,?,?)")
            .bind(reserved.resource_id).bind(request.template_id).bind(template.try_get::<i64,_>("revision")?).execute(&mut *tx).await?;
        IntegrationRequest::complete(&mut tx, caller.id, "workflow_attempt", &scope, &key).await?;
    }
    tx.commit().await?;
    let attempt = bind_attempt_space(&deployment, reserved.resource_id, &root).await?;
    Ok(Json(ApiResponse::success(attempt)))
}

async fn bound_root(pool: &SqlitePool, workspace_id: Uuid) -> Result<PathBuf, ApiError> {
    let workspace = Workspace::find_by_id(pool, workspace_id)
        .await?
        .ok_or_else(|| ApiError::Conflict("Workflow workspace is unavailable".into()))?;
    if !workspace.is_direct_folder() {
        return Err(ApiError::Conflict(
            "External workflow requires the project directory, not an isolated worktree".into(),
        ));
    }
    let path = workspace
        .container_ref
        .ok_or_else(|| ApiError::Conflict("Workflow workspace has no directory".into()))?;
    let mut path = PathBuf::from(path);
    if let Some(relative) = Session::resolve_agent_working_dir(pool, workspace_id).await? {
        path.push(relative);
    }
    tokio::fs::canonicalize(path)
        .await
        .map_err(|_| ApiError::Conflict("Workflow workspace directory is unavailable".into()))
}

async fn bind_attempt_space(
    deployment: &DeploymentImpl,
    attempt_id: Uuid,
    root: &FsPath,
) -> Result<WorkflowAttemptResponse, ApiError> {
    static BIND_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _binding_guard = BIND_LOCK.lock().await;
    let pool = &deployment.db().pool;
    let attempt = workflows::workflow_attempt_by_id(pool, attempt_id)
        .await?
        .ok_or_else(|| ApiError::Conflict("Workflow Task is unavailable".into()))?;
    let workspace_id = if let Some(id) = attempt.workspace_id {
        if bound_root(pool, id).await? != root {
            return Err(ApiError::Conflict(
                "Workflow Task directory differs from the project directory; create a new Task"
                    .into(),
            ));
        }
        id
    } else {
        let id = DeploymentWorkflowWorkspaceResolver::new(deployment.clone())
            .create_or_bind_main_workspace(WorkflowWorkspaceRequest {
                issue_id: attempt.issue_id,
                run_id: attempt.id,
                project_id: Some(attempt.project_id),
                existing_workspace_id: None,
                directory_path: Some(root.to_string_lossy().into_owned()),
                repo_overrides: Vec::new(),
                branch_name: "direct-folder".into(),
            })
            .await?;
        sqlx::query(
            "UPDATE workflow_attempts SET workspace_id=? WHERE id=? AND workspace_id IS NULL",
        )
        .bind(id)
        .bind(attempt_id)
        .execute(pool)
        .await?;
        let actual: Uuid =
            sqlx::query_scalar("SELECT workspace_id FROM workflow_attempts WHERE id=?")
                .bind(attempt_id)
                .fetch_one(pool)
                .await?;
        if actual != id {
            Workspace::delete(pool, id).await?;
        }
        actual
    };
    let template = workflows::get_workflow_template(pool, attempt.workflow_id).await?;
    let mut graph: WorkflowGraph = serde_json::from_str(&template.graph_json)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    if workflows::ensure_agent_node_sessions(pool, workspace_id, &mut graph).await? {
        workflows::persist_workflow_graph(pool, attempt.workflow_id, template.revision, &graph)
            .await
            .map_err(ApiError::from)?;
    }
    if attempt.latest_run_id.is_none() {
        workflows::update_workflow_attempt_runtime(
            pool,
            attempt_id,
            None,
            Some(workspace_id),
            WorkflowAttemptStatus::Ready,
        )
        .await?;
    }
    workflows::workflow_attempt_by_id(pool, attempt_id)
        .await?
        .ok_or_else(|| ApiError::Conflict("Workflow Task is unavailable".into()))
}

async fn submit_run(
    State(deployment): State<DeploymentImpl>,
    Extension(caller): Extension<IntegrationCaller>,
    Path((project_id, issue_id, attempt_id)): Path<(Uuid, Uuid, Uuid)>,
    headers: HeaderMap,
    Json(request): Json<RunExternalWorkflow>,
) -> Result<(StatusCode, Json<ApiResponse<WorkflowRunResponse>>), ApiError> {
    let pool = &deployment.db().pool;
    ensure_issue(pool, &caller, project_id, issue_id).await?;
    let attempt = workflows::workflow_attempt_by_id(pool, attempt_id)
        .await?
        .filter(|a| a.project_id == project_id && a.issue_id == issue_id)
        .ok_or_else(|| ApiError::Forbidden("Workflow Task is not available in this Task".into()))?;
    let key = request_key(&headers)?;
    let hash = request_hash(&request)?;
    let scope = attempt_id.to_string();
    let mut tx = pool.begin().await?;
    let reserved = IntegrationRequest::reserve(
        &mut tx,
        caller.id,
        "workflow_run",
        &scope,
        &key,
        &hash,
        None,
    )
    .await?;
    if reserved.request_hash != hash {
        return Err(ApiError::Conflict(
            "IDEMPOTENCY_CONFLICT: Idempotency-Key has different parameters".into(),
        ));
    }
    let accepted: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_runs WHERE id=?)")
            .bind(reserved.resource_id)
            .fetch_one(&mut *tx)
            .await?;
    if !accepted {
        let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_attempt_sources s JOIN workflows w ON w.id=s.template_id WHERE s.attempt_id=? AND w.external_enabled=1)")
            .bind(attempt_id).fetch_one(&mut *tx).await?;
        if !allowed {
            return Err(ApiError::BadRequest(
                "Workflow template is not enabled for external calls".into(),
            ));
        }
    }
    tx.commit().await?;
    let management_caller = WorkflowManagementCaller::Instance {
        instance_id: attempt_id,
        namespace: format!("integration:{}", caller.id),
        integration_id: Some(caller.id),
    };
    management::verify_management_caller(pool, &management_caller).await?;
    if accepted {
        return Ok((
            StatusCode::ACCEPTED,
            Json(ApiResponse::success(
                runner::get_workflow_run_response(pool, reserved.resource_id).await?,
            )),
        ));
    }
    let root = project_root(&deployment, project_id).await?;
    let attempt = bind_attempt_space(&deployment, attempt.id, &root).await?;
    let accepted = management::submit_workflow(
        pool,
        &management_caller,
        WorkflowSubmission {
            request_id: key.clone(),
            action: if attempt.latest_run_id.is_some() {
                WorkflowSubmissionAction::Rework
            } else {
                WorkflowSubmissionAction::Start
            },
            input_text: Some(request.input_text),
            material_paths: request.material_paths,
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
        Some(reserved.resource_id),
        "external",
    )
    .await?;
    let run = runner::get_workflow_run_response(pool, accepted.run_id).await?;
    workflows::sync_attempt_from_run(pool, &run).await?;
    let mut tx = pool.begin().await?;
    IntegrationRequest::complete(&mut tx, caller.id, "workflow_run", &scope, &key).await?;
    tx.commit().await?;
    Ok((StatusCode::ACCEPTED, Json(ApiResponse::success(run))))
}

async fn read_run(
    deployment: &DeploymentImpl,
    project_id: Uuid,
    run_id: Uuid,
) -> Result<ExternalWorkflowRun, ApiError> {
    let pool = &deployment.db().pool;
    let run = runner::reconcile_workflow_run_with_arena_and_boundary(
        pool,
        run_id,
        &DeploymentWorkflowAgentExecutor::new(deployment.clone()),
        &DeploymentWorkflowArenaCreator::new(deployment.clone()),
        &DeploymentAgentRunReconciliationBoundary::new(deployment.clone()),
    )
    .await?;
    let file_changes = workflows::workflow_file_summary(pool, run_id).await?;
    Ok(ExternalWorkflowRun {
        project_id,
        run,
        file_changes,
    })
}

async fn get_run(
    State(deployment): State<DeploymentImpl>,
    Extension(caller): Extension<IntegrationCaller>,
    Path((project_id, issue_id, run_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<ApiResponse<ExternalWorkflowRun>>, ApiError> {
    ensure_run(&deployment.db().pool, &caller, project_id, issue_id, run_id).await?;
    Ok(Json(ApiResponse::success(
        read_run(&deployment, project_id, run_id).await?,
    )))
}

async fn cancel_run(
    State(deployment): State<DeploymentImpl>,
    Extension(caller): Extension<IntegrationCaller>,
    Path((project_id, issue_id, run_id)): Path<(Uuid, Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<WorkflowRunResponse>>, WorkflowManagementApiError> {
    ensure_run(&deployment.db().pool, &caller, project_id, issue_id, run_id).await?;
    let pool = &deployment.db().pool;
    let scoped = integration_run_caller(pool, &caller, run_id).await?;
    let key = workflows::workflow_operation_key(&headers, format!("cancel:{run_id}"))?;
    management::stop_workflow(
        pool,
        &scoped,
        run_id,
        &key,
        &DeploymentWorkflowRunCanceller::new(deployment.clone()),
    )
    .await?;
    let run = runner::get_workflow_run_response(pool, run_id).await?;
    Ok(Json(ApiResponse::success(run)))
}

async fn interactions(
    State(deployment): State<DeploymentImpl>,
    Extension(caller): Extension<IntegrationCaller>,
    Path((project_id, issue_id, run_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<ApiResponse<Vec<WorkflowInteraction>>>, ApiError> {
    let pool = &deployment.db().pool;
    ensure_run(pool, &caller, project_id, issue_id, run_id).await?;
    let result = management::list_run_interactions(pool, run_id).await?;
    Ok(Json(ApiResponse::success(result)))
}

async fn respond(
    State(deployment): State<DeploymentImpl>,
    Extension(caller): Extension<IntegrationCaller>,
    Path((project_id, issue_id, run_id, interaction_id)): Path<(Uuid, Uuid, Uuid, Uuid)>,
    headers: HeaderMap,
    Json(response): Json<WorkflowInteractionResponse>,
) -> Result<Json<ApiResponse<WorkflowRunResponse>>, WorkflowManagementApiError> {
    let pool = &deployment.db().pool;
    ensure_run(pool, &caller, project_id, issue_id, run_id).await?;
    let executor = DeploymentWorkflowAgentExecutor::new(deployment.clone());
    let arena = DeploymentWorkflowArenaCreator::new(deployment.clone());
    let scoped = integration_run_caller(pool, &caller, run_id).await?;
    let request_id =
        workflows::workflow_operation_key(&headers, format!("response:{interaction_id}"))?;
    let run = management::respond_to_workflow(
        pool,
        &scoped,
        management::WorkflowManagementInteractionRequest {
            run_id,
            node_execution_id: interaction_id,
            request_id,
            response,
        },
        &executor,
        &arena,
        &DeploymentWorkflowArenaWinnerApplier::new(deployment.clone()),
    )
    .await?;
    Ok(Json(ApiResponse::success(run)))
}

async fn integration_run_caller(
    pool: &SqlitePool,
    caller: &IntegrationCaller,
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
        namespace: format!("integration:{}", caller.id),
        integration_id: Some(caller.id),
    })
}
