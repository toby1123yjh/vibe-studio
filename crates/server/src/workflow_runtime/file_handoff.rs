//! File content is consumed from disk. Durable bindings contain only identity,
//! paths and content hashes; an Agent transcript is never a fallback input.
use workflow::files::{
    WorkflowFileRef, WorkflowHandoff, control_file_path, readable_file, sha256_bytes,
    write_json_atomic, write_text_atomic,
};

use super::*;

pub(super) fn file_error(path: &str, error: impl std::fmt::Display) -> ApiError {
    // The supplied reference is relative. Do not expose the host's root path.
    ApiError::BadRequest(format!(
        "Workflow file `{path}` is unavailable or invalid: {error}"
    ))
}

pub(super) async fn execution_id(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    iteration: i64,
) -> Result<Uuid, ApiError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM node_executions WHERE run_id=? AND node_id=? AND iteration=?",
    )
    .bind(run_id)
    .bind(node_id)
    .bind(iteration)
    .fetch_one(pool)
    .await?)
}

pub(super) fn index_path(run_id: Uuid, execution_id: Uuid) -> String {
    format!(".vibe-kanban/workflows/{run_id}/handoffs/{execution_id}.json")
}

pub(crate) async fn read_bound_file(
    root: &std::path::Path,
    reference: &WorkflowFileRef,
) -> Result<Vec<u8>, ApiError> {
    let path = readable_file(root, &reference.path)
        .await
        .map_err(|error| file_error(&reference.path, error))?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| file_error(&reference.path, error))?;
    if sha256_bytes(&bytes) != reference.sha256 {
        return Err(ApiError::Conflict(format!(
            "Workflow file `{}` changed after acceptance",
            reference.path
        )));
    }
    Ok(bytes)
}

pub(super) async fn bind_file(
    root: &std::path::Path,
    relative: &str,
) -> Result<WorkflowFileRef, ApiError> {
    let path = readable_file(root, relative)
        .await
        .map_err(|error| file_error(relative, error))?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| file_error(relative, error))?;
    Ok(WorkflowFileRef {
        path: relative.replace('\\', "/"),
        sha256: sha256_bytes(&bytes),
    })
}

