//! Single authority for portable project business data.
//!
//! The old SQLite rows are a derived FK/search projection, never a read fallback.
//! All writers hold BEGIN IMMEDIATE until atomic publication, including workflow
//! acceptance. Publishing is staged so rejected runtime transactions do not save
//! a task. If SQLite commit fails afterwards, the file remains authoritative and
//! reopening repairs the projection; it is never overwritten with stale SQL.
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

use api_types::{
    CreateProjectRequest, CreateTaskRequest, Project, ProjectStatus, Tag, Task, TaskAssignee,
    TaskFollower, TaskRelationship, TaskTag,
};
use chrono::Utc;
use db::models::{
    project::DEFAULT_PROJECT_ID,
    scratch::{ProjectRepoDefaultsData, ScratchPayload},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{SqliteConnection, SqlitePool};
use uuid::Uuid;

use crate::error::ApiError;

const FORMAT_VERSION: u32 = 1;
const MAX_BYTES: u64 = 32 * 1024 * 1024;
const METADATA: &str = ".vibe-kanban";
const DOCUMENT: &str = "project.json";

pub(crate) fn workflow_task_id(namespace: &str, request_id: &str) -> Uuid {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(
        format!(
            "vibe-kanban:workflow-task:{}:{namespace}:{request_id}",
            namespace.len()
        )
        .as_bytes(),
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectDocument {
    pub schema_version: u32,
    pub revision: i64,
    pub project: Project,
    pub statuses: Vec<ProjectStatus>,
    pub tags: Vec<Tag>,
    pub tasks: Vec<Task>,
    pub task_tags: Vec<TaskTag>,
    pub task_assignees: Vec<TaskAssignee>,
    pub task_followers: Vec<TaskFollower>,
    pub task_relationships: Vec<TaskRelationship>,
}

fn invalid(message: impl std::fmt::Display) -> ApiError {
    ApiError::Conflict(format!("PROJECT_DATA_INVALID: {message}"))
}

impl ProjectDocument {
    pub fn new(request: CreateProjectRequest) -> Self {
        let id = request.id.unwrap_or_else(Uuid::new_v4);
        let now = Utc::now();
        Self {
            schema_version: FORMAT_VERSION,
            revision: 1,
            project: Project {
                id,
                organization_id: Uuid::from_u128(2),
                name: request.name,
                color: request.color,
                sort_order: 0,
                created_at: now,
                updated_at: now,
            },
            statuses: super::local_remote::DEFAULT_STATUSES
                .into_iter()
                .map(|(name, color, sort_order, hidden)| ProjectStatus {
                    id: Uuid::new_v4(),
                    project_id: id,
                    name: name.into(),
                    color: color.into(),
                    sort_order,
                    hidden,
                    created_at: now,
                })
                .collect(),
            tags: super::local_remote::DEFAULT_TAGS
                .into_iter()
                .map(|(name, color)| Tag {
                    id: Uuid::new_v4(),
                    project_id: id,
                    name: name.into(),
                    color: color.into(),
                })
                .collect(),
            tasks: vec![],
            task_tags: vec![],
            task_assignees: vec![],
            task_followers: vec![],
            task_relationships: vec![],
        }
    }

    pub fn validate(&self) -> Result<(), ApiError> {
        if self.schema_version != FORMAT_VERSION || self.revision < 1 {
            return Err(invalid("Unsupported schema version or revision"));
        }
        if self.project.id == DEFAULT_PROJECT_ID
            || self.project.id.is_nil()
            || self.project.name.trim().is_empty()
        {
            return Err(invalid("Invalid portable project identity"));
        }
        let mut ids = HashSet::new();
        for id in self
            .statuses
            .iter()
            .map(|s| s.id)
            .chain(self.tags.iter().map(|s| s.id))
            .chain(self.tasks.iter().map(|s| s.id))
            .chain(self.task_tags.iter().map(|s| s.id))
            .chain(self.task_assignees.iter().map(|s| s.id))
            .chain(self.task_followers.iter().map(|s| s.id))
            .chain(self.task_relationships.iter().map(|s| s.id))
        {
            if id.is_nil() || !ids.insert(id) {
                return Err(invalid("Duplicate or empty record identity"));
            }
        }
        if self
            .statuses
            .iter()
            .any(|s| s.project_id != self.project.id)
            || self.tags.iter().any(|s| s.project_id != self.project.id)
        {
            return Err(invalid("Cross-project metadata"));
        }
        let tasks: HashSet<_> = self.tasks.iter().map(|t| t.id).collect();
        let statuses: HashSet<_> = self.statuses.iter().map(|s| s.id).collect();
        let tags: HashSet<_> = self.tags.iter().map(|t| t.id).collect();
        let mut numbers = HashSet::new();
        for task in &self.tasks {
            if task.project_id != self.project.id
                || !statuses.contains(&task.status_id)
                || task.title.trim().is_empty()
                || !task.sort_order.is_finite()
                || task.parent_issue_sort_order.is_some_and(|v| !v.is_finite())
                || task.issue_number < 1
                || !numbers.insert(task.issue_number)
            {
                return Err(invalid("Invalid task identity, status, title or ordering"));
            }
            let mut parent = task.parent_issue_id;
            let mut visited = HashSet::from([task.id]);
            while let Some(id) = parent {
                if !visited.insert(id) {
                    return Err(invalid("Task parent cycle"));
                }
                parent = self
                    .tasks
                    .iter()
                    .find(|t| t.id == id)
                    .ok_or_else(|| invalid("Unknown task parent"))?
                    .parent_issue_id;
            }
        }
        if self
            .task_tags
            .iter()
            .any(|r| !tasks.contains(&r.issue_id) || !tags.contains(&r.tag_id))
            || self
                .task_assignees
                .iter()
                .any(|r| !tasks.contains(&r.issue_id))
            || self
                .task_followers
                .iter()
                .any(|r| !tasks.contains(&r.issue_id))
            || self.task_relationships.iter().any(|r| {
                !tasks.contains(&r.issue_id)
                    || !tasks.contains(&r.related_issue_id)
                    || r.issue_id == r.related_issue_id
            })
        {
            return Err(invalid("Unknown or cross-project task relationship"));
        }
        Ok(())
    }

    pub fn insert_task(&mut self, request: CreateTaskRequest) -> Result<Uuid, ApiError> {
        let id = request.id.unwrap_or_else(Uuid::new_v4);
        // Stable IDs allow recovery when the file published but SQL commit failed.
        if let Some(existing) = self.tasks.iter().find(|t| t.id == id) {
            if existing.project_id == request.project_id
                && existing.title == request.title
                && existing.description == request.description
                && existing.status_id == request.status_id
                && existing.priority == request.priority
                && existing.start_date == request.start_date
                && existing.target_date == request.target_date
                && existing.completed_at == request.completed_at
                && existing.sort_order == request.sort_order
                && existing.parent_issue_id == request.parent_issue_id
                && existing.parent_issue_sort_order == request.parent_issue_sort_order
                && existing.extension_metadata == request.extension_metadata
            {
                return Ok(id);
            }
            return Err(invalid("Task ID already exists with different content"));
        }
        let issue_number = self
            .tasks
            .iter()
            .map(|t| t.issue_number)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| invalid("Task number overflow"))?;
        let now = Utc::now();
        self.tasks.push(Task {
            id,
            project_id: request.project_id,
            issue_number,
            simple_id: format!("TASK-{issue_number}"),
            status_id: request.status_id,
            title: request.title,
            description: request.description,
            priority: request.priority,
            start_date: request.start_date,
            target_date: request.target_date,
            completed_at: request.completed_at,
            sort_order: request.sort_order,
            parent_issue_id: request.parent_issue_id,
            parent_issue_sort_order: request.parent_issue_sort_order,
            extension_metadata: request.extension_metadata,
            creator_user_id: None,
            created_at: now,
            updated_at: now,
        });
        Ok(id)
    }
}

/// Keep metadata below the canonical project root; never follow metadata links.
fn metadata_path(root: &Path, create: bool) -> Result<PathBuf, ApiError> {
    let path = root.join(METADATA);
    if create {
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    let info = std::fs::symlink_metadata(&path)?;
    if !info.is_dir()
        || info.file_type().is_symlink()
        || std::fs::canonicalize(&path)?.parent() != Some(root)
    {
        return Err(invalid("Metadata directory is not inside the project"));
    }
    let target = path.join(DOCUMENT);
    match std::fs::symlink_metadata(&target) {
        Ok(info) if !info.is_file() || info.file_type().is_symlink() => {
            return Err(invalid("Project document must be a regular file"));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(target)
}

fn read_path(path: &Path) -> Result<ProjectDocument, ApiError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ApiError::PayloadTooLarge);
    }
    let document: ProjectDocument = serde_json::from_slice(&bytes).map_err(invalid)?;
    document.validate()?;
    Ok(document)
}

struct ReadSnapshot {
    version: (Option<SystemTime>, u64),
    document: Arc<ProjectDocument>,
}

fn read_snapshots() -> &'static Mutex<HashMap<PathBuf, ReadSnapshot>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, ReadSnapshot>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Pure authority read for monitoring. Deliberately does not adopt a project,
/// start a write transaction, or repair its derived SQL business projection.
pub(crate) async fn read_snapshot(
    pool: &SqlitePool,
    project_id: Uuid,
    force: bool,
) -> Result<Arc<ProjectDocument>, ApiError> {
    let root: Option<String> =
        sqlx::query_scalar("SELECT directory_path FROM project_task_mounts WHERE project_id=?")
            .bind(project_id)
            .fetch_optional(pool)
            .await?;
    let root = root.ok_or_else(|| invalid("Project metadata is not mounted on this host"))?;
    tokio::task::spawn_blocking(move || {
        let root = std::fs::canonicalize(root)?;
        let path = metadata_path(&root, false)?;
        let metadata = std::fs::metadata(&path)?;
        let version = (metadata.modified().ok(), metadata.len());
        if !force
            && let Some(cached) = read_snapshots()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&path)
            && cached.version == version
            && cached.document.project.id == project_id
        {
            return Ok(cached.document.clone());
        }
        let document = Arc::new(read_path(&path)?);
        if document.project.id != project_id {
            return Err(invalid(
                "Project identity does not match its registered directory",
            ));
        }
        let mut cache = read_snapshots().lock().unwrap_or_else(|e| e.into_inner());
        // Documents can be large. Keep only a small number of reusable authority reads.
        if cache.len() >= 32 && !cache.contains_key(&path) {
            cache.clear();
        }
        cache.insert(
            path,
            ReadSnapshot {
                version,
                document: document.clone(),
            },
        );
        Ok(document)
    })
    .await
    .map_err(|e| ApiError::Conflict(format!("Project metadata read interrupted: {e}")))?
}

pub(crate) struct PreparedDocument {
    temporary: tempfile::NamedTempFile,
    destination: PathBuf,
    expected_revision: Option<i64>,
}

impl PreparedDocument {
    pub fn publish(self) -> Result<(), ApiError> {
        let cache_path = self.destination.clone();
        if let Some(revision) = self.expected_revision {
            if read_path(&self.destination)?.revision != revision {
                return Err(invalid("Project changed during this operation; retry"));
            }
            self.temporary
                .persist(&self.destination)
                .map_err(|e| ApiError::Io(e.error))?;
        } else {
            self.temporary
                .persist_noclobber(&self.destination)
                .map_err(|e| ApiError::Io(e.error))?;
        }
        read_snapshots()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&cache_path);
        Ok(())
    }
}

