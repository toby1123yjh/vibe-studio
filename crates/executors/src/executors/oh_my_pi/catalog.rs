//! Bounded application-side picker observations from actual OMP sessions.
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    command::CmdOverrides, model_selector::ModelSelectorConfig, runtime::AgentEventPayload,
};

pub const CATALOG_EVENT: &str = "omp_model_catalog";
type CacheKey = (PathBuf, String);
type Cache = HashMap<CacheKey, (Instant, ModelSelectorConfig)>;
static CATALOGS: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CatalogScope {
    pub working_dir: PathBuf,
    pub scope_id: String,
}

/// Safe opaque identity shared by process-host observations and picker discovery.
pub fn catalog_identity(cwd: &Path, cmd: &CmdOverrides) -> String {
    scope(cwd, cmd).scope_id
}

pub(crate) fn scope(cwd: &Path, cmd: &CmdOverrides) -> CatalogScope {
    let working_dir = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let ambient: BTreeMap<_, _> = std::env::vars()
        .filter(|(key, _)| {
            key.starts_with("OMP_")
                || key.starts_with("PI_")
                || key.starts_with("ANTHROPIC_")
                || key.starts_with("OPENAI_")
                || key.starts_with("GOOGLE_")
                || key.starts_with("XDG_")
        })
        .collect();
    let root = cmd
        .env
        .as_ref()
        .and_then(|env| {
            env.get("PI_CODING_AGENT_DIR")
                .or_else(|| env.get("OMP_AGENT_DIR"))
        })
        .map(PathBuf::from)
        .or_else(super::oh_my_pi_agent_root);
    let mut files = vec![working_dir.join(".omp/config.yml")];
    if let Some(root) = root {
        files.extend([root.join("config.yml"), root.join("models.json")]);
    }
    // Include installed command identity so replacement/upgrades invalidate a
    // previously observed catalog without launching any discovery session.
    let executable = cmd.base_command_override.as_deref().unwrap_or("omp");
    if let Some(executable) = workspace_utils::shell::resolve_executable_path_blocking(executable) {
        files.push(executable);
    }
    let identities: Vec<_> = files
        .iter()
        .map(|path| {
            let metadata = std::fs::metadata(path).ok();
            (
                path,
                metadata.as_ref().map(std::fs::Metadata::len),
                metadata.and_then(|meta| meta.modified().ok()),
            )
        })
        .collect();
    let value = serde_json::json!({"base_scope":crate::executors::acp::session_config::catalog_identity(cmd),
        "ambient":ambient,"files":identities});
    CatalogScope {
        working_dir,
        scope_id: format!("{:x}", Sha256::digest(value.to_string().as_bytes())),
    }
}

#[derive(Deserialize)]
struct Observation {
    working_dir: PathBuf,
    scope_id: String,
    model_selector: ModelSelectorConfig,
}

/// Call after the real process-host event has been appended in the application.
pub fn cache_catalog_extension(provider_id: &str, event: &AgentEventPayload) {
    let AgentEventPayload::ProviderExtension {
        provider_namespace,
        provider_event,
        payload,
    } = event
    else {
        return;
    };
    if provider_id != "oh_my_pi"
        || provider_namespace != provider_id
        || provider_event != CATALOG_EVENT
    {
        return;
    }
    let Ok(observation) = serde_json::from_value::<Observation>(payload.clone()) else {
        return;
    };
    if !observation.working_dir.is_absolute()
        || observation.scope_id.len() != 64
        || !observation
            .scope_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || observation.model_selector.models.is_empty()
        || observation
            .model_selector
            .default_model
            .as_ref()
            .is_some_and(|default| {
                !observation
                    .model_selector
                    .models
                    .iter()
                    .any(|model| &model.id == default)
            })
    {
        return;
    }
    if let Ok(mut cache) = CATALOGS.lock() {
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(
            (observation.working_dir, observation.scope_id),
            (Instant::now(), observation.model_selector),
        );
    }
}

pub fn observed_catalog(cwd: &Path, cmd: &CmdOverrides) -> Option<ModelSelectorConfig> {
    let scope = scope(cwd, cmd);
    let cache = CATALOGS.lock().ok()?;
    let (observed, selector) = cache.get(&(scope.working_dir, scope.scope_id))?;
    (observed.elapsed() < Duration::from_secs(300)).then(|| selector.clone())
}

pub fn invalidate_catalog(cwd: &Path) {
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    if let Ok(mut cache) = CATALOGS.lock() {
        cache.retain(|(path, _), _| path != &cwd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_only_matching_provider_and_command_scope() {
        let directory = tempfile::tempdir().unwrap();
        let cmd = CmdOverrides::default();
        let scope = scope(directory.path(), &cmd);
        let event = AgentEventPayload::ProviderExtension {
            provider_namespace: "oh_my_pi".into(),
            provider_event: CATALOG_EVENT.into(),
            payload: serde_json::json!({"working_dir":scope.working_dir,"scope_id":scope.scope_id,
                "model_selector":{"models":[{"id":"p/m","name":"Model","reasoning_options":[]}],
                    "providers":[],"agents":[],"permissions":[],"default_model":"p/m"}}),
        };
        cache_catalog_extension("codex", &event);
        assert!(observed_catalog(directory.path(), &cmd).is_none());
        cache_catalog_extension("oh_my_pi", &event);
        assert!(observed_catalog(directory.path(), &cmd).is_some());
        let other = CmdOverrides {
            additional_params: Some(vec!["--profile".into(), "other".into()]),
            ..Default::default()
        };
        assert!(observed_catalog(directory.path(), &other).is_none());
        invalidate_catalog(directory.path());
        assert!(observed_catalog(directory.path(), &cmd).is_none());
    }
}
