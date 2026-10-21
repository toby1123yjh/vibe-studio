use api_types::{
    CreateProjectRequest, CreateProjectStatusRequest, CreateTagRequest, CreateTaskAssigneeRequest,
    CreateTaskCommentReactionRequest, CreateTaskCommentRequest, CreateTaskFollowerRequest,
    CreateTaskRelationshipRequest, CreateTaskRequest, CreateTaskTagRequest, DeleteResponse,
    ListMembersResponse, ListOrganizationsResponse, MemberRole, MutationResponse,
    OrganizationMember, OrganizationMemberWithProfile, OrganizationWithRole, Project,
    ProjectStatus, Tag, Task, TaskAssignee, TaskComment, TaskCommentReaction, TaskFollower,
    TaskPriority, TaskRelationship, TaskRelationshipType, TaskTag, UpdateProjectRequest,
    UpdateProjectStatusRequest, UpdateTagRequest, UpdateTaskCommentRequest, UpdateTaskRequest,
    User, Workspace,
};
use axum::{
    Router,
    extract::{Json, Path, Query, State},
    response::Json as ResponseJson,
    routing::{delete, get, patch, post},
};
use chrono::{DateTime, Utc};
use db::models::{
    arena_group::{
        ArenaCandidate, ArenaCandidatePurpose, ArenaGroup, ArenaGroupError, ArenaLifecycleStatus,
        ArenaMode, ArenaStatus, CreateArenaCandidate, CreateArenaGroup,
    },
    execution_process::ExecutionProcessRunReason,
    requests::WorkspaceRepoInput,
    session::{CreateSession, Session},
    task::{CreateExecution, Execution, ExecutionKind},
    workspace::Workspace as DbWorkspace,
    workspace_repo::WorkspaceRepo,
};
use deployment::Deployment;
use executors::{
    profile::ExecutorConfig,
    runtime::{AgentRunStatus, AgentRuntimeMessageRole, RunState},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use services::services::container::ContainerService;
use sqlx::{Row, SqlitePool, sqlite::SqliteRow, types::Json as SqlxJson};
use ts_rs::TS;
use uuid::Uuid;

use crate::{
    DeploymentImpl,
    error::ApiError,
    routes::{scheduled_tasks, sessions, workflows, workspaces::create::create_workspace_record},
};

const LOCAL_PROJECT_COLOR: &str = "210 80% 52%";
pub(super) const DISCUSS_STATUS: (&str, &str, i32, bool) = ("Discuss", "215 12% 55%", 0, false);

pub(super) const DEFAULT_STATUSES: [(&str, &str, i32, bool); 6] = [
    DISCUSS_STATUS,
    ("Todo", "210 80% 52%", 100, false),
    ("In Progress", "38 92% 50%", 200, false),
    ("In Review", "265 70% 62%", 300, false),
    ("Done", "145 63% 42%", 400, false),
    ("Cancelled", "0 0% 50%", 500, true),
];

pub(super) const DEFAULT_TAGS: [(&str, &str); 4] = [
    ("bug", "355 65% 53%"),
    ("feature", "124 82% 30%"),
    ("documentation", "205 100% 40%"),
    ("enhancement", "181 72% 78%"),
];

const ARENA_SYNTHESIS_WORKSPACE_PREFIX: &str = "Arena Synthesis";

#[derive(Debug, Deserialize)]
struct OrganizationQuery {
    organization_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct ProjectQuery {
    project_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct TaskQuery {
    #[serde(rename = "task_id")]
    issue_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct UserQuery {
    user_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct BulkUpdateProjectItem {
    id: Uuid,
    #[serde(flatten)]
    changes: UpdateProjectRequest,
}

#[derive(Debug, Deserialize)]
struct BulkUpdateProjectsRequest {
    updates: Vec<BulkUpdateProjectItem>,
}

#[derive(Debug, Deserialize)]
struct BulkUpdateProjectStatusItem {
    id: Uuid,
    #[serde(flatten)]
    changes: UpdateProjectStatusRequest,
}

#[derive(Debug, Deserialize)]
struct BulkUpdateProjectStatusesRequest {
    updates: Vec<BulkUpdateProjectStatusItem>,
}

#[derive(Debug, Deserialize)]
struct BulkUpdateTaskItem {
    id: Uuid,
    #[serde(flatten)]
    changes: UpdateTaskRequest,
}

#[derive(Debug, Deserialize)]
struct BulkUpdateTasksRequest {
    updates: Vec<BulkUpdateTaskItem>,
}

pub fn router(deployment: &DeploymentImpl) -> Router<DeploymentImpl> {
    Router::new()
        .route("/v1/organizations", get(list_organizations))
        .route("/v1/organizations/{org_id}/members", get(list_members))
        .route(
            "/v1/fallback/organization_members",
            get(fallback_organization_members),
        )
        .route("/v1/fallback/users", get(fallback_users))
        .route("/v1/fallback/projects", get(fallback_projects))
        .route(
            "/v1/fallback/project_statuses",
            get(fallback_project_statuses),
        )
        .route("/v1/fallback/tasks", get(fallback_issues))
        .route("/v1/fallback/tags", get(fallback_tags))
        .route("/v1/fallback/task_tags", get(fallback_issue_tags))
        .route("/v1/fallback/task_assignees", get(fallback_issue_assignees))
        .route("/v1/fallback/task_followers", get(fallback_issue_followers))
        .route(
            "/v1/fallback/task_relationships",
            get(fallback_issue_relationships),
        )
        .route("/v1/fallback/pull_requests", get(fallback_pull_requests))
        .route(
            "/v1/fallback/pull_request_issues",
            get(fallback_pull_request_issues),
        )
        .route(
            "/v1/fallback/project_workspaces",
            get(fallback_project_workspaces),
        )
        .route(
            "/v1/fallback/user_workspaces",
            get(fallback_user_workspaces),
        )
        .route("/v1/fallback/notifications", get(fallback_notifications))
        .route("/v1/fallback/task_comments", get(fallback_issue_comments))
        .route(
            "/v1/fallback/task_comment_reactions",
            get(fallback_issue_comment_reactions),
        )
        .route("/v1/fallback/workflows", get(fallback_workflows))
        .route("/v1/fallback/workflow_runs", get(fallback_workflow_runs))
        .route(
            "/v1/fallback/node_executions",
            get(fallback_node_executions),
        )
        .route("/v1/projects", post(create_project))
        .route("/v1/projects/open", post(open_project))
        .route("/v1/projects/bulk", post(bulk_update_projects))
        .route(
            "/v1/projects/{project_id}",
            get(get_project)
                .patch(update_project)
                .delete(delete_project),
        )
        .route("/v1/project_statuses", post(create_project_status))
        .route(
            "/v1/project_statuses/bulk",
            post(bulk_update_project_statuses),
        )
        .route(
            "/v1/project_statuses/{status_id}",
            patch(update_project_status).delete(delete_project_status),
        )
        .route("/v1/tasks", post(create_issue))
        .route("/v1/tasks/bulk", post(bulk_update_issues))
        .route(
            "/v1/tasks/{issue_id}",
            patch(update_issue).delete(delete_issue),
        )
        .route("/v1/tags", post(create_tag))
        .route("/v1/tags/{tag_id}", patch(update_tag).delete(delete_tag))
        .route("/v1/task_tags", post(create_issue_tag))
        .route("/v1/task_tags/{issue_tag_id}", delete(delete_issue_tag))
        .route("/v1/task_assignees", post(create_issue_assignee))
        .route(
            "/v1/task_assignees/{issue_assignee_id}",
            delete(delete_issue_assignee),
        )
        .route("/v1/task_followers", post(create_issue_follower))
        .route(
            "/v1/task_followers/{issue_follower_id}",
            delete(delete_issue_follower),
        )
        .route("/v1/task_relationships", post(create_issue_relationship))
        .route(
            "/v1/task_relationships/{relationship_id}",
            delete(delete_issue_relationship),
        )
        .route("/v1/task_comments", post(create_issue_comment))
        .route(
            "/v1/task_comments/{comment_id}",
            patch(update_issue_comment).delete(delete_issue_comment),
        )
        .route(
            "/v1/task_comment_reactions",
            post(create_issue_comment_reaction),
        )
        .route(
            "/v1/task_comment_reactions/{reaction_id}",
            delete(delete_issue_comment_reaction),
        )
        // ── AI Arena (race mode) ────────────────────────────────────────
        // see docs/future/ai-arena/spec.md §4 + plan.md Step 1.3
        .route("/v1/fallback/arena_groups", get(fallback_arena_groups))
        .route(
            "/v1/tasks/{issue_id}/workspaces",
            get(list_issue_workspaces),
        )
        .route("/v1/tasks/{issue_id}/arena", post(create_arena_group))
        .route(
            "/v1/tasks/{issue_id}/arena/active",
            get(get_active_arena_for_issue),
        )
        .route(
            "/v1/arena/{group_id}",
            get(get_arena_group).delete(dissolve_arena_group),
        )
        .route("/v1/arena/{group_id}/close", post(close_arena_group))
        .route("/v1/arena/{group_id}/message", post(send_arena_message))
        .route(
            "/v1/arena/{group_id}/start-implementation",
            post(start_arena_implementation),
        )
        .route(
            "/v1/arena/{group_id}/promote",
            post(promote_arena_workspace),
        )
        .route(
            "/v1/arena/{group_id}/workspaces/{workspace_id}/retry",
            post(retry_arena_workspace),
        )
        .merge(workflows::router(deployment))
        .merge(scheduled_tasks::router(deployment))
}

fn local_user_id() -> Uuid {
    Uuid::from_u128(1)
}

fn local_org_id() -> Uuid {
    Uuid::from_u128(2)
}

fn txid() -> i64 {
    Utc::now().timestamp_millis()
}

fn priority_from_str(value: Option<String>) -> Option<TaskPriority> {
    match value.as_deref() {
        Some("urgent") => Some(TaskPriority::Urgent),
        Some("high") => Some(TaskPriority::High),
        Some("medium") => Some(TaskPriority::Medium),
        Some("low") => Some(TaskPriority::Low),
        _ => None,
    }
}

fn relationship_type_from_str(value: String) -> TaskRelationshipType {
    match value.as_str() {
        "blocking" => TaskRelationshipType::Blocking,
        "has_duplicate" => TaskRelationshipType::HasDuplicate,
        _ => TaskRelationshipType::Related,
    }
}

fn empty_rows(table: &str) -> ResponseJson<Value> {
    ResponseJson(json!({ table: [] }))
}

async fn ensure_project_metadata(pool: &SqlitePool) -> Result<(), ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT p.id
        FROM projects p
        LEFT JOIN local_project_metadata m ON m.project_id = p.id
        WHERE m.project_id IS NULL
        ORDER BY p.created_at ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    for row in rows {
        let project_id: Uuid = row.try_get("id")?;
        let sort_order: i32 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM local_project_metadata",
        )
        .fetch_one(pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO local_project_metadata
                (project_id, organization_id, color, sort_order)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(project_id)
        .bind(local_org_id())
        .bind(LOCAL_PROJECT_COLOR)
        .bind(sort_order)
        .execute(pool)
        .await?;
    }

    Ok(())
}

pub(super) fn project_from_row(row: &SqliteRow) -> Result<Project, sqlx::Error> {
    Ok(Project {
        id: row.try_get("id")?,
        organization_id: row.try_get("organization_id")?,
        name: row.try_get("name")?,
        color: row.try_get("color")?,
        sort_order: row.try_get("sort_order")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn list_local_projects(pool: &SqlitePool) -> Result<Vec<Project>, ApiError> {
    ensure_project_metadata(pool).await?;

    let rows = sqlx::query(
        r#"
        SELECT
            p.id,
            m.organization_id,
            p.name,
            m.color,
            m.sort_order,
            p.created_at,
            p.updated_at
        FROM projects p
        JOIN local_project_metadata m ON m.project_id = p.id
        WHERE m.organization_id = ?
        ORDER BY m.sort_order ASC, p.created_at ASC
        "#,
    )
    .bind(local_org_id())
    .fetch_all(pool)
    .await?;

    let mut projects = Vec::with_capacity(rows.len());
    for row in rows {
        let project = project_from_row(&row)?;
        let mounted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM project_task_mounts WHERE project_id=?)",
        )
        .bind(project.id)
        .fetch_one(pool)
        .await?;
        projects.push(if mounted {
            super::project_store::read(pool, project.id).await?.project
        } else {
            project
        });
    }
    projects.sort_by_key(|p| p.sort_order);
    Ok(projects)
}

pub(crate) async fn get_local_project(
    pool: &SqlitePool,
    project_id: Uuid,
) -> Result<Project, ApiError> {
    let mounted: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM project_task_mounts WHERE project_id=?)")
            .bind(project_id)
            .fetch_one(pool)
            .await?;
    if mounted {
        return Ok(super::project_store::read(pool, project_id).await?.project);
    }
    ensure_project_metadata(pool).await?;

    let row = sqlx::query(
        r#"
        SELECT
            p.id,
            m.organization_id,
            p.name,
            m.color,
            m.sort_order,
            p.created_at,
            p.updated_at
        FROM projects p
        JOIN local_project_metadata m ON m.project_id = p.id
        WHERE p.id = ?
        "#,
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::BadRequest("Project not found".to_string()))?;

    project_from_row(&row).map_err(ApiError::from)
}

async fn create_local_project(
    _pool: &SqlitePool,
    _request: CreateProjectRequest,
) -> Result<Project, ApiError> {
    Err(ApiError::BadRequest(
        "Choose a project directory and use the project open endpoint".into(),
    ))
}

async fn update_local_project(
    pool: &SqlitePool,
    project_id: Uuid,
    changes: UpdateProjectRequest,
) -> Result<Project, ApiError> {
    if project_id == db::models::project::DEFAULT_PROJECT_ID {
        return Err(ApiError::BadRequest(
            "The default project is read-only".into(),
        ));
    }
    super::project_store::mutate(pool, project_id, |document| {
        if let Some(name) = changes.name {
            document.project.name = name;
        }
        if let Some(color) = changes.color {
            document.project.color = color;
        }
        if let Some(sort_order) = changes.sort_order {
            document.project.sort_order = sort_order;
        }
        document.project.updated_at = Utc::now();
        Ok(document.project.clone())
    })
    .await
}

pub(super) fn status_from_row(row: &SqliteRow) -> Result<ProjectStatus, sqlx::Error> {
    Ok(ProjectStatus {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        name: row.try_get("name")?,
        color: row.try_get("color")?,
        sort_order: row.try_get("sort_order")?,
        hidden: row.try_get("hidden")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn list_project_statuses(
    pool: &SqlitePool,
    project_id: Uuid,
) -> Result<Vec<ProjectStatus>, ApiError> {
    if project_id == db::models::project::DEFAULT_PROJECT_ID {
        return Ok(vec![]);
    }
    let mut rows = super::project_store::read(pool, project_id).await?.statuses;
    rows.sort_by_key(|row| row.sort_order);
    Ok(rows)
}

async fn ensure_default_statuses(
    pool: &SqlitePool,
    project_id: Uuid,
) -> Result<Vec<ProjectStatus>, ApiError> {
    list_project_statuses(pool, project_id).await
}

async fn create_local_status(
    pool: &SqlitePool,
    request: CreateProjectStatusRequest,
) -> Result<ProjectStatus, ApiError> {
    super::project_store::mutate(pool, request.project_id, |document| {
        let row = ProjectStatus {
            id: request.id.unwrap_or_else(Uuid::new_v4),
            project_id: request.project_id,
            name: request.name,
            color: request.color,
            sort_order: request.sort_order,
            hidden: request.hidden,
            created_at: Utc::now(),
        };
        document.statuses.push(row.clone());
        Ok(row)
    })
    .await
}

async fn update_local_status(
    pool: &SqlitePool,
    status_id: Uuid,
    changes: UpdateProjectStatusRequest,
) -> Result<ProjectStatus, ApiError> {
    let owner = super::project_store::owner(pool, "statuses", status_id).await?;
    super::project_store::mutate(pool, owner, |document| {
        let row = document
            .statuses
            .iter_mut()
            .find(|row| row.id == status_id)
            .ok_or_else(|| ApiError::BadRequest("Project status not found".into()))?;
        if let Some(name) = changes.name {
            row.name = name;
        }
        if let Some(color) = changes.color {
            row.color = color;
        }
        if let Some(sort_order) = changes.sort_order {
            row.sort_order = sort_order;
        }
        if let Some(hidden) = changes.hidden {
            row.hidden = hidden;
        }
        Ok(row.clone())
    })
    .await
}

pub(super) fn issue_from_row(row: &SqliteRow) -> Result<Task, sqlx::Error> {
    let extension_metadata = row
        .try_get::<String, _>("extension_metadata")
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .unwrap_or(Value::Null);

    Ok(Task {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        issue_number: row.try_get("issue_number")?,
        simple_id: row.try_get("simple_id")?,
        status_id: row.try_get("status_id")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        priority: priority_from_str(row.try_get("priority")?),
        start_date: row.try_get("start_date")?,
        target_date: row.try_get("target_date")?,
        completed_at: row.try_get("completed_at")?,
        sort_order: row.try_get("sort_order")?,
        parent_issue_id: row.try_get("parent_issue_id")?,
        parent_issue_sort_order: row.try_get("parent_issue_sort_order")?,
        extension_metadata,
        creator_user_id: row.try_get("creator_user_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn list_project_issues(pool: &SqlitePool, project_id: Uuid) -> Result<Vec<Task>, ApiError> {
    if project_id == db::models::project::DEFAULT_PROJECT_ID {
        return Ok(vec![]);
    }
    let mut rows = super::project_store::read(pool, project_id).await?.tasks;
    rows.sort_by(|a, b| {
        a.sort_order
            .total_cmp(&b.sort_order)
            .then(a.created_at.cmp(&b.created_at))
    });
    Ok(rows)
}

pub(crate) async fn get_local_issue(pool: &SqlitePool, issue_id: Uuid) -> Result<Task, ApiError> {
    let owner = super::project_store::owner(pool, "tasks", issue_id).await?;
    super::project_store::read(pool, owner)
        .await?
        .tasks
        .into_iter()
        .find(|row| row.id == issue_id)
        .ok_or_else(|| ApiError::BadRequest("Record not found".into()))
}

async fn create_local_issue(
    pool: &SqlitePool,
    request: CreateTaskRequest,
) -> Result<Task, ApiError> {
    super::project_store::mutate(pool, request.project_id, |document| {
        let id = document.insert_task(request)?;
        document
            .tasks
            .iter()
            .find(|row| row.id == id)
            .cloned()
            .ok_or_else(|| ApiError::BadRequest("Execution not found".into()))
    })
    .await
}

pub(crate) async fn insert_local_issue(
    conn: &mut sqlx::SqliteConnection,
    request: CreateTaskRequest,
) -> Result<(Uuid, super::project_store::PreparedDocument), ApiError> {
    super::project_store::insert_task_in(conn, request).await
}

async fn update_local_issue(
    pool: &SqlitePool,
    issue_id: Uuid,
    changes: UpdateTaskRequest,
) -> Result<Task, ApiError> {
    let owner = super::project_store::owner(pool, "tasks", issue_id).await?;
    super::project_store::mutate(pool, owner, |document| {
        let row = document
            .tasks
            .iter_mut()
            .find(|row| row.id == issue_id)
            .ok_or_else(|| ApiError::BadRequest("Execution not found".into()))?;
        super::project_store::patch(row, changes)?;
        row.updated_at = Utc::now();
        Ok(row.clone())
    })
    .await
}

pub(super) fn tag_from_row(row: &SqliteRow) -> Result<Tag, sqlx::Error> {
    Ok(Tag {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        name: row.try_get("name")?,
        color: row.try_get("color")?,
    })
}

async fn list_project_tags(pool: &SqlitePool, project_id: Uuid) -> Result<Vec<Tag>, ApiError> {
    if project_id == db::models::project::DEFAULT_PROJECT_ID {
        return Ok(vec![]);
    }
    let mut rows = super::project_store::read(pool, project_id).await?.tags;
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

pub(super) fn issue_tag_from_row(row: &SqliteRow) -> Result<TaskTag, sqlx::Error> {
    Ok(TaskTag {
        id: row.try_get("id")?,
        issue_id: row.try_get("issue_id")?,
        tag_id: row.try_get("tag_id")?,
    })
}

async fn list_project_issue_tags(
    pool: &SqlitePool,
    project_id: Uuid,
) -> Result<Vec<TaskTag>, ApiError> {
    if project_id == db::models::project::DEFAULT_PROJECT_ID {
        return Ok(vec![]);
    }
    Ok(super::project_store::read(pool, project_id)
        .await?
        .task_tags)
}

pub(super) fn issue_assignee_from_row(row: &SqliteRow) -> Result<TaskAssignee, sqlx::Error> {
    Ok(TaskAssignee {
        id: row.try_get("id")?,
        issue_id: row.try_get("issue_id")?,
        user_id: row.try_get("user_id")?,
        assigned_at: row.try_get("assigned_at")?,
    })
}

async fn list_project_issue_assignees(
    pool: &SqlitePool,
    project_id: Uuid,
) -> Result<Vec<TaskAssignee>, ApiError> {
    if project_id == db::models::project::DEFAULT_PROJECT_ID {
        return Ok(vec![]);
    }
    Ok(super::project_store::read(pool, project_id)
        .await?
        .task_assignees)
}

pub(super) fn issue_follower_from_row(row: &SqliteRow) -> Result<TaskFollower, sqlx::Error> {
    Ok(TaskFollower {
        id: row.try_get("id")?,
        issue_id: row.try_get("issue_id")?,
        user_id: row.try_get("user_id")?,
    })
}

async fn list_project_issue_followers(
    pool: &SqlitePool,
    project_id: Uuid,
) -> Result<Vec<TaskFollower>, ApiError> {
    if project_id == db::models::project::DEFAULT_PROJECT_ID {
        return Ok(vec![]);
    }
    Ok(super::project_store::read(pool, project_id)
        .await?
        .task_followers)
}

pub(super) fn issue_relationship_from_row(
    row: &SqliteRow,
) -> Result<TaskRelationship, sqlx::Error> {
    Ok(TaskRelationship {
        id: row.try_get("id")?,
        issue_id: row.try_get("issue_id")?,
        related_issue_id: row.try_get("related_issue_id")?,
        relationship_type: relationship_type_from_str(row.try_get("relationship_type")?),
        created_at: row.try_get("created_at")?,
    })
}

async fn list_project_issue_relationships(
    pool: &SqlitePool,
    project_id: Uuid,
) -> Result<Vec<TaskRelationship>, ApiError> {
    if project_id == db::models::project::DEFAULT_PROJECT_ID {
        return Ok(vec![]);
    }
    Ok(super::project_store::read(pool, project_id)
        .await?
        .task_relationships)
}

fn issue_comment_from_row(row: &SqliteRow) -> Result<TaskComment, sqlx::Error> {
    Ok(TaskComment {
        id: row.try_get("id")?,
        issue_id: row.try_get("issue_id")?,
        author_id: row.try_get("author_id")?,
        parent_id: row.try_get("parent_id")?,
        message: row.try_get("message")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn list_issue_comments(
    pool: &SqlitePool,
    issue_id: Uuid,
) -> Result<Vec<TaskComment>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT id, issue_id, author_id, parent_id, message, created_at, updated_at
        FROM local_issue_comments
        WHERE issue_id = ?
        ORDER BY created_at ASC
        "#,
    )
    .bind(issue_id)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(issue_comment_from_row)
        .collect::<Result<Vec<_>, _>>()
        .map_err(ApiError::from)
}

async fn get_issue_comment(pool: &SqlitePool, comment_id: Uuid) -> Result<TaskComment, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT id, issue_id, author_id, parent_id, message, created_at, updated_at
        FROM local_issue_comments
        WHERE id = ?
        "#,
    )
    .bind(comment_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::BadRequest("Task comment not found".to_string()))?;

    issue_comment_from_row(&row).map_err(ApiError::from)
}

fn issue_comment_reaction_from_row(row: &SqliteRow) -> Result<TaskCommentReaction, sqlx::Error> {
    Ok(TaskCommentReaction {
        id: row.try_get("id")?,
        comment_id: row.try_get("comment_id")?,
        user_id: row.try_get("user_id")?,
        emoji: row.try_get("emoji")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn list_issue_comment_reactions(
    pool: &SqlitePool,
    issue_id: Uuid,
) -> Result<Vec<TaskCommentReaction>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT r.id, r.comment_id, r.user_id, r.emoji, r.created_at
        FROM local_issue_comment_reactions r
        JOIN local_issue_comments c ON c.id = r.comment_id
        WHERE c.issue_id = ?
        ORDER BY r.created_at ASC
        "#,
    )
    .bind(issue_id)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(issue_comment_reaction_from_row)
        .collect::<Result<Vec<_>, _>>()
        .map_err(ApiError::from)
}

async fn get_issue_comment_reaction(
    pool: &SqlitePool,
    reaction_id: Uuid,
) -> Result<TaskCommentReaction, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT id, comment_id, user_id, emoji, created_at
        FROM local_issue_comment_reactions
        WHERE id = ?
        "#,
    )
    .bind(reaction_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::BadRequest("Task comment reaction not found".to_string()))?;

    issue_comment_reaction_from_row(&row).map_err(ApiError::from)
}

async fn list_project_workspaces(
    pool: &SqlitePool,
    project_id: Uuid,
) -> Result<Vec<Workspace>, ApiError> {
    let workspaces = sqlx::query_as::<_, Workspace>(
        r#"
        WITH workspace_scope AS (
            SELECT session.workspace_id, task.project_id, task.issue_id,
                   task.updated_at AS task_updated_at
            FROM tasks task
            JOIN agent_task_bindings binding ON binding.task_id = task.id
            JOIN sessions session ON session.id = binding.session_id
            UNION ALL
            SELECT attempt.workspace_id, task.project_id, task.issue_id,
                   task.updated_at
            FROM tasks task
            JOIN workflow_attempts attempt ON attempt.task_id = task.id
            WHERE attempt.workspace_id IS NOT NULL
            UNION ALL
            SELECT candidate.workspace_id, task.project_id, task.issue_id,
                   task.updated_at
            FROM tasks task
            JOIN arena_groups arena ON arena.task_id = task.id
            JOIN arena_candidates candidate ON candidate.arena_group_id = arena.id
        ), ranked_scope AS (
            SELECT *, ROW_NUMBER() OVER (
                PARTITION BY workspace_id
                ORDER BY task_updated_at DESC, issue_id ASC
            ) AS row_number
            FROM workspace_scope
        )
        SELECT
            w.id,
            scope.project_id,
            ? AS owner_user_id,
            scope.issue_id,
            w.id AS local_workspace_id,
            w.name,
            w.archived,
            NULL AS files_changed,
            NULL AS lines_added,
            NULL AS lines_removed,
            w.created_at,
            w.updated_at
        FROM workspaces w
        JOIN ranked_scope scope
          ON scope.workspace_id = w.id AND scope.row_number = 1
        WHERE scope.project_id = ?
        ORDER BY w.updated_at DESC
        "#,
    )
    .bind(local_user_id())
    .bind(project_id)
    .fetch_all(pool)
    .await?;

    Ok(workspaces)
}

async fn list_user_workspaces(pool: &SqlitePool) -> Result<Vec<Workspace>, ApiError> {
    let workspaces = sqlx::query_as::<_, Workspace>(
        r#"
        WITH workspace_scope AS (
            SELECT session.workspace_id, task.project_id, task.issue_id,
                   task.updated_at AS task_updated_at
            FROM tasks task
            JOIN agent_task_bindings binding ON binding.task_id = task.id
            JOIN sessions session ON session.id = binding.session_id
            UNION ALL
            SELECT attempt.workspace_id, task.project_id, task.issue_id,
                   task.updated_at
            FROM tasks task
            JOIN workflow_attempts attempt ON attempt.task_id = task.id
            WHERE attempt.workspace_id IS NOT NULL
            UNION ALL
            SELECT candidate.workspace_id, task.project_id, task.issue_id,
                   task.updated_at
            FROM tasks task
            JOIN arena_groups arena ON arena.task_id = task.id
            JOIN arena_candidates candidate ON candidate.arena_group_id = arena.id
        ), ranked_scope AS (
            SELECT *, ROW_NUMBER() OVER (
                PARTITION BY workspace_id
                ORDER BY task_updated_at DESC, issue_id ASC
            ) AS row_number
            FROM workspace_scope
        )
        SELECT
            w.id,
            scope.project_id,
            ? AS owner_user_id,
            scope.issue_id,
            w.id AS local_workspace_id,
            w.name,
            w.archived,
            NULL AS files_changed,
            NULL AS lines_added,
            NULL AS lines_removed,
            w.created_at,
            w.updated_at
        FROM workspaces w
        JOIN ranked_scope scope
          ON scope.workspace_id = w.id AND scope.row_number = 1
        ORDER BY w.updated_at DESC
        "#,
    )
    .bind(local_user_id())
    .fetch_all(pool)
    .await?;

    Ok(workspaces)
}

async fn list_organizations() -> ResponseJson<ListOrganizationsResponse> {
    let now = Utc::now();
    ResponseJson(ListOrganizationsResponse {
        organizations: vec![OrganizationWithRole {
            id: local_org_id(),
            name: "Local".to_string(),
            slug: "local".to_string(),
            is_personal: true,
            issue_prefix: "LOCAL".to_string(),
            created_at: now,
            updated_at: now,
            user_role: MemberRole::Admin,
        }],
    })
}

async fn list_members(Path(_org_id): Path<Uuid>) -> ResponseJson<ListMembersResponse> {
    ResponseJson(ListMembersResponse {
        members: vec![OrganizationMemberWithProfile {
            user_id: local_user_id(),
            role: MemberRole::Admin,
            joined_at: Utc::now(),
            first_name: Some("Local".to_string()),
            last_name: Some("User".to_string()),
            username: Some("local".to_string()),
            email: Some("local@vibe-kanban.local".to_string()),
            avatar_url: None,
        }],
    })
}

async fn fallback_organization_members(
    Query(query): Query<OrganizationQuery>,
) -> ResponseJson<Value> {
    if query.organization_id != Some(local_org_id()) {
        return empty_rows("organization_member_metadata");
    }

    ResponseJson(json!({
        "organization_member_metadata": [OrganizationMember {
            organization_id: local_org_id(),
            user_id: local_user_id(),
            role: MemberRole::Admin,
            joined_at: Utc::now(),
            last_seen_at: None,
        }]
    }))
}

async fn fallback_users(Query(query): Query<OrganizationQuery>) -> ResponseJson<Value> {
    if query.organization_id != Some(local_org_id()) {
        return empty_rows("users");
    }

    ResponseJson(json!({
        "users": [User {
            id: local_user_id(),
            email: "local@vibe-kanban.local".to_string(),
            first_name: Some("Local".to_string()),
            last_name: Some("User".to_string()),
            username: Some("local".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }]
    }))
}

async fn fallback_projects(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<OrganizationQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    if query.organization_id != Some(local_org_id()) {
        return Ok(empty_rows("projects"));
    }

    let projects = list_local_projects(&deployment.db().pool).await?;
    Ok(ResponseJson(json!({ "projects": projects })))
}

async fn fallback_project_statuses(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    let Some(project_id) = query.project_id else {
        return Ok(empty_rows("project_statuses"));
    };

    let statuses = ensure_default_statuses(&deployment.db().pool, project_id).await?;
    Ok(ResponseJson(json!({ "project_statuses": statuses })))
}

async fn fallback_issues(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    let Some(project_id) = query.project_id else {
        return Ok(empty_rows("tasks"));
    };

    let issues = list_project_issues(&deployment.db().pool, project_id).await?;
    Ok(ResponseJson(json!({ "tasks": issues })))
}

async fn fallback_tags(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    let Some(project_id) = query.project_id else {
        return Ok(empty_rows("tags"));
    };

    let tags = list_project_tags(&deployment.db().pool, project_id).await?;
    Ok(ResponseJson(json!({ "tags": tags })))
}

async fn fallback_issue_tags(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    let Some(project_id) = query.project_id else {
        return Ok(empty_rows("task_tags"));
    };

    let issue_tags = list_project_issue_tags(&deployment.db().pool, project_id).await?;
    Ok(ResponseJson(json!({ "task_tags": issue_tags })))
}

async fn fallback_issue_assignees(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    let Some(project_id) = query.project_id else {
        return Ok(empty_rows("task_assignees"));
    };

    let issue_assignees = list_project_issue_assignees(&deployment.db().pool, project_id).await?;
    Ok(ResponseJson(json!({ "task_assignees": issue_assignees })))
}

async fn fallback_issue_followers(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    let Some(project_id) = query.project_id else {
        return Ok(empty_rows("task_followers"));
    };

    let issue_followers = list_project_issue_followers(&deployment.db().pool, project_id).await?;
    Ok(ResponseJson(json!({ "task_followers": issue_followers })))
}

async fn fallback_issue_relationships(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    let Some(project_id) = query.project_id else {
        return Ok(empty_rows("task_relationships"));
    };

    let issue_relationships =
        list_project_issue_relationships(&deployment.db().pool, project_id).await?;
    Ok(ResponseJson(
        json!({ "task_relationships": issue_relationships }),
    ))
}

async fn fallback_pull_requests() -> ResponseJson<Value> {
    empty_rows("pull_requests")
}

async fn fallback_pull_request_issues() -> ResponseJson<Value> {
    empty_rows("pull_request_issues")
}

async fn fallback_project_workspaces(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    let Some(project_id) = query.project_id else {
        return Ok(empty_rows("workspaces"));
    };

    let workspaces = list_project_workspaces(&deployment.db().pool, project_id).await?;
    Ok(ResponseJson(json!({ "workspaces": workspaces })))
}

async fn fallback_user_workspaces(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<UserQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    if query
        .user_id
        .is_some_and(|user_id| user_id != local_user_id())
    {
        return Ok(empty_rows("workspaces"));
    }

    let workspaces = list_user_workspaces(&deployment.db().pool).await?;
    Ok(ResponseJson(json!({ "workspaces": workspaces })))
}

async fn fallback_notifications() -> ResponseJson<Value> {
    empty_rows("notifications")
}

async fn fallback_issue_comments(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<TaskQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    let Some(issue_id) = query.issue_id else {
        return Ok(empty_rows("task_comments"));
    };

    let issue_comments = list_issue_comments(&deployment.db().pool, issue_id).await?;
    Ok(ResponseJson(json!({ "task_comments": issue_comments })))
}

async fn fallback_issue_comment_reactions(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<TaskQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    let Some(issue_id) = query.issue_id else {
        return Ok(empty_rows("task_comment_reactions"));
    };

    let issue_comment_reactions =
        list_issue_comment_reactions(&deployment.db().pool, issue_id).await?;
    Ok(ResponseJson(
        json!({ "task_comment_reactions": issue_comment_reactions }),
    ))
}

async fn fallback_workflows(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<workflows::FallbackWorkflowsQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    Ok(ResponseJson(
        workflows::fallback_workflows_payload(&deployment.db().pool, query.project_id).await?,
    ))
}

async fn fallback_workflow_runs(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<workflows::FallbackWorkflowRunsQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    Ok(ResponseJson(
        workflows::fallback_workflow_runs_payload(
            &deployment.db().pool,
            query.issue_id,
            query.workflow_id,
        )
        .await?,
    ))
}

async fn fallback_node_executions(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<workflows::FallbackNodeExecutionsQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    Ok(ResponseJson(
        workflows::fallback_node_executions_payload(&deployment.db().pool, query.run_id).await?,
    ))
}

#[derive(Debug, Deserialize)]
struct OpenProjectRequest {
    directory_path: String,
    name: String,
    color: String,
    id: Option<Uuid>,
}

async fn open_project(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<OpenProjectRequest>,
) -> Result<ResponseJson<MutationResponse<Project>>, ApiError> {
    let data = super::project_store::open(
        &deployment.db().pool,
        &request.directory_path,
        CreateProjectRequest {
            id: request.id,
            organization_id: local_org_id(),
            name: request.name,
            color: request.color,
        },
    )
    .await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn create_project(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateProjectRequest>,
) -> Result<ResponseJson<MutationResponse<Project>>, ApiError> {
    let data = create_local_project(&deployment.db().pool, request).await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn get_project(
    State(deployment): State<DeploymentImpl>,
    Path(project_id): Path<Uuid>,
) -> Result<ResponseJson<Project>, ApiError> {
    Ok(ResponseJson(
        get_local_project(&deployment.db().pool, project_id).await?,
    ))
}

async fn update_project(
    State(deployment): State<DeploymentImpl>,
    Path(project_id): Path<Uuid>,
    Json(changes): Json<UpdateProjectRequest>,
) -> Result<ResponseJson<MutationResponse<Project>>, ApiError> {
    let data = update_local_project(&deployment.db().pool, project_id, changes).await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn bulk_update_projects(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<BulkUpdateProjectsRequest>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    for update in request.updates {
        update_local_project(&deployment.db().pool, update.id, update.changes).await?;
    }

    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn delete_project(
    State(deployment): State<DeploymentImpl>,
    Path(project_id): Path<Uuid>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    super::project_store::delete(&deployment.db().pool, project_id).await?;
    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn create_project_status(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateProjectStatusRequest>,
) -> Result<ResponseJson<MutationResponse<ProjectStatus>>, ApiError> {
    let data = create_local_status(&deployment.db().pool, request).await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn update_project_status(
    State(deployment): State<DeploymentImpl>,
    Path(status_id): Path<Uuid>,
    Json(changes): Json<UpdateProjectStatusRequest>,
) -> Result<ResponseJson<MutationResponse<ProjectStatus>>, ApiError> {
    let data = update_local_status(&deployment.db().pool, status_id, changes).await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn bulk_update_project_statuses(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<BulkUpdateProjectStatusesRequest>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    for update in request.updates {
        update_local_status(&deployment.db().pool, update.id, update.changes).await?;
    }

    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn delete_project_status(
    State(deployment): State<DeploymentImpl>,
    Path(status_id): Path<Uuid>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    let pool = &deployment.db().pool;
    let owner = super::project_store::owner(pool, "statuses", status_id).await?;
    super::project_store::mutate(pool, owner, |document| {
        document.statuses.retain(|row| row.id != status_id);
        Ok(())
    })
    .await?;
    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn create_issue(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateTaskRequest>,
) -> Result<ResponseJson<MutationResponse<Task>>, ApiError> {
    let data = create_local_issue(&deployment.db().pool, request).await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn update_issue(
    State(deployment): State<DeploymentImpl>,
    Path(issue_id): Path<Uuid>,
    Json(changes): Json<UpdateTaskRequest>,
) -> Result<ResponseJson<MutationResponse<Task>>, ApiError> {
    let data = update_local_issue(&deployment.db().pool, issue_id, changes).await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn bulk_update_issues(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<BulkUpdateTasksRequest>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    for update in request.updates {
        update_local_issue(&deployment.db().pool, update.id, update.changes).await?;
    }

    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn delete_issue(
    State(deployment): State<DeploymentImpl>,
    Path(issue_id): Path<Uuid>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    let pool = &deployment.db().pool;
    let owner = super::project_store::owner(pool, "tasks", issue_id).await?;
    super::project_store::mutate(pool, owner, |document| {
        document.tasks.retain(|row| row.id != issue_id);
        document
            .tasks
            .iter_mut()
            .filter(|row| row.parent_issue_id == Some(issue_id))
            .for_each(|row| {
                row.parent_issue_id = None;
                row.parent_issue_sort_order = None;
            });
        document.task_tags.retain(|row| row.issue_id != issue_id);
        document
            .task_assignees
            .retain(|row| row.issue_id != issue_id);
        document
            .task_followers
            .retain(|row| row.issue_id != issue_id);
        document
            .task_relationships
            .retain(|row| row.issue_id != issue_id && row.related_issue_id != issue_id);
        Ok(())
    })
    .await?;
    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn create_tag(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateTagRequest>,
) -> Result<ResponseJson<MutationResponse<Tag>>, ApiError> {
    let data =
        super::project_store::mutate(&deployment.db().pool, request.project_id, |document| {
            let row = Tag {
                id: request.id.unwrap_or_else(Uuid::new_v4),
                project_id: request.project_id,
                name: request.name,
                color: request.color,
            };
            document.tags.push(row.clone());
            Ok(row)
        })
        .await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn update_tag(
    State(deployment): State<DeploymentImpl>,
    Path(tag_id): Path<Uuid>,
    Json(changes): Json<UpdateTagRequest>,
) -> Result<ResponseJson<MutationResponse<Tag>>, ApiError> {
    let pool = &deployment.db().pool;
    let owner = super::project_store::owner(pool, "tags", tag_id).await?;
    let data = super::project_store::mutate(pool, owner, |document| {
        let row = document
            .tags
            .iter_mut()
            .find(|row| row.id == tag_id)
            .ok_or_else(|| ApiError::BadRequest("Tag not found".into()))?;
        if let Some(name) = changes.name {
            row.name = name;
        }
        if let Some(color) = changes.color {
            row.color = color;
        }
        Ok(row.clone())
    })
    .await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn delete_tag(
    State(deployment): State<DeploymentImpl>,
    Path(tag_id): Path<Uuid>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    let pool = &deployment.db().pool;
    let owner = super::project_store::owner(pool, "tags", tag_id).await?;
    super::project_store::mutate(pool, owner, |document| {
        document.tags.retain(|row| row.id != tag_id);
        document.task_tags.retain(|row| row.tag_id != tag_id);
        Ok(())
    })
    .await?;
    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn create_issue_tag(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateTaskTagRequest>,
) -> Result<ResponseJson<MutationResponse<TaskTag>>, ApiError> {
    let pool = &deployment.db().pool;
    let owner = super::project_store::owner(pool, "tasks", request.issue_id).await?;
    let data = super::project_store::mutate(pool, owner, |document| {
        if let Some(row) = document
            .task_tags
            .iter()
            .find(|row| row.issue_id == request.issue_id && row.tag_id == request.tag_id)
        {
            return Ok(row.clone());
        }
        let row = TaskTag {
            id: request.id.unwrap_or_else(Uuid::new_v4),
            issue_id: request.issue_id,
            tag_id: request.tag_id,
        };
        document.task_tags.push(row.clone());
        Ok(row)
    })
    .await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn delete_issue_tag(
    State(deployment): State<DeploymentImpl>,
    Path(issue_tag_id): Path<Uuid>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    let pool = &deployment.db().pool;
    let owner = super::project_store::owner(pool, "task_tags", issue_tag_id).await?;
    super::project_store::mutate(pool, owner, |document| {
        document.task_tags.retain(|row| row.id != issue_tag_id);
        Ok(())
    })
    .await?;
    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn create_issue_assignee(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateTaskAssigneeRequest>,
) -> Result<ResponseJson<MutationResponse<TaskAssignee>>, ApiError> {
    let pool = &deployment.db().pool;
    let owner = super::project_store::owner(pool, "tasks", request.issue_id).await?;
    let data = super::project_store::mutate(pool, owner, |document| {
        if let Some(row) = document
            .task_assignees
            .iter()
            .find(|row| row.issue_id == request.issue_id && row.user_id == request.user_id)
        {
            return Ok(row.clone());
        }
        let row = TaskAssignee {
            id: request.id.unwrap_or_else(Uuid::new_v4),
            issue_id: request.issue_id,
            user_id: request.user_id,
            assigned_at: Utc::now(),
        };
        document.task_assignees.push(row.clone());
        Ok(row)
    })
    .await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn delete_issue_assignee(
    State(deployment): State<DeploymentImpl>,
    Path(issue_assignee_id): Path<Uuid>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    let pool = &deployment.db().pool;
    let owner = super::project_store::owner(pool, "task_assignees", issue_assignee_id).await?;
    super::project_store::mutate(pool, owner, |document| {
        document
            .task_assignees
            .retain(|row| row.id != issue_assignee_id);
        Ok(())
    })
    .await?;
    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn create_issue_follower(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateTaskFollowerRequest>,
) -> Result<ResponseJson<MutationResponse<TaskFollower>>, ApiError> {
    let pool = &deployment.db().pool;
    let owner = super::project_store::owner(pool, "tasks", request.issue_id).await?;
    let data = super::project_store::mutate(pool, owner, |document| {
        if let Some(row) = document
            .task_followers
            .iter()
            .find(|row| row.issue_id == request.issue_id && row.user_id == request.user_id)
        {
            return Ok(row.clone());
        }
        let row = TaskFollower {
            id: request.id.unwrap_or_else(Uuid::new_v4),
            issue_id: request.issue_id,
            user_id: request.user_id,
        };
        document.task_followers.push(row.clone());
        Ok(row)
    })
    .await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn delete_issue_follower(
    State(deployment): State<DeploymentImpl>,
    Path(issue_follower_id): Path<Uuid>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    let pool = &deployment.db().pool;
    let owner = super::project_store::owner(pool, "task_followers", issue_follower_id).await?;
    super::project_store::mutate(pool, owner, |document| {
        document
            .task_followers
            .retain(|row| row.id != issue_follower_id);
        Ok(())
    })
    .await?;
    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn create_issue_relationship(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateTaskRelationshipRequest>,
) -> Result<ResponseJson<MutationResponse<TaskRelationship>>, ApiError> {
    let pool = &deployment.db().pool;
    let owner = super::project_store::owner(pool, "tasks", request.issue_id).await?;
    let data = super::project_store::mutate(pool, owner, |document| {
        if let Some(row) = document.task_relationships.iter().find(|row| {
            row.issue_id == request.issue_id
                && row.related_issue_id == request.related_issue_id
                && row.relationship_type == request.relationship_type
        }) {
            return Ok(row.clone());
        }
        let row = TaskRelationship {
            id: request.id.unwrap_or_else(Uuid::new_v4),
            issue_id: request.issue_id,
            related_issue_id: request.related_issue_id,
            relationship_type: request.relationship_type,
            created_at: Utc::now(),
        };
        document.task_relationships.push(row.clone());
        Ok(row)
    })
    .await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn delete_issue_relationship(
    State(deployment): State<DeploymentImpl>,
    Path(relationship_id): Path<Uuid>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    let pool = &deployment.db().pool;
    let owner = super::project_store::owner(pool, "task_relationships", relationship_id).await?;
    super::project_store::mutate(pool, owner, |document| {
        document
            .task_relationships
            .retain(|row| row.id != relationship_id);
        Ok(())
    })
    .await?;
    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn create_issue_comment(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateTaskCommentRequest>,
) -> Result<ResponseJson<MutationResponse<TaskComment>>, ApiError> {
    let id = request.id.unwrap_or_else(Uuid::new_v4);
    sqlx::query(
        r#"
        INSERT INTO local_issue_comments
            (id, issue_id, author_id, parent_id, message)
        VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(id)
    .bind(request.issue_id)
    .bind(local_user_id())
    .bind(request.parent_id)
    .bind(request.message)
    .execute(&deployment.db().pool)
    .await?;

    let data = get_issue_comment(&deployment.db().pool, id).await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn update_issue_comment(
    State(deployment): State<DeploymentImpl>,
    Path(comment_id): Path<Uuid>,
    Json(changes): Json<UpdateTaskCommentRequest>,
) -> Result<ResponseJson<MutationResponse<TaskComment>>, ApiError> {
    let existing = get_issue_comment(&deployment.db().pool, comment_id).await?;
    sqlx::query(
        r#"
        UPDATE local_issue_comments
        SET message = ?, parent_id = ?, updated_at = datetime('now', 'subsec')
        WHERE id = ?
        "#,
    )
    .bind(changes.message.unwrap_or(existing.message))
    .bind(changes.parent_id.unwrap_or(existing.parent_id))
    .bind(comment_id)
    .execute(&deployment.db().pool)
    .await?;

    let data = get_issue_comment(&deployment.db().pool, comment_id).await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn delete_issue_comment(
    State(deployment): State<DeploymentImpl>,
    Path(comment_id): Path<Uuid>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    sqlx::query("DELETE FROM local_issue_comments WHERE id = ?")
        .bind(comment_id)
        .execute(&deployment.db().pool)
        .await?;
    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

async fn create_issue_comment_reaction(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<CreateTaskCommentReactionRequest>,
) -> Result<ResponseJson<MutationResponse<TaskCommentReaction>>, ApiError> {
    let id = request.id.unwrap_or_else(Uuid::new_v4);
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO local_issue_comment_reactions
            (id, comment_id, user_id, emoji)
        VALUES (?, ?, ?, ?)
        "#,
    )
    .bind(id)
    .bind(request.comment_id)
    .bind(local_user_id())
    .bind(request.emoji)
    .execute(&deployment.db().pool)
    .await?;

    let data = get_issue_comment_reaction(&deployment.db().pool, id).await?;
    Ok(ResponseJson(MutationResponse { data, txid: txid() }))
}

async fn delete_issue_comment_reaction(
    State(deployment): State<DeploymentImpl>,
    Path(reaction_id): Path<Uuid>,
) -> Result<ResponseJson<DeleteResponse>, ApiError> {
    sqlx::query("DELETE FROM local_issue_comment_reactions WHERE id = ?")
        .bind(reaction_id)
        .execute(&deployment.db().pool)
        .await?;
    Ok(ResponseJson(DeleteResponse { txid: txid() }))
}

// ─────────────────────────────────────────────────────────────────────
// AI Arena (race mode)
//
// One arena_group represents N parallel attempts at a single local
// kanban issue, each running in its own workspace + worktree under a
// different executor. See docs/future/ai-arena/spec.md for the full
// design and notes-step0.md §2 / §5 for the rationale behind the
// promote-via-archived-flag pattern (it triggers the existing 1h
// accelerated cleanup path in `find_expired_for_cleanup`).
// ─────────────────────────────────────────────────────────────────────

const ARENA_MIN_ATTEMPTS: usize = 2;
const ARENA_MAX_ATTEMPTS: usize = 6;

/// Per-project ceiling for parallel arena attempts. Falls back to the
/// global hard limit (`ARENA_MAX_ATTEMPTS`) when the row hasn't been
/// migrated yet (e.g. very old DBs predating the `arena_max_workspaces`
/// column).
async fn arena_max_for_project(pool: &SqlitePool, project_id: Uuid) -> Result<usize, ApiError> {
    let row: Option<i64> = sqlx::query_scalar(
        r#"SELECT arena_max_workspaces
             FROM local_project_metadata
            WHERE project_id = ?"#,
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?;

    let project_cap = row
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(ARENA_MAX_ATTEMPTS);

    Ok(project_cap.clamp(ARENA_MIN_ATTEMPTS, ARENA_MAX_ATTEMPTS))
}

/// One executor slot in a `CreateArenaRequest`. Mirrors the structure
/// of `CreateAndStartWorkspaceRequest` for a single attempt: a fully
/// resolved `ExecutorConfig`, an optional per-attempt name, and an
/// optional per-attempt prompt override (defaults to the group's
/// shared `prompt`).
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct ArenaAttemptInput {
    pub executor_config: ExecutorConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional per-attempt prompt override. Falls back to the
    /// group-level `prompt` when not provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct CreateArenaRequest {
    pub project_id: Uuid,
    pub base_branch: String,
    pub prompt: String,
    #[serde(default)]
    pub mode: ArenaMode,
    pub repos: Vec<WorkspaceRepoInput>,
    pub attempts: Vec<ArenaAttemptInput>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ArenaWorkspaceSummary {
    pub candidate_id: Uuid,
    pub workspace_id: Uuid,
    pub session_id: Option<Uuid>,
    pub name: Option<String>,
    pub branch: String,
    pub purpose: ArenaCandidatePurpose,
    pub arena_status: ArenaStatus,
    pub executor: Option<String>,
    pub variant: Option<String>,
    pub latest_agent_run_status: Option<AgentRunStatus>,
    pub has_uncommitted_changes: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, sqlx::Type)]
#[sqlx(type_name = "arena_event_kind", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ArenaEventKind {
    AskAll,
    Workspace,
    Challenge,
    Synthesize,
    StartImplementation,
}

#[derive(Debug, Clone, Serialize, TS, sqlx::FromRow)]
pub struct ArenaEvent {
    pub id: Uuid,
    pub arena_group_id: Uuid,
    pub kind: ArenaEventKind,
    pub prompt: String,
    pub source_workspace_id: Option<Uuid>,
    pub target_workspace_id: Option<Uuid>,
    pub synthesis_workspace_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ArenaGroupResponse {
    #[serde(flatten)]
    pub group: ArenaGroup,
    pub workspaces: Vec<ArenaWorkspaceSummary>,
    pub events: Vec<ArenaEvent>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct PromoteArenaRequest {
    pub candidate_id: Uuid,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct RetryArenaRequest {
    pub executor_config: ExecutorConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional override; falls back to the group's prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct DissolveArenaResponse {
    pub group_id: Uuid,
    pub workspaces_archived: usize,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct CloseArenaResponse {
    pub group_id: Uuid,
    pub lifecycle_status: ArenaLifecycleStatus,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct StartArenaImplementationRequest {
    pub candidate_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_config: Option<ExecutorConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArenaMessageTarget {
    All,
    Workspace {
        workspace_id: Uuid,
    },
    Challenge {
        responder_workspace_id: Uuid,
        source_workspace_id: Uuid,
    },
    Synthesize {
        #[serde(default)]
        options: ArenaSynthesizeOptions,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ArenaSynthesizeOptions {
    pub include_original_prompt: bool,
    pub include_attempt_summaries: bool,
    pub include_activity: bool,
}

impl Default for ArenaSynthesizeOptions {
    fn default() -> Self {
        Self {
            include_original_prompt: true,
            include_attempt_summaries: true,
            include_activity: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct ArenaMessageRequest {
    pub target: ArenaMessageTarget,
    pub prompt: String,
    pub executor_config: ExecutorConfig,
    #[serde(default)]
    pub executor_configs: Vec<ArenaWorkspaceExecutorConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
pub struct ArenaWorkspaceExecutorConfig {
    pub workspace_id: Uuid,
    pub executor_config: ExecutorConfig,
}

/// Verify the issue exists and belongs to the project.
async fn ensure_issue_in_project(
    pool: &SqlitePool,
    issue_id: Uuid,
    project_id: Uuid,
) -> Result<(), ApiError> {
    let document = super::project_store::read(pool, project_id).await?;
    if !document.tasks.iter().any(|task| task.id == issue_id) {
        return Err(ApiError::BadRequest(format!(
            "Task {issue_id} does not belong to project {project_id}"
        )));
    }
    Ok(())
}

fn find_candidate_workspace_in_group<'workspace, 'candidate>(
    workspaces: &'workspace [DbWorkspace],
    candidates: &'candidate [ArenaCandidate],
    group_id: Uuid,
    workspace_id: Uuid,
) -> Result<(&'workspace DbWorkspace, &'candidate ArenaCandidate), ArenaGroupError> {
    let candidate = candidates
        .iter()
        .find(|candidate| {
            candidate.arena_group_id == group_id && candidate.workspace_id == workspace_id
        })
        .ok_or(ArenaGroupError::WorkspaceNotInGroup {
            group_id,
            workspace_id,
        })?;
    let workspace = workspaces
        .iter()
        .find(|workspace| workspace.id == candidate.workspace_id)
        .ok_or(ArenaGroupError::WorkspaceNotInGroup {
            group_id,
            workspace_id,
        })?;

    Ok((workspace, candidate))
}

fn find_candidate_workspace_by_id_in_group<'workspace, 'candidate>(
    workspaces: &'workspace [DbWorkspace],
    candidates: &'candidate [ArenaCandidate],
    group_id: Uuid,
    candidate_id: Uuid,
) -> Result<(&'workspace DbWorkspace, &'candidate ArenaCandidate), ArenaGroupError> {
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.arena_group_id == group_id && candidate.id == candidate_id)
        .ok_or(ArenaGroupError::CandidateNotInGroup {
            group_id,
            candidate_id,
        })?;
    let workspace = workspaces
        .iter()
        .find(|workspace| workspace.id == candidate.workspace_id)
        .ok_or(ArenaGroupError::WorkspaceNotInGroup {
            group_id,
            workspace_id: candidate.workspace_id,
        })?;

    Ok((workspace, candidate))
}

fn find_attempt_workspace_in_group<'workspace>(
    workspaces: &'workspace [DbWorkspace],
    candidates: &[ArenaCandidate],
    group_id: Uuid,
    workspace_id: Uuid,
) -> Result<&'workspace DbWorkspace, ArenaGroupError> {
    let (workspace, candidate) =
        find_candidate_workspace_in_group(workspaces, candidates, group_id, workspace_id)?;
    if candidate.purpose != ArenaCandidatePurpose::Attempt {
        return Err(ArenaGroupError::ValidationError(format!(
            "Arena candidate {} is {:?}, not an attempt",
            candidate.id, candidate.purpose
        )));
    }

    Ok(workspace)
}

fn build_synthesis_prompt(
    instruction: &str,
    original_prompt: &str,
    attempt_sections: &[String],
    activity_sections: &[String],
    options: &ArenaSynthesizeOptions,
) -> String {
    let mut sections = vec![
        "You are the independent synthesis agent for an AI Arena.\n\
         Write a neutral decision memo. Compare the attempts, preserve disagreement, tradeoffs, and open risks.\n\
         Do not inherit or assume any single attempt's position. Do not create commits, push branches, or open PRs."
            .to_string(),
        format!("User synthesis instruction:\n{}", instruction.trim()),
    ];

    if options.include_original_prompt {
        sections.push(format!(
            "Original Arena prompt:\n{}",
            original_prompt.trim()
        ));
    }

    if options.include_attempt_summaries {
        sections.push(format!(
            "Attempt summaries:\n\n{}",
            if attempt_sections.is_empty() {
                "No attempt summaries available yet.".to_string()
            } else {
                attempt_sections.join("\n\n")
            }
        ));
    }

    if options.include_activity {
        sections.push(format!(
            "Arena activity history:\n\n{}",
            if activity_sections.is_empty() {
                "No prior page-level activity recorded.".to_string()
            } else {
                activity_sections.join("\n\n")
            }
        ));
    }

    sections.join("\n\n---\n\n")
}

async fn latest_agent_run_state_for_workspace(
    pool: &SqlitePool,
    workspace_id: Uuid,
) -> Result<Option<RunState>, ApiError> {
    Ok(sqlx::query_scalar::<_, SqlxJson<RunState>>(
        r#"SELECT state.state_json
             FROM agent_runs run
             JOIN agent_run_state state ON state.agent_run_id = run.id
            WHERE run.workspace_id = ?
            ORDER BY run.created_at DESC, run.id DESC
            LIMIT 1"#,
    )
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?
    .map(|state| state.0))
}

async fn workspace_to_summary(
    pool: &SqlitePool,
    ws: &DbWorkspace,
    executor_config: Option<&ExecutorConfig>,
) -> Result<ArenaWorkspaceSummary, ApiError> {
    let latest_agent_run = latest_agent_run_state_for_workspace(pool, ws.id).await?;
    let candidate = ArenaCandidate::find_by_workspace_id(pool, ws.id)
        .await?
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "Arena workspace {} has no explicit candidate identity",
                ws.id
            ))
        })?;
    let group = ArenaGroup::find_by_id(pool, candidate.arena_group_id)
        .await?
        .ok_or_else(|| ApiError::from(ArenaGroupError::NotFound))?;
    let arena_status = if group.winner_candidate_id == Some(candidate.id) {
        ArenaStatus::Promoted
    } else if ws.archived {
        ArenaStatus::Archived
    } else {
        ArenaStatus::Active
    };

    Ok(ArenaWorkspaceSummary {
        candidate_id: candidate.id,
        workspace_id: ws.id,
        session_id: latest_agent_run.as_ref().map(|state| state.session_id),
        name: ws.name.clone(),
        branch: ws.branch.clone(),
        purpose: candidate.purpose,
        arena_status,
        executor: executor_config.map(|c| c.executor.to_string()),
        variant: executor_config.and_then(|c| c.variant.clone()),
        latest_agent_run_status: latest_agent_run.map(|state| state.status),
        has_uncommitted_changes: None,
    })
}

async fn start_arena_workspace(
    deployment: &DeploymentImpl,
    workspace: &DbWorkspace,
    executor_config: ExecutorConfig,
    prompt: String,
) -> Result<(), ApiError> {
    let pool = &deployment.db().pool;
    deployment.container().create(workspace).await?;

    let workspace = DbWorkspace::find_by_id(pool, workspace.id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Arena workspace not found".to_string()))?;
    let session = Session::create(
        pool,
        &CreateSession {
            executor: Some(executor_config.executor.to_string()),
            name: None,
        },
        Uuid::new_v4(),
        workspace.id,
    )
    .await?;
    let repos = WorkspaceRepo::find_repos_for_workspace(pool, workspace.id).await?;
    let repos_with_setup = repos
        .iter()
        .filter(|repo| repo.setup_script.is_some())
        .collect::<Vec<_>>();

    if repos_with_setup.is_empty() {
        sessions::start_coding_agent_execution_for_session(
            deployment,
            session,
            prompt,
            None,
            executor_config,
            None,
            None,
        )
        .await?;
    } else if repos_with_setup
        .iter()
        .all(|repo| repo.parallel_setup_script)
    {
        for repo in repos_with_setup {
            if let Some(setup_action) = deployment
                .container()
                .setup_actions_for_repos(std::slice::from_ref(repo))
                && let Err(error) = deployment
                    .container()
                    .start_execution(
                        &workspace,
                        &session,
                        &setup_action,
                        &ExecutionProcessRunReason::SetupScript,
                    )
                    .await
            {
                tracing::warn!(
                    workspace_id = %workspace.id,
                    repo_id = %repo.id,
                    "failed to start parallel Arena setup script: {error:#}"
                );
            }
        }
        sessions::start_coding_agent_execution_for_session(
            deployment,
            session,
            prompt,
            None,
            executor_config,
            None,
            None,
        )
        .await?;
    } else if let Some(setup_action) = deployment.container().setup_actions_for_repos(&repos) {
        let reserved = sessions::reserve_coding_agent_execution_for_session(
            deployment,
            session,
            prompt,
            None,
            executor_config,
            None,
            None,
        )
        .await?;
        sessions::setup_gate::start_reserved_after_setup(
            deployment,
            reserved.agent_run_id,
            &setup_action,
        )
        .await?;
    } else {
        return Err(ApiError::BadRequest(
            "Arena workspace setup configuration was inconsistent".to_string(),
        ));
    }

    Ok(())
}

/// Spawn one explicit Arena candidate: create the Workspace row, attach its
/// repositories, persist candidate identity, then start the initial Agent run.
/// The caller performs group-wide cleanup for every error after Execution creation.
async fn spawn_arena_attempt(
    deployment: &DeploymentImpl,
    group: &ArenaGroup,
    _issue_id: Uuid,
    _project_id: Uuid,
    repos: &[WorkspaceRepoInput],
    attempt: ArenaAttemptInput,
    attempt_index: usize,
) -> Result<(DbWorkspace, ExecutorConfig), ApiError> {
    let pool = &deployment.db().pool;

    let attempt_prompt = attempt
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| group.prompt.clone());
    if attempt_prompt.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Arena prompt must not be empty.".to_string(),
        ));
    }

    let display_name = attempt.name.clone().unwrap_or_else(|| {
        format!(
            "{}-arena-{}",
            attempt.executor_config.executor.to_string().to_lowercase(),
            attempt_index + 1
        )
    });

    let workspace = create_workspace_record(deployment, Some(display_name)).await?;

    let mut managed_workspace = deployment
        .workspace_manager()
        .load_managed_workspace(workspace.clone())
        .await?;
    for repo in repos {
        managed_workspace
            .add_repository(repo, deployment.git())
            .await
            .map_err(ApiError::from)?;
    }

    let workspace = managed_workspace.workspace.clone();

    let mut transaction = pool.begin().await?;
    ArenaCandidate::create(
        &mut transaction,
        &CreateArenaCandidate {
            id: Uuid::new_v4(),
            arena_group_id: group.id,
            workspace_id: workspace.id,
            purpose: ArenaCandidatePurpose::Attempt,
            sort_order: attempt_index as i64,
        },
    )
    .await?;
    transaction.commit().await?;

    start_arena_workspace(
        deployment,
        &workspace,
        attempt.executor_config.clone(),
        attempt_prompt,
    )
    .await?;

    let updated = DbWorkspace::find_by_id(pool, workspace.id)
        .await?
        .unwrap_or(workspace);

    Ok((updated, attempt.executor_config))
}

async fn workspaces_for_group(
    pool: &SqlitePool,
    group_id: Uuid,
) -> Result<Vec<ArenaWorkspaceSummary>, ApiError> {
    let workspaces = DbWorkspace::find_by_arena_group_id(pool, group_id).await?;
    let mut summaries = Vec::with_capacity(workspaces.len());
    for ws in workspaces.iter() {
        summaries.push(workspace_to_summary(pool, ws, None).await?);
    }
    Ok(summaries)
}

async fn arena_events_for_group(
    pool: &SqlitePool,
    group_id: Uuid,
) -> Result<Vec<ArenaEvent>, ApiError> {
    Ok(sqlx::query_as::<_, ArenaEvent>(
        r#"SELECT id,
                  arena_group_id,
                  kind,
                  prompt,
                  source_workspace_id,
                  target_workspace_id,
                  synthesis_workspace_id,
                  created_at
             FROM arena_events
            WHERE arena_group_id = ?
            ORDER BY created_at ASC"#,
    )
    .bind(group_id)
    .fetch_all(pool)
    .await?)
}

struct RecordArenaEvent<'a> {
    group_id: Uuid,
    kind: ArenaEventKind,
    prompt: &'a str,
    source_workspace_id: Option<Uuid>,
    target_workspace_id: Option<Uuid>,
    synthesis_workspace_id: Option<Uuid>,
}

async fn record_arena_event(
    pool: &SqlitePool,
    event: RecordArenaEvent<'_>,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"INSERT INTO arena_events
               (id, arena_group_id, kind, prompt, source_workspace_id, target_workspace_id, synthesis_workspace_id)
           VALUES (?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(Uuid::new_v4())
    .bind(event.group_id)
    .bind(event.kind)
    .bind(event.prompt)
    .bind(event.source_workspace_id)
    .bind(event.target_workspace_id)
    .bind(event.synthesis_workspace_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn arena_group_response(
    pool: &SqlitePool,
    group: ArenaGroup,
) -> Result<ArenaGroupResponse, ApiError> {
    let workspaces = workspaces_for_group(pool, group.id).await?;
    let events = arena_events_for_group(pool, group.id).await?;
    Ok(ArenaGroupResponse {
        group,
        workspaces,
        events,
    })
}

async fn create_arena_group(
    State(deployment): State<DeploymentImpl>,
    Path(issue_id): Path<Uuid>,
    Json(payload): Json<CreateArenaRequest>,
) -> Result<ResponseJson<MutationResponse<ArenaGroupResponse>>, ApiError> {
    let CreateArenaRequest {
        project_id,
        base_branch,
        prompt,
        mode,
        repos,
        attempts,
    } = payload;

    if attempts.len() < ARENA_MIN_ATTEMPTS {
        return Err(ApiError::BadRequest(format!(
            "Arena requires at least {ARENA_MIN_ATTEMPTS} attempts, got {}",
            attempts.len()
        )));
    }

    let pool = &deployment.db().pool;
    let project_max = arena_max_for_project(pool, project_id).await?;
    if attempts.len() > project_max {
        return Err(ApiError::BadRequest(format!(
            "Project allows at most {project_max} arena attempts, got {}",
            attempts.len()
        )));
    }
    if repos.is_empty() {
        return Err(ApiError::BadRequest(
            "At least one repository is required.".to_string(),
        ));
    }
    if prompt.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Arena prompt must not be empty.".to_string(),
        ));
    }

    ensure_issue_in_project(pool, issue_id, project_id).await?;

    if ArenaGroup::find_active_by_issue_id(pool, issue_id)
        .await?
        .is_some()
    {
        return Err(ApiError::BadRequest(format!(
            "issue {issue_id} already has an active arena group; close, adopt, promote, or dissolve it first"
        )));
    }

    let task_id = Uuid::new_v4();
    let group_id = Uuid::new_v4();
    let mut transaction = pool.begin().await?;
    Execution::create(
        &mut transaction,
        &CreateExecution {
            id: task_id,
            project_id,
            issue_id,
            parent_task_id: None,
            title: prompt.trim().chars().take(160).collect(),
            execution_kind: ExecutionKind::Arena,
        },
    )
    .await?;
    let group = ArenaGroup::create(
        &mut transaction,
        &CreateArenaGroup {
            id: group_id,
            task_id,
            prompt: prompt.clone(),
            base_branch: base_branch.clone(),
            mode,
        },
    )
    .await?;
    transaction.commit().await?;

    let mut summaries = Vec::with_capacity(attempts.len());
    for (idx, attempt) in attempts.into_iter().enumerate() {
        let attempt_result = match spawn_arena_attempt(
            &deployment,
            &group,
            issue_id,
            project_id,
            &repos,
            attempt,
            idx,
        )
        .await
        {
            Ok((workspace, executor_config)) => {
                workspace_to_summary(pool, &workspace, Some(&executor_config)).await
            }
            Err(error) => Err(error),
        };
        match attempt_result {
            Ok(summary) => summaries.push(summary),
            Err(err) => {
                tracing::error!(
                    arena_group_id = %group.id,
                    attempt_index = idx,
                    "failed to spawn arena attempt: {err:#}"
                );
                if let Err(cleanup_err) = cleanup_failed_arena_group(pool, group.id).await {
                    tracing::warn!(
                        arena_group_id = %group.id,
                        "failed to clean up partially-created arena group: {cleanup_err:#}"
                    );
                }
                return Err(err);
            }
        }
    }

    deployment
        .track_if_analytics_allowed(
            "arena_group_created",
            json!({
                "arena_group_id": group.id.to_string(),
                "issue_id":       issue_id.to_string(),
                "attempt_count":  summaries.len(),
            }),
        )
        .await;

    Ok(ResponseJson(MutationResponse {
        data: ArenaGroupResponse {
            group,
            workspaces: summaries,
            events: Vec::new(),
        },
        txid: txid(),
    }))
}

async fn cleanup_failed_arena_group(pool: &SqlitePool, group_id: Uuid) -> Result<usize, ApiError> {
    let siblings = DbWorkspace::find_by_arena_group_id(pool, group_id).await?;
    let mut archived = 0usize;

    for ws in siblings.iter() {
        DbWorkspace::set_archived(pool, ws.id, true).await?;
        archived += 1;
    }

    ArenaGroup::delete(pool, group_id).await?;
    Ok(archived)
}

async fn get_active_arena_for_issue(
    State(deployment): State<DeploymentImpl>,
    Path(issue_id): Path<Uuid>,
) -> Result<ResponseJson<Option<ArenaGroupResponse>>, ApiError> {
    let pool = &deployment.db().pool;
    let Some(group) = ArenaGroup::find_active_by_issue_id(pool, issue_id).await? else {
        return Ok(ResponseJson(None));
    };
    Ok(ResponseJson(Some(arena_group_response(pool, group).await?)))
}

async fn get_arena_group(
    State(deployment): State<DeploymentImpl>,
    Path(group_id): Path<Uuid>,
) -> Result<ResponseJson<ArenaGroupResponse>, ApiError> {
    let pool = &deployment.db().pool;
    let group = ArenaGroup::find_by_id(pool, group_id)
        .await?
        .ok_or_else(|| ApiError::from(ArenaGroupError::NotFound))?;
    Ok(ResponseJson(arena_group_response(pool, group).await?))
}

async fn close_arena_group(
    State(deployment): State<DeploymentImpl>,
    Path(group_id): Path<Uuid>,
) -> Result<ResponseJson<MutationResponse<CloseArenaResponse>>, ApiError> {
    let pool = &deployment.db().pool;
    let group = ArenaGroup::find_by_id(pool, group_id)
        .await?
        .ok_or_else(|| ApiError::from(ArenaGroupError::NotFound))?;

    if group.lifecycle_status == ArenaLifecycleStatus::Open {
        ArenaGroup::set_lifecycle_status(pool, group.id, ArenaLifecycleStatus::Closed).await?;
    }

    Ok(ResponseJson(MutationResponse {
        data: CloseArenaResponse {
            group_id,
            lifecycle_status: ArenaLifecycleStatus::Closed,
        },
        txid: txid(),
    }))
}

async fn start_arena_implementation(
    State(deployment): State<DeploymentImpl>,
    Path(group_id): Path<Uuid>,
    Json(payload): Json<StartArenaImplementationRequest>,
) -> Result<ResponseJson<MutationResponse<ArenaGroupResponse>>, ApiError> {
    let pool = &deployment.db().pool;
    let group = ArenaGroup::find_by_id(pool, group_id)
        .await?
        .ok_or_else(|| ApiError::from(ArenaGroupError::NotFound))?;

    let siblings = DbWorkspace::find_by_arena_group_id(pool, group.id).await?;
    let candidates = ArenaCandidate::list_for_group(pool, group.id).await?;
    let (workspace, candidate) = find_candidate_workspace_by_id_in_group(
        &siblings,
        &candidates,
        group.id,
        payload.candidate_id,
    )
    .map_err(ApiError::from)?;
    ArenaGroup::select_winner(
        pool,
        group.id,
        candidate.id,
        ArenaLifecycleStatus::ImplementationStarted,
    )
    .await?;
    record_arena_event(
        pool,
        RecordArenaEvent {
            group_id: group.id,
            kind: ArenaEventKind::StartImplementation,
            prompt: payload.follow_up_prompt.as_deref().unwrap_or(""),
            source_workspace_id: None,
            target_workspace_id: Some(candidate.workspace_id),
            synthesis_workspace_id: None,
        },
    )
    .await?;

    if let Some(prompt) = payload
        .follow_up_prompt
        .as_deref()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        let _ = start_arena_follow_up(
            &deployment,
            workspace,
            prompt.to_string(),
            payload.executor_config.clone(),
        )
        .await?;
    }

    let group = ArenaGroup::find_by_id(pool, group.id)
        .await?
        .ok_or_else(|| ApiError::from(ArenaGroupError::NotFound))?;

    Ok(ResponseJson(MutationResponse {
        data: arena_group_response(pool, group).await?,
        txid: txid(),
    }))
}

async fn latest_workspace_summary_text(
    pool: &SqlitePool,
    workspace_id: Uuid,
) -> Result<Option<String>, ApiError> {
    Ok(latest_agent_run_state_for_workspace(pool, workspace_id)
        .await?
        .and_then(|state| state.terminal_output)
        .filter(|message| message.role == AgentRuntimeMessageRole::Assistant)
        .map(|message| message.content))
}

fn build_challenge_prompt(user_prompt: &str, source_label: &str, source_summary: &str) -> String {
    format!(
        "The user wants you to critique or respond to another Arena attempt.\n\
         Other attempt: {source_label}\n\n\
         Other attempt summary:\n{source_summary}\n\n\
         User instruction:\n{user_prompt}"
    )
}

fn workspace_label(workspace: &DbWorkspace, index: usize) -> String {
    workspace
        .name
        .clone()
        .unwrap_or_else(|| format!("Attempt {}", index + 1))
}

async fn workspace_repo_inputs(
    pool: &SqlitePool,
    workspace_id: Uuid,
) -> Result<Vec<WorkspaceRepoInput>, ApiError> {
    let repo_rows: Vec<(Uuid, String)> = sqlx::query_as::<_, (Uuid, String)>(
        r#"SELECT repo_id, target_branch
             FROM workspace_repos
            WHERE workspace_id = ?
            ORDER BY created_at ASC"#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(repo_rows
        .into_iter()
        .map(|(repo_id, target_branch)| WorkspaceRepoInput {
            repo_id,
            target_branch,
        })
        .collect())
}

fn event_activity_section(event: &ArenaEvent, workspaces: &[DbWorkspace]) -> String {
    let label_for = |workspace_id: Option<Uuid>| {
        workspace_id
            .and_then(|id| {
                workspaces
                    .iter()
                    .position(|workspace| workspace.id == id)
                    .map(|index| workspace_label(&workspaces[index], index))
            })
            .unwrap_or_else(|| "Arena".to_string())
    };

    let title = match event.kind {
        ArenaEventKind::AskAll => "Ask all".to_string(),
        ArenaEventKind::Workspace => format!("Message to {}", label_for(event.target_workspace_id)),
        ArenaEventKind::Challenge => format!(
            "{} challenged {}",
            label_for(event.target_workspace_id),
            label_for(event.source_workspace_id)
        ),
        ArenaEventKind::Synthesize => "Synthesize".to_string(),
        ArenaEventKind::StartImplementation => {
            format!(
                "Start implementation from {}",
                label_for(event.target_workspace_id)
            )
        }
    };

    format!("{title}:\n{}", event.prompt)
}

async fn spawn_arena_synthesis_workspace(
    deployment: &DeploymentImpl,
    group: &ArenaGroup,
    source_workspace: &DbWorkspace,
    existing_synthesis_count: usize,
    prompt: String,
    executor_config: ExecutorConfig,
) -> Result<DbWorkspace, ApiError> {
    let pool = &deployment.db().pool;
    let repos = workspace_repo_inputs(pool, source_workspace.id).await?;
    if repos.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "workspace {} has no repos to mirror for synthesis",
            source_workspace.id
        )));
    }

    let display_name = format!(
        "{} {}",
        ARENA_SYNTHESIS_WORKSPACE_PREFIX,
        existing_synthesis_count + 1
    );
    let workspace = create_workspace_record(deployment, Some(display_name)).await?;

    let mut managed_workspace = deployment
        .workspace_manager()
        .load_managed_workspace(workspace.clone())
        .await?;
    for repo in &repos {
        managed_workspace
            .add_repository(repo, deployment.git())
            .await
            .map_err(ApiError::from)?;
    }

    let workspace = managed_workspace.workspace.clone();
    let sort_order = ArenaCandidate::list_for_group(pool, group.id).await?.len() as i64;
    let mut transaction = pool.begin().await?;
    ArenaCandidate::create(
        &mut transaction,
        &CreateArenaCandidate {
            id: Uuid::new_v4(),
            arena_group_id: group.id,
            workspace_id: workspace.id,
            purpose: ArenaCandidatePurpose::Synthesis,
            sort_order,
        },
    )
    .await?;
    transaction.commit().await?;

    start_arena_workspace(deployment, &workspace, executor_config, prompt).await?;

    let updated = DbWorkspace::find_by_id(pool, workspace.id)
        .await?
        .unwrap_or(workspace);

    Ok(updated)
}

fn executor_config_for_arena_message(
    default_config: &ExecutorConfig,
    overrides: &[ArenaWorkspaceExecutorConfig],
    workspace_id: Uuid,
) -> ExecutorConfig {
    overrides
        .iter()
        .find(|override_config| override_config.workspace_id == workspace_id)
        .map(|override_config| override_config.executor_config.clone())
        .unwrap_or_else(|| default_config.clone())
}

async fn start_arena_follow_up(
    deployment: &DeploymentImpl,
    workspace: &DbWorkspace,
    prompt: String,
    executor_config: Option<ExecutorConfig>,
) -> Result<Uuid, ApiError> {
    let pool = &deployment.db().pool;
    let latest_agent_run = latest_agent_run_state_for_workspace(pool, workspace.id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Arena workspace has no AgentRun".to_string()))?;
    let session = Session::find_by_id(pool, latest_agent_run.session_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Arena AgentRun session not found".to_string()))?;

    let executor_config = executor_config.ok_or_else(|| {
        ApiError::BadRequest("Arena follow-up requires an executor_config".to_string())
    })?;
    let snapshot = sessions::start_coding_agent_execution_for_session(
        deployment,
        session,
        prompt,
        None,
        executor_config,
        None,
        None,
    )
    .await?;

    Ok(snapshot.agent_run_id)
}

async fn send_arena_message(
    State(deployment): State<DeploymentImpl>,
    Path(group_id): Path<Uuid>,
    Json(payload): Json<ArenaMessageRequest>,
) -> Result<ResponseJson<MutationResponse<ArenaGroupResponse>>, ApiError> {
    let ArenaMessageRequest {
        target,
        prompt,
        executor_config,
        executor_configs,
    } = payload;
    let pool = &deployment.db().pool;
    let group = ArenaGroup::find_by_id(pool, group_id)
        .await?
        .ok_or_else(|| ApiError::from(ArenaGroupError::NotFound))?;
    let siblings = DbWorkspace::find_by_arena_group_id(pool, group.id).await?;
    let candidates = ArenaCandidate::list_for_group(pool, group.id).await?;
    let attempt_workspace_ids = candidates
        .iter()
        .filter(|candidate| candidate.purpose == ArenaCandidatePurpose::Attempt)
        .map(|candidate| candidate.workspace_id)
        .collect::<std::collections::HashSet<_>>();
    let attempt_siblings: Vec<&DbWorkspace> = siblings
        .iter()
        .filter(|workspace| attempt_workspace_ids.contains(&workspace.id))
        .collect();

    match target {
        ArenaMessageTarget::All => {
            if attempt_siblings.is_empty() {
                return Err(ApiError::BadRequest(
                    "Arena group has no attempts to message".to_string(),
                ));
            }
            for workspace in attempt_siblings.iter().copied() {
                let workspace_executor_config = executor_config_for_arena_message(
                    &executor_config,
                    &executor_configs,
                    workspace.id,
                );
                start_arena_follow_up(
                    &deployment,
                    workspace,
                    prompt.clone(),
                    Some(workspace_executor_config),
                )
                .await?;
            }
            record_arena_event(
                pool,
                RecordArenaEvent {
                    group_id: group.id,
                    kind: ArenaEventKind::AskAll,
                    prompt: &prompt,
                    source_workspace_id: None,
                    target_workspace_id: None,
                    synthesis_workspace_id: None,
                },
            )
            .await?;
        }
        ArenaMessageTarget::Workspace { workspace_id } => {
            let workspace = attempt_siblings
                .iter()
                .copied()
                .find(|ws| ws.id == workspace_id)
                .ok_or_else(|| ArenaGroupError::WorkspaceNotInGroup {
                    group_id: group.id,
                    workspace_id,
                })?;
            start_arena_follow_up(
                &deployment,
                workspace,
                prompt.clone(),
                Some(executor_config.clone()),
            )
            .await?;
            record_arena_event(
                pool,
                RecordArenaEvent {
                    group_id: group.id,
                    kind: ArenaEventKind::Workspace,
                    prompt: &prompt,
                    source_workspace_id: None,
                    target_workspace_id: Some(workspace_id),
                    synthesis_workspace_id: None,
                },
            )
            .await?;
        }
        ArenaMessageTarget::Challenge {
            responder_workspace_id,
            source_workspace_id,
        } => {
            let responder = attempt_siblings
                .iter()
                .copied()
                .find(|ws| ws.id == responder_workspace_id)
                .ok_or_else(|| ArenaGroupError::WorkspaceNotInGroup {
                    group_id: group.id,
                    workspace_id: responder_workspace_id,
                })?;
            if !attempt_siblings
                .iter()
                .any(|ws| ws.id == source_workspace_id)
            {
                return Err(ApiError::from(ArenaGroupError::WorkspaceNotInGroup {
                    group_id: group.id,
                    workspace_id: source_workspace_id,
                }));
            }
            let source_summary = latest_workspace_summary_text(pool, source_workspace_id)
                .await?
                .unwrap_or_else(|| "No summary available yet.".to_string());
            let challenge_prompt =
                build_challenge_prompt(&prompt, &source_workspace_id.to_string(), &source_summary);
            start_arena_follow_up(
                &deployment,
                responder,
                challenge_prompt,
                Some(executor_config.clone()),
            )
            .await?;
            record_arena_event(
                pool,
                RecordArenaEvent {
                    group_id: group.id,
                    kind: ArenaEventKind::Challenge,
                    prompt: &prompt,
                    source_workspace_id: Some(source_workspace_id),
                    target_workspace_id: Some(responder_workspace_id),
                    synthesis_workspace_id: None,
                },
            )
            .await?;
        }
        ArenaMessageTarget::Synthesize { options } => {
            let Some(source_workspace) = attempt_siblings.first().copied() else {
                return Err(ApiError::BadRequest(
                    "Arena group has no workspaces to synthesize".to_string(),
                ));
            };
            let mut summaries = Vec::new();
            for (index, sibling) in attempt_siblings.iter().copied().enumerate() {
                let summary = latest_workspace_summary_text(pool, sibling.id)
                    .await?
                    .unwrap_or_else(|| "No summary available yet.".to_string());
                summaries.push(format!(
                    "{} ({})\n{}",
                    workspace_label(sibling, index),
                    sibling.id,
                    summary
                ));
            }

            let events = arena_events_for_group(pool, group.id).await?;
            let activity_sections: Vec<String> = events
                .iter()
                .map(|event| event_activity_section(event, &siblings))
                .collect();
            let synthesis_prompt = build_synthesis_prompt(
                &prompt,
                &group.prompt,
                &summaries,
                &activity_sections,
                &options,
            );
            let synthesis_count = candidates
                .iter()
                .filter(|candidate| candidate.purpose == ArenaCandidatePurpose::Synthesis)
                .count();
            let synthesis_workspace = spawn_arena_synthesis_workspace(
                &deployment,
                &group,
                source_workspace,
                synthesis_count,
                synthesis_prompt,
                executor_config.clone(),
            )
            .await?;
            record_arena_event(
                pool,
                RecordArenaEvent {
                    group_id: group.id,
                    kind: ArenaEventKind::Synthesize,
                    prompt: &prompt,
                    source_workspace_id: None,
                    target_workspace_id: None,
                    synthesis_workspace_id: Some(synthesis_workspace.id),
                },
            )
            .await?;
        }
    }

    let group = ArenaGroup::find_by_id(pool, group.id)
        .await?
        .ok_or_else(|| ApiError::from(ArenaGroupError::NotFound))?;

    Ok(ResponseJson(MutationResponse {
        data: arena_group_response(pool, group).await?,
        txid: txid(),
    }))
}

async fn promote_arena_workspace(
    State(deployment): State<DeploymentImpl>,
    Path(group_id): Path<Uuid>,
    Json(payload): Json<PromoteArenaRequest>,
) -> Result<ResponseJson<MutationResponse<ArenaGroupResponse>>, ApiError> {
    let pool = &deployment.db().pool;

    let group = ArenaGroup::find_by_id(pool, group_id)
        .await?
        .ok_or_else(|| ApiError::from(ArenaGroupError::NotFound))?;

    if group.mode == ArenaMode::Design
        && group.lifecycle_status != ArenaLifecycleStatus::ImplementationStarted
    {
        return Err(ApiError::BadRequest(
            "Design Arena attempts must be started as implementation before promote.".to_string(),
        ));
    }

    let siblings = DbWorkspace::find_by_arena_group_id(pool, group.id).await?;
    let candidates = ArenaCandidate::list_for_group(pool, group.id).await?;
    let (_, candidate) = find_candidate_workspace_by_id_in_group(
        &siblings,
        &candidates,
        group.id,
        payload.candidate_id,
    )
    .map_err(ApiError::from)?;
    match group.winner_candidate_id {
        None => {
            ArenaGroup::select_winner(pool, group.id, candidate.id, ArenaLifecycleStatus::Adopted)
                .await?;
        }
        Some(winner_id) if winner_id == candidate.id => {
            ArenaGroup::set_lifecycle_status(pool, group.id, ArenaLifecycleStatus::Adopted).await?;
        }
        Some(_) => {
            return Err(ApiError::Conflict(format!(
                "Arena group {} already selected another candidate",
                group.id
            )));
        }
    }

    // 2. Archive every other sibling (arena_status=archived AND archived=true)
    //    so the existing 1h cleanup path picks them up. We deliberately do
    //    NOT call container.delete here — that synchronously removes the
    //    worktree and would block this request.
    for ws in siblings.iter() {
        if ws.id == candidate.workspace_id {
            continue;
        }
        DbWorkspace::set_archived(pool, ws.id, true).await?;
    }

    let group = ArenaGroup::find_by_id(pool, group.id)
        .await?
        .ok_or_else(|| ApiError::from(ArenaGroupError::NotFound))?;

    deployment
        .track_if_analytics_allowed(
            "arena_workspace_promoted",
            json!({
                "arena_group_id":       group.id.to_string(),
                "winner_candidate_id":  payload.candidate_id.to_string(),
                "promoted_workspace_id": candidate.workspace_id.to_string(),
                "sibling_count":        siblings.len().saturating_sub(1),
            }),
        )
        .await;

    Ok(ResponseJson(MutationResponse {
        data: arena_group_response(pool, group).await?,
        txid: txid(),
    }))
}

async fn retry_arena_workspace(
    State(deployment): State<DeploymentImpl>,
    Path((group_id, workspace_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<RetryArenaRequest>,
) -> Result<ResponseJson<MutationResponse<ArenaGroupResponse>>, ApiError> {
    let pool = &deployment.db().pool;

    let group = ArenaGroup::find_by_id(pool, group_id)
        .await?
        .ok_or_else(|| ApiError::from(ArenaGroupError::NotFound))?;
    if group.winner_candidate_id.is_some() {
        return Err(ApiError::from(ArenaGroupError::AlreadyHasWinner {
            group_id: group.id,
        }));
    }

    // Verify workspace belongs to group + figure out repos to reuse.
    let siblings = DbWorkspace::find_by_arena_group_id(pool, group.id).await?;
    let candidates = ArenaCandidate::list_for_group(pool, group.id).await?;
    let target = find_attempt_workspace_in_group(&siblings, &candidates, group.id, workspace_id)
        .map_err(ApiError::from)?;

    // Pull repo set from the failing workspace so the retry mirrors it.
    // Runtime-checked query to avoid requiring a fresh .sqlx cache for
    // a fork-only path; matches the style of sibling queries in this file.
    let repo_rows: Vec<(Uuid, String)> = sqlx::query_as::<_, (Uuid, String)>(
        r#"SELECT repo_id, target_branch
             FROM workspace_repos
            WHERE workspace_id = ?
            ORDER BY created_at ASC"#,
    )
    .bind(target.id)
    .fetch_all(pool)
    .await?;
    let repos: Vec<WorkspaceRepoInput> = repo_rows
        .into_iter()
        .map(|(repo_id, target_branch)| WorkspaceRepoInput {
            repo_id,
            target_branch,
        })
        .collect();

    if repos.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "workspace {workspace_id} has no repos to mirror; cannot retry"
        )));
    }

    // Mark the failing attempt as archived (arena-internally) but
    // **don't** flip the user-archived flag — the user might want to
    // keep its logs around.
    DbWorkspace::set_archived(pool, workspace_id, true).await?;

    let attempts_so_far = candidates
        .iter()
        .filter(|candidate| candidate.purpose == ArenaCandidatePurpose::Attempt)
        .count();
    let attempt = ArenaAttemptInput {
        executor_config: payload.executor_config,
        name: payload.name,
        prompt: payload.prompt,
    };
    let task = Execution::find_by_id(pool, group.task_id)
        .await?
        .ok_or_else(|| ApiError::Conflict("Arena Execution is missing".to_string()))?;
    spawn_arena_attempt(
        &deployment,
        &group,
        task.issue_id,
        task.project_id,
        &repos,
        attempt,
        attempts_so_far,
    )
    .await?;

    let group = ArenaGroup::find_by_id(pool, group.id)
        .await?
        .ok_or_else(|| ApiError::from(ArenaGroupError::NotFound))?;

    Ok(ResponseJson(MutationResponse {
        data: arena_group_response(pool, group).await?,
        txid: txid(),
    }))
}

async fn dissolve_arena_group(
    State(deployment): State<DeploymentImpl>,
    Path(group_id): Path<Uuid>,
) -> Result<ResponseJson<MutationResponse<DissolveArenaResponse>>, ApiError> {
    let pool = &deployment.db().pool;

    let group = ArenaGroup::find_by_id(pool, group_id)
        .await?
        .ok_or_else(|| ApiError::from(ArenaGroupError::NotFound))?;
    if group.winner_candidate_id.is_some() {
        return Err(ApiError::BadRequest(
            "Cannot dissolve a promoted arena group; the merged attempt is now your work."
                .to_string(),
        ));
    }

    let siblings = DbWorkspace::find_by_arena_group_id(pool, group.id).await?;
    let mut archived = 0usize;
    for ws in siblings.iter() {
        DbWorkspace::set_archived(pool, ws.id, true).await?;
        archived += 1;
    }

    ArenaGroup::delete(pool, group.id).await?;

    Ok(ResponseJson(MutationResponse {
        data: DissolveArenaResponse {
            group_id: group.id,
            workspaces_archived: archived,
        },
        txid: txid(),
    }))
}

async fn fallback_arena_groups(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ProjectQuery>,
) -> Result<ResponseJson<Value>, ApiError> {
    let Some(project_id) = query.project_id else {
        return Ok(empty_rows("arena_groups"));
    };

    let groups = ArenaGroup::find_all_by_project_id(&deployment.db().pool, project_id).await?;
    Ok(ResponseJson(json!({ "arena_groups": groups })))
}

async fn list_issue_workspaces(
    State(deployment): State<DeploymentImpl>,
    Path(issue_id): Path<Uuid>,
) -> Result<ResponseJson<Vec<DbWorkspace>>, ApiError> {
    // Fixes the dead-code `?task_id=` pattern noted in
    // docs/future/ai-arena/notes-step0.md §3.5: list every workspace
    // that the local kanban issue links to (regardless of arena
    // membership).
    let pool = &deployment.db().pool;
    // Keep this projection runtime-checked until the owning SQLx metadata is
    // regenerated at the Phase 1 gate.
    let workspace_ids: Vec<Uuid> = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT session.workspace_id
        FROM tasks task
        JOIN agent_task_bindings binding ON binding.task_id = task.id
        JOIN sessions session ON session.id = binding.session_id
        WHERE task.issue_id = ?
        UNION
        SELECT attempt.workspace_id
        FROM tasks task
        JOIN workflow_attempts attempt ON attempt.task_id = task.id
        WHERE task.issue_id = ? AND attempt.workspace_id IS NOT NULL
        UNION
        SELECT candidate.workspace_id
        FROM tasks task
        JOIN arena_groups arena ON arena.task_id = task.id
        JOIN arena_candidates candidate ON candidate.arena_group_id = arena.id
        WHERE task.issue_id = ?
        "#,
    )
    .bind(issue_id)
    .bind(issue_id)
    .bind(issue_id)
    .fetch_all(pool)
    .await?;

    let mut workspaces = Vec::with_capacity(workspace_ids.len());
    for ws_id in workspace_ids {
        if let Some(ws) = DbWorkspace::find_by_id(pool, ws_id).await? {
            workspaces.push(ws);
        }
    }
    Ok(ResponseJson(workspaces))
}

#[cfg(test)]
mod tests {
    use api_types::{CreateProjectRequest, CreateTaskRequest, TaskPriority};
    use chrono::Utc;
    use executors::runtime::{AgentRunStatus, ProjectionStatus, RunState};
    use serde_json::Value;
    use sqlx::{SqlitePool, sqlite::SqlitePoolOptions, types::Json};
    use uuid::Uuid;

    async fn setup_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        sqlx::migrate!("../db/migrations")
            .run(&pool)
            .await
            .expect("migrate fixture");
        pool
    }

    async fn setup_agent_run_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        for statement in [
            r#"
            CREATE TABLE agent_runs (
                id BLOB PRIMARY KEY,
                session_id BLOB NOT NULL,
                workspace_id BLOB NOT NULL,
                created_at TEXT NOT NULL
            )
            "#,
            r#"
            CREATE TABLE agent_run_state (
                agent_run_id BLOB PRIMARY KEY,
                state_json TEXT NOT NULL
            )
            "#,
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("create AgentRun test schema");
        }

        pool
    }

    fn run_state(session_id: Uuid, agent_run_id: Uuid, status: AgentRunStatus) -> RunState {
        RunState {
            state_schema_version: 1,
            reducer_version: 1,
            session_id,
            agent_run_id,
            turn_id: Uuid::new_v4(),
            status,
            projection_status: ProjectionStatus::Current,
            last_run_attempt_id: None,
            last_run_attempt_number: 0,
            last_event_sequence: 0,
            last_event_id: None,
            provider_session: None,
            terminal_output: None,
            last_error: None,
            observed_configuration: None,
            pending_interactions: Vec::new(),
            closed_interaction_ids: Vec::new(),
            unknown_event_count: 0,
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn latest_arena_agent_run_keeps_session_and_state_together() {
        let pool = setup_agent_run_pool().await;
        let workspace_id = Uuid::new_v4();
        let older_session_id = Uuid::new_v4();
        let older_run_id = Uuid::new_v4();
        let latest_session_id = Uuid::new_v4();
        let latest_run_id = Uuid::new_v4();

        for (run_id, session_id, created_at, state) in [
            (
                older_run_id,
                older_session_id,
                "2026-08-14T01:00:00Z",
                run_state(older_session_id, older_run_id, AgentRunStatus::Succeeded),
            ),
            (
                latest_run_id,
                latest_session_id,
                "2026-08-14T02:00:00Z",
                run_state(latest_session_id, latest_run_id, AgentRunStatus::Running),
            ),
        ] {
            sqlx::query(
                "INSERT INTO agent_runs (id, session_id, workspace_id, created_at) VALUES (?, ?, ?, ?)",
            )
            .bind(run_id)
            .bind(session_id)
            .bind(workspace_id)
            .bind(created_at)
            .execute(&pool)
            .await
            .expect("insert AgentRun");
            sqlx::query("INSERT INTO agent_run_state (agent_run_id, state_json) VALUES (?, ?)")
                .bind(run_id)
                .bind(Json(state))
                .execute(&pool)
                .await
                .expect("insert RunState");
        }

        let latest = super::latest_agent_run_state_for_workspace(&pool, workspace_id)
            .await
            .expect("load latest Arena AgentRun")
            .expect("latest Arena AgentRun");

        assert_eq!(latest.agent_run_id, latest_run_id);
        assert_eq!(latest.session_id, latest_session_id);
        assert_eq!(latest.status, AgentRunStatus::Running);
    }

    #[tokio::test]
    async fn stale_project_scope_queries_cannot_restore_a_deleted_project() {
        let pool = setup_pool().await;
        let directory = tempfile::tempdir().unwrap();
        let project = crate::routes::project_store::open(
            &pool,
            directory.path().to_str().unwrap(),
            CreateProjectRequest {
                id: None,
                organization_id: super::local_org_id(),
                name: "Detached project".into(),
                color: "210 80% 52%".into(),
            },
        )
        .await
        .unwrap();
        let scratch: String = sqlx::query_scalar(
            "SELECT payload FROM scratch WHERE id=? AND scratch_type='PROJECT_REPO_DEFAULTS'",
        )
        .bind(project.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let path = directory.path().join(".vibe-kanban").join("project.json");
        let before = std::fs::read(&path).unwrap();
        crate::routes::project_store::delete(&pool, project.id)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO scratch(id,scratch_type,payload) VALUES (?,'PROJECT_REPO_DEFAULTS',?)",
        )
        .bind(project.id)
        .bind(scratch)
        .execute(&pool)
        .await
        .unwrap();

        for _ in 0..2 {
            // These are the same readers used by the old board's fallback GETs.
            assert!(super::get_local_project(&pool, project.id).await.is_err());
            assert!(
                super::ensure_default_statuses(&pool, project.id)
                    .await
                    .is_err()
            );
            assert!(super::list_project_issues(&pool, project.id).await.is_err());
            assert!(super::list_project_tags(&pool, project.id).await.is_err());
            assert!(
                super::list_project_issue_tags(&pool, project.id)
                    .await
                    .is_err()
            );
            assert!(
                super::list_project_issue_assignees(&pool, project.id)
                    .await
                    .is_err()
            );
            assert!(
                super::list_project_issue_followers(&pool, project.id)
                    .await
                    .is_err()
            );
            assert!(
                super::list_project_issue_relationships(&pool, project.id)
                    .await
                    .is_err()
            );
            assert!(
                super::list_local_projects(&pool)
                    .await
                    .unwrap()
                    .iter()
                    .all(|row| row.id != project.id)
            );
        }
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[tokio::test]
    async fn local_issue_create_seeds_default_statuses_and_simple_id() {
        let pool = setup_pool().await;
        let directory = tempfile::tempdir().unwrap();
        let project = crate::routes::project_store::open(
            &pool,
            directory.path().to_str().unwrap(),
            CreateProjectRequest {
                id: None,
                organization_id: super::local_org_id(),
                name: "Local project".into(),
                color: "210 80% 52%".into(),
            },
        )
        .await
        .unwrap();
        let project_id = project.id;

        let statuses = super::ensure_default_statuses(&pool, project_id)
            .await
            .expect("seed statuses");

        assert_eq!(statuses.len(), 6);
        assert_eq!(statuses[0].name, "Discuss");
        assert_eq!(statuses[1].name, "Todo");
        assert_eq!(statuses[5].name, "Cancelled");
        assert!(statuses[5].hidden);

        let issue = super::create_local_issue(
            &pool,
            CreateTaskRequest {
                id: None,
                project_id,
                status_id: statuses[0].id,
                title: "First local issue".to_string(),
                description: Some("stored in the project file".to_string()),
                priority: Some(TaskPriority::High),
                start_date: None,
                target_date: None,
                completed_at: None,
                sort_order: 1001.0,
                parent_issue_id: None,
                parent_issue_sort_order: None,
                extension_metadata: Value::Null,
            },
        )
        .await
        .expect("create local issue");

        assert_eq!(issue.project_id, project_id);
        assert_eq!(issue.status_id, statuses[0].id);
        assert_eq!(issue.issue_number, 1);
        assert_eq!(issue.simple_id, "TASK-1");
        assert_eq!(issue.title, "First local issue");
        assert_eq!(issue.priority, Some(TaskPriority::High));
    }

    #[tokio::test]
    async fn local_project_create_seeds_default_tags() {
        let pool = setup_pool().await;
        let directory = tempfile::tempdir().unwrap();

        let project = crate::routes::project_store::open(
            &pool,
            directory.path().to_str().unwrap(),
            CreateProjectRequest {
                id: None,
                organization_id: super::local_org_id(),
                name: "Tagged Project".to_string(),
                color: "210 80% 52%".to_string(),
            },
        )
        .await
        .expect("create local project");

        let tags = super::list_project_tags(&pool, project.id)
            .await
            .expect("list default tags");

        let tag_names: Vec<_> = tags.iter().map(|tag| tag.name.as_str()).collect();
        assert_eq!(
            tag_names,
            vec!["bug", "documentation", "enhancement", "feature"]
        );
    }

    fn test_workspace(workspace_id: Uuid, name: Option<&str>) -> super::DbWorkspace {
        let now = Utc::now();
        super::DbWorkspace {
            id: workspace_id,
            container_ref: None,
            workspace_kind: db::models::workspace::WorkspaceKind::Worktree,
            container_ownership: db::models::workspace::ContainerOwnership::Managed,
            branch: "arena-test".to_string(),
            setup_completed_at: None,
            created_at: now,
            updated_at: now,
            archived: false,
            pinned: false,
            name: name.map(str::to_string),
            worktree_deleted: false,
        }
    }

    fn test_candidate(
        group_id: Uuid,
        workspace_id: Uuid,
        purpose: super::ArenaCandidatePurpose,
        sort_order: i64,
    ) -> super::ArenaCandidate {
        let now = Utc::now();
        super::ArenaCandidate {
            id: Uuid::new_v4(),
            arena_group_id: group_id,
            workspace_id,
            purpose,
            sort_order,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn attempt_workspace_lookup_rejects_synthesis_workspaces() {
        let group_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        let synthesis_id = Uuid::new_v4();
        let workspaces = vec![
            test_workspace(attempt_id, Some("renamed synthesis-looking workspace")),
            test_workspace(synthesis_id, Some("renamed ordinary workspace")),
        ];
        let candidates = vec![
            test_candidate(
                group_id,
                attempt_id,
                super::ArenaCandidatePurpose::Attempt,
                0,
            ),
            test_candidate(
                group_id,
                synthesis_id,
                super::ArenaCandidatePurpose::Synthesis,
                1,
            ),
        ];

        let attempt =
            super::find_attempt_workspace_in_group(&workspaces, &candidates, group_id, attempt_id)
                .expect("attempt workspace");
        assert_eq!(attempt.id, attempt_id);

        let synthesis_result = super::find_attempt_workspace_in_group(
            &workspaces,
            &candidates,
            group_id,
            synthesis_id,
        );
        assert!(synthesis_result.is_err());
    }

    #[test]
    fn synthesis_prompt_only_includes_selected_context_sections() {
        let options = super::ArenaSynthesizeOptions {
            include_original_prompt: true,
            include_attempt_summaries: true,
            include_activity: false,
        };

        let prompt = super::build_synthesis_prompt(
            "Write the decision memo.",
            "Original arena prompt",
            &[
                "Attempt A:\nUse the lightweight design.".to_string(),
                "Attempt B:\nUse the workflow design.".to_string(),
            ],
            &["Ask all: explain risks".to_string()],
            &options,
        );

        assert!(prompt.contains("Write the decision memo."));
        assert!(prompt.contains("Original arena prompt"));
        assert!(prompt.contains("Attempt A"));
        assert!(!prompt.contains("Ask all: explain risks"));
    }
}
