//! Host-owned Git connections and durable, cancellable repository imports.
mod credentials;
mod transport;
pub mod types;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use axum::{
    Json, Router,
    extract::{Path, State},
    response::IntoResponse,
    routing::{get, post},
};
use db::models::repo::Repo;
use deployment::Deployment;
use sqlx::{FromRow, SqlitePool};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
pub use transport::worker_main;
#[cfg(debug_assertions)]
pub(crate) use transport::writer_absent;
use utils::response::ApiResponse;
use uuid::Uuid;

use self::{
    transport::{Control, Progress, RemoteUrl},
    types::*,
};
use crate::{DeploymentImpl, error::ApiError};

pub(super) fn bad(message: impl Into<String>) -> ApiError {
    ApiError::BadRequest(message.into())
}
pub(super) fn conflict(message: impl Into<String>) -> ApiError {
    ApiError::Conflict(message.into())
}
type Controls = Mutex<HashMap<Uuid, Arc<Control>>>;
static CONTROLS: OnceLock<Controls> = OnceLock::new();
static START_GATE: AsyncMutex<()> = AsyncMutex::const_new(());
static OWNER: Mutex<Option<std::fs::File>> = Mutex::new(None);

fn ensure_owner() -> Result<(), ApiError> {
    let mut owner = OWNER
        .lock()
        .map_err(|_| conflict("Git import owner unavailable"))?;
    if owner.is_none() {
        let path = utils::assets::asset_dir().join("git-import-owner.lock");
        if path.exists() && credentials::is_link(&std::fs::symlink_metadata(&path)?) {
            return Err(conflict("Git owner lock must not be a link"));
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        file.try_lock().map_err(|_|conflict("Another Vibe Studio process manages Git imports for this data directory. Use that instance or close it before retrying."))?;
        *owner = Some(file);
    }
    Ok(())
}

async fn require_owner(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if let Err(error) = ensure_owner() {
        return error.into_response();
    }
    next.run(request).await
}

fn controls() -> &'static Controls {
    CONTROLS.get_or_init(Default::default)
}
fn control(id: Uuid) -> Result<Option<Arc<Control>>, ApiError> {
    Ok(controls()
        .lock()
        .map_err(|_| conflict("Import controls unavailable"))?
        .get(&id)
        .cloned())
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route(
            "/git-connections",
            get(list_connections).post(create_connection),
        )
        .route(
            "/git-connections/{id}",
            get(get_connection)
                .put(update_connection)
                .delete(delete_connection),
        )
        .route("/git-connections/{id}/test", post(test_connection))
        .route("/git-imports/inspect", post(inspect))
        .route("/git-imports", get(list_jobs).post(start))
        .route("/git-imports/{id}", get(get_job))
        .route("/git-imports/{id}/cancel", post(cancel))
        .layer(axum::middleware::from_fn(require_owner))
}