fn stage(
    root: &Path,
    document: &ProjectDocument,
    expected_revision: Option<i64>,
) -> Result<PreparedDocument, ApiError> {
    document.validate()?;
    let destination = metadata_path(root, true)?;
    let mut temporary = tempfile::NamedTempFile::new_in(
        destination
            .parent()
            .ok_or_else(|| invalid("Missing metadata directory"))?,
    )?;
    let bytes = serde_json::to_vec_pretty(document).map_err(invalid)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ApiError::PayloadTooLarge);
    }
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    Ok(PreparedDocument {
        temporary,
        destination,
        expected_revision,
    })
}

pub(crate) async fn load_in(
    conn: &mut SqliteConnection,
    project_id: Uuid,
) -> Result<(PathBuf, ProjectDocument), ApiError> {
    let path: Option<String> =
        sqlx::query_scalar("SELECT directory_path FROM project_task_mounts WHERE project_id=?")
            .bind(project_id)
            .fetch_optional(&mut *conn)
            .await?;
    let root = if let Some(path) = path {
        tokio::fs::canonicalize(path).await?
    } else {
        return adopt_in(conn, project_id).await;
    };
    let document = read_path(&metadata_path(&root, false)?)?;
    if document.project.id != project_id {
        return Err(invalid(
            "Project identity does not match its registered directory",
        ));
    }
    Ok((root, document))
}

