use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
};

use axum::{
    Extension, Router,
    body::Body,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{Json as ResponseJson, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use db::models::{
    repo::{Repo, RepoError},
    workspace::Workspace,
    workspace_repo::WorkspaceRepo,
};
use deployment::Deployment;
use mime_guess::MimeGuess;
use serde::{Deserialize, Serialize};
use services::services::{container::ContainerService, file::FileError};
use tokio::{fs, io::AsyncReadExt};
use tokio_util::io::ReaderStream;
use ts_rs::TS;
use url::form_urlencoded::Serializer;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

const DIRECTORY_ENTRY_LIMIT: usize = 2_000;
const TEXT_CONTENT_LIMIT_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum WorkspaceFileKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkspaceFileEntry {
    pub workspace_id: Uuid,
    pub repo_id: Uuid,
    pub repo_name: String,
    pub path: String,
    pub name: String,
    pub kind: WorkspaceFileKind,
    pub size_bytes: Option<i64>,
    pub modified_at: Option<DateTime<Utc>>,
    pub mime_type: Option<String>,
    pub is_binary: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkspaceFileRepoNode {
    pub workspace_id: Uuid,
    pub repo_id: Uuid,
    pub repo_name: String,
    pub repo_display_name: String,
    pub entries: Vec<WorkspaceFileEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkspaceFileTreeResponse {
    pub repos: Vec<WorkspaceFileRepoNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkspaceFileDirectoryResponse {
    pub workspace_id: Uuid,
    pub repo_id: Uuid,
    pub repo_name: String,
    pub path: String,
    pub entries: Vec<WorkspaceFileEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum WorkspaceFileContentKind {
    Text,
    Image,
    Binary,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WorkspaceFileContent {
    pub workspace_id: Uuid,
    pub repo_id: Uuid,
    pub repo_name: String,
    pub path: String,
    pub name: String,
    pub kind: WorkspaceFileContentKind,
    pub mime_type: Option<String>,
    pub language: Option<String>,
    pub content: Option<String>,
    pub raw_url: Option<String>,
    pub size_bytes: i64,
    pub truncated: bool,
}

#[derive(Debug, Deserialize)]
pub struct WorkspaceFileQuery {
    pub repo_id: Uuid,
    pub path: Option<String>,
}

#[derive(Debug, Clone)]
struct NormalizedWorkspacePath {
    fs_path: PathBuf,
    display_path: String,
}

struct ResolvedWorkspaceRepo {
    repo: Repo,
    canonical_repo_root: PathBuf,
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/tree", get(get_workspace_file_tree))
        .route("/directory", get(get_workspace_file_directory))
        .route("/content", get(get_workspace_file_content))
        .route("/raw", get(serve_workspace_file_raw))
}

pub async fn get_workspace_file_tree(
    Extension(workspace): Extension<Workspace>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<WorkspaceFileTreeResponse>>, ApiError> {
    let repos =
        WorkspaceRepo::find_repos_for_workspace(&deployment.db().pool, workspace.id).await?;
    let container_ref = deployment
        .container()
        .ensure_container_exists(&workspace)
        .await?;
    let workspace_dir = PathBuf::from(container_ref);
    let mut repo_nodes = Vec::with_capacity(repos.len());

    for repo in repos {
        let repo_root = workspace_dir.join(&repo.name);
        let canonical_repo_root = fs::canonicalize(&repo_root)
            .await
            .map_err(|_| ApiError::File(FileError::NotFound))?;
        let (entries, truncated) = list_directory_entries(
            &workspace,
            &repo,
            &canonical_repo_root,
            &canonical_repo_root,
            "",
        )
        .await?;

        repo_nodes.push(WorkspaceFileRepoNode {
            workspace_id: workspace.id,
            repo_id: repo.id,
            repo_name: repo.name,
            repo_display_name: repo.display_name,
            entries,
            truncated,
        });
    }

    Ok(ResponseJson(ApiResponse::success(
        WorkspaceFileTreeResponse { repos: repo_nodes },
    )))
}

pub async fn get_workspace_file_directory(
    Extension(workspace): Extension<Workspace>,
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<WorkspaceFileQuery>,
) -> Result<ResponseJson<ApiResponse<WorkspaceFileDirectoryResponse>>, ApiError> {
    let target = normalize_relative_path(query.path.as_deref())?;
    let resolved = resolve_workspace_repo_root(&deployment, &workspace, query.repo_id).await?;
    let directory_path =
        canonical_target_under_root(&resolved.canonical_repo_root, &target).await?;
    let metadata = fs::metadata(&directory_path)
        .await
        .map_err(|_| ApiError::File(FileError::NotFound))?;
    if !metadata.is_dir() {
        return Err(ApiError::BadRequest("Path is not a directory".to_string()));
    }

    let (entries, truncated) = list_directory_entries(
        &workspace,
        &resolved.repo,
        &resolved.canonical_repo_root,
        &directory_path,
        &target.display_path,
    )
    .await?;

    Ok(ResponseJson(ApiResponse::success(
        WorkspaceFileDirectoryResponse {
            workspace_id: workspace.id,
            repo_id: resolved.repo.id,
            repo_name: resolved.repo.name,
            path: target.display_path,
            entries,
            truncated,
        },
    )))
}

pub async fn get_workspace_file_content(
    Extension(workspace): Extension<Workspace>,
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<WorkspaceFileQuery>,
) -> Result<ResponseJson<ApiResponse<WorkspaceFileContent>>, ApiError> {
    let target = normalize_relative_path(query.path.as_deref())?;
    if target.display_path.is_empty() {
        return Err(ApiError::BadRequest(
            "Path must point to a file".to_string(),
        ));
    }

    let resolved = resolve_workspace_repo_root(&deployment, &workspace, query.repo_id).await?;
    let file_path = canonical_target_under_root(&resolved.canonical_repo_root, &target).await?;
    let metadata = fs::metadata(&file_path)
        .await
        .map_err(|_| ApiError::File(FileError::NotFound))?;
    if metadata.is_dir() {
        return Err(ApiError::BadRequest("Path is a directory".to_string()));
    }

    let content = workspace_file_content_from_path(
        workspace.id,
        resolved.repo.id,
        &resolved.repo.name,
        &target.display_path,
        &file_path,
        &metadata,
    )
    .await?;

    Ok(ResponseJson(ApiResponse::success(content)))
}

pub async fn serve_workspace_file_raw(
    Extension(workspace): Extension<Workspace>,
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<WorkspaceFileQuery>,
) -> Result<Response, ApiError> {
    let target = normalize_relative_path(query.path.as_deref())?;
    if target.display_path.is_empty() {
        return Err(ApiError::BadRequest(
            "Path must point to a file".to_string(),
        ));
    }

    let resolved = resolve_workspace_repo_root(&deployment, &workspace, query.repo_id).await?;
    let file_path = canonical_target_under_root(&resolved.canonical_repo_root, &target).await?;
    let file = tokio::fs::File::open(&file_path)
        .await
        .map_err(|_| ApiError::File(FileError::NotFound))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|_| ApiError::File(FileError::NotFound))?;
    if metadata.is_dir() {
        return Err(ApiError::BadRequest("Path is a directory".to_string()));
    }

    let mime_type = mime_type_for_path(&target.display_path)
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let (content_type, content_disposition) =
        workspace_raw_content_type_and_disposition(&mime_type);

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");

    if let Some(content_disposition) = content_disposition {
        response = response.header(header::CONTENT_DISPOSITION, content_disposition);
    }

    response
        .body(body)
        .map_err(|e| ApiError::File(FileError::ResponseBuildError(e.to_string())))
}

async fn resolve_workspace_repo_root(
    deployment: &DeploymentImpl,
    workspace: &Workspace,
    repo_id: Uuid,
) -> Result<ResolvedWorkspaceRepo, ApiError> {
    let pool = &deployment.db().pool;
    let workspace_repo = WorkspaceRepo::find_by_workspace_and_repo_id(pool, workspace.id, repo_id)
        .await?
        .ok_or(RepoError::NotFound)?;
    let repo = Repo::find_by_id(pool, workspace_repo.repo_id)
        .await?
        .ok_or(RepoError::NotFound)?;

    let container_ref = deployment
        .container()
        .ensure_container_exists(workspace)
        .await?;
    let repo_root = PathBuf::from(container_ref).join(&repo.name);
    let canonical_repo_root = fs::canonicalize(&repo_root)
        .await
        .map_err(|_| ApiError::File(FileError::NotFound))?;

    Ok(ResolvedWorkspaceRepo {
        repo,
        canonical_repo_root,
    })
}

fn normalize_relative_path(path: Option<&str>) -> Result<NormalizedWorkspacePath, ApiError> {
    let raw = path.unwrap_or("");
    if raw.contains('\0') {
        return Err(ApiError::BadRequest("Path is invalid".to_string()));
    }

    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/')
        || normalized.starts_with("//")
        || has_windows_drive_prefix(&normalized)
    {
        return Err(ApiError::BadRequest("Path must be relative".to_string()));
    }

    let mut fs_path = PathBuf::new();
    let mut display_parts = Vec::new();

    for part in normalized.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || has_windows_drive_prefix(part) {
            return Err(ApiError::BadRequest(
                "Path must stay inside the repo".to_string(),
            ));
        }

        fs_path.push(part);
        display_parts.push(part.to_string());
    }

    Ok(NormalizedWorkspacePath {
        fs_path,
        display_path: display_parts.join("/"),
    })
}

fn has_windows_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

async fn canonical_target_under_root(
    canonical_repo_root: &Path,
    target: &NormalizedWorkspacePath,
) -> Result<PathBuf, ApiError> {
    let full_path = if target.fs_path.as_os_str().is_empty() {
        canonical_repo_root.to_path_buf()
    } else {
        canonical_repo_root.join(&target.fs_path)
    };
    let canonical_target = fs::canonicalize(full_path)
        .await
        .map_err(|_| ApiError::File(FileError::NotFound))?;

    if !canonical_target.starts_with(canonical_repo_root) {
        return Err(ApiError::File(FileError::NotFound));
    }

    Ok(canonical_target)
}

async fn list_directory_entries(
    workspace: &Workspace,
    repo: &Repo,
    canonical_repo_root: &Path,
    directory_path: &Path,
    parent_display_path: &str,
) -> Result<(Vec<WorkspaceFileEntry>, bool), ApiError> {
    let mut read_dir = fs::read_dir(directory_path)
        .await
        .map_err(|_| ApiError::File(FileError::NotFound))?;
    let mut entries = Vec::new();
    let mut truncated = false;

    while let Some(entry) = read_dir.next_entry().await? {
        if entries.len() >= DIRECTORY_ENTRY_LIMIT {
            truncated = true;
            break;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        let display_path = join_display_path(parent_display_path, &name);
        let entry_path = entry.path();
        let Ok(canonical_entry_path) = fs::canonicalize(&entry_path).await else {
            continue;
        };
        if !canonical_entry_path.starts_with(canonical_repo_root) {
            continue;
        }

        let Ok(metadata) = fs::metadata(&canonical_entry_path).await else {
            continue;
        };
        let kind = if metadata.is_dir() {
            WorkspaceFileKind::Directory
        } else if metadata.is_file() {
            WorkspaceFileKind::File
        } else {
            continue;
        };

        entries.push(workspace_file_entry(
            workspace.id,
            repo,
            &display_path,
            &name,
            kind,
            &metadata,
        ));
    }

    sort_file_entries(&mut entries);
    Ok((entries, truncated))
}

fn workspace_file_entry(
    workspace_id: Uuid,
    repo: &Repo,
    display_path: &str,
    name: &str,
    kind: WorkspaceFileKind,
    metadata: &std::fs::Metadata,
) -> WorkspaceFileEntry {
    let mime_type = if kind == WorkspaceFileKind::File {
        mime_type_for_path(display_path)
    } else {
        None
    };
    let is_binary = if kind == WorkspaceFileKind::File {
        Some(is_likely_binary_path(display_path, mime_type.as_deref()))
    } else {
        None
    };

    WorkspaceFileEntry {
        workspace_id,
        repo_id: repo.id,
        repo_name: repo.name.clone(),
        path: display_path.to_string(),
        name: name.to_string(),
        kind,
        size_bytes: if metadata.is_file() {
            Some(size_to_i64(metadata.len()))
        } else {
            None
        },
        modified_at: metadata.modified().ok().map(DateTime::<Utc>::from),
        mime_type,
        is_binary,
    }
}

async fn workspace_file_content_from_path(
    workspace_id: Uuid,
    repo_id: Uuid,
    repo_name: &str,
    display_path: &str,
    file_path: &Path,
    metadata: &std::fs::Metadata,
) -> Result<WorkspaceFileContent, ApiError> {
    let preview = file_content_preview(display_path, file_path, metadata).await?;
    Ok(WorkspaceFileContent {
        workspace_id,
        repo_id,
        repo_name: repo_name.to_string(),
        path: display_path.to_string(),
        name: file_name_from_display_path(display_path, repo_name),
        kind: preview.kind,
        mime_type: preview.mime_type,
        language: preview.language,
        content: preview.content,
        raw_url: Some(workspace_file_raw_url(workspace_id, repo_id, display_path)),
        size_bytes: preview.size_bytes,
        truncated: preview.truncated,
    })
}

/// Preview bytes have no repository identity; plain-folder workflow files use
/// exactly the same caps and MIME policy as repository previews.
pub(crate) struct FileContentPreview {
    pub kind: WorkspaceFileContentKind,
    pub mime_type: Option<String>,
    pub language: Option<String>,
    pub content: Option<String>,
    pub size_bytes: i64,
    pub truncated: bool,
}

pub(crate) async fn file_content_preview(
    display_path: &str,
    file_path: &Path,
    metadata: &std::fs::Metadata,
) -> Result<FileContentPreview, ApiError> {
    let file = tokio::fs::File::open(file_path)
        .await
        .map_err(|_| ApiError::File(FileError::NotFound))?;
    file_content_preview_from_file(display_path, file, metadata).await
}

pub(crate) async fn file_content_preview_from_file(
    display_path: &str,
    file: tokio::fs::File,
    metadata: &std::fs::Metadata,
) -> Result<FileContentPreview, ApiError> {
    let mime_type = mime_type_for_path(display_path);
    let mut preview = FileContentPreview {
        kind: WorkspaceFileContentKind::Binary,
        mime_type,
        language: None,
        content: None,
        size_bytes: size_to_i64(metadata.len()),
        truncated: false,
    };
    if preview
        .mime_type
        .as_deref()
        .is_some_and(is_safe_inline_workspace_image_mime_type)
    {
        preview.kind = WorkspaceFileContentKind::Image;
        return Ok(preview);
    }
    if preview
        .mime_type
        .as_deref()
        .is_some_and(is_always_unsupported_preview_mime_type)
    {
        preview.kind = WorkspaceFileContentKind::Unsupported;
        return Ok(preview);
    }
    let (bytes, truncated) = read_preview_file_bytes(file).await?;
    if !bytes.contains(&0) {
        if let Some(content) = utf8_preview_string(&bytes) {
            preview.kind = WorkspaceFileContentKind::Text;
            preview.language = language_from_path(display_path);
            preview.content = Some(content);
            preview.truncated = truncated;
        }
    }
    Ok(preview)
}

async fn read_preview_file_bytes(file: tokio::fs::File) -> Result<(Vec<u8>, bool), ApiError> {
    let mut limited = file.take((TEXT_CONTENT_LIMIT_BYTES + 1) as u64);
    let mut bytes = Vec::new();
    limited.read_to_end(&mut bytes).await?;

    let truncated = bytes.len() > TEXT_CONTENT_LIMIT_BYTES;
    if truncated {
        bytes.truncate(TEXT_CONTENT_LIMIT_BYTES);
    }

    Ok((bytes, truncated))
}

fn utf8_preview_string(bytes: &[u8]) -> Option<String> {
    match std::str::from_utf8(bytes) {
        Ok(content) => Some(content.to_string()),
        Err(err) if err.error_len().is_none() && err.valid_up_to() > 0 => {
            Some(String::from_utf8_lossy(&bytes[..err.valid_up_to()]).to_string())
        }
        Err(_) => None,
    }
}

fn sort_file_entries(entries: &mut [WorkspaceFileEntry]) {
    entries.sort_by(|a, b| match (a.kind, b.kind) {
        (WorkspaceFileKind::Directory, WorkspaceFileKind::File) => Ordering::Less,
        (WorkspaceFileKind::File, WorkspaceFileKind::Directory) => Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
}

fn join_display_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn file_name_from_display_path(display_path: &str, fallback: &str) -> String {
    display_path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn mime_type_for_path(path: &str) -> Option<String> {
    MimeGuess::from_path(path).first_raw().map(str::to_string)
}

fn workspace_file_raw_url(workspace_id: Uuid, repo_id: Uuid, path: &str) -> String {
    let query = Serializer::new(String::new())
        .append_pair("repo_id", &repo_id.to_string())
        .append_pair("path", path)
        .finish();
    format!("/api/workspaces/{workspace_id}/files/raw?{query}")
}

pub(crate) fn workspace_raw_content_type_and_disposition(
    mime_type: &str,
) -> (&str, Option<&'static str>) {
    if is_safe_inline_workspace_image_mime_type(mime_type) {
        (mime_type, None)
    } else {
        ("application/octet-stream", Some("attachment"))
    }
}

fn is_safe_inline_workspace_image_mime_type(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/bmp"
            | "image/x-icon"
            | "image/tiff"
    )
}

fn is_always_unsupported_preview_mime_type(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "application/pdf"
            | "application/zip"
            | "application/x-zip-compressed"
            | "application/x-tar"
            | "application/gzip"
            | "application/x-7z-compressed"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/msword"
            | "application/vnd.ms-excel"
            | "application/vnd.ms-powerpoint"
    )
}

fn is_likely_binary_path(path: &str, mime_type: Option<&str>) -> bool {
    if let Some(mime_type) = mime_type {
        if is_textual_mime_type(mime_type) {
            return false;
        }
        if mime_type.starts_with("image/") || is_always_unsupported_preview_mime_type(mime_type) {
            return true;
        }
    }

    !is_likely_text_extension(path)
}

fn is_textual_mime_type(mime_type: &str) -> bool {
    mime_type.starts_with("text/")
        || matches!(
            mime_type,
            "application/json"
                | "application/javascript"
                | "application/typescript"
                | "application/xml"
                | "application/x-sh"
                | "application/x-yaml"
                | "image/svg+xml"
        )
}

fn is_likely_text_extension(path: &str) -> bool {
    extension_lower(path).is_some_and(|ext| language_from_extension(&ext).is_some())
}

fn language_from_path(path: &str) -> Option<String> {
    extension_lower(path).and_then(|ext| language_from_extension(&ext).map(str::to_string))
}

fn extension_lower(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
}

fn language_from_extension(extension: &str) -> Option<&'static str> {
    match extension {
        "bat" => Some("batch"),
        "c" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Some("cpp"),
        "cs" => Some("csharp"),
        "css" => Some("css"),
        "diff" | "patch" => Some("diff"),
        "go" => Some("go"),
        "h" => Some("c"),
        "html" | "htm" => Some("html"),
        "java" => Some("java"),
        "js" | "mjs" | "cjs" | "jsx" => Some("javascript"),
        "json" | "jsonc" => Some("json"),
        "kt" | "kts" => Some("kotlin"),
        "lua" => Some("lua"),
        "md" | "mdx" | "markdown" => Some("markdown"),
        "php" => Some("php"),
        "ps1" => Some("powershell"),
        "py" | "pyw" => Some("python"),
        "rb" => Some("ruby"),
        "rs" => Some("rust"),
        "scss" | "sass" => Some("scss"),
        "sh" | "bash" | "zsh" => Some("shell"),
        "sql" => Some("sql"),
        "svelte" => Some("svelte"),
        "swift" => Some("swift"),
        "toml" => Some("toml"),
        "ts" | "tsx" => Some("typescript"),
        "txt" | "log" => Some("text"),
        "vue" => Some("vue"),
        "xml" => Some("xml"),
        "yaml" | "yml" => Some("yaml"),
        _ => None,
    }
}

fn size_to_i64(size: u64) -> i64 {
    i64::try_from(size).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn normalizes_repo_relative_paths() {
        let normalized = normalize_relative_path(Some("src//lib/./main.rs")).unwrap();
        assert_eq!(normalized.display_path, "src/lib/main.rs");
        assert_eq!(
            normalized.fs_path,
            PathBuf::from("src").join("lib").join("main.rs")
        );
    }

    #[test]
    fn rejects_parent_traversal() {
        assert!(normalize_relative_path(Some("../secret.txt")).is_err());
        assert!(normalize_relative_path(Some("src/../../secret.txt")).is_err());
    }

    #[test]
    fn rejects_absolute_paths() {
        assert!(normalize_relative_path(Some("/etc/passwd")).is_err());
        assert!(normalize_relative_path(Some("\\windows\\system.ini")).is_err());
    }

    #[test]
    fn rejects_windows_drive_paths() {
        assert!(normalize_relative_path(Some("C:\\Users\\me\\secret.txt")).is_err());
        assert!(normalize_relative_path(Some("C:secret.txt")).is_err());
    }

    #[tokio::test]
    async fn canonical_target_rejects_symlink_escape_on_unix() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let temp = tempfile::tempdir().unwrap();
            let repo_root = temp.path().join("repo");
            let outside = temp.path().join("outside");
            fs::create_dir_all(&repo_root).unwrap();
            fs::write(&outside, "secret").unwrap();
            symlink(&outside, repo_root.join("link")).unwrap();

            let canonical_repo_root = tokio::fs::canonicalize(&repo_root).await.unwrap();
            let target = normalize_relative_path(Some("link")).unwrap();
            assert!(
                canonical_target_under_root(&canonical_repo_root, &target)
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn content_truncates_large_text() {
        let temp = tempfile::tempdir().unwrap();
        let file_path = temp.path().join("large.txt");
        fs::write(&file_path, "a".repeat(TEXT_CONTENT_LIMIT_BYTES + 10)).unwrap();
        let metadata = fs::metadata(&file_path).unwrap();

        let content = workspace_file_content_from_path(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "repo",
            "large.txt",
            &file_path,
            &metadata,
        )
        .await
        .unwrap();

        assert_eq!(content.kind, WorkspaceFileContentKind::Text);
        assert!(content.truncated);
        assert_eq!(content.content.unwrap().len(), TEXT_CONTENT_LIMIT_BYTES);
    }

    #[tokio::test]
    async fn binary_content_is_not_inlined() {
        let temp = tempfile::tempdir().unwrap();
        let file_path = temp.path().join("data.bin");
        fs::write(&file_path, [0_u8, 159, 146, 150]).unwrap();
        let metadata = fs::metadata(&file_path).unwrap();

        let content = workspace_file_content_from_path(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "repo",
            "data.bin",
            &file_path,
            &metadata,
        )
        .await
        .unwrap();

        assert_eq!(content.kind, WorkspaceFileContentKind::Binary);
        assert_eq!(content.content, None);
        assert!(content.raw_url.is_some());
    }

    #[tokio::test]
    async fn image_content_uses_raw_url_without_inline_body() {
        let temp = tempfile::tempdir().unwrap();
        let file_path = temp.path().join("image.png");
        fs::write(&file_path, b"not a real png but metadata only").unwrap();
        let metadata = fs::metadata(&file_path).unwrap();

        let content = workspace_file_content_from_path(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "repo",
            "image.png",
            &file_path,
            &metadata,
        )
        .await
        .unwrap();

        assert_eq!(content.kind, WorkspaceFileContentKind::Image);
        assert_eq!(content.mime_type.as_deref(), Some("image/png"));
        assert_eq!(content.content, None);
        assert!(content.raw_url.is_some());
    }

    #[test]
    fn raw_headers_allow_safe_images_inline() {
        let (content_type, disposition) = workspace_raw_content_type_and_disposition("image/png");
        assert_eq!(content_type, "image/png");
        assert_eq!(disposition, None);
    }

    #[test]
    fn raw_headers_force_html_svg_and_unknown_to_attachment() {
        for mime_type in [
            "text/html",
            "image/svg+xml",
            "application/pdf",
            "application/octet-stream",
        ] {
            let (content_type, disposition) = workspace_raw_content_type_and_disposition(mime_type);
            assert_eq!(content_type, "application/octet-stream");
            assert_eq!(disposition, Some("attachment"));
        }
    }
}
