use std::path::PathBuf;

use axum::{
    Router,
    extract::{Query, State},
    response::Json as ResponseJson,
    routing::{get, post},
};
use db::models::repo::Repo;
use deployment::Deployment;
use serde::{Deserialize, Serialize};
use services::services::filesystem::{DirectoryEntry, DirectoryListResponse, FilesystemError};
use ts_rs::TS;
use utils::response::ApiResponse;

use crate::{DeploymentImpl, error::ApiError};

#[cfg(windows)]
mod windows_picker;

#[derive(Debug, Deserialize)]
pub struct ListDirectoryQuery {
    path: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
pub struct PickFolderRequest {
    pub initial_path: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
pub struct InspectDirectoryRequest {
    pub path: String,
}

#[derive(Debug, Serialize, TS)]
pub struct DirectoryInspection {
    pub path: String,
    pub is_git_repo: bool,
    pub repo: Option<Repo>,
    pub current_branch: Option<String>,
}

pub(crate) async fn validate_directory_path(
    deployment: &DeploymentImpl,
    raw_path: &str,
) -> Result<PathBuf, ApiError> {
    let raw_path = raw_path.trim();
    if raw_path.is_empty() {
        return Err(ApiError::BadRequest(
            "A directory path is required.".to_string(),
        ));
    }

    let normalized_path = deployment.repo().normalize_path(raw_path).map_err(|e| {
        ApiError::BadRequest(format!("Invalid directory path '{}': {}", raw_path, e))
    })?;

    let metadata = tokio::fs::metadata(&normalized_path).await.map_err(|e| {
        ApiError::BadRequest(format!(
            "Directory path is not accessible: {} ({})",
            normalized_path.display(),
            e
        ))
    })?;

    if !metadata.is_dir() {
        return Err(ApiError::BadRequest(format!(
            "Directory path is not a directory: {}",
            normalized_path.display()
        )));
    }

    let _ = tokio::fs::read_dir(&normalized_path).await.map_err(|e| {
        ApiError::BadRequest(format!(
            "Directory path is not readable: {} ({})",
            normalized_path.display(),
            e
        ))
    })?;

    Ok(normalized_path)
}

pub async fn pick_folder(
    ResponseJson(payload): ResponseJson<PickFolderRequest>,
) -> ResponseJson<ApiResponse<Option<String>>> {
    let mut dialog = rfd::AsyncFileDialog::new();

    // The browser owns the foreground window, not this HTTP server. Without an
    // explicit owner Windows can place the native picker behind the browser.
    #[cfg(windows)]
    let parent = windows_picker::ForegroundOwner::capture();
    #[cfg(windows)]
    if let Some(parent) = &parent {
        dialog = dialog.set_parent(parent);
    }

    if let Some(title) = payload
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        dialog = dialog.set_title(title);
    }

    if let Some(initial_path) = payload
        .initial_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        let path = PathBuf::from(initial_path);
        if path.is_dir() {
            dialog = dialog.set_directory(path);
        }
    }

    let selected_path = dialog
        .pick_folder()
        .await
        .map(|handle| handle.path().to_string_lossy().to_string());

    ResponseJson(ApiResponse::success(selected_path))
}

pub async fn inspect_directory(
    State(deployment): State<DeploymentImpl>,
    ResponseJson(payload): ResponseJson<InspectDirectoryRequest>,
) -> Result<ResponseJson<ApiResponse<DirectoryInspection>>, ApiError> {
    let normalized_path = validate_directory_path(&deployment, &payload.path).await?;
    let normalized_path_string = normalized_path.to_string_lossy().to_string();
    let is_git_repo = normalized_path.join(".git").exists()
        && deployment.git().is_repo_openable(&normalized_path);

    let (repo, current_branch) = if is_git_repo {
        let current_branch = deployment.git().get_current_branch(&normalized_path).ok();
        let repo = deployment
            .repo()
            .register(&deployment.db().pool, &normalized_path_string, None)
            .await?;
        (Some(repo), current_branch)
    } else {
        (None, None)
    };

    Ok(ResponseJson(ApiResponse::success(DirectoryInspection {
        path: normalized_path_string,
        is_git_repo,
        repo,
        current_branch,
    })))
}

pub async fn list_directory(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ListDirectoryQuery>,
) -> Result<ResponseJson<ApiResponse<DirectoryListResponse>>, ApiError> {
    match deployment.filesystem().list_directory(query.path).await {
        Ok(response) => Ok(ResponseJson(ApiResponse::success(response))),
        Err(FilesystemError::DirectoryDoesNotExist) => {
            Ok(ResponseJson(ApiResponse::error("Directory does not exist")))
        }
        Err(FilesystemError::PathIsNotDirectory) => {
            Ok(ResponseJson(ApiResponse::error("Path is not a directory")))
        }
        Err(FilesystemError::Io(e)) => {
            tracing::error!("Failed to read directory: {}", e);
            Ok(ResponseJson(ApiResponse::error(&format!(
                "Failed to read directory: {}",
                e
            ))))
        }
    }
}

pub async fn list_git_repos(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ListDirectoryQuery>,
) -> Result<ResponseJson<ApiResponse<Vec<DirectoryEntry>>>, ApiError> {
    let res = if let Some(ref path) = query.path {
        deployment
            .filesystem()
            .list_git_repos(Some(path.clone()), 800, 1200, Some(3))
            .await
    } else {
        deployment
            .filesystem()
            .list_common_git_repos(800, 1200, Some(4))
            .await
    };
    match res {
        Ok(response) => Ok(ResponseJson(ApiResponse::success(response))),
        Err(FilesystemError::DirectoryDoesNotExist) => {
            Ok(ResponseJson(ApiResponse::error("Directory does not exist")))
        }
        Err(FilesystemError::PathIsNotDirectory) => {
            Ok(ResponseJson(ApiResponse::error("Path is not a directory")))
        }
        Err(FilesystemError::Io(e)) => {
            tracing::error!("Failed to read directory: {}", e);
            Ok(ResponseJson(ApiResponse::error(&format!(
                "Failed to read directory: {}",
                e
            ))))
        }
    }
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/filesystem/directory", get(list_directory))
        .route("/filesystem/pick-folder", post(pick_folder))
        .route("/filesystem/inspect-directory", post(inspect_directory))
        .route("/filesystem/git-repos", get(list_git_repos))
}