/// Exactly-once adoption of pre-file-store projects. A registered mount never
/// enters this path, even if its file was deleted or corrupted.
async fn adopt_in(
    conn: &mut SqliteConnection,
    project_id: Uuid,
) -> Result<(PathBuf, ProjectDocument), ApiError> {
    if project_id == DEFAULT_PROJECT_ID {
        return Err(invalid("Default project has no portable task directory"));
    }
    let payload: Option<String> = sqlx::query_scalar(
        "SELECT payload FROM scratch WHERE id=? AND scratch_type='PROJECT_REPO_DEFAULTS'",
    )
    .bind(project_id)
    .fetch_optional(&mut *conn)
    .await?;
    let defaults = payload
        .map(|raw| serde_json::from_str::<ScratchPayload>(&raw))
        .transpose()
        .map_err(invalid)?
        .and_then(|p| match p {
            ScratchPayload::ProjectRepoDefaults(d) => Some(d),
            _ => None,
        });
    let path = if let Some(path) = defaults
        .as_ref()
        .and_then(|d| d.directory_path.as_deref())
        .filter(|p| !p.trim().is_empty())
    {
        PathBuf::from(path)
    } else {
        let paths:Vec<String>=sqlx::query_scalar("SELECT r.path FROM repos r JOIN project_repos p ON p.repo_id=r.id WHERE p.project_id=?").bind(project_id).fetch_all(&mut *conn).await?;
        let path = if paths.len() == 1 {
            paths.into_iter().next()
        } else if paths.is_empty() && defaults.as_ref().is_some_and(|d| d.repos.len() == 1) {
            sqlx::query_scalar("SELECT path FROM repos WHERE id=?")
                .bind(defaults.as_ref().unwrap().repos[0].repo_id)
                .fetch_optional(&mut *conn)
                .await?
        } else {
            None
        };
        PathBuf::from(path.ok_or_else(|| {
            ApiError::Conflict(
                "PROJECT_SPACE_REQUIRED: Open this project's directory before managing its tasks"
                    .into(),
            )
        })?)
    };
    if !path.is_absolute() {
        return Err(invalid("Project directory must be absolute"));
    }
    let root = tokio::fs::canonicalize(path).await?;
    let target = metadata_path(&root, true)?;
    let exists = target.try_exists()?;
    let document = if exists {
        read_path(&target)?
    } else {
        legacy_in(conn, project_id).await?
    };
    if document.project.id != project_id {
        return Err(ApiError::Conflict(
            "Directory contains another project; open it using its existing identity".into(),
        ));
    }
    register_in(conn, &root, &document).await?;
    project_in(conn, &document).await?;
    if !exists {
        stage(&root, &document, None)?.publish()?;
    }
    Ok((root, document))
}

async fn legacy_in(
    conn: &mut SqliteConnection,
    project_id: Uuid,
) -> Result<ProjectDocument, ApiError> {
    use super::local_remote::*;
    let row=sqlx::query("SELECT p.*,COALESCE(m.organization_id,X'00000000000000000000000000000002') AS organization_id,COALESCE(m.color,'210 80% 52%') AS color,COALESCE(m.sort_order,0) AS sort_order FROM projects p LEFT JOIN local_project_metadata m ON m.project_id=p.id WHERE p.id=?").bind(project_id).fetch_one(&mut *conn).await?;
    let project = project_from_row(&row)?;
    let statuses = sqlx::query("SELECT * FROM local_project_statuses WHERE project_id=?")
        .bind(project_id)
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(status_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let tags = sqlx::query("SELECT * FROM local_tags WHERE project_id=?")
        .bind(project_id)
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(tag_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let tasks = sqlx::query("SELECT * FROM local_issues WHERE project_id=?")
        .bind(project_id)
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(issue_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let task_tags=sqlx::query("SELECT r.* FROM local_issue_tags r JOIN local_issues i ON i.id=r.issue_id WHERE i.project_id=?").bind(project_id).fetch_all(&mut *conn).await?.iter().map(issue_tag_from_row).collect::<Result<Vec<_>,_>>()?;
    let task_assignees=sqlx::query("SELECT r.* FROM local_issue_assignees r JOIN local_issues i ON i.id=r.issue_id WHERE i.project_id=?").bind(project_id).fetch_all(&mut *conn).await?.iter().map(issue_assignee_from_row).collect::<Result<Vec<_>,_>>()?;
    let task_followers=sqlx::query("SELECT r.* FROM local_issue_followers r JOIN local_issues i ON i.id=r.issue_id WHERE i.project_id=?").bind(project_id).fetch_all(&mut *conn).await?.iter().map(issue_follower_from_row).collect::<Result<Vec<_>,_>>()?;
    let task_relationships=sqlx::query("SELECT r.* FROM local_issue_relationships r JOIN local_issues i ON i.id=r.issue_id WHERE i.project_id=?").bind(project_id).fetch_all(&mut *conn).await?.iter().map(issue_relationship_from_row).collect::<Result<Vec<_>,_>>()?;
    let document = ProjectDocument {
        schema_version: FORMAT_VERSION,
        revision: 1,
        project,
        statuses,
        tags,
        tasks,
        task_tags,
        task_assignees,
        task_followers,
        task_relationships,
    };
    document.validate()?;
    Ok(document)
}

pub(crate) async fn read(pool: &SqlitePool, project_id: Uuid) -> Result<ProjectDocument, ApiError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let (_, document) = load_in(&mut tx, project_id).await?;
    project_in(&mut tx, &document).await?;
    tx.commit().await?;
    Ok(document)
}

/// New work must resolve its business task from the file, never just its FK index.
/// Historical runs and cancellation deliberately do not use this guard.
pub(crate) async fn require_task(
    pool: &SqlitePool,
    project_id: Uuid,
    task_id: Uuid,
) -> Result<Task, ApiError> {
    read(pool, project_id)
        .await?
        .tasks
        .into_iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| ApiError::BadRequest("Task is not available in this project".into()))
}

pub(crate) async fn validate_directory_binding(
    pool: &SqlitePool,
    project_id: Uuid,
    defaults: &ProjectRepoDefaultsData,
) -> Result<(), ApiError> {
    let registered: Option<String> =
        sqlx::query_scalar("SELECT directory_path FROM project_task_mounts WHERE project_id=?")
            .bind(project_id)
            .fetch_optional(pool)
            .await?;
    let Some(registered) = registered else {
        return Ok(());
    };
    let path = if let Some(path) = defaults
        .directory_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
    {
        path.to_owned()
    } else if defaults.repos.len() == 1 {
        sqlx::query_scalar("SELECT path FROM repos WHERE id=?")
            .bind(defaults.repos[0].repo_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| invalid("Project repository is unavailable"))?
    } else {
        return Err(ApiError::Conflict(
            "A file-owned project must retain its registered directory".into(),
        ));
    };
    if tokio::fs::canonicalize(path).await? != PathBuf::from(registered) {
        return Err(ApiError::Conflict(
            "Open the new directory as a project instead of changing this project's data location"
                .into(),
        ));
    }
    Ok(())
}

pub(crate) async fn mutate<T>(
    pool: &SqlitePool,
    project_id: Uuid,
    change: impl FnOnce(&mut ProjectDocument) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let (root, mut document) = load_in(&mut tx, project_id).await?;
    let revision = document.revision;
    let result = change(&mut document)?;
    document.revision = revision
        .checked_add(1)
        .ok_or_else(|| invalid("Revision overflow"))?;
    document.project.updated_at = Utc::now();
    document.validate()?;
    project_in(&mut tx, &document).await?;
    stage(&root, &document, Some(revision))?.publish()?;
    tx.commit().await?;
    Ok(result)
}

pub(crate) async fn insert_task_in(
    conn: &mut SqliteConnection,
    request: CreateTaskRequest,
) -> Result<(Uuid, PreparedDocument), ApiError> {
    let (root, mut document) = load_in(conn, request.project_id).await?;
    let revision = document.revision;
    let id = document.insert_task(request)?;
    document.revision = document
        .revision
        .checked_add(1)
        .ok_or_else(|| invalid("Revision overflow"))?;
    document.validate()?;
    project_in(conn, &document).await?;
    Ok((id, stage(&root, &document, Some(revision))?))
}

pub(crate) async fn owner(pool: &SqlitePool, table: &str, id: Uuid) -> Result<Uuid, ApiError> {
    // SQL is used for identity routing only. Content always comes from the file.
    let query = match table {
        "tasks" => "SELECT project_id FROM local_issues WHERE id=?",
        "statuses" => "SELECT project_id FROM local_project_statuses WHERE id=?",
        "tags" => "SELECT project_id FROM local_tags WHERE id=?",
        "task_tags" => {
            "SELECT i.project_id FROM local_issue_tags r JOIN local_issues i ON i.id=r.issue_id WHERE r.id=?"
        }
        "task_assignees" => {
            "SELECT i.project_id FROM local_issue_assignees r JOIN local_issues i ON i.id=r.issue_id WHERE r.id=?"
        }
        "task_followers" => {
            "SELECT i.project_id FROM local_issue_followers r JOIN local_issues i ON i.id=r.issue_id WHERE r.id=?"
        }
        "task_relationships" => {
            "SELECT i.project_id FROM local_issue_relationships r JOIN local_issues i ON i.id=r.issue_id WHERE r.id=?"
        }
        _ => return Err(invalid("Unknown task collection")),
    };
    sqlx::query_scalar(query)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Record not found".into()))
}

pub(crate) async fn open(
    pool: &SqlitePool,
    directory: &str,
    request: CreateProjectRequest,
) -> Result<Project, ApiError> {
    let path = Path::new(directory);
    if !path.is_absolute() {
        return Err(ApiError::BadRequest(
            "Select an absolute project directory".into(),
        ));
    }
    let root = tokio::fs::canonicalize(path).await?;
    if !root.is_dir() {
        return Err(ApiError::BadRequest("Select a project directory".into()));
    }
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let target = metadata_path(&root, true)?;
    let existing = target.try_exists()?;
    let mounted: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_task_mounts WHERE directory_path=?)",
    )
    .bind(root.to_string_lossy().as_ref())
    .fetch_one(&mut *tx)
    .await?;
    if mounted && !existing {
        return Err(invalid(
            "The registered project document is missing; it must not be recreated from the local index",
        ));
    }
    let document = if existing {
        read_path(&target)?
    } else if let Some(id) = request.id {
        let registered: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=?)")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        if registered {
            legacy_in(&mut tx, id).await?
        } else {
            ProjectDocument::new(request)
        }
    } else {
        ProjectDocument::new(request)
    };
    register_in(&mut tx, &root, &document).await?;
    project_in(&mut tx, &document).await?;
    if !existing {
        stage(&root, &document, None)?.publish()?;
    }
    tx.commit().await?;
    Ok(document.project)
}

