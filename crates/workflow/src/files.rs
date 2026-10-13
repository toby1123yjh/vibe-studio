//! File protocols contain references and control data, not execution status.
//! A valid document is still unsubmitted until the runtime accepts it.
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tokio::fs;

pub const WORKFLOW_FILE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum WorkflowFileError {
    #[error("workflow control path is invalid: {0}")]
    InvalidPath(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowFileRef {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowHandoff {
    pub schema_version: u32,
    pub run_id: String,
    pub node_execution_id: String,
    pub summary_path: Option<String>,
    pub artifact_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitTaskManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub split_execution_id: String,
    pub tasks: Vec<SplitTask>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitTask {
    pub subtask_id: String,
    pub title: String,
    pub instruction_path: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchPlan {
    pub schema_version: u32,
    pub run_id: String,
    pub split_execution_id: String,
    pub manifest_sha256: String,
    pub stages: Vec<DispatchStage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchStage {
    pub assignments: Vec<DispatchAssignment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchAssignment {
    pub subtask_id: String,
    pub node_id: String,
}

pub fn validate_manifest(manifest: &SplitTaskManifest) -> Result<(), String> {
    use std::collections::HashSet;
    if manifest.schema_version != WORKFLOW_FILE_SCHEMA_VERSION {
        return Err("Unsupported split manifest schema_version".into());
    }
    if manifest.tasks.is_empty() {
        return Err("Split manifest must contain at least one subtask".into());
    }
    let mut ids = HashSet::new();
    for task in &manifest.tasks {
        if task.subtask_id.is_empty()
            || !task
                .subtask_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || !ids.insert(task.subtask_id.as_str())
        {
            return Err(format!(
                "Invalid or duplicate subtask_id `{}`",
                task.subtask_id
            ));
        }
        if task.title.trim().is_empty() || task.instruction_path.trim().is_empty() {
            return Err(format!(
                "Subtask `{}` needs a title and instruction_path",
                task.subtask_id
            ));
        }
    }
    for task in &manifest.tasks {
        let mut seen = HashSet::new();
        for dependency in &task.depends_on {
            if dependency == &task.subtask_id
                || !ids.contains(dependency.as_str())
                || !seen.insert(dependency)
            {
                return Err(format!(
                    "Subtask `{}` has invalid dependency `{dependency}`",
                    task.subtask_id
                ));
            }
        }
    }
    // Detect cycles before an automatic manager turn is authorized.
    let mut completed = HashSet::new();
    loop {
        let before = completed.len();
        for task in &manifest.tasks {
            if task
                .depends_on
                .iter()
                .all(|id| completed.contains(id.as_str()))
            {
                completed.insert(task.subtask_id.as_str());
            }
        }
        if completed.len() == manifest.tasks.len() {
            return Ok(());
        }
        if completed.len() == before {
            return Err("Split manifest dependencies contain a cycle".into());
        }
    }
}

/// Resolve a workflow control file without allowing it to escape the project
/// root. Control files are always addressed with project-relative paths.
pub fn relative_control_path(root: &Path, relative: &str) -> Result<PathBuf, WorkflowFileError> {
    let value = relative.replace('\\', "/");
    let path = Path::new(&value);
    if value.trim().is_empty() || path.is_absolute() || value.contains(':') || value.contains('\0')
    {
        return Err(WorkflowFileError::InvalidPath(relative.to_owned()));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(WorkflowFileError::InvalidPath(relative.to_owned()));
    }
    Ok(root.join(path))
}

/// Check the actual filesystem target as well as its lexical path. This also
/// rejects Windows junctions and directory symlinks pointing outside the root.
pub async fn readable_file(root: &Path, relative: &str) -> Result<PathBuf, WorkflowFileError> {
    let target = relative_control_path(root, relative)?;
    let canonical_root = fs::canonicalize(root).await?;
    let canonical = fs::canonicalize(target).await?;
    if !canonical.starts_with(&canonical_root) || !fs::metadata(&canonical).await?.is_file() {
        return Err(WorkflowFileError::InvalidPath(relative.to_owned()));
    }
    Ok(canonical)
}

/// Validate each existing parent before creating a system control directory.
/// A symlinked `.vibe-kanban` must not cause writes outside the project.
pub async fn control_file_path(root: &Path, relative: &str) -> Result<PathBuf, WorkflowFileError> {
    let target = relative_control_path(root, relative)?;
    let canonical_root = fs::canonicalize(root).await?;
    let parent = target
        .parent()
        .ok_or_else(|| WorkflowFileError::InvalidPath(relative.to_owned()))?;
    let mut current = root.to_path_buf();
    for component in parent
        .strip_prefix(root)
        .map_err(|_| WorkflowFileError::InvalidPath(relative.to_owned()))?
        .components()
    {
        current.push(component);
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let canonical = fs::canonicalize(&current).await?;
        if !canonical.starts_with(&canonical_root) || !fs::metadata(&canonical).await?.is_dir() {
            return Err(WorkflowFileError::InvalidPath(relative.to_owned()));
        }
    }
    if fs::symlink_metadata(&target).await.is_ok() {
        let canonical = fs::canonicalize(&target).await?;
        if !canonical.starts_with(&canonical_root) || !fs::metadata(&canonical).await?.is_file() {
            return Err(WorkflowFileError::InvalidPath(relative.to_owned()));
        }
    }
    Ok(target)
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn run_control_dir(root: &Path, run_id: &str) -> Result<PathBuf, WorkflowFileError> {
    if run_id.is_empty()
        || !run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(WorkflowFileError::InvalidPath(run_id.to_owned()));
    }
    Ok(root.join(".vibe-kanban").join("workflows").join(run_id))
}

pub fn split_control_dir(
    root: &Path,
    run_id: &str,
    split_execution_id: &str,
) -> Result<PathBuf, WorkflowFileError> {
    let run = run_control_dir(root, run_id)?;
    if split_execution_id.is_empty()
        || !split_execution_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(WorkflowFileError::InvalidPath(
            split_execution_id.to_owned(),
        ));
    }
    Ok(run.join("splits").join(split_execution_id))
}

/// Write a control document atomically. A file appearing on disk is not an
/// acceptance signal; the runtime still validates and records the DB event.
pub async fn write_json_atomic<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<(), WorkflowFileError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    write_atomic(path, &bytes).await
}

pub async fn write_text_atomic(path: &Path, value: &str) -> Result<(), WorkflowFileError> {
    write_atomic(path, value.as_bytes()).await
}

async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), WorkflowFileError> {
    let parent = path
        .parent()
        .ok_or_else(|| WorkflowFileError::InvalidPath(path.display().to_string()))?;
    fs::create_dir_all(parent).await?;
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    fs::write(&temporary, bytes).await?;
    match fs::rename(&temporary, path).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary).await;
            Err(error.into())
        }
    }
}

pub async fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, WorkflowFileError> {
    Ok(serde_json::from_slice(&fs::read(path).await?)?)
}

pub async fn sha256_file(path: &Path) -> Result<String, WorkflowFileError> {
    let mut hasher = Sha256::new();
    hasher.update(fs::read(path).await?);
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> SplitTaskManifest {
        SplitTaskManifest {
            schema_version: WORKFLOW_FILE_SCHEMA_VERSION,
            run_id: "run".into(),
            split_execution_id: "split".into(),
            tasks: vec![
                SplitTask {
                    subtask_id: "research".into(),
                    title: "Research".into(),
                    instruction_path: "research.md".into(),
                    depends_on: vec![],
                },
                SplitTask {
                    subtask_id: "report".into(),
                    title: "Report".into(),
                    instruction_path: "report.md".into(),
                    depends_on: vec!["research".into()],
                },
            ],
        }
    }

    #[test]
    fn manifest_rejects_cycles_duplicates_and_unknown_dependencies() {
        validate_manifest(&manifest()).unwrap();
        let mut cycle = manifest();
        cycle.tasks[0].depends_on.push("report".into());
        assert!(validate_manifest(&cycle).unwrap_err().contains("cycle"));
        let mut duplicate = manifest();
        duplicate.tasks[1].subtask_id = "research".into();
        assert!(
            validate_manifest(&duplicate)
                .unwrap_err()
                .contains("duplicate")
        );
        let mut unknown = manifest();
        unknown.tasks[1].depends_on = vec!["missing".into()];
        assert!(
            validate_manifest(&unknown)
                .unwrap_err()
                .contains("invalid dependency")
        );
    }

    #[test]
    fn relative_paths_reject_escape_on_all_platforms() {
        let root = Path::new("workspace");
        for invalid in [
            "",
            "../outside",
            "nested/../../outside",
            r"nested\..\outside",
            "/outside",
            r"C:\outside",
            r"\\host\share\outside",
            "file\0name",
        ] {
            assert!(
                relative_control_path(root, invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        assert_eq!(
            relative_control_path(root, r"notes\result.md").unwrap(),
            root.join("notes/result.md")
        );
        assert!(run_control_dir(root, "../run").is_err());
        assert!(split_control_dir(root, "run", "../../split").is_err());
    }

    #[tokio::test]
    async fn atomic_files_are_readable_and_hashes_detect_changes() {
        let root = tempfile::tempdir().unwrap();
        let path = control_file_path(root.path(), ".vibe-kanban/workflows/run/tasks.json")
            .await
            .unwrap();
        write_json_atomic(&path, &manifest()).await.unwrap();
        let accepted_hash = sha256_file(&path).await.unwrap();
        let resolved = readable_file(root.path(), ".vibe-kanban/workflows/run/tasks.json")
            .await
            .unwrap();
        let read: SplitTaskManifest = read_json(&resolved).await.unwrap();
        assert_eq!(read, manifest());
        let mut changed = manifest();
        changed.tasks[0].title = "Changed".into();
        write_json_atomic(&path, &changed).await.unwrap();
        assert_ne!(sha256_file(&path).await.unwrap(), accepted_hash);
        let mut entries = fs::read_dir(path.parent().unwrap()).await.unwrap();
        let mut count = 0;
        while entries.next_entry().await.unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 1, "atomic writes must not retain temporary files");
        assert!(
            readable_file(root.path(), ".vibe-kanban/workflows/run")
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_files_and_control_parents_cannot_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("private.md"), b"private")
            .await
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
        assert!(
            readable_file(root.path(), "linked/private.md")
                .await
                .is_err()
        );
        assert!(
            control_file_path(root.path(), "linked/control/new.json")
                .await
                .is_err()
        );
        assert!(!outside.path().join("control").exists());
        std::os::unix::fs::symlink(
            outside.path().join("private.md"),
            root.path().join("private.md"),
        )
        .unwrap();
        assert!(control_file_path(root.path(), "private.md").await.is_err());
    }
}