pub(super) async fn request_reference(
    pool: &SqlitePool,
    run_id: Uuid,
) -> Result<WorkflowFileRef, ApiError> {
    let (workspace_id, input): (Uuid, String) =
        sqlx::query_as("SELECT workspace_id,input_text FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    let root = workflow_control_root(pool, workspace_id).await?;
    let relative = format!(".vibe-kanban/workflows/{run_id}/request.md");
    let path = control_file_path(&root, &relative)
        .await
        .map_err(|error| file_error(&relative, error))?;
    match tokio::fs::read(&path).await {
        Ok(bytes) if bytes != input.as_bytes() => {
            return Err(ApiError::Conflict(
                "The accepted workflow request file was modified".into(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_text_atomic(&path, &input)
                .await
                .map_err(|error| file_error(&relative, error))?;
        }
        Err(error) => return Err(file_error(&relative, error)),
    }
    Ok(WorkflowFileRef {
        path: relative,
        sha256: sha256_bytes(input.as_bytes()),
    })
}

/// Validate the Agent-written index after its real successful terminal event.
/// Acceptance is immutable: replay verifies the original bytes and references.
pub(super) async fn accept(
    pool: &SqlitePool,
    run_id: Uuid,
    node_execution_id: Uuid,
    require_content: bool,
) -> Result<String, ApiError> {
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    let root = workflow_control_root(pool, workspace_id).await?;
    let relative = index_path(run_id, node_execution_id);
    let index = bind_file(&root, &relative).await?;
    let bytes = read_bound_file(&root, &index).await?;
    let handoff: WorkflowHandoff =
        serde_json::from_slice(&bytes).map_err(|error| file_error(&relative, error))?;
    if handoff.schema_version != 1
        || handoff.run_id != run_id.to_string()
        || handoff.node_execution_id != node_execution_id.to_string()
    {
        return Err(ApiError::BadRequest(format!(
            "Workflow handoff `{relative}` has the wrong schema or execution identity"
        )));
    }
    let mut paths = handoff.artifact_paths;
    if let Some(summary) = handoff.summary_path {
        paths.insert(0, summary);
    }
    if require_content && paths.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "Workflow handoff `{relative}` contains no readable delivery file"
        )));
    }
    let mut seen = HashSet::new();
    let mut references = Vec::new();
    for path in paths {
        if path.replace('\\', "/") == relative {
            return Err(ApiError::BadRequest(
                "A handoff index cannot be its own deliverable".into(),
            ));
        }
        if seen.insert(path.replace('\\', "/")) {
            references.push(bind_file(&root, &path).await?);
        }
    }
    let files_json =
        serde_json::to_string(&references).map_err(|error| file_error(&relative, error))?;
    // Acquire the writer before checking accepted state. A deferred WAL
    // snapshot cannot safely upgrade while another Agent is projecting events.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    // A canceled/failed execution cannot acquire new delivery after stopping.
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM node_executions n JOIN workflow_runs r ON r.id=n.run_id WHERE n.id=? AND n.run_id=? AND n.status IN ('pending','running','awaiting_human','awaiting_arena','cancelling','succeeded') AND r.status NOT IN ('canceled','failed','succeeded'))")
        .bind(node_execution_id).bind(run_id).fetch_one(&mut *tx).await?;
    if !valid {
        return Err(ApiError::Conflict(
            "Workflow handoff execution is no longer active".into(),
        ));
    }
    let existing: Option<(String, String, String)> = sqlx::query_as("SELECT handoff_path,handoff_sha256,files_json FROM workflow_node_handoffs WHERE node_execution_id=?")
        .bind(node_execution_id).fetch_optional(&mut *tx).await?;
    if let Some((path, hash, files)) = existing {
        if path != relative || hash != index.sha256 || files != files_json {
            return Err(ApiError::Conflict(
                "Workflow handoff changed after acceptance".into(),
            ));
        }
    } else {
        sqlx::query("INSERT INTO workflow_node_handoffs(node_execution_id,run_id,handoff_path,handoff_sha256,files_json) VALUES (?,?,?,?,?)")
            .bind(node_execution_id).bind(run_id).bind(&relative).bind(&index.sha256).bind(&files_json).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(relative)
}

/// Read only accepted successful producers, including exact reused execution
/// identities. There is deliberately no `output_text` content fallback.
pub(super) async fn verified(
    pool: &SqlitePool,
    root: &std::path::Path,
    execution_id: Uuid,
) -> Result<(WorkflowFileRef, Vec<WorkflowFileRef>), ApiError> {
    let (path, hash, files): (String, String, String) = sqlx::query_as("SELECT h.handoff_path,h.handoff_sha256,h.files_json FROM workflow_node_handoffs h JOIN node_executions n ON n.id=h.node_execution_id WHERE h.node_execution_id=? AND n.status='succeeded'")
        .bind(execution_id).fetch_optional(pool).await?
        .ok_or_else(|| ApiError::Conflict(format!("Successful source execution {execution_id} has no accepted file handoff")))?;
    let index = WorkflowFileRef { path, sha256: hash };
    read_bound_file(root, &index).await?;
    let files: Vec<WorkflowFileRef> =
        serde_json::from_str(&files).map_err(|error| file_error(&index.path, error))?;
    for reference in &files {
        read_bound_file(root, reference).await?;
    }
    Ok((index, files))
}

