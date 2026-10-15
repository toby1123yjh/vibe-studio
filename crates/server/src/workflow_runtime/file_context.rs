//! Read accepted file references without revalidating mutable workspace bytes or
//! running lifecycle reconciliation. Execution/batch membership owns attribution.
use std::collections::{HashMap, HashSet};

use db::models::workflow_files::{WorkflowFileReferenceView, WorkflowMissingSubtaskView};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, SqlitePool};
use ts_rs::TS;
use uuid::Uuid;
use workflow::WorkflowGraph;

use crate::error::ApiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum WorkflowContextFileRole {
    Output,
    Input,
    Control,
    Handoff,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowContextFileView {
    pub role: WorkflowContextFileRole,
    pub path: String,
    pub sha256: Option<String>,
    pub source_run_id: Uuid,
    pub source_node_execution_id: Option<Uuid>,
    pub source_node_id: Option<String>,
    pub source_session_id: Option<Uuid>,
    pub source_workspace_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowContextExecutionView {
    pub node_execution_id: Uuid,
    pub node_id: String,
    pub run_id: Uuid,
    pub workspace_id: Uuid,
    pub session_id: Option<Uuid>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkflowFileContextView {
    pub run_id: Uuid,
    pub workspace_id: Uuid,
    pub node_execution_id: Option<Uuid>,
    pub execution: Option<WorkflowContextExecutionView>,
    pub files: Vec<WorkflowContextFileView>,
    pub missing_subtasks: Vec<WorkflowMissingSubtaskView>,
}

struct RunFiles {
    workspace_id: Uuid,
    graph: WorkflowGraph,
    effective: Vec<Uuid>,
    materials: Vec<String>,
    retry_source: Option<Uuid>,
    missing: Vec<WorkflowMissingSubtaskView>,
}

struct ExecutionFiles {
    id: Uuid,
    run_id: Uuid,
    node_id: String,
    node_type: String,
    session_id: Option<Uuid>,
    status: String,
    started_at: Option<f64>,
    finished_at: Option<f64>,
    handoff: Option<(String, String, Vec<WorkflowFileReferenceView>)>,
}

struct BatchFiles {
    id: Uuid,
    run_id: Uuid,
    split_execution_id: Uuid,
    summary_execution_id: Option<Uuid>,
    source_batch_id: Option<Uuid>,
    manifest: WorkflowFileReferenceView,
    dispatch: Option<(String, Option<String>)>,
    items: Vec<SubtaskFiles>,
}

struct SubtaskFiles {
    id: String,
    title: String,
    instruction: WorkflowFileReferenceView,
    dependencies: Vec<String>,
    execution_id: Option<Uuid>,
    state: String,
    error: Option<String>,
}

#[derive(Default)]
struct FileSnapshot {
    runs: HashMap<Uuid, RunFiles>,
    executions: HashMap<Uuid, ExecutionFiles>,
    batches: Vec<BatchFiles>,
}

fn decode<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, ApiError> {
    serde_json::from_str(json)
        .map_err(|_| ApiError::Conflict("Accepted workflow file metadata is invalid".into()))
}

impl FileSnapshot {
    async fn load(conn: &mut SqliteConnection, run_id: Uuid) -> Result<Self, ApiError> {
        let mut snapshot = Self::default();
        let mut pending = vec![run_id];
        while let Some(id) = pending.pop() {
            if snapshot.runs.contains_key(&id) {
                continue;
            }
            let row = sqlx::query("SELECT r.workspace_id,r.graph_snapshot,s.action,s.material_paths_json,s.source_run_id,d.missing_subtasks_json FROM workflow_runs r LEFT JOIN workflow_run_submissions s ON s.run_id=r.id LEFT JOIN workflow_run_deliveries d ON d.run_id=r.id WHERE r.id=?")
                .bind(id).fetch_optional(&mut *conn).await?
                .ok_or_else(|| ApiError::BadRequest("Workflow Run not found".into()))?;
            let workspace_id = row
                .try_get::<Option<Uuid>, _>("workspace_id")?
                .ok_or_else(|| ApiError::Conflict("Workflow workspace is unavailable".into()))?;
            let graph = row
                .try_get::<Option<String>, _>("graph_snapshot")?
                .ok_or_else(|| ApiError::Conflict("Workflow Run has no frozen graph".into()))?;
            let action: Option<String> = row.try_get("action")?;
            let retry_source = if action.as_deref() == Some("retry") {
                row.try_get::<Option<Uuid>, _>("source_run_id")?
            } else {
                None
            };
            pending.extend(retry_source);
            let material_json: Option<String> = row.try_get("material_paths_json")?;
            let missing_json: Option<String> = row.try_get("missing_subtasks_json")?;
            let effective = sqlx::query("SELECT e.source_node_execution_id,n.run_id FROM workflow_effective_node_executions e JOIN node_executions n ON n.id=e.source_node_execution_id WHERE e.run_id=? ORDER BY e.node_id,e.iteration,e.source_node_execution_id")
                .bind(id).fetch_all(&mut *conn).await?;
            let mut execution_ids = Vec::new();
            for source in effective {
                execution_ids.push(source.try_get("source_node_execution_id")?);
                pending.push(source.try_get("run_id")?);
            }
            snapshot.runs.insert(
                id,
                RunFiles {
                    workspace_id,
                    graph: decode(&graph)?,
                    effective: execution_ids,
                    materials: material_json
                        .as_deref()
                        .map(decode)
                        .transpose()?
                        .unwrap_or_default(),
                    retry_source,
                    missing: missing_json
                        .as_deref()
                        .map(decode)
                        .transpose()?
                        .unwrap_or_default(),
                },
            );
            let executions = sqlx::query("SELECT n.id,n.run_id,n.node_id,n.node_type,n.session_id,n.status,julianday(n.started_at) AS started_at,julianday(n.finished_at) AS finished_at,h.handoff_path,h.handoff_sha256,h.files_json FROM node_executions n LEFT JOIN workflow_node_handoffs h ON h.node_execution_id=n.id AND h.run_id=n.run_id WHERE n.run_id=? ORDER BY n.node_id,n.iteration,n.id")
                .bind(id).fetch_all(&mut *conn).await?;
            for execution in executions {
                let execution_id = execution.try_get("id")?;
                let handoff_path: Option<String> = execution.try_get("handoff_path")?;
                let handoff = handoff_path
                    .map(|path| {
                        Ok::<_, ApiError>((
                            path,
                            execution.try_get("handoff_sha256")?,
                            decode(execution.try_get("files_json")?)?,
                        ))
                    })
                    .transpose()?;
                snapshot.executions.insert(
                    execution_id,
                    ExecutionFiles {
                        id: execution_id,
                        run_id: id,
                        node_id: execution.try_get("node_id")?,
                        node_type: execution.try_get("node_type")?,
                        session_id: execution.try_get("session_id")?,
                        status: execution.try_get("status")?,
                        started_at: execution.try_get("started_at")?,
                        finished_at: execution.try_get("finished_at")?,
                        handoff,
                    },
                );
            }
            let batches = sqlx::query("SELECT b.*,n.run_id AS split_run_id,source.run_id AS source_batch_run_id FROM workflow_split_batches b JOIN node_executions n ON n.id=b.node_execution_id LEFT JOIN workflow_split_batches source ON source.id=b.source_batch_id WHERE b.run_id=? ORDER BY b.created_at,b.id")
                .bind(id).fetch_all(&mut *conn).await?;
            for batch in batches {
                pending.push(batch.try_get("split_run_id")?);
                pending.extend(batch.try_get::<Option<Uuid>, _>("source_batch_run_id")?);
                let batch_id: Uuid = batch.try_get("id")?;
                let items = sqlx::query("SELECT s.*,n.run_id AS execution_run_id FROM workflow_split_subtasks s LEFT JOIN node_executions n ON n.id=s.node_execution_id WHERE s.batch_id=? ORDER BY s.stage_index,s.subtask_id")
                    .bind(batch_id).fetch_all(&mut *conn).await?;
                let mut members = Vec::new();
                for item in items {
                    pending.extend(item.try_get::<Option<Uuid>, _>("execution_run_id")?);
                    members.push(SubtaskFiles {
                        id: item.try_get("subtask_id")?,
                        title: item.try_get("title")?,
                        instruction: WorkflowFileReferenceView {
                            path: item.try_get("instruction_path")?,
                            sha256: item.try_get("instruction_sha256")?,
                        },
                        dependencies: decode(item.try_get("depends_on_json")?)?,
                        execution_id: item.try_get("node_execution_id")?,
                        state: item.try_get("state")?,
                        error: item.try_get("error_text")?,
                    });
                }
                let dispatch_path: Option<String> = batch.try_get("dispatch_path")?;
                snapshot.batches.push(BatchFiles {
                    id: batch_id,
                    run_id: id,
                    split_execution_id: batch.try_get("node_execution_id")?,
                    summary_execution_id: batch.try_get("summary_node_execution_id")?,
                    source_batch_id: batch.try_get("source_batch_id")?,
                    manifest: WorkflowFileReferenceView {
                        path: batch.try_get("manifest_path")?,
                        sha256: batch.try_get("manifest_sha256")?,
                    },
                    dispatch: dispatch_path
                        .map(|path| Ok::<_, ApiError>((path, batch.try_get("dispatch_sha256")?)))
                        .transpose()?,
                    items: members,
                });
            }
        }
        Ok(snapshot)
    }

    fn file(
        &self,
        run_id: Uuid,
        execution: Option<&ExecutionFiles>,
        role: WorkflowContextFileRole,
        path: &str,
        sha256: Option<&str>,
    ) -> WorkflowContextFileView {
        WorkflowContextFileView {
            role,
            path: path.replace('\\', "/"),
            sha256: sha256.map(str::to_owned),
            source_run_id: run_id,
            source_workspace_id: self.runs[&run_id].workspace_id,
            source_node_execution_id: execution.map(|execution| execution.id),
            source_node_id: execution.map(|execution| execution.node_id.clone()),
            source_session_id: execution.and_then(|execution| execution.session_id),
        }
    }

    fn request_files(&self, run_id: Uuid) -> Vec<WorkflowContextFileView> {
        let mut files = vec![self.file(
            run_id,
            None,
            WorkflowContextFileRole::Input,
            &format!(".vibe-kanban/workflows/{run_id}/request.md"),
            None,
        )];
        let source = self.material_source(run_id);
        files.extend(
            self.runs[&source]
                .materials
                .iter()
                .map(|path| self.file(source, None, WorkflowContextFileRole::Input, path, None)),
        );
        if !self.runs[&run_id].missing.is_empty() {
            files.push(self.file(
                run_id,
                None,
                WorkflowContextFileRole::Control,
                &format!(".vibe-kanban/workflows/{run_id}/partial-delivery.json"),
                None,
            ));
        }
        files
    }

    fn material_source(&self, run_id: Uuid) -> Uuid {
        let mut source = run_id;
        let mut seen = HashSet::new();
        while seen.insert(source) {
            let Some(previous) = self.runs[&source].retry_source else {
                break;
            };
            source = previous;
        }
        source
    }

    fn batch_files(
        &self,
        batch: &BatchFiles,
        selected_item: Option<&SubtaskFiles>,
    ) -> Vec<WorkflowContextFileView> {
        let splitter = &self.executions[&batch.split_execution_id];
        let mut files = vec![self.file(
            splitter.run_id,
            Some(splitter),
            WorkflowContextFileRole::Control,
            &batch.manifest.path,
            Some(&batch.manifest.sha256),
        )];
        let items: Vec<_> = selected_item
            .map(|item| vec![item])
            .unwrap_or_else(|| batch.items.iter().collect());
        files.extend(items.into_iter().map(|item| {
            self.file(
                splitter.run_id,
                Some(splitter),
                if selected_item.is_some() {
                    WorkflowContextFileRole::Input
                } else {
                    WorkflowContextFileRole::Control
                },
                &item.instruction.path,
                Some(&item.instruction.sha256),
            )
        }));
        if let Some((path, hash)) = &batch.dispatch {
            // Cloned plans keep the original accepted dispatch, not a new
            // dispatch attributed to the current Run or its main Agent.
            let mut origin = batch;
            let mut seen = HashSet::new();
            while seen.insert(origin.id) {
                let Some(previous) = origin
                    .source_batch_id
                    .and_then(|id| self.batches.iter().find(|batch| batch.id == id))
                else {
                    break;
                };
                if previous.dispatch != batch.dispatch {
                    break;
                }
                origin = previous;
            }
            files.push(self.file(
                origin.run_id,
                None,
                WorkflowContextFileRole::Control,
                path,
                hash.as_deref(),
            ));
        }
        files
    }

    fn execution_batch(
        &self,
        execution: &ExecutionFiles,
    ) -> Option<(&BatchFiles, Option<&SubtaskFiles>)> {
        self.batches
            .iter()
            .filter(|batch| batch.run_id == execution.run_id)
            .find_map(|batch| {
                if batch.split_execution_id == execution.id
                    || batch.summary_execution_id == Some(execution.id)
                {
                    Some((batch, None))
                } else {
                    batch
                        .items
                        .iter()
                        .find(|item| item.execution_id == Some(execution.id))
                        .map(|item| (batch, Some(item)))
                }
            })
    }

    fn incoming(&self, execution: &ExecutionFiles) -> Vec<Uuid> {
        if let Some((batch, item)) = self.execution_batch(execution) {
            if let Some(item) = item {
                return batch
                    .items
                    .iter()
                    .filter(|source| {
                        item.dependencies.contains(&source.id) && source.state == "succeeded"
                    })
                    .filter_map(|source| source.execution_id)
                    .collect();
            }
            if batch.summary_execution_id == Some(execution.id) {
                return batch
                    .items
                    .iter()
                    .filter(|source| source.state == "succeeded")
                    .filter_map(|source| source.execution_id)
                    .collect();
            }
        }
        let run = &self.runs[&execution.run_id];
        let source_nodes: HashSet<_> = run
            .graph
            .edges
            .iter()
            .filter(|edge| edge.target == execution.node_id)
            .map(|edge| edge.source.as_str())
            .collect();
        run.effective.iter().copied().filter(|id| {
            let source = &self.executions[id];
            source_nodes.contains(source.node_id.as_str()) && source.status == "succeeded"
                && !matches!((source.finished_at, execution.started_at), (Some(finished), Some(started)) if finished > started)
        }).collect()
    }

    fn protocol_file(
        &self,
        execution: &ExecutionFiles,
        reference: &WorkflowFileReferenceView,
    ) -> Option<WorkflowContextFileView> {
        let material_source = self.material_source(execution.run_id);
        if self.runs[&material_source]
            .materials
            .iter()
            .any(|path| path.replace('\\', "/") == reference.path.replace('\\', "/"))
        {
            return Some(self.file(
                material_source,
                None,
                WorkflowContextFileRole::Input,
                &reference.path,
                Some(&reference.sha256),
            ));
        }
        for id in self.runs.keys() {
            if reference.path.replace('\\', "/")
                == format!(".vibe-kanban/workflows/{id}/request.md")
            {
                return Some(self.file(
                    *id,
                    None,
                    WorkflowContextFileRole::Input,
                    &reference.path,
                    Some(&reference.sha256),
                ));
            }
            if reference.path.replace('\\', "/")
                == format!(".vibe-kanban/workflows/{id}/partial-delivery.json")
            {
                return Some(self.file(
                    *id,
                    None,
                    WorkflowContextFileRole::Control,
                    &reference.path,
                    Some(&reference.sha256),
                ));
            }
        }
        for batch in &self.batches {
            if batch.manifest.path == reference.path
                || batch
                    .items
                    .iter()
                    .any(|item| item.instruction.path == reference.path)
            {
                let source = &self.executions[&batch.split_execution_id];
                return Some(self.file(
                    source.run_id,
                    Some(source),
                    WorkflowContextFileRole::Control,
                    &reference.path,
                    Some(&reference.sha256),
                ));
            }
            if batch
                .dispatch
                .as_ref()
                .is_some_and(|(path, _)| path == &reference.path)
            {
                return self
                    .batch_files(batch, None)
                    .into_iter()
                    .find(|file| file.path == reference.path);
            }
        }
        // The splitter publishes only protocol/instruction files. An accepted
        // handoff can briefly precede publication of the batch metadata.
        (execution.node_type == "task_splitter").then(|| {
            self.file(
                execution.run_id,
                Some(execution),
                WorkflowContextFileRole::Control,
                &reference.path,
                Some(&reference.sha256),
            )
        })
    }

    fn accepted_files(
        &self,
        execution: &ExecutionFiles,
        outputs: bool,
    ) -> Vec<WorkflowContextFileView> {
        let Some((path, hash, references)) = &execution.handoff else {
            return Vec::new();
        };
        let mut files = vec![self.file(
            execution.run_id,
            Some(execution),
            WorkflowContextFileRole::Handoff,
            path,
            Some(hash),
        )];
        if execution.status != "succeeded" {
            return files;
        }
        let produces = matches!(
            execution.node_type.as_str(),
            "agent" | "arena" | "transform"
        );
        for reference in references {
            if let Some(mut file) = self.protocol_file(execution, reference) {
                if !outputs && file.role != WorkflowContextFileRole::Control {
                    file.role = WorkflowContextFileRole::Input;
                }
                files.push(file);
            } else if produces {
                files.push(self.file(
                    execution.run_id,
                    Some(execution),
                    if outputs {
                        WorkflowContextFileRole::Output
                    } else {
                        WorkflowContextFileRole::Input
                    },
                    &reference.path,
                    Some(&reference.sha256),
                ));
            }
            // Structural Nodes only forward references. Their upstream
            // producer is exposed separately; never count a new artifact here.
        }
        files
    }

    fn incoming_files(&self, execution: &ExecutionFiles) -> Vec<WorkflowContextFileView> {
        let mut files = Vec::new();
        let mut pending = self.incoming(execution);
        let mut seen = HashSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let source = &self.executions[&id];
            files.extend(self.accepted_files(source, false));
            if !matches!(
                source.node_type.as_str(),
                "agent" | "arena" | "transform" | "task_splitter"
            ) {
                pending.extend(self.incoming(source));
            }
        }
        files
    }

    fn project(
        &self,
        run_id: Uuid,
        selected: Option<Uuid>,
    ) -> Result<WorkflowFileContextView, ApiError> {
        let run = &self.runs[&run_id];
        let mut files;
        let mut missing;
        let resolved_execution;
        if let Some(id) = selected {
            let explicitly_bound = run.effective.contains(&id)
                || self.batches.iter().any(|batch| {
                    batch.run_id == run_id
                        && (batch.split_execution_id == id
                            || batch.items.iter().any(|item| item.execution_id == Some(id)))
                });
            if !explicitly_bound {
                return Err(ApiError::BadRequest("Node execution does not belong to this Workflow Run or its accepted reused results".into()));
            }
            let execution = self.executions.get(&id).ok_or_else(|| {
                ApiError::Conflict("Bound workflow execution is unavailable".into())
            })?;
            resolved_execution = Some(WorkflowContextExecutionView {
                node_execution_id: execution.id,
                node_id: execution.node_id.clone(),
                run_id: execution.run_id,
                workspace_id: self.runs[&execution.run_id].workspace_id,
                session_id: execution.session_id,
                status: execution.status.clone(),
            });
            files = self.request_files(execution.run_id);
            files.extend(self.incoming_files(execution));
            files.extend(self.accepted_files(execution, true));
            missing = self.runs[&execution.run_id].missing.clone();
            if let Some((batch, item)) = self.execution_batch(execution) {
                files.extend(self.batch_files(batch, item));
                // Workers expose only missing dependencies; a splitter/summary
                // exposes the exact batch, never another iteration's work.
                missing.extend(
                    batch
                        .items
                        .iter()
                        .filter(|source| {
                            source.state != "succeeded"
                                && item.is_none_or(|item| item.dependencies.contains(&source.id))
                        })
                        .map(missing_subtask),
                );
                if item.is_some() {
                    missing.retain(|source| {
                        item.is_some_and(|item| item.dependencies.contains(&source.subtask_id))
                    });
                }
            }
        } else {
            resolved_execution = None;
            files = self.request_files(run_id);
            missing = run.missing.clone();
            for id in &run.effective {
                files.extend(self.accepted_files(&self.executions[id], true));
            }
            for batch in self.batches.iter().filter(|batch| batch.run_id == run_id) {
                files.extend(self.batch_files(batch, None));
                // Summary-only rework keeps successful workers through exact
                // batch membership, even without ordinary result-reuse rows.
                for id in batch
                    .items
                    .iter()
                    .filter(|item| item.state == "succeeded")
                    .filter_map(|item| item.execution_id)
                {
                    files.extend(self.accepted_files(&self.executions[&id], true));
                }
                missing.extend(
                    batch
                        .items
                        .iter()
                        .filter(|item| item.state != "succeeded")
                        .map(missing_subtask),
                );
            }
        }
        files.sort_by(|left, right| {
            (
                role_order(left.role),
                &left.path,
                left.source_run_id,
                left.source_node_execution_id,
                left.sha256.is_none(),
            )
                .cmp(&(
                    role_order(right.role),
                    &right.path,
                    right.source_run_id,
                    right.source_node_execution_id,
                    right.sha256.is_none(),
                ))
        });
        let mut seen = HashSet::new();
        files.retain(|file| {
            seen.insert((
                file.role,
                file.path.clone(),
                file.source_run_id,
                file.source_node_execution_id,
            ))
        });
        let mut seen_missing = HashSet::new();
        missing.retain(|item| {
            seen_missing.insert((
                item.subtask_id.clone(),
                item.title.clone(),
                item.state.clone(),
                item.reason.clone(),
            ))
        });
        Ok(WorkflowFileContextView {
            run_id,
            workspace_id: run.workspace_id,
            node_execution_id: selected,
            execution: resolved_execution,
            files,
            missing_subtasks: missing,
        })
    }
}

fn missing_subtask(item: &SubtaskFiles) -> WorkflowMissingSubtaskView {
    WorkflowMissingSubtaskView {
        subtask_id: item.id.clone(),
        title: item.title.clone(),
        state: item.state.clone(),
        reason: item.error.clone(),
    }
}

fn role_order(role: WorkflowContextFileRole) -> u8 {
    match role {
        WorkflowContextFileRole::Output => 0,
        WorkflowContextFileRole::Input => 1,
        WorkflowContextFileRole::Control => 2,
        WorkflowContextFileRole::Handoff => 3,
    }
}

pub async fn read_workflow_file_context(
    pool: &SqlitePool,
    run_id: Uuid,
    node_execution_id: Option<Uuid>,
) -> Result<WorkflowFileContextView, ApiError> {
    // One read snapshot prevents polling from combining different acceptance
    // generations. No IMMEDIATE writer, file access, hash check or reconciliation.
    let mut transaction = pool.begin().await?;
    let snapshot = FileSnapshot::load(&mut transaction, run_id).await?;
    let view = snapshot.project(run_id, node_execution_id)?;
    transaction.commit().await?;
    Ok(view)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    async fn fixture() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(r#"
            CREATE TABLE workflow_runs(id BLOB PRIMARY KEY,workspace_id BLOB,graph_snapshot TEXT);
            CREATE TABLE node_executions(id BLOB PRIMARY KEY,run_id BLOB,node_id TEXT,node_type TEXT,iteration INTEGER,status TEXT,session_id BLOB,started_at TEXT,finished_at TEXT);
            CREATE TABLE workflow_result_reuse(run_id BLOB,node_id TEXT,iteration INTEGER,source_node_execution_id BLOB);
            CREATE VIEW workflow_effective_node_executions AS
                SELECT run_id,id AS source_node_execution_id,node_id,iteration,status FROM node_executions
                UNION ALL SELECT reuse.run_id,n.id,reuse.node_id,reuse.iteration,n.status FROM workflow_result_reuse reuse JOIN node_executions n ON n.id=reuse.source_node_execution_id;
            CREATE TABLE workflow_node_handoffs(node_execution_id BLOB PRIMARY KEY,run_id BLOB,handoff_path TEXT,handoff_sha256 TEXT,files_json TEXT);
            CREATE TABLE workflow_run_submissions(run_id BLOB,action TEXT,material_paths_json TEXT,source_run_id BLOB);
            CREATE TABLE workflow_run_deliveries(run_id BLOB,missing_subtasks_json TEXT);
            CREATE TABLE workflow_split_batches(id BLOB PRIMARY KEY,run_id BLOB,node_execution_id BLOB,summary_node_execution_id BLOB,source_batch_id BLOB,manifest_path TEXT,manifest_sha256 TEXT,dispatch_path TEXT,dispatch_sha256 TEXT,created_at TEXT);
            CREATE TABLE workflow_split_subtasks(batch_id BLOB,subtask_id TEXT,title TEXT,instruction_path TEXT,instruction_sha256 TEXT,depends_on_json TEXT,node_execution_id BLOB,state TEXT,error_text TEXT,stage_index INTEGER);
        "#).execute(&pool).await.unwrap();
        pool
    }

    async fn run(pool: &SqlitePool, id: Uuid, edges: &[(&str, &str)]) -> Uuid {
        let workspace = Uuid::new_v4();
        let graph = json!({"version":2,"nodes":[],"edges":edges.iter().enumerate().map(|(index,(source,target))|json!({"id":index.to_string(),"source":source,"target":target,"type":"default"})).collect::<Vec<_>>()});
        sqlx::query("INSERT INTO workflow_runs VALUES (?,?,?)")
            .bind(id)
            .bind(workspace)
            .bind(graph.to_string())
            .execute(pool)
            .await
            .unwrap();
        workspace
    }

    async fn execution(
        pool: &SqlitePool,
        run: Uuid,
        node: &str,
        kind: &str,
        iteration: i64,
    ) -> (Uuid, Uuid) {
        let id = Uuid::new_v4();
        let session = Uuid::new_v4();
        sqlx::query("INSERT INTO node_executions VALUES (?,?,?,?,?,'succeeded',?,'2026-10-14 01:00:00','2026-10-14 01:00:01')")
            .bind(id).bind(run).bind(node).bind(kind).bind(iteration).bind(session).execute(pool).await.unwrap();
        (id, session)
    }

    async fn handoff(pool: &SqlitePool, run: Uuid, execution: Uuid, paths: &[&str]) {
        let references: Vec<_> = paths
            .iter()
            .map(|path| json!({"path":path,"sha256":format!("accepted:{path}")}))
            .collect();
        sqlx::query("INSERT INTO workflow_node_handoffs VALUES (?,?,?,?,?)")
            .bind(execution)
            .bind(run)
            .bind(format!(
                ".vibe-kanban/workflows/{run}/handoffs/{execution}.json"
            ))
            .bind("accepted-index-hash")
            .bind(serde_json::to_string(&references).unwrap())
            .execute(pool)
            .await
            .unwrap();
    }

    async fn batch(pool: &SqlitePool, run: Uuid, split: Uuid, summary: Uuid) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO workflow_split_batches VALUES (?,?,?,?,NULL,?,'manifest-hash',?,'dispatch-hash','2026-10-14')")
            .bind(id).bind(run).bind(split).bind(summary)
            .bind(format!(".vibe-kanban/workflows/{run}/splits/{split}/tasks.json"))
            .bind(format!(".vibe-kanban/workflows/{run}/splits/{split}/dispatch.json"))
            .execute(pool).await.unwrap();
        id
    }

    async fn item(
        pool: &SqlitePool,
        batch: Uuid,
        id: &str,
        execution: Option<Uuid>,
        state: &str,
        dependencies: &[&str],
    ) {
        sqlx::query("INSERT INTO workflow_split_subtasks VALUES (?,?,?,?,?,?,?,?,NULL,0)")
            .bind(batch)
            .bind(id)
            .bind(format!("Work {id}"))
            .bind(format!("instructions/{batch}/{id}.md"))
            .bind("instruction-hash")
            .bind(serde_json::to_string(dependencies).unwrap())
            .bind(execution)
            .bind(state)
            .execute(pool)
            .await
            .unwrap();
    }

    fn output_paths(view: &WorkflowFileContextView) -> Vec<&str> {
        view.files
            .iter()
            .filter(|file| file.role == WorkflowContextFileRole::Output)
            .map(|file| file.path.as_str())
            .collect()
    }

    #[tokio::test]
    async fn exact_execution_separates_inputs_outputs_and_rejects_unrelated_ids() {
        let pool = fixture().await;
        let run_id = Uuid::new_v4();
        run(&pool, run_id, &[("research", "report")]).await;
        let (research, research_session) = execution(&pool, run_id, "research", "agent", 0).await;
        let (report, _) = execution(&pool, run_id, "report", "agent", 0).await;
        sqlx::query("UPDATE node_executions SET started_at='2026-10-14 02:00:00' WHERE id=?")
            .bind(report)
            .execute(&pool)
            .await
            .unwrap();
        handoff(&pool, run_id, research, &["research.md"]).await;
        handoff(&pool, run_id, report, &["report.md", "materials.txt"]).await;
        sqlx::query(
            "INSERT INTO workflow_run_submissions VALUES (?,'start','[\"materials.txt\"]',NULL)",
        )
        .bind(run_id)
        .execute(&pool)
        .await
        .unwrap();
        let view = read_workflow_file_context(&pool, run_id, Some(report))
            .await
            .unwrap();
        assert_eq!(output_paths(&view), ["report.md"]);
        let input = view
            .files
            .iter()
            .find(|file| file.path == "research.md")
            .unwrap();
        assert_eq!(input.role, WorkflowContextFileRole::Input);
        assert_eq!(input.source_node_execution_id, Some(research));
        assert_eq!(input.source_session_id, Some(research_session));
        assert_eq!(
            view.files
                .iter()
                .filter(|file| file.path == "materials.txt")
                .count(),
            1
        );
        assert!(matches!(
            read_workflow_file_context(&pool, run_id, Some(Uuid::new_v4())).await,
            Err(ApiError::BadRequest(_))
        ));
    }

    #[tokio::test]
    async fn split_inputs_use_exact_dependencies_and_summary_batch_iteration() {
        let pool = fixture().await;
        let run_id = Uuid::new_v4();
        run(&pool, run_id, &[("split", "worker"), ("worker", "summary")]).await;
        let (split, _) = execution(&pool, run_id, "split", "task_splitter", 0).await;
        let (summary, _) = execution(&pool, run_id, "summary", "agent", 0).await;
        let (a, _) = execution(&pool, run_id, "worker", "agent", 0).await;
        let (b, _) = execution(&pool, run_id, "worker", "agent", 1).await;
        let (peer, _) = execution(&pool, run_id, "worker", "agent", 2).await;
        let current = batch(&pool, run_id, split, summary).await;
        item(&pool, current, "alpha", Some(a), "succeeded", &[]).await;
        item(&pool, current, "beta", Some(b), "succeeded", &["alpha"]).await;
        item(&pool, current, "unrelated-failure", None, "failed", &[]).await;
        handoff(&pool, run_id, a, &["alpha.md"]).await;
        handoff(&pool, run_id, b, &["beta.md"]).await;
        handoff(&pool, run_id, peer, &["other-batch.md"]).await;
        let (other_split, _) = execution(&pool, run_id, "split", "task_splitter", 1).await;
        let (other_summary, _) = execution(&pool, run_id, "summary", "agent", 1).await;
        let other = batch(&pool, run_id, other_split, other_summary).await;
        item(&pool, other, "peer", Some(peer), "succeeded", &[]).await;
        let worker = read_workflow_file_context(&pool, run_id, Some(b))
            .await
            .unwrap();
        assert_eq!(output_paths(&worker), ["beta.md"]);
        assert!(
            worker
                .files
                .iter()
                .any(|file| file.path == "alpha.md" && file.role == WorkflowContextFileRole::Input)
        );
        assert!(
            !worker.files.iter().any(|file| file.path == "other-batch.md"
                || file.path.ends_with("/unrelated-failure.md"))
        );
        assert!(worker.missing_subtasks.is_empty());
        let summary = read_workflow_file_context(&pool, run_id, Some(summary))
            .await
            .unwrap();
        assert!(summary.files.iter().any(|file| file.path == "alpha.md"));
        assert!(summary.files.iter().any(|file| file.path == "beta.md"));
        assert!(
            !summary
                .files
                .iter()
                .any(|file| file.path == "other-batch.md")
        );
        assert_eq!(summary.missing_subtasks.len(), 1);
        assert_eq!(summary.missing_subtasks[0].subtask_id, "unrelated-failure");
    }

    #[tokio::test]
    async fn reused_files_keep_original_run_workspace_and_nullable_session() {
        let pool = fixture().await;
        let original = Uuid::new_v4();
        let source_workspace = run(&pool, original, &[]).await;
        let (source, _) = execution(&pool, original, "research", "agent", 0).await;
        sqlx::query("UPDATE node_executions SET session_id=NULL WHERE id=?")
            .bind(source)
            .execute(&pool)
            .await
            .unwrap();
        handoff(&pool, original, source, &["kept.md"]).await;
        let current = Uuid::new_v4();
        let current_workspace = run(&pool, current, &[]).await;
        sqlx::query("INSERT INTO workflow_result_reuse VALUES (?,'research',0,?)")
            .bind(current)
            .bind(source)
            .execute(&pool)
            .await
            .unwrap();
        for selected in [None, Some(source)] {
            let view = read_workflow_file_context(&pool, current, selected)
                .await
                .unwrap();
            assert_eq!(view.run_id, current);
            assert_eq!(view.workspace_id, current_workspace);
            assert_eq!(output_paths(&view), ["kept.md"]);
            let output = view
                .files
                .iter()
                .find(|file| file.path == "kept.md")
                .unwrap();
            assert_eq!(output.source_run_id, original);
            assert_eq!(output.source_workspace_id, source_workspace);
            assert_eq!(output.source_node_execution_id, Some(source));
            assert_eq!(output.source_session_id, None);
            if selected.is_some() {
                let execution = view.execution.unwrap();
                assert_eq!(execution.node_execution_id, source);
                assert_eq!(execution.run_id, original);
                assert_eq!(execution.workspace_id, source_workspace);
                assert_eq!(execution.session_id, None);
            } else {
                assert!(view.execution.is_none());
            }
        }
    }

    #[tokio::test]
    async fn batch_bound_execution_resolves_without_output_files() {
        let pool = fixture().await;
        let original = Uuid::new_v4();
        let workspace = run(&pool, original, &[]).await;
        let (split, _) = execution(&pool, original, "split", "task_splitter", 0).await;
        let (worker, session) = execution(&pool, original, "worker", "agent", 0).await;
        let (unbound, _) = execution(&pool, original, "other", "agent", 0).await;
        let current = Uuid::new_v4();
        run(&pool, current, &[]).await;
        let (summary, _) = execution(&pool, current, "summary", "agent", 0).await;
        let accepted = batch(&pool, current, split, summary).await;
        item(&pool, accepted, "kept", Some(worker), "succeeded", &[]).await;

        let view = read_workflow_file_context(&pool, current, Some(worker))
            .await
            .unwrap();
        assert!(
            view.files
                .iter()
                .all(|file| file.source_node_execution_id != Some(worker))
        );
        let resolved = view.execution.unwrap();
        assert_eq!(resolved.node_execution_id, worker);
        assert_eq!(resolved.node_id, "worker");
        assert_eq!(resolved.run_id, original);
        assert_eq!(resolved.workspace_id, workspace);
        assert_eq!(resolved.session_id, Some(session));
        assert_eq!(resolved.status, "succeeded");
        assert!(matches!(
            read_workflow_file_context(&pool, current, Some(unbound)).await,
            Err(ApiError::BadRequest(_))
        ));
        assert!(
            read_workflow_file_context(&pool, current, None)
                .await
                .unwrap()
                .execution
                .is_none()
        );
    }

    #[tokio::test]
    async fn cloned_batch_members_and_dispatch_keep_their_actual_sources() {
        let pool = fixture().await;
        let original = Uuid::new_v4();
        let source_workspace = run(&pool, original, &[]).await;
        let (split, _) = execution(&pool, original, "split", "task_splitter", 0).await;
        let (worker, session) = execution(&pool, original, "worker", "agent", 0).await;
        let (old_summary, _) = execution(&pool, original, "summary", "agent", 0).await;
        let source_batch = batch(&pool, original, split, old_summary).await;
        item(&pool, source_batch, "kept", Some(worker), "succeeded", &[]).await;
        item(&pool, source_batch, "missing", None, "failed", &[]).await;
        handoff(&pool, original, worker, &["kept.md"]).await;

        let current = Uuid::new_v4();
        run(&pool, current, &[]).await;
        let (summary, _) = execution(&pool, current, "summary", "agent", 0).await;
        let clone = Uuid::new_v4();
        sqlx::query("INSERT INTO workflow_split_batches SELECT ?,?,node_execution_id,?,id,manifest_path,manifest_sha256,dispatch_path,dispatch_sha256,created_at FROM workflow_split_batches WHERE id=?")
            .bind(clone).bind(current).bind(summary).bind(source_batch).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO workflow_split_subtasks SELECT ?,subtask_id,title,instruction_path,instruction_sha256,depends_on_json,node_execution_id,state,error_text,stage_index FROM workflow_split_subtasks WHERE batch_id=?")
            .bind(clone).bind(source_batch).execute(&pool).await.unwrap();

        for selected in [None, Some(summary), Some(worker)] {
            let view = read_workflow_file_context(&pool, current, selected)
                .await
                .unwrap();
            let kept = view
                .files
                .iter()
                .find(|file| file.path == "kept.md")
                .unwrap();
            assert_eq!(kept.source_run_id, original);
            assert_eq!(kept.source_workspace_id, source_workspace);
            assert_eq!(kept.source_node_execution_id, Some(worker));
            assert_eq!(kept.source_session_id, Some(session));
            assert_eq!(
                kept.role,
                if selected == Some(summary) {
                    WorkflowContextFileRole::Input
                } else {
                    WorkflowContextFileRole::Output
                }
            );
            let dispatch = view
                .files
                .iter()
                .find(|file| file.path.ends_with("/dispatch.json"))
                .unwrap();
            assert_eq!(dispatch.source_run_id, original);
            assert_eq!(dispatch.source_workspace_id, source_workspace);
            assert_eq!(dispatch.source_node_execution_id, None);
            assert_eq!(dispatch.source_session_id, None);
            assert_eq!(dispatch.role, WorkflowContextFileRole::Control);
            if selected == Some(worker) {
                assert!(view.missing_subtasks.is_empty());
            } else {
                assert_eq!(view.missing_subtasks.len(), 1);
                assert_eq!(view.missing_subtasks[0].subtask_id, "missing");
            }
        }
    }

    #[tokio::test]
    async fn protocol_and_forwarded_files_never_inflate_delivery_counts() {
        let pool = fixture().await;
        let run_id = Uuid::new_v4();
        run(&pool, run_id, &[("split", "report"), ("report", "end")]).await;
        let (split, _) = execution(&pool, run_id, "split", "task_splitter", 0).await;
        let (report, _) = execution(&pool, run_id, "report", "agent", 0).await;
        let (end, _) = execution(&pool, run_id, "end", "end", 0).await;
        let batch = batch(&pool, run_id, split, report).await;
        item(&pool, batch, "alpha", None, "failed", &[]).await;
        let manifest = format!(".vibe-kanban/workflows/{run_id}/splits/{split}/tasks.json");
        let instruction = format!("instructions/{batch}/alpha.md");
        handoff(&pool, run_id, split, &[&manifest, &instruction]).await;
        handoff(&pool, run_id, report, &["report.md"]).await;
        handoff(&pool, run_id, end, &["report.md"]).await;
        let all = read_workflow_file_context(&pool, run_id, None)
            .await
            .unwrap();
        assert_eq!(output_paths(&all), ["report.md"]);
        let splitter = read_workflow_file_context(&pool, run_id, Some(split))
            .await
            .unwrap();
        assert!(output_paths(&splitter).is_empty());
        assert!(
            splitter
                .files
                .iter()
                .any(|file| file.path == manifest && file.role == WorkflowContextFileRole::Control)
        );
        assert!(
            splitter
                .files
                .iter()
                .any(|file| file.path == instruction
                    && file.role == WorkflowContextFileRole::Control)
        );
    }

    #[tokio::test]
    async fn metadata_read_needs_no_files_and_does_not_mutate_runtime() {
        let pool = fixture().await;
        let run_id = Uuid::new_v4();
        run(&pool, run_id, &[]).await;
        let (source, _) = execution(&pool, run_id, "report", "agent", 0).await;
        handoff(&pool, run_id, source, &["deleted-or-changed.md"]).await;
        let before: i64 = sqlx::query_scalar("SELECT total_changes()")
            .fetch_one(&pool)
            .await
            .unwrap();
        let view = read_workflow_file_context(&pool, run_id, Some(source))
            .await
            .unwrap();
        let after: i64 = sqlx::query_scalar("SELECT total_changes()")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(output_paths(&view), ["deleted-or-changed.md"]);
        assert_eq!(
            view.files
                .iter()
                .find(|file| file.path == "deleted-or-changed.md")
                .unwrap()
                .sha256
                .as_deref(),
            Some("accepted:deleted-or-changed.md")
        );
    }

    #[tokio::test]
    async fn retry_uses_original_materials_and_ignores_unused_new_materials() {
        let pool = fixture().await;
        let original = Uuid::new_v4();
        run(&pool, original, &[]).await;
        sqlx::query(
            "INSERT INTO workflow_run_submissions VALUES (?,'start','[\"source.txt\"]',NULL)",
        )
        .bind(original)
        .execute(&pool)
        .await
        .unwrap();
        let retry = Uuid::new_v4();
        run(&pool, retry, &[]).await;
        let (report, _) = execution(&pool, retry, "report", "agent", 0).await;
        sqlx::query("INSERT INTO workflow_run_submissions VALUES (?,'retry','[\"unused.txt\"]',?)")
            .bind(retry)
            .bind(original)
            .execute(&pool)
            .await
            .unwrap();
        let view = read_workflow_file_context(&pool, retry, Some(report))
            .await
            .unwrap();
        let material = view
            .files
            .iter()
            .find(|file| file.path == "source.txt")
            .unwrap();
        assert_eq!(material.source_run_id, original);
        assert_eq!(material.source_node_execution_id, None);
        assert!(!view.files.iter().any(|file| file.path == "unused.txt"));
    }

    #[tokio::test]
    async fn an_ordinary_iteration_does_not_claim_future_upstream_files() {
        let pool = fixture().await;
        let run_id = Uuid::new_v4();
        run(&pool, run_id, &[("research", "report")]).await;
        let (early, _) = execution(&pool, run_id, "research", "agent", 0).await;
        let (late, _) = execution(&pool, run_id, "research", "agent", 1).await;
        let (report, _) = execution(&pool, run_id, "report", "agent", 0).await;
        sqlx::query("UPDATE node_executions SET started_at='2026-10-14 02:00:00' WHERE id=?")
            .bind(report)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE node_executions SET finished_at='2026-10-14 03:00:00' WHERE id=?")
            .bind(late)
            .execute(&pool)
            .await
            .unwrap();
        handoff(&pool, run_id, early, &["early.md"]).await;
        handoff(&pool, run_id, late, &["future.md"]).await;
        let view = read_workflow_file_context(&pool, run_id, Some(report))
            .await
            .unwrap();
        assert!(view.files.iter().any(|file| file.path == "early.md"));
        assert!(!view.files.iter().any(|file| file.path == "future.md"));
    }
}
