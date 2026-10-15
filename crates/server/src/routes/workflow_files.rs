//! Read current workflow files under the exact execution workspace, including
//! plain directories. A file reference does not imply historical byte storage.
use std::path::PathBuf;

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::Response,
    routing::get,
};
use cap_std::fs::Dir;
use deployment::Deployment;
use serde::{Deserialize, Serialize};
use services::services::file::FileError;
use tokio_util::io::ReaderStream;
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use super::workspaces::files::{
    WorkspaceFileContentKind, file_content_preview_from_file,
    workspace_raw_content_type_and_disposition,
};
use crate::{
    DeploymentImpl,
    error::ApiError,
    workflow_runtime::{
        file_context::{WorkflowFileContextView, read_workflow_file_context},
        runner,
    },
};

#[derive(Debug, Serialize, TS)]
pub struct WorkflowFileContent {
    pub run_id: Uuid,
    pub workspace_id: Uuid,
    pub path: String,
    pub name: String,
    pub kind: WorkspaceFileContentKind,
    pub mime_type: Option<String>,
    pub language: Option<String>,
    pub content: Option<String>,
    pub raw_url: Option<String>,
    #[ts(type = "number")]
    pub size_bytes: i64,
    pub truncated: bool,
}

#[derive(Debug, Deserialize)]
struct WorkflowFileQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
struct WorkflowFileContextQuery {
    node_execution_id: Option<Uuid>,
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/workflow-runs/{run_id}/file-context", get(file_context))
        .route("/workflow-runs/{run_id}/files/content", get(content))
        .route("/workflow-runs/{run_id}/files/raw", get(raw))
}

async fn file_context(
    State(deployment): State<DeploymentImpl>,
    Path(run_id): Path<Uuid>,
    Query(query): Query<WorkflowFileContextQuery>,
) -> Result<Json<ApiResponse<WorkflowFileContextView>>, ApiError> {
    Ok(Json(ApiResponse::success(
        read_workflow_file_context(&deployment.db().pool, run_id, query.node_execution_id).await?,
    )))
}

async fn open_file(
    deployment: &DeploymentImpl,
    run_id: Uuid,
    relative: &str,
) -> Result<(Uuid, String, tokio::fs::File), ApiError> {
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_optional(&deployment.db().pool)
            .await?
            .ok_or_else(|| ApiError::BadRequest("Workflow Run not found".into()))?;
    let root = runner::workflow_control_root(&deployment.db().pool, workspace_id).await?;
    let path = super::integrations::files::relative_path(relative)?;
    if path.as_os_str().is_empty() {
        return Err(ApiError::BadRequest("Path must point to a file".into()));
    }
    let display_path = path.to_string_lossy().replace('\\', "/");
    let file = tokio::task::spawn_blocking(move || open_under_root(root, path))
        .await
        .map_err(|_| ApiError::BadRequest("Workflow file read interrupted".into()))??;
    Ok((workspace_id, display_path, tokio::fs::File::from_std(file)))
}

fn open_under_root(root: PathBuf, path: PathBuf) -> Result<std::fs::File, ApiError> {
    let root = Dir::open_ambient_dir(root, cap_std::ambient_authority())
        .map_err(|_| ApiError::Conflict("Workflow workspace is unavailable".into()))?;
    // The directory capability prevents parent symlink/junction replacement
    // between validation and opening from granting access outside this root.
    let file = root
        .open(&path)
        .map_err(|_| ApiError::File(FileError::NotFound))?;
    if !file
        .metadata()
        .map_err(|_| ApiError::File(FileError::NotFound))?
        .is_file()
    {
        return Err(ApiError::BadRequest("Path is not a regular file".into()));
    }
    Ok(file.into_std())
}

async fn content(
    State(deployment): State<DeploymentImpl>,
    Path(run_id): Path<Uuid>,
    Query(query): Query<WorkflowFileQuery>,
) -> Result<Json<ApiResponse<WorkflowFileContent>>, ApiError> {
    let (workspace_id, path, file) = open_file(&deployment, run_id, &query.path).await?;
    let metadata = file.metadata().await?;
    let preview = file_content_preview_from_file(&path, file, &metadata).await?;
    let raw_url = format!(
        "/api/workflow-runs/{run_id}/files/raw?{}",
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("path", &path)
            .finish()
    );
    Ok(Json(ApiResponse::success(WorkflowFileContent {
        run_id,
        workspace_id,
        name: path.rsplit('/').next().unwrap_or_default().to_owned(),
        path,
        kind: preview.kind,
        mime_type: preview.mime_type,
        language: preview.language,
        content: preview.content,
        raw_url: Some(raw_url),
        size_bytes: preview.size_bytes,
        truncated: preview.truncated,
    })))
}

async fn raw(
    State(deployment): State<DeploymentImpl>,
    Path(run_id): Path<Uuid>,
    Query(query): Query<WorkflowFileQuery>,
) -> Result<Response, ApiError> {
    let (_, path, file) = open_file(&deployment, run_id, &query.path).await?;
    let metadata = file.metadata().await?;
    let mime = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();
    let (content_type, disposition) = workspace_raw_content_type_and_disposition(&mime);
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if let Some(disposition) = disposition {
        response = response.header(header::CONTENT_DISPOSITION, disposition);
    }
    response
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|error| ApiError::File(FileError::ResponseBuildError(error.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_directory_files_use_a_contained_capability() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("report.md"), "report").unwrap();
        assert!(open_under_root(root.path().into(), "report.md".into()).is_ok());
        assert!(open_under_root(root.path().into(), "../report.md".into()).is_err());
        assert!(open_under_root(root.path().into(), ".".into()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn file_reads_reject_an_escaping_symlink() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.md"), "secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        assert!(open_under_root(root.path().into(), "escape/secret.md".into()).is_err());
    }
}