pub(super) async fn incoming(
    pool: &SqlitePool,
    graph: &WorkflowGraph,
    run_id: Uuid,
    node: &WorkflowNode,
    iteration: i64,
) -> Result<Vec<(String, Uuid, WorkflowFileRef, Vec<WorkflowFileRef>)>, ApiError> {
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    let root = workflow_control_root(pool, workspace_id).await?;
    // Summary-only deliveries and batch exits have explicit membership, not a
    // guessed latest iteration for each graph edge.
    if let Some(batch_id) = sqlx::query_scalar::<_, Uuid>("SELECT id FROM workflow_split_batches WHERE run_id=? AND summary_node_execution_id IN (SELECT id FROM node_executions WHERE run_id=? AND node_id=? AND iteration=?)")
        .bind(run_id).bind(run_id).bind(&node.id).bind(iteration).fetch_optional(pool).await? {
        let rows: Vec<(String, Uuid)> = sqlx::query_as("SELECT subtask_id,node_execution_id FROM workflow_split_subtasks WHERE batch_id=? AND state='succeeded' ORDER BY stage_index,subtask_id")
            .bind(batch_id).fetch_all(pool).await?;
        let mut result = Vec::new();
        for (id, execution_id) in rows {
            let (index, files) = verified(pool, &root, execution_id).await?;
            result.push((id, execution_id, index, files));
        }
        return Ok(result);
    }
    let mut result = Vec::new();
    for edge in graph.edges.iter().filter(|edge| edge.target == node.id) {
        let executions: Vec<Uuid> = sqlx::query_scalar("SELECT source_node_execution_id FROM workflow_effective_node_executions WHERE run_id=? AND node_id=? AND status='succeeded' ORDER BY iteration")
            .bind(run_id).bind(&edge.source).fetch_all(pool).await?;
        for id in executions {
            let (index, files) = verified(pool, &root, id).await?;
            result.push((edge.source.clone(), id, index, files));
        }
    }
    Ok(result)
}

pub(super) async fn context(
    pool: &SqlitePool,
    graph: &WorkflowGraph,
    run_id: Uuid,
    node: &WorkflowNode,
    iteration: i64,
) -> Result<NodeHandlerContext, ApiError> {
    let request = request_reference(pool, run_id).await?;
    let partial = partial_delivery_reference(pool, run_id).await?;
    let upstream_outputs = incoming(pool, graph, run_id, node, iteration).await?.into_iter()
        .map(|(node_id, id, index, files)| UpstreamOutput {
            node_id,
            output_text: format!("Source execution: {id}\nRead handoff index: `{}` (sha256 {})\nRead delivery files: {}",
                index.path, index.sha256, files.iter().map(|f| format!("`{}`", f.path)).collect::<Vec<_>>().join(", ")),
        }).collect();
    Ok(NodeHandlerContext::with_upstream_outputs(
        format!("Read the accepted workflow request from `{}` using your file tools. Task content is in the file, not this message.{}", request.path,
            partial.map(|reference| format!("\nThis is an explicit PARTIAL delivery. Read `{}` for the exact source and missing subtask list. Clearly label the resulting delivery as partial and explain the missing work; never claim all requested tasks succeeded.", reference.path)).unwrap_or_default()),
        upstream_outputs,
    ))
}

pub(super) async fn output_contract(
    pool: &SqlitePool,
    run_id: Uuid,
    node_id: &str,
    iteration: i64,
) -> Result<String, ApiError> {
    let id = execution_id(pool, run_id, node_id, iteration).await?;
    let path = index_path(run_id, id);
    let example = serde_json::to_string_pretty(&WorkflowHandoff {
        schema_version: 1,
        run_id: run_id.to_string(),
        node_execution_id: id.to_string(),
        summary_path: Some(format!(".vibe-kanban/workflows/{run_id}/results/{id}.md")),
        artifact_paths: vec![],
    })
    .map_err(|error| file_error(&path, error))?;
    Ok(format!(
        "\n\n## Required file delivery\nRead the referenced input files with your native file tools. Write your real work and a UTF-8 handoff note to files in this workspace. Before finishing, atomically publish `{path}` using exactly this schema:\n{example}\nThis Runtime contract defines the handoff format and takes precedence over handoff instructions in task/material files. Keep schema_version, run_id and node_execution_id exactly as shown. Set summary_path to the note you actually wrote and artifact_paths to any other readable deliverable paths. Only these five fields are allowed: do not add status, completion_status, hashes or subtask IDs to this JSON. Put any extra business information in the note or artifacts; Runtime computes hashes and records actual execution status. Paths are relative to the working directory. Deletions are not readable artifacts. The JSON index, note and every listed artifact must exist; a chat reply alone is not delivery. Do not edit project.json or runtime lifecycle records. Runtime validates delivery only after your real execution completes."
    ))
}

