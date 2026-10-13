//! One management service for the local page, scoped main-Agent MCP and HTTP
//! integrations. This module does not start a second scheduler or transcript.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use api_types::CreateTaskRequest;
use chrono::{DateTime, Utc};
use db::models::{
    session::Session,
    workflow_files::{WorkflowDeliveryCompleteness, WorkflowMissingSubtaskView},
    workflow_management::WorkflowMainSessionBinding,
};
use executors::profile::ExecutorConfig;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection, SqlitePool};
use ts_rs::TS;
use uuid::Uuid;
use workflow::{
    WorkflowGraph,
    dispatch::{dispatch_scope, validate_dispatch},
    files::{
        DispatchPlan, SplitTaskManifest, relative_control_path, sha256_bytes, validate_manifest,
    },
    graph::WorkflowNodeKind,
    rework::{ReworkPlan, ReworkSourceExecution, plan_rework},
};

use super::runner::{
    self, WorkflowRunCanceller, WorkflowWorkspaceRequest, WorkflowWorkspaceResolver,
};
use crate::{
    error::ApiError,
    routes::{
        integrations::request_hash,
        local_remote::insert_local_issue,
        workflows::{
            self, TriggerWorkflowRequest, WorkflowAttemptResponse, WorkflowRunResponse,
            WorkflowTemplateResponse,
        },
    },
};

#[path = "batch_rework.rs"]
mod batch_rework;

/// Submission already has a fixed workspace. Preparing its main Session may
/// bind to that workspace, but cannot allocate another project directory.
struct BoundMainWorkspace(Uuid);

