use std::future::Future;

use api_types::{DeleteResponse, MutationResponse};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    response::Json as ResponseJson,
    routing::{get, post},
};
use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;
use db::models::workflow::WorkflowRunStatus;
use deployment::Deployment;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, sqlite::SqliteRow};
use ts_rs::TS;
use uuid::Uuid;
use workflow::{WorkflowGraph, graph::WorkflowNodeKind};

use crate::{
    DeploymentImpl,
    error::ApiError,
    routes::workflows::{
        CreateWorkflowAttemptRequest, RunWorkflowAttemptRequest, WorkflowAttemptResponse,
        WorkflowRunResponse, create_issue_workflow_attempt, get_workflow_template,
        run_workflow_attempt_runtime_with_arena, workflow_attempt_by_workflow_id,
    },
    workflow_runtime::{
        arena::DeploymentWorkflowArenaCreator, runner::DeploymentWorkflowAgentExecutor,
        workspace::DeploymentWorkflowWorkspaceResolver,
    },
};

const SCHEDULED_TASK_TICK_SECONDS: u64 = 30;
const SCHEDULED_TASK_CLAIM_SECONDS: i64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ScheduledTaskTargetType {
    Workflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ScheduledTaskKind {
    Daily,
    Weekly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ScheduledTaskConcurrencyPolicy {
    SkipIfRunning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ScheduledTaskLastStatus {
    Idle,
    Running,
    AwaitingHuman,
    AwaitingArena,
    Succeeded,
    Failed,
    Skipped,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ScheduledTaskResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub target_type: ScheduledTaskTargetType,
    pub target_id: Uuid,
    #[serde(rename = "context_task_id")]
    #[ts(rename = "context_task_id")]
    pub context_issue_id: Uuid,
    pub name: Option<String>,
    pub enabled: bool,
    pub schedule_kind: ScheduledTaskKind,
    pub time_of_day: String,
    pub weekday: Option<i32>,
    pub timezone: String,
    pub input_text: String,
    pub concurrency_policy: ScheduledTaskConcurrencyPolicy,
    pub last_run_id: Option<Uuid>,
    pub last_status: ScheduledTaskLastStatus,
    pub last_error: Option<String>,
    pub last_triggered_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ScheduledTaskListResponse {
    pub tasks: Vec<ScheduledTaskResponse>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct ListScheduledTasksQuery {
    #[serde(default)]
    #[ts(optional)]
    pub target_type: Option<ScheduledTaskTargetType>,
    #[serde(default)]
    #[ts(optional)]
    pub target_id: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct UpsertScheduledTaskRequest {
    pub target_type: ScheduledTaskTargetType,
    pub target_id: Uuid,
    #[serde(rename = "context_task_id")]
    #[ts(rename = "context_task_id")]
    pub context_issue_id: Uuid,
    #[serde(default)]
    #[ts(optional)]
    pub name: Option<String>,
    pub enabled: bool,
    pub schedule_kind: ScheduledTaskKind,
    pub time_of_day: String,
    #[serde(default)]
    #[ts(optional)]
    pub weekday: Option<i32>,
    pub timezone: String,
    pub input_text: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct UpdateScheduledTaskRequest {
    #[serde(default)]
    #[ts(optional)]
    #[serde(rename = "context_task_id")]
    #[ts(rename = "context_task_id")]
    pub context_issue_id: Option<Uuid>,
    #[serde(default)]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub enabled: Option<bool>,
    #[serde(default)]
    #[ts(optional)]
    pub schedule_kind: Option<ScheduledTaskKind>,
    #[serde(default)]
    #[ts(optional)]
    pub time_of_day: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub weekday: Option<i32>,
    #[serde(default)]
    #[ts(optional)]
    pub timezone: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub input_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ScheduledTaskRunNowResponse {
    pub task: ScheduledTaskResponse,
    #[serde(default)]
    #[ts(optional)]
    pub run: Option<WorkflowRunResponse>,
    pub skipped: bool,
}

#[derive(Debug, Clone)]
struct ScheduledTaskConfig {
    schedule_kind: ScheduledTaskKind,
    time_of_day: String,
    weekday: Option<i32>,
    timezone: String,
}

pub fn router(deployment: &DeploymentImpl) -> Router<DeploymentImpl> {
    Router::new()
        .route(
            "/v1/projects/{project_id}/scheduled-tasks",
            get(list_scheduled_tasks).post(upsert_scheduled_task),
        )
        .route(
            "/v1/scheduled-tasks/{task_id}",
            get(get_scheduled_task)
                .put(update_scheduled_task)
                .delete(delete_scheduled_task),
        )
        .route(
            "/v1/scheduled-tasks/{task_id}/run-now",
            post(run_scheduled_task_now),
        )
        .with_state(deployment.clone())
}

pub fn spawn_scheduled_task_loop(deployment: DeploymentImpl) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
            SCHEDULED_TASK_TICK_SECONDS,
        ));
        interval.tick().await;

        loop {
            interval.tick().await;
            if let Err(err) = run_due_scheduled_tasks(&deployment).await {
                tracing::warn!("scheduled task loop failed: {err}");
            }
        }
    });
}

async fn list_scheduled_tasks(
    State(deployment): State<DeploymentImpl>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListScheduledTasksQuery>,
) -> Result<ResponseJson<ScheduledTaskListResponse>, ApiError> {
    ensure_project_exists(&deployment.db().pool, project_id).await?;
    Ok(ResponseJson(ScheduledTaskListResponse {
        tasks: list_scheduled_tasks_for_project(&deployment.db().pool, project_id, query).await?,
    }))
}

async fn get_scheduled_task(
    State(deployment): State<DeploymentImpl>,
    Path(task_id): Path<Uuid>,
) -> Result<ResponseJson<ScheduledTaskResponse>, ApiError> {
    Ok(ResponseJson(
        scheduled_task_by_id(&deployment.db().pool, task_id)
            .await?
            .ok_or_else(|| ApiError::BadRequest("Scheduled task not found".to_string()))?,
    ))
}

async fn upsert_scheduled_task(
    State(deployment): State<DeploymentImpl>,
    Path(project_id): Path<Uuid>,
    Json(request): Json<UpsertScheduledTaskRequest>,
) -> Result<ResponseJson<MutationResponse<ScheduledTaskResponse>>, ApiError> {
    let data = upsert_file_owned_schedule(&deployment.db().pool, project_id, request).await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn update_scheduled_task(
    State(deployment): State<DeploymentImpl>,
    Path(task_id): Path<Uuid>,
    Json(request): Json<UpdateScheduledTaskRequest>,
) -> Result<ResponseJson<MutationResponse<ScheduledTaskResponse>>, ApiError> {
    let data = update_file_owned_schedule(&deployment.db().pool, task_id, request).await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn delete_scheduled_task(
    State(deployment): State<DeploymentImpl>,
    Path(task_id): Path<Uuid>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    delete_scheduled_task_record(&deployment.db().pool, task_id).await?;
    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn run_scheduled_task_now(
    State(deployment): State<DeploymentImpl>,
    Path(task_id): Path<Uuid>,
) -> Result<ResponseJson<MutationResponse<ScheduledTaskRunNowResponse>>, ApiError> {
    let result = dispatch_scheduled_task(&deployment, task_id, true).await?;
    Ok(ResponseJson(MutationResponse {
        data: result,
        txid: txid(),
    }))
}

async fn run_due_scheduled_tasks(deployment: &DeploymentImpl) -> Result<(), ApiError> {
    let now = Utc::now();
    let claim_until = now + Duration::seconds(SCHEDULED_TASK_CLAIM_SECONDS);
    let task_ids = claim_due_scheduled_tasks(&deployment.db().pool, now, claim_until).await?;

    for task_id in task_ids {
        let deployment = deployment.clone();
        tokio::spawn(async move {
            if let Err(err) = dispatch_scheduled_task(&deployment, task_id, false).await {
                tracing::warn!(%task_id, "scheduled task dispatch failed: {err}");
            }
        });
    }

    Ok(())
}

async fn claim_due_scheduled_tasks(
    pool: &SqlitePool,
    now: DateTime<Utc>,
    claim_until: DateTime<Utc>,
) -> Result<Vec<Uuid>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT id
        FROM scheduled_tasks
        WHERE enabled = 1
          AND next_run_at IS NOT NULL
          AND next_run_at <= ?
          AND (claim_expires_at IS NULL OR claim_expires_at <= ?)
        ORDER BY next_run_at ASC
        LIMIT 10
        "#,
    )
    .bind(now)
    .bind(now)
    .fetch_all(pool)
    .await?;

    let mut task_ids = Vec::with_capacity(rows.len());
    for row in rows {
        let task_id: Uuid = row.try_get("id")?;
        let result = sqlx::query(
            r#"
            UPDATE scheduled_tasks
            SET claim_expires_at = ?,
                updated_at = datetime('now', 'subsec')
            WHERE id = ?
              AND enabled = 1
              AND next_run_at IS NOT NULL
              AND next_run_at <= ?
              AND (claim_expires_at IS NULL OR claim_expires_at <= ?)
            "#,
        )
        .bind(claim_until)
        .bind(task_id)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;

        if result.rows_affected() > 0 {
            task_ids.push(task_id);
        }
    }

    Ok(task_ids)
}

async fn dispatch_scheduled_task(
    deployment: &DeploymentImpl,
    task_id: Uuid,
    manual: bool,
) -> Result<ScheduledTaskRunNowResponse, ApiError> {
    let result = dispatch_scheduled_task_inner(deployment, task_id, manual).await;
    if !manual && result.is_err() {
        let _ = clear_scheduled_task_claim(&deployment.db().pool, task_id).await;
    }
    result
}

async fn dispatch_scheduled_task_inner(
    deployment: &DeploymentImpl,
    task_id: Uuid,
    manual: bool,
) -> Result<ScheduledTaskRunNowResponse, ApiError> {
    let pool = &deployment.db().pool;
    dispatch_file_owned_schedule(pool, task_id, manual, |task, manual| {
        let deployment = deployment.clone();
        async move { dispatch_workflow_scheduled_task(&deployment, &task, manual).await }
    })
    .await
}

async fn dispatch_file_owned_schedule<F, Fut>(
    pool: &SqlitePool,
    task_id: Uuid,
    manual: bool,
    dispatch_target: F,
) -> Result<ScheduledTaskRunNowResponse, ApiError>
where
    F: FnOnce(ScheduledTaskResponse, bool) -> Fut,
    Fut: Future<Output = Result<(WorkflowAttemptResponse, WorkflowRunResponse), ApiError>>,
{
    dispatch_scheduled_task_record(pool, task_id, manual, |task, manual| async move {
        // Validate before the callback can create a draft Execution/instance,
        // not only later when the Run is accepted. Failures retain the schedule
        // and use its normal failure/backoff reporting; active Runs still skip.
        super::project_store::require_task(pool, task.project_id, task.context_issue_id).await?;
        dispatch_target(task, manual).await
    })
    .await
}

async fn dispatch_scheduled_task_record<F, Fut>(
    pool: &SqlitePool,
    task_id: Uuid,
    manual: bool,
    dispatch_target: F,
) -> Result<ScheduledTaskRunNowResponse, ApiError>
where
    F: FnOnce(ScheduledTaskResponse, bool) -> Fut,
    Fut: Future<Output = Result<(WorkflowAttemptResponse, WorkflowRunResponse), ApiError>>,
{
    let task = scheduled_task_by_id(pool, task_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Scheduled task not found".to_string()))?;

    if !manual && !task.enabled {
        clear_scheduled_task_claim(pool, task.id).await?;
        return Ok(ScheduledTaskRunNowResponse {
            task,
            run: None,
            skipped: true,
        });
    }

    let next_run_at = compute_next_run_at_from_task(&task, Utc::now())?;

    if matches!(task.target_type, ScheduledTaskTargetType::Workflow)
        && previous_scheduled_run_is_active(pool, task.last_run_id).await?
    {
        let updated = mark_scheduled_task_skipped(
            pool,
            task.id,
            next_run_at,
            "Previous scheduled run is still active",
        )
        .await?;
        return Ok(ScheduledTaskRunNowResponse {
            task: updated,
            run: None,
            skipped: true,
        });
    }

    let schedule_id = task.id;
    let dispatch_result = dispatch_target(task, manual).await;

    match dispatch_result {
        Ok((attempt, run)) => {
            let updated =
                mark_scheduled_task_run_started(pool, schedule_id, run.id, run.status, next_run_at)
                    .await?;
            tracing::info!(
                scheduled_task_id = %schedule_id,
                workflow_attempt_id = %attempt.id,
                workflow_run_id = %run.id,
                manual,
                "scheduled task started workflow run"
            );
            Ok(ScheduledTaskRunNowResponse {
                task: updated,
                run: Some(run),
                skipped: false,
            })
        }
        Err(err) => {
            let error_message = err.to_string();
            let updated =
                mark_scheduled_task_failed(pool, schedule_id, next_run_at, &error_message).await?;
            if manual {
                Err(err)
            } else {
                Ok(ScheduledTaskRunNowResponse {
                    task: updated,
                    run: None,
                    skipped: false,
                })
            }
        }
    }
}

async fn dispatch_workflow_scheduled_task(
    deployment: &DeploymentImpl,
    task: &ScheduledTaskResponse,
    manual: bool,
) -> Result<(WorkflowAttemptResponse, WorkflowRunResponse), ApiError> {
    ensure_workflow_target(&deployment.db().pool, task.project_id, task.target_id).await?;

    let workflow = get_workflow_template(&deployment.db().pool, task.target_id).await?;
    let attempt = create_issue_workflow_attempt(
        &deployment.db().pool,
        task.project_id,
        task.context_issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some(task.name.clone().unwrap_or_else(|| workflow.name.clone())),
            graph_json: instantiate_workflow_template_graph(&workflow.graph_json)?,
            repos: None,
        },
    )
    .await?;

    let workspace_resolver = DeploymentWorkflowWorkspaceResolver::new(deployment.clone());
    let agent_executor = DeploymentWorkflowAgentExecutor::new(deployment.clone());
    let arena_creator = DeploymentWorkflowArenaCreator::new(deployment.clone());
    let run = run_workflow_attempt_runtime_with_arena(
        &deployment.db().pool,
        attempt.id,
        RunWorkflowAttemptRequest {
            directory_path: None,
            workspace_id: None,
            trigger_source: if manual {
                "schedule_manual".to_string()
            } else {
                "schedule".to_string()
            },
            input_text: task.input_text.clone(),
            repos: None,
        },
        &workspace_resolver,
        &agent_executor,
        &arena_creator,
    )
    .await?;

    Ok((attempt, run))
}

async fn upsert_file_owned_schedule(
    pool: &SqlitePool,
    project_id: Uuid,
    request: UpsertScheduledTaskRequest,
) -> Result<ScheduledTaskResponse, ApiError> {
    super::project_store::require_task(pool, project_id, request.context_issue_id).await?;
    upsert_scheduled_task_record(pool, project_id, request).await
}

async fn update_file_owned_schedule(
    pool: &SqlitePool,
    task_id: Uuid,
    request: UpdateScheduledTaskRequest,
) -> Result<ScheduledTaskResponse, ApiError> {
    let existing = scheduled_task_by_id(pool, task_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Scheduled task not found".to_string()))?;
    let context_id = request
        .context_issue_id
        .unwrap_or(existing.context_issue_id);
    // An unavailable directory must not prevent disabling an existing schedule.
    // Enabling or changing its business Task always needs current file authority.
    if request.enabled.unwrap_or(existing.enabled) || context_id != existing.context_issue_id {
        super::project_store::require_task(pool, existing.project_id, context_id).await?;
    }
    update_scheduled_task_record(pool, task_id, request).await
}

async fn upsert_scheduled_task_record(
    pool: &SqlitePool,
    project_id: Uuid,
    request: UpsertScheduledTaskRequest,
) -> Result<ScheduledTaskResponse, ApiError> {
    ensure_project_exists(pool, project_id).await?;
    validate_target_and_context(
        pool,
        project_id,
        request.target_type,
        request.target_id,
        request.context_issue_id,
    )
    .await?;
    let config = validate_schedule_config(
        request.schedule_kind,
        &request.time_of_day,
        request.weekday,
        &request.timezone,
    )?;
    let next_run_at = if request.enabled {
        Some(compute_next_run_at(&config, Utc::now())?)
    } else {
        None
    };
    let task_id = Uuid::new_v4();
    let name = normalize_optional_string(request.name);

    sqlx::query(
        r#"
        INSERT INTO scheduled_tasks (
            id, project_id, target_type, target_id, context_issue_id, name, enabled,
            schedule_kind, time_of_day, weekday, timezone, input_text,
            concurrency_policy, next_run_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'skip_if_running', ?)
        ON CONFLICT(project_id, target_type, target_id) DO UPDATE SET
            context_issue_id = excluded.context_issue_id,
            name = excluded.name,
            enabled = excluded.enabled,
            schedule_kind = excluded.schedule_kind,
            time_of_day = excluded.time_of_day,
            weekday = excluded.weekday,
            timezone = excluded.timezone,
            input_text = excluded.input_text,
            next_run_at = excluded.next_run_at,
            claim_expires_at = NULL,
            updated_at = datetime('now', 'subsec')
        "#,
    )
    .bind(task_id)
    .bind(project_id)
    .bind(target_type_value(request.target_type))
    .bind(request.target_id)
    .bind(request.context_issue_id)
    .bind(name)
    .bind(bool_to_i64(request.enabled))
    .bind(schedule_kind_value(request.schedule_kind))
    .bind(config.time_of_day)
    .bind(config.weekday)
    .bind(config.timezone)
    .bind(request.input_text)
    .bind(next_run_at)
    .execute(pool)
    .await?;

    scheduled_task_for_target(pool, project_id, request.target_type, request.target_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Scheduled task not found after save".to_string()))
}

async fn update_scheduled_task_record(
    pool: &SqlitePool,
    task_id: Uuid,
    request: UpdateScheduledTaskRequest,
) -> Result<ScheduledTaskResponse, ApiError> {
    let existing = scheduled_task_by_id(pool, task_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Scheduled task not found".to_string()))?;

    let context_issue_id = request
        .context_issue_id
        .unwrap_or(existing.context_issue_id);
    let schedule_kind = request.schedule_kind.unwrap_or(existing.schedule_kind);
    let time_of_day = request.time_of_day.unwrap_or(existing.time_of_day);
    let name = request
        .name
        .map(Some)
        .unwrap_or(existing.name)
        .and_then(|value| normalize_optional_string(Some(value)));
    let weekday = if matches!(schedule_kind, ScheduledTaskKind::Daily) {
        None
    } else {
        request.weekday.or(existing.weekday)
    };
    let timezone = request.timezone.unwrap_or(existing.timezone);
    let enabled = request.enabled.unwrap_or(existing.enabled);
    validate_target_and_context(
        pool,
        existing.project_id,
        existing.target_type,
        existing.target_id,
        context_issue_id,
    )
    .await?;
    let config = validate_schedule_config(schedule_kind, &time_of_day, weekday, &timezone)?;
    let next_run_at = if enabled {
        Some(compute_next_run_at(&config, Utc::now())?)
    } else {
        None
    };

    sqlx::query(
        r#"
        UPDATE scheduled_tasks
        SET context_issue_id = ?,
            name = ?,
            enabled = ?,
            schedule_kind = ?,
            time_of_day = ?,
            weekday = ?,
            timezone = ?,
            input_text = ?,
            next_run_at = ?,
            claim_expires_at = NULL,
            updated_at = datetime('now', 'subsec')
        WHERE id = ?
        "#,
    )
    .bind(context_issue_id)
    .bind(name)
    .bind(bool_to_i64(enabled))
    .bind(schedule_kind_value(schedule_kind))
    .bind(config.time_of_day)
    .bind(config.weekday)
    .bind(config.timezone)
    .bind(request.input_text.unwrap_or(existing.input_text))
    .bind(next_run_at)
    .bind(task_id)
    .execute(pool)
    .await?;

    scheduled_task_by_id(pool, task_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Scheduled task not found after update".to_string()))
}

async fn delete_scheduled_task_record(pool: &SqlitePool, task_id: Uuid) -> Result<(), ApiError> {
    let result = sqlx::query("DELETE FROM scheduled_tasks WHERE id = ?")
        .bind(task_id)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::BadRequest("Scheduled task not found".to_string()));
    }

    Ok(())
}

async fn list_scheduled_tasks_for_project(
    pool: &SqlitePool,
    project_id: Uuid,
    query: ListScheduledTasksQuery,
) -> Result<Vec<ScheduledTaskResponse>, ApiError> {
    let mut builder = QueryBuilder::<Sqlite>::new(
        r#"
        SELECT id, project_id, target_type, target_id, context_issue_id, name, enabled,
               schedule_kind, time_of_day, weekday, timezone, input_text,
               concurrency_policy, last_run_id, last_status, last_error,
               last_triggered_at, next_run_at, created_at, updated_at
        FROM scheduled_tasks
        WHERE project_id =
        "#,
    );
    builder.push_bind(project_id);

    if let Some(target_type) = query.target_type {
        builder.push(" AND target_type = ");
        builder.push_bind(target_type_value(target_type));
    }
    if let Some(target_id) = query.target_id {
        builder.push(" AND target_id = ");
        builder.push_bind(target_id);
    }
    builder.push(" ORDER BY updated_at DESC, created_at DESC");

    let rows = builder.build().fetch_all(pool).await?;
    rows.iter().map(scheduled_task_from_row).collect()
}

async fn scheduled_task_by_id(
    pool: &SqlitePool,
    task_id: Uuid,
) -> Result<Option<ScheduledTaskResponse>, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT id, project_id, target_type, target_id, context_issue_id, name, enabled,
               schedule_kind, time_of_day, weekday, timezone, input_text,
               concurrency_policy, last_run_id, last_status, last_error,
               last_triggered_at, next_run_at, created_at, updated_at
        FROM scheduled_tasks
        WHERE id = ?
        "#,
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(scheduled_task_from_row).transpose()
}

async fn scheduled_task_for_target(
    pool: &SqlitePool,
    project_id: Uuid,
    target_type: ScheduledTaskTargetType,
    target_id: Uuid,
) -> Result<Option<ScheduledTaskResponse>, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT id, project_id, target_type, target_id, context_issue_id, name, enabled,
               schedule_kind, time_of_day, weekday, timezone, input_text,
               concurrency_policy, last_run_id, last_status, last_error,
               last_triggered_at, next_run_at, created_at, updated_at
        FROM scheduled_tasks
        WHERE project_id = ? AND target_type = ? AND target_id = ?
        "#,
    )
    .bind(project_id)
    .bind(target_type_value(target_type))
    .bind(target_id)
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(scheduled_task_from_row).transpose()
}

async fn previous_scheduled_run_is_active(
    pool: &SqlitePool,
    run_id: Option<Uuid>,
) -> Result<bool, ApiError> {
    let Some(run_id) = run_id else {
        return Ok(false);
    };

    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM workflow_runs WHERE id = ?")
            .bind(run_id)
            .fetch_optional(pool)
            .await?;

    Ok(matches!(
        status.as_deref(),
        Some("pending" | "running" | "awaiting_human" | "awaiting_arena")
    ))
}

async fn mark_scheduled_task_run_started(
    pool: &SqlitePool,
    task_id: Uuid,
    run_id: Uuid,
    run_status: WorkflowRunStatus,
    next_run_at: DateTime<Utc>,
) -> Result<ScheduledTaskResponse, ApiError> {
    sqlx::query(
        r#"
        UPDATE scheduled_tasks
        SET last_run_id = ?,
            last_status = ?,
            last_error = NULL,
            last_triggered_at = ?,
            next_run_at = ?,
            claim_expires_at = NULL,
            updated_at = datetime('now', 'subsec')
        WHERE id = ?
        "#,
    )
    .bind(run_id)
    .bind(last_status_from_run_status(run_status))
    .bind(Utc::now())
    .bind(next_run_at)
    .bind(task_id)
    .execute(pool)
    .await?;

    scheduled_task_by_id(pool, task_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Scheduled task not found after run".to_string()))
}

async fn mark_scheduled_task_skipped(
    pool: &SqlitePool,
    task_id: Uuid,
    next_run_at: DateTime<Utc>,
    error: &str,
) -> Result<ScheduledTaskResponse, ApiError> {
    sqlx::query(
        r#"
        UPDATE scheduled_tasks
        SET last_status = 'skipped',
            last_error = ?,
            last_triggered_at = ?,
            next_run_at = ?,
            claim_expires_at = NULL,
            updated_at = datetime('now', 'subsec')
        WHERE id = ?
        "#,
    )
    .bind(error)
    .bind(Utc::now())
    .bind(next_run_at)
    .bind(task_id)
    .execute(pool)
    .await?;

    scheduled_task_by_id(pool, task_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Scheduled task not found after skip".to_string()))
}

async fn mark_scheduled_task_failed(
    pool: &SqlitePool,
    task_id: Uuid,
    next_run_at: DateTime<Utc>,
    error: &str,
) -> Result<ScheduledTaskResponse, ApiError> {
    sqlx::query(
        r#"
        UPDATE scheduled_tasks
        SET last_status = 'failed',
            last_error = ?,
            last_triggered_at = ?,
            next_run_at = ?,
            claim_expires_at = NULL,
            updated_at = datetime('now', 'subsec')
        WHERE id = ?
        "#,
    )
    .bind(error)
    .bind(Utc::now())
    .bind(next_run_at)
    .bind(task_id)
    .execute(pool)
    .await?;

    scheduled_task_by_id(pool, task_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Scheduled task not found after failure".to_string()))
}

async fn clear_scheduled_task_claim(pool: &SqlitePool, task_id: Uuid) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE scheduled_tasks
        SET claim_expires_at = NULL,
            updated_at = datetime('now', 'subsec')
        WHERE id = ?
        "#,
    )
    .bind(task_id)
    .execute(pool)
    .await?;
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

async fn validate_target_and_context(
    pool: &SqlitePool,
    project_id: Uuid,
    target_type: ScheduledTaskTargetType,
    target_id: Uuid,
    context_issue_id: Uuid,
) -> Result<(), ApiError> {
    ensure_issue_belongs_to_project(pool, project_id, context_issue_id).await?;
    match target_type {
        ScheduledTaskTargetType::Workflow => {
            ensure_workflow_target(pool, project_id, target_id).await?;
        }
    }
    Ok(())
}

async fn ensure_workflow_target(
    pool: &SqlitePool,
    project_id: Uuid,
    workflow_id: Uuid,
) -> Result<(), ApiError> {
    let workflow = get_workflow_template(pool, workflow_id).await?;
    if let Some(owner_project_id) = workflow.project_id {
        if owner_project_id != project_id {
            return Err(ApiError::BadRequest(
                "Workflow does not belong to project".to_string(),
            ));
        }
    }
    if workflow_attempt_by_workflow_id(pool, workflow_id)
        .await?
        .is_some()
    {
        return Err(ApiError::BadRequest(
            "Scheduled task target must be a workflow template".to_string(),
        ));
    }
    Ok(())
}

fn instantiate_workflow_template_graph(graph_json: &str) -> Result<String, ApiError> {
    let mut graph: WorkflowGraph = serde_json::from_str(graph_json)
        .map_err(|err| ApiError::BadRequest(format!("Invalid workflow graph JSON: {err}")))?;
    for node in graph
        .nodes
        .iter_mut()
        .filter(|node| node.kind == WorkflowNodeKind::Agent)
    {
        node.data.session_id = None;
    }
    serde_json::to_string(&graph)
        .map_err(|err| ApiError::BadRequest(format!("Invalid workflow graph JSON: {err}")))
}

fn scheduled_task_from_row(row: &SqliteRow) -> Result<ScheduledTaskResponse, ApiError> {
    let enabled: i64 = row.try_get("enabled")?;
    Ok(ScheduledTaskResponse {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        target_type: target_type_from_str(&row.try_get::<String, _>("target_type")?)?,
        target_id: row.try_get("target_id")?,
        context_issue_id: row.try_get("context_issue_id")?,
        name: row.try_get("name")?,
        enabled: enabled != 0,
        schedule_kind: schedule_kind_from_str(&row.try_get::<String, _>("schedule_kind")?)?,
        time_of_day: row.try_get("time_of_day")?,
        weekday: row.try_get("weekday")?,
        timezone: row.try_get("timezone")?,
        input_text: row.try_get("input_text")?,
        concurrency_policy: concurrency_policy_from_str(
            &row.try_get::<String, _>("concurrency_policy")?,
        )?,
        last_run_id: row.try_get("last_run_id")?,
        last_status: last_status_from_str(&row.try_get::<String, _>("last_status")?)?,
        last_error: row.try_get("last_error")?,
        last_triggered_at: row.try_get("last_triggered_at")?,
        next_run_at: row.try_get("next_run_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn validate_schedule_config(
    schedule_kind: ScheduledTaskKind,
    time_of_day: &str,
    weekday: Option<i32>,
    timezone: &str,
) -> Result<ScheduledTaskConfig, ApiError> {
    parse_time_of_day(time_of_day)?;
    let timezone = timezone.trim();
    if timezone.is_empty() {
        return Err(ApiError::BadRequest("Timezone is required".to_string()));
    }
    if !is_supported_timezone(timezone) {
        return Err(ApiError::BadRequest(format!(
            "Unsupported timezone `{timezone}`"
        )));
    }
    match schedule_kind {
        ScheduledTaskKind::Daily => {
            if weekday.is_some() {
                return Err(ApiError::BadRequest(
                    "Daily scheduled tasks must not include weekday".to_string(),
                ));
            }
        }
        ScheduledTaskKind::Weekly => {
            if !matches!(weekday, Some(1..=7)) {
                return Err(ApiError::BadRequest(
                    "Weekly scheduled tasks require weekday 1-7".to_string(),
                ));
            }
        }
    }

    Ok(ScheduledTaskConfig {
        schedule_kind,
        time_of_day: time_of_day.trim().to_string(),
        weekday,
        timezone: timezone.to_string(),
    })
}

fn compute_next_run_at_from_task(
    task: &ScheduledTaskResponse,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>, ApiError> {
    let config = validate_schedule_config(
        task.schedule_kind,
        &task.time_of_day,
        task.weekday,
        &task.timezone,
    )?;
    compute_next_run_at(&config, after)
}

fn compute_next_run_at(
    config: &ScheduledTaskConfig,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>, ApiError> {
    let time = parse_time_of_day(&config.time_of_day)?;
    let local_after = utc_to_local(after, &config.timezone)?;
    let mut date = local_after.date();

    for _ in 0..370 {
        if matches!(config.schedule_kind, ScheduledTaskKind::Weekly) {
            let expected = weekday_from_i32(config.weekday.ok_or_else(|| {
                ApiError::BadRequest("Weekly scheduled tasks require weekday".to_string())
            })?);
            if date.weekday() != expected {
                date = date.succ_opt().ok_or_else(|| {
                    ApiError::BadRequest("Unable to compute next scheduled date".to_string())
                })?;
                continue;
            }
        }

        let candidate_local = NaiveDateTime::new(date, time);
        if let Some(candidate_utc) = local_to_utc(candidate_local, &config.timezone, after)? {
            return Ok(candidate_utc);
        }
        date = date.succ_opt().ok_or_else(|| {
            ApiError::BadRequest("Unable to compute next scheduled date".to_string())
        })?;
    }

    Err(ApiError::BadRequest(
        "Unable to compute next scheduled run".to_string(),
    ))
}

fn parse_time_of_day(value: &str) -> Result<NaiveTime, ApiError> {
    NaiveTime::parse_from_str(value.trim(), "%H:%M")
        .map_err(|_| ApiError::BadRequest("time_of_day must use HH:mm 24-hour format".to_string()))
}

fn is_supported_timezone(value: &str) -> bool {
    value.parse::<Tz>().is_ok()
}

fn utc_to_local(value: DateTime<Utc>, timezone: &str) -> Result<NaiveDateTime, ApiError> {
    let timezone = parse_timezone(timezone)?;
    Ok(value.with_timezone(&timezone).naive_local())
}

fn local_to_utc(
    value: NaiveDateTime,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, ApiError> {
    let timezone = parse_timezone(timezone)?;
    Ok(resolve_local_datetime(value, timezone, after))
}

fn parse_timezone(value: &str) -> Result<Tz, ApiError> {
    value
        .parse::<Tz>()
        .map_err(|_| ApiError::BadRequest(format!("Unsupported timezone `{value}`")))
}

fn resolve_local_datetime(
    value: NaiveDateTime,
    timezone: Tz,
    after: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    match timezone.from_local_datetime(&value) {
        LocalResult::Single(local) => {
            let utc = local.with_timezone(&Utc);
            (utc > after).then_some(utc)
        }
        LocalResult::Ambiguous(early, late) => {
            let early = early.with_timezone(&Utc);
            let late = late.with_timezone(&Utc);
            if early > after {
                Some(early)
            } else if late > after {
                Some(late)
            } else {
                None
            }
        }
        LocalResult::None => (1..=180).find_map(|minutes| {
            let shifted = value + Duration::minutes(minutes);
            resolve_local_datetime(shifted, timezone, after)
        }),
    }
}

fn weekday_from_i32(value: i32) -> Weekday {
    match value {
        1 => Weekday::Mon,
        2 => Weekday::Tue,
        3 => Weekday::Wed,
        4 => Weekday::Thu,
        5 => Weekday::Fri,
        6 => Weekday::Sat,
        _ => Weekday::Sun,
    }
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

fn bool_to_i64(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

fn target_type_value(value: ScheduledTaskTargetType) -> &'static str {
    match value {
        ScheduledTaskTargetType::Workflow => "workflow",
    }
}

fn target_type_from_str(value: &str) -> Result<ScheduledTaskTargetType, ApiError> {
    match value {
        "workflow" => Ok(ScheduledTaskTargetType::Workflow),
        other => Err(ApiError::BadRequest(format!(
            "Unknown scheduled task target type `{other}`"
        ))),
    }
}

fn schedule_kind_value(value: ScheduledTaskKind) -> &'static str {
    match value {
        ScheduledTaskKind::Daily => "daily",
        ScheduledTaskKind::Weekly => "weekly",
    }
}

fn schedule_kind_from_str(value: &str) -> Result<ScheduledTaskKind, ApiError> {
    match value {
        "daily" => Ok(ScheduledTaskKind::Daily),
        "weekly" => Ok(ScheduledTaskKind::Weekly),
        other => Err(ApiError::BadRequest(format!(
            "Unknown scheduled task kind `{other}`"
        ))),
    }
}

fn concurrency_policy_from_str(value: &str) -> Result<ScheduledTaskConcurrencyPolicy, ApiError> {
    match value {
        "skip_if_running" => Ok(ScheduledTaskConcurrencyPolicy::SkipIfRunning),
        other => Err(ApiError::BadRequest(format!(
            "Unknown scheduled task concurrency policy `{other}`"
        ))),
    }
}

fn last_status_from_str(value: &str) -> Result<ScheduledTaskLastStatus, ApiError> {
    match value {
        "idle" => Ok(ScheduledTaskLastStatus::Idle),
        "running" => Ok(ScheduledTaskLastStatus::Running),
        "awaiting_human" => Ok(ScheduledTaskLastStatus::AwaitingHuman),
        "awaiting_arena" => Ok(ScheduledTaskLastStatus::AwaitingArena),
        "succeeded" => Ok(ScheduledTaskLastStatus::Succeeded),
        "failed" => Ok(ScheduledTaskLastStatus::Failed),
        "skipped" => Ok(ScheduledTaskLastStatus::Skipped),
        "canceled" => Ok(ScheduledTaskLastStatus::Canceled),
        other => Err(ApiError::BadRequest(format!(
            "Unknown scheduled task status `{other}`"
        ))),
    }
}

fn last_status_from_run_status(status: WorkflowRunStatus) -> &'static str {
    match status {
        WorkflowRunStatus::Pending | WorkflowRunStatus::Running | WorkflowRunStatus::Cancelling => {
            "running"
        }
        WorkflowRunStatus::AwaitingHuman => "awaiting_human",
        WorkflowRunStatus::AwaitingArena => "awaiting_arena",
        WorkflowRunStatus::Succeeded => "succeeded",
        WorkflowRunStatus::Failed => "failed",
        WorkflowRunStatus::Canceled => "canceled",
    }
}

fn txid() -> i64 {
    Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    #[test]
    fn daily_next_run_uses_timezone_local_time() {
        let config = ScheduledTaskConfig {
            schedule_kind: ScheduledTaskKind::Daily,
            time_of_day: "09:00".to_string(),
            weekday: None,
            timezone: "Asia/Shanghai".to_string(),
        };
        let after = Utc.with_ymd_and_hms(2026, 7, 6, 0, 30, 0).unwrap();

        let next = compute_next_run_at(&config, after).unwrap();

        assert_eq!(next, Utc.with_ymd_and_hms(2026, 7, 6, 1, 0, 0).unwrap());
    }

    #[test]
    fn daily_next_run_rolls_to_tomorrow_after_local_time() {
        let config = ScheduledTaskConfig {
            schedule_kind: ScheduledTaskKind::Daily,
            time_of_day: "09:00".to_string(),
            weekday: None,
            timezone: "Asia/Shanghai".to_string(),
        };
        let after = Utc.with_ymd_and_hms(2026, 7, 6, 1, 30, 0).unwrap();

        let next = compute_next_run_at(&config, after).unwrap();

        assert_eq!(next, Utc.with_ymd_and_hms(2026, 7, 7, 1, 0, 0).unwrap());
    }

    #[test]
    fn daily_next_run_uses_iana_timezone_dst_rules() {
        let config = ScheduledTaskConfig {
            schedule_kind: ScheduledTaskKind::Daily,
            time_of_day: "09:00".to_string(),
            weekday: None,
            timezone: "America/New_York".to_string(),
        };
        let after = Utc.with_ymd_and_hms(2026, 7, 6, 12, 30, 0).unwrap();

        let next = compute_next_run_at(&config, after).unwrap();

        assert_eq!(next, Utc.with_ymd_and_hms(2026, 7, 6, 13, 0, 0).unwrap());
    }

    #[test]
    fn weekly_next_run_finds_requested_weekday() {
        let config = ScheduledTaskConfig {
            schedule_kind: ScheduledTaskKind::Weekly,
            time_of_day: "09:00".to_string(),
            weekday: Some(3),
            timezone: "UTC".to_string(),
        };
        let after = Utc.with_ymd_and_hms(2026, 7, 6, 1, 0, 0).unwrap();

        let next = compute_next_run_at(&config, after).unwrap();

        assert_eq!(next, Utc.with_ymd_and_hms(2026, 7, 8, 9, 0, 0).unwrap());
    }

    #[test]
    fn weekly_requires_weekday() {
        let err =
            validate_schedule_config(ScheduledTaskKind::Weekly, "09:00", None, "UTC").unwrap_err();

        assert!(err.to_string().contains("weekday"));
    }

    #[tokio::test]
    async fn upsert_update_list_and_delete_scheduled_task() {
        let pool = setup_scheduled_task_pool().await;
        let ids = insert_scheduled_task_fixture(&pool).await;

        let created = upsert_scheduled_task_record(
            &pool,
            ids.project_id,
            upsert_request(ids.workflow_id, ids.issue_id),
        )
        .await
        .expect("create scheduled task");

        assert_eq!(created.target_type, ScheduledTaskTargetType::Workflow);
        assert_eq!(created.target_id, ids.workflow_id);
        assert_eq!(created.context_issue_id, ids.issue_id);
        assert_eq!(created.name.as_deref(), Some("Morning check"));
        assert!(created.enabled);
        assert_eq!(created.schedule_kind, ScheduledTaskKind::Daily);
        assert_eq!(created.weekday, None);
        assert!(created.next_run_at.is_some());

        let listed = list_scheduled_tasks_for_project(
            &pool,
            ids.project_id,
            ListScheduledTasksQuery {
                target_type: Some(ScheduledTaskTargetType::Workflow),
                target_id: Some(ids.workflow_id),
            },
        )
        .await
        .expect("list scheduled tasks");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);

        let updated = update_scheduled_task_record(
            &pool,
            created.id,
            UpdateScheduledTaskRequest {
                context_issue_id: None,
                name: Some("Weekly check".to_string()),
                enabled: Some(false),
                schedule_kind: Some(ScheduledTaskKind::Weekly),
                time_of_day: Some("10:30".to_string()),
                weekday: Some(5),
                timezone: Some("UTC".to_string()),
                input_text: Some("Run weekly".to_string()),
            },
        )
        .await
        .expect("update scheduled task");

        assert_eq!(updated.name.as_deref(), Some("Weekly check"));
        assert!(!updated.enabled);
        assert_eq!(updated.schedule_kind, ScheduledTaskKind::Weekly);
        assert_eq!(updated.weekday, Some(5));
        assert_eq!(updated.time_of_day, "10:30");
        assert_eq!(updated.timezone, "UTC");
        assert_eq!(updated.input_text, "Run weekly");
        assert!(updated.next_run_at.is_none());

        delete_scheduled_task_record(&pool, created.id)
            .await
            .expect("delete scheduled task");
        assert!(
            scheduled_task_by_id(&pool, created.id)
                .await
                .expect("read deleted scheduled task")
                .is_none()
        );
    }

    #[tokio::test]
    async fn upsert_rejects_issue_from_another_project() {
        let pool = setup_scheduled_task_pool().await;
        let ids = insert_scheduled_task_fixture(&pool).await;
        let other_project_id = Uuid::new_v4();
        let other_issue_id = Uuid::new_v4();
        insert_project(&pool, other_project_id).await;
        insert_issue(&pool, other_project_id, other_issue_id).await;

        let err = upsert_scheduled_task_record(
            &pool,
            ids.project_id,
            upsert_request(ids.workflow_id, other_issue_id),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Task not found for project"));
    }

    #[tokio::test]
    async fn due_task_claim_sets_claim_expiry_once() {
        let pool = setup_scheduled_task_pool().await;
        let ids = insert_scheduled_task_fixture(&pool).await;
        let task = upsert_scheduled_task_record(
            &pool,
            ids.project_id,
            upsert_request(ids.workflow_id, ids.issue_id),
        )
        .await
        .expect("create scheduled task");
        let now = Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).unwrap();
        sqlx::query("UPDATE scheduled_tasks SET next_run_at = ? WHERE id = ?")
            .bind(now - Duration::minutes(1))
            .bind(task.id)
            .execute(&pool)
            .await
            .expect("make task due");

        let claimed = claim_due_scheduled_tasks(&pool, now, now + Duration::seconds(120))
            .await
            .expect("claim due task");
        let claimed_again = claim_due_scheduled_tasks(&pool, now, now + Duration::seconds(120))
            .await
            .expect("claim due task again");

        assert_eq!(claimed, vec![task.id]);
        assert!(claimed_again.is_empty());
    }

    #[tokio::test]
    async fn run_now_marks_task_started() {
        let pool = setup_scheduled_task_pool().await;
        let ids = insert_scheduled_task_fixture(&pool).await;
        let task = upsert_scheduled_task_record(
            &pool,
            ids.project_id,
            upsert_request(ids.workflow_id, ids.issue_id),
        )
        .await
        .expect("create scheduled task");
        let run_id = Uuid::new_v4();

        let result =
            dispatch_scheduled_task_record(&pool, task.id, true, move |task, manual| async move {
                assert!(manual);
                assert_eq!(task.input_text, "Check status");
                Ok(fake_workflow_dispatch(
                    task.project_id,
                    task.context_issue_id,
                    task.target_id,
                    run_id,
                    WorkflowRunStatus::Succeeded,
                ))
            })
            .await
            .expect("run task now");

        assert!(!result.skipped);
        assert_eq!(result.run.as_ref().map(|run| run.id), Some(run_id));
        assert_eq!(result.task.last_run_id, Some(run_id));
        assert_eq!(result.task.last_status, ScheduledTaskLastStatus::Succeeded);
        assert!(result.task.last_error.is_none());
        assert!(result.task.next_run_at.is_some());
    }

    #[tokio::test]
    async fn active_previous_run_is_skipped_without_dispatching() {
        let pool = setup_scheduled_task_pool().await;
        let ids = insert_scheduled_task_fixture(&pool).await;
        let task = upsert_scheduled_task_record(
            &pool,
            ids.project_id,
            upsert_request(ids.workflow_id, ids.issue_id),
        )
        .await
        .expect("create scheduled task");
        let active_run_id = Uuid::new_v4();
        insert_workflow_run(
            &pool,
            active_run_id,
            ids.workflow_id,
            ids.issue_id,
            "running",
        )
        .await;
        sqlx::query("UPDATE scheduled_tasks SET last_run_id = ? WHERE id = ?")
            .bind(active_run_id)
            .bind(task.id)
            .execute(&pool)
            .await
            .expect("link active run");

        let result =
            dispatch_scheduled_task_record(&pool, task.id, false, |_task, _manual| async {
                panic!("active previous runs must skip before dispatch");
            })
            .await
            .expect("skip active previous run");

        assert!(result.skipped);
        assert!(result.run.is_none());
        assert_eq!(result.task.last_run_id, Some(active_run_id));
        assert_eq!(result.task.last_status, ScheduledTaskLastStatus::Skipped);
        assert_eq!(
            result.task.last_error.as_deref(),
            Some("Previous scheduled run is still active")
        );
        assert!(result.task.next_run_at.is_some());
    }

    #[tokio::test]
    async fn file_owned_schedule_rejects_stale_projection_before_write_or_dispatch() {
        use crate::routes::{
            project_store,
            workflows::{CreateWorkflowRequest, create_project_workflow},
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let project = project_store::open(
            &pool,
            directory.path().to_str().unwrap(),
            api_types::CreateProjectRequest {
                id: None,
                organization_id: Uuid::from_u128(2),
                name: "Scheduled project".into(),
                color: "210 80% 52%".into(),
            },
        )
        .await
        .unwrap();
        let task_id = project_store::mutate(&pool, project.id, |document| {
            document.insert_task(
                serde_json::from_value(json!({
                    "project_id": project.id,
                    "status_id": document.statuses[0].id,
                    "title": "Scheduled business task",
                    "sort_order": 0.0,
                    "extension_metadata": null
                }))
                .unwrap(),
            )
        })
        .await
        .unwrap();
        let workflow = create_project_workflow(
            &pool,
            project.id,
            CreateWorkflowRequest {
                name: "Scheduled workflow".into(),
                description: None,
                graph_json: valid_graph_json(),
            },
        )
        .await
        .unwrap();
        let schedule =
            upsert_file_owned_schedule(&pool, project.id, upsert_request(workflow.id, task_id))
                .await
                .unwrap();
        let path = directory.path().join(".vibe-kanban/project.json");
        let original = std::fs::read(&path).unwrap();

        for missing in [false, true] {
            if missing {
                std::fs::remove_file(&path).unwrap();
            } else {
                std::fs::write(&path, b"{invalid").unwrap();
            }
            assert!(
                upsert_file_owned_schedule(
                    &pool,
                    project.id,
                    upsert_request(workflow.id, task_id),
                )
                .await
                .is_err()
            );
            assert!(
                update_file_owned_schedule(
                    &pool,
                    schedule.id,
                    serde_json::from_value(json!({ "enabled": true })).unwrap(),
                )
                .await
                .is_err()
            );
            assert!(
                dispatch_file_owned_schedule(&pool, schedule.id, true, |_, _| async {
                    panic!("missing task authority must not create an Execution or Run")
                })
                .await
                .is_err()
            );
            let automatic = dispatch_file_owned_schedule(&pool, schedule.id, false, |_, _| async {
                panic!("automatic dispatch must check the same task authority")
            })
            .await
            .unwrap();
            assert_eq!(automatic.task.last_status, ScheduledTaskLastStatus::Failed);
            assert!(automatic.task.last_error.is_some());
            assert!(automatic.task.next_run_at.is_some());
            assert!(automatic.run.is_none());
            let runtime: i64 = sqlx::query_scalar(
                "SELECT (SELECT COUNT(*) FROM tasks) + (SELECT COUNT(*) FROM workflow_attempts) + (SELECT COUNT(*) FROM workflow_runs)",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(runtime, 0);
            let indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_issues WHERE id=?")
                .bind(task_id)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(
                indexed, 1,
                "the stale projection remains, but cannot authorize work"
            );
        }

        let disabled = update_file_owned_schedule(
            &pool,
            schedule.id,
            serde_json::from_value(json!({ "enabled": false })).unwrap(),
        )
        .await
        .unwrap();
        assert!(!disabled.enabled);
        assert!(disabled.next_run_at.is_none());
        // Restoring the same authoritative file permits normal enabling again.
        std::fs::write(&path, original).unwrap();
        let enabled = update_file_owned_schedule(
            &pool,
            schedule.id,
            serde_json::from_value(json!({ "enabled": true })).unwrap(),
        )
        .await
        .unwrap();
        assert!(enabled.enabled);
        std::fs::remove_file(&path).unwrap();
        delete_scheduled_task_record(&pool, schedule.id)
            .await
            .unwrap();
        assert!(
            scheduled_task_by_id(&pool, schedule.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    struct ScheduledTaskFixtureIds {
        project_id: Uuid,
        issue_id: Uuid,
        workflow_id: Uuid,
    }

    async fn setup_scheduled_task_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        for statement in [
            r#"
            CREATE TABLE projects (
                id BLOB PRIMARY KEY,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
            )
            "#,
            r#"
            CREATE TABLE local_issues (
                id BLOB PRIMARY KEY,
                project_id BLOB NOT NULL,
                title TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
            )
            "#,
            r#"
            CREATE TABLE workflows (
                id BLOB PRIMARY KEY,
                source TEXT NOT NULL CHECK (source IN ('system','project')),
                project_id BLOB,
                name TEXT NOT NULL,
                description TEXT,
                graph_json TEXT NOT NULL,
                revision INTEGER NOT NULL DEFAULT 1,
                external_enabled INTEGER NOT NULL DEFAULT 0,
                main_agent_config_json TEXT,
                main_agent_prompt TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
                CHECK (
                    (source = 'system' AND project_id IS NULL) OR
                    (source = 'project' AND project_id IS NOT NULL)
                )
            )
            "#,
            r#"
            CREATE TABLE workflow_attempts (
                id BLOB PRIMARY KEY,
                task_id BLOB NOT NULL,
                project_id BLOB NOT NULL,
                issue_id BLOB NOT NULL,
                workflow_id BLOB NOT NULL,
                latest_run_id BLOB,
                workspace_id BLOB,
                name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft',
                main_session_id BLOB,
                main_session_bound_at TEXT,
                definition_locked_at TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
                UNIQUE (workflow_id)
            )
            "#,
            r#"
            CREATE TABLE tasks (
                id BLOB PRIMARY KEY,
                project_id BLOB NOT NULL,
                issue_id BLOB NOT NULL,
                title TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
            )
            "#,
            r#"
            CREATE TABLE workflow_attempt_sources (
                attempt_id BLOB PRIMARY KEY,
                template_id BLOB NOT NULL,
                template_revision INTEGER NOT NULL
            )
            "#,
            r#"
            CREATE TABLE workflow_runs (
                id BLOB PRIMARY KEY,
                workflow_id BLOB NOT NULL,
                attempt_id BLOB,
                issue_id BLOB NOT NULL,
                workspace_id BLOB,
                trigger_source TEXT NOT NULL DEFAULT 'manual',
                input_text TEXT NOT NULL,
                graph_snapshot TEXT,
                output_text TEXT,
                status TEXT NOT NULL DEFAULT 'pending',
                started_at TEXT,
                finished_at TEXT,
                error_text TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
            )
            "#,
            r#"
            CREATE TABLE scheduled_tasks (
                id BLOB PRIMARY KEY,
                project_id BLOB NOT NULL,
                target_type TEXT NOT NULL CHECK (target_type IN ('workflow')),
                target_id BLOB NOT NULL,
                context_issue_id BLOB NOT NULL,
                name TEXT,
                enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
                schedule_kind TEXT NOT NULL CHECK (schedule_kind IN ('daily', 'weekly')),
                time_of_day TEXT NOT NULL,
                weekday INTEGER CHECK (weekday IS NULL OR weekday BETWEEN 1 AND 7),
                timezone TEXT NOT NULL,
                input_text TEXT NOT NULL DEFAULT '',
                concurrency_policy TEXT NOT NULL DEFAULT 'skip_if_running'
                    CHECK (concurrency_policy IN ('skip_if_running')),
                last_run_id BLOB,
                last_status TEXT NOT NULL DEFAULT 'idle',
                last_error TEXT,
                last_triggered_at TEXT,
                next_run_at TEXT,
                claim_expires_at TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
                UNIQUE (project_id, target_type, target_id),
                CHECK (
                    (schedule_kind = 'daily' AND weekday IS NULL)
                    OR (schedule_kind = 'weekly' AND weekday IS NOT NULL)
                )
            )
            "#,
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("create scheduled task test schema");
        }

        pool
    }

    async fn insert_scheduled_task_fixture(pool: &SqlitePool) -> ScheduledTaskFixtureIds {
        let project_id = Uuid::new_v4();
        let issue_id = Uuid::new_v4();
        let workflow_id = Uuid::new_v4();

        insert_project(pool, project_id).await;
        insert_issue(pool, project_id, issue_id).await;
        sqlx::query(
            r#"
            INSERT INTO workflows (id, source, project_id, name, description, graph_json)
            VALUES (?, 'project', ?, 'Project Flow', NULL, ?)
            "#,
        )
        .bind(workflow_id)
        .bind(project_id)
        .bind(valid_graph_json())
        .execute(pool)
        .await
        .expect("insert workflow");

        ScheduledTaskFixtureIds {
            project_id,
            issue_id,
            workflow_id,
        }
    }

    async fn insert_project(pool: &SqlitePool, project_id: Uuid) {
        sqlx::query("INSERT INTO projects (id, name) VALUES (?, 'Project')")
            .bind(project_id)
            .execute(pool)
            .await
            .expect("insert project");
    }

    async fn insert_issue(pool: &SqlitePool, project_id: Uuid, issue_id: Uuid) {
        sqlx::query("INSERT INTO local_issues (id, project_id, title) VALUES (?, ?, 'Task')")
            .bind(issue_id)
            .bind(project_id)
            .execute(pool)
            .await
            .expect("insert issue");
    }

    async fn insert_workflow_run(
        pool: &SqlitePool,
        run_id: Uuid,
        workflow_id: Uuid,
        issue_id: Uuid,
        status: &str,
    ) {
        sqlx::query(
            r#"
            INSERT INTO workflow_runs (id, workflow_id, issue_id, trigger_source, input_text, status)
            VALUES (?, ?, ?, 'schedule', 'input', ?)
            "#,
        )
        .bind(run_id)
        .bind(workflow_id)
        .bind(issue_id)
        .bind(status)
        .execute(pool)
        .await
        .expect("insert workflow run");
    }

    fn upsert_request(workflow_id: Uuid, issue_id: Uuid) -> UpsertScheduledTaskRequest {
        UpsertScheduledTaskRequest {
            target_type: ScheduledTaskTargetType::Workflow,
            target_id: workflow_id,
            context_issue_id: issue_id,
            name: Some(" Morning check ".to_string()),
            enabled: true,
            schedule_kind: ScheduledTaskKind::Daily,
            time_of_day: "09:00".to_string(),
            weekday: None,
            timezone: "Asia/Shanghai".to_string(),
            input_text: "Check status".to_string(),
        }
    }

    fn fake_workflow_dispatch(
        project_id: Uuid,
        issue_id: Uuid,
        workflow_id: Uuid,
        run_id: Uuid,
        status: WorkflowRunStatus,
    ) -> (WorkflowAttemptResponse, WorkflowRunResponse) {
        let now = Utc::now();
        let attempt_id = Uuid::new_v4();
        (
            WorkflowAttemptResponse {
                id: attempt_id,
                project_id,
                issue_id,
                workflow_id,
                latest_run_id: Some(run_id),
                template_id: None,
                main_session_id: None,
                main_session_bound_at: None,
                definition_locked_at: None,
                workspace_id: None,
                name: "Scheduled task run".to_string(),
                status: db::models::workflow::WorkflowAttemptStatus::Succeeded,
                created_at: now,
                updated_at: now,
            },
            WorkflowRunResponse {
                queue_phase: None,
                id: run_id,
                orchestration_run_id: None,
                workflow_id,
                attempt_id: Some(attempt_id),
                issue_id,
                workspace_id: None,
                trigger_source: "schedule_manual".to_string(),
                input_text: "Check status".to_string(),
                output_text: Some("Done".to_string()),
                status,
                started_at: Some(now),
                finished_at: Some(now),
                error_text: None,
                created_at: now,
                updated_at: now,
                nodes: Vec::new(),
                split_batches: Vec::new(),
                handoffs: Vec::new(),
                delivery: None,
                request_path: None,
                runtime_view: None,
            },
        )
    }

    fn valid_graph_json() -> String {
        json!({
            "version": 1,
            "nodes": [
                { "id": "start", "type": "start", "data": { "display_name": "Start" } },
                { "id": "end", "type": "end", "data": { "display_name": "End" } }
            ],
            "edges": [
                { "id": "e1", "source": "start", "target": "end", "type": "default" }
            ]
        })
        .to_string()
    }
}
