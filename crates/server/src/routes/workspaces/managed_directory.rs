//! Explicit ownership of auto-created standalone-session directories.
//! External paths never acquire ownership merely because they are below a root.
use std::path::{Path, PathBuf};

use axum::{
    Json,
    extract::{Extension, State},
};
use db::models::{
    requests::{SessionDeletionInfo, SessionDeletionResult},
    session::Session,
    task::Execution,
    workspace::Workspace,
};
use deployment::Deployment;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection, SqlitePool};
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

const MARKER: &str = ".vibe-kanban-managed.json";

#[derive(Debug, FromRow)]
struct Ownership {
    workspace_id: Uuid,
    root_path: String,
    directory_path: String,
    ownership_token: String,
    state: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Marker {
    workspace_id: Uuid,
    ownership_token: String,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Conflict(message.into())
}

pub(crate) fn retained_creation_error(
    workspace_id: Uuid,
    directory_path: &str,
    error: ApiError,
) -> ApiError {
    invalid(format!(
        "Session start failed: {error}. Workspace {workspace_id} and its directory were retained at {directory_path}. Retrying new-session creation allocates another directory."
    ))
}

pub(crate) fn validate_root_setting(value: Option<&str>) -> Result<(), String> {
    if let Some(value) = value {
        if value.trim().is_empty() || !Path::new(value.trim()).is_absolute() {
            return Err("Managed workspace root must be an absolute directory path, or null to use the default.".into());
        }
    }
    Ok(())
}

pub(crate) async fn create(
    deployment: &DeploymentImpl,
    name: Option<String>,
) -> Result<Workspace, ApiError> {
    let setting = deployment
        .config()
        .read()
        .await
        .managed_workspace_root
        .clone();
    validate_root_setting(setting.as_deref()).map_err(ApiError::BadRequest)?;
    let root = setting
        .map(|path| PathBuf::from(path.trim()))
        .unwrap_or_else(|| utils::assets::asset_dir().join("workspaces"));
    tokio::fs::create_dir_all(&root).await?;
    let root = tokio::fs::canonicalize(root).await?;
    let workspace_id = Uuid::new_v4();
    let directory = root.join(workspace_id.to_string());
    // Never reuse an existing directory, even if it appears empty.
    tokio::fs::create_dir(&directory).await?;
    let marker = Marker {
        workspace_id,
        ownership_token: Uuid::new_v4().to_string(),
    };
    let marker_path = directory.join(MARKER);
    let creation = async {
        use tokio::io::AsyncWriteExt;
        let mut marker_file = tokio::fs::OpenOptions::new().create_new(true).write(true).open(&marker_path).await?;
        marker_file.write_all(&serde_json::to_vec(&marker).map_err(|error| invalid(error.to_string()))?).await?;
        marker_file.flush().await?;
        marker_file.sync_all().await?;
        let mut transaction = deployment.db().pool.begin().await?;
        // Broad External protection intentionally excludes automatic worktree
        // expiry/cleanup. Only the narrow ownership registry grants deletion.
        let workspace = sqlx::query_as::<_, Workspace>(
            "INSERT INTO workspaces (id, container_ref, workspace_kind, container_ownership, branch, name) VALUES (?, ?, 'direct_folder', 'external', 'direct-folder', ?) RETURNING *"
        ).bind(workspace_id).bind(directory.to_string_lossy().as_ref()).bind(name)
            .fetch_one(&mut *transaction).await?;
        sqlx::query("INSERT INTO managed_workspace_directories (workspace_id, root_path, directory_path, ownership_token) VALUES (?, ?, ?, ?)")
            .bind(workspace_id).bind(root.to_string_lossy().as_ref())
            .bind(directory.to_string_lossy().as_ref()).bind(&marker.ownership_token)
            .execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok::<_, ApiError>(workspace)
    }.await;
    if creation.is_err() {
        // No recursive compensation. Preserve unexpected files on failure.
        let _ = tokio::fs::remove_file(marker_path).await;
        let _ = tokio::fs::remove_dir(directory).await;
    }
    creation
}

async fn ownership(
    connection: &mut SqliteConnection,
    session_id: Uuid,
) -> Result<Option<Ownership>, ApiError> {
    Ok(sqlx::query_as::<_, Ownership>(
        "SELECT m.workspace_id, m.root_path, m.directory_path, m.ownership_token, m.state FROM managed_workspace_directories m JOIN sessions s ON s.workspace_id = m.workspace_id WHERE s.id = ?"
    ).bind(session_id).fetch_optional(connection).await?)
}

async fn ensure_unshared(
    connection: &mut SqliteConnection,
    owner: &Ownership,
    session_id: Uuid,
) -> Result<(), ApiError> {
    let others: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE workspace_id = ? AND id != ?)",
    )
    .bind(owner.workspace_id)
    .bind(session_id)
    .fetch_one(&mut *connection)
    .await?;
    if others {
        return Err(invalid(
            "Other sessions share this directory; only the selected session can be deleted.",
        ));
    }
    // A separately attached external workspace also counts as a consumer.
    let others: Vec<String> = sqlx::query_scalar("SELECT container_ref FROM workspaces WHERE id != ? AND container_ref IS NOT NULL UNION SELECT r.path FROM workspace_repos wr JOIN repos r ON r.id = wr.repo_id WHERE wr.workspace_id != ?")
        .bind(owner.workspace_id).bind(owner.workspace_id).fetch_all(&mut *connection).await?;
    // An interrupted deletion may already have moved the files. Protect both
    // durable identities so another workspace attached to recovery files also
    // prevents removal. Full marker/path validation remains required separately.
    let targets = [
        PathBuf::from(&owner.directory_path),
        quarantine_path(owner)?,
    ];
    for other in others {
        let candidate = PathBuf::from(other);
        let candidate = tokio::fs::canonicalize(&candidate)
            .await
            .unwrap_or(candidate);
        if targets.iter().any(|target| {
            candidate == *target || candidate.starts_with(target) || target.starts_with(&candidate)
        }) {
            return Err(invalid(
                "Another workspace references this directory; files were retained.",
            ));
        }
    }
    Ok(())
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn quarantine_path(owner: &Ownership) -> Result<PathBuf, ApiError> {
    let token = Uuid::parse_str(&owner.ownership_token)
        .map_err(|_| invalid("Invalid managed directory ownership token."))?;
    // Deterministic from durable ownership, so an interrupted rename is
    // recoverable even if SQLite rolls back quarantine_path on process exit.
    Ok(Path::new(&owner.root_path).join(format!(".{}-deleted-{token}", owner.workspace_id)))
}

