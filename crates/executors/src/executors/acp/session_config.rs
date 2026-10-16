//! Provider-owned ACP lifecycle and advertised configuration selection.
//!
//! Select values are opaque: never split model ids or trim reasoning values.
//! A model change replaces the entire option set before effort is validated.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

use agent_client_protocol as proto;
use agent_client_protocol::Agent as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    agent_tools::native_assets,
    executors::ExecutorError,
    model_selector::{
        AgentInfo, CapabilitySupport, ModelCapabilities, ModelInfo, ModelSelectorConfig,
        PermissionPolicy, ReasoningOption,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AcpDialect {
    Gemini,
    Opencode,
    DeepseekHarness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeRestoreMethod {
    Resume,
    Load,
}

impl AcpDialect {
    pub fn restore_method(
        self,
        capabilities: &proto::AgentCapabilities,
    ) -> Result<NativeRestoreMethod, ExecutorError> {
        if self != Self::Gemini && capabilities.session_capabilities.resume.is_some() {
            return Ok(NativeRestoreMethod::Resume);
        }
        if self != Self::DeepseekHarness && capabilities.load_session {
            return Ok(NativeRestoreMethod::Load);
        }
        Err(config_error(
            "The agent does not advertise the required native session restoration method",
        ))
    }

    pub const fn effort_option(self) -> &'static str {
        match self {
            Self::Opencode => "effort",
            Self::DeepseekHarness => "reasoning_effort",
            Self::Gemini => "",
        }
    }
}

pub(crate) fn config_error(message: &str) -> ExecutorError {
    ExecutorError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message,
    ))
}

fn select<'a>(
    options: &'a [proto::SessionConfigOption],
    id: &str,
) -> Option<&'a proto::SessionConfigSelect> {
    options.iter().find_map(|option| {
        if option.id.0.as_ref() != id {
            return None;
        }
        match &option.kind {
            proto::SessionConfigKind::Select(select) => Some(select),
            _ => None,
        }
    })
}

fn values(select: &proto::SessionConfigSelect) -> Vec<&proto::SessionConfigSelectOption> {
    match &select.options {
        proto::SessionConfigSelectOptions::Ungrouped(options) => options.iter().collect(),
        proto::SessionConfigSelectOptions::Grouped(groups) => {
            groups.iter().flat_map(|group| &group.options).collect()
        }
        _ => Vec::new(),
    }
}

pub(crate) fn validate_selection(
    options: &[proto::SessionConfigOption],
    id: &str,
    value: &str,
) -> Result<(), ExecutorError> {
    let selector = select(options, id).ok_or_else(|| {
        config_error("The agent does not advertise the requested session configuration option")
    })?;
    if !values(selector)
        .iter()
        .any(|option| option.value.0.as_ref() == value)
    {
        // Requested values can be sensitive. Never include them, native
        // configuration or provider error.data in logs or errors.
        return Err(config_error(
            "The selected value is not advertised by this agent/model",
        ));
    }
    Ok(())
}

pub(crate) async fn apply_config_options(
    connection: &proto::ClientSideConnection,
    session_id: &str,
    dialect: AcpDialect,
    mut options: Vec<proto::SessionConfigOption>,
    model: Option<&str>,
    effort: Option<&str>,
    mode: Option<&str>,
) -> Result<Vec<proto::SessionConfigOption>, ExecutorError> {
    for (id, value) in [
        ("model", model),
        (dialect.effort_option(), effort),
        ("mode", mode),
    ] {
        let Some(value) = value else {
            continue;
        };
        validate_selection(&options, id, value)?;
        let response = connection
            .set_session_config_option(proto::SetSessionConfigOptionRequest::new(
                proto::SessionId::new(session_id.to_owned()),
                id.to_owned(),
                value.to_owned(),
            ))
            .await
            .map_err(|_| config_error("The agent rejected the selected session configuration"))?;
        options = response.config_options;
        let confirmed =
            select(&options, id).is_some_and(|select| select.current_value.0.as_ref() == value);
        if !confirmed {
            return Err(config_error(
                "The agent did not confirm the selected session configuration",
            ));
        }
    }
    // Later selectors (notably native mode) can replace the model and its
    // effort again. Confirm the complete requested tuple, not only each
    // intermediate acknowledgement.
    for (id, value) in [
        ("model", model),
        (dialect.effort_option(), effort),
        ("mode", mode),
    ] {
        if let Some(value) = value
            && !select(&options, id)
                .is_some_and(|selected| selected.current_value.0.as_ref() == value)
        {
            return Err(config_error(
                "The agent changed an explicitly selected option while applying session configuration",
            ));
        }
    }
    Ok(options)
}