pub(crate) async fn create_in(
    conn: &mut SqliteConnection,
    root: &Path,
    request: CreateProjectRequest,
) -> Result<Option<PreparedDocument>, ApiError> {
    let path = metadata_path(root, true)?;
    let exists = path.try_exists()?;
    let mounted: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_task_mounts WHERE directory_path=?)",
    )
    .bind(root.to_string_lossy().as_ref())
    .fetch_one(&mut *conn)
    .await?;
    if mounted && !exists {
        return Err(invalid("The registered project document is missing"));
    }
    let document = if exists {
        let document = read_path(&path)?;
        if Some(document.project.id) != request.id {
            return Err(invalid(
                "Allocated directory has a different project identity",
            ));
        }
        document
    } else {
        ProjectDocument::new(request)
    };
    register_in(conn, root, &document).await?;
    project_in(conn, &document).await?;
    if exists {
        Ok(None)
    } else {
        Ok(Some(stage(root, &document, None)?))
    }
}

async fn register_in(
    conn: &mut SqliteConnection,
    root: &Path,
    document: &ProjectDocument,
) -> Result<(), ApiError> {
    document.validate()?;
    let existing: Option<String> =
        sqlx::query_scalar("SELECT directory_path FROM project_task_mounts WHERE project_id=?")
            .bind(document.project.id)
            .fetch_optional(&mut *conn)
            .await?;
    if existing
        .as_ref()
        .is_some_and(|path| PathBuf::from(path) != root)
    {
        return Err(ApiError::Conflict(
            "This project is already open from another directory".into(),
        ));
    }
    if existing.is_none() {
        let payload: Option<String> = sqlx::query_scalar(
            "SELECT payload FROM scratch WHERE id=? AND scratch_type='PROJECT_REPO_DEFAULTS'",
        )
        .bind(document.project.id)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(payload) = payload {
            let payload: ScratchPayload = serde_json::from_str(&payload).map_err(invalid)?;
            if let ScratchPayload::ProjectRepoDefaults(defaults) = payload {
                if let Some(path) = defaults
                    .directory_path
                    .filter(|path| !path.trim().is_empty())
                {
                    if tokio::fs::canonicalize(path).await? != root {
                        return Err(ApiError::Conflict(
                            "This project's existing local workspace belongs to another directory"
                                .into(),
                        ));
                    }
                } else if defaults.repos.len() == 1 {
                    let path: Option<String> =
                        sqlx::query_scalar("SELECT path FROM repos WHERE id=?")
                            .bind(defaults.repos[0].repo_id)
                            .fetch_optional(&mut *conn)
                            .await?;
                    let path =
                        path.ok_or_else(|| invalid("Existing project repository is unavailable"))?;
                    if tokio::fs::canonicalize(path).await? != root {
                        return Err(ApiError::Conflict(
                            "This project's existing repository belongs to another directory"
                                .into(),
                        ));
                    }
                }
            }
        }
        let paths:Vec<String>=sqlx::query_scalar("SELECT r.path FROM repos r JOIN project_repos p ON p.repo_id=r.id WHERE p.project_id=?").bind(document.project.id).fetch_all(&mut *conn).await?;
        if paths.len() == 1 && tokio::fs::canonicalize(&paths[0]).await? != root {
            return Err(ApiError::Conflict(
                "This project's registered repository belongs to another directory".into(),
            ));
        }
    }
    let other: Option<Uuid> =
        sqlx::query_scalar("SELECT project_id FROM project_task_mounts WHERE directory_path=?")
            .bind(root.to_string_lossy().as_ref())
            .fetch_optional(&mut *conn)
            .await?;
    if other.is_some_and(|id| id != document.project.id) {
        return Err(invalid(
            "Directory is already registered to a different project identity",
        ));
    }
    sqlx::query("INSERT INTO projects(id,name,created_at,updated_at) VALUES (?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,updated_at=excluded.updated_at").bind(document.project.id).bind(&document.project.name).bind(document.project.created_at).bind(document.project.updated_at).execute(&mut *conn).await?;
    sqlx::query("INSERT INTO project_task_mounts(project_id,directory_path,indexed_revision) VALUES (?,?,0) ON CONFLICT(project_id) DO NOTHING").bind(document.project.id).bind(root.to_string_lossy().as_ref()).execute(&mut *conn).await?;
    let payload = ScratchPayload::ProjectRepoDefaults(ProjectRepoDefaultsData {
        repos: vec![],
        directory_path: Some(root.to_string_lossy().into_owned()),
    });
    sqlx::query("INSERT INTO scratch(id,scratch_type,payload) VALUES (?,'PROJECT_REPO_DEFAULTS',?) ON CONFLICT(id,scratch_type) DO NOTHING").bind(document.project.id).bind(serde_json::to_string(&payload).map_err(invalid)?).execute(&mut *conn).await?;
    Ok(())
}