async fn path_present(path: &Path) -> Result<bool, std::io::Error> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

async fn directory_location(owner: &Ownership) -> Result<PathBuf, ApiError> {
    let original = PathBuf::from(&owner.directory_path);
    let path = if path_present(&original).await? {
        original
    } else {
        quarantine_path(owner)?
    };
    verify_directory(owner, &path).await?;
    Ok(path)
}

async fn verify_directory(owner: &Ownership, path: &Path) -> Result<(), ApiError> {
    let root = Path::new(&owner.root_path);
    if !root.is_absolute() || !path.is_absolute() || path.parent() != Some(root) {
        return Err(invalid(
            "Managed directory ownership path is invalid; files were retained.",
        ));
    }
    if Path::new(&owner.directory_path)
        .file_name()
        .and_then(|name| name.to_str())
        != Some(owner.workspace_id.to_string().as_str())
    {
        return Err(invalid(
            "Managed directory must be the exact allocated UUID child.",
        ));
    }
    if tokio::fs::canonicalize(root).await? != root || tokio::fs::canonicalize(path).await? != path
    {
        return Err(invalid(
            "Managed directory path changed; files were retained.",
        ));
    }
    let metadata = tokio::fs::symlink_metadata(path).await?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(invalid("Managed directory is a link or not a directory."));
    }
    let marker_path = path.join(MARKER);
    let metadata = tokio::fs::symlink_metadata(&marker_path).await?;
    if !metadata.is_file() || is_link(&metadata) || metadata.len() > 4096 {
        return Err(invalid("Managed directory ownership marker is invalid."));
    }
    let actual: Marker = serde_json::from_slice(&tokio::fs::read(marker_path).await?)
        .map_err(|error| invalid(error.to_string()))?;
    if actual
        != (Marker {
            workspace_id: owner.workspace_id,
            ownership_token: owner.ownership_token.clone(),
        })
    {
        return Err(invalid(
            "Managed directory ownership marker does not match; files were retained.",
        ));
    }
    Ok(())
}

