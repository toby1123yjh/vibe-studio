use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use chrono::Utc;
use db::models::dashboard::DashboardFacts;
use sqlx::SqlitePool;
use uuid::Uuid;

use super::{
    query::{self, DashboardRead},
    types::*,
};
use crate::error::ApiError;

type GitCache = HashMap<(String, PathBuf), (Instant, DashboardGitRepository)>;
fn git_cache() -> &'static Mutex<GitCache> {
    static CACHE: OnceLock<Mutex<GitCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Return a display-safe URL and only an unambiguous public repository link.
pub(super) fn sanitize_remote(raw: &str) -> Option<(String, Option<String>)> {
    if let Ok(mut url) = url::Url::parse(raw) {
        if url.cannot_be_a_base() {
            return None;
        }
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.set_query(None);
        url.set_fragment(None);
        let host = url.host_str().unwrap_or("").to_string();
        let path = url.path().trim_end_matches('/').trim_end_matches(".git");
        let web = matches!(host.as_str(), "github.com" | "gitlab.com" | "bitbucket.org")
            .then(|| format!("https://{host}{path}"));
        return Some((url.to_string(), web));
    }
    // SCP-style SSH syntax; strip all userinfo. Do not echo arbitrary URLs
    // whose credential syntax cannot be parsed reliably.
    if !raw.contains("://")
        && let Some((host, path)) = raw.split_once(':')
    {
        let host = host.rsplit('@').next()?;
        if !host.is_empty()
            && !host.contains(['\\', '/', ' ', '?', '#'])
            && !path.contains(['?', '#'])
        {
            let web = matches!(host, "github.com" | "gitlab.com" | "bitbucket.org").then(|| {
                format!(
                    "https://{host}/{}",
                    path.trim_start_matches('/').trim_end_matches(".git")
                )
            });
            return Some((format!("{host}:{path}"), web));
        }
    }
    // Plain filesystem remotes contain no URL authentication material.
    (!raw.contains('@') && !raw.contains("://") && !raw.contains(['?', '#']))
        .then(|| (raw.to_string(), None))
}

fn inspect_git(path: &Path) -> DashboardGitRepository {
    let mut result = DashboardGitRepository {
        path: path.to_string_lossy().into_owned(),
        status: "not_git".into(),
        branch: None,
        commit: None,
        remotes: vec![],
        issue: None,
    };
    if !path.exists() {
        result.status = "missing".into();
        return result;
    }
    let repo = match git2::Repository::discover(path) {
        Ok(repo) => repo,
        Err(error) => {
            if error.code() != git2::ErrorCode::NotFound {
                result.status = "unreadable".into();
                result.issue = Some("Repository metadata could not be read".into());
            }
            return result;
        }
    };
    result.status = "git".into();
    let root = repo.workdir().unwrap_or_else(|| repo.path());
    result.path = root.to_string_lossy().into_owned();
    let service = git::GitService::new();
    if let Ok(head) = service.get_head_info(root) {
        result.branch = (!repo.head_detached().unwrap_or(false)).then_some(head.branch);
        result.commit = Some(head.oid.chars().take(12).collect());
    } else {
        result.issue = Some("Repository HEAD is unavailable or unborn".into());
    }
    match service.list_remotes(root) {
        Ok(remotes) => {
            for remote in remotes {
                if let Some((url, web_url)) = sanitize_remote(&remote.url) {
                    result.remotes.push(DashboardGitRemote {
                        name: remote.name,
                        url,
                        web_url,
                    });
                } else {
                    result.issue = Some("An unparseable remote URL was omitted".into());
                }
            }
        }
        Err(_) => result.issue = Some("Repository remotes could not be read".into()),
    }
    result
}

async fn repository(
    scope: &str,
    path: PathBuf,
    refresh: bool,
) -> Result<DashboardGitRepository, ApiError> {
    let path = tokio::fs::canonicalize(&path).await.unwrap_or(path);
    let key = (scope.to_string(), path.clone());
    if !refresh
        && let Some((time, repo)) = git_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        && time.elapsed() < Duration::from_secs(30)
    {
        return Ok(repo.clone());
    }
    let result = tokio::task::spawn_blocking(move || inspect_git(&path))
        .await
        .map_err(|e| ApiError::Conflict(format!("Git metadata read interrupted: {e}")))?;
    let mut cache = git_cache().lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|_, (time, _)| time.elapsed() < Duration::from_secs(30));
    if cache.len() >= 256 {
        cache.clear();
    }
    cache.insert(key, (Instant::now(), result.clone()));
    Ok(result)
}