#[async_trait::async_trait]
impl WorkflowWorkspaceResolver for BoundMainWorkspace {
    async fn create_or_bind_main_workspace(
        &self,
        request: WorkflowWorkspaceRequest,
    ) -> Result<Uuid, ApiError> {
        if request.existing_workspace_id != Some(self.0) {
            return Err(ApiError::Conflict(
                "The workflow's fixed workspace is unavailable".into(),
            ));
        }
        Ok(self.0)
    }
    async fn cleanup_created_main_workspace(&self, _: Uuid) -> Result<(), ApiError> {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PrepareWorkflowMainSessionRequest {
    pub project_id: Uuid,
    pub workflow_id: Uuid,
    pub request_id: String,
    #[serde(default)]
    #[ts(optional)]
    pub issue_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WorkflowContextView {
    pub project_id: Uuid,
    pub main_session_id: Uuid,
    pub workflow_id: Uuid,
    pub workflow_name: String,
    pub main_agent_config: ExecutorConfig,
    pub main_agent_prompt: String,
    pub prepared_issue_id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub instance_id: Option<Uuid>,
    pub workspace_id: Uuid,
    pub latest_run_id: Option<Uuid>,
    pub definition_locked_at: Option<DateTime<Utc>>,
    pub allowed_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WorkflowMainSessionView {
    pub session: Session,
    pub context: WorkflowContextView,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowSubmissionScope {
    #[default]
    All,
    FromNodes {
        node_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum WorkflowSubmissionAction {
    Start,
    Retry,
    Rework,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum WorkflowActivePolicy {
    AfterCurrent,
    StopThenRun,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSubmission {
    pub request_id: String,
    pub action: WorkflowSubmissionAction,
    #[serde(default)]
    #[ts(optional)]
    pub input_text: Option<String>,
    #[serde(default)]
    pub material_paths: Vec<String>,
    #[serde(default)]
    #[ts(optional)]
    pub source_run_id: Option<Uuid>,
    #[serde(default)]
    #[ts(optional)]
    pub source_node_execution_id: Option<Uuid>,
    #[serde(default)]
    pub scope: WorkflowSubmissionScope,
    #[serde(default)]
    #[ts(optional)]
    pub active_policy: Option<WorkflowActivePolicy>,
    #[serde(default)]
    #[ts(optional)]
    pub source_message_id: Option<Uuid>,
    #[serde(default)]
    #[ts(optional)]
    pub delivery_intent: Option<WorkflowDeliveryCompleteness>,
    #[serde(default)]
    #[ts(optional)]
    pub source_batch_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDispatchRequest {
    pub request_id: String,
    pub batch_id: Uuid,
    /// Project-relative path to the dispatch.json written by the main Agent.
    pub dispatch_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AcceptedWorkflowDispatch {
    pub request_id: String,
    pub batch_id: Uuid,
    pub run_id: Uuid,
    pub assigned_count: u32,
    pub stage_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AcceptedWorkflowSubmission {
    pub request_id: String,
    pub project_id: Uuid,
    pub issue_id: Uuid,
    pub instance_id: Uuid,
    pub run_id: Uuid,
    pub phase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum WorkflowStopStatus {
    NotRequested,
    Requested,
    Confirmed,
    Unreachable,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowStopView {
    pub run_id: Uuid,
    pub instance_id: Uuid,
    pub status: db::models::workflow::WorkflowRunStatus,
    pub stop_status: WorkflowStopStatus,
    pub affected_run_ids: Vec<Uuid>,
    pub unresolved_source_run_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WorkflowNotificationView {
    pub id: Uuid,
    #[ts(type = "number")]
    pub sequence: i64,
    pub instance_id: Uuid,
    pub run_id: Uuid,
    pub main_session_id: Option<Uuid>,
    pub event_key: String,
    pub kind: String,
    pub node_execution_id: Option<Uuid>,
    pub interaction_id: Option<Uuid>,
    pub observed_status: String,
    pub summary: String,
    pub created_at: DateTime<Utc>,
    pub current_status: String,
    pub is_resolved: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WorkflowNotificationPage {
    pub notifications: Vec<WorkflowNotificationView>,
    #[ts(type = "number | null")]
    pub next_cursor: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowReuseView {
    pub node_id: String,
    #[ts(type = "number")]
    pub iteration: i64,
    pub source_node_execution_id: Uuid,
    pub source_run_id: Uuid,
    pub output_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WorkflowInstanceView {
    pub instance: WorkflowAttemptResponse,
    pub graph_json: String,
    pub runs: Vec<WorkflowRunResponse>,
    pub reuse: Vec<WorkflowReuseView>,
    pub skipped_node_ids: Vec<String>,
    pub interactions: Vec<crate::routes::integrations::workflows::WorkflowInteraction>,
    pub notifications: Vec<WorkflowNotificationView>,
    #[ts(type = "number | null")]
    pub next_cursor: Option<i64>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WorkflowCallableTemplatePage {
    pub workflows: Vec<WorkflowTemplateResponse>,
    #[ts(type = "number | null")]
    pub next_cursor: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowManagementInteractionRequest {
    pub run_id: Uuid,
    pub node_execution_id: Uuid,
    pub request_id: String,
    #[serde(flatten)]
    pub response: crate::routes::integrations::workflows::WorkflowInteractionResponse,
}

/// Created by the authenticated boundary, never deserialised from tool input.
#[derive(Debug, Clone)]
pub enum WorkflowManagementCaller {
    MainSession {
        session_id: Uuid,
        /// All three are Some for MCP, all None for the authorised local page.
        agent_run_id: Option<Uuid>,
        turn_id: Option<Uuid>,
        token_hash: Option<String>,
    },
    Instance {
        instance_id: Uuid,
        namespace: String,
        integration_id: Option<Uuid>,
    },
}

pub fn validate_request_id(value: &str) -> Result<(), ApiError> {
    if value.trim().is_empty() || value.chars().count() > 200 {
        return Err(ApiError::BadRequest(
            "request_id must contain 1–200 characters".into(),
        ));
    }
    Ok(())
}

pub async fn read_workflow_context(
    pool: &SqlitePool,
    session_id: Uuid,
) -> Result<Option<WorkflowContextView>, ApiError> {
    let Some(binding) = WorkflowMainSessionBinding::find_by_session_id(pool, session_id).await?
    else {
        return Ok(None);
    };
    let (project_id,workspace_id):(Uuid,Uuid)=sqlx::query_as("SELECT m.project_id,s.workspace_id FROM sessions s JOIN session_project_memberships m ON m.session_id=s.id WHERE s.id=?")
        .bind(session_id).fetch_optional(pool).await?.ok_or(ApiError::Unauthorized)?;
    let template = workflows::get_workflow_template(pool, binding.workflow_id).await?;
    if template.project_id.is_some_and(|id| id != project_id) {
        return Err(ApiError::Forbidden(
            "Workflow Session belongs to another project".into(),
        ));
    }
    let instance_id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM workflow_attempts WHERE main_session_id=?")
            .bind(session_id)
            .fetch_optional(pool)
            .await?;
    let instance = if let Some(id) = instance_id {
        workflows::workflow_attempt_by_id(pool, id).await?
    } else {
        None
    };
    if instance
        .as_ref()
        .is_some_and(|a| a.project_id != project_id || a.workspace_id != Some(workspace_id))
    {
        return Err(ApiError::Forbidden(
            "Workflow Session ancestry is inconsistent".into(),
        ));
    }
    let main_agent_config = serde_json::from_str(&binding.main_agent_config_json)
        .map_err(|_| ApiError::Conflict("Captured main Agent configuration is invalid".into()))?;
    let initial = instance.as_ref().is_none_or(|a| a.latest_run_id.is_none());
    Ok(Some(WorkflowContextView {
        project_id,
        main_session_id: session_id,
        workflow_id: binding.workflow_id,
        workflow_name: template.name,
        main_agent_config,
        main_agent_prompt: binding.main_agent_prompt,
        prepared_issue_id: binding.prepared_issue_id,
        issue_id: instance
            .as_ref()
            .map(|a| a.issue_id)
            .or(binding.prepared_issue_id),
        instance_id,
        workspace_id,
        latest_run_id: instance.as_ref().and_then(|a| a.latest_run_id),
        definition_locked_at: instance.as_ref().and_then(|a| a.definition_locked_at),
        allowed_actions: if initial {
            vec!["start".into()]
        } else {
            vec![
                "get".into(),
                "retry".into(),
                "rework".into(),
                "stop".into(),
                "respond".into(),
                "dispatch".into(),
            ]
        },
    }))
}

/// Preparing a discussion Session never creates an Task, Task or AgentRun.
pub async fn prepare_workflow_main_session<W: WorkflowWorkspaceResolver>(
    pool: &SqlitePool,
    request: PrepareWorkflowMainSessionRequest,
    resolver: &W,
) -> Result<WorkflowMainSessionView, ApiError> {
    static PREPARE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = PREPARE_LOCK.lock().await;
    validate_request_id(&request.request_id)?;
    let hash = request_hash(&request)?;
    let template = workflows::get_workflow_template(pool, request.workflow_id).await?;
    if template
        .project_id
        .is_some_and(|id| id != request.project_id)
    {
        return Err(ApiError::Forbidden(
            "Workflow template is not available in this project".into(),
        ));
    }
    if let Some(issue_id) = request.issue_id {
        crate::routes::project_store::require_task(pool, request.project_id, issue_id).await?;
    }
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(issue_id) = request.issue_id {
        let valid: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM local_issues WHERE id=? AND project_id=?)",
        )
        .bind(issue_id)
        .bind(request.project_id)
        .fetch_one(&mut *tx)
        .await?;
        if !valid {
            return Err(ApiError::Forbidden(
                "Task is not available in this project".into(),
            ));
        }
    }
    let prior = sqlx::query(
        "SELECT * FROM workflow_main_session_requests WHERE project_id=? AND request_id=?",
    )
    .bind(request.project_id)
    .bind(&request.request_id)
    .fetch_optional(&mut *tx)
    .await?;
    if prior
        .as_ref()
        .is_some_and(|row| row.get::<String, _>("request_hash") != hash)
    {
        return Err(ApiError::Conflict(
            "IDEMPOTENCY_CONFLICT: request_id has different parameters".into(),
        ));
    }
    let existing_instance = if let Some(issue_id) = request.issue_id {
        sqlx::query("SELECT a.id,a.workflow_id,a.main_session_id,a.main_session_bound_at,a.workspace_id,s.template_id,w.main_agent_config_json,w.main_agent_prompt FROM workflow_attempts a JOIN workflows w ON w.id=a.workflow_id LEFT JOIN workflow_attempt_sources s ON s.attempt_id=a.id WHERE a.issue_id=?")
            .bind(issue_id).fetch_optional(&mut *tx).await?
    } else {
        None
    };
    if let Some(row) = &existing_instance {
        let publication = row
            .try_get::<Option<Uuid>, _>("template_id")?
            .unwrap_or(row.try_get("workflow_id")?);
        if publication != request.workflow_id {
            return Err(ApiError::Conflict(
                "INSTANCE_BINDING_CONFLICT: Task is bound to a different workflow publication"
                    .into(),
            ));
        }
        if row
            .try_get::<Option<String>, _>("main_session_bound_at")?
            .is_some()
            && row.try_get::<Option<Uuid>, _>("main_session_id")?.is_none()
        {
            return Err(ApiError::Conflict(
                "Workflow instance's original main Session was deleted; it cannot be replaced"
                    .into(),
            ));
        }
    }
    let bound_id = existing_instance
        .as_ref()
        .map(|row| row.try_get::<Option<Uuid>, _>("main_session_id"))
        .transpose()?
        .flatten();
    let prepared = if let Some(issue_id) = request.issue_id {
        sqlx::query("SELECT b.session_id,b.workflow_id,b.main_agent_config_json,b.main_agent_prompt,s.workspace_id FROM workflow_main_session_bindings b JOIN sessions s ON s.id=b.session_id WHERE b.prepared_issue_id=?")
            .bind(issue_id).fetch_optional(&mut *tx).await?
    } else {
        None
    };
    if prepared
        .as_ref()
        .is_some_and(|row| row.get::<Uuid, _>("workflow_id") != request.workflow_id)
    {
        return Err(ApiError::Conflict(
            "Task already has a main Session for another publication".into(),
        ));
    }
    let reopening = bound_id.or(prepared.as_ref().map(|row| row.get("session_id")));
    if let Some(session_id) = reopening {
        let binding=sqlx::query("SELECT b.*,s.workspace_id,m.project_id FROM workflow_main_session_bindings b JOIN sessions s ON s.id=b.session_id JOIN session_project_memberships m ON m.session_id=s.id WHERE b.session_id=?")
            .bind(session_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::Conflict("Original main Session binding is unavailable; it cannot be replaced".into()))?;
        if binding.try_get::<Uuid, _>("workflow_id")? != request.workflow_id
            || binding.try_get::<Uuid, _>("project_id")? != request.project_id
        {
            return Err(ApiError::Forbidden(
                "Original main Session ancestry is inconsistent".into(),
            ));
        }
        if prior
            .as_ref()
            .is_some_and(|row| row.get::<Uuid, _>("session_id") != session_id)
        {
            return Err(ApiError::Conflict(
                "Prepare request is already bound to another Session".into(),
            ));
        }
        sqlx::query("INSERT INTO workflow_main_session_requests(project_id,request_id,request_hash,workflow_id,issue_id,session_id,workspace_id,main_agent_config_json,main_agent_prompt,completed_at) VALUES (?,?,?,?,?,?,?,?,?,datetime('now','subsec')) ON CONFLICT(project_id,request_id) DO NOTHING")
            .bind(request.project_id).bind(&request.request_id).bind(&hash).bind(request.workflow_id).bind(request.issue_id).bind(session_id).bind(binding.try_get::<Uuid,_>("workspace_id")?).bind(binding.try_get::<String,_>("main_agent_config_json")?).bind(binding.try_get::<String,_>("main_agent_prompt")?).execute(&mut *tx).await?;
        tx.commit().await?;
        let session = Session::find_by_id(pool, session_id)
            .await?
            .ok_or_else(|| ApiError::Conflict("Original main Session was deleted".into()))?;
        let context = read_workflow_context(pool, session_id)
            .await?
            .ok_or(ApiError::Unauthorized)?;
        return Ok(WorkflowMainSessionView { session, context });
    }
    if prior
        .as_ref()
        .is_some_and(|row| row.get::<Option<String>, _>("completed_at").is_some())
    {
        let session_id = prior
            .as_ref()
            .expect("checked prior")
            .try_get("session_id")?;
        tx.commit().await?;
        let session = Session::find_by_id(pool, session_id)
            .await?
            .ok_or_else(|| {
                ApiError::Conflict("Prepared Session was deleted; replay cannot recreate it".into())
            })?;
        let context = read_workflow_context(pool, session_id)
            .await?
            .ok_or(ApiError::Unauthorized)?;
        return Ok(WorkflowMainSessionView { session, context });
    }
    // A second prepare key for the same Task joins the first durable
    // reservation, including recovery before any Session has been inserted.
    let shared = if let Some(issue_id) = request.issue_id {
        sqlx::query("SELECT * FROM workflow_main_session_requests WHERE issue_id=? ORDER BY created_at,request_id LIMIT 1")
            .bind(issue_id).fetch_optional(&mut *tx).await?
    } else {
        None
    };
    if shared
        .as_ref()
        .is_some_and(|row| row.get::<Uuid, _>("workflow_id") != request.workflow_id)
    {
        return Err(ApiError::Conflict(
            "Task main Session preparation already selected another publication".into(),
        ));
    }
    if shared
        .as_ref()
        .is_some_and(|row| row.get::<Option<String>, _>("completed_at").is_some())
    {
        return Err(ApiError::Conflict(
            "Task's prepared original main Session was deleted; it cannot be replaced".into(),
        ));
    }
    let captured_config = if let Some(row) = &existing_instance {
        row.try_get::<Option<String>, _>("main_agent_config_json")?
    } else {
        template
            .main_agent_config
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| ApiError::BadRequest("Invalid main Agent configuration".into()))?
    };
    let capture = prior.as_ref().or(shared.as_ref());
    let session_id = capture
        .map(|row| row.get("session_id"))
        .unwrap_or_else(Uuid::new_v4);
    let captured_json = if let Some(row) = capture {
        row.try_get("main_agent_config_json")?
    } else {
        captured_config.ok_or_else(|| {
            ApiError::Conflict(
                "Configure this workflow's captured main Agent before opening its Session".into(),
            )
        })?
    };
    let captured_prompt = if let Some(row) = capture {
        row.try_get::<String, _>("main_agent_prompt")?
    } else if let Some(row) = &existing_instance {
        row.try_get::<Option<String>, _>("main_agent_prompt")?
            .unwrap_or_default()
    } else {
        template.main_agent_prompt.unwrap_or_default()
    };
    let reserved_workspace = capture
        .map(|row| row.try_get::<Option<Uuid>, _>("workspace_id"))
        .transpose()?
        .flatten();
    sqlx::query("INSERT INTO workflow_main_session_requests(project_id,request_id,request_hash,workflow_id,issue_id,session_id,workspace_id,main_agent_config_json,main_agent_prompt) VALUES (?,?,?,?,?,?,?,?,?) ON CONFLICT(project_id,request_id) DO NOTHING")
        .bind(request.project_id).bind(&request.request_id).bind(&hash).bind(request.workflow_id).bind(request.issue_id).bind(session_id).bind(reserved_workspace).bind(&captured_json).bind(&captured_prompt).execute(&mut *tx).await?;
    let existing_workspace = existing_instance
        .as_ref()
        .map(|row| row.try_get::<Option<Uuid>, _>("workspace_id"))
        .transpose()?
        .flatten();
    tx.commit().await?;
    let workspace_id = resolver
        .create_or_bind_main_workspace(WorkflowWorkspaceRequest {
            directory_path: None,
            issue_id: request.issue_id.unwrap_or(session_id),
            run_id: session_id,
            project_id: Some(request.project_id),
            existing_workspace_id: reserved_workspace.or(existing_workspace),
            repo_overrides: Vec::new(),
            branch_name: format!("vk/main-session/{session_id}"),
        })
        .await?;
    // Resolver recovers this reserved Session's workspace by stable marker.
    let agent_working_dir = Session::resolve_agent_working_dir(pool, workspace_id).await?;
    let captured: ExecutorConfig = serde_json::from_str(&captured_json)
        .map_err(|_| ApiError::Conflict("Captured main Agent configuration is invalid".into()))?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("UPDATE workflow_main_session_requests SET workspace_id=COALESCE(workspace_id,?) WHERE project_id=? AND request_id=?")
        .bind(workspace_id).bind(request.project_id).bind(&request.request_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO sessions(id,workspace_id,name,executor,agent_working_dir) VALUES (?,?,?,?,?) ON CONFLICT(id) DO NOTHING")
        .bind(session_id).bind(workspace_id).bind(&template.name).bind(captured.executor.to_string()).bind(agent_working_dir).execute(&mut *tx).await?;
    sqlx::query("UPDATE session_project_memberships SET project_id=? WHERE session_id=?")
        .bind(request.project_id)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
    if let Some(issue_id) = request.issue_id {
        let other:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_main_session_bindings WHERE prepared_issue_id=? AND session_id<>?)")
            .bind(issue_id).bind(session_id).fetch_one(&mut *tx).await?;
        if other {
            return Err(ApiError::Conflict(
                "Task already has a prepared main Session".into(),
            ));
        }
    }
    sqlx::query("INSERT INTO workflow_main_session_bindings(session_id,workflow_id,prepared_issue_id,main_agent_config_json,main_agent_prompt) VALUES (?,?,?,?,?) ON CONFLICT(session_id) DO NOTHING")
        .bind(session_id).bind(request.workflow_id).bind(request.issue_id).bind(captured_json).bind(captured_prompt).execute(&mut *tx).await?;
    if let Some(row) = existing_instance {
        let attached=sqlx::query("UPDATE workflow_attempts SET main_session_id=?,main_session_bound_at=COALESCE(main_session_bound_at,datetime('now','subsec')),workspace_id=COALESCE(workspace_id,?) WHERE id=? AND (main_session_bound_at IS NULL OR main_session_id=?)")
            .bind(session_id).bind(workspace_id).bind(row.try_get::<Uuid,_>("id")?).bind(session_id).execute(&mut *tx).await?.rows_affected();
        if attached != 1 {
            return Err(ApiError::Conflict(
                "Workflow main Session attachment lost a concurrent race".into(),
            ));
        }
    }
    sqlx::query("UPDATE workflow_main_session_requests SET workspace_id=?,completed_at=datetime('now','subsec') WHERE session_id=?")
        .bind(workspace_id).bind(session_id).execute(&mut *tx).await?;
    tx.commit().await?;
    let session = Session::find_by_id(pool, session_id)
        .await?
        .ok_or_else(|| ApiError::Conflict("Prepared Session was removed".into()))?;
    let context = read_workflow_context(pool, session_id)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    Ok(WorkflowMainSessionView { session, context })
}

const NOTIFICATION_SELECT: &str = "SELECT n.*,COALESCE(e.status,r.status) AS current_status,CASE WHEN n.kind<>'interaction_required' OR e.id IS NULL OR e.status<>n.observed_status OR r.status IN ('succeeded','failed','canceled') OR EXISTS(SELECT 1 FROM workflow_interaction_responses a WHERE a.node_execution_id=n.node_execution_id) THEN 1 ELSE 0 END AS is_resolved FROM workflow_notifications n JOIN workflow_runs r ON r.id=n.run_id LEFT JOIN node_executions e ON e.id=n.node_execution_id";

fn notification_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<WorkflowNotificationView, sqlx::Error> {
    Ok(WorkflowNotificationView {
        id: row.try_get("id")?,
        sequence: row.try_get("sequence")?,
        instance_id: row.try_get("instance_id")?,
        run_id: row.try_get("run_id")?,
        main_session_id: row.try_get("main_session_id")?,
        event_key: row.try_get("event_key")?,
        kind: row.try_get("kind")?,
        node_execution_id: row.try_get("node_execution_id")?,
        interaction_id: row.try_get("interaction_id")?,
        observed_status: row.try_get("observed_status")?,
        summary: row.try_get("summary")?,
        created_at: row.try_get("created_at")?,
        current_status: row.try_get("current_status")?,
        is_resolved: row.try_get("is_resolved")?,
    })
}

pub async fn list_workflow_notifications(
    pool: &SqlitePool,
    session_id: Uuid,
    cursor: i64,
    limit: u32,
) -> Result<WorkflowNotificationPage, ApiError> {
    let limit = limit.clamp(1, 100);
    let query = format!(
        "{NOTIFICATION_SELECT} WHERE n.main_session_id=? AND n.sequence>? ORDER BY n.sequence LIMIT ?"
    );
    let rows = sqlx::query(&query)
        .bind(session_id)
        .bind(cursor.max(0))
        .bind(i64::from(limit) + 1)
        .fetch_all(pool)
        .await?;
    let more = rows.len() > limit as usize;
    let notifications = rows
        .iter()
        .take(limit as usize)
        .map(notification_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = if more {
        notifications.last().map(|item| item.sequence)
    } else {
        None
    };
    Ok(WorkflowNotificationPage {
        notifications,
        next_cursor,
    })
}

#[derive(Debug)]
struct ManagementScope {
    project_id: Uuid,
    issue_id: Option<Uuid>,
    instance_id: Option<Uuid>,
    workflow_id: Uuid,
    publication_id: Uuid,
    workspace_id: Option<Uuid>,
    main_session_id: Option<Uuid>,
    source_message_id: Option<Uuid>,
    namespace: String,
    integration_id: Option<Uuid>,
    handover_run_id: Option<Uuid>,
    handover_batch_id: Option<Uuid>,
}

async fn resolve_scope_in(
    conn: &mut SqliteConnection,
    caller: &WorkflowManagementCaller,
) -> Result<ManagementScope, ApiError> {
    let scope = match caller {
        WorkflowManagementCaller::MainSession {
            session_id,
            agent_run_id,
            turn_id,
            token_hash,
        } => {
            let row=sqlx::query("SELECT b.*,m.project_id,s.workspace_id,w.project_id AS template_project_id FROM workflow_main_session_bindings b JOIN sessions s ON s.id=b.session_id JOIN session_project_memberships m ON m.session_id=s.id JOIN workflows w ON w.id=b.workflow_id WHERE b.session_id=?")
                .bind(session_id).fetch_optional(&mut *conn).await?.ok_or(ApiError::Unauthorized)?;
            let scoped = agent_run_id.is_some() || turn_id.is_some() || token_hash.is_some();
            if scoped {
                if agent_run_id.is_none()
                    || turn_id.is_none()
                    || token_hash.is_none()
                    || row.try_get::<Option<Uuid>, _>("actual_main_agent_run_id")? != *agent_run_id
                    || row.try_get::<Option<Uuid>, _>("actual_main_turn_id")? != *turn_id
                    || row.try_get::<Option<String>, _>("token_hash")?.as_ref()
                        != token_hash.as_ref()
                {
                    return Err(ApiError::Unauthorized);
                }
                let real:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM agent_runs r JOIN agent_turns t ON t.agent_run_id=r.id WHERE r.id=? AND r.session_id=? AND t.id=? AND (json_extract(t.input_message,'$.role')='user' OR (json_extract(t.input_message,'$.role')='system' AND EXISTS(SELECT 1 FROM workflow_handover_events e JOIN workflow_runs w ON w.id=e.run_id JOIN workflow_attempts a ON a.id=w.attempt_id WHERE e.agent_run_id=r.id AND e.session_id=r.session_id AND e.turn_id=t.id AND e.state='claimed' AND a.main_session_id=e.session_id AND w.status IN ('running','awaiting_human','awaiting_arena') AND w.error_text IS NULL AND NOT EXISTS(SELECT 1 FROM workflow_stop_intents s WHERE s.run_id=w.id)))))")
                    .bind(agent_run_id).bind(session_id).bind(turn_id).fetch_one(&mut *conn).await?;
                if !real {
                    return Err(ApiError::Unauthorized);
                }
            }
            let project_id: Uuid = row.try_get("project_id")?;
            if row
                .try_get::<Option<Uuid>, _>("template_project_id")?
                .is_some_and(|id| id != project_id)
            {
                return Err(ApiError::Forbidden(
                    "Workflow binding project changed".into(),
                ));
            }
            let instance=sqlx::query("SELECT a.id,a.issue_id,a.workflow_id,a.workspace_id,t.project_id,s.template_id FROM workflow_attempts a JOIN tasks t ON t.id=a.task_id LEFT JOIN workflow_attempt_sources s ON s.attempt_id=a.id WHERE a.main_session_id=?")
                .bind(session_id).fetch_optional(&mut *conn).await?;
            if let Some(instance) = &instance {
                let publication_id = instance
                    .try_get::<Option<Uuid>, _>("template_id")?
                    .unwrap_or(instance.try_get("workflow_id")?);
                if instance.try_get::<Uuid, _>("project_id")? != project_id
                    || publication_id != row.try_get::<Uuid, _>("workflow_id")?
                    || instance.try_get::<Option<Uuid>, _>("workspace_id")?
                        != Some(row.try_get("workspace_id")?)
                {
                    return Err(ApiError::Forbidden(
                        "Workflow instance ancestry changed".into(),
                    ));
                }
            }
            let handover: Option<(Uuid, Uuid)> = if scoped {
                sqlx::query_as("SELECT e.run_id,e.batch_id FROM workflow_handover_events e JOIN agent_turns t ON t.id=e.turn_id WHERE e.agent_run_id=? AND e.turn_id=? AND e.session_id=? AND e.state='claimed' AND json_extract(t.input_message,'$.role')='system'")
                    .bind(agent_run_id).bind(turn_id).bind(session_id).fetch_optional(&mut *conn).await?
            } else {
                None
            };
            ManagementScope {
                project_id,
                issue_id: instance
                    .as_ref()
                    .map(|r| r.try_get("issue_id"))
                    .transpose()?
                    .or(row.try_get("prepared_issue_id")?),
                instance_id: instance.as_ref().map(|r| r.try_get("id")).transpose()?,
                workflow_id: instance
                    .as_ref()
                    .map(|r| r.try_get("workflow_id"))
                    .transpose()?
                    .unwrap_or(row.try_get("workflow_id")?),
                publication_id: row.try_get("workflow_id")?,
                workspace_id: Some(row.try_get("workspace_id")?),
                main_session_id: Some(*session_id),
                source_message_id: if scoped {
                    row.try_get("source_message_id")?
                } else {
                    None
                },
                namespace: format!("main-session:{session_id}"),
                integration_id: None,
                handover_run_id: handover.map(|identity| identity.0),
                handover_batch_id: handover.map(|identity| identity.1),
            }
        }
        WorkflowManagementCaller::Instance {
            instance_id,
            namespace,
            integration_id,
        } => {
            let row=sqlx::query("SELECT a.id,a.issue_id,a.workflow_id,a.workspace_id,a.main_session_id,t.project_id,s.template_id FROM workflow_attempts a JOIN tasks t ON t.id=a.task_id LEFT JOIN workflow_attempt_sources s ON s.attempt_id=a.id WHERE a.id=?")
                .bind(instance_id).fetch_optional(&mut *conn).await?.ok_or_else(||ApiError::BadRequest("Workflow instance not found".into()))?;
            ManagementScope {
                project_id: row.try_get("project_id")?,
                issue_id: Some(row.try_get("issue_id")?),
                instance_id: Some(*instance_id),
                workflow_id: row.try_get("workflow_id")?,
                publication_id: row
                    .try_get::<Option<Uuid>, _>("template_id")?
                    .unwrap_or(row.try_get("workflow_id")?),
                workspace_id: row.try_get("workspace_id")?,
                main_session_id: row.try_get("main_session_id")?,
                source_message_id: None,
                namespace: namespace.clone(),
                integration_id: *integration_id,
                handover_run_id: None,
                handover_batch_id: None,
            }
        }
    };
    if let Some(integration_id) = scope.integration_id {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM external_integrations i JOIN external_integration_projects p ON p.integration_id=i.id WHERE i.id=? AND i.enabled=1 AND p.project_id=?)")
            .bind(integration_id).bind(scope.project_id).fetch_one(&mut *conn).await?;
        if !valid {
            return Err(ApiError::Forbidden(
                "External integration project access was revoked".into(),
            ));
        }
    }
    Ok(scope)
}

pub async fn verify_management_caller(
    pool: &SqlitePool,
    caller: &WorkflowManagementCaller,
) -> Result<(), ApiError> {
    let mut conn = pool.acquire().await?;
    resolve_scope_in(&mut conn, caller).await?;
    Ok(())
}

fn action_name(action: WorkflowSubmissionAction) -> &'static str {
    match action {
        WorkflowSubmissionAction::Start => "start",
        WorkflowSubmissionAction::Retry => "retry",
        WorkflowSubmissionAction::Rework => "rework",
    }
}
fn policy_name(policy: WorkflowActivePolicy) -> &'static str {
    match policy {
        WorkflowActivePolicy::AfterCurrent => "after_current",
        WorkflowActivePolicy::StopThenRun => "stop_then_run",
    }
}
fn terminal(status: &str) -> bool {
    matches!(status, "succeeded" | "failed" | "canceled")
}

async fn insert_agent_sessions_in(
    conn: &mut SqliteConnection,
    graph: &mut WorkflowGraph,
    workspace_id: Uuid,
    project_id: Uuid,
    working_dir: Option<&str>,
) -> Result<(), ApiError> {
    for node in graph.nodes.iter_mut().filter(|node| {
        matches!(
            node.kind,
            WorkflowNodeKind::Agent | WorkflowNodeKind::TaskSplitter
        )
    }) {
        if node.data.session_id.is_some() {
            continue;
        }
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO sessions(id,workspace_id,name,agent_working_dir) VALUES (?,?,?,?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(format!(
            "Workflow {}",
            node.data.display_name.as_deref().unwrap_or(&node.id)
        ))
        .bind(working_dir)
        .execute(&mut *conn)
        .await?;
        sqlx::query("UPDATE session_project_memberships SET project_id=? WHERE session_id=?")
            .bind(project_id)
            .bind(id)
            .execute(&mut *conn)
            .await?;
        node.data.session_id = Some(id.to_string());
    }
    Ok(())
}

async fn accepted_view_in(
    conn: &mut SqliteConnection,
    run_id: Uuid,
    request_id: &str,
) -> Result<AcceptedWorkflowSubmission, ApiError> {
    let row=sqlx::query("SELECT r.id,r.issue_id,r.attempt_id,q.project_id,q.phase FROM workflow_runs r JOIN workflow_run_queue q ON q.run_id=r.id WHERE r.id=?")
        .bind(run_id).fetch_one(&mut *conn).await?;
    Ok(AcceptedWorkflowSubmission {
        request_id: request_id.into(),
        project_id: row.try_get("project_id")?,
        issue_id: row.try_get("issue_id")?,
        instance_id: row.try_get("attempt_id")?,
        run_id,
        phase: row.try_get("phase")?,
    })
}

/// Caller authority, source CAS, new business identities, frozen definition,
/// FIFO entry, dependency and replacement-stop intent are one write transaction.
/// The function never invokes a provider; the existing dispatcher owns launch.
pub async fn submit_workflow(
    pool: &SqlitePool,
    caller: &WorkflowManagementCaller,
    submission: WorkflowSubmission,
    reserved_run_id: Option<Uuid>,
    trigger_source: &str,
) -> Result<AcceptedWorkflowSubmission, ApiError> {
    validate_request_id(&submission.request_id)?;
    let hash = request_hash(&submission)?;
    // Reject paths at the existing file boundary. The actual workspace is read
    // from the durable Session/instance, never a tool-supplied directory.
    let materials = submission
        .material_paths
        .iter()
        .map(|path| crate::routes::integrations::files::relative_path(path))
        .collect::<Result<Vec<_>, _>>()?;
    let mut read_conn = pool.acquire().await?;
    let initial_scope = resolve_scope_in(&mut read_conn, caller).await?;
    if initial_scope.handover_run_id.is_some() {
        return Err(ApiError::Forbidden("An automatic dispatch handover cannot submit a new Run; dispatch the accepted batch instead".into()));
    }
    if let Some(instance_id) = initial_scope.instance_id {
        if let Some(row)=sqlx::query("SELECT run_id,request_hash FROM workflow_run_submissions WHERE caller_namespace=? AND instance_id=? AND request_id=?")
            .bind(&initial_scope.namespace).bind(instance_id).bind(&submission.request_id).fetch_optional(&mut *read_conn).await? {
            if row.try_get::<String,_>("request_hash")?!=hash {return Err(ApiError::Conflict("IDEMPOTENCY_CONFLICT: request_id has different parameters".into()));}
            let view=accepted_view_in(&mut read_conn,row.try_get("run_id")?,&submission.request_id).await?;
            return Ok(view);
        }
    }
    let workspace_id = initial_scope.workspace_id.ok_or_else(|| {
        ApiError::Conflict(
            "Configure this workflow instance's project workspace before submitting".into(),
        )
    })?;
    let root: Option<String> =
        sqlx::query_scalar("SELECT container_ref FROM workspaces WHERE id=?")
            .bind(workspace_id)
            .fetch_optional(&mut *read_conn)
            .await?
            .flatten();
    let graph_json: String = if let Some(instance_id) = initial_scope.instance_id {
        sqlx::query_scalar("SELECT COALESCE(a.frozen_graph_json,w.graph_json) FROM workflow_attempts a JOIN workflows w ON w.id=a.workflow_id WHERE a.id=?")
            .bind(instance_id).fetch_one(&mut *read_conn).await?
    } else {
        sqlx::query_scalar("SELECT graph_json FROM workflows WHERE id=?")
            .bind(initial_scope.workflow_id)
            .fetch_one(&mut *read_conn)
            .await?
    };
    let preflight_graph: WorkflowGraph = serde_json::from_str(&graph_json)
        .map_err(|_| ApiError::Conflict("Workflow graph is invalid".into()))?;
    drop(read_conn);
    // Business task authority must be available before accepting runtime work.
    crate::routes::project_store::read(pool, initial_scope.project_id).await?;
    if preflight_graph
        .nodes
        .iter()
        .any(|node| node.kind == WorkflowNodeKind::TaskSplitter)
    {
        if let Some(issue_id) = initial_scope.issue_id {
            prepare_workflow_main_session(
                pool,
                PrepareWorkflowMainSessionRequest {
                    project_id: initial_scope.project_id,
                    workflow_id: initial_scope.publication_id,
                    request_id: format!("splitter-main:{issue_id}"),
                    issue_id: Some(issue_id),
                },
                &BoundMainWorkspace(workspace_id),
            )
            .await?;
        } else if initial_scope.main_session_id.is_none() {
            return Err(ApiError::Conflict("A task-splitter workflow requires its original main Agent Session before acceptance".into()));
        }
    }
    let working_dir = Session::resolve_agent_working_dir(pool, workspace_id).await?;
    if !materials.is_empty() {
        let mut root = std::path::PathBuf::from(
            root.ok_or_else(|| ApiError::Conflict("Workflow workspace is not ready".into()))?,
        );
        if let Some(relative) = &working_dir {
            root.push(relative);
        }
        let root = tokio::fs::canonicalize(root)
            .await
            .map_err(|_| ApiError::Conflict("Workflow workspace is unavailable".into()))?;
        for path in &materials {
            let target = tokio::fs::canonicalize(root.join(path))
                .await
                .map_err(|_| ApiError::BadRequest("Material path does not exist".into()))?;
            if !target.starts_with(&root) {
                return Err(ApiError::BadRequest(
                    "Material path leaves the workflow workspace".into(),
                ));
            }
        }
    }
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let mut scope = resolve_scope_in(&mut tx, caller).await?;
    if scope.handover_run_id.is_some() {
        return Err(ApiError::Forbidden(
            "An automatic dispatch handover cannot submit a new Run".into(),
        ));
    }
    if scope.workspace_id != Some(workspace_id) {
        return Err(ApiError::Conflict(
            "Workflow workspace changed before acceptance".into(),
        ));
    }
    if let Some(instance_id) = scope.instance_id {
        if let Some(row)=sqlx::query("SELECT run_id,request_hash FROM workflow_run_submissions WHERE caller_namespace=? AND instance_id=? AND request_id=?")
            .bind(&scope.namespace).bind(instance_id).bind(&submission.request_id).fetch_optional(&mut *tx).await? {
            if row.try_get::<String,_>("request_hash")?!=hash {return Err(ApiError::Conflict("IDEMPOTENCY_CONFLICT: request_id has different parameters".into()));}
            let view=accepted_view_in(&mut tx,row.try_get("run_id")?,&submission.request_id).await?;
            tx.commit().await?;
            return Ok(view);
        }
    }
    if submission.source_message_id.is_some()
        && submission.source_message_id != scope.source_message_id
    {
        return Err(ApiError::BadRequest(
            "source_message_id must be the real User input of this verified main-Agent launch"
                .into(),
        ));
    }
    if let Some(integration_id) = scope.integration_id {
        let enabled: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workflows WHERE id=? AND external_enabled=1)",
        )
        .bind(scope.publication_id)
        .fetch_one(&mut *tx)
        .await?;
        if !enabled {
            return Err(ApiError::Conflict(
                "Workflow publication is not enabled for external calls".into(),
            ));
        }
        let _ = integration_id;
    }
    let run_id = reserved_run_id.unwrap_or_else(Uuid::new_v4);
    let latest: Option<Uuid> = if let Some(instance_id) = scope.instance_id {
        sqlx::query_scalar("SELECT latest_run_id FROM workflow_attempts WHERE id=?")
            .bind(instance_id)
            .fetch_one(&mut *tx)
            .await?
    } else {
        None
    };
    match submission.action {
        WorkflowSubmissionAction::Start => {
            if latest.is_some()
                || submission.source_run_id.is_some()
                || submission.source_node_execution_id.is_some()
            {
                return Err(ApiError::Conflict("start is only for initial acceptance; use retry or rework on the existing instance".into()));
            }
            if submission.active_policy.is_some()
                || !matches!(submission.scope, WorkflowSubmissionScope::All)
            {
                return Err(ApiError::BadRequest(
                    "Initial execution does not have a source policy or partial scope".into(),
                ));
            }
        }
        WorkflowSubmissionAction::Retry | WorkflowSubmissionAction::Rework => {
            if submission.source_run_id.is_none() || latest != submission.source_run_id {
                return Err(ApiError::Conflict("STALE_EXECUTION_BASIS: source_run_id must be this instance's latest accepted Run; reload its current history".into()));
            }
        }
    }
    let source = if let Some(id) = submission.source_run_id {
        Some(sqlx::query("SELECT id,attempt_id,status,input_text FROM workflow_runs WHERE id=? AND attempt_id=?")
            .bind(id).bind(scope.instance_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::BadRequest("Source Run does not belong to this instance".into()))?)
    } else {
        None
    };
    let source_status = source
        .as_ref()
        .map(|r| r.try_get::<String, _>("status"))
        .transpose()?;
    let source_unfinished = source_status
        .as_deref()
        .is_some_and(|status| !terminal(status));
    if source_unfinished
        && submission.action == WorkflowSubmissionAction::Rework
        && submission.active_policy.is_none()
    {
        return Err(ApiError::BadRequest("Choose after_current or stop_then_run for an unfinished source; no implicit stop is allowed".into()));
    }
    if submission.action == WorkflowSubmissionAction::Retry
        && (source_unfinished || submission.active_policy.is_some())
    {
        return Err(ApiError::Conflict(
            "INVALID_RETRY_TARGET: Retry requires a settled failed source execution".into(),
        ));
    }
    let input = match submission.action {
        WorkflowSubmissionAction::Retry => source
            .as_ref()
            .ok_or_else(|| ApiError::BadRequest("Retry requires a source".into()))?
            .try_get::<String, _>("input_text")?,
        _ => submission
            .input_text
            .as_ref()
            .filter(|input| !input.trim().is_empty())
            .cloned()
            .ok_or_else(|| {
                ApiError::BadRequest("Accepted requirements are required for start/rework".into())
            })?,
    };
    let input = if materials.is_empty() || submission.action == WorkflowSubmissionAction::Retry {
        input
    } else {
        format!(
            "{input}\n\nProject-relative materials:\n{}",
            materials
                .iter()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    let mut staged_task = None;
    let mut graph: WorkflowGraph = if let Some(instance_id) = scope.instance_id {
        let json:String=sqlx::query_scalar("SELECT COALESCE(a.frozen_graph_json,w.graph_json) FROM workflow_attempts a JOIN workflows w ON w.id=a.workflow_id WHERE a.id=?")
            .bind(instance_id).fetch_one(&mut *tx).await?;
        serde_json::from_str(&json)
            .map_err(|_| ApiError::Conflict("Frozen workflow is invalid".into()))?
    } else {
        let row=sqlx::query("SELECT graph_json,name,revision,main_agent_config_json,main_agent_prompt FROM workflows WHERE id=?")
            .bind(scope.publication_id).fetch_one(&mut *tx).await?;
        let mut graph: WorkflowGraph =
            serde_json::from_str(&row.try_get::<String, _>("graph_json")?)
                .map_err(|_| ApiError::BadRequest("Workflow graph is invalid".into()))?;
        for node in &mut graph.nodes {
            node.data.session_id = None;
        }
        let issue_id = if let Some(issue_id) = scope.issue_id {
            issue_id
        } else {
            let status_id:Uuid=sqlx::query_scalar("SELECT id FROM local_project_statuses WHERE project_id=? AND hidden=0 ORDER BY CASE WHEN lower(name)='todo' THEN 0 ELSE 1 END,sort_order LIMIT 1")
                .bind(scope.project_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::Conflict("Project has no Task status".into()))?;
            let (issue_id, staged) = insert_local_issue(
                &mut tx,
                CreateTaskRequest {
                    id: Some(crate::routes::project_store::workflow_task_id(
                        &scope.namespace,
                        &submission.request_id,
                    )),
                    project_id: scope.project_id,
                    status_id,
                    title: input
                        .lines()
                        .next()
                        .unwrap_or("Workflow")
                        .chars()
                        .take(160)
                        .collect(),
                    description: Some(input.clone()),
                    priority: None,
                    start_date: None,
                    target_date: None,
                    completed_at: None,
                    sort_order: 0.0,
                    parent_issue_id: None,
                    parent_issue_sort_order: None,
                    extension_metadata: Value::Null,
                },
            )
            .await?;
            staged_task = Some(staged);
            issue_id
        };
        if let Some(existing)=sqlx::query("SELECT a.id,a.workflow_id,s.template_id,a.main_session_id,a.main_session_bound_at,a.workspace_id FROM workflow_attempts a LEFT JOIN workflow_attempt_sources s ON s.attempt_id=a.id WHERE a.issue_id=?")
            .bind(issue_id).fetch_optional(&mut *tx).await? {
            let publication=existing.try_get::<Option<Uuid>,_>("template_id")?.unwrap_or(existing.try_get("workflow_id")?);
            if publication!=scope.publication_id {return Err(ApiError::Conflict("INSTANCE_BINDING_CONFLICT: Task already has a different workflow instance".into()));}
            let bound_at:Option<String>=existing.try_get("main_session_bound_at")?;
            if bound_at.is_some() && existing.try_get::<Option<Uuid>,_>("main_session_id")?!=scope.main_session_id {
                return Err(ApiError::Conflict("Workflow instance's original main Session cannot be replaced".into()));
            }
            if existing.try_get::<Option<Uuid>,_>("workspace_id")?.is_some_and(|id|id!=workspace_id) {return Err(ApiError::Conflict("Prepared Session does not use the instance's fixed workspace".into()));}
            scope.instance_id=Some(existing.try_get("id")?);scope.workflow_id=existing.try_get("workflow_id")?;
            let json:String=sqlx::query_scalar("SELECT COALESCE(a.frozen_graph_json,w.graph_json) FROM workflow_attempts a JOIN workflows w ON w.id=a.workflow_id WHERE a.id=?")
                .bind(scope.instance_id).fetch_one(&mut *tx).await?;
            graph=serde_json::from_str(&json).map_err(|_|ApiError::Conflict("Workflow instance graph is invalid".into()))?;
            let latest:Option<Uuid>=sqlx::query_scalar("SELECT latest_run_id FROM workflow_attempts WHERE id=?").bind(scope.instance_id).fetch_one(&mut *tx).await?;
            if latest.is_some() {return Err(ApiError::Conflict("Task already has accepted work; read its existing workflow instance first".into()));}
        }else{
            let instance_id=Uuid::new_v4();let workflow_id=Uuid::new_v4();
            workflows::insert_workflow_attempt(&mut tx,instance_id,workflow_id,scope.project_id,issue_id,row.try_get("name")?,serde_json::to_string(&graph).map_err(|_|ApiError::BadRequest("Invalid workflow graph".into()))?).await?;
            sqlx::query("INSERT INTO workflow_attempt_sources(attempt_id,template_id,template_revision) VALUES (?,?,?)")
                .bind(instance_id).bind(scope.publication_id).bind(row.try_get::<i64,_>("revision")?).execute(&mut *tx).await?;
            sqlx::query("UPDATE workflows SET main_agent_config_json=?,main_agent_prompt=? WHERE id=?")
                .bind(row.try_get::<Option<String>,_>("main_agent_config_json")?).bind(row.try_get::<Option<String>,_>("main_agent_prompt")?).bind(workflow_id).execute(&mut *tx).await?;
            scope.instance_id=Some(instance_id);scope.workflow_id=workflow_id;
        }
        scope.issue_id = Some(issue_id);
        graph
    };
    let instance_id = scope
        .instance_id
        .ok_or_else(|| ApiError::Conflict("Workflow instance was not prepared".into()))?;
    let issue_id = scope
        .issue_id
        .ok_or_else(|| ApiError::Conflict("Workflow Task was not prepared".into()))?;
    if let Some(session_id) = scope.main_session_id {
        let attached=sqlx::query("UPDATE workflow_attempts SET main_session_id=?,main_session_bound_at=COALESCE(main_session_bound_at,datetime('now','subsec')),workspace_id=COALESCE(workspace_id,?) WHERE id=? AND (main_session_bound_at IS NULL OR main_session_id=?)")
            .bind(session_id).bind(workspace_id).bind(instance_id).bind(session_id).execute(&mut *tx).await?.rows_affected();
        if attached != 1 {
            return Err(ApiError::Conflict(
                "Workflow instance already had a different main Session".into(),
            ));
        }
        sqlx::query("UPDATE workflows SET main_agent_config_json=(SELECT main_agent_config_json FROM workflow_main_session_bindings WHERE session_id=?),main_agent_prompt=(SELECT main_agent_prompt FROM workflow_main_session_bindings WHERE session_id=?) WHERE id=? AND EXISTS(SELECT 1 FROM workflow_main_session_bindings WHERE session_id=?) AND NOT EXISTS(SELECT 1 FROM workflow_attempts WHERE workflow_id=? AND definition_locked_at IS NOT NULL)")
            .bind(session_id).bind(session_id).bind(scope.workflow_id).bind(session_id).bind(scope.workflow_id).execute(&mut *tx).await?;
    }
    insert_agent_sessions_in(
        &mut tx,
        &mut graph,
        workspace_id,
        scope.project_id,
        working_dir.as_deref(),
    )
    .await?;
    workflow::validation::validate_graph_for_run(&graph)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    // Node Session allocation is part of acceptance, not a graph author edit.
    sqlx::query("UPDATE workflows SET graph_json=? WHERE id=? AND NOT EXISTS(SELECT 1 FROM workflow_attempts a WHERE a.workflow_id=workflows.id AND a.definition_locked_at IS NOT NULL)")
        .bind(serde_json::to_string(&graph).map_err(|_|ApiError::BadRequest("Invalid graph".into()))?).bind(scope.workflow_id).execute(&mut *tx).await?;
    let mut actual_scope = if submission.action == WorkflowSubmissionAction::Retry {
        let execution_id = submission.source_node_execution_id.ok_or_else(|| {
            ApiError::BadRequest(
                "INVALID_RETRY_TARGET: Retry requires source_node_execution_id".into(),
            )
        })?;
        let node_id:String=sqlx::query_scalar("SELECT n.node_id FROM node_executions n WHERE n.id=?1 AND (n.run_id=?2 OR EXISTS(SELECT 1 FROM workflow_split_subtasks s JOIN workflow_split_batches b ON b.id=s.batch_id WHERE b.run_id=?2 AND s.node_execution_id=n.id AND s.state='failed')) AND n.status='failed' AND n.node_type IN ('agent','task_splitter','condition','transform')")
            .bind(execution_id).bind(submission.source_run_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::BadRequest("INVALID_RETRY_TARGET: Retry source must be an actual supported failed NodeExecution of this source Run".into()))?;
        WorkflowSubmissionScope::FromNodes {
            node_ids: vec![node_id],
        }
    } else {
        submission.scope.clone()
    };
    let mut batch_request = submission.clone();
    batch_request.scope = actual_scope.clone();
    let source_batch_id =
        match batch_rework::select_batch_in(&mut tx, &graph, &batch_request).await? {
            Some((batch_id, normalized_scope)) => {
                actual_scope = normalized_scope;
                Some(batch_id)
            }
            None => None,
        };
    let roots = match &actual_scope {
        WorkflowSubmissionScope::All => None,
        WorkflowSubmissionScope::FromNodes { node_ids } => Some(node_ids.as_slice()),
    };
    // Validate frozen IDs / route immediately where possible. Missing reuse
    // after acceptance becomes a queryable planning failure, never lost work.
    if let Some(ids) = roots {
        if ids.is_empty()
            || ids.iter().collect::<HashSet<_>>().len() != ids.len()
            || ids
                .iter()
                .any(|id| !graph.nodes.iter().any(|node| &node.id == id))
        {
            return Err(ApiError::BadRequest(
                "INVALID_REWORK_SCOPE: Partial scope requires distinct frozen-graph Node IDs"
                    .into(),
            ));
        }
    }
    let settled = if let Some(source_run_id) = submission.source_run_id {
        source_boundary_settled_in(&mut tx, source_run_id).await?
    } else {
        true
    };
    // Known-invalid settled input is a rejection, not an accepted failed Run.
    // Only a dependency that settles after acceptance may fail its fixed plan.
    let basis = RunPlanningBasis {
        action: submission.action,
        scope: actual_scope.clone(),
        source_run_id: submission.source_run_id,
        source_batch_id,
        source_node_execution_id: submission.source_node_execution_id,
        partial: submission.delivery_intent == Some(WorkflowDeliveryCompleteness::Partial),
    };
    let settled_plan = if settled {
        Some(plan_submission_in(&mut tx, &graph, workspace_id, &basis).await?)
    } else {
        None
    };
    let phase = if !settled {
        if submission.active_policy == Some(WorkflowActivePolicy::StopThenRun) {
            "stopping_source"
        } else {
            "waiting_for_source"
        }
    } else {
        "queued"
    };
    let none = HashSet::new();
    runner::initialize_workflow_run_in(
        &mut tx,
        run_id,
        scope.workflow_id,
        Some(instance_id),
        workspace_id,
        &TriggerWorkflowRequest {
            issue_id,
            workspace_id: Some(workspace_id),
            trigger_source: trigger_source.into(),
            input_text: input,
        },
        &graph,
        Some(scope.project_id),
        phase,
        Some(&none),
    )
    .await?;
    sqlx::query("INSERT INTO workflow_run_submissions(run_id,instance_id,caller_namespace,request_id,request_hash,action,material_paths_json,scope_json,source_run_id,source_node_execution_id,source_message_id,active_policy,source_batch_id) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(run_id).bind(instance_id).bind(&scope.namespace).bind(&submission.request_id).bind(hash).bind(action_name(submission.action)).bind(serde_json::to_string(&submission.material_paths).unwrap_or_else(|_|"[]".into())).bind(serde_json::to_string(&actual_scope).map_err(|_|ApiError::BadRequest("Invalid scope".into()))?).bind(submission.source_run_id).bind(submission.source_node_execution_id).bind(scope.source_message_id).bind(submission.active_policy.map(policy_name)).bind(source_batch_id).execute(&mut *tx).await?;
    if basis.partial {
        sqlx::query("INSERT INTO workflow_run_deliveries(run_id,completeness,source_run_id,source_batch_id) VALUES (?,'partial',?,?) ON CONFLICT(run_id) DO UPDATE SET completeness='partial',source_run_id=excluded.source_run_id,source_batch_id=excluded.source_batch_id")
            .bind(run_id).bind(basis.source_run_id).bind(source_batch_id).execute(&mut *tx).await?;
    }
    if !settled && submission.active_policy == Some(WorkflowActivePolicy::StopThenRun) {
        let source_run_id = submission
            .source_run_id
            .ok_or_else(|| ApiError::BadRequest("Stop policy requires a source".into()))?;
        sqlx::query("INSERT INTO workflow_stop_intents(id,caller_namespace,request_id,run_id,successor_run_id) VALUES (?,?,?,?,?)")
            .bind(Uuid::new_v4()).bind(&scope.namespace).bind(format!("replacement:{}",submission.request_id)).bind(source_run_id).bind(run_id).execute(&mut *tx).await?;
        cancel_pending_in(&mut tx, source_run_id).await?;
    }
    if let Some(plan) = settled_plan {
        if let Some(batch) = &plan.batch {
            batch_rework::apply_in(&mut tx, run_id, batch).await?;
        }
        apply_run_plan_in(
            &mut tx,
            run_id,
            &graph,
            workspace_id,
            Some(instance_id),
            issue_id,
            plan.nodes,
        )
        .await?;
    }
    let view = accepted_view_in(&mut tx, run_id, &submission.request_id).await?;
    if let Some(staged) = staged_task {
        staged.publish()?;
    }
    tx.commit().await?;
    Ok(view)
}

pub(crate) async fn source_boundary_settled_in(
    conn: &mut SqliteConnection,
    run_id: Uuid,
) -> Result<bool, ApiError> {
    let settled: bool = sqlx::query_scalar("WITH owned_runs(id) AS (SELECT ?1 UNION SELECT o.id FROM orchestration_runs o JOIN node_executions n ON n.arena_group_id=o.source_definition_id WHERE n.run_id=?1 AND o.product_kind='arena') SELECT EXISTS(SELECT 1 FROM workflow_runs WHERE id=?1 AND status IN ('succeeded','failed','canceled')) AND NOT EXISTS(SELECT 1 FROM workflow_project_slots WHERE run_id=?1) AND NOT EXISTS(SELECT 1 FROM node_executions WHERE run_id=?1 AND status IN ('running','cancelling','awaiting_human','awaiting_arena')) AND NOT EXISTS(SELECT 1 FROM orchestration_agent_run_links l LEFT JOIN agent_run_state s ON s.agent_run_id=l.agent_run_id WHERE l.orchestration_run_id IN (SELECT id FROM owned_runs) AND (s.agent_run_id IS NULL OR s.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed'))) AND NOT EXISTS(SELECT 1 FROM orchestration_agent_run_links l JOIN agent_run_attempts a ON a.agent_run_id=l.agent_run_id LEFT JOIN agent_process_registry p ON p.run_attempt_id=a.id WHERE l.orchestration_run_id IN (SELECT id FROM owned_runs) AND (p.run_attempt_id IS NULL OR p.registry_status<>'exited')) AND NOT EXISTS(SELECT 1 FROM orchestration_outbox WHERE orchestration_run_id IN (SELECT id FROM owned_runs) AND delivery_status IN ('pending','delivering','failed'))")
        .bind(run_id).fetch_one(&mut *conn).await?;
    Ok(settled && !super::handover::has_unsettled_in(conn, run_id).await?)
}

/// Accept a file-backed dispatch plan for one accepted split batch. The plan
/// is frozen in the database only after its manifest identity and graph scope
/// are checked; merely creating/editing dispatch.json never starts work.
pub async fn dispatch_workflow(
    pool: &SqlitePool,
    caller: &WorkflowManagementCaller,
    request: WorkflowDispatchRequest,
) -> Result<AcceptedWorkflowDispatch, ApiError> {
    validate_request_id(&request.request_id)?;
    let scope = {
        let mut conn = pool.acquire().await?;
        resolve_scope_in(&mut conn, caller).await?
    };
    let row = sqlx::query(
        "SELECT b.run_id,b.node_execution_id,b.manifest_path,b.manifest_sha256,b.state,r.attempt_id,r.graph_snapshot,r.workspace_id,n.node_id FROM workflow_split_batches b JOIN workflow_runs r ON r.id=b.run_id JOIN node_executions n ON n.id=b.node_execution_id WHERE b.id=?",
    )
    .bind(request.batch_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::BadRequest("Workflow split batch not found".into()))?;
    let run_id: Uuid = row.try_get("run_id")?;
    let attempt_id: Option<Uuid> = row.try_get("attempt_id")?;
    if attempt_id.is_none()
        || scope.instance_id != attempt_id
        || scope.workspace_id != Some(row.try_get("workspace_id")?)
    {
        return Err(ApiError::Forbidden(
            "Dispatch batch is outside the current workflow instance".into(),
        ));
    }
    if scope.handover_run_id.is_some_and(|id| id != run_id)
        || scope
            .handover_batch_id
            .is_some_and(|id| id != request.batch_id)
    {
        return Err(ApiError::Forbidden(
            "Automatic handover can dispatch only its original Run".into(),
        ));
    }
    let workspace_id: Uuid = row.try_get("workspace_id")?;
    let root = runner::workflow_control_root(pool, workspace_id).await?;
    let dispatch_path = checked_workflow_file(&root, &request.dispatch_path).await?;
    let dispatch_bytes = tokio::fs::read(&dispatch_path).await?;
    let dispatch_sha = sha256_bytes(&dispatch_bytes);
    let plan: DispatchPlan = serde_json::from_slice(&dispatch_bytes)
        .map_err(|error| ApiError::BadRequest(format!("Invalid dispatch plan: {error}")))?;
    let expected_sha: String = row.try_get("manifest_sha256")?;
    let manifest_bytes = batch_rework::verify_file(
        &root,
        &row.try_get::<String, _>("manifest_path")?,
        &expected_sha,
    )
    .await?;
    let manifest: SplitTaskManifest = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        ApiError::BadRequest(format!("Accepted split manifest is unavailable: {error}"))
    })?;
    validate_manifest(&manifest).map_err(ApiError::BadRequest)?;
    if manifest.run_id != run_id.to_string()
        || manifest.split_execution_id != row.try_get::<Uuid, _>("node_execution_id")?.to_string()
        || plan.manifest_sha256 != expected_sha
    {
        return Err(ApiError::Conflict(
            "Accepted split manifest changed after acceptance".into(),
        ));
    }
    let graph_json: String = row.try_get("graph_snapshot")?;
    let graph: WorkflowGraph = serde_json::from_str(&graph_json)
        .map_err(|_| ApiError::Conflict("Frozen workflow graph is invalid".into()))?;
    let node_id: String = row.try_get("node_id")?;
    let split_scope = dispatch_scope(&graph, &node_id).map_err(ApiError::BadRequest)?;
    validate_dispatch(&split_scope, &manifest, &plan).map_err(ApiError::BadRequest)?;
    for task in &manifest.tasks {
        let bound: (String, String) = sqlx::query_as("SELECT instruction_path,instruction_sha256 FROM workflow_split_subtasks WHERE batch_id=? AND subtask_id=?")
            .bind(request.batch_id).bind(&task.subtask_id).fetch_optional(pool).await?
            .ok_or_else(||ApiError::Conflict("Split task acceptance is inconsistent".into()))?;
        if bound.0 != task.instruction_path {
            return Err(ApiError::Conflict(
                "Accepted instruction path changed".into(),
            ));
        }
        batch_rework::verify_file(&root, &bound.0, &bound.1).await?;
    }
    let hash = request_hash(&json!({"request":request,"dispatch_sha256":dispatch_sha}))?;

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let current_scope = resolve_scope_in(&mut tx, caller).await?;
    if current_scope.instance_id != attempt_id || current_scope.workspace_id != Some(workspace_id) {
        return Err(ApiError::Forbidden(
            "Workflow dispatch scope changed".into(),
        ));
    }
    if current_scope.handover_run_id.is_some_and(|id| id != run_id)
        || current_scope
            .handover_batch_id
            .is_some_and(|id| id != request.batch_id)
    {
        return Err(ApiError::Forbidden(
            "Automatic handover dispatch scope changed".into(),
        ));
    }
    if let Some(existing) = sqlx::query(
        "SELECT request_hash,result_json FROM workflow_operation_requests WHERE caller_namespace=? AND operation='workflow_dispatch' AND resource_id=? AND request_id=?",
    )
    .bind(&scope.namespace)
    .bind(request.batch_id)
    .bind(&request.request_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        if existing.try_get::<String,_>("request_hash")? != hash {
            return Err(ApiError::Conflict("IDEMPOTENCY_CONFLICT: dispatch request or file contents changed".into()));
        }
        let result: String = existing.try_get("result_json")?;
        return serde_json::from_str(&result)
            .map_err(|_| ApiError::Conflict("Stored dispatch result is invalid".into()));
    }
    let changed = sqlx::query("UPDATE workflow_split_batches SET dispatch_path=?,dispatch_sha256=?,state='dispatching',updated_at=datetime('now','subsec') WHERE id=? AND state='accepted' AND EXISTS(SELECT 1 FROM workflow_runs r JOIN workflow_project_slots p ON p.run_id=r.id WHERE r.id=workflow_split_batches.run_id AND r.status IN ('running','awaiting_human','awaiting_arena') AND r.error_text IS NULL) AND NOT EXISTS(SELECT 1 FROM workflow_stop_intents i WHERE i.run_id=workflow_split_batches.run_id)")
        .bind(request.dispatch_path.replace('\\', "/")).bind(&dispatch_sha).bind(request.batch_id).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError::Conflict(
            "DISPATCH_NOT_ACCEPTED: batch already dispatched or run is no longer accepting work"
                .into(),
        ));
    }
    let mut assigned_count = 0u32;
    for (stage_index, stage) in plan.stages.iter().enumerate() {
        for assignment in &stage.assignments {
            let changed = sqlx::query(
                "UPDATE workflow_split_subtasks SET stage_index=?,node_id=?,state='queued',updated_at=datetime('now','subsec') WHERE batch_id=? AND subtask_id=? AND state='pending'",
            )
            .bind(stage_index as i64)
            .bind(&assignment.node_id)
            .bind(request.batch_id)
            .bind(&assignment.subtask_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if changed != 1 {
                return Err(ApiError::Conflict(
                    "Dispatch subtask disappeared during acceptance".into(),
                ));
            }
            assigned_count += 1;
        }
    }
    // A user turn can accept dispatch before the automatic System handover is
    // claimed. Settle only the unlaunched event in this acceptance transaction;
    // an already claimed Agent must still reach its real process boundary.
    sqlx::query("UPDATE workflow_handover_events SET state='completed',error_text=NULL,updated_at=datetime('now','subsec') WHERE batch_id=? AND run_id=? AND state='pending' AND agent_run_id IS NULL")
        .bind(request.batch_id)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
    let accepted = AcceptedWorkflowDispatch {
        request_id: request.request_id.clone(),
        batch_id: request.batch_id,
        run_id,
        assigned_count,
        stage_count: plan.stages.len() as u32,
    };
    sqlx::query("INSERT INTO workflow_operation_requests(caller_namespace,operation,resource_id,request_id,request_hash,result_json) VALUES (?,?,?,?,?,?)")
        .bind(&scope.namespace)
        .bind("workflow_dispatch")
        .bind(request.batch_id)
        .bind(&request.request_id)
        .bind(hash)
        .bind(serde_json::to_string(&accepted).map_err(|error| ApiError::BadRequest(error.to_string()))?)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(accepted)
}

pub(super) async fn checked_workflow_file(
    root: &Path,
    relative: &str,
) -> Result<PathBuf, ApiError> {
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|_| ApiError::Conflict("Workflow workspace is unavailable".into()))?;
    let path = relative_control_path(&root, relative)
        .map_err(|_| ApiError::BadRequest("Invalid workflow file path".into()))?;
    let path = tokio::fs::canonicalize(path)
        .await
        .map_err(|_| ApiError::BadRequest(format!("Workflow file is unavailable: {relative}")))?;
    if !path.starts_with(&root) || !tokio::fs::metadata(&path).await?.is_file() {
        return Err(ApiError::BadRequest(
            "Workflow file leaves its workspace or is not a file".into(),
        ));
    }
    Ok(path)
}

async fn source_executions_in(
    conn: &mut SqliteConnection,
    run_id: Uuid,
) -> Result<Vec<ReworkSourceExecution>, ApiError> {
    let rows=sqlx::query("SELECT source_node_execution_id,node_id,iteration,status,output_text FROM workflow_effective_node_executions WHERE run_id=? UNION ALL SELECT NULL,node_id,0,'skipped',NULL FROM workflow_node_dispositions WHERE run_id=?")
        .bind(run_id).bind(run_id).fetch_all(&mut *conn).await?;
    rows.into_iter()
        .map(|row| {
            Ok(ReworkSourceExecution {
                execution_id: row
                    .try_get::<Option<Uuid>, _>("source_node_execution_id")?
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
                node_id: row.try_get("node_id")?,
                iteration: row.try_get("iteration")?,
                status: match row.try_get::<String, _>("status")?.as_str() {
                    "succeeded" => workflow::planner::NodeExecutionStatus::Succeeded,
                    "skipped" => workflow::planner::NodeExecutionStatus::Skipped,
                    "failed" => workflow::planner::NodeExecutionStatus::Failed,
                    "cancelled" => workflow::planner::NodeExecutionStatus::Cancelled,
                    _ => workflow::planner::NodeExecutionStatus::Pending,
                },
                output_text: row.try_get("output_text")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(ApiError::from)
}

struct RunPlanningBasis {
    action: WorkflowSubmissionAction,
    scope: WorkflowSubmissionScope,
    source_run_id: Option<Uuid>,
    source_batch_id: Option<Uuid>,
    source_node_execution_id: Option<Uuid>,
    partial: bool,
}

struct SubmissionPlan {
    nodes: ReworkPlan,
    batch: Option<batch_rework::BatchReworkPlan>,
}

async fn plan_submission_in(
    conn: &mut SqliteConnection,
    graph: &WorkflowGraph,
    workspace_id: Uuid,
    basis: &RunPlanningBasis,
) -> Result<SubmissionPlan, ApiError> {
    let batch = if let Some(batch_id) = basis.source_batch_id {
        Some(
            batch_rework::plan_in(
                conn,
                graph,
                workspace_id,
                basis.source_run_id.ok_or_else(|| {
                    ApiError::Conflict("Batch reuse requires its exact source Run".into())
                })?,
                batch_id,
                basis.partial,
                basis.action,
                &basis.scope,
                basis.source_node_execution_id,
            )
            .await?,
        )
    } else {
        None
    };
    let nodes = if let Some(batch) = &batch {
        batch.plan.clone()
    } else {
        let roots = match &basis.scope {
            WorkflowSubmissionScope::All => None,
            WorkflowSubmissionScope::FromNodes { node_ids } => Some(node_ids.as_slice()),
        };
        let source = if let Some(id) = basis.source_run_id {
            source_executions_in(conn, id).await?
        } else {
            Vec::new()
        };
        plan_rework(graph, roots, &source).map_err(|error| {
            if error.starts_with("Cannot reuse") {
                ApiError::Conflict(format!("REUSE_UNAVAILABLE: {error}"))
            } else {
                ApiError::BadRequest(format!("INVALID_REWORK_SCOPE: {error}"))
            }
        })?
    };
    if !nodes.reuse.is_empty() {
        let root = runner::workflow_control_root_in(conn, workspace_id).await?;
        for binding in &nodes.reuse {
            let id = Uuid::parse_str(&binding.source_execution_id).map_err(|_| {
                ApiError::Conflict("Reuse requires an actual source execution".into())
            })?;
            batch_rework::verify_handoff_in(conn, &root, id).await?;
        }
    }
    Ok(SubmissionPlan { nodes, batch })
}

async fn fail_planning_in(
    conn: &mut SqliteConnection,
    run_id: Uuid,
    error: &ApiError,
) -> Result<(), ApiError> {
    sqlx::query("UPDATE workflow_run_submissions SET planning_state='failed' WHERE run_id=? AND planning_state='pending'")
        .bind(run_id).execute(&mut *conn).await?;
    sqlx::query("UPDATE workflow_runs SET status='failed',error_text=?,finished_at=datetime('now','subsec'),updated_at=datetime('now','subsec') WHERE id=? AND status='pending'")
        .bind(format!("WORKFLOW_PLANNING_FAILED: {error}")).bind(run_id).execute(&mut *conn).await?;
    sqlx::query("UPDATE workflow_run_queue SET phase='finished' WHERE run_id=? AND phase IN ('queued','waiting_for_source','stopping_source')")
        .bind(run_id).execute(&mut *conn).await?;
    sync_terminal_attempt_in(conn, run_id).await
}

#[allow(clippy::too_many_arguments)]
async fn fix_run_plan_in(
    conn: &mut SqliteConnection,
    run_id: Uuid,
    graph: &WorkflowGraph,
    workspace_id: Uuid,
    instance_id: Option<Uuid>,
    issue_id: Uuid,
    basis: &RunPlanningBasis,
) -> Result<(), ApiError> {
    let plan = plan_submission_in(conn, graph, workspace_id, basis).await;
    match plan {
        Ok(plan) => {
            if let Some(batch) = &plan.batch {
                batch_rework::apply_in(conn, run_id, batch).await?;
            }
            apply_run_plan_in(
                conn,
                run_id,
                graph,
                workspace_id,
                instance_id,
                issue_id,
                plan.nodes,
            )
            .await
        }
        Err(error @ (ApiError::BadRequest(_) | ApiError::Conflict(_))) => {
            fail_planning_in(conn, run_id, &error).await
        }
        Err(error) => Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_run_plan_in(
    conn: &mut SqliteConnection,
    run_id: Uuid,
    graph: &WorkflowGraph,
    workspace_id: Uuid,
    instance_id: Option<Uuid>,
    issue_id: Uuid,
    plan: ReworkPlan,
) -> Result<(), ApiError> {
    for binding in &plan.reuse {
        let source_id = Uuid::parse_str(&binding.source_execution_id)
            .map_err(|_| ApiError::Conflict("Reuse lineage is not an actual execution".into()))?;
        sqlx::query("INSERT INTO workflow_result_reuse(run_id,node_id,iteration,source_node_execution_id) VALUES (?,?,?,?)")
            .bind(run_id).bind(&binding.node_id).bind(binding.iteration).bind(source_id).execute(&mut *conn).await?;
    }
    for id in &plan.skipped_node_ids {
        sqlx::query("INSERT INTO workflow_node_dispositions(run_id,node_id,disposition,reason) VALUES (?,?,'skipped','Untouched Condition preserves the settled source route')")
            .bind(run_id).bind(id).execute(&mut *conn).await?;
    }
    let parent = runner::workflow_task_parent(conn, instance_id, issue_id).await?;
    let batch_nodes = workflow::dispatch::batch_managed_nodes(graph);
    for node in graph
        .nodes
        .iter()
        .filter(|node| plan.affected_node_ids.contains(&node.id) && !batch_nodes.contains(&node.id))
    {
        runner::materialize_node_execution(conn, run_id, workspace_id, parent, node, 0).await?;
    }
    sqlx::query("UPDATE workflow_run_submissions SET planning_state='ready',affected_nodes_json=? WHERE run_id=? AND planning_state='pending'")
        .bind(serde_json::to_string(&plan.affected_node_ids).map_err(|_|ApiError::BadRequest("Invalid rework plan".into()))?).bind(run_id).execute(&mut *conn).await?;
    Ok(())
}

/// The queue is not yet claimed: cancellation and terminal notification are
/// atomic, and an asynchronous readiness worker cannot resurrect this entry.
async fn cancel_pending_in(conn: &mut SqliteConnection, run_id: Uuid) -> Result<bool, ApiError> {
    let changed=sqlx::query("UPDATE workflow_run_queue SET phase='finished' WHERE run_id=? AND phase IN ('queued','waiting_for_source','stopping_source') AND EXISTS(SELECT 1 FROM workflow_runs WHERE id=? AND status='pending')")
        .bind(run_id).bind(run_id).execute(&mut *conn).await?.rows_affected()==1;
    if changed {
        sqlx::query("UPDATE workflow_runs SET status='canceled',finished_at=datetime('now','subsec'),updated_at=datetime('now','subsec') WHERE id=? AND status='pending'")
            .bind(run_id).execute(&mut *conn).await?;
        sqlx::query("UPDATE node_executions SET status='cancelled',finished_at=datetime('now','subsec') WHERE run_id=? AND status='pending'")
            .bind(run_id).execute(&mut *conn).await?;
        sqlx::query("UPDATE workflow_split_subtasks SET state='cancelled',updated_at=datetime('now','subsec') WHERE batch_id IN (SELECT id FROM workflow_split_batches WHERE run_id=?) AND state IN ('pending','queued')")
            .bind(run_id).execute(&mut *conn).await?;
        sqlx::query("UPDATE workflow_split_batches SET state='cancelled',updated_at=datetime('now','subsec') WHERE run_id=? AND state IN ('pending','accepted','dispatching','running','partial')")
            .bind(run_id).execute(&mut *conn).await?;
        sync_terminal_attempt_in(conn, run_id).await?;
    }
    Ok(changed)
}

async fn sync_terminal_attempt_in(
    conn: &mut SqliteConnection,
    run_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("UPDATE workflow_attempts SET status=(SELECT status FROM workflow_runs WHERE id=?),updated_at=datetime('now','subsec') WHERE latest_run_id=? AND EXISTS(SELECT 1 FROM workflow_runs WHERE id=? AND status IN ('failed','canceled','succeeded'))")
        .bind(run_id).bind(run_id).bind(run_id).execute(conn).await?;
    Ok(())
}

/// Recover dependencies in the original dispatcher. Each plan is fixed once,
/// only after its source outcome and process/slot boundary are settled.
pub async fn resolve_waiting_submissions(pool: &SqlitePool) -> Result<(), ApiError> {
    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT q.run_id FROM workflow_run_queue q JOIN workflow_runs r ON r.id=q.run_id JOIN workflow_run_submissions s ON s.run_id=r.id WHERE q.phase IN ('waiting_for_source','stopping_source') AND r.status='pending' AND s.planning_state='pending' ORDER BY q.sequence")
        .fetch_all(pool).await?;
    for id in ids {
        let result:Result<(),ApiError>=async {
            let mut tx=pool.begin_with("BEGIN IMMEDIATE").await?;
            let row=sqlx::query("SELECT r.graph_snapshot,r.workspace_id,r.attempt_id,r.issue_id,s.scope_json,s.source_run_id,s.source_batch_id,s.source_node_execution_id,s.action,d.completeness FROM workflow_runs r JOIN workflow_run_submissions s ON s.run_id=r.id JOIN workflow_run_queue q ON q.run_id=r.id LEFT JOIN workflow_run_deliveries d ON d.run_id=r.id WHERE r.id=? AND r.status='pending' AND q.phase IN ('waiting_for_source','stopping_source') AND s.planning_state='pending'")
                .bind(id).fetch_optional(&mut *tx).await?;
            let Some(row)=row else{return Ok(());};
            let source_run_id:Uuid=row.try_get("source_run_id")?;
            if !source_boundary_settled_in(&mut tx,source_run_id).await? {return Ok(());}
            let parsed: Result<(WorkflowGraph, WorkflowSubmissionScope, WorkflowSubmissionAction),ApiError> = (|| {
                let graph=serde_json::from_str(&row.try_get::<String,_>("graph_snapshot")?).map_err(|_|ApiError::Conflict("Accepted frozen graph is invalid".into()))?;
                let scope=serde_json::from_str(&row.try_get::<String,_>("scope_json")?).map_err(|_|ApiError::Conflict("Accepted scope is invalid".into()))?;
                let action=match row.try_get::<String,_>("action")?.as_str() {
                    "start" => WorkflowSubmissionAction::Start,
                    "retry" => WorkflowSubmissionAction::Retry,
                    "rework" => WorkflowSubmissionAction::Rework,
                    _ => return Err(ApiError::Conflict("Accepted action is invalid".into())),
                };
                Ok((graph,scope,action))
            })();
            let (graph,scope,action)=match parsed {
                Ok(value)=>value,
                Err(error @ (ApiError::BadRequest(_) | ApiError::Conflict(_)))=>{
                    fail_planning_in(&mut tx,id,&error).await?;
                    tx.commit().await?;
                    return Ok(());
                }
                Err(error)=>return Err(error),
            };
            let basis=RunPlanningBasis {
                action, scope, source_run_id:Some(source_run_id), source_batch_id:row.try_get("source_batch_id")?,
                source_node_execution_id:row.try_get("source_node_execution_id")?,
                partial:row.try_get::<Option<String>,_>("completeness")?.as_deref()==Some("partial"),
            };
            fix_run_plan_in(&mut tx,id,&graph,row.try_get("workspace_id")?,row.try_get("attempt_id")?,row.try_get("issue_id")?,&basis).await?;
            sqlx::query("UPDATE workflow_run_queue SET phase='queued' WHERE run_id=? AND phase IN ('waiting_for_source','stopping_source') AND EXISTS(SELECT 1 FROM workflow_runs WHERE id=? AND status='pending') AND EXISTS(SELECT 1 FROM workflow_run_submissions WHERE run_id=? AND planning_state='ready')")
                .bind(id).bind(id).bind(id).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(())
        }.await;
        if let Err(error) = result {
            tracing::warn!(run_id=%id,%error,"Workflow dependency planning will retry; other projects continue");
        }
    }
    Ok(())
}

pub async fn deliver_stop_intents<C: WorkflowRunCanceller>(
    pool: &SqlitePool,
    canceller: &C,
) -> Result<(), ApiError> {
    let intents: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id,run_id FROM workflow_stop_intents WHERE state='pending' ORDER BY created_at,id",
    )
    .fetch_all(pool)
    .await?;
    for (intent_id, run_id) in intents {
        if super::queue::is_starting(run_id) {
            continue;
        }
        // The runtime's existing cancellation command keys are deterministic;
        // crash/replay may re-deliver but cannot issue another native stop.
        match runner::cancel_workflow_run_runtime(pool, run_id, canceller).await {
            Ok(run) => {
                workflows::sync_attempt_from_run(pool, &run).await?;
                db::models::workflow_queue::WorkflowQueueEntry::release_terminal(pool, run_id)
                    .await?;
                // A drain pass can finish before a failed command's retry
                // backoff expires. Keep the durable intent pending until all
                // owned processes, commands and the project slot have settled.
                let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
                if !source_boundary_settled_in(&mut tx, run_id).await? {
                    tx.commit().await?;
                    continue;
                }
                sqlx::query("UPDATE workflow_stop_intents SET state='delivered',delivered_at=datetime('now','subsec') WHERE id=? AND state='pending'")
                    .bind(intent_id).execute(&mut *tx).await?;
                tx.commit().await?;
            }
            Err(error) => {
                tracing::warn!(%run_id,%error,"Durable workflow stop delivery will retry")
            }
        }
    }
    Ok(())
}

pub async fn stop_workflow<C: WorkflowRunCanceller>(
    pool: &SqlitePool,
    caller: &WorkflowManagementCaller,
    run_id: Uuid,
    request_id: &str,
    canceller: &C,
) -> Result<WorkflowStopView, ApiError> {
    validate_request_id(request_id)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let scope = resolve_scope_in(&mut tx, caller).await?;
    if scope.handover_run_id.is_some_and(|id| id != run_id) {
        return Err(ApiError::Forbidden(
            "Automatic handover can stop only its original Run".into(),
        ));
    }
    let instance_id = scope.instance_id.ok_or_else(|| {
        ApiError::Conflict("This main Session has not accepted a workflow".into())
    })?;
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workflow_runs WHERE id=? AND attempt_id=?)",
    )
    .bind(run_id)
    .bind(instance_id)
    .fetch_one(&mut *tx)
    .await?;
    if !valid {
        return Err(ApiError::Forbidden(
            "Run is not part of the bound workflow instance".into(),
        ));
    }
    // Persist the scope of the user stop, including descendant successors, so
    // replay never cancels unrelated future submissions with another key.
    let replay:Option<String>=sqlx::query_scalar("SELECT result_json FROM workflow_operation_requests WHERE caller_namespace=? AND operation='stop' AND resource_id=? AND request_id=?")
        .bind(&scope.namespace).bind(run_id).bind(request_id).fetch_optional(&mut *tx).await?.flatten();
    let mut affected: Vec<Uuid> = if let Some(json) = replay {
        serde_json::from_str(&json)
            .map_err(|_| ApiError::Conflict("Stop request record is invalid".into()))?
    } else {
        Vec::new()
    };
    if affected.is_empty() {
        affected.push(run_id);
        let successors:Vec<Uuid>=sqlx::query_scalar("WITH RECURSIVE dependent(id) AS (SELECT ?1 UNION SELECT s.run_id FROM workflow_run_submissions s JOIN dependent d ON s.source_run_id=d.id WHERE s.instance_id=?2) SELECT d.id FROM dependent d JOIN workflow_runs r ON r.id=d.id JOIN workflow_run_queue q ON q.run_id=r.id WHERE d.id<>?1 AND r.status='pending' AND q.phase IN ('queued','waiting_for_source','stopping_source') ORDER BY q.sequence")
            .bind(run_id).bind(instance_id).fetch_all(&mut *tx).await?;
        for id in successors {
            if cancel_pending_in(&mut tx, id).await? {
                affected.push(id);
            }
        }
        cancel_pending_in(&mut tx, run_id).await?;
        sqlx::query("INSERT INTO workflow_stop_intents(id,caller_namespace,request_id,run_id) VALUES (?,?,?,?) ON CONFLICT(caller_namespace,run_id,request_id) DO NOTHING")
            .bind(Uuid::new_v4()).bind(&scope.namespace).bind(request_id).bind(run_id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO workflow_operation_requests(caller_namespace,operation,resource_id,request_id,request_hash,result_json) VALUES (?,'stop',?,?,?,?)")
            .bind(&scope.namespace).bind(run_id).bind(request_id).bind(request_hash(&json!({"run_id":run_id}))?).bind(serde_json::to_string(&affected).map_err(|_|ApiError::BadRequest("Invalid stop scope".into()))?).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    deliver_stop_intents(pool, canceller).await?;
    let run = runner::get_workflow_run_response(pool, run_id).await?;
    let mut conn = pool.acquire().await?;
    let settled = source_boundary_settled_in(&mut conn, run_id).await?;
    let instance_runs: Vec<Uuid> = sqlx::query_scalar(
        "SELECT r.id FROM workflow_runs r WHERE r.attempt_id=? ORDER BY r.created_at,r.id",
    )
    .bind(instance_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut unresolved = Vec::new();
    for id in instance_runs {
        if !source_boundary_settled_in(&mut conn, id).await? {
            unresolved.push(id);
        }
    }
    let unreachable:bool=sqlx::query_scalar("WITH owned_runs(id) AS (SELECT ?1 UNION SELECT o.id FROM orchestration_runs o JOIN node_executions n ON n.arena_group_id=o.source_definition_id WHERE n.run_id=?1 AND o.product_kind='arena') SELECT EXISTS(SELECT 1 FROM orchestration_agent_run_links l JOIN agent_run_attempts a ON a.agent_run_id=l.agent_run_id LEFT JOIN agent_process_registry p ON p.run_attempt_id=a.id WHERE l.orchestration_run_id IN(SELECT id FROM owned_runs) AND (p.run_attempt_id IS NULL OR p.registry_status='unreachable'))")
        .bind(run_id).fetch_one(&mut *conn).await?;
    Ok(WorkflowStopView {
        run_id,
        instance_id,
        status: run.status,
        stop_status: if settled {
            WorkflowStopStatus::Confirmed
        } else if unreachable {
            WorkflowStopStatus::Unreachable
        } else {
            WorkflowStopStatus::Requested
        },
        affected_run_ids: affected,
        unresolved_source_run_ids: unresolved,
    })
}

pub async fn list_callable_workflows(
    pool: &SqlitePool,
    caller: &WorkflowManagementCaller,
    cursor: i64,
    limit: u32,
) -> Result<WorkflowCallableTemplatePage, ApiError> {
    let mut conn = pool.acquire().await?;
    let scope = resolve_scope_in(&mut conn, caller).await?;
    drop(conn);
    let templates = workflows::list_project_workflows(pool, scope.project_id).await?;
    let offset = usize::try_from(cursor.max(0)).unwrap_or(usize::MAX);
    let limit = limit.clamp(1, 100) as usize;
    let total = templates.len();
    let workflows = templates.into_iter().skip(offset).take(limit).collect();
    Ok(WorkflowCallableTemplatePage {
        workflows,
        next_cursor: if offset.saturating_add(limit) < total {
            Some(i64::try_from(offset + limit).unwrap_or(i64::MAX))
        } else {
            None
        },
    })
}

pub async fn get_workflow_instance(
    pool: &SqlitePool,
    caller: &WorkflowManagementCaller,
    run_id: Option<Uuid>,
    cursor: Option<i64>,
    limit: u32,
) -> Result<Option<WorkflowInstanceView>, ApiError> {
    let mut conn = pool.acquire().await?;
    let scope = resolve_scope_in(&mut conn, caller).await?;
    drop(conn);
    let Some(instance_id) = scope.instance_id else {
        return Ok(None);
    };
    let instance = workflows::workflow_attempt_by_id(pool, instance_id)
        .await?
        .ok_or_else(|| ApiError::Conflict("Workflow instance was removed".into()))?;
    let graph_json:String=sqlx::query_scalar("SELECT COALESCE(a.frozen_graph_json,w.graph_json) FROM workflow_attempts a JOIN workflows w ON w.id=a.workflow_id WHERE a.id=?")
        .bind(instance_id).fetch_one(pool).await?;
    let limit = limit.clamp(1, 50) as usize;
    let rows=sqlx::query("SELECT id,rowid AS history_cursor FROM workflow_runs WHERE attempt_id=? AND (? IS NULL OR id=?) AND (? IS NULL OR rowid<?) ORDER BY rowid DESC LIMIT ?")
        .bind(instance_id).bind(run_id).bind(run_id).bind(cursor).bind(cursor).bind((limit+1) as i64).fetch_all(pool).await?;
    if run_id.is_some() && rows.is_empty() {
        return Err(ApiError::Forbidden(
            "Run is not part of this bound workflow instance".into(),
        ));
    }
    let more = rows.len() > limit;
    let mut runs = Vec::new();
    for row in rows.iter().take(limit) {
        runs.push(runner::get_workflow_run_response(pool, row.try_get("id")?).await?);
    }
    let selected_run_id = run_id.or(instance.latest_run_id);
    let mut reuse = Vec::new();
    let mut skipped_node_ids = Vec::new();
    let mut interactions = Vec::new();
    let mut notifications = Vec::new();
    if let Some(id) = selected_run_id {
        let rows=sqlx::query("SELECT reuse.node_id,reuse.iteration,reuse.source_node_execution_id,n.run_id AS source_run_id,n.output_text FROM workflow_result_reuse reuse JOIN node_executions n ON n.id=reuse.source_node_execution_id WHERE reuse.run_id=? ORDER BY reuse.node_id,reuse.iteration")
            .bind(id).fetch_all(pool).await?;
        reuse = rows
            .into_iter()
            .map(|r| {
                Ok(WorkflowReuseView {
                    node_id: r.try_get("node_id")?,
                    iteration: r.try_get("iteration")?,
                    source_node_execution_id: r.try_get("source_node_execution_id")?,
                    source_run_id: r.try_get("source_run_id")?,
                    output_text: r.try_get("output_text")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        skipped_node_ids = sqlx::query_scalar(
            "SELECT node_id FROM workflow_node_dispositions WHERE run_id=? ORDER BY node_id",
        )
        .bind(id)
        .fetch_all(pool)
        .await?;
        interactions = list_run_interactions(pool, id).await?;
        let query = format!(
            "{NOTIFICATION_SELECT} WHERE n.instance_id=? AND n.run_id=? ORDER BY n.sequence"
        );
        notifications = sqlx::query(&query)
            .bind(instance_id)
            .bind(id)
            .fetch_all(pool)
            .await?
            .iter()
            .map(notification_from_row)
            .collect::<Result<Vec<_>, _>>()?;
    }
    let next_cursor = if more {
        rows.get(limit - 1)
            .map(|r| r.try_get("history_cursor"))
            .transpose()?
    } else {
        None
    };
    Ok(Some(WorkflowInstanceView {
        instance,
        graph_json,
        runs,
        reuse,
        skipped_node_ids,
        interactions,
        notifications,
        next_cursor,
    }))
}

pub async fn list_run_interactions(
    pool: &SqlitePool,
    run_id: Uuid,
) -> Result<Vec<crate::routes::integrations::workflows::WorkflowInteraction>, ApiError> {
    let rows=sqlx::query("SELECT n.id,n.node_id,n.iteration,n.node_type,n.output_text FROM node_executions n JOIN workflow_runs r ON r.id=n.run_id WHERE n.run_id=? AND n.node_type IN ('human_gate','condition','arena') AND n.status IN ('awaiting_human','awaiting_arena') AND r.status IN ('running','awaiting_human','awaiting_arena') AND NOT EXISTS(SELECT 1 FROM workflow_interaction_responses a WHERE a.node_execution_id=n.id) ORDER BY n.created_at,n.id")
        .bind(run_id).fetch_all(pool).await?;
    let graph_json: String =
        sqlx::query_scalar("SELECT graph_snapshot FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    let graph: WorkflowGraph = serde_json::from_str(&graph_json)
        .map_err(|_| ApiError::Conflict("Frozen graph is invalid".into()))?;
    let mut result = Vec::new();
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let node_id: String = row.try_get("node_id")?;
        let arena_candidate_ids=sqlx::query_scalar("SELECT c.id FROM arena_candidates c JOIN node_executions n ON n.arena_group_id=c.arena_group_id WHERE n.id=? ORDER BY c.id")
            .bind(id).fetch_all(pool).await?;
        let branch_targets = graph
            .edges
            .iter()
            .filter(|edge| edge.source == node_id)
            .map(|edge| edge.target.clone())
            .collect();
        result.push(
            crate::routes::integrations::workflows::WorkflowInteraction {
                id,
                run_id,
                node_id,
                iteration: row.try_get("iteration")?,
                node_type: row.try_get("node_type")?,
                output_text: row.try_get("output_text")?,
                branch_targets,
                arena_candidate_ids,
            },
        );
    }
    Ok(result)
}

pub async fn respond_to_workflow<A, R, W>(
    pool: &SqlitePool,
    caller: &WorkflowManagementCaller,
    request: WorkflowManagementInteractionRequest,
    executor: &A,
    arena: &R,
    winner: &W,
) -> Result<WorkflowRunResponse, ApiError>
where
    A: runner::WorkflowAgentExecutor,
    R: super::arena::WorkflowArenaCreator,
    W: super::arena::WorkflowArenaWinnerApplier,
{
    use crate::routes::integrations::workflows::WorkflowInteractionResponse;
    validate_request_id(&request.request_id)?;
    let hash = request_hash(&request)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let scope = resolve_scope_in(&mut tx, caller).await?;
    if scope.handover_run_id.is_some_and(|id| id != request.run_id) {
        return Err(ApiError::Forbidden(
            "Automatic handover can respond only to its original Run".into(),
        ));
    }
    let row=sqlx::query("SELECT n.node_id,n.node_type,n.status,n.arena_group_id,r.status AS run_status,r.graph_snapshot FROM node_executions n JOIN workflow_runs r ON r.id=n.run_id WHERE n.id=? AND n.run_id=? AND r.attempt_id=?")
        .bind(request.node_execution_id).bind(request.run_id).bind(scope.instance_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::Forbidden("Interaction is not part of this bound workflow instance".into()))?;
    let node_id: String = row.try_get("node_id")?;
    let previous=sqlx::query("SELECT request_hash,result_json FROM workflow_operation_requests WHERE caller_namespace=? AND operation='respond' AND resource_id=? AND request_id=?")
        .bind(&scope.namespace).bind(request.node_execution_id).bind(&request.request_id).fetch_optional(&mut *tx).await?;
    if let Some(previous) = &previous {
        if previous.try_get::<String, _>("request_hash")? != hash {
            return Err(ApiError::Conflict(
                "IDEMPOTENCY_CONFLICT: response request_id has different parameters".into(),
            ));
        }
        if previous
            .try_get::<Option<String>, _>("result_json")?
            .is_some()
        {
            tx.commit().await?;
            return runner::get_workflow_run_response(pool, request.run_id).await;
        }
    } else {
        let node_type: String = row.try_get("node_type")?;
        let node_status: String = row.try_get("status")?;
        if !matches!(
            row.try_get::<String, _>("run_status")?.as_str(),
            "running" | "awaiting_human" | "awaiting_arena"
        ) {
            return Err(ApiError::Conflict(
                "Workflow Run no longer accepts this interaction".into(),
            ));
        }
        let claimed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_interaction_responses WHERE node_execution_id=?) OR EXISTS(SELECT 1 FROM workflow_operation_requests WHERE operation='respond' AND resource_id=?)")
            .bind(request.node_execution_id).bind(request.node_execution_id).fetch_one(&mut *tx).await?;
        if claimed || !matches!(node_status.as_str(), "awaiting_human" | "awaiting_arena") {
            return Err(ApiError::Conflict(
                "Workflow interaction is stale or already answered".into(),
            ));
        }
        match &request.response {
            WorkflowInteractionResponse::Approve | WorkflowInteractionResponse::Reject
                if node_type == "human_gate" && node_status == "awaiting_human" => {}
            WorkflowInteractionResponse::SelectBranch {
                selected_target_node_ids,
                reason,
            } if node_type == "condition" && node_status == "awaiting_human" => {
                let graph: WorkflowGraph =
                    serde_json::from_str(&row.try_get::<String, _>("graph_snapshot")?)
                        .map_err(|_| ApiError::Conflict("Frozen workflow is invalid".into()))?;
                let node = graph
                    .nodes
                    .iter()
                    .find(|node| node.id == node_id)
                    .ok_or_else(|| {
                        ApiError::Conflict("Interaction Node is not in the frozen graph".into())
                    })?;
                super::condition_router::build_manual_route(
                    &graph,
                    node,
                    selected_target_node_ids,
                    reason.as_deref(),
                    false,
                )
                .map_err(|error| ApiError::BadRequest(error.to_string()))?;
            }
            WorkflowInteractionResponse::SelectArenaWinner { candidate_id }
                if node_type == "arena" && node_status == "awaiting_arena" =>
            {
                let valid: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM arena_candidates WHERE id=? AND arena_group_id=?)",
                )
                .bind(candidate_id)
                .bind(row.try_get::<Option<Uuid>, _>("arena_group_id")?)
                .fetch_one(&mut *tx)
                .await?;
                if !valid {
                    return Err(ApiError::BadRequest(
                        "Winner is not a candidate in this exact Arena execution".into(),
                    ));
                }
            }
            _ => {
                return Err(ApiError::BadRequest(
                    "Action is not allowed for this exact interaction".into(),
                ));
            }
        }
        sqlx::query("INSERT INTO workflow_operation_requests(caller_namespace,operation,resource_id,request_id,request_hash) VALUES (?,'respond',?,?,?)")
            .bind(&scope.namespace).bind(request.node_execution_id).bind(&request.request_id).bind(hash).execute(&mut *tx).await?;
    }
    let already_applied: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workflow_interaction_responses WHERE node_execution_id=?)",
    )
    .bind(request.node_execution_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    let run = if already_applied {
        // A crash after the exact CAS must not repeat an Arena application or
        // dispatch. Recovery reconciles the durable original decision.
        runner::get_workflow_run_response(pool, request.run_id).await?
    } else {
        match request.response {
            WorkflowInteractionResponse::Approve => {
                runner::approve_human_node_at(
                    pool,
                    request.run_id,
                    &node_id,
                    Some(request.node_execution_id),
                    executor,
                    arena,
                )
                .await?
            }
            WorkflowInteractionResponse::Reject => {
                runner::reject_human_node_at(
                    pool,
                    request.run_id,
                    &node_id,
                    Some(request.node_execution_id),
                )
                .await?
            }
            WorkflowInteractionResponse::SelectBranch {
                selected_target_node_ids,
                reason,
            } => {
                runner::select_condition_branch_at(
                    pool,
                    request.run_id,
                    &node_id,
                    Some(request.node_execution_id),
                    selected_target_node_ids,
                    reason,
                    executor,
                    arena,
                )
                .await?
            }
            WorkflowInteractionResponse::SelectArenaWinner { candidate_id } => {
                runner::select_arena_winner_at(
                    pool,
                    request.run_id,
                    &node_id,
                    Some(request.node_execution_id),
                    candidate_id,
                    executor,
                    arena,
                    winner,
                )
                .await?
            }
        }
    };
    sqlx::query("UPDATE workflow_operation_requests SET result_json=? WHERE caller_namespace=? AND operation='respond' AND resource_id=? AND request_id=?")
        .bind(json!({"run_id":request.run_id}).to_string()).bind(&scope.namespace).bind(request.node_execution_id).bind(&request.request_id).execute(pool).await?;
    workflows::sync_attempt_from_run(pool, &run).await?;
    Ok(run)
}