pub(crate) async fn preflight_files(
    deployment: &DeploymentImpl,
    session_id: Uuid,
    requested: bool,
) -> Result<(), ApiError> {
    if !requested {
        return Ok(());
    }
    let mut connection = deployment.db().pool.acquire().await?;
    let owner = ownership(&mut connection, session_id)
        .await?
        .ok_or_else(|| invalid("External directory files cannot be deleted."))?;
    if owner.state != "active" {
        return Err(invalid("Managed directory ownership is not active."));
    }
    ensure_unshared(&mut connection, &owner, session_id).await?;
    directory_location(&owner).await?;
    Ok(())
}

pub(crate) async fn deletion_info(
    State(deployment): State<DeploymentImpl>,
    Extension(session): Extension<Session>,
) -> Result<Json<ApiResponse<SessionDeletionInfo>>, ApiError> {
    let mut connection = deployment.db().pool.acquire().await?;
    let owner = ownership(&mut connection, session.id).await?;
    let mut info = SessionDeletionInfo {
        can_delete_managed_files: false,
        managed_directory_path: None,
    };
    if let Some(owner) = owner {
        info.managed_directory_path = Some(owner.directory_path.clone());
        if let Ok(location) = directory_location(&owner).await {
            info.managed_directory_path = Some(location.to_string_lossy().into());
            info.can_delete_managed_files = owner.state == "active"
                && ensure_unshared(&mut connection, &owner, session.id)
                    .await
                    .is_ok();
        }
    }
    Ok(Json(ApiResponse::success(info)))
}

/// Queue gate and safe stop preflight are owned by the caller. Database guards
/// repeat under BEGIN IMMEDIATE; directory rename is reversible until commit.
pub(crate) async fn delete(
    deployment: &DeploymentImpl,
    session_id: Uuid,
    task_id: Option<Uuid>,
    delete_files: bool,
) -> Result<SessionDeletionResult, ApiError> {
    let pool = &deployment.db().pool;
    delete_with_pool(pool, session_id, task_id, delete_files).await
}