/// Project exact file state without REPLACE, preserving runtime foreign keys.
async fn project_in(
    conn: &mut SqliteConnection,
    document: &ProjectDocument,
) -> Result<(), ApiError> {
    document.validate()?;
    let id = document.project.id;
    sqlx::query("UPDATE projects SET name=?,updated_at=? WHERE id=?")
        .bind(&document.project.name)
        .bind(document.project.updated_at)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("INSERT INTO local_project_metadata(project_id,organization_id,color,sort_order) VALUES (?,?,?,?) ON CONFLICT(project_id) DO UPDATE SET color=excluded.color,sort_order=excluded.sort_order").bind(id).bind(Uuid::from_u128(2)).bind(&document.project.color).bind(document.project.sort_order).execute(&mut *conn).await?;
    // Deferred FK validation allows parents to appear after children in a file.
    sqlx::query("PRAGMA defer_foreign_keys=ON")
        .execute(&mut *conn)
        .await?;
    for status in &document.statuses {
        upsert(
            conn,
            "local_project_statuses",
            serde_json::to_value(status).map_err(invalid)?,
            id,
        )
        .await?;
    }
    for tag in &document.tags {
        upsert(
            conn,
            "local_tags",
            serde_json::to_value(tag).map_err(invalid)?,
            id,
        )
        .await?;
    }
    for task in &document.tasks {
        upsert(
            conn,
            "local_issues",
            serde_json::to_value(task).map_err(invalid)?,
            id,
        )
        .await?;
    }
    // Associations have no runtime children and are derived as a whole.
    for table in [
        "local_issue_tags",
        "local_issue_assignees",
        "local_issue_followers",
        "local_issue_relationships",
    ] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE issue_id IN (SELECT id FROM local_issues WHERE project_id=?)"
        ))
        .bind(id)
        .execute(&mut *conn)
        .await?;
    }
    for row in &document.task_tags {
        upsert(
            conn,
            "local_issue_tags",
            serde_json::to_value(row).map_err(invalid)?,
            id,
        )
        .await?;
    }
    for row in &document.task_assignees {
        upsert(
            conn,
            "local_issue_assignees",
            serde_json::to_value(row).map_err(invalid)?,
            id,
        )
        .await?;
    }
    for row in &document.task_followers {
        upsert(
            conn,
            "local_issue_followers",
            serde_json::to_value(row).map_err(invalid)?,
            id,
        )
        .await?;
    }
    for row in &document.task_relationships {
        upsert(
            conn,
            "local_issue_relationships",
            serde_json::to_value(row).map_err(invalid)?,
            id,
        )
        .await?;
    }
    for (table, keep) in [
        (
            "local_issues",
            document.tasks.iter().map(|t| t.id).collect::<HashSet<_>>(),
        ),
        (
            "local_project_statuses",
            document.statuses.iter().map(|s| s.id).collect(),
        ),
        ("local_tags", document.tags.iter().map(|t| t.id).collect()),
    ] {
        let current: Vec<Uuid> =
            sqlx::query_scalar(&format!("SELECT id FROM {table} WHERE project_id=?"))
                .bind(id)
                .fetch_all(&mut *conn)
                .await?;
        for missing in current.into_iter().filter(|value| !keep.contains(value)) {
            if table == "local_issues" {
                let used: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE issue_id=?) OR EXISTS(SELECT 1 FROM workflow_runs WHERE issue_id=?)").bind(missing).bind(missing).fetch_one(&mut *conn).await?;
                if used {
                    return Err(ApiError::Conflict(
                        "Stop and remove this task's executions before deleting it".into(),
                    ));
                }
            }
            sqlx::query(&format!("DELETE FROM {table} WHERE id=?"))
                .bind(missing)
                .execute(&mut *conn)
                .await?;
        }
    }
    // Catch constraints before publishing; no success with a broken projection.
    if sqlx::query("PRAGMA foreign_key_check")
        .fetch_optional(&mut *conn)
        .await?
        .is_some()
    {
        return Err(invalid(
            "Task mutation conflicts with local runtime references",
        ));
    }
    sqlx::query("UPDATE project_task_mounts SET indexed_revision=? WHERE project_id=?")
        .bind(document.revision)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

fn projection_row(mut row: Value) -> Result<Value, ApiError> {
    let values = row
        .as_object_mut()
        .ok_or_else(|| invalid("Expected record"))?;
    // Public Task terminology does not rename historical SQLite FK columns.
    for (public, physical) in [
        ("task_id", "issue_id"),
        ("task_number", "issue_number"),
        ("parent_task_id", "parent_issue_id"),
        ("parent_task_sort_order", "parent_issue_sort_order"),
        ("related_task_id", "related_issue_id"),
    ] {
        if let Some(value) = values.remove(public) {
            values.insert(physical.into(), value);
        }
    }
    Ok(row)
}