/// Gemini exposes legacy model/mode state, not the configOptions dialect.
#[derive(Debug, Clone, Default)]
pub(crate) struct LegacySessionConfiguration {
    pub models: Option<proto::SessionModelState>,
    pub modes: Option<proto::SessionModeState>,
}

pub(crate) async fn apply_legacy_configuration(
    connection: &proto::ClientSideConnection,
    session_id: &str,
    mut state: LegacySessionConfiguration,
    model: Option<&str>,
    mode: Option<&str>,
) -> Result<LegacySessionConfiguration, ExecutorError> {
    // Validate both selections before sending either request. An absent
    // catalog means unknown support, not permission to silently ignore input.
    if let Some(model) = model {
        let models = state
            .models
            .as_ref()
            .ok_or_else(|| config_error("Gemini did not advertise model selection"))?;
        if !models
            .available_models
            .iter()
            .any(|candidate| candidate.model_id.0.as_ref() == model)
        {
            return Err(config_error("Gemini did not advertise the selected model"));
        }
    }
    if let Some(mode) = mode {
        let modes = state
            .modes
            .as_ref()
            .ok_or_else(|| config_error("Gemini did not advertise mode selection"))?;
        if !modes
            .available_modes
            .iter()
            .any(|candidate| candidate.id.0.as_ref() == mode)
        {
            return Err(config_error("Gemini did not advertise the selected mode"));
        }
    }
    if let Some(model) = model {
        connection
            .set_session_model(proto::SetSessionModelRequest::new(
                session_id.to_owned(),
                model.to_owned(),
            ))
            .await
            .map_err(|_| config_error("Gemini rejected the selected model; no prompt was sent"))?;
        // The legacy setter returns an empty response, not resolved model
        // state. Keep the catalog, but do not present the request as observed.
        if let Some(models) = state.models.as_mut()
            && models.current_model_id.0.as_ref() != model
        {
            models.current_model_id = proto::ModelId::new("");
        }
    }
    if let Some(mode) = mode {
        connection
            .set_session_mode(proto::SetSessionModeRequest::new(
                session_id.to_owned(),
                mode.to_owned(),
            ))
            .await
            .map_err(|_| config_error("Gemini rejected the selected mode; no prompt was sent"))?;
        if let Some(modes) = state.modes.as_mut()
            && modes.current_mode_id.0.as_ref() != mode
        {
            modes.current_mode_id = proto::SessionModeId::new("");
        }
    }
    Ok(state)
}

/// Native-confirmed settings only. Requested values remain in ExecutorConfig.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRuntimeConfiguration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_id: Option<String>,
    pub source: String,
}

pub(crate) fn observed_configuration(
    dialect: AcpDialect,
    options: &[proto::SessionConfigOption],
) -> AcpRuntimeConfiguration {
    AcpRuntimeConfiguration {
        model_id: select(options, "model").map(|value| value.current_value.0.to_string()),
        reasoning_id: select(options, dialect.effort_option())
            .map(|value| value.current_value.0.to_string()),
        source: "acp_config_options".to_owned(),
    }
}

