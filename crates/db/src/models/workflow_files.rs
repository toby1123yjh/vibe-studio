//! Read-only projections of accepted workflow files and internal split work.
//! These records never become project business Tasks or editable task bodies.
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowFileReferenceView {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowHandoffView {
    pub node_execution_id: Uuid,
    pub run_id: Uuid,
    pub handoff_path: String,
    pub files: Vec<WorkflowFileReferenceView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowHandoverView {
    pub state: String,
    pub session_id: Uuid,
    pub agent_run_id: Option<Uuid>,
    pub run_attempt_id: Option<Uuid>,
    pub error_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowSplitSubtaskView {
    pub id: Uuid,
    pub subtask_id: String,
    pub title: String,
    pub instruction_path: String,
    pub node_id: Option<String>,
    #[ts(type = "number")]
    pub stage_index: i64,
    pub state: String,
    pub depends_on: Vec<String>,
    pub node_execution_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
    pub error_text: Option<String>,
    pub handoff_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowSplitBatchView {
    pub id: Uuid,
    pub run_id: Uuid,
    pub node_execution_id: Uuid,
    pub splitter_node_id: String,
    pub summary_node_id: String,
    pub manifest_path: String,
    pub manifest_sha256: String,
    pub dispatch_path: Option<String>,
    pub state: String,
    pub source_batch_id: Option<Uuid>,
    pub summary_node_execution_id: Option<Uuid>,
    pub error_text: Option<String>,
    pub handover: Option<WorkflowHandoverView>,
    pub subtasks: Vec<WorkflowSplitSubtaskView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowMissingSubtaskView {
    pub subtask_id: String,
    pub title: String,
    pub state: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum WorkflowDeliveryCompleteness {
    #[default]
    Full,
    Partial,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowDeliveryView {
    pub completeness: WorkflowDeliveryCompleteness,
    pub source_run_id: Option<Uuid>,
    pub source_batch_id: Option<Uuid>,
    pub missing_subtasks: Vec<WorkflowMissingSubtaskView>,
}

#[derive(Debug, Default)]
pub struct WorkflowFileRunView {
    pub split_batches: Vec<WorkflowSplitBatchView>,
    pub handoffs: Vec<WorkflowHandoffView>,
    pub delivery: Option<WorkflowDeliveryView>,
}

fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, sqlx::Error> {
    serde_json::from_str(value).map_err(|error| sqlx::Error::Decode(Box::new(error)))
}

impl WorkflowFileRunView {
    pub async fn load(pool: &SqlitePool, run_id: Uuid) -> Result<Self, sqlx::Error> {
        let mut subtasks: HashMap<Uuid, Vec<WorkflowSplitSubtaskView>> = HashMap::new();
        let rows = sqlx::query(
            "SELECT s.*,n.agent_run_id,COALESCE(s.session_id,n.session_id) AS resolved_session_id,COALESCE(n.error_text,s.error_text) AS resolved_error,h.handoff_path FROM workflow_split_subtasks s JOIN workflow_split_batches b ON b.id=s.batch_id LEFT JOIN node_executions n ON n.id=s.node_execution_id LEFT JOIN workflow_node_handoffs h ON h.node_execution_id=s.node_execution_id WHERE b.run_id=? ORDER BY s.stage_index,s.created_at,s.subtask_id",
        ).bind(run_id).fetch_all(pool).await?;
        for row in rows {
            subtasks
                .entry(row.try_get("batch_id")?)
                .or_default()
                .push(WorkflowSplitSubtaskView {
                    id: row.try_get("id")?,
                    subtask_id: row.try_get("subtask_id")?,
                    title: row.try_get("title")?,
                    instruction_path: row.try_get("instruction_path")?,
                    node_id: row.try_get("node_id")?,
                    stage_index: row.try_get("stage_index")?,
                    state: row.try_get("state")?,
                    depends_on: decode(row.try_get("depends_on_json")?)?,
                    node_execution_id: row.try_get("node_execution_id")?,
                    session_id: row.try_get("resolved_session_id")?,
                    agent_run_id: row.try_get("agent_run_id")?,
                    error_text: row.try_get("resolved_error")?,
                    handoff_path: row.try_get("handoff_path")?,
                });
        }
        let batches = sqlx::query(
            "SELECT b.*,n.node_id,h.state AS handover_state,h.session_id AS handover_session_id,h.agent_run_id AS handover_agent_run_id,h.run_attempt_id AS handover_run_attempt_id,h.error_text AS handover_error,r.graph_snapshot FROM workflow_split_batches b JOIN node_executions n ON n.id=b.node_execution_id JOIN workflow_runs r ON r.id=b.run_id LEFT JOIN workflow_handover_events h ON h.batch_id=b.id WHERE b.run_id=? ORDER BY b.created_at,b.id",
        ).bind(run_id).fetch_all(pool).await?;
        let mut view = Self::default();
        for row in batches {
            let id: Uuid = row.try_get("id")?;
            let splitter_node_id: String = row.try_get("node_id")?;
            let graph: serde_json::Value = decode(row.try_get("graph_snapshot")?)?;
            let summary_node_id = summary_node_id(&graph, &splitter_node_id).unwrap_or_default();
            let handover = row
                .try_get::<Option<String>, _>("handover_state")?
                .map(|state| {
                    Ok::<_, sqlx::Error>(WorkflowHandoverView {
                        state,
                        session_id: row.try_get("handover_session_id")?,
                        agent_run_id: row.try_get("handover_agent_run_id")?,
                        run_attempt_id: row.try_get("handover_run_attempt_id")?,
                        error_text: row.try_get("handover_error")?,
                    })
                })
                .transpose()?;
            view.split_batches.push(WorkflowSplitBatchView {
                id,
                run_id,
                node_execution_id: row.try_get("node_execution_id")?,
                splitter_node_id,
                summary_node_id,
                manifest_path: row.try_get("manifest_path")?,
                manifest_sha256: row.try_get("manifest_sha256")?,
                dispatch_path: row.try_get("dispatch_path")?,
                state: row.try_get("state")?,
                source_batch_id: row.try_get("source_batch_id")?,
                summary_node_execution_id: row.try_get("summary_node_execution_id")?,
                error_text: row.try_get("error_text")?,
                handover,
                subtasks: subtasks.remove(&id).unwrap_or_default(),
            });
        }
        let handoffs = sqlx::query("SELECT * FROM workflow_node_handoffs WHERE run_id=? ORDER BY created_at,node_execution_id")
            .bind(run_id).fetch_all(pool).await?;
        for row in handoffs {
            view.handoffs.push(WorkflowHandoffView {
                node_execution_id: row.try_get("node_execution_id")?,
                run_id,
                handoff_path: row.try_get("handoff_path")?,
                files: decode(row.try_get("files_json")?)?,
            });
        }
        if let Some(row) = sqlx::query("SELECT * FROM workflow_run_deliveries WHERE run_id=?")
            .bind(run_id)
            .fetch_optional(pool)
            .await?
        {
            let completeness = match row.try_get::<&str, _>("completeness")? {
                "partial" => WorkflowDeliveryCompleteness::Partial,
                _ => WorkflowDeliveryCompleteness::Full,
            };
            view.delivery = Some(WorkflowDeliveryView {
                completeness,
                source_run_id: row.try_get("source_run_id")?,
                source_batch_id: row.try_get("source_batch_id")?,
                missing_subtasks: decode(row.try_get("missing_subtasks_json")?)?,
            });
        }
        Ok(view)
    }
}

// The frozen graph has already been validated by workflow::dispatch_scope.
// Keep the DB projection independent of the runtime crate's graph machinery.
fn summary_node_id(graph: &serde_json::Value, splitter_id: &str) -> Option<String> {
    let edges = graph.get("edges")?.as_array()?;
    let executor = edges
        .iter()
        .find(|edge| edge.get("source").and_then(serde_json::Value::as_str) == Some(splitter_id))?
        .get("target")?
        .as_str()?;
    edges
        .iter()
        .find(|edge| edge.get("source").and_then(serde_json::Value::as_str) == Some(executor))?
        .get("target")?
        .as_str()
        .map(str::to_owned)
}