async fn list_connections(
    State(deployment): State<DeploymentImpl>,
) -> Result<Json<ApiResponse<Vec<GitConnection>>>, ApiError> {
    Ok(Json(ApiResponse::success(
        credentials::list(&deployment.db().pool).await?,
    )))
}
async fn get_connection(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<GitConnection>>, ApiError> {
    Ok(Json(ApiResponse::success(
        credentials::get(&deployment.db().pool, id).await?,
    )))
}
async fn create_connection(
    State(deployment): State<DeploymentImpl>,
    Json(input): Json<WriteGitConnection>,
) -> Result<Json<ApiResponse<GitConnection>>, ApiError> {
    Ok(Json(ApiResponse::success(
        credentials::save(&deployment.db().pool, Uuid::new_v4(), input, false).await?,
    )))
}
async fn update_connection(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
    Json(input): Json<WriteGitConnection>,
) -> Result<Json<ApiResponse<GitConnection>>, ApiError> {
    Ok(Json(ApiResponse::success(
        credentials::save(&deployment.db().pool, id, input, true).await?,
    )))
}
async fn delete_connection(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    credentials::delete(&deployment.db().pool, id).await?;
    Ok(Json(ApiResponse::success(())))
}

async fn root(deployment: &DeploymentImpl) -> PathBuf {
    deployment
        .config()
        .read()
        .await
        .managed_workspace_root
        .as_ref()
        .map(|path| PathBuf::from(path.trim()))
        .unwrap_or_else(|| utils::assets::asset_dir().join("workspaces"))
}

async fn inspect(
    State(deployment): State<DeploymentImpl>,
    Json(input): Json<InspectGitRemote>,
) -> Result<Json<ApiResponse<GitRemoteInspection>>, ApiError> {
    // Worker owns credentials until the actual transport exits even if the
    // browser disconnects while waiting for branch discovery.
    let result = tokio::spawn(async move { inspect_owned(deployment, input).await })
        .await
        .map_err(|_| conflict("Git inspection worker failed"))??;
    Ok(Json(ApiResponse::success(result)))
}

async fn test_connection(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
    Json(input): Json<TestGitConnection>,
) -> Result<Json<ApiResponse<GitRemoteInspection>>, ApiError> {
    inspect(
        State(deployment),
        Json(InspectGitRemote {
            url: input.url,
            connection_id: Some(id),
        }),
    )
    .await
}

async fn inspect_owned(
    deployment: DeploymentImpl,
    input: InspectGitRemote,
) -> Result<GitRemoteInspection, ApiError> {
    let remote = transport::parse_url(&input.url)?;
    let _lease = input.connection_id.map(credentials::lease).transpose()?;
    let connection = if let Some(id) = input.connection_id {
        let connection = credentials::get(&deployment.db().pool, id).await?;
        transport::pin_connection(&remote, &connection)?;
        Some(connection)
    } else {
        None
    };
    let (branches, default_branch) = if let Some(connection) =
        connection.filter(|c| c.auth_mode == "private_key")
    {
        let secret = credentials::read_secret(&deployment.db().pool, connection.id).await?;
        transport::inspect_imported(remote.clone(), secret, Arc::new(Control::default())).await?
    } else {
        transport::inspect_native(&remote).await?
    };
    Ok(GitRemoteInspection {
        url: remote.url,
        branches,
        default_branch,
        suggested_directory: root(&deployment)
            .await
            .join(format!("{}-{}", remote.name, Uuid::new_v4()))
            .to_string_lossy()
            .into(),
    })
}

#[derive(FromRow)]
struct JobRecord {
    id: Uuid,
    request_id: Uuid,
    request_json: String,
    transport: String,
    writer_pid: Option<i64>,
    url: String,
    connection_id: Option<Uuid>,
    branch: Option<String>,
    directory_path: String,
    state: String,
    phase: String,
    progress: Option<i32>,
    error: Option<String>,
    repo_id: Option<Uuid>,
}

async fn record(pool: &SqlitePool, id: Uuid) -> Result<JobRecord, ApiError> {
    sqlx::query_as("SELECT * FROM git_import_jobs WHERE id=?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| bad("Git import job not found"))
}

async fn project(pool: &SqlitePool, record: JobRecord) -> Result<GitImportJob, ApiError> {
    let repo = if let Some(id) = record.repo_id {
        Repo::find_by_id(pool, id).await?
    } else {
        None
    };
    Ok(GitImportJob {
        id: record.id,
        request_id: record.request_id,
        url: record.url,
        connection_id: record.connection_id,
        branch: record.branch,
        directory_path: record.directory_path,
        state: record.state,
        phase: record.phase,
        progress: record.progress,
        error: record.error,
        repo,
    })
}

async fn read_job(pool: &SqlitePool, id: Uuid) -> Result<GitImportJob, ApiError> {
    let mut job = record(pool, id).await?;
    if matches!(job.state.as_str(), "queued" | "running" | "cancelling") && control(id)?.is_none() {
        let exited = job.writer_pid.is_none()
            || job
                .writer_pid
                .is_some_and(|pid| transport::writer_absent(pid, job.transport == "native"));
        let (state, message) = if exited {
            (
                "failed",
                "Import was interrupted by a server restart. Its previous writer has exited; partial files were retained. Retry in a new directory.",
            )
        } else {
            (
                "cancelling",
                "Import was interrupted by a server restart. The previous native Git process exit cannot yet be confirmed. Files were retained; inspect that process before recovery. This is not a confirmed cancellation.",
            )
        };
        sqlx::query("UPDATE git_import_jobs SET state=?,phase='interrupted',error=? WHERE id=? AND state IN ('queued','running','cancelling')")
            .bind(state).bind(message).bind(id).execute(pool).await?;
        job = record(pool, id).await?;
    }
    project(pool, job).await
}

async fn get_job(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<GitImportJob>>, ApiError> {
    Ok(Json(ApiResponse::success(
        read_job(&deployment.db().pool, id).await?,
    )))
}
async fn list_jobs(
    State(deployment): State<DeploymentImpl>,
) -> Result<Json<ApiResponse<Vec<GitImportJob>>>, ApiError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM git_import_jobs ORDER BY created_at DESC,id DESC LIMIT 50",
    )
    .fetch_all(&deployment.db().pool)
    .await?;
    let mut jobs = Vec::new();
    for id in ids {
        jobs.push(read_job(&deployment.db().pool, id).await?);
    }
    Ok(Json(ApiResponse::success(jobs)))
}

async fn start(
    State(deployment): State<DeploymentImpl>,
    Json(input): Json<StartGitImport>,
) -> Result<Json<ApiResponse<GitImportJob>>, ApiError> {
    let result = tokio::spawn(async move { start_owned(deployment, input).await })
        .await
        .map_err(|_| conflict("Git start worker failed"))??;
    Ok(Json(ApiResponse::success(result)))
}

async fn existing_request(
    pool: &SqlitePool,
    request_id: Uuid,
    request_json: &str,
) -> Result<Option<Uuid>, ApiError> {
    let existing: Option<JobRecord> =
        sqlx::query_as("SELECT * FROM git_import_jobs WHERE request_id=?")
            .bind(request_id)
            .fetch_optional(pool)
            .await?;
    match existing {
        Some(existing) if existing.request_json == request_json => Ok(Some(existing.id)),
        Some(_) => Err(conflict(
            "Import request ID was already used for a different request",
        )),
        None => Ok(None),
    }
}

async fn start_owned(
    deployment: DeploymentImpl,
    input: StartGitImport,
) -> Result<GitImportJob, ApiError> {
    let _start = START_GATE.lock().await;
    let remote = transport::parse_url(&input.url)?;
    transport::validate_branch(input.branch.as_deref())?;
    let request_json =
        serde_json::to_string(&input).map_err(|_| bad("Invalid Git import request"))?;
    if let Some(id) =
        existing_request(&deployment.db().pool, input.request_id, &request_json).await?
    {
        return read_job(&deployment.db().pool, id).await;
    }
    let lease = input.connection_id.map(credentials::lease).transpose()?;
    let connection = if let Some(id) = input.connection_id {
        let connection = credentials::get(&deployment.db().pool, id).await?;
        transport::pin_connection(&remote, &connection)?;
        Some(connection)
    } else {
        None
    };
    let secret = if let Some(connection) = connection.filter(|c| c.auth_mode == "private_key") {
        Some(credentials::read_secret(&deployment.db().pool, connection.id).await?)
    } else {
        None
    };
    let path = transport::reserve_directory(
        &root(&deployment).await,
        &remote,
        input.directory_path.as_deref(),
    )
    .await?;
    let id = Uuid::new_v4();
    let control = Arc::new(Control::default());
    controls()
        .lock()
        .map_err(|_| conflict("Import controls unavailable"))?
        .insert(id, control.clone());
    let insertion=sqlx::query("INSERT INTO git_import_jobs(id,request_id,request_json,transport,url,connection_id,branch,directory_path,state,phase) VALUES(?,?,?,?,?,?,?,?,'queued','preparing')")
        .bind(id).bind(input.request_id).bind(request_json).bind(if secret.is_some(){"imported"}else{"native"}).bind(&remote.url).bind(input.connection_id).bind(&input.branch).bind(path.to_string_lossy().as_ref()).execute(&deployment.db().pool).await;
    if let Err(error) = insertion {
        if let Ok(mut controls) = controls().lock() {
            controls.remove(&id);
        }
        return Err(conflict(format!(
            "Import could not be recorded. Empty reserved directory retained at {}: {error}",
            path.display()
        )));
    }
    let pool = deployment.db().pool.clone();
    tokio::spawn(async move {
        let _lease = lease;
        let result = run_import(&deployment, id, remote, input.branch, path, secret, control).await;
        if let Err(error) = result {
            // Keep an accurate failure and retained path; never log credentials
            // or raw remote diagnostics.
            let _=sqlx::query("UPDATE git_import_jobs SET state='failed',phase='error',error=? WHERE id=? AND state IN ('queued','running','cancelling')").bind(error.to_string()).bind(id).execute(&deployment.db().pool).await;
        }
        if let Ok(mut controls) = controls().lock() {
            controls.remove(&id);
        }
    });
    project(&pool, record(&pool, id).await?).await
}

async fn run_import(
    deployment: &DeploymentImpl,
    id: Uuid,
    remote: RemoteUrl,
    branch: Option<String>,
    path: PathBuf,
    secret: Option<credentials::Secret>,
    control: Arc<Control>,
) -> Result<(), ApiError> {
    let pool = &deployment.db().pool;
    transport::verify_target(&path).await?;
    sqlx::query("UPDATE git_import_jobs SET state=CASE WHEN state='cancelling' THEN state ELSE 'running' END,phase='connecting' WHERE id=?").bind(id).execute(pool).await?;
    let (sender, mut receiver) = mpsc::channel::<Progress>(8);
    let operation = async {
        if let Some(secret) = secret {
            transport::clone_imported(
                remote,
                secret,
                branch,
                path.clone(),
                control.clone(),
                sender,
                (pool.clone(), id),
            )
            .await
        } else {
            transport::clone_native(
                &remote,
                branch.as_deref(),
                &path,
                control.clone(),
                sender,
                (pool.clone(), id),
            )
            .await
        }
    };
    tokio::pin!(operation);
    let mut projection_error = None;
    let result = loop {
        tokio::select! {
            result=&mut operation=>break result,
            Some(progress)=receiver.recv()=>{
                if projection_error.is_none() {
                    if let Err(error)=sqlx::query("UPDATE git_import_jobs SET phase=?,progress=?,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=? AND state IN ('running','cancelling')")
                        .bind(progress.phase).bind(progress.percent).bind(id).execute(pool).await {
                        projection_error=Some(ApiError::from(error));
                        control.request_cancel();
                    }
                }
            }
        }
    };
    let _publication = control.publication.lock().await;
    if let Some(error) = projection_error {
        return Err(error);
    }
    if control.user_cancelled() {
        sqlx::query("UPDATE git_import_jobs SET state='cancelled',phase='cancelled',error='Import cancelled; partial files retained' WHERE id=?").bind(id).execute(pool).await?;
        return Ok(());
    }
    result?;
    transport::verify_target(&path).await?;
    let repo = deployment
        .repo()
        .register(pool, path.to_string_lossy().as_ref(), None)
        .await?;
    sqlx::query("UPDATE git_import_jobs SET state='succeeded',phase='complete',progress=100,error=NULL,repo_id=?,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?")
        .bind(repo.id).bind(id).execute(pool).await?;
    Ok(())
}

async fn cancel(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<GitImportJob>>, ApiError> {
    if let Some(control) = control(id)? {
        let _publication = control.publication.lock().await;
        let job = record(&deployment.db().pool, id).await?;
        if matches!(job.state.as_str(), "queued" | "running" | "cancelling") {
            sqlx::query(
                "UPDATE git_import_jobs SET state='cancelling',phase='stopping' WHERE id=?",
            )
            .bind(id)
            .execute(&deployment.db().pool)
            .await?;
            control.request_user_cancel();
        }
    }
    Ok(Json(ApiResponse::success(
        read_job(&deployment.db().pool, id).await?,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn repeated_request_and_restart_recovery_preserve_writer_truth() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE repos(id BLOB PRIMARY KEY);")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../../db/migrations/20260910000000_git_imports.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let request = Uuid::new_v4();
        let id = Uuid::new_v4();
        assert!(
            existing_request(&pool, request, "{}")
                .await
                .unwrap()
                .is_none()
        );
        sqlx::query("INSERT INTO git_import_jobs(id,request_id,request_json,transport,url,directory_path,state,phase) VALUES(?,?,'{}','imported','git@example.invalid:repo','test-only-path','queued','preparing')")
            .bind(id).bind(request).execute(&pool).await.unwrap();
        assert_eq!(
            existing_request(&pool, request, "{}").await.unwrap(),
            Some(id)
        );
        assert!(existing_request(&pool, request, "different").await.is_err());
        let never_spawned = read_job(&pool, id).await.unwrap();
        assert_eq!(never_spawned.state, "failed");
        for pid in [-1, i64::from(std::process::id())] {
            sqlx::query("UPDATE git_import_jobs SET state='running',writer_pid=? WHERE id=?")
                .bind(pid)
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            let unconfirmed = read_job(&pool, id).await.unwrap();
            assert_eq!(unconfirmed.state, "cancelling");
            assert!(
                unconfirmed
                    .error
                    .unwrap()
                    .contains("not a confirmed cancellation")
            );
        }
        sqlx::query("UPDATE git_import_jobs SET state='succeeded',phase='complete' WHERE id=?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(read_job(&pool, id).await.unwrap().state, "succeeded");
    }
}