async fn upsert(
    conn: &mut SqliteConnection,
    table: &str,
    row: Value,
    project: Uuid,
) -> Result<(), ApiError> {
    let row = projection_row(row)?;
    let values = row.as_object().ok_or_else(|| invalid("Expected record"))?;
    if matches!(
        table,
        "local_issues" | "local_project_statuses" | "local_tags"
    ) {
        let id = Uuid::parse_str(values["id"].as_str().ok_or_else(|| invalid("Missing ID"))?)
            .map_err(invalid)?;
        let previous: Option<Uuid> =
            sqlx::query_scalar(&format!("SELECT project_id FROM {table} WHERE id=?"))
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?;
        if previous.is_some_and(|id| id != project) {
            return Err(invalid("Record ID belongs to another registered project"));
        }
    }
    if matches!(
        table,
        "local_issue_tags"
            | "local_issue_assignees"
            | "local_issue_followers"
            | "local_issue_relationships"
    ) {
        let id = Uuid::parse_str(values["id"].as_str().ok_or_else(|| invalid("Missing ID"))?)
            .map_err(invalid)?;
        let previous: Option<Uuid> = sqlx::query_scalar(&format!(
            "SELECT i.project_id FROM {table} r JOIN local_issues i ON i.id=r.issue_id WHERE r.id=?"
        ))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
        if previous.is_some_and(|id| id != project) {
            return Err(invalid(
                "Relationship ID belongs to another registered project",
            ));
        }
    }
    let columns = values.keys().cloned().collect::<Vec<_>>();
    let fields = columns.join(",");
    let placeholders = vec!["?"; columns.len()].join(",");
    let updates = columns
        .iter()
        .filter(|c| c.as_str() != "id")
        .map(|c| format!("{c}=excluded.{c}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "INSERT INTO {table}({fields}) VALUES ({placeholders}) ON CONFLICT(id) DO UPDATE SET {updates}"
    );
    let mut query = sqlx::query(&sql);
    for column in &columns {
        let value = &values[column];
        if column == "extension_metadata" {
            query = query.bind(value.to_string());
        } else if column == "id" || (column.ends_with("_id") && column != "simple_id") {
            let id = value
                .as_str()
                .map(Uuid::parse_str)
                .transpose()
                .map_err(invalid)?;
            query = query.bind(id);
        } else {
            query = match value {
                Value::Null => query.bind(Option::<String>::None),
                Value::Bool(v) => query.bind(*v),
                Value::Number(v) => {
                    if let Some(v) = v.as_i64() {
                        query.bind(v)
                    } else {
                        query.bind(v.as_f64().ok_or_else(|| invalid("Invalid number"))?)
                    }
                }
                Value::String(v) => query.bind(v.clone()),
                _ => return Err(invalid("Unexpected nested record")),
            };
        }
    }
    query.execute(&mut *conn).await?;
    Ok(())
}

pub(crate) fn patch<T: Serialize + DeserializeOwned>(
    value: &mut T,
    changes: impl Serialize,
) -> Result<(), ApiError> {
    let mut object = serde_json::to_value(&*value).map_err(invalid)?;
    let changes = serde_json::to_value(changes).map_err(invalid)?;
    for (key, value) in changes
        .as_object()
        .ok_or_else(|| invalid("Expected patch"))?
    {
        object[key] = value.clone();
    }
    *value = serde_json::from_value(object).map_err(invalid)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    use super::*;

    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(":memory:")
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        pool
    }

    fn project_request() -> CreateProjectRequest {
        CreateProjectRequest {
            id: None,
            organization_id: Uuid::from_u128(2),
            name: "Portable project".into(),
            color: "210 80% 52%".into(),
        }
    }
    fn task_request(document: &ProjectDocument) -> CreateTaskRequest {
        CreateTaskRequest {
            id: Some(Uuid::new_v4()),
            project_id: document.project.id,
            status_id: document.statuses[0].id,
            title: "Write an industry report".into(),
            description: Some("Use project-relative source files".into()),
            priority: None,
            start_date: None,
            target_date: None,
            completed_at: None,
            sort_order: 0.0,
            parent_issue_id: None,
            parent_issue_sort_order: None,
            extension_metadata: Value::Null,
        }
    }

    #[tokio::test]
    async fn read_snapshot_follows_file_changes_without_repairing_sql() {
        let pool = pool().await;
        let directory = tempfile::tempdir().unwrap();
        let project = open(&pool, directory.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        let first = read_snapshot(&pool, project.id, false).await.unwrap();
        let cached = read_snapshot(&pool, project.id, false).await.unwrap();
        assert!(Arc::ptr_eq(&first, &cached));

        let root = directory.path().canonicalize().unwrap();
        let path = root.join(METADATA).join(DOCUMENT);
        let mut changed = first.as_ref().clone();
        changed.project.name = "External authoritative title with different length".into();
        changed.revision += 1;
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        let external = read_snapshot(&pool, project.id, false).await.unwrap();
        assert_eq!(external.project.name, changed.project.name);
        assert_eq!(external.revision, changed.revision);
        let (sql_name, indexed_revision): (String, i64) = sqlx::query_as(
            "SELECT p.name,m.indexed_revision FROM projects p JOIN project_task_mounts m ON m.project_id=p.id WHERE p.id=?",
        ).bind(project.id).fetch_one(&pool).await.unwrap();
        assert_eq!(sql_name, first.project.name);
        assert_eq!(indexed_revision, first.revision);

        // A corrupt authority cannot be hidden by a previously cached document.
        std::fs::write(&path, b"{broken").unwrap();
        assert!(read_snapshot(&pool, project.id, false).await.is_err());
        assert!(read_snapshot(&pool, project.id, true).await.is_err());
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(read_snapshot(&pool, project.id, false).await.is_ok());

        // Platform publication actively evicts the shared pure-read cache.
        changed.revision += 1;
        changed.project.name = "Published title".into();
        stage(&root, &changed, Some(external.revision))
            .unwrap()
            .publish()
            .unwrap();
        assert_eq!(
            read_snapshot(&pool, project.id, false)
                .await
                .unwrap()
                .project
                .name,
            "Published title"
        );
        std::fs::remove_file(&path).unwrap();
        assert!(read_snapshot(&pool, project.id, false).await.is_err());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn read_snapshot_reports_windows_file_access_failure_without_stale_fallback() {
        use std::os::windows::fs::OpenOptionsExt;

        let pool = pool().await;
        let directory = tempfile::tempdir().unwrap();
        let project = open(&pool, directory.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        read_snapshot(&pool, project.id, false).await.unwrap();
        let path = directory.path().join(METADATA).join(DOCUMENT);
        let exclusive = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(read_snapshot(&pool, project.id, true).await.is_err());
        drop(exclusive);
        assert!(read_snapshot(&pool, project.id, true).await.is_ok());
    }

    #[tokio::test]
    async fn task_crud_uses_file_authority_and_preserves_ids_on_a_fresh_host() {
        let pool = pool().await;
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let project = open(&pool, first.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        let document = read(&pool, project.id).await.unwrap();
        let request = task_request(&document);
        let id = request.id.unwrap();
        mutate(&pool, project.id, |document| document.insert_task(request))
            .await
            .unwrap();
        mutate(&pool, project.id, |document| {
            document.tasks[0].title = "An updated report".into();
            Ok(())
        })
        .await
        .unwrap();
        let bytes = std::fs::read(first.path().join(METADATA).join(DOCUMENT)).unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("An updated report"));
        assert!(!String::from_utf8_lossy(&bytes).contains("session_id"));
        std::fs::create_dir(second.path().join(METADATA)).unwrap();
        std::fs::write(second.path().join(METADATA).join(DOCUMENT), bytes).unwrap();
        let fresh = super::tests::pool().await;
        let restored = open(&fresh, second.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        assert_eq!(restored.id, project.id);
        let restored = read(&fresh, project.id).await.unwrap();
        assert_eq!(restored.tasks[0].id, id);
        assert_eq!(restored.tasks[0].title, "An updated report");
        let runtime:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM sessions)+(SELECT COUNT(*) FROM tasks)+(SELECT COUNT(*) FROM workflow_runs)").fetch_one(&fresh).await.unwrap();
        assert_eq!(runtime, 0);
        mutate(&fresh, project.id, |document| {
            document.tasks.clear();
            Ok(())
        })
        .await
        .unwrap();
        assert!(read(&fresh, project.id).await.unwrap().tasks.is_empty());
        assert_eq!(read(&pool, project.id).await.unwrap().tasks.len(), 1);
    }

    #[tokio::test]
    async fn copied_project_keeps_task_hierarchy_tags_and_relationships() {
        let source_pool = pool().await;
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let project = open(
            &source_pool,
            source.path().to_str().unwrap(),
            project_request(),
        )
        .await
        .unwrap();
        let original = read(&source_pool, project.id).await.unwrap();
        let parent = task_request(&original);
        let parent_id = parent.id.unwrap();
        let mut child = task_request(&original);
        let child_id = child.id.unwrap();
        child.parent_issue_id = Some(parent_id);
        child.parent_issue_sort_order = Some(1.0);
        mutate(&source_pool, project.id, |document| {
            document.insert_task(parent)?;
            document.insert_task(child)?;
            document.task_tags.push(TaskTag {
                id: Uuid::new_v4(),
                issue_id: child_id,
                tag_id: document.tags[0].id,
            });
            document.task_relationships.push(TaskRelationship {
                id: Uuid::new_v4(),
                issue_id: parent_id,
                related_issue_id: child_id,
                relationship_type: api_types::TaskRelationshipType::Related,
                created_at: Utc::now(),
            });
            Ok(())
        })
        .await
        .unwrap();
        let expected = read(&source_pool, project.id).await.unwrap();
        std::fs::create_dir(destination.path().join(METADATA)).unwrap();
        std::fs::copy(
            source.path().join(METADATA).join(DOCUMENT),
            destination.path().join(METADATA).join(DOCUMENT),
        )
        .unwrap();
        let destination_pool = pool().await;
        let restored = open(
            &destination_pool,
            destination.path().to_str().unwrap(),
            project_request(),
        )
        .await
        .unwrap();
        assert_eq!(restored.id, project.id);
        let actual = read(&destination_pool, restored.id).await.unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        assert!(
            sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&destination_pool)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn concurrent_writers_preserve_both_tasks_and_distinct_numbers() {
        let directory = tempfile::tempdir().unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(directory.path().join("test.sqlite"))
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .busy_timeout(std::time::Duration::from_secs(5)),
            )
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        let project = open(&pool, directory.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        let document = read(&pool, project.id).await.unwrap();
        let first = task_request(&document);
        let second = task_request(&document);
        let first_id = first.id.unwrap();
        let second_id = second.id.unwrap();
        let (a, b) = tokio::join!(
            mutate(&pool, project.id, |document| document.insert_task(first)),
            mutate(&pool, project.id, |document| document.insert_task(second)),
        );
        assert_eq!(a.unwrap(), first_id);
        assert_eq!(b.unwrap(), second_id);
        let document = read(&pool, project.id).await.unwrap();
        assert_eq!(document.tasks.len(), 2);
        assert_eq!(document.revision, 3);
        assert_ne!(
            document.tasks[0].issue_number,
            document.tasks[1].issue_number
        );
        pool.close().await;
    }

    #[tokio::test]
    async fn invalid_or_missing_mounted_file_never_falls_back_to_sql() {
        let pool = pool().await;
        let directory = tempfile::tempdir().unwrap();
        let project = open(&pool, directory.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        let document = read(&pool, project.id).await.unwrap();
        let request = task_request(&document);
        mutate(&pool, project.id, |document| document.insert_task(request))
            .await
            .unwrap();
        let path = directory.path().join(METADATA).join(DOCUMENT);
        let mut unsupported = read(&pool, project.id).await.unwrap();
        unsupported.schema_version = FORMAT_VERSION + 1;
        let bytes = serde_json::to_vec(&unsupported).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(read(&pool, project.id).await.is_err());
        assert!(mutate(&pool, project.id, |_| Ok(())).await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::write(&path, b"{broken").unwrap();
        assert!(read(&pool, project.id).await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
        std::fs::remove_file(&path).unwrap();
        assert!(read(&pool, project.id).await.is_err());
        assert!(
            open(&pool, directory.path().to_str().unwrap(), project_request())
                .await
                .is_err()
        );
        let mut retry = project_request();
        retry.id = Some(project.id);
        assert!(
            open(&pool, directory.path().to_str().unwrap(), retry)
                .await
                .is_err()
        );
        assert!(!path.exists());
        let indexed: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM local_issues WHERE project_id=?")
                .bind(project.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(indexed, 1);
    }

    #[tokio::test]
    async fn rejected_file_mutation_keeps_prior_document_and_projection() {
        let pool = pool().await;
        let directory = tempfile::tempdir().unwrap();
        let project = open(&pool, directory.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        let document = read(&pool, project.id).await.unwrap();
        let request = task_request(&document);
        mutate(&pool, project.id, |document| document.insert_task(request))
            .await
            .unwrap();
        let path = directory.path().join(METADATA).join(DOCUMENT);
        let before = std::fs::read(&path).unwrap();
        assert!(
            mutate(&pool, project.id, |document| {
                document.tasks[0].status_id = Uuid::new_v4();
                Ok(())
            })
            .await
            .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert_eq!(
            read(&pool, project.id).await.unwrap().tasks[0].status_id,
            document.statuses[0].id
        );
    }

    #[tokio::test]
    async fn staged_runtime_rejection_does_not_publish_a_task() {
        let pool = pool().await;
        let directory = tempfile::tempdir().unwrap();
        let project = open(&pool, directory.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        let document = read(&pool, project.id).await.unwrap();
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let (_, staged) = insert_task_in(&mut tx, task_request(&document))
            .await
            .unwrap();
        drop(staged);
        tx.rollback().await.unwrap();
        assert!(read(&pool, project.id).await.unwrap().tasks.is_empty());
    }

    #[tokio::test]
    async fn published_file_repairs_rolled_back_sql_without_duplicate_task() {
        let pool = pool().await;
        let directory = tempfile::tempdir().unwrap();
        let project = open(&pool, directory.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        let document = read(&pool, project.id).await.unwrap();
        let request = task_request(&document);
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let (_, staged) = insert_task_in(&mut tx, request.clone()).await.unwrap();
        staged.publish().unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(read(&pool, project.id).await.unwrap().tasks.len(), 1);
        mutate(&pool, project.id, |document| document.insert_task(request))
            .await
            .unwrap();
        assert_eq!(read(&pool, project.id).await.unwrap().tasks.len(), 1);
    }

    #[tokio::test]
    async fn double_mount_and_cross_project_ids_are_rejected() {
        let pool = pool().await;
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let project = open(&pool, first.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        std::fs::create_dir(second.path().join(METADATA)).unwrap();
        std::fs::copy(
            first.path().join(METADATA).join(DOCUMENT),
            second.path().join(METADATA).join(DOCUMENT),
        )
        .unwrap();
        assert!(
            open(&pool, second.path().to_str().unwrap(), project_request())
                .await
                .is_err()
        );
        let mut document = read(&pool, project.id).await.unwrap();
        document.statuses[0].project_id = Uuid::new_v4();
        assert!(document.validate().is_err());
    }

    #[test]
    fn projection_keeps_historical_sql_columns_and_public_task_fields_separate() {
        let id = Uuid::new_v4();
        let value=projection_row(json!({"task_id":id,"task_number":1,"parent_task_id":null,"parent_task_sort_order":null,"related_task_id":id,"simple_id":"TASK-1"})).unwrap();
        assert_eq!(value["issue_id"], json!(id));
        assert_eq!(value["issue_number"], 1);
        assert_eq!(value["simple_id"], "TASK-1");
        assert!(value.get("task_id").is_none());
        assert!(value.get("parent_issue_id").is_some());
    }

    #[test]
    fn duplicate_task_id_requires_identical_authored_request() {
        let mut document = ProjectDocument::new(project_request());
        let request = task_request(&document);
        document.insert_task(request.clone()).unwrap();
        document.insert_task(request.clone()).unwrap();
        let mut changed = request.clone();
        changed.status_id = document.statuses[1].id;
        assert!(document.insert_task(changed).is_err());
        let mut changed = request;
        changed.extension_metadata = json!({"other":"payload"});
        assert!(document.insert_task(changed).is_err());
        assert_eq!(document.tasks.len(), 1);
    }

    #[tokio::test]
    async fn pre_adoption_root_cannot_be_rebound_by_copied_manifest() {
        let pool = pool().await;
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let project = open(&pool, first.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        sqlx::query("DELETE FROM project_task_mounts WHERE project_id=?")
            .bind(project.id)
            .execute(&pool)
            .await
            .unwrap();
        std::fs::create_dir(second.path().join(METADATA)).unwrap();
        std::fs::copy(
            first.path().join(METADATA).join(DOCUMENT),
            second.path().join(METADATA).join(DOCUMENT),
        )
        .unwrap();
        assert!(
            open(&pool, second.path().to_str().unwrap(), project_request())
                .await
                .is_err()
        );
        assert_eq!(
            read(&pool, project.id).await.unwrap().project.id,
            project.id
        );
    }

    #[test]
    fn staged_publication_detects_revision_conflicts_and_keeps_valid_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let mut document = ProjectDocument::new(project_request());
        stage(&root, &document, None).unwrap().publish().unwrap();
        document.revision = 2;
        let first = stage(&root, &document, Some(1)).unwrap();
        let second = stage(&root, &document, Some(1)).unwrap();
        first.publish().unwrap();
        assert!(second.publish().is_err());
        assert_eq!(
            read_path(&root.join(METADATA).join(DOCUMENT))
                .unwrap()
                .revision,
            2
        );
    }

    #[tokio::test]
    async fn mounted_directory_cannot_change_through_workspace_defaults() {
        let pool = pool().await;
        let directory = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let project = open(&pool, directory.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        let defaults = ProjectRepoDefaultsData {
            repos: vec![],
            directory_path: Some(directory.path().to_string_lossy().into_owned()),
        };
        validate_directory_binding(&pool, project.id, &defaults)
            .await
            .unwrap();
        let defaults = ProjectRepoDefaultsData {
            repos: vec![],
            directory_path: Some(other.path().to_string_lossy().into_owned()),
        };
        assert!(
            validate_directory_binding(&pool, project.id, &defaults)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn associations_cannot_steal_another_projects_identity() {
        let pool = pool().await;
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let a = open(&pool, first.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        let b = open(&pool, second.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        let association_id = Uuid::new_v4();
        for (project, should_succeed) in [(a.id, true), (b.id, false)] {
            let document = read(&pool, project).await.unwrap();
            let request = task_request(&document);
            let task_id = request.id.unwrap();
            let result = mutate(&pool, project, |document| {
                document.insert_task(request)?;
                document.task_tags.push(TaskTag {
                    id: association_id,
                    issue_id: task_id,
                    tag_id: document.tags[0].id,
                });
                Ok(())
            })
            .await;
            assert_eq!(result.is_ok(), should_succeed);
        }
        assert_eq!(
            owner(&pool, "task_tags", association_id).await.unwrap(),
            a.id
        );
        assert!(read(&pool, b.id).await.unwrap().tasks.is_empty());
    }

    #[tokio::test]
    async fn rejected_runtime_deletion_preserves_file_and_execution() {
        let pool = pool().await;
        let directory = tempfile::tempdir().unwrap();
        let project = open(&pool, directory.path().to_str().unwrap(), project_request())
            .await
            .unwrap();
        let document = read(&pool, project.id).await.unwrap();
        let request = task_request(&document);
        let task_id = request.id.unwrap();
        mutate(&pool, project.id, |document| document.insert_task(request))
            .await
            .unwrap();
        let execution_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tasks(id,project_id,issue_id,title,execution_kind) VALUES (?,?,?,'Local execution','agent')").bind(execution_id).bind(project.id).bind(task_id).execute(&pool).await.unwrap();
        assert!(
            mutate(&pool, project.id, |document| {
                document.tasks.clear();
                Ok(())
            })
            .await
            .is_err()
        );
        assert_eq!(read(&pool, project.id).await.unwrap().tasks[0].id, task_id);
        let preserved: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?)")
            .bind(execution_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(preserved);
    }

    #[tokio::test]
    async fn one_time_adoption_keeps_ids_and_never_reseeds_deleted_tags() {
        let pool = pool().await;
        let directory = tempfile::tempdir().unwrap();
        let document = ProjectDocument::new(project_request());
        let id = document.project.id;
        let root = directory.path().canonicalize().unwrap();
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        register_in(&mut tx, &root, &document).await.unwrap();
        project_in(&mut tx, &document).await.unwrap();
        sqlx::query("DELETE FROM project_task_mounts WHERE project_id=?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let adopted = read(&pool, id).await.unwrap();
        assert_eq!(adopted.statuses[0].id, document.statuses[0].id);
        mutate(&pool, id, |document| {
            document.tags.clear();
            Ok(())
        })
        .await
        .unwrap();
        assert!(read(&pool, id).await.unwrap().tags.is_empty());
        assert!(root.join(METADATA).join(DOCUMENT).is_file());
    }
}