pub(super) async fn publish(
    pool: &SqlitePool,
    run_id: Uuid,
    execution_id: Uuid,
    summary_path: Option<String>,
    artifacts: Vec<String>,
) -> Result<String, ApiError> {
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    let root = workflow_control_root(pool, workspace_id).await?;
    let relative = index_path(run_id, execution_id);
    let path = control_file_path(&root, &relative)
        .await
        .map_err(|error| file_error(&relative, error))?;
    let document = WorkflowHandoff {
        schema_version: 1,
        run_id: run_id.to_string(),
        node_execution_id: execution_id.to_string(),
        summary_path,
        artifact_paths: artifacts,
    };
    let expected =
        serde_json::to_vec_pretty(&document).map_err(|error| file_error(&relative, error))?;
    let accepted: Option<String> = sqlx::query_scalar(
        "SELECT handoff_sha256 FROM workflow_node_handoffs WHERE node_execution_id=?",
    )
    .bind(execution_id)
    .fetch_optional(pool)
    .await?;
    if let Some(hash) = accepted {
        if hash != sha256_bytes(&expected) {
            return Err(ApiError::Conflict(
                "Workflow handoff changed after acceptance".into(),
            ));
        }
        read_bound_file(
            &root,
            &WorkflowFileRef {
                path: relative.clone(),
                sha256: hash,
            },
        )
        .await?;
    } else {
        write_json_atomic(&path, &document)
            .await
            .map_err(|error| file_error(&relative, error))?;
    }
    accept(pool, run_id, execution_id, false).await
}