pub(super) async fn read(
    pool: &SqlitePool,
    reference: DashboardWorkRef,
    session_id: Option<Uuid>,
    agent_run_id: Option<Uuid>,
    refresh: bool,
) -> Result<DashboardContext, ApiError> {
    let read = DashboardRead::load(pool, refresh).await?;
    let work = read
        .works
        .iter()
        .find(|w| w.row.work == reference)
        .ok_or_else(|| ApiError::Conflict("Dashboard work no longer exists".into()))?;
    let session_ids: HashSet<_> = work.children.iter().filter_map(|c| c.session_id).collect();
    if session_id.is_some_and(|id| !session_ids.contains(&id))
        || agent_run_id.is_some_and(|id| {
            !work.children.iter().any(|c| {
                c.agent_run_id == Some(id)
                    && session_id.is_none_or(|session| c.session_id == Some(session))
            })
        })
    {
        return Err(ApiError::BadRequest(
            "Context reference does not belong to the selected work".into(),
        ));
    }
    let mut sessions = vec![];
    let mut paths = HashSet::new();
    let mut workspaces = HashSet::new();
    for child in &work.children {
        let Some(id) = child.session_id else {
            continue;
        };
        if session_id.is_some_and(|selected| selected != id)
            || agent_run_id.is_some_and(|selected| child.agent_run_id != Some(selected))
        {
            continue;
        }
        let Some(session) = read.facts.sessions.iter().find(|s| s.id == id) else {
            continue;
        };
        if sessions.iter().any(|s: &DashboardSessionContext| {
            s.session_id == id && s.agent_run_id == child.agent_run_id
        }) {
            continue;
        }
        let run = child
            .agent_run_id
            .and_then(|id| read.facts.runs.iter().find(|r| r.id == id));
        if let Some(path) = child.directory.path.as_ref() {
            paths.insert(PathBuf::from(path));
        }
        workspaces.insert(session.workspace_id);
        sessions.push(DashboardSessionContext {
            session_id: id,
            workspace_id: session.workspace_id,
            agent_run_id: child.agent_run_id,
            directory: child.directory.clone(),
            agent: child.agent.clone(),
            started_at: run
                .and_then(|r| r.started_at)
                .or_else(|| run.map(|r| r.created_at)),
            finished_at: run.and_then(|r| r.finished_at),
        });
    }
    for workspace_id in workspaces {
        for repo in DashboardFacts::repositories(pool, workspace_id).await? {
            let path = if repo.workspace_kind == "worktree" {
                repo.workspace_path
                    .map(|p| PathBuf::from(p).join(&repo.name))
                    .unwrap_or_else(|| PathBuf::from(repo.path))
            } else {
                PathBuf::from(repo.path)
            };
            paths.insert(path);
        }
    }
    let scope = query::data_scope(pool);
    let mut repositories = vec![];
    for path in paths {
        let repo = repository(&scope, path, refresh).await?;
        if !repositories
            .iter()
            .any(|r: &DashboardGitRepository| r.path == repo.path)
        {
            repositories.push(repo);
        }
    }
    repositories.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(DashboardContext {
        work: reference,
        generated_at: Utc::now(),
        sessions,
        repositories,
        issues: work.row.issues.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remotes_never_publish_credentials() {
        let (url, web) =
            sanitize_remote("https://user:secret@github.com/owner/repo.git?token=secret#secret")
                .unwrap();
        assert_eq!(url, "https://github.com/owner/repo.git");
        assert_eq!(web.as_deref(), Some("https://github.com/owner/repo"));
        let (url, web) = sanitize_remote("git@github.com:owner/repo.git").unwrap();
        assert_eq!(url, "github.com:owner/repo.git");
        assert_eq!(web.as_deref(), Some("https://github.com/owner/repo"));
        assert!(sanitize_remote("https://user:secret@[invalid").is_none());
        assert!(
            sanitize_remote("https://token@self-hosted.example/scm/team/repo.git")
                .unwrap()
                .1
                .is_none()
        );
    }
}