async fn delete_with_pool(
    pool: &SqlitePool,
    session_id: Uuid,
    task_id: Option<Uuid>,
    delete_files: bool,
) -> Result<SessionDeletionResult, ApiError> {
    let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
    let retained_warning = if !delete_files {
        if let Some(owner) = ownership(&mut transaction, session_id).await? {
            directory_location(&owner).await.ok()
                .filter(|location| location != Path::new(&owner.directory_path))
                .map(|location| format!("Session deleted; files from an interrupted deletion were retained at {} (original path {}).", location.display(), owner.directory_path))
        } else {
            None
        }
    } else {
        None
    };
    let owner = if delete_files {
        let owner = ownership(&mut transaction, session_id)
            .await?
            .ok_or_else(|| {
                invalid("This is an external directory; its files cannot be deleted.")
            })?;
        if owner.state != "active"
            || Path::new(&owner.directory_path)
                .file_name()
                .and_then(|name| name.to_str())
                != Some(owner.workspace_id.to_string().as_str())
        {
            return Err(invalid("Managed directory ownership is no longer active."));
        }
        ensure_unshared(&mut transaction, &owner, session_id).await?;
        directory_location(&owner).await?;
        Some(owner)
    } else {
        None
    };
    if let Some(task_id) = task_id {
        Execution::delete_agent_with_session_in_transaction(&mut transaction, task_id, session_id)
            .await?;
    } else {
        Session::delete_in_transaction(&mut transaction, session_id).await?;
    }
    let quarantine = if let Some(owner) = &owner {
        let quarantine = quarantine_path(owner)?;
        sqlx::query("UPDATE managed_workspace_directories SET state = 'removed', quarantine_path = ? WHERE workspace_id = ?")
            .bind(quarantine.to_string_lossy().as_ref()).bind(owner.workspace_id).execute(&mut *transaction).await?;
        let location = directory_location(owner).await?;
        if location != quarantine {
            if path_present(&quarantine).await? {
                return Err(invalid(format!(
                    "A previous deletion directory exists at {}; files were retained.",
                    quarantine.display()
                )));
            }
            tokio::fs::rename(&location, &quarantine).await.map_err(|error| {
                invalid(format!(
                    "Unable to move the working directory at {} for deletion: {error}. The session, its history and files were retained. Close applications using this directory and retry, or delete the session without removing its files.",
                    location.display()
                ))
            })?;
        }
        Some(quarantine)
    } else {
        None
    };
    if let Err(error) = transaction.commit().await {
        if let (Some(owner), Some(quarantine)) = (&owner, &quarantine) {
            if path_present(Path::new(&owner.directory_path))
                .await
                .unwrap_or(true)
                || tokio::fs::rename(quarantine, &owner.directory_path)
                    .await
                    .is_err()
            {
                return Err(invalid(format!(
                    "Deletion failed; directory retained at {}. Original path {} could not be restored: {error}",
                    quarantine.display(),
                    owner.directory_path
                )));
            }
        }
        return Err(ApiError::Database(error));
    }
    let mut result = SessionDeletionResult {
        warning: retained_warning,
    };
    if let (Some(owner), Some(quarantine)) = (owner, quarantine) {
        let cleanup = async {
            verify_directory(&owner, &quarantine).await?;
            // std/tokio remove_dir_all does not follow child symlinks. The
            // root itself was checked for symlinks and Windows reparse points.
            tokio::fs::remove_dir_all(&quarantine).await?;
            Ok::<_, ApiError>(())
        }
        .await;
        if let Err(error) = cleanup {
            result.warning = Some(format!(
                "Session deleted, but some files were retained at {}: {error}",
                quarantine.display()
            ));
        } else {
            let _ = sqlx::query("UPDATE managed_workspace_directories SET quarantine_path = NULL WHERE workspace_id = ?")
                .bind(owner.workspace_id).execute(pool).await;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DeletionFixture {
        _temporary: tempfile::TempDir,
        pool: SqlitePool,
        owner: Ownership,
        session_id: Uuid,
        sibling_session_id: Uuid,
    }

    impl DeletionFixture {
        async fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let root = tokio::fs::canonicalize(temporary.path()).await.unwrap();
            let workspace_id = Uuid::new_v4();
            let directory = root.join(workspace_id.to_string());
            tokio::fs::create_dir(&directory).await.unwrap();
            let owner = Ownership {
                workspace_id,
                root_path: root.to_string_lossy().into(),
                directory_path: directory.to_string_lossy().into(),
                ownership_token: Uuid::new_v4().to_string(),
                state: "active".into(),
            };
            tokio::fs::write(
                directory.join(MARKER),
                serde_json::to_vec(&Marker {
                    workspace_id,
                    ownership_token: owner.ownership_token.clone(),
                })
                .unwrap(),
            )
            .await
            .unwrap();
            tokio::fs::write(directory.join("work.txt"), "retained work")
                .await
                .unwrap();

            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();
            sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
            let session_id = Uuid::new_v4();
            let sibling_session_id = Uuid::new_v4();
            let sibling_workspace_id = Uuid::new_v4();
            for (workspace, session, path) in [
                (workspace_id, session_id, Some(&owner.directory_path)),
                (sibling_workspace_id, sibling_session_id, None),
            ] {
                sqlx::query("INSERT INTO workspaces (id, branch, workspace_kind, container_ownership, container_ref) VALUES (?, 'direct-folder', 'direct_folder', 'external', ?)")
                    .bind(workspace).bind(path).execute(&pool).await.unwrap();
                sqlx::query("INSERT INTO sessions (id, workspace_id) VALUES (?, ?)")
                    .bind(session)
                    .bind(workspace)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            sqlx::query("INSERT INTO managed_workspace_directories (workspace_id, root_path, directory_path, ownership_token) VALUES (?, ?, ?, ?)")
                .bind(workspace_id).bind(&owner.root_path).bind(&owner.directory_path)
                .bind(&owner.ownership_token).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO scratch (id, scratch_type, payload) VALUES (?, 'draft_follow_up', '{}')")
                .bind(session_id).execute(&pool).await.unwrap();
            let run_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO agent_runs (id, session_id, workspace_id, request_id, idempotency_key, correlation_id, schema_version, payload_version, runtime_profile_id, provider_id, workspace_mode, workspace_path, status, request_envelope) VALUES (?, ?, ?, ?, ?, ?, 1, 1, 'test-profile', 'codex', 'shared_workspace', ?, 'succeeded', '{}')",
            )
            .bind(run_id).bind(session_id).bind(workspace_id).bind(Uuid::new_v4())
            .bind(run_id.to_string()).bind(Uuid::new_v4()).bind(&owner.directory_path)
            .execute(&pool).await.unwrap();
            sqlx::query(
                "INSERT INTO agent_conversation_sessions (session_id, epoch) VALUES (?, ?)",
            )
            .bind(session_id)
            .bind(Uuid::new_v4())
            .execute(&pool)
            .await
            .unwrap();
            Self {
                _temporary: temporary,
                pool,
                owner,
                session_id,
                sibling_session_id,
            }
        }

        async fn assert_records(&self, present: bool) {
            for (table, column) in [
                ("sessions", "id"),
                ("scratch", "id"),
                ("agent_runs", "session_id"),
                ("agent_conversation_sessions", "session_id"),
            ] {
                let count: i64 =
                    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?"))
                        .bind(self.session_id)
                        .fetch_one(&self.pool)
                        .await
                        .unwrap();
                assert_eq!(count, i64::from(present), "{table}");
            }
            assert!(
                Session::find_by_id(&self.pool, self.sibling_session_id)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert!(
                sqlx::query("PRAGMA foreign_key_check")
                    .fetch_all(&self.pool)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn managed_session_deletion_respects_the_file_choice() {
        for delete_files in [false, true] {
            let fixture = DeletionFixture::new().await;
            let result = delete_with_pool(&fixture.pool, fixture.session_id, None, delete_files)
                .await
                .unwrap();
            assert!(result.warning.is_none());
            fixture.assert_records(false).await;
            assert_eq!(
                Path::new(&fixture.owner.directory_path).exists(),
                !delete_files
            );
            assert!(!quarantine_path(&fixture.owner).unwrap().exists());
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn dropping_process_group_releases_directory_even_after_launcher_exit() {
        use std::{process::Stdio, time::Duration};

        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use utils::command_ext::GroupSpawnNoWindowExt;

        for launcher_exited in [false, true] {
            let fixture = DeletionFixture::new().await;
            let mut command = tokio::process::Command::new("powershell.exe");
            command
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    r#"$ErrorActionPreference = 'Stop'
$child = Start-Process -FilePath (Join-Path $PSHOME 'powershell.exe') -ArgumentList '-NoLogo', '-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 30' -WorkingDirectory (Get-Location).Path -PassThru -WindowStyle Hidden
[Console]::Out.WriteLine($child.Id)
[Console]::In.ReadLine() | Out-Null"#,
                ])
                .current_dir(&fixture.owner.directory_path)
                .kill_on_drop(true)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let mut child = command.group_spawn_no_window().unwrap();
            let mut stdout = BufReader::new(child.inner().stdout.take().unwrap());
            let mut descendant_pid = String::new();
            tokio::time::timeout(
                Duration::from_secs(10),
                stdout.read_line(&mut descendant_pid),
            )
            .await
            .expect("launcher must report its descendant before cleanup")
            .unwrap();
            assert!(descendant_pid.trim().parse::<u32>().unwrap() > 0);

            if launcher_exited {
                let mut stdin = child.inner().stdin.take().unwrap();
                stdin.write_all(b"\n").await.unwrap();
                drop(stdin);
                // The launcher is gone but its descendant still owns the cwd.
                // Waiting only for the launcher must not release job ownership.
                assert!(
                    tokio::time::timeout(Duration::from_secs(10), child.inner().wait())
                        .await
                        .unwrap()
                        .unwrap()
                        .success()
                );
            }
            assert!(matches!(
                delete_with_pool(&fixture.pool, fixture.session_id, None, true).await,
                Err(ApiError::Conflict(_))
            ));
            fixture.assert_records(true).await;

            // This is also the cancellation/drop path of native CLI discovery.
            drop(child);
            drop(stdout);
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match delete_with_pool(&fixture.pool, fixture.session_id, None, true).await {
                        Ok(_) => break,
                        Err(ApiError::Conflict(_)) => {
                            tokio::time::sleep(Duration::from_millis(25)).await;
                        }
                        Err(error) => panic!("unexpected deletion failure: {error}"),
                    }
                }
            })
            .await
            .expect("dropping the job must terminate descendants and release their cwd");
            fixture.assert_records(false).await;
            assert!(!Path::new(&fixture.owner.directory_path).exists());
            assert!(!quarantine_path(&fixture.owner).unwrap().exists());
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn locked_directory_keeps_session_and_history_until_retry() {
        use std::os::windows::fs::OpenOptionsExt;

        for delete_files in [true, false] {
            let fixture = DeletionFixture::new().await;
            // Reproduce an application retaining this directory as its cwd:
            // READ/WRITE sharing does not permit the quarantine rename.
            let directory_handle = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0x1 | 0x2)
                .custom_flags(0x02000000) // FILE_FLAG_BACKUP_SEMANTICS
                .open(&fixture.owner.directory_path)
                .unwrap();
            let result =
                delete_with_pool(&fixture.pool, fixture.session_id, None, delete_files).await;
            if delete_files {
                let error = result.unwrap_err();
                assert!(matches!(error, ApiError::Conflict(_)), "{error}");
                let message = error.to_string();
                assert!(message.contains("session, its history and files were retained"));
                assert!(message.contains("Close applications"));
                fixture.assert_records(true).await;
                let state: (String, Option<String>) = sqlx::query_as(
                    "SELECT state, quarantine_path FROM managed_workspace_directories WHERE workspace_id = ?",
                )
                .bind(fixture.owner.workspace_id).fetch_one(&fixture.pool).await.unwrap();
                assert_eq!(state, ("active".into(), None));
                assert!(
                    Path::new(&fixture.owner.directory_path)
                        .join("work.txt")
                        .exists()
                );
                assert!(!quarantine_path(&fixture.owner).unwrap().exists());
                drop(directory_handle);
                delete_with_pool(&fixture.pool, fixture.session_id, None, true)
                    .await
                    .unwrap();
            } else {
                // Keeping files never needs the directory rename, even while
                // another application has the directory open.
                assert!(result.unwrap().warning.is_none());
                drop(directory_handle);
            }
            fixture.assert_records(false).await;
            assert_eq!(
                Path::new(&fixture.owner.directory_path).exists(),
                !delete_files
            );
            assert!(!quarantine_path(&fixture.owner).unwrap().exists());
        }
    }

    #[test]
    fn creation_failure_reports_retained_directory_and_workspace() {
        let workspace_id = Uuid::new_v4();
        let error = retained_creation_error(
            workspace_id,
            "retained-directory",
            ApiError::BadRequest("launch rejected".into()),
        )
        .to_string();
        assert!(error.contains(&workspace_id.to_string()));
        assert!(error.contains("retained-directory"));
        assert!(error.contains("launch rejected"));
        assert!(error.contains("allocates another directory"));
    }

    #[test]
    fn root_setting_requires_absolute_path_and_null_means_default() {
        assert!(validate_root_setting(None).is_ok());
        assert!(validate_root_setting(Some("")).is_err());
        assert!(validate_root_setting(Some("relative/path")).is_err());
        assert!(validate_root_setting(Some(std::env::temp_dir().to_str().unwrap())).is_ok());
    }

    #[tokio::test]
    async fn marker_and_exact_parent_are_required_before_deletion() {
        let temporary = tempfile::tempdir().unwrap();
        let root = tokio::fs::canonicalize(temporary.path()).await.unwrap();
        let id = Uuid::new_v4();
        let directory = root.join(id.to_string());
        tokio::fs::create_dir(&directory).await.unwrap();
        let mut owner = Ownership {
            workspace_id: id,
            root_path: root.to_string_lossy().into(),
            directory_path: directory.to_string_lossy().into(),
            ownership_token: "test-token".into(),
            state: "active".into(),
        };
        assert!(verify_directory(&owner, &directory).await.is_err());
        tokio::fs::write(
            directory.join(MARKER),
            serde_json::to_vec(&Marker {
                workspace_id: id,
                ownership_token: "test-token".into(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
        verify_directory(&owner, &directory).await.unwrap();
        owner.ownership_token = "wrong-token".into();
        assert!(verify_directory(&owner, &directory).await.is_err());
        owner.ownership_token = "test-token".into();
        owner.root_path = directory.to_string_lossy().into();
        assert!(verify_directory(&owner, &directory).await.is_err());
    }

    #[tokio::test]
    async fn interrupted_quarantine_remains_discoverable_without_committed_path_update() {
        let temporary = tempfile::tempdir().unwrap();
        let root = tokio::fs::canonicalize(temporary.path()).await.unwrap();
        let id = Uuid::new_v4();
        let directory = root.join(id.to_string());
        tokio::fs::create_dir(&directory).await.unwrap();
        let owner = Ownership {
            workspace_id: id,
            root_path: root.to_string_lossy().into(),
            directory_path: directory.to_string_lossy().into(),
            ownership_token: Uuid::new_v4().to_string(),
            state: "active".into(),
        };
        tokio::fs::write(
            directory.join(MARKER),
            serde_json::to_vec(&Marker {
                workspace_id: id,
                ownership_token: owner.ownership_token.clone(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
        tokio::fs::write(directory.join("keep.txt"), "user work")
            .await
            .unwrap();
        let quarantine = quarantine_path(&owner).unwrap();
        tokio::fs::rename(&directory, &quarantine).await.unwrap();
        assert_eq!(directory_location(&owner).await.unwrap(), quarantine);
        assert_eq!(
            tokio::fs::read_to_string(quarantine.join("keep.txt"))
                .await
                .unwrap(),
            "user work"
        );
    }

    #[tokio::test]
    async fn removed_directory_rejects_new_and_rebound_sessions() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE workspaces(id BLOB PRIMARY KEY); CREATE TABLE sessions(id BLOB PRIMARY KEY, workspace_id BLOB);")
            .execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../../../db/migrations/20260909000000_managed_workspace_directories.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let workspace = Uuid::new_v4();
        let session = Uuid::new_v4();
        sqlx::query("INSERT INTO workspaces VALUES (?)")
            .bind(workspace)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO managed_workspace_directories (workspace_id, root_path, directory_path, ownership_token, state) VALUES (?, 'root', 'child', 'token', 'removed')")
            .bind(workspace).execute(&pool).await.unwrap();
        let insert = sqlx::query("INSERT INTO sessions VALUES (?, ?)")
            .bind(session)
            .bind(workspace)
            .execute(&pool)
            .await;
        assert!(
            insert
                .unwrap_err()
                .to_string()
                .contains("Managed session directory was removed")
        );
        sqlx::query("INSERT INTO sessions VALUES (?, NULL)")
            .bind(session)
            .execute(&pool)
            .await
            .unwrap();
        let update = sqlx::query("UPDATE sessions SET workspace_id = ? WHERE id = ?")
            .bind(workspace)
            .bind(session)
            .execute(&pool)
            .await;
        assert!(
            update
                .unwrap_err()
                .to_string()
                .contains("Managed session directory was removed")
        );
        let retained: Option<Uuid> =
            sqlx::query_scalar("SELECT workspace_id FROM sessions WHERE id = ?")
                .bind(session)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(retained.is_none());
    }

    #[tokio::test]
    async fn ownership_never_infers_external_paths_and_rejects_shared_aliases() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE workspaces(id BLOB PRIMARY KEY, container_ref TEXT); CREATE TABLE sessions(id BLOB PRIMARY KEY, workspace_id BLOB); CREATE TABLE repos(id BLOB PRIMARY KEY, path TEXT); CREATE TABLE workspace_repos(workspace_id BLOB, repo_id BLOB);")
            .execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../../../db/migrations/20260909000000_managed_workspace_directories.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let root = tokio::fs::canonicalize(temporary.path()).await.unwrap();
        let workspace = Uuid::new_v4();
        let session = Uuid::new_v4();
        let target = root.join(workspace.to_string());
        sqlx::query("INSERT INTO workspaces VALUES (?, ?)")
            .bind(workspace)
            .bind(target.to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sessions VALUES (?, ?)")
            .bind(session)
            .bind(workspace)
            .execute(&pool)
            .await
            .unwrap();
        let mut connection = pool.acquire().await.unwrap();
        assert!(ownership(&mut connection, session).await.unwrap().is_none());
        let owner = Ownership {
            workspace_id: workspace,
            root_path: root.to_string_lossy().into(),
            directory_path: target.to_string_lossy().into(),
            ownership_token: Uuid::new_v4().to_string(),
            state: "active".into(),
        };
        ensure_unshared(&mut connection, &owner, session)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sessions VALUES (?, ?)")
            .bind(Uuid::new_v4())
            .bind(workspace)
            .execute(&mut *connection)
            .await
            .unwrap();
        assert!(
            ensure_unshared(&mut connection, &owner, session)
                .await
                .is_err()
        );
        sqlx::query("DELETE FROM sessions WHERE id != ?")
            .bind(session)
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workspaces VALUES (?, ?)")
            .bind(Uuid::new_v4())
            .bind(root.to_string_lossy().as_ref())
            .execute(&mut *connection)
            .await
            .unwrap();
        assert!(
            ensure_unshared(&mut connection, &owner, session)
                .await
                .is_err()
        );
        sqlx::query("DELETE FROM workspaces WHERE id != ?")
            .bind(workspace)
            .execute(&mut *connection)
            .await
            .unwrap();
        let quarantine = quarantine_path(&owner).unwrap();
        tokio::fs::create_dir(&target).await.unwrap();
        tokio::fs::rename(&target, &quarantine).await.unwrap();
        // A separate workspace can attach to files retained by an interrupted
        // rename. The original path no longer exists, but removal is not safe.
        sqlx::query("INSERT INTO workspaces VALUES (?, ?)")
            .bind(Uuid::new_v4())
            .bind(quarantine.to_string_lossy().as_ref())
            .execute(&mut *connection)
            .await
            .unwrap();
        assert!(
            ensure_unshared(&mut connection, &owner, session)
                .await
                .is_err()
        );
    }
}