/// Partial delivery limitations are input files, never a fabricated successful
/// producer. The DB retains only explicit source identities and the missing set.
pub(super) async fn partial_delivery_reference(
    pool: &SqlitePool,
    run_id: Uuid,
) -> Result<Option<WorkflowFileRef>, ApiError> {
    let row: Option<(Uuid,Uuid,String)> = sqlx::query_as("SELECT source_run_id,source_batch_id,missing_subtasks_json FROM workflow_run_deliveries WHERE run_id=? AND completeness='partial'")
        .bind(run_id).fetch_optional(pool).await?;
    let Some((source_run_id, source_batch_id, missing)) = row else {
        return Ok(None);
    };
    let document = json!({"schema_version":1,"run_id":run_id,"completeness":"partial",
        "source_run_id":source_run_id,"source_batch_id":source_batch_id,
        "missing_subtasks":serde_json::from_str::<Value>(&missing).map_err(|error| ApiError::Conflict(error.to_string()))?});
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| ApiError::Conflict(error.to_string()))?;
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    let root = workflow_control_root(pool, workspace_id).await?;
    let relative = format!(".vibe-kanban/workflows/{run_id}/partial-delivery.json");
    let path = control_file_path(&root, &relative)
        .await
        .map_err(|error| file_error(&relative, error))?;
    match tokio::fs::read(&path).await {
        Ok(bytes) if bytes != text.as_bytes() => {
            return Err(ApiError::Conflict(
                "Partial delivery input changed after acceptance".into(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_text_atomic(&path, &text)
                .await
                .map_err(|error| file_error(&relative, error))?
        }
        Err(error) => return Err(file_error(&relative, error)),
    }
    Ok(Some(WorkflowFileRef {
        path: relative,
        sha256: sha256_bytes(text.as_bytes()),
    }))
}

pub(super) async fn forward(
    pool: &SqlitePool,
    graph: &WorkflowGraph,
    run_id: Uuid,
    node: &WorkflowNode,
    iteration: i64,
) -> Result<String, ApiError> {
    let id = execution_id(pool, run_id, &node.id, iteration).await?;
    let mut paths: Vec<String> = incoming(pool, graph, run_id, node, iteration)
        .await?
        .into_iter()
        .flat_map(|(_, _, _, files)| files.into_iter().map(|f| f.path))
        .collect();
    if node.kind == WorkflowNodeKind::Start {
        paths.push(request_reference(pool, run_id).await?.path);
    }
    if let Some(partial) = partial_delivery_reference(pool, run_id).await? {
        paths.push(partial.path);
    }
    paths.sort();
    paths.dedup();
    publish(pool, run_id, id, None, paths).await
}

pub(super) async fn transform(
    pool: &SqlitePool,
    graph: &WorkflowGraph,
    run_id: Uuid,
    node: &WorkflowNode,
    iteration: i64,
) -> Result<String, ApiError> {
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    let root = workflow_control_root(pool, workspace_id).await?;
    let incoming = incoming(pool, graph, run_id, node, iteration).await?;
    let mut refs: Vec<WorkflowFileRef> = incoming
        .into_iter()
        .flat_map(|(_, _, _, files)| files)
        .collect();
    if refs.is_empty() {
        refs.push(request_reference(pool, run_id).await?);
    }
    let mut contents = Vec::new();
    let mut seen = HashSet::new();
    for reference in refs {
        if !seen.insert(reference.path.clone()) {
            continue;
        }
        let bytes = read_bound_file(&root, &reference).await?;
        contents.push(String::from_utf8(bytes).map_err(|_| {
            ApiError::BadRequest(format!(
                "Transform input `{}` must be UTF-8 text",
                reference.path
            ))
        })?);
    }
    let mut output = workflow::transform::apply_transform(&node.data, &contents.join("\n\n"))
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let mut artifacts = Vec::new();
    if let Some(partial) = partial_delivery_reference(pool, run_id).await? {
        // Read and retain the limitations even when the configured transform
        // only extracts a field or formats the successful result documents.
        let limitations = String::from_utf8(read_bound_file(&root, &partial).await?)
            .map_err(|_| ApiError::Conflict("Partial delivery limitations are not UTF-8".into()))?;
        output = format!(
            "# Partial workflow delivery\n\n{output}\n\n## Missing work and delivery limits\n\n```json\n{limitations}\n```\n"
        );
        artifacts.push(partial.path);
    }
    let id = execution_id(pool, run_id, &node.id, iteration).await?;
    let relative = format!(".vibe-kanban/workflows/{run_id}/results/{id}.md");
    write_result_note(pool, id, &root, &relative, &output).await?;
    publish(pool, run_id, id, Some(relative), artifacts).await
}

/// A crash between acceptance and the terminal projection must not let a
/// runtime-produced note overwrite or silently repair an accepted handoff.
pub(super) async fn write_result_note(
    pool: &SqlitePool,
    execution_id: Uuid,
    root: &std::path::Path,
    relative: &str,
    output: &str,
) -> Result<(), ApiError> {
    let accepted: Option<String> = sqlx::query_scalar(
        "SELECT files_json FROM workflow_node_handoffs WHERE node_execution_id=?",
    )
    .bind(execution_id)
    .fetch_optional(pool)
    .await?;
    if let Some(accepted) = accepted {
        let references: Vec<WorkflowFileRef> =
            serde_json::from_str(&accepted).map_err(|error| file_error(relative, error))?;
        let reference = references
            .iter()
            .find(|reference| reference.path == relative)
            .ok_or_else(|| ApiError::Conflict("Accepted workflow result note changed".into()))?;
        if reference.sha256 != sha256_bytes(output.as_bytes()) {
            return Err(ApiError::Conflict(
                "Accepted workflow result content changed".into(),
            ));
        }
        read_bound_file(root, reference).await?;
        return Ok(());
    }
    let path = control_file_path(root, relative)
        .await
        .map_err(|error| file_error(relative, error))?;
    write_text_atomic(&path, output)
        .await
        .map_err(|error| file_error(relative, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (SqlitePool, tempfile::TempDir, Uuid) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        fixture_with_pool(pool).await
    }

    async fn fixture_with_pool(pool: SqlitePool) -> (SqlitePool, tempfile::TempDir, Uuid) {
        sqlx::raw_sql(r#"
            CREATE TABLE workspaces(id BLOB PRIMARY KEY,container_ref TEXT,workspace_kind TEXT);
            CREATE TABLE repos(id BLOB PRIMARY KEY,name TEXT,display_name TEXT,default_working_dir TEXT);
            CREATE TABLE workspace_repos(workspace_id BLOB,repo_id BLOB);
            CREATE TABLE workflow_runs(id BLOB PRIMARY KEY,workspace_id BLOB,input_text TEXT,status TEXT);
            CREATE TABLE node_executions(id BLOB PRIMARY KEY,run_id BLOB,node_id TEXT,iteration INTEGER,status TEXT,output_text TEXT);
            CREATE VIEW workflow_effective_node_executions AS SELECT run_id,id AS source_node_execution_id,node_id,iteration,status FROM node_executions;
            CREATE TABLE workflow_node_handoffs(node_execution_id BLOB PRIMARY KEY,run_id BLOB,handoff_path TEXT,handoff_sha256 TEXT,files_json TEXT);
            CREATE TABLE workflow_split_batches(id BLOB PRIMARY KEY,run_id BLOB,summary_node_execution_id BLOB);
            CREATE TABLE workflow_split_subtasks(batch_id BLOB,subtask_id TEXT,state TEXT,node_execution_id BLOB,stage_index INTEGER);
            CREATE TABLE workflow_run_deliveries(run_id BLOB,completeness TEXT,source_run_id BLOB,source_batch_id BLOB,missing_subtasks_json TEXT);
        "#).execute(&pool).await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let workspace_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        sqlx::query("INSERT INTO workspaces VALUES (?,?,'direct_folder')")
            .bind(workspace_id)
            .bind(root.path().to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_runs VALUES (?,?,'Write a research report','running')")
            .bind(run_id)
            .bind(workspace_id)
            .execute(&pool)
            .await
            .unwrap();
        (pool, root, run_id)
    }

    async fn execution(pool: &SqlitePool, run_id: Uuid, node: &str, iteration: i64) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO node_executions VALUES (?,?,?,?, 'running','TRANSCRIPT_MUST_NOT_BE_AN_INPUT')")
            .bind(id).bind(run_id).bind(node).bind(iteration).execute(pool).await.unwrap();
        id
    }

    async fn deliver(
        pool: &SqlitePool,
        root: &std::path::Path,
        run_id: Uuid,
        id: Uuid,
        path: &str,
    ) {
        let document = WorkflowHandoff {
            schema_version: 1,
            run_id: run_id.to_string(),
            node_execution_id: id.to_string(),
            summary_path: Some(path.into()),
            artifact_paths: vec![],
        };
        let index = control_file_path(root, &index_path(run_id, id))
            .await
            .unwrap();
        write_json_atomic(&index, &document).await.unwrap();
        accept(pool, run_id, id, true).await.unwrap();
        sqlx::query("UPDATE node_executions SET status='succeeded' WHERE id=?")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }

    fn graph() -> WorkflowGraph {
        serde_json::from_value(json!({"version":2,"nodes":[
            {"id":"research","type":"agent","data":{}},
            {"id":"report","type":"agent","data":{}}
        ],"edges":[{"id":"edge","source":"research","target":"report","type":"default"}]}))
        .unwrap()
    }

    #[tokio::test]
    async fn handoff_acceptance_waits_for_an_independent_sqlite_writer() {
        use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

        let database = tempfile::tempdir().unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(database.path().join("handoff.sqlite"))
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(std::time::Duration::from_secs(5)),
            )
            .await
            .unwrap();
        let (pool, root, run_id) = fixture_with_pool(pool).await;
        let id = execution(&pool, run_id, "research", 0).await;
        tokio::fs::write(root.path().join("research.md"), "actual delivery")
            .await
            .unwrap();
        let document = WorkflowHandoff {
            schema_version: 1,
            run_id: run_id.to_string(),
            node_execution_id: id.to_string(),
            summary_path: Some("research.md".into()),
            artifact_paths: vec![],
        };
        let path = control_file_path(root.path(), &index_path(run_id, id))
            .await
            .unwrap();
        write_json_atomic(&path, &document).await.unwrap();
        {
            let mut writer = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
            sqlx::query("UPDATE node_executions SET output_text='independent writer' WHERE id=?")
                .bind(id)
                .execute(&mut *writer)
                .await
                .unwrap();
            let acceptance = accept(&pool, run_id, id, true);
            tokio::pin!(acceptance);
            tokio::select! {
                result = &mut acceptance => panic!("Handoff did not wait for the writer: {result:?}"),
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
            }
            writer.commit().await.unwrap();
            assert_eq!(acceptance.await.unwrap(), index_path(run_id, id));
        }
        assert_eq!(
            accept(&pool, run_id, id, true).await.unwrap(),
            index_path(run_id, id)
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_node_handoffs")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        pool.close().await;
    }

    #[tokio::test]
    async fn ordinary_handoff_is_file_only_and_missing_artifact_has_no_transcript_fallback() {
        let (pool, root, run_id) = fixture().await;
        let id = execution(&pool, run_id, "research", 0).await;
        tokio::fs::write(
            root.path().join("research.md"),
            "FILE_BODY_MUST_NOT_BE_COPIED",
        )
        .await
        .unwrap();
        deliver(&pool, root.path(), run_id, id, "research.md").await;
        let graph = graph();
        let context = context(&pool, &graph, run_id, &graph.nodes[1], 0)
            .await
            .unwrap();
        let prompt = format!("{}{}", context.run_input_text, context.upstream_text());
        assert!(prompt.contains("research.md"));
        assert!(prompt.contains(&id.to_string()));
        assert!(!prompt.contains("TRANSCRIPT_MUST_NOT_BE_AN_INPUT"));
        assert!(!prompt.contains("FILE_BODY_MUST_NOT_BE_COPIED"));
        tokio::fs::remove_file(root.path().join("research.md"))
            .await
            .unwrap();
        assert!(
            incoming(&pool, &graph, run_id, &graph.nodes[1], 0)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn successful_reply_without_actual_delivery_files_is_rejected() {
        let (pool, root, run_id) = fixture().await;
        let id = execution(&pool, run_id, "research", 0).await;
        assert!(accept(&pool, run_id, id, true).await.is_err());
        let document = WorkflowHandoff {
            schema_version: 1,
            run_id: run_id.to_string(),
            node_execution_id: id.to_string(),
            summary_path: Some("missing.md".into()),
            artifact_paths: vec![],
        };
        let path = control_file_path(root.path(), &index_path(run_id, id))
            .await
            .unwrap();
        write_json_atomic(&path, &document).await.unwrap();
        assert!(accept(&pool, run_id, id, true).await.is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_node_handoffs")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn recovery_never_overwrites_or_recreates_an_accepted_result_note() {
        let (pool, root, run_id) = fixture().await;
        let id = execution(&pool, run_id, "report", 0).await;
        write_result_note(&pool, id, root.path(), "report.md", "accepted report")
            .await
            .unwrap();
        deliver(&pool, root.path(), run_id, id, "report.md").await;
        tokio::fs::write(root.path().join("report.md"), "changed after acceptance")
            .await
            .unwrap();
        assert!(
            write_result_note(&pool, id, root.path(), "report.md", "accepted report")
                .await
                .is_err()
        );
        assert_eq!(
            tokio::fs::read_to_string(root.path().join("report.md"))
                .await
                .unwrap(),
            "changed after acceptance"
        );
        tokio::fs::remove_file(root.path().join("report.md"))
            .await
            .unwrap();
        assert!(
            write_result_note(&pool, id, root.path(), "report.md", "accepted report")
                .await
                .is_err()
        );
        assert!(!root.path().join("report.md").exists());
    }

    #[tokio::test]
    async fn summary_reads_only_its_exact_batch_iteration() {
        let (pool, root, run_id) = fixture().await;
        let mut sources = Vec::new();
        for iteration in 0..2 {
            let source = execution(&pool, run_id, "research", iteration).await;
            let result = format!("research-{iteration}.md");
            tokio::fs::write(root.path().join(&result), format!("research {iteration}"))
                .await
                .unwrap();
            deliver(&pool, root.path(), run_id, source, &result).await;
            let summary = execution(&pool, run_id, "report", iteration).await;
            let batch = Uuid::new_v4();
            sqlx::query("INSERT INTO workflow_split_batches VALUES (?,?,?)")
                .bind(batch)
                .bind(run_id)
                .bind(summary)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO workflow_split_subtasks VALUES (?,'item','succeeded',?,0)")
                .bind(batch)
                .bind(source)
                .execute(&pool)
                .await
                .unwrap();
            sources.push(source);
        }
        let graph = graph();
        let refs = incoming(&pool, &graph, run_id, &graph.nodes[1], 1)
            .await
            .unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].1, sources[1]);
        assert_eq!(refs[0].3[0].path, "research-1.md");
    }

    #[tokio::test]
    async fn partial_transform_reads_and_retains_missing_work_limits() {
        let (pool, root, run_id) = fixture().await;
        let source = execution(&pool, run_id, "research", 0).await;
        tokio::fs::write(root.path().join("research.md"), "finished research")
            .await
            .unwrap();
        deliver(&pool, root.path(), run_id, source, "research.md").await;
        let summary = execution(&pool, run_id, "report", 0).await;
        sqlx::query("INSERT INTO workflow_run_deliveries VALUES (?,'partial',?,?,?)")
            .bind(run_id)
            .bind(Uuid::new_v4())
            .bind(Uuid::new_v4())
            .bind(r#"[{"subtask_id":"missing","title":"Unfinished analysis","state":"failed"}]"#)
            .execute(&pool)
            .await
            .unwrap();
        let mut graph = graph();
        graph.nodes[1].kind = WorkflowNodeKind::Transform;
        graph.nodes[1].data.mode = Some(workflow::graph::TransformMode::Template);
        graph.nodes[1].data.template = Some("{{input}}".into());
        transform(&pool, &graph, run_id, &graph.nodes[1], 0)
            .await
            .unwrap();
        let output = tokio::fs::read_to_string(root.path().join(format!(
            ".vibe-kanban/workflows/{run_id}/results/{summary}.md"
        )))
        .await
        .unwrap();
        assert!(output.contains("Partial workflow delivery"));
        assert!(output.contains("finished research"));
        assert!(output.contains("Unfinished analysis"));
        let references: String = sqlx::query_scalar(
            "SELECT files_json FROM workflow_node_handoffs WHERE node_execution_id=?",
        )
        .bind(summary)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(references.contains("partial-delivery.json"));
    }
}