pub(crate) fn observe_legacy_catalog(
    cwd: PathBuf,
    identity: String,
    state: &LegacySessionConfiguration,
) -> Option<AcpCatalogObservation> {
    let models = state.models.as_ref()?;
    let current = models.current_model_id.0.as_ref();
    let observation = AcpCatalogObservation {
        working_dir: cwd,
        scope_id: identity,
        model_selector: ModelSelectorConfig {
            models: models
                .available_models
                .iter()
                .map(|model| ModelInfo {
                    id: model.model_id.0.to_string(),
                    name: model.name.clone(),
                    capabilities: Some(ModelCapabilities {
                        // Thinking budgets belong to native model settings. ACP
                        // does not advertise a per-attempt thinking selector.
                        reasoning: CapabilitySupport::Unsupported,
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .collect(),
            default_model: models
                .available_models
                .iter()
                .any(|model| model.model_id.0.as_ref() == current)
                .then(|| current.to_owned()),
            agents: state
                .modes
                .as_ref()
                .map(|modes| {
                    modes
                        .available_modes
                        .iter()
                        .map(|mode| AgentInfo {
                            id: mode.id.0.to_string(),
                            label: mode.name.clone(),
                            description: mode.description.clone(),
                            is_default: mode.id == modes.current_mode_id,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            // Native modes are the authority. Do not add an overlapping
            // generic policy switch beside default/auto_edit/yolo/plan.
            permissions: Vec::new(),
            ..Default::default()
        },
    };
    cache_observation(AcpDialect::Gemini, &observation);
    Some(observation)
}

pub(crate) fn is_catalog_context_env(key: &str) -> bool {
    crate::workflow_mcp::is_context_env_key(key)
        || key.eq_ignore_ascii_case(crate::workflow_mcp::BACKEND_URL_ENV)
        || key.eq_ignore_ascii_case(crate::workflow_mcp::EXECUTABLE_ENV)
        || [
            "VK_WORKSPACE_ID",
            "VK_WORKSPACE_BRANCH",
            "VK_AGENT_RUN_ID",
            "VK_RUN_ATTEMPT_ID",
        ]
        .iter()
        .any(|context| key.eq_ignore_ascii_case(context))
}

/// Opaque native-authority scope shared by launch observations and discovery.
/// Platform run context and the currently selected model/effort are not routes.
pub fn catalog_identity(cmd: &crate::command::CmdOverrides) -> String {
    let env: std::collections::BTreeMap<_, _> = cmd
        .env
        .as_ref()
        .into_iter()
        .flat_map(|env| env.iter())
        .filter(|(key, _)| !is_catalog_context_env(key))
        .collect();
    let ambient: std::collections::BTreeMap<_, _> = std::env::vars()
        .filter(|(key, _)| {
            let key = key.to_ascii_uppercase();
            key.starts_with("OPENCODE_")
                || key.starts_with("GEMINI_")
                || key.starts_with("GOOGLE_")
                || key.starts_with("DSH_")
                || key.starts_with("DEEPSEEK_")
                || key.starts_with("XDG_")
                || matches!(
                    key.as_str(),
                    "HOME" | "USERPROFILE" | "PATH" | "NODE_OPTIONS"
                )
        })
        .collect();
    let value = serde_json::json!({"base":cmd.base_command_override, "args":cmd.additional_params,
        "env":env, "ambient":ambient});
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

/// Invalidate observed capabilities when the installed CLI or native settings
/// change. File contents only contribute to an opaque hash; never emit them.
pub fn scoped_catalog_identity(
    dialect: AcpDialect,
    cwd: &Path,
    cmd: &crate::command::CmdOverrides,
) -> String {
    let env = |name: &str| {
        cmd.env
            .as_ref()
            .and_then(|env| env.get(name))
            .cloned()
            .or_else(|| std::env::var(name).ok())
    };
    let home = env(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_default();
    let configured = |name: &str| {
        env(name)
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                let path = if value == "~" {
                    home.clone()
                } else if let Some(rest) = value
                    .strip_prefix("~/")
                    .or_else(|| value.strip_prefix("~\\"))
                {
                    home.join(rest)
                } else {
                    PathBuf::from(value)
                };
                if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                }
            })
    };
    let mut files = Vec::new();
    let executable = match dialect {
        AcpDialect::Gemini => {
            let root = configured("GEMINI_CLI_HOME")
                .unwrap_or_else(|| home.clone())
                .join(".gemini");
            files.extend([
                root.join("settings.json"),
                root.join(".env"),
                root.join("oauth_creds.json"),
            ]);
            files.extend([
                cwd.join(".gemini/settings.json"),
                cwd.join(".gemini/.env"),
                cwd.join(".env"),
            ]);
            for key in [
                "GEMINI_CLI_SYSTEM_SETTINGS_PATH",
                "GEMINI_CLI_SYSTEM_DEFAULTS_PATH",
            ] {
                files.extend(configured(key));
            }
            "gemini"
        }
        AcpDialect::Opencode => {
            let root = configured("XDG_CONFIG_HOME")
                .unwrap_or_else(|| home.join(".config"))
                .join("opencode");
            files.extend(native_assets::opencode_config_candidates(&root, true));
            if let Some(root) = configured("OPENCODE_CONFIG_DIR") {
                files.extend(native_assets::opencode_config_candidates(&root, false));
            }
            files.extend(configured("OPENCODE_CONFIG"));
            files.push(
                configured("XDG_DATA_HOME")
                    .unwrap_or_else(|| home.join(".local/share"))
                    .join("opencode/auth.json"),
            );
            let project_root = native_assets::deepseek_project_root(cwd);
            for directory in cwd
                .ancestors()
                .take_while(|directory| directory.starts_with(project_root))
            {
                files.extend(native_assets::opencode_config_candidates(directory, false));
                files.extend(native_assets::opencode_config_candidates(
                    &directory.join(".opencode"),
                    false,
                ));
            }
            "opencode"
        }
        AcpDialect::DeepseekHarness => {
            let root = configured("DSH_HOME").unwrap_or_else(|| home.join(".dsh"));
            files.extend([
                root.join("cordis.patch.yml"),
                root.join("profiles/acp/cordis.patch.yml"),
                root.join(".credentials.yaml"),
                root.join(".env"),
                cwd.join(".env"),
            ]);
            "dsh"
        }
    };
    let identities: Vec<_> = files
        .iter()
        .map(|path| {
            let metadata = std::fs::metadata(path).ok();
            // Native settings are small. Oversized/unreadable files still use their
            // metadata and the cache TTL, without unbounded synchronous reads.
            let digest = metadata
                .as_ref()
                .filter(|meta| meta.is_file() && meta.len() <= 2 * 1024 * 1024)
                .and_then(|_| std::fs::read(path).ok())
                .map(|bytes| format!("{:x}", Sha256::digest(bytes)));
            (
                path,
                metadata.as_ref().map(std::fs::Metadata::len),
                metadata.and_then(|meta| meta.modified().ok()),
                digest,
            )
        })
        .collect();
    let program = cmd.base_command_override.as_deref().unwrap_or(executable);
    let explicit_program = cwd.join(program);
    let program_path = if explicit_program.is_file() {
        Some(explicit_program)
    } else {
        workspace_utils::shell::resolve_executable_path_blocking(program)
    };
    let executable_identity = program_path.map(|path| {
        let metadata = std::fs::metadata(&path).ok();
        (
            path,
            metadata.as_ref().map(std::fs::Metadata::len),
            metadata.and_then(|meta| meta.modified().ok()),
        )
    });
    let value = serde_json::json!({"base":catalog_identity(cmd), "files":identities, "executable":executable_identity});
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

pub const ACP_CATALOG_EVENT: &str = "acp_config_catalog";

/// A safe picker projection sent through the existing process-host observation
/// channel. No native config, credentials, transcript or workflow mounts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcpCatalogObservation {
    pub working_dir: PathBuf,
    pub scope_id: String,
    pub model_selector: ModelSelectorConfig,
}

type ObservedCatalogs = HashMap<(AcpDialect, PathBuf, String), (Instant, ModelSelectorConfig)>;
static OBSERVED_CATALOGS: LazyLock<Mutex<ObservedCatalogs>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn observe_catalog(
    dialect: AcpDialect,
    cwd: PathBuf,
    identity: String,
    options: &[proto::SessionConfigOption],
) -> Option<AcpCatalogObservation> {
    let model = select(options, "model")?;
    let effort = select(options, dialect.effort_option());
    let model_id = model.current_value.0.to_string();
    let mut seen = HashSet::new();
    let model_values: Vec<_> = values(model)
        .into_iter()
        .filter(|value| seen.insert(value.value.0.to_string()))
        .collect();
    let default_model = model_values
        .iter()
        .any(|value| value.value.0.as_ref() == model_id)
        .then(|| model_id.clone());
    let selector = ModelSelectorConfig {
        // Model ids (including DSH's JSON pair) remain opaque to the UI.
        providers: Vec::new(),
        models: model_values
            .into_iter()
            .map(|value| ModelInfo {
                id: value.value.0.to_string(),
                name: value.name.clone(),
                provider_id: None,
                // Effort belongs only to the model for which it was advertised.
                reasoning_options: if value.value.0.as_ref() == model_id {
                    effort
                        .map(|effort| {
                            values(effort)
                                .into_iter()
                                .map(|value| ReasoningOption {
                                    id: value.value.0.to_string(),
                                    label: value.name.clone(),
                                    is_default: value.value == effort.current_value,
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                },
                capabilities: Some(ModelCapabilities {
                    reasoning: if value.value.0.as_ref() != model_id {
                        CapabilitySupport::Unknown
                    } else if effort.is_some() {
                        CapabilitySupport::Supported
                    } else {
                        CapabilitySupport::Unsupported
                    },
                    // Neither provider advertises exact model input
                    // modalities here. initialize.promptCapabilities.image
                    // is transport-wide and must not enable every route.
                    ..Default::default()
                }),
            })
            .collect(),
        default_model,
        agents: select(options, "mode")
            .map(|mode| {
                values(mode)
                    .into_iter()
                    .map(|value| AgentInfo {
                        id: value.value.0.to_string(),
                        label: value.name.clone(),
                        description: value.description.clone(),
                        is_default: value.value == mode.current_value,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        permissions: vec![PermissionPolicy::Auto, PermissionPolicy::Supervised],
    };
    let observation = AcpCatalogObservation {
        working_dir: cwd,
        scope_id: identity,
        model_selector: selector,
    };
    cache_observation(dialect, &observation);
    Some(observation)
}

fn cache_observation(dialect: AcpDialect, observation: &AcpCatalogObservation) {
    if let Ok(mut catalogs) = OBSERVED_CATALOGS.lock() {
        let key = (
            dialect,
            observation.working_dir.clone(),
            observation.scope_id.clone(),
        );
        // A bounded in-memory cache, not another session/history store.
        if catalogs.len() >= 64 && !catalogs.contains_key(&key) {
            catalogs.clear();
        }
        catalogs.insert(key, (Instant::now(), observation.model_selector.clone()));
    }
}

/// Import only the matching provider's safe extension after canonical append.
/// Called in the application process, not just the isolated process host.
pub fn cache_catalog_extension(provider_id: &str, payload: &crate::runtime::AgentEventPayload) {
    let crate::runtime::AgentEventPayload::ProviderExtension {
        provider_namespace,
        provider_event,
        payload,
    } = payload
    else {
        return;
    };
    if provider_namespace != provider_id || provider_event != ACP_CATALOG_EVENT {
        return;
    }
    let dialect = match provider_id {
        "gemini" => AcpDialect::Gemini,
        "opencode" => AcpDialect::Opencode,
        "deepseek_harness" => AcpDialect::DeepseekHarness,
        _ => return,
    };
    let Ok(observation) = serde_json::from_value::<AcpCatalogObservation>(payload.clone()) else {
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
            .is_some_and(|id| {
                !observation
                    .model_selector
                    .models
                    .iter()
                    .any(|model| &model.id == id)
            })
    {
        return;
    }
    cache_observation(dialect, &observation);
}

pub(crate) fn observed_catalog(
    dialect: AcpDialect,
    cwd: &std::path::Path,
    identity: &str,
) -> Option<ModelSelectorConfig> {
    OBSERVED_CATALOGS
        .lock()
        .ok()?
        .get(&(dialect, cwd.to_path_buf(), identity.to_string()))
        .filter(|(observed, _)| observed.elapsed() < Duration::from_secs(300))
        .map(|(_, selector)| selector.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_catalog_changes_after_native_file_or_executable_update() {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path();
        let executable = cwd.join("test-agent");
        std::fs::write(&executable, "release-a").unwrap();
        let cmd = crate::command::CmdOverrides {
            base_command_override: Some(executable.to_string_lossy().into_owned()),
            env: Some(std::collections::HashMap::from([
                ("GEMINI_CLI_HOME".into(), cwd.to_string_lossy().into_owned()),
                ("DSH_HOME".into(), cwd.to_string_lossy().into_owned()),
                (
                    "OPENCODE_CONFIG_DIR".into(),
                    cwd.to_string_lossy().into_owned(),
                ),
            ])),
            ..Default::default()
        };
        std::fs::create_dir(cwd.join(".gemini")).unwrap();
        for (dialect, path) in [
            (AcpDialect::Gemini, cwd.join(".gemini/settings.json")),
            (AcpDialect::Opencode, cwd.join("opencode.jsonc")),
            (AcpDialect::DeepseekHarness, cwd.join("cordis.patch.yml")),
        ] {
            let before = scoped_catalog_identity(dialect, cwd, &cmd);
            std::fs::write(&path, "first-native-value").unwrap();
            let created = scoped_catalog_identity(dialect, cwd, &cmd);
            assert_ne!(before, created);
            // Same-size rewrites must invalidate even on coarse-mtime filesystems.
            std::fs::write(&path, "other-native-value").unwrap();
            assert_ne!(created, scoped_catalog_identity(dialect, cwd, &cmd));
            let mut context = cmd.clone();
            context
                .env
                .as_mut()
                .unwrap()
                .insert("VK_AGENT_RUN_ID".into(), "run-specific".into());
            assert_eq!(
                scoped_catalog_identity(dialect, cwd, &cmd),
                scoped_catalog_identity(dialect, cwd, &context)
            );
        }
        let before = scoped_catalog_identity(AcpDialect::Gemini, cwd, &cmd);
        std::fs::write(&executable, "release-with-a-different-size").unwrap();
        assert_ne!(
            before,
            scoped_catalog_identity(AcpDialect::Gemini, cwd, &cmd)
        );
    }

    #[test]
    fn observed_catalog_expires_without_erasing_native_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let key = (
            AcpDialect::Gemini,
            directory.path().to_path_buf(),
            "expired".to_owned(),
        );
        OBSERVED_CATALOGS.lock().unwrap().insert(
            key.clone(),
            (
                Instant::now() - Duration::from_secs(301),
                ModelSelectorConfig::default(),
            ),
        );
        assert!(observed_catalog(key.0, &key.1, &key.2).is_none());
        OBSERVED_CATALOGS.lock().unwrap().remove(&key);
    }

    fn option(id: &str, current: &str, values: &[&str]) -> proto::SessionConfigOption {
        proto::SessionConfigOption::select(
            id.to_owned(),
            id.to_owned(),
            current.to_owned(),
            values
                .iter()
                .map(|value| {
                    proto::SessionConfigSelectOption::new(value.to_string(), value.to_string())
                })
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn opaque_model_and_empty_effort_are_preserved() {
        let model = r#"["provider/name","model/name"]"#;
        let options = vec![
            option("model", model, &[model]),
            option("reasoning_effort", "", &["", "high"]),
        ];
        assert!(validate_selection(&options, "model", model).is_ok());
        assert!(validate_selection(&options, "reasoning_effort", "").is_ok());
        assert!(validate_selection(&options, "reasoning_effort", "max").is_err());
        assert!(validate_selection(&options, "effort", "high").is_err());
    }

    #[test]
    fn runtime_configuration_preserves_empty_effort_and_excludes_private_options() {
        let model = r#"["provider/name","model/name"]"#;
        let observation = observed_configuration(
            AcpDialect::DeepseekHarness,
            &[
                option("model", model, &[model]),
                option("reasoning_effort", "", &["", "high"]),
                option("private_api_key", "never-project", &["never-project"]),
            ],
        );
        assert_eq!(observation.model_id.as_deref(), Some(model));
        assert_eq!(observation.reasoning_id.as_deref(), Some(""));
        assert!(
            !serde_json::to_string(&observation)
                .unwrap()
                .contains("never-project")
        );
    }

    #[test]
    fn gemini_catalog_uses_native_models_modes_and_never_invents_thinking_or_image_support() {
        let directory = tempfile::tempdir().unwrap();
        let state = LegacySessionConfiguration {
            models: Some(
                serde_json::from_value(serde_json::json!({
                    "currentModelId": "native/model", "availableModels": [
                        {"modelId":"native/model", "name":"Native model"},
                        {"modelId":"future/model", "name":"Future model"}
                    ]
                }))
                .unwrap(),
            ),
            modes: Some(
                serde_json::from_value(serde_json::json!({
                    "currentModeId":"auto_edit", "availableModes":[
                        {"id":"default", "name":"Default"},
                        {"id":"auto_edit", "name":"Auto edit"},
                        {"id":"plan", "name":"Plan"}
                    ]
                }))
                .unwrap(),
            ),
        };
        let identity = catalog_identity(&crate::command::CmdOverrides::default());
        let observation =
            observe_legacy_catalog(directory.path().to_owned(), identity.clone(), &state).unwrap();
        assert_eq!(
            observation.model_selector.default_model.as_deref(),
            Some("native/model")
        );
        assert_eq!(observation.model_selector.agents[1].id, "auto_edit");
        assert!(observation.model_selector.agents[1].is_default);
        assert!(observation.model_selector.permissions.is_empty());
        for model in &observation.model_selector.models {
            assert!(model.reasoning_options.is_empty());
            let capabilities = model.capabilities.as_ref().unwrap();
            assert_eq!(capabilities.reasoning, CapabilitySupport::Unsupported);
            assert!(capabilities.input_modalities.is_empty());
        }
        let extension = crate::runtime::AgentEventPayload::ProviderExtension {
            provider_namespace: "gemini".into(),
            provider_event: ACP_CATALOG_EVENT.into(),
            payload: serde_json::to_value(&observation).unwrap(),
        };
        cache_catalog_extension("gemini", &extension);
        assert_eq!(
            observed_catalog(AcpDialect::Gemini, directory.path(), &identity)
                .unwrap()
                .models
                .len(),
            2
        );
    }

    #[test]
    fn dsh_requires_advertised_resume_and_never_falls_back_to_load() {
        let mut capabilities = proto::AgentCapabilities::default();
        capabilities.load_session = true;
        assert_eq!(
            AcpDialect::Opencode.restore_method(&capabilities).unwrap(),
            NativeRestoreMethod::Load
        );
        assert!(
            AcpDialect::DeepseekHarness
                .restore_method(&capabilities)
                .is_err()
        );
        capabilities.session_capabilities.resume = Some(proto::SessionResumeCapabilities::new());
        assert_eq!(
            AcpDialect::DeepseekHarness
                .restore_method(&capabilities)
                .unwrap(),
            NativeRestoreMethod::Resume
        );
        assert_eq!(
            AcpDialect::Opencode.restore_method(&capabilities).unwrap(),
            NativeRestoreMethod::Resume
        );
    }

    #[test]
    fn reasoning_options_do_not_leak_to_other_models() {
        let cwd = PathBuf::from("catalog-test-root");
        let _ = observe_catalog(
            AcpDialect::Opencode,
            cwd.clone(),
            "profile-a".to_string(),
            &[
                option("model", "p/a", &["p/a", "p/b"]),
                option("effort", "high", &["low", "high"]),
            ],
        );
        let catalog = observed_catalog(AcpDialect::Opencode, &cwd, "profile-a").unwrap();
        assert!(catalog.providers.is_empty());
        assert_eq!(catalog.models[0].reasoning_options.len(), 2);
        assert!(catalog.models[1].reasoning_options.is_empty());
        assert!(observed_catalog(AcpDialect::Opencode, &cwd, "profile-b").is_none());
    }

    #[test]
    fn catalog_scope_ignores_platform_context_but_isolates_native_routes() {
        let baseline = crate::command::CmdOverrides::default();
        let identity = catalog_identity(&baseline);
        let mut context = baseline.clone();
        context.env = Some(std::collections::HashMap::from([
            ("VK_AGENT_RUN_ID".into(), "first-run".into()),
            ("VK_RUN_ATTEMPT_ID".into(), "first-attempt".into()),
            ("VK_WORKSPACE_ID".into(), "workspace".into()),
            ("VK_WORKSPACE_BRANCH".into(), "auto-first-branch".into()),
            (
                crate::workflow_mcp::TOKEN_ENV.into(),
                "private-workflow-token".into(),
            ),
            (crate::workflow_mcp::SESSION_ID_ENV.into(), "session".into()),
            (
                crate::workflow_mcp::BACKEND_URL_ENV.into(),
                "http://127.0.0.1:9999".into(),
            ),
            (
                crate::workflow_mcp::EXECUTABLE_ENV.into(),
                "workflow-mcp".into(),
            ),
        ]));
        assert_eq!(catalog_identity(&context), identity);
        context
            .env
            .as_mut()
            .unwrap()
            .insert("VK_AGENT_RUN_ID".into(), "next-run".into());
        context
            .env
            .as_mut()
            .unwrap()
            .insert("VK_WORKSPACE_BRANCH".into(), "auto-next-branch".into());
        assert_eq!(catalog_identity(&context), identity);
        assert_eq!(identity.len(), 64);
        assert!(!identity.contains("private-workflow-token"));

        let mut command = baseline.clone();
        command.base_command_override = Some("custom-opencode".into());
        assert_ne!(catalog_identity(&command), identity);
        command = baseline.clone();
        command.additional_params = Some(vec!["--custom-profile".into()]);
        assert_ne!(catalog_identity(&command), identity);
        for key in [
            "OPENCODE_CONFIG",
            "DSH_HOME",
            "XDG_CONFIG_HOME",
            "HOME",
            "CUSTOM_MODEL_ROUTE",
        ] {
            let mut routed = baseline.clone();
            routed.env = Some(std::collections::HashMap::from([(
                key.into(),
                "native-route-a".into(),
            )]));
            let route_a = catalog_identity(&routed);
            assert_ne!(route_a, identity, "{key}");
            routed
                .env
                .as_mut()
                .unwrap()
                .insert(key.into(), "native-route-b".into());
            assert_ne!(catalog_identity(&routed), route_a, "{key}");
        }
    }

    #[test]
    fn model_switch_replaces_catalog_and_never_copies_previous_effort() {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().to_path_buf();
        let identity = catalog_identity(&crate::command::CmdOverrides::default());
        let _ = observe_catalog(
            AcpDialect::Opencode,
            cwd.clone(),
            identity.clone(),
            &[
                option("model", "p/a", &["p/a", "p/b"]),
                option("effort", "high", &["high"]),
            ],
        );
        let _ = observe_catalog(
            AcpDialect::Opencode,
            cwd.clone(),
            identity.clone(),
            &[
                option("model", "p/b", &["p/a", "p/b", "p/b"]),
                option("effort", "", &[""]),
                option(
                    "private_api_key",
                    "native-private-value",
                    &["native-private-value"],
                ),
            ],
        );
        let catalog = observed_catalog(AcpDialect::Opencode, &cwd, &identity).unwrap();
        assert_eq!(catalog.default_model.as_deref(), Some("p/b"));
        assert_eq!(catalog.models.len(), 2);
        assert!(catalog.models[0].reasoning_options.is_empty());
        assert_eq!(catalog.models[1].reasoning_options[0].id, "");
        assert!(
            !serde_json::to_string(&catalog)
                .unwrap()
                .contains("native-private-value")
        );
    }

    #[test]
    fn serialized_observation_imports_only_the_matching_provider_scope() {
        let directory = tempfile::tempdir().unwrap();
        let identity = catalog_identity(&crate::command::CmdOverrides::default());
        let observation = AcpCatalogObservation {
            working_dir: directory.path().to_path_buf(),
            scope_id: identity.clone(),
            model_selector: ModelSelectorConfig {
                models: vec![ModelInfo {
                    id: "opaque/model".into(),
                    name: "Model".into(),
                    provider_id: None,
                    reasoning_options: vec![ReasoningOption {
                        id: "".into(),
                        label: "No effort".into(),
                        is_default: true,
                    }],
                    ..Default::default()
                }],
                default_model: Some("opaque/model".into()),
                ..Default::default()
            },
        };
        let extension = crate::runtime::AgentEventPayload::ProviderExtension {
            provider_namespace: "deepseek_harness".into(),
            provider_event: ACP_CATALOG_EVENT.into(),
            payload: serde_json::to_value(&observation).unwrap(),
        };
        let transported = serde_json::from_str::<crate::runtime::AgentEventPayload>(
            &serde_json::to_string(&extension).unwrap(),
        )
        .unwrap();
        assert!(
            observed_catalog(AcpDialect::DeepseekHarness, directory.path(), &identity).is_none()
        );
        cache_catalog_extension("opencode", &transported);
        assert!(
            observed_catalog(AcpDialect::DeepseekHarness, directory.path(), &identity).is_none()
        );
        cache_catalog_extension("deepseek_harness", &transported);
        assert_eq!(
            observed_catalog(AcpDialect::DeepseekHarness, directory.path(), &identity)
                .unwrap()
                .models[0]
                .reasoning_options[0]
                .id,
            ""
        );
        assert!(observed_catalog(AcpDialect::Opencode, directory.path(), &identity).is_none());
        assert!(
            observed_catalog(
                AcpDialect::DeepseekHarness,
                directory.path(),
                &"f".repeat(64)
            )
            .is_none()
        );
    }
}
