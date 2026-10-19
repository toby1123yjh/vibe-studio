//! Adapter-owned native settings management for direct agent providers.
//!
//! Native provider files remain authoritative. Discovery never creates or
//! changes them; mutations use descriptor-owned paths, full-file revisions,
//! targeted document edits, atomic replacement, and operation-level rollback.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use chrono::{DateTime, Utc};
use jsonc_parser::ParseOptions;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;
use toml_edit::{DocumentMut, Item};
use ts_rs::TS;
use uuid::Uuid;

use crate::{
    agent_tools::native_assets,
    executors::{
        codex::codex_home_for, oh_my_pi::oh_my_pi_agent_root_for, provider_adapter::DirectProvider,
    },
    mcp_config::update_jsonc_content,
    profile::ExecutorProfileId,
};

mod native_model_defaults;
pub use native_model_defaults::native_model_defaults_for;

pub const SETTINGS_PROFILE_STORE_VERSION: u16 = 1;

// Profile mutations are synchronous and infrequent. Serializing the complete
// read-check-write transaction prevents a confirmed copy from overwriting a
// concurrent profile edit after its source/target revisions were verified.
static PROFILE_STORE_MUTATION_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum AgentSettingsProvider {
    Codex,
    ClaudeCode,
    Gemini,
    OhMyPi,
    Opencode,
    DeepseekHarness,
}

impl AgentSettingsProvider {
    pub const ALL: [Self; 6] = [
        Self::Codex,
        Self::ClaudeCode,
        Self::Gemini,
        Self::OhMyPi,
        Self::Opencode,
        Self::DeepseekHarness,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude_code",
            Self::Gemini => "gemini",
            Self::OhMyPi => "oh_my_pi",
            Self::Opencode => "opencode",
            Self::DeepseekHarness => "deepseek_harness",
        }
    }

    const fn executable(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude",
            Self::Gemini => "gemini",
            Self::OhMyPi => "omp",
            Self::Opencode => "opencode",
            Self::DeepseekHarness => "dsh",
        }
    }
}

impl From<DirectProvider> for AgentSettingsProvider {
    fn from(value: DirectProvider) -> Self {
        match value {
            DirectProvider::Codex => Self::Codex,
            DirectProvider::ClaudeCode => Self::ClaudeCode,
            DirectProvider::Gemini => Self::Gemini,
            DirectProvider::OhMyPi => Self::OhMyPi,
            DirectProvider::Opencode => Self::Opencode,
            DirectProvider::DeepseekHarness => Self::DeepseekHarness,
        }
    }
}

impl From<AgentSettingsProvider> for DirectProvider {
    fn from(value: AgentSettingsProvider) -> Self {
        match value {
            AgentSettingsProvider::Codex => Self::Codex,
            AgentSettingsProvider::ClaudeCode => Self::ClaudeCode,
            AgentSettingsProvider::Gemini => Self::Gemini,
            AgentSettingsProvider::OhMyPi => Self::OhMyPi,
            AgentSettingsProvider::Opencode => Self::Opencode,
            AgentSettingsProvider::DeepseekHarness => Self::DeepseekHarness,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum SettingScope {
    User,
    Project,
}

impl SettingScope {
    const fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum SettingSection {
    General,
    PermissionsSandbox,
    Instructions,
    Environment,
    ProviderSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum SettingValueType {
    String,
    Boolean,
    Number,
    StringList,
    StringMap,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum SettingControl {
    Text,
    Textarea,
    Toggle,
    Select,
    Number,
    StringList,
    KeyValue,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum SettingActivation {
    Immediate,
    NextSession,
    ExplicitRestart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum NativeConfigFormat {
    Toml,
    Json,
    Jsonc,
    Yaml,
    Dotenv,
    Text,
    Opaque,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum NativeParseStatus {
    Missing,
    Parsed,
    Invalid,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
pub struct SettingKey {
    pub namespace: String,
    pub name: String,
}

impl SettingKey {
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }

    pub fn id(&self) -> String {
        format!("{}.{}", self.namespace, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SettingOption {
    pub value: Value,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
pub struct SettingValidation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SettingCapabilities {
    pub readable: bool,
    pub writable: bool,
    pub resettable: bool,
    pub profile_storable: bool,
    pub run_override: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct NativeSettingLocation {
    pub file_id: String,
    pub scope: SettingScope,
    pub native_path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SettingDescriptor {
    pub key: SettingKey,
    pub section: SettingSection,
    pub label: String,
    pub description: String,
    pub value_type: SettingValueType,
    pub control: SettingControl,
    #[serde(default)]
    pub options: Vec<SettingOption>,
    #[serde(default)]
    pub validation: SettingValidation,
    pub supported_scopes: Vec<SettingScope>,
    pub capabilities: SettingCapabilities,
    pub native_locations: Vec<NativeSettingLocation>,
    pub activation: SettingActivation,
    pub sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SettingsCapabilities {
    pub readable: bool,
    pub native_writable: bool,
    pub profile_storage: bool,
    pub per_run_overrides: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct NativeConfigFile {
    pub file_id: String,
    pub format: NativeConfigFormat,
    pub scope: SettingScope,
    pub exists: bool,
    pub parse_status: NativeParseStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    pub writable: bool,
    #[serde(default)]
    pub managed_setting_keys: Vec<SettingKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SettingSourceValue {
    pub source: String,
    pub scope: SettingScope,
    pub file_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    pub configured: bool,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct EffectiveSetting {
    pub key: SettingKey,
    #[serde(default)]
    pub sources: Vec<SettingSourceValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_source: Option<String>,
    pub configured: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct UnknownNativeNode {
    pub file_id: String,
    pub scope: SettingScope,
    pub field_path: String,
    pub value_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AgentSettingIssue {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_key: Option<String>,
    pub recovery: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SettingsSnapshot {
    pub provider: AgentSettingsProvider,
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_version: Option<String>,
    pub schema_revision: String,
    pub capabilities: SettingsCapabilities,
    pub descriptors: Vec<SettingDescriptor>,
    pub native_files: Vec<NativeConfigFile>,
    pub effective_settings: Vec<EffectiveSetting>,
    #[serde(default)]
    pub unknown_native_nodes: Vec<UnknownNativeNode>,
    #[serde(default)]
    pub limitations: Vec<String>,
    #[serde(default)]
    pub errors: Vec<AgentSettingIssue>,
}

impl SettingsSnapshot {
    fn with_sensitive_values_redacted(mut self) -> Self {
        let sensitive_keys: BTreeSet<_> = self
            .descriptors
            .iter()
            .filter(|descriptor| descriptor.sensitive)
            .map(|descriptor| descriptor.key.id())
            .collect();
        for setting in &mut self.effective_settings {
            if sensitive_keys.contains(&setting.key.id()) {
                setting.effective_value = None;
                for source in &mut setting.sources {
                    source.value = None;
                }
            }
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AgentSettingsInventory {
    pub providers: Vec<SettingsSnapshot>,
    #[serde(default)]
    pub errors: Vec<AgentSettingsProviderError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AgentSettingsProviderError {
    pub provider: AgentSettingsProvider,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum SettingOperation {
    Preserve {
        key: SettingKey,
        scope: SettingScope,
    },
    Replace {
        key: SettingKey,
        scope: SettingScope,
        value: Value,
    },
    Clear {
        key: SettingKey,
        scope: SettingScope,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SettingsPatch {
    pub provider: AgentSettingsProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    pub expected_file_revisions: BTreeMap<String, String>,
    pub operations: Vec<SettingOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct NativeFileDiff {
    pub file_id: String,
    pub format: NativeConfigFormat,
    pub scope: SettingScope,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SettingsDiff {
    pub provider: AgentSettingsProvider,
    pub files: Vec<NativeFileDiff>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ApplySettingsRequest {
    pub patch: SettingsPatch,
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RevealAgentSettingRequest {
    pub provider: AgentSettingsProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    pub key: SettingKey,
    pub scope: SettingScope,
    pub expected_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RevealAgentSettingResponse {
    pub key: SettingKey,
    pub scope: SettingScope,
    pub value: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeFilePatch {
    pub provider: AgentSettingsProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    pub file_id: String,
    pub expected_revision: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConfigProfile {
    pub id: Uuid,
    pub provider: AgentSettingsProvider,
    pub executor_profile: ExecutorProfileId,
    pub name: String,
    pub schema_version: u16,
    #[serde(default)]
    pub setting_overrides: BTreeMap<String, Value>,
    #[serde(default)]
    pub provider_extensions: BTreeMap<String, Value>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub custom_args: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

/// Browser-safe projection of a stored configuration profile.
///
/// Credential values and provider-owned extension values intentionally never
/// cross this boundary. Ordinary settings, Environment, and custom arguments
/// remain visible because they are user-managed profile configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConfigProfileView {
    pub id: Uuid,
    pub provider: AgentSettingsProvider,
    pub executor_profile: ExecutorProfileId,
    pub name: String,
    pub schema_version: u16,
    #[serde(default)]
    pub setting_overrides: BTreeMap<String, Value>,
    #[serde(default)]
    pub configured_credential_keys: Vec<String>,
    #[serde(default)]
    pub provider_extension_keys: Vec<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub custom_args: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SaveConfigProfileRequest {
    pub profile: ConfigProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct UpdateConfigProfileRequest {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub custom_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DeleteConfigProfileRequest {
    pub id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DuplicateConfigProfileRequest {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CopyProfilePreviewRequest {
    pub id: Uuid,
    pub target_provider: AgentSettingsProvider,
    pub target_executor_profile: ExecutorProfileId,
    pub target_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_profile_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ProfileCopyPreview {
    pub profile: ConfigProfileView,
    #[serde(default)]
    pub added_keys: Vec<String>,
    #[serde(default)]
    pub overwritten_keys: Vec<String>,
    #[serde(default)]
    pub skipped_keys: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub source_updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CopyConfigProfileRequest {
    pub preview: CopyProfilePreviewRequest,
    pub expected_source_updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_target_updated_at: Option<DateTime<Utc>>,
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ProfileApplyPreviewRequest {
    pub id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    pub scope: SettingScope,
    pub expected_file_revisions: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ApplyConfigProfileRequest {
    pub preview: ProfileApplyPreviewRequest,
    pub confirmed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum AgentSettingErrorCode {
    InvalidRequest,
    InvalidConfiguration,
    NotFound,
    StaleRevision,
    Unsupported,
    UnsafePath,
    ValidationFailed,
    Io,
    RollbackFailed,
    VerificationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AgentSettingOperationError {
    pub code: AgentSettingErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentSettingsProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting_key: Option<String>,
    pub recovery: String,
}

#[derive(Debug, Error)]
pub enum AgentSettingError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("invalid native configuration: {0}")]
    InvalidConfiguration(String),
    #[error("setting or profile not found: {0}")]
    NotFound(String),
    #[error("native content changed since it was read")]
    StaleRevision,
    #[error("operation is unsupported: {0}")]
    Unsupported(String),
    #[error("unsafe path: {0}")]
    UnsafePath(String),
    #[error("validation failed: {0}")]
    ValidationFailed(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("rollback failed: {0}")]
    RollbackFailed(String),
    #[error("write verification failed: {0}")]
    VerificationFailed(String),
}

impl AgentSettingError {
    pub fn operation_error(
        &self,
        provider: Option<AgentSettingsProvider>,
        file_id: Option<String>,
        setting_key: Option<String>,
    ) -> AgentSettingOperationError {
        let (code, recovery) = match self {
            Self::InvalidRequest(_) => (
                AgentSettingErrorCode::InvalidRequest,
                "Review the request and try again.",
            ),
            Self::InvalidConfiguration(_) => (
                AgentSettingErrorCode::InvalidConfiguration,
                "Fix the native file syntax, then refresh.",
            ),
            Self::NotFound(_) => (
                AgentSettingErrorCode::NotFound,
                "Refresh settings and choose an existing item.",
            ),
            Self::StaleRevision => (
                AgentSettingErrorCode::StaleRevision,
                "Refresh, review the external changes, and retry.",
            ),
            Self::Unsupported(_) => (
                AgentSettingErrorCode::Unsupported,
                "Use a capability supported by this installed provider.",
            ),
            Self::UnsafePath(_) => (
                AgentSettingErrorCode::UnsafePath,
                "Choose a file or project inside the advertised provider roots.",
            ),
            Self::ValidationFailed(_) => (
                AgentSettingErrorCode::ValidationFailed,
                "Correct the highlighted value and preview again.",
            ),
            Self::Io(_) => (
                AgentSettingErrorCode::Io,
                "Check file permissions and retry.",
            ),
            Self::RollbackFailed(_) => (
                AgentSettingErrorCode::RollbackFailed,
                "Inspect the listed native files before retrying.",
            ),
            Self::VerificationFailed(_) => (
                AgentSettingErrorCode::VerificationFailed,
                "Refresh and inspect the provider file before retrying.",
            ),
        };
        AgentSettingOperationError {
            code,
            message: self.to_string(),
            provider,
            file_id,
            setting_key,
            recovery: recovery.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigProfileStore {
    version: u16,
    #[serde(default)]
    profiles: Vec<ConfigProfile>,
}

struct PreparedProfileCopy {
    profile: ConfigProfile,
    added_keys: Vec<String>,
    overwritten_keys: Vec<String>,
    skipped_keys: Vec<String>,
    warnings: Vec<String>,
    source_updated_at: DateTime<Utc>,
    target_updated_at: Option<DateTime<Utc>>,
}

impl Default for ConfigProfileStore {
    fn default() -> Self {
        Self {
            version: SETTINGS_PROFILE_STORE_VERSION,
            profiles: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentSettingsService {
    home_dir: PathBuf,
    profile_store_path: PathBuf,
}

impl AgentSettingsService {
    pub fn new(home_dir: PathBuf, profile_store_path: PathBuf) -> Self {
        Self {
            home_dir,
            profile_store_path,
        }
    }

    pub fn from_system() -> Result<Self, AgentSettingError> {
        let home_dir = dirs::home_dir().ok_or_else(|| {
            AgentSettingError::InvalidRequest(
                "could not determine the user home directory".to_string(),
            )
        })?;
        Ok(Self::new(
            home_dir,
            workspace_utils::assets::asset_dir().join("agent-settings/profiles-v1.json"),
        ))
    }

    pub fn manager(
        &self,
        provider: AgentSettingsProvider,
        project_path: Option<&Path>,
    ) -> ProviderSettingsManager {
        DirectProvider::from(provider)
            .settings_manager(self.home_dir.clone(), project_path.map(Path::to_path_buf))
    }

    pub fn discover(
        &self,
        provider: Option<AgentSettingsProvider>,
        project_path: Option<&Path>,
    ) -> AgentSettingsInventory {
        let requested: Vec<_> = provider
            .map(|provider| vec![provider])
            .unwrap_or_else(|| AgentSettingsProvider::ALL.to_vec());
        let mut providers = Vec::new();
        let mut errors = Vec::new();
        for provider in requested {
            match self.manager(provider, project_path).discover() {
                Ok(snapshot) => providers.push(snapshot),
                Err(error) => {
                    errors.push(AgentSettingsProviderError {
                        provider,
                        message: error.to_string(),
                    });
                    providers.push(self.manager(provider, project_path).error_snapshot(&error));
                }
            }
        }
        AgentSettingsInventory { providers, errors }
    }

    pub fn diff(&self, patch: &SettingsPatch) -> Result<SettingsDiff, AgentSettingError> {
        self.manager(patch.provider, patch.project_path.as_deref().map(Path::new))
            .diff(patch)
    }

    pub fn apply(
        &self,
        request: ApplySettingsRequest,
    ) -> Result<SettingsSnapshot, AgentSettingError> {
        if !request.confirmed {
            return Err(AgentSettingError::InvalidRequest(
                "settings apply requires explicit diff confirmation".to_string(),
            ));
        }
        self.manager(
            request.patch.provider,
            request.patch.project_path.as_deref().map(Path::new),
        )
        .apply(&request.patch)
    }

    pub fn reveal(
        &self,
        request: RevealAgentSettingRequest,
    ) -> Result<RevealAgentSettingResponse, AgentSettingError> {
        self.manager(
            request.provider,
            request.project_path.as_deref().map(Path::new),
        )
        .reveal(&request.key, request.scope, &request.expected_revision)
    }

    pub fn list_profiles(
        &self,
        provider: Option<AgentSettingsProvider>,
    ) -> Result<Vec<ConfigProfileView>, AgentSettingError> {
        let mut profiles = self.load_profile_store()?.profiles;
        if let Some(provider) = provider {
            profiles.retain(|profile| profile.provider == provider);
        }
        profiles.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        Ok(profiles
            .into_iter()
            .map(|profile| self.profile_view(profile))
            .collect())
    }

    pub fn save_profile(
        &self,
        request: SaveConfigProfileRequest,
    ) -> Result<ConfigProfileView, AgentSettingError> {
        let _guard = lock_profile_store_mutation()?;
        let profile = self.persist_profile_unlocked(request.profile)?;
        Ok(self.profile_view(profile))
    }

    fn persist_profile_unlocked(
        &self,
        mut profile: ConfigProfile,
    ) -> Result<ConfigProfile, AgentSettingError> {
        let descriptors = self.manager(profile.provider, None).descriptors();
        // Profile environment is its own canonical, provider-neutral field.
        // Do not retain a second copy in setting_overrides; profile Apply
        // projects it to common.environment only when the target Adapter owns
        // that native setting.
        profile.setting_overrides.remove("common.environment");
        validate_profile(&profile, &descriptors)?;
        if let Some(descriptor) = descriptors
            .iter()
            .find(|descriptor| descriptor.key.id() == "common.environment")
        {
            validate_setting_value(descriptor, &profile_environment_value(&profile))?;
        }
        profile.schema_version = SETTINGS_PROFILE_STORE_VERSION;
        profile.updated_at = Utc::now();
        let mut store = self.load_profile_store()?;
        if let Some(existing) = store.profiles.iter_mut().find(|item| item.id == profile.id) {
            *existing = profile.clone();
        } else {
            store.profiles.push(profile.clone());
        }
        self.save_profile_store(&store)?;
        Ok(profile)
    }

    pub fn update_profile(
        &self,
        request: UpdateConfigProfileRequest,
    ) -> Result<ConfigProfileView, AgentSettingError> {
        let _guard = lock_profile_store_mutation()?;
        let mut store = self.load_profile_store()?;
        let profile = store
            .profiles
            .iter_mut()
            .find(|profile| profile.id == request.id)
            .ok_or_else(|| AgentSettingError::NotFound(request.id.to_string()))?;
        profile.name = request.name;
        profile.environment = request.environment;
        profile.custom_args = request.custom_args;

        if profile.name.trim().is_empty() {
            return Err(AgentSettingError::ValidationFailed(
                "profile name cannot be empty".to_string(),
            ));
        }
        let descriptors = self.manager(profile.provider, None).descriptors();
        if let Some(descriptor) = descriptors
            .iter()
            .find(|descriptor| descriptor.key.id() == "common.environment")
        {
            validate_setting_value(descriptor, &profile_environment_value(profile))?;
        }
        profile.updated_at = Utc::now();
        let updated = profile.clone();
        self.save_profile_store(&store)?;
        Ok(self.profile_view(updated))
    }

    pub fn delete_profile(
        &self,
        request: DeleteConfigProfileRequest,
    ) -> Result<(), AgentSettingError> {
        let _guard = lock_profile_store_mutation()?;
        let mut store = self.load_profile_store()?;
        let before = store.profiles.len();
        store.profiles.retain(|profile| profile.id != request.id);
        if before == store.profiles.len() {
            return Err(AgentSettingError::NotFound(request.id.to_string()));
        }
        self.save_profile_store(&store)
    }

    pub fn duplicate_profile(
        &self,
        request: DuplicateConfigProfileRequest,
    ) -> Result<ConfigProfileView, AgentSettingError> {
        let _guard = lock_profile_store_mutation()?;
        let store = self.load_profile_store()?;
        let mut profile = store
            .profiles
            .iter()
            .find(|profile| profile.id == request.id)
            .cloned()
            .ok_or_else(|| AgentSettingError::NotFound(request.id.to_string()))?;
        profile.id = Uuid::new_v4();
        profile.name = request.name;
        let profile = self.persist_profile_unlocked(profile)?;
        Ok(self.profile_view(profile))
    }

    pub fn copy_profile_preview(
        &self,
        request: CopyProfilePreviewRequest,
    ) -> Result<ProfileCopyPreview, AgentSettingError> {
        let store = self.load_profile_store()?;
        let prepared = self.prepare_profile_copy(&store, &request)?;
        Ok(ProfileCopyPreview {
            profile: self.profile_view(prepared.profile),
            added_keys: prepared.added_keys,
            overwritten_keys: prepared.overwritten_keys,
            skipped_keys: prepared.skipped_keys,
            warnings: prepared.warnings,
            source_updated_at: prepared.source_updated_at,
            target_updated_at: prepared.target_updated_at,
        })
    }

    pub fn copy_profile(
        &self,
        request: CopyConfigProfileRequest,
    ) -> Result<ConfigProfileView, AgentSettingError> {
        if !request.confirmed {
            return Err(AgentSettingError::InvalidRequest(
                "profile copy requires explicit target diff confirmation".to_string(),
            ));
        }
        let _guard = lock_profile_store_mutation()?;
        let mut store = self.load_profile_store()?;
        let mut prepared = self.prepare_profile_copy(&store, &request.preview)?;
        if prepared.source_updated_at != request.expected_source_updated_at
            || prepared.target_updated_at != request.expected_target_updated_at
        {
            return Err(AgentSettingError::StaleRevision);
        }

        prepared.profile.updated_at = Utc::now();
        if let Some(target_profile_id) = request.preview.target_profile_id {
            let target = store
                .profiles
                .iter_mut()
                .find(|profile| profile.id == target_profile_id)
                .ok_or_else(|| AgentSettingError::NotFound(target_profile_id.to_string()))?;
            *target = prepared.profile.clone();
        } else {
            store.profiles.push(prepared.profile.clone());
        }
        self.save_profile_store(&store)?;
        Ok(self.profile_view(prepared.profile))
    }

    fn prepare_profile_copy(
        &self,
        store: &ConfigProfileStore,
        request: &CopyProfilePreviewRequest,
    ) -> Result<PreparedProfileCopy, AgentSettingError> {
        let source = store
            .profiles
            .iter()
            .find(|profile| profile.id == request.id)
            .ok_or_else(|| AgentSettingError::NotFound(request.id.to_string()))?;
        if source.provider == request.target_provider {
            return Err(AgentSettingError::ValidationFailed(
                "cross-provider copy requires a different target provider".to_string(),
            ));
        }

        let target = request
            .target_profile_id
            .map(|target_profile_id| {
                store
                    .profiles
                    .iter()
                    .find(|profile| profile.id == target_profile_id)
                    .ok_or_else(|| AgentSettingError::NotFound(target_profile_id.to_string()))
            })
            .transpose()?;
        if let Some(target) = target {
            if target.provider != request.target_provider {
                return Err(AgentSettingError::ValidationFailed(
                    "target profile does not belong to the selected provider".to_string(),
                ));
            }
        } else if request.target_name.trim().is_empty() {
            return Err(AgentSettingError::ValidationFailed(
                "target profile name cannot be empty".to_string(),
            ));
        }
        let target_executor_profile = target
            .map(|profile| &profile.executor_profile)
            .unwrap_or(&request.target_executor_profile);
        if DirectProvider::from_base_agent(target_executor_profile.executor.clone())
            != Some(DirectProvider::from(request.target_provider))
        {
            return Err(AgentSettingError::ValidationFailed(
                "target executor profile does not belong to the selected provider".to_string(),
            ));
        }

        let target_descriptors = self.manager(request.target_provider, None).descriptors();
        let target_by_key: BTreeMap<_, _> = target_descriptors
            .iter()
            .filter(|descriptor| descriptor.capabilities.profile_storable && !descriptor.sensitive)
            .map(|descriptor| (descriptor.key.id(), descriptor))
            .collect();
        let mut profile = target.cloned().unwrap_or_else(|| ConfigProfile {
            id: Uuid::new_v4(),
            provider: request.target_provider,
            executor_profile: request.target_executor_profile.clone(),
            name: request.target_name.trim().to_string(),
            schema_version: SETTINGS_PROFILE_STORE_VERSION,
            setting_overrides: BTreeMap::new(),
            provider_extensions: BTreeMap::new(),
            environment: BTreeMap::new(),
            custom_args: Vec::new(),
            updated_at: Utc::now(),
        });
        let mut added_keys = Vec::new();
        let mut overwritten_keys = Vec::new();
        let mut skipped_keys = Vec::new();
        let mut warnings = Vec::new();

        for (key, value) in &source.setting_overrides {
            let Some(descriptor) = target_by_key
                .get(key)
                .filter(|_| key.starts_with("common."))
            else {
                skipped_keys.push(key.clone());
                continue;
            };
            if validate_setting_value(descriptor, value).is_err() {
                skipped_keys.push(key.clone());
                warnings.push(format!(
                    "{key} was skipped because the target Adapter rejected its value."
                ));
                continue;
            }
            match profile.setting_overrides.get(key) {
                None => added_keys.push(key.clone()),
                Some(existing) if existing != value => overwritten_keys.push(key.clone()),
                Some(_) => {}
            }
            profile.setting_overrides.insert(key.clone(), value.clone());
        }
        skipped_keys.extend(source.provider_extensions.keys().cloned());

        classify_replaced_profile_field(
            "profile.environment",
            profile.environment != source.environment,
            profile.environment.is_empty(),
            source.environment.is_empty(),
            &mut added_keys,
            &mut overwritten_keys,
        );
        if profile.environment != source.environment && !profile.environment.is_empty() {
            warnings.push(
                "Source Environment will replace the existing target Profile Environment."
                    .to_string(),
            );
        }
        profile.environment = source.environment.clone();

        classify_replaced_profile_field(
            "profile.custom_args",
            profile.custom_args != source.custom_args,
            profile.custom_args.is_empty(),
            source.custom_args.is_empty(),
            &mut added_keys,
            &mut overwritten_keys,
        );
        if profile.custom_args != source.custom_args && !profile.custom_args.is_empty() {
            warnings.push(
                "Source custom arguments will replace the existing target Profile custom arguments."
                    .to_string(),
            );
        }
        profile.custom_args = source.custom_args.clone();

        if let Some(descriptor) = target_descriptors
            .iter()
            .find(|descriptor| descriptor.key.id() == "common.environment")
        {
            validate_setting_value(descriptor, &profile_environment_value(&profile))?;
        }
        if !skipped_keys.is_empty() {
            warnings.push(
                "Provider-specific, credential, extension, or unsupported settings will be skipped."
                    .to_string(),
            );
        }
        if target.is_some() {
            warnings.push(
                "Target-only settings, credentials, unknown fields, and Provider extensions will be preserved."
                    .to_string(),
            );
        } else {
            validate_profile(&profile, &target_descriptors)?;
        }
        added_keys.sort();
        added_keys.dedup();
        overwritten_keys.sort();
        overwritten_keys.dedup();
        skipped_keys.sort();
        skipped_keys.dedup();

        Ok(PreparedProfileCopy {
            profile,
            added_keys,
            overwritten_keys,
            skipped_keys,
            warnings,
            source_updated_at: source.updated_at,
            target_updated_at: target.map(|profile| profile.updated_at),
        })
    }

    pub fn preview_profile_apply(
        &self,
        request: &ProfileApplyPreviewRequest,
    ) -> Result<SettingsDiff, AgentSettingError> {
        let profile = self
            .load_profile_store()?
            .profiles
            .into_iter()
            .find(|profile| profile.id == request.id)
            .ok_or_else(|| AgentSettingError::NotFound(request.id.to_string()))?;
        let manager = self.manager(
            profile.provider,
            request.project_path.as_deref().map(Path::new),
        );
        let descriptors = manager.descriptors();
        let patch = profile_patch(&profile, request, &descriptors);
        let mut diff = manager.diff(&patch)?;
        if !profile.custom_args.is_empty() {
            diff.warnings.push(
                "Custom arguments remain profile/run configuration and are not written to native settings."
                    .to_string(),
            );
        }
        if !profile.environment.is_empty()
            && !descriptors
                .iter()
                .any(|descriptor| descriptor.key.id() == "common.environment")
        {
            diff.warnings.push(
                "This provider does not expose native Environment settings; profile Environment remains stored for run configuration."
                    .to_string(),
            );
        }
        Ok(diff)
    }

    pub fn apply_profile(
        &self,
        request: ApplyConfigProfileRequest,
    ) -> Result<SettingsSnapshot, AgentSettingError> {
        if !request.confirmed {
            return Err(AgentSettingError::InvalidRequest(
                "profile apply requires explicit diff confirmation".to_string(),
            ));
        }
        let profile = self
            .load_profile_store()?
            .profiles
            .into_iter()
            .find(|profile| profile.id == request.preview.id)
            .ok_or_else(|| AgentSettingError::NotFound(request.preview.id.to_string()))?;
        let manager = self.manager(
            profile.provider,
            request.preview.project_path.as_deref().map(Path::new),
        );
        let descriptors = manager.descriptors();
        self.apply(ApplySettingsRequest {
            patch: profile_patch(&profile, &request.preview, &descriptors),
            confirmed: true,
        })
    }

    fn load_profile_store(&self) -> Result<ConfigProfileStore, AgentSettingError> {
        let content = match fs::read_to_string(&self.profile_store_path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ConfigProfileStore::default());
            }
            Err(error) => return Err(error.into()),
        };
        let store: ConfigProfileStore = serde_json::from_str(&content).map_err(|error| {
            AgentSettingError::InvalidConfiguration(format!(
                "profile store {}: {error}",
                self.profile_store_path.display()
            ))
        })?;
        if store.version != SETTINGS_PROFILE_STORE_VERSION {
            return Err(AgentSettingError::Unsupported(format!(
                "profile store version {}",
                store.version
            )));
        }
        Ok(store)
    }

    fn save_profile_store(&self, store: &ConfigProfileStore) -> Result<(), AgentSettingError> {
        let content = serde_json::to_vec_pretty(store).map_err(|error| {
            AgentSettingError::InvalidConfiguration(format!("profile serialization: {error}"))
        })?;
        atomic_write(&self.profile_store_path, &content)
    }

    fn profile_view(&self, profile: ConfigProfile) -> ConfigProfileView {
        let descriptors = self.manager(profile.provider, None).descriptors();
        let descriptor_sensitivity: BTreeMap<_, _> = descriptors
            .iter()
            .map(|descriptor| (descriptor.key.id(), descriptor.sensitive))
            .collect();
        let mut configured_credential_keys = Vec::new();
        let setting_overrides = profile
            .setting_overrides
            .into_iter()
            .filter_map(|(key, value)| {
                match descriptor_sensitivity.get(&key) {
                    Some(true) => {
                        configured_credential_keys.push(key);
                        None
                    }
                    Some(false) => Some((key, value)),
                    // A profile store may outlive the Adapter schema that
                    // created it. Unknown overrides remain lossless in the
                    // internal store, but their values are not safe to expose
                    // through the browser projection because a removed or
                    // future field may be a credential.
                    None => None,
                }
            })
            .collect();
        configured_credential_keys.sort();
        let provider_extension_keys = profile.provider_extensions.keys().cloned().collect();
        ConfigProfileView {
            id: profile.id,
            provider: profile.provider,
            executor_profile: profile.executor_profile,
            name: profile.name,
            schema_version: profile.schema_version,
            setting_overrides,
            configured_credential_keys,
            provider_extension_keys,
            environment: profile.environment,
            custom_args: profile.custom_args,
            updated_at: profile.updated_at,
        }
    }
}

fn lock_profile_store_mutation() -> Result<MutexGuard<'static, ()>, AgentSettingError> {
    PROFILE_STORE_MUTATION_LOCK
        .lock()
        .map_err(|_| std::io::Error::other("profile store mutation lock was poisoned").into())
}

fn classify_replaced_profile_field(
    key: &str,
    changed: bool,
    target_empty: bool,
    source_empty: bool,
    added_keys: &mut Vec<String>,
    overwritten_keys: &mut Vec<String>,
) {
    if !changed {
        return;
    }
    if target_empty && !source_empty {
        added_keys.push(key.to_string());
    } else {
        overwritten_keys.push(key.to_string());
    }
}

fn profile_environment_value(profile: &ConfigProfile) -> Value {
    Value::Object(
        profile
            .environment
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

fn profile_patch(
    profile: &ConfigProfile,
    request: &ProfileApplyPreviewRequest,
    descriptors: &[SettingDescriptor],
) -> SettingsPatch {
    let mut operations: Vec<_> = profile
        .setting_overrides
        .iter()
        .filter_map(|(key, value)| {
            let (namespace, name) = key.split_once('.')?;
            Some(SettingOperation::Replace {
                key: SettingKey::new(namespace, name),
                scope: request.scope,
                value: value.clone(),
            })
        })
        .collect();
    if descriptors.iter().any(|descriptor| {
        descriptor.key.id() == "common.environment"
            && descriptor.capabilities.profile_storable
            && descriptor.supported_scopes.contains(&request.scope)
    }) {
        operations.push(SettingOperation::Replace {
            key: SettingKey::new("common", "environment"),
            scope: request.scope,
            value: profile_environment_value(profile),
        });
    }
    SettingsPatch {
        provider: profile.provider,
        project_path: request.project_path.clone(),
        expected_file_revisions: request.expected_file_revisions.clone(),
        operations,
    }
}

fn validate_profile(
    profile: &ConfigProfile,
    descriptors: &[SettingDescriptor],
) -> Result<(), AgentSettingError> {
    if profile.name.trim().is_empty() {
        return Err(AgentSettingError::ValidationFailed(
            "profile name cannot be empty".to_string(),
        ));
    }
    let by_key: BTreeMap<_, _> = descriptors
        .iter()
        .map(|descriptor| (descriptor.key.id(), descriptor))
        .collect();
    for (key, value) in &profile.setting_overrides {
        let descriptor = by_key
            .get(key)
            .ok_or_else(|| AgentSettingError::ValidationFailed(format!("unknown setting {key}")))?;
        if !descriptor.capabilities.profile_storable {
            return Err(AgentSettingError::Unsupported(format!(
                "{key} cannot be stored in a profile"
            )));
        }
        validate_setting_value(descriptor, value)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ProviderSettingsManager {
    provider: AgentSettingsProvider,
    home_dir: PathBuf,
    user_root: PathBuf,
    project_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct NativeFileSpec {
    id: String,
    path: PathBuf,
    format: NativeConfigFormat,
    scope: SettingScope,
    writable: bool,
    raw_editable: bool,
    dialect: NativeDialect,
    inline_content: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeDialect {
    Document,
    Cordis,
    Credentials,
}

#[derive(Debug, Clone)]
struct ParsedNativeFile {
    spec: NativeFileSpec,
    bytes: Option<Vec<u8>>,
    revision: String,
    content: String,
    value: Option<Value>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct RenderedFile {
    spec: NativeFileSpec,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
}

impl ProviderSettingsManager {
    pub fn new(
        provider: AgentSettingsProvider,
        home_dir: PathBuf,
        project_path: Option<PathBuf>,
    ) -> Self {
        let project_path = project_path.map(|path| fs::canonicalize(&path).unwrap_or(path));
        let user_root = match provider {
            AgentSettingsProvider::Codex => codex_home_for(&home_dir),
            AgentSettingsProvider::ClaudeCode => home_dir.join(".claude"),
            AgentSettingsProvider::Gemini => home_dir.join(".gemini"),
            AgentSettingsProvider::OhMyPi => oh_my_pi_agent_root_for(&home_dir),
            AgentSettingsProvider::Opencode => native_assets::opencode_root_for(&home_dir),
            AgentSettingsProvider::DeepseekHarness => native_assets::deepseek_home_for(&home_dir),
        };
        Self {
            provider,
            home_dir,
            user_root,
            project_path,
        }
    }

    pub const fn provider(&self) -> AgentSettingsProvider {
        self.provider
    }

    pub fn descriptors(&self) -> Vec<SettingDescriptor> {
        let parsed_files: Vec<_> = self
            .file_specs()
            .into_iter()
            .map(read_native_file)
            .collect();
        self.descriptors_for_files(&parsed_files)
    }

    fn descriptors_for_files(&self, parsed_files: &[ParsedNativeFile]) -> Vec<SettingDescriptor> {
        let connection_provider = match self.provider {
            AgentSettingsProvider::Codex => {
                effective_string_at_path(parsed_files, &["model_provider"])
                    .unwrap_or_else(|| "openai".to_string())
            }
            AgentSettingsProvider::OhMyPi => {
                effective_string_at_path(parsed_files, &["modelRoles", "default"])
                    .and_then(|model| {
                        model
                            .split_once('/')
                            .map(|(provider, _)| provider.trim().to_string())
                    })
                    .filter(|provider| !provider.is_empty())
                    .unwrap_or_else(|| "openai".to_string())
            }
            AgentSettingsProvider::ClaudeCode | AgentSettingsProvider::Gemini => String::new(),
            AgentSettingsProvider::Opencode => effective_string_at_path(parsed_files, &["model"])
                .and_then(|model| {
                    model
                        .split_once('/')
                        .map(|(provider, _)| provider.to_string())
                })
                .unwrap_or_default(),
            AgentSettingsProvider::DeepseekHarness => {
                effective_string_at_path(parsed_files, &["llm-deepseek", "config", "apiKeyEnv"])
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "DEEPSEEK_API_KEY".into())
            }
        };
        let mut descriptors = provider_descriptors(self.provider, &connection_provider);
        if matches!(
            self.provider,
            AgentSettingsProvider::Opencode | AgentSettingsProvider::DeepseekHarness
        ) {
            for descriptor in &mut descriptors {
                let original = std::mem::take(&mut descriptor.native_locations);
                for location in &original {
                    for file in parsed_files {
                        let matches_config =
                            matches!(location.file_id.as_str(), "user_config" | "project_config")
                                && file.spec.scope == location.scope
                                && (file.spec.id.ends_with("config")
                                    || file.spec.dialect == NativeDialect::Cordis);
                        if matches_config || file.spec.id == location.file_id {
                            let mut mapped = location.clone();
                            mapped.file_id = file.spec.id.clone();
                            descriptor.native_locations.push(mapped);
                        }
                    }
                }
                descriptor.supported_scopes = descriptor
                    .native_locations
                    .iter()
                    .map(|location| location.scope)
                    .collect();
                descriptor.supported_scopes.sort();
                descriptor.supported_scopes.dedup();
            }
        }
        descriptors
    }

    fn validate_project_path(&self) -> Result<(), AgentSettingError> {
        let Some(project_path) = &self.project_path else {
            return Ok(());
        };
        if !project_path.is_absolute() {
            return Err(AgentSettingError::UnsafePath(
                "project path must be absolute".to_string(),
            ));
        }
        if !project_path.is_dir() {
            return Err(AgentSettingError::UnsafePath(format!(
                "project path does not exist or is not a directory: {}",
                project_path.display()
            )));
        }
        Ok(())
    }

    fn file_specs(&self) -> Vec<NativeFileSpec> {
        let mut files = match self.provider {
            AgentSettingsProvider::Codex => vec![
                file_spec(
                    "user_config",
                    self.user_root.join("config.toml"),
                    NativeConfigFormat::Toml,
                    SettingScope::User,
                ),
                file_spec(
                    "user_auth",
                    self.user_root.join("auth.json"),
                    NativeConfigFormat::Json,
                    SettingScope::User,
                ),
            ],
            AgentSettingsProvider::ClaudeCode => vec![file_spec(
                "user_settings",
                self.user_root.join("settings.json"),
                NativeConfigFormat::Json,
                SettingScope::User,
            )],
            AgentSettingsProvider::Gemini => vec![
                file_spec(
                    "user_settings",
                    self.user_root.join("settings.json"),
                    NativeConfigFormat::Json,
                    SettingScope::User,
                ),
                file_spec(
                    "user_env",
                    self.user_root.join(".env"),
                    NativeConfigFormat::Dotenv,
                    SettingScope::User,
                ),
            ],
            AgentSettingsProvider::OhMyPi => vec![
                file_spec(
                    "user_config",
                    self.user_root.join("config.yml"),
                    NativeConfigFormat::Yaml,
                    SettingScope::User,
                ),
                file_spec(
                    "user_models",
                    self.user_root.join("models.yml"),
                    NativeConfigFormat::Yaml,
                    SettingScope::User,
                ),
            ],
            AgentSettingsProvider::Opencode => self.opencode_file_specs(),
            AgentSettingsProvider::DeepseekHarness => self.deepseek_file_specs(),
        };
        if let Some(project_path) = &self.project_path {
            if matches!(
                self.provider,
                AgentSettingsProvider::Opencode | AgentSettingsProvider::DeepseekHarness
            ) {
                return files;
            }
            let spec = match self.provider {
                AgentSettingsProvider::Codex => file_spec(
                    "project_config",
                    project_path.join(".codex/config.toml"),
                    NativeConfigFormat::Toml,
                    SettingScope::Project,
                ),
                AgentSettingsProvider::ClaudeCode => file_spec(
                    "project_settings",
                    project_path.join(".claude/settings.json"),
                    NativeConfigFormat::Json,
                    SettingScope::Project,
                ),
                AgentSettingsProvider::Gemini => file_spec(
                    "project_settings",
                    project_path.join(".gemini/settings.json"),
                    NativeConfigFormat::Json,
                    SettingScope::Project,
                ),
                AgentSettingsProvider::OhMyPi => {
                    let mut spec = file_spec(
                        "project_config",
                        project_path.join(".omp/config.yml"),
                        NativeConfigFormat::Yaml,
                        SettingScope::Project,
                    );
                    // Current OMP project settings support is deliberately
                    // discovery-only until a versioned native setting mapping
                    // is available beyond project model roles.
                    spec.writable = false;
                    spec.raw_editable = false;
                    spec
                }
                AgentSettingsProvider::Opencode | AgentSettingsProvider::DeepseekHarness => {
                    unreachable!("provider-specific file specs are complete")
                }
            };
            files.push(spec);
            if self.provider == AgentSettingsProvider::Gemini {
                files.push(file_spec(
                    "project_env",
                    project_path.join(".gemini/.env"),
                    NativeConfigFormat::Dotenv,
                    SettingScope::Project,
                ));
            }
        }
        files
    }

    fn opencode_file_specs(&self) -> Vec<NativeFileSpec> {
        let mut files = vec![file_spec(
            "user_auth",
            native_assets::opencode_auth_path(&self.home_dir),
            NativeConfigFormat::Json,
            SettingScope::User,
        )];
        let roots = native_assets::opencode_config_roots(&self.home_dir);
        self.push_opencode_directory(
            &mut files,
            &self.user_root,
            "user",
            SettingScope::User,
            true,
        );
        if let Some(path) = native_assets::opencode_custom_config(&self.home_dir) {
            files.push(file_spec(
                "custom_config",
                path,
                NativeConfigFormat::Jsonc,
                SettingScope::User,
            ));
        }
        if let Some(project) = &self.project_path {
            // Parent project files are loaded in native precedence order. Do
            // not traverse beyond the nearest Git worktree when one exists.
            let root = native_assets::deepseek_project_root(project);
            let mut ancestors: Vec<_> = project
                .ancestors()
                .take_while(|path| path.starts_with(root))
                .collect();
            ancestors.reverse();
            for (index, path) in ancestors.iter().enumerate() {
                self.push_opencode_directory(
                    &mut files,
                    path,
                    &format!("project_{index}"),
                    SettingScope::Project,
                    false,
                );
            }
            for (index, path) in ancestors.iter().enumerate() {
                self.push_opencode_directory(
                    &mut files,
                    &path.join(".opencode"),
                    &format!("asset_{index}"),
                    SettingScope::Project,
                    false,
                );
            }
        }
        for (index, root) in roots.iter().skip(1).enumerate() {
            self.push_opencode_directory(
                &mut files,
                root,
                &format!("custom_dir_{index}"),
                SettingScope::User,
                false,
            );
        }
        if let Ok(content) = std::env::var("OPENCODE_CONFIG_CONTENT") {
            if !content.trim().is_empty() {
                let mut spec = file_spec(
                    "inline_config",
                    self.user_root.join(".inline"),
                    NativeConfigFormat::Jsonc,
                    SettingScope::User,
                );
                spec.writable = false;
                spec.raw_editable = false;
                spec.inline_content = Some(content);
                files.push(spec);
            }
        }
        files
    }

    fn push_opencode_directory(
        &self,
        files: &mut Vec<NativeFileSpec>,
        root: &Path,
        prefix: &str,
        scope: SettingScope,
        legacy: bool,
    ) {
        let target = native_assets::opencode_write_target(root, legacy);
        for (index, path) in native_assets::opencode_config_candidates(root, legacy)
            .into_iter()
            .enumerate()
        {
            let mut spec = file_spec(
                &format!("{prefix}_{index}_config"),
                path.clone(),
                NativeConfigFormat::Jsonc,
                scope,
            );
            spec.writable = path == target;
            spec.raw_editable = false;
            files.push(spec);
        }
    }

    /// Configuration-only paths shared by model discovery and full settings
    /// management. Building these specs never reads credentials or .env files.
    fn deepseek_config_file_specs(&self) -> Vec<NativeFileSpec> {
        let mut profile = file_spec(
            "acp_profile_config",
            self.user_root.join("profiles/acp/cordis.patch.yml"),
            NativeConfigFormat::Yaml,
            SettingScope::User,
        );
        profile.dialect = NativeDialect::Cordis;
        profile.writable = false;
        profile.raw_editable = false;
        let mut home = file_spec(
            "user_config",
            self.user_root.join("cordis.patch.yml"),
            NativeConfigFormat::Yaml,
            SettingScope::User,
        );
        home.dialect = NativeDialect::Cordis;
        home.raw_editable = false;
        vec![profile, home]
    }

    fn deepseek_file_specs(&self) -> Vec<NativeFileSpec> {
        let mut files = self.deepseek_config_file_specs();
        let mut credentials = file_spec(
            "user_credentials",
            self.user_root.join(".credentials.yaml"),
            NativeConfigFormat::Yaml,
            SettingScope::User,
        );
        credentials.dialect = NativeDialect::Credentials;
        credentials.raw_editable = false;
        let mut env = file_spec(
            "home_env",
            self.user_root.join(".env"),
            NativeConfigFormat::Dotenv,
            SettingScope::User,
        );
        env.writable = false;
        env.raw_editable = false;
        files.push(env);
        if let Some(project) = &self.project_path {
            let mut env = file_spec(
                "project_env",
                project.join(".env"),
                NativeConfigFormat::Dotenv,
                SettingScope::Project,
            );
            env.writable = false;
            env.raw_editable = false;
            files.push(env);
        }
        files.push(credentials);
        let key = effective_string_at_path(
            &self
                .deepseek_config_file_specs()
                .iter()
                .cloned()
                .map(read_native_file)
                .collect::<Vec<_>>(),
            &["llm-deepseek", "config", "apiKeyEnv"],
        )
        .unwrap_or_else(|| "DEEPSEEK_API_KEY".into());
        let values: Map<String, Value> = [key.as_str(), "DEEPSEEK_BASE_URL"]
            .into_iter()
            .filter_map(|name| {
                std::env::var(name)
                    .ok()
                    .map(|value| (name.to_string(), Value::String(value)))
            })
            .collect();
        if !values.is_empty() {
            let mut env = file_spec(
                "launch_environment",
                self.user_root.join(".launch-env"),
                NativeConfigFormat::Json,
                SettingScope::User,
            );
            env.writable = false;
            env.raw_editable = false;
            env.inline_content = Some(Value::Object(values).to_string());
            files.push(env);
        }
        files
    }

    pub fn discover(&self) -> Result<SettingsSnapshot, AgentSettingError> {
        self.discover_with_sensitive_values(false)
    }

    fn discover_with_sensitive_values(
        &self,
        include_sensitive_values: bool,
    ) -> Result<SettingsSnapshot, AgentSettingError> {
        self.validate_project_path()?;
        let parsed_files: Vec<_> = self
            .file_specs()
            .into_iter()
            .map(read_native_file)
            .collect();
        let descriptors = self.descriptors_for_files(&parsed_files);
        let installed = self.provider_root().exists()
            || parsed_files.iter().any(|file| file.bytes.is_some())
            || workspace_utils::shell::resolve_executable_path_blocking(self.provider.executable())
                .is_some();
        let provider_version = DirectProvider::from(self.provider)
            .versions()
            .runtime
            .map(str::to_string);
        let schema_revision =
            revision_bytes(&serde_json::to_vec(&descriptors).map_err(|error| {
                AgentSettingError::InvalidConfiguration(format!(
                    "could not serialize setting descriptors: {error}"
                ))
            })?);

        let mut native_files = Vec::new();
        let mut errors = Vec::new();
        for file in &parsed_files {
            if file.error.is_some() {
                errors.push(AgentSettingIssue {
                    // Parser diagnostics can contain snippets from the native
                    // file. Keep the ordinary browser projection field-safe;
                    // the parse status and opaque file id are sufficient for
                    // recovery without returning native bytes or secrets.
                    message: "Native configuration could not be parsed.".to_string(),
                    file_id: Some(file.spec.id.clone()),
                    setting_key: None,
                    recovery: "Fix this file's syntax outside Vibe Studio, then refresh."
                        .to_string(),
                });
            }
            let managed_setting_keys = descriptors
                .iter()
                .filter(|descriptor| {
                    descriptor.native_locations.iter().any(|location| {
                        location.file_id == file.spec.id && location.scope == file.spec.scope
                    })
                })
                .map(|descriptor| descriptor.key.clone())
                .collect();
            native_files.push(NativeConfigFile {
                file_id: file.spec.id.clone(),
                format: file.spec.format,
                scope: file.spec.scope,
                exists: file.bytes.is_some(),
                parse_status: if file.bytes.is_none() {
                    NativeParseStatus::Missing
                } else if file.spec.format == NativeConfigFormat::Opaque {
                    NativeParseStatus::Unsupported
                } else if file.error.is_some() {
                    NativeParseStatus::Invalid
                } else {
                    NativeParseStatus::Parsed
                },
                revision: Some(file.revision.clone()),
                writable: file.spec.writable && file.error.is_none(),
                managed_setting_keys,
            });
        }

        let effective_settings = descriptors
            .iter()
            .map(|descriptor| {
                effective_setting(
                    descriptor,
                    &descriptors,
                    &parsed_files,
                    include_sensitive_values,
                )
            })
            .collect();
        let unknown_native_nodes = unknown_native_nodes(&descriptors, &parsed_files);

        Ok(SettingsSnapshot {
            provider: self.provider,
            installed,
            provider_version,
            schema_revision,
            capabilities: SettingsCapabilities {
                readable: true,
                native_writable: descriptors
                    .iter()
                    .any(|descriptor| descriptor.capabilities.writable),
                profile_storage: true,
                per_run_overrides: descriptors
                    .iter()
                    .any(|descriptor| descriptor.capabilities.run_override),
            },
            descriptors,
            native_files,
            effective_settings,
            unknown_native_nodes,
            limitations: provider_limitations(self.provider),
            errors,
        })
    }

    fn error_snapshot(&self, error: &AgentSettingError) -> SettingsSnapshot {
        let descriptors = self.descriptors();
        SettingsSnapshot {
            provider: self.provider,
            installed: false,
            provider_version: None,
            schema_revision: String::new(),
            capabilities: SettingsCapabilities {
                readable: false,
                native_writable: false,
                profile_storage: true,
                per_run_overrides: descriptors
                    .iter()
                    .any(|descriptor| descriptor.capabilities.run_override),
            },
            descriptors,
            native_files: Vec::new(),
            effective_settings: Vec::new(),
            unknown_native_nodes: Vec::new(),
            limitations: provider_limitations(self.provider),
            errors: vec![AgentSettingIssue {
                message: error.to_string(),
                file_id: None,
                setting_key: None,
                recovery: "Verify the project path and provider configuration, then refresh."
                    .to_string(),
            }],
        }
    }

    pub fn diff(&self, patch: &SettingsPatch) -> Result<SettingsDiff, AgentSettingError> {
        if patch.provider != self.provider {
            return Err(AgentSettingError::InvalidRequest(
                "patch provider does not match settings manager".to_string(),
            ));
        }
        let rendered = self.render_patch(patch)?;
        Ok(SettingsDiff {
            provider: self.provider,
            files: rendered
                .into_iter()
                .map(|file| NativeFileDiff {
                    file_id: file.spec.id,
                    format: file.spec.format,
                    scope: file.spec.scope,
                    changed: file.before.as_deref() != Some(file.after.as_slice()),
                })
                .collect(),
            warnings: vec![
                "Persistent changes affect new provider sessions; active sessions are not restarted."
                    .to_string(),
            ],
        })
    }

    pub fn apply(&self, patch: &SettingsPatch) -> Result<SettingsSnapshot, AgentSettingError> {
        let rendered = self.render_patch(patch)?;
        write_all_or_restore(&rendered)?;
        match self
            .discover_with_sensitive_values(true)
            .and_then(|snapshot| {
                verify_operations(&snapshot, &patch.operations, &rendered)?;
                Ok(snapshot.with_sensitive_values_redacted())
            }) {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => match rollback_files(&rendered) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(AgentSettingError::RollbackFailed(format!(
                    "apply verification failed: {error}; {rollback_error}"
                ))),
            },
        }
    }

    pub fn reveal(
        &self,
        key: &SettingKey,
        scope: SettingScope,
        expected_revision: &str,
    ) -> Result<RevealAgentSettingResponse, AgentSettingError> {
        self.validate_project_path()?;
        let parsed_files: Vec<_> = self
            .file_specs()
            .into_iter()
            .map(read_native_file)
            .collect();
        let descriptors = self.descriptors_for_files(&parsed_files);
        let key_id = key.id();
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.key == *key)
            .ok_or_else(|| AgentSettingError::NotFound(key_id.clone()))?;
        if !descriptor.sensitive {
            return Err(AgentSettingError::Unsupported(format!(
                "{key_id} is available through ordinary settings discovery"
            )));
        }
        let location = descriptor
            .native_locations
            .iter()
            .rev()
            .find(|location| {
                location.scope == scope
                    && parsed_files.iter().any(|file| {
                        file.spec.id == location.file_id
                            && file.value.as_ref().is_some_and(|value| {
                                value_at_path(value, &location.native_path).is_some()
                            })
                    })
            })
            .ok_or_else(|| {
                AgentSettingError::Unsupported(format!(
                    "{key_id} is not available at {} scope",
                    scope.label()
                ))
            })?;
        let file = parsed_files
            .iter()
            .find(|file| file.spec.id == location.file_id)
            .ok_or_else(|| AgentSettingError::NotFound(location.file_id.clone()))?;
        if file.error.is_some() {
            return Err(AgentSettingError::InvalidConfiguration(format!(
                "{} contains invalid configuration",
                location.file_id
            )));
        }
        if file.revision != expected_revision {
            return Err(AgentSettingError::StaleRevision);
        }
        let value = file
            .value
            .as_ref()
            .and_then(|value| value_at_path(value, &location.native_path))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AgentSettingError::NotFound(key_id.clone()))?;
        Ok(RevealAgentSettingResponse {
            key: key.clone(),
            scope,
            value: value.to_string(),
            revision: file.revision.clone(),
        })
    }

    pub fn diff_native_file(
        &self,
        patch: &NativeFilePatch,
    ) -> Result<SettingsDiff, AgentSettingError> {
        let rendered = self.render_native_file(patch)?;
        Ok(SettingsDiff {
            provider: self.provider,
            files: vec![NativeFileDiff {
                file_id: rendered.spec.id,
                format: rendered.spec.format,
                scope: rendered.spec.scope,
                changed: rendered.before.as_deref() != Some(rendered.after.as_slice()),
            }],
            warnings: vec![
                "Raw editing can change provider-native fields that Vibe Studio does not interpret."
                    .to_string(),
            ],
        })
    }

    pub fn apply_native_file(
        &self,
        patch: &NativeFilePatch,
    ) -> Result<SettingsSnapshot, AgentSettingError> {
        let rendered = self.render_native_file(patch)?;
        write_all_or_restore(std::slice::from_ref(&rendered))?;
        let result = (|| {
            let observed = fs::read(&rendered.spec.path)?;
            if observed != rendered.after {
                return Err(AgentSettingError::VerificationFailed(format!(
                    "{} did not round-trip",
                    rendered.spec.id
                )));
            }
            self.discover()
        })();
        match result {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => match rollback_files(std::slice::from_ref(&rendered)) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(AgentSettingError::RollbackFailed(format!(
                    "native-file verification failed: {error}; {rollback_error}"
                ))),
            },
        }
    }

    fn render_patch(&self, patch: &SettingsPatch) -> Result<Vec<RenderedFile>, AgentSettingError> {
        self.validate_project_path()?;
        if patch.provider != self.provider {
            return Err(AgentSettingError::InvalidRequest(
                "patch provider does not match settings manager".into(),
            ));
        }
        if patch.operations.is_empty() {
            return Err(AgentSettingError::InvalidRequest(
                "settings patch must contain at least one operation".to_string(),
            ));
        }
        let descriptors = self.descriptors();
        let by_key: BTreeMap<_, _> = descriptors
            .iter()
            .map(|descriptor| (descriptor.key.id(), descriptor))
            .collect();
        let specs: BTreeMap<_, _> = self
            .file_specs()
            .into_iter()
            .map(|spec| (spec.id.clone(), spec))
            .collect();
        let mut file_operations: BTreeMap<String, Vec<&SettingOperation>> = BTreeMap::new();
        for operation in &patch.operations {
            let (key, scope, value) = match operation {
                SettingOperation::Preserve { key, scope } => (key, *scope, None),
                SettingOperation::Replace { key, scope, value } => (key, *scope, Some(value)),
                SettingOperation::Clear { key, scope } => (key, *scope, None),
            };
            let key_id = key.id();
            let descriptor = by_key.get(&key_id).ok_or_else(|| {
                AgentSettingError::ValidationFailed(format!("unknown setting {key_id}"))
            })?;
            if matches!(operation, SettingOperation::Preserve { .. }) {
                if !descriptor.supported_scopes.contains(&scope) {
                    return Err(AgentSettingError::Unsupported(format!(
                        "{key_id} is not available at {} scope",
                        scope.label()
                    )));
                }
                continue;
            }
            if !descriptor.capabilities.writable {
                return Err(AgentSettingError::Unsupported(format!(
                    "{key_id} is read-only"
                )));
            }
            if matches!(operation, SettingOperation::Clear { .. })
                && !descriptor.capabilities.resettable
            {
                return Err(AgentSettingError::Unsupported(format!(
                    "{key_id} cannot be reset"
                )));
            }
            if let Some(value) = value {
                validate_setting_value(descriptor, value)?;
            }
            let location = descriptor
                .native_locations
                .iter()
                .rev()
                .find(|location| {
                    location.scope == scope
                        && specs
                            .get(&location.file_id)
                            .is_some_and(|file| file.writable)
                })
                .ok_or_else(|| {
                    AgentSettingError::Unsupported(format!(
                        "{key_id} is not writable at {} scope",
                        scope.label()
                    ))
                })?;
            file_operations
                .entry(location.file_id.clone())
                .or_default()
                .push(operation);
        }

        let mut rendered = Vec::new();
        for (file_id, operations) in file_operations {
            let spec = specs
                .get(&file_id)
                .cloned()
                .ok_or_else(|| AgentSettingError::NotFound(file_id.clone()))?;
            if !spec.writable {
                return Err(AgentSettingError::Unsupported(format!(
                    "{} is discovery-only",
                    spec.id
                )));
            }
            let parsed = read_native_file(spec.clone());
            require_expected_revision(&patch.expected_file_revisions, &spec.id, &parsed.revision)?;
            if parsed.error.is_some() {
                return Err(AgentSettingError::InvalidConfiguration(format!(
                    "{} contains invalid configuration",
                    spec.id
                )));
            }
            let mut native_operations = operations
                .into_iter()
                .map(|operation| {
                    let (key, scope, value) = match operation {
                        SettingOperation::Replace { key, scope, value } => {
                            (key, *scope, Some(value.clone()))
                        }
                        SettingOperation::Clear { key, scope } => (key, *scope, None),
                        SettingOperation::Preserve { .. } => {
                            unreachable!("preserve operations do not render files")
                        }
                    };
                    let descriptor = by_key
                        .get(&key.id())
                        .expect("validated descriptor remains available");
                    let path = descriptor
                        .native_locations
                        .iter()
                        .find(|location| location.scope == scope && location.file_id == spec.id)
                        .expect("validated location remains available")
                        .native_path
                        .clone();
                    (path, value, descriptor, scope)
                })
                .collect::<Vec<_>>();
            // Parent maps are always rendered before their explicitly managed
            // child settings. This lets a normal Environment edit retain API
            // and credential fields without bypassing their own validation.
            native_operations.sort_by_key(|(path, _, _, _)| path.len());
            let explicitly_mutated_paths: BTreeSet<_> = native_operations
                .iter()
                .map(|(path, _, _, _)| path.clone())
                .collect();
            let original_value = parsed.value.as_ref();
            let native_operations = native_operations
                .into_iter()
                .map(|(path, value, descriptor, scope)| {
                    Ok((
                        path.clone(),
                        preserve_managed_descendants(
                            descriptor,
                            &descriptors,
                            &spec.id,
                            scope,
                            &path,
                            original_value,
                            &explicitly_mutated_paths,
                            value,
                        )?,
                    ))
                })
                .collect::<Result<Vec<_>, AgentSettingError>>()?;
            let after = self.render_provider_document(&spec, &parsed, native_operations)?;
            rendered.push(RenderedFile {
                spec,
                before: parsed.bytes,
                after: after.into_bytes(),
            });
        }
        Ok(rendered)
    }

    fn render_provider_document(
        &self,
        spec: &NativeFileSpec,
        parsed: &ParsedNativeFile,
        operations: Vec<(Vec<String>, Option<Value>)>,
    ) -> Result<String, AgentSettingError> {
        let fail = AgentSettingError::InvalidConfiguration;
        match spec.dialect {
            NativeDialect::Document => {
                // Replacing a native API key must never turn an OAuth entry
                // into an incomplete or mixed authentication record.
                if self.provider == AgentSettingsProvider::Opencode && spec.id == "user_auth" {
                    let mut operations = operations;
                    let mut types = Vec::new();
                    for (path, replacement) in &mut operations {
                        if path.len() == 2 && path[1] == "key" {
                            if parsed
                                .value
                                .as_ref()
                                .and_then(|value| value.get(&path[0]))
                                .and_then(|entry| entry.get("type"))
                                .and_then(Value::as_str)
                                .is_some_and(|kind| kind != "api")
                            {
                                return Err(AgentSettingError::Unsupported("OAuth authentication is managed by native OpenCode; it is not replaced by API-key edits".into()));
                            }
                            if replacement.is_some() {
                                types.push((
                                    vec![path[0].clone(), "type".into()],
                                    Some(Value::String("api".into())),
                                ));
                            } else {
                                path.pop();
                            }
                        }
                    }
                    operations.extend(types);
                    return render_document(spec.format, &parsed.content, operations);
                }
                render_document(spec.format, &parsed.content, operations)
            }
            NativeDialect::Cordis => {
                let mut desired = parsed
                    .value
                    .clone()
                    .unwrap_or_else(|| Value::Object(Map::new()));
                // A new home override copies the entire preceding native row
                // config before editing it: Cordis replaces config wholesale.
                let profile = read_native_file(self.deepseek_file_specs().remove(0));
                for (path, _) in &operations {
                    let id = &path[0];
                    if desired.get(id).and_then(|row| row.get("config")).is_none() {
                        if profile.error.is_some() {
                            return Err(fail(
                                "ACP profile configuration cannot be safely preserved by a home override"
                                    .into(),
                            ));
                        }
                        let inherited_config = profile
                            .value
                            .as_ref()
                            .and_then(|value| value.get(id))
                            .and_then(|row| row.get("config"))
                            .cloned()
                            .unwrap_or_else(|| Value::Object(Map::new()));
                        let mut row = desired
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({"id": id}));
                        // A home row may override disabled or other metadata
                        // without its own config. Preserve that metadata and
                        // seed only config, never the profile's plugin insert.
                        update_json_path(&mut row, &["config".into()], Some(inherited_config))?;
                        update_json_path(&mut desired, std::slice::from_ref(id), Some(row))?;
                    }
                }
                for (path, replacement) in operations {
                    if path == ["acp", "config", "model"] {
                        let pair = replacement.as_ref().map(deepseek_model_pair).transpose()?;
                        update_json_path(
                            &mut desired,
                            &["acp".into(), "config".into(), "provider".into()],
                            pair.as_ref().map(|pair| Value::String(pair[0].clone())),
                        )?;
                        update_json_path(
                            &mut desired,
                            &path,
                            pair.map(|pair| Value::String(pair[1].clone())),
                        )?;
                    } else {
                        update_json_path(&mut desired, &path, replacement)?;
                    }
                }
                native_assets::render_cordis_patch(&parsed.content, &desired).map_err(fail)
            }
            NativeDialect::Credentials => {
                let before = parsed
                    .value
                    .clone()
                    .unwrap_or_else(|| Value::Object(Map::new()));
                let mut desired = before.clone();
                if desired.as_object().is_some_and(Map::is_empty) {
                    desired = serde_json::json!({"version": 1, "refs": {}, "records": {}});
                }
                for (path, replacement) in operations {
                    update_json_path(&mut desired, &path, replacement)?;
                }
                validate_deepseek_credentials(&desired)?;
                native_assets::render_yaml_mapping(&parsed.content, &before, &desired, 0)
                    .map_err(fail)
            }
        }
    }

    fn render_native_file(
        &self,
        patch: &NativeFilePatch,
    ) -> Result<RenderedFile, AgentSettingError> {
        self.validate_project_path()?;
        if patch.provider != self.provider {
            return Err(AgentSettingError::InvalidRequest(
                "native patch provider does not match settings manager".to_string(),
            ));
        }
        let spec = self
            .file_specs()
            .into_iter()
            .find(|spec| spec.id == patch.file_id)
            .ok_or_else(|| AgentSettingError::NotFound(patch.file_id.clone()))?;
        if !spec.raw_editable {
            return Err(AgentSettingError::Unsupported(format!(
                "{} does not support raw text editing",
                spec.id
            )));
        }
        let parsed = read_native_file(spec.clone());
        if parsed.bytes.is_none() {
            return Err(AgentSettingError::NotFound(format!(
                "{} does not exist",
                spec.path.display()
            )));
        }
        if parsed.revision != patch.expected_revision {
            return Err(AgentSettingError::StaleRevision);
        }
        parse_document(spec.format, &patch.content)?;
        Ok(RenderedFile {
            spec,
            before: parsed.bytes,
            after: patch.content.as_bytes().to_vec(),
        })
    }

    fn provider_root(&self) -> PathBuf {
        self.user_root.clone()
    }
}

fn file_spec(
    id: &str,
    path: PathBuf,
    format: NativeConfigFormat,
    scope: SettingScope,
) -> NativeFileSpec {
    NativeFileSpec {
        id: id.to_string(),
        path,
        format,
        scope,
        writable: true,
        raw_editable: !matches!(format, NativeConfigFormat::Opaque),
        dialect: NativeDialect::Document,
        inline_content: None,
    }
}

fn provider_descriptors(
    provider: AgentSettingsProvider,
    connection_provider: &str,
) -> Vec<SettingDescriptor> {
    match provider {
        AgentSettingsProvider::Codex => codex_descriptors(connection_provider),
        AgentSettingsProvider::ClaudeCode => claude_descriptors(),
        AgentSettingsProvider::Gemini => gemini_descriptors(),
        AgentSettingsProvider::OhMyPi => oh_my_pi_descriptors(connection_provider),
        AgentSettingsProvider::Opencode => opencode_descriptors(connection_provider),
        AgentSettingsProvider::DeepseekHarness => deepseek_descriptors(connection_provider),
    }
}

fn descriptor(
    namespace: &str,
    name: &str,
    section: SettingSection,
    label: &str,
    description: &str,
    value_type: SettingValueType,
    control: SettingControl,
    locations: &[(&str, SettingScope, &[&str])],
    run_override: bool,
) -> SettingDescriptor {
    SettingDescriptor {
        key: SettingKey::new(namespace, name),
        section,
        label: label.to_string(),
        description: description.to_string(),
        value_type,
        control,
        options: Vec::new(),
        validation: SettingValidation::default(),
        supported_scopes: locations.iter().map(|(_, scope, _)| *scope).collect(),
        capabilities: SettingCapabilities {
            readable: true,
            writable: !locations.is_empty(),
            resettable: !locations.is_empty(),
            profile_storable: !locations.is_empty(),
            run_override,
        },
        native_locations: locations
            .iter()
            .map(|(file_id, scope, path)| NativeSettingLocation {
                file_id: (*file_id).to_string(),
                scope: *scope,
                native_path: path.iter().map(|segment| (*segment).to_string()).collect(),
            })
            .collect(),
        activation: SettingActivation::NextSession,
        sensitive: false,
    }
}

fn sensitive_descriptor(mut descriptor: SettingDescriptor) -> SettingDescriptor {
    descriptor.sensitive = true;
    descriptor.capabilities.profile_storable = false;
    descriptor.capabilities.run_override = false;
    descriptor
}

fn api_address_descriptor(
    description: &str,
    locations: &[(&str, SettingScope, &[&str])],
) -> SettingDescriptor {
    let mut descriptor = descriptor(
        "common",
        "api_address",
        SettingSection::General,
        "API address",
        description,
        SettingValueType::String,
        SettingControl::Text,
        locations,
        false,
    );
    descriptor.validation.max_length = Some(2048);
    descriptor
}

fn credential_descriptor(
    namespace: &str,
    name: &str,
    label: &str,
    description: &str,
    locations: &[(&str, SettingScope, &[&str])],
) -> SettingDescriptor {
    sensitive_descriptor(descriptor(
        namespace,
        name,
        SettingSection::General,
        label,
        description,
        SettingValueType::String,
        SettingControl::Text,
        locations,
        false,
    ))
}

fn common_locations<'a>(path: &'a [&'a str]) -> [(&'a str, SettingScope, &'a [&'a str]); 2] {
    [
        ("user_config", SettingScope::User, path),
        ("project_config", SettingScope::Project, path),
    ]
}

fn common_settings_locations<'a>(
    path: &'a [&'a str],
) -> [(&'a str, SettingScope, &'a [&'a str]); 2] {
    [
        ("user_settings", SettingScope::User, path),
        ("project_settings", SettingScope::Project, path),
    ]
}

fn gemini_env_locations<'a>(path: &'a [&'a str]) -> [(&'a str, SettingScope, &'a [&'a str]); 2] {
    [
        ("user_env", SettingScope::User, path),
        ("project_env", SettingScope::Project, path),
    ]
}

fn select_options(mut descriptor: SettingDescriptor, values: &[(&str, &str)]) -> SettingDescriptor {
    descriptor.options = values
        .iter()
        .map(|(value, label)| SettingOption {
            value: Value::String((*value).to_string()),
            label: (*label).to_string(),
            description: None,
        })
        .collect();
    descriptor
}

fn codex_descriptors(connection_provider: &str) -> Vec<SettingDescriptor> {
    let mut descriptors = vec![
        descriptor(
            "common",
            "model",
            SettingSection::General,
            "Model",
            "Default model used by new Codex sessions.",
            SettingValueType::String,
            SettingControl::Text,
            &common_locations(&["model"]),
            true,
        ),
        descriptor(
            "common",
            "reasoning",
            SettingSection::General,
            "Reasoning effort",
            "Native default model reasoning effort. Accepted values depend on the installed CLI and selected model; per-run choices come from its native catalog.",
            SettingValueType::String,
            SettingControl::Text,
            &common_locations(&["model_reasoning_effort"]),
            true,
        ),
        select_options(
            descriptor(
                "common",
                "approval_policy",
                SettingSection::PermissionsSandbox,
                "Approval policy",
                "When Codex asks before running commands.",
                SettingValueType::String,
                SettingControl::Select,
                &common_locations(&["approval_policy"]),
                true,
            ),
            &[
                ("untrusted", "Untrusted"),
                ("on-request", "On request"),
                ("never", "Never"),
            ],
        ),
        select_options(
            descriptor(
                "common",
                "sandbox_mode",
                SettingSection::PermissionsSandbox,
                "Sandbox mode",
                "Filesystem and process isolation used by new sessions.",
                SettingValueType::String,
                SettingControl::Select,
                &common_locations(&["sandbox_mode"]),
                false,
            ),
            &[
                ("read-only", "Read only"),
                ("workspace-write", "Workspace write"),
                ("danger-full-access", "Full access"),
            ],
        ),
        descriptor(
            "codex",
            "web_search",
            SettingSection::ProviderSettings,
            "Web search",
            "Enable Codex's documented web search feature flag.",
            SettingValueType::Boolean,
            SettingControl::Toggle,
            &common_locations(&["features", "web_search"]),
            false,
        ),
        descriptor(
            "common",
            "environment",
            SettingSection::Environment,
            "Environment variables",
            "Exact environment values injected by Codex's shell environment policy.",
            SettingValueType::StringMap,
            SettingControl::KeyValue,
            &common_locations(&["shell_environment_policy", "set"]),
            true,
        ),
        descriptor(
            "codex",
            "model_provider",
            SettingSection::ProviderSettings,
            "Model provider",
            "Provider identifier used to resolve the default Codex connection.",
            SettingValueType::String,
            SettingControl::Text,
            &common_locations(&["model_provider"]),
            false,
        ),
    ];
    let api_address_path = ["model_providers", connection_provider, "base_url"];
    descriptors.push(api_address_descriptor(
        "Complete base URL for the selected Codex model provider. Paths are preserved.",
        &common_locations(&api_address_path),
    ));
    descriptors.push(credential_descriptor(
        "common",
        "api_key",
        "API key",
        "OpenAI API key stored in Codex's native authentication file.",
        &[("user_auth", SettingScope::User, &["OPENAI_API_KEY"])],
    ));
    descriptors
}

fn claude_descriptors() -> Vec<SettingDescriptor> {
    let mut descriptors = vec![
        descriptor(
            "claude_code",
            "model_settings",
            SettingSection::General,
            "Per-model effort",
            "Native modelSettings keyed by canonical model name. effortLevel supports low, medium, high and xhigh; max is session-only. Unknown model settings are preserved.",
            SettingValueType::Json,
            SettingControl::Json,
            &common_settings_locations(&["modelSettings"]),
            false,
        ),
        descriptor(
            "common",
            "model",
            SettingSection::General,
            "Model",
            "Default model used by new Claude Code sessions.",
            SettingValueType::String,
            SettingControl::Text,
            &common_settings_locations(&["model"]),
            true,
        ),
        select_options(
            descriptor(
                "common",
                "reasoning",
                SettingSection::General,
                "Effort",
                "Legacy native effort default. Newer models use per-model effort settings; this is not confirmation of runtime effort.",
                SettingValueType::String,
                SettingControl::Select,
                &common_settings_locations(&["effortLevel"]),
                true,
            ),
            &[
                ("low", "Low"),
                ("medium", "Medium"),
                ("high", "High"),
                ("xhigh", "Extra high"),
            ],
        ),
        select_options(
            descriptor(
                "common",
                "permission_mode",
                SettingSection::PermissionsSandbox,
                "Permission mode",
                "Default Claude Code permission mode.",
                SettingValueType::String,
                SettingControl::Select,
                &common_settings_locations(&["permissions", "defaultMode"]),
                true,
            ),
            &[
                ("default", "Default"),
                ("auto", "Auto (native classifier)"),
                ("acceptEdits", "Accept edits"),
                ("plan", "Plan"),
                ("dontAsk", "Don't ask"),
                ("bypassPermissions", "Bypass permissions"),
            ],
        ),
        descriptor(
            "claude_code",
            "permission_allow",
            SettingSection::PermissionsSandbox,
            "Allowed tools",
            "Native Claude Code allow rules.",
            SettingValueType::StringList,
            SettingControl::StringList,
            &common_settings_locations(&["permissions", "allow"]),
            false,
        ),
        descriptor(
            "claude_code",
            "permission_deny",
            SettingSection::PermissionsSandbox,
            "Denied tools",
            "Native Claude Code deny rules.",
            SettingValueType::StringList,
            SettingControl::StringList,
            &common_settings_locations(&["permissions", "deny"]),
            false,
        ),
        descriptor(
            "common",
            "environment",
            SettingSection::Environment,
            "Environment variables",
            "Exact environment values set for Claude Code sessions.",
            SettingValueType::StringMap,
            SettingControl::KeyValue,
            &common_settings_locations(&["env"]),
            true,
        ),
        descriptor(
            "claude_code",
            "output_style",
            SettingSection::ProviderSettings,
            "Output style",
            "Named Claude Code output style.",
            SettingValueType::String,
            SettingControl::Text,
            &common_settings_locations(&["outputStyle"]),
            false,
        ),
        descriptor(
            "claude_code",
            "enabled_plugins",
            SettingSection::ProviderSettings,
            "Enabled plugins",
            "Native plugin enablement map.",
            SettingValueType::StringMap,
            SettingControl::KeyValue,
            &common_settings_locations(&["enabledPlugins"]),
            false,
        ),
        descriptor(
            "claude_code",
            "hooks",
            SettingSection::ProviderSettings,
            "Hooks",
            "Claude Code hook definitions. Advanced JSON preserves provider-specific shapes.",
            SettingValueType::Json,
            SettingControl::Json,
            &common_settings_locations(&["hooks"]),
            false,
        ),
        descriptor(
            "claude_code",
            "status_line",
            SettingSection::ProviderSettings,
            "Status line",
            "Claude Code status-line configuration.",
            SettingValueType::Json,
            SettingControl::Json,
            &common_settings_locations(&["statusLine"]),
            false,
        ),
    ];
    descriptors.push(api_address_descriptor(
        "Complete Anthropic-compatible base URL used by Claude Code.",
        &common_settings_locations(&["env", "ANTHROPIC_BASE_URL"]),
    ));
    descriptors.push(credential_descriptor(
        "common",
        "api_key",
        "API key",
        "Anthropic API key used by Claude Code.",
        &common_settings_locations(&["env", "ANTHROPIC_API_KEY"]),
    ));
    descriptors.push(credential_descriptor(
        "claude_code",
        "auth_token",
        "Auth token",
        "Bearer credential used by Anthropic-compatible Claude Code gateways.",
        &common_settings_locations(&["env", "ANTHROPIC_AUTH_TOKEN"]),
    ));
    descriptors
}

fn gemini_descriptors() -> Vec<SettingDescriptor> {
    let mut descriptors = vec![
        descriptor(
            "gemini",
            "model_overrides",
            SettingSection::General,
            "Model thinking budgets",
            "Native modelConfigs.overrides. Thinking budget entries require an explicit match.model and integer modelConfig.generateContentConfig.thinkingConfig.thinkingBudget. Other native override rules are preserved. This changes native defaults, not one attempt; model-specific API limits still apply.",
            SettingValueType::Json,
            SettingControl::Json,
            &common_settings_locations(&["modelConfigs", "overrides"]),
            false,
        ),
        descriptor(
            "common",
            "model",
            SettingSection::General,
            "Model",
            "Default model used by new Gemini sessions.",
            SettingValueType::String,
            SettingControl::Text,
            &common_settings_locations(&["model", "name"]),
            true,
        ),
        descriptor(
            "common",
            "sandbox_enabled",
            SettingSection::PermissionsSandbox,
            "Sandbox",
            "Run Gemini tools in its native sandbox.",
            SettingValueType::Boolean,
            SettingControl::Toggle,
            &common_settings_locations(&["tools", "sandbox"]),
            false,
        ),
        descriptor(
            "gemini",
            "allowed_tools",
            SettingSection::PermissionsSandbox,
            "Allowed tools",
            "Native Gemini tool allow policy.",
            SettingValueType::StringList,
            SettingControl::StringList,
            &common_settings_locations(&["tools", "allowed"]),
            false,
        ),
        descriptor(
            "gemini",
            "excluded_tools",
            SettingSection::PermissionsSandbox,
            "Excluded tools",
            "Native Gemini tool exclusion policy.",
            SettingValueType::StringList,
            SettingControl::StringList,
            &common_settings_locations(&["tools", "exclude"]),
            false,
        ),
        descriptor(
            "gemini",
            "checkpointing",
            SettingSection::ProviderSettings,
            "Checkpointing",
            "Enable Gemini checkpointing when supported.",
            SettingValueType::Boolean,
            SettingControl::Toggle,
            &common_settings_locations(&["general", "checkpointing", "enabled"]),
            false,
        ),
        descriptor(
            "gemini",
            "telemetry",
            SettingSection::ProviderSettings,
            "Telemetry",
            "Enable Gemini CLI telemetry.",
            SettingValueType::Boolean,
            SettingControl::Toggle,
            &common_settings_locations(&["telemetry", "enabled"]),
            false,
        ),
        descriptor(
            "gemini",
            "auth_type",
            SettingSection::ProviderSettings,
            "Authentication type",
            "Gemini CLI native authentication selection.",
            SettingValueType::String,
            SettingControl::Text,
            &common_settings_locations(&["security", "auth", "selectedType"]),
            false,
        ),
    ];
    descriptors.push(api_address_descriptor(
        "Complete Gemini API base URL. Normal base paths remain visible.",
        &gemini_env_locations(&["GOOGLE_GEMINI_BASE_URL"]),
    ));
    descriptors.push(credential_descriptor(
        "common",
        "api_key",
        "API key",
        "Gemini API key from the provider-native .env file.",
        &gemini_env_locations(&["GEMINI_API_KEY"]),
    ));
    descriptors
}

fn oh_my_pi_descriptors(connection_provider: &str) -> Vec<SettingDescriptor> {
    let user = |path: &'static [&'static str]| [("user_config", SettingScope::User, path)];
    let mut descriptors = vec![
        descriptor(
            "common",
            "model",
            SettingSection::General,
            "Model",
            "Default provider/model selector used by new Oh My Pi sessions.",
            SettingValueType::String,
            SettingControl::Text,
            &[
                (
                    "user_config",
                    SettingScope::User,
                    &["modelRoles", "default"],
                ),
                (
                    "project_config",
                    SettingScope::Project,
                    &["modelRoles", "default"],
                ),
            ],
            true,
        ),
        descriptor(
            "oh_my_pi",
            "extensions",
            SettingSection::ProviderSettings,
            "Extensions",
            "OMP extension packages loaded by the canonical agent profile.",
            SettingValueType::StringList,
            SettingControl::StringList,
            &user(&["extensions"]),
            false,
        ),
        descriptor(
            "oh_my_pi",
            "disabled_extensions",
            SettingSection::ProviderSettings,
            "Disabled extensions",
            "OMP extensions retained but disabled.",
            SettingValueType::StringList,
            SettingControl::StringList,
            &user(&["disabledExtensions"]),
            false,
        ),
        descriptor(
            "oh_my_pi",
            "dark_theme",
            SettingSection::ProviderSettings,
            "Dark theme",
            "OMP dark-mode theme identifier.",
            SettingValueType::String,
            SettingControl::Text,
            &user(&["theme", "dark"]),
            false,
        ),
        descriptor(
            "oh_my_pi",
            "light_theme",
            SettingSection::ProviderSettings,
            "Light theme",
            "OMP light-mode theme identifier.",
            SettingValueType::String,
            SettingControl::Text,
            &user(&["theme", "light"]),
            false,
        ),
        descriptor(
            "oh_my_pi",
            "compaction_enabled",
            SettingSection::ProviderSettings,
            "Compaction",
            "Enable OMP context compaction.",
            SettingValueType::Boolean,
            SettingControl::Toggle,
            &user(&["compaction", "enabled"]),
            false,
        ),
        descriptor(
            "oh_my_pi",
            "startup_quiet",
            SettingSection::General,
            "Quiet startup",
            "Reduce OMP startup output.",
            SettingValueType::Boolean,
            SettingControl::Toggle,
            &user(&["startup", "quiet"]),
            false,
        ),
    ];
    let api_address_path = ["providers", connection_provider, "baseUrl"];
    descriptors.push(api_address_descriptor(
        "Complete base URL for the provider selected by the default Oh My Pi model.",
        &[("user_models", SettingScope::User, &api_address_path)],
    ));
    let api_key_path = ["providers", connection_provider, "apiKey"];
    descriptors.push(credential_descriptor(
        "common",
        "api_key",
        "API key",
        "Credential value or environment-variable reference for the selected Oh My Pi provider.",
        &[("user_models", SettingScope::User, &api_key_path)],
    ));
    descriptors
}

fn provider_limitations(provider: AgentSettingsProvider) -> Vec<String> {
    match provider {
        AgentSettingsProvider::Codex => vec![
            "Native profiles and per-project trust entries remain available through raw TOML until their installed-version schema can be verified.".to_string(),
        ],
        AgentSettingsProvider::ClaudeCode => vec![
            "Managed settings use ~/.claude/settings.json and project .claude/settings.json; ~/.claude.json authentication and MCP state is not edited here.".to_string(),
        ],
        AgentSettingsProvider::Gemini => vec![
            "Extensions and version-dependent policy nodes remain native-only and are preserved by targeted writes.".to_string(),
        ],
        AgentSettingsProvider::OhMyPi => vec![
            "Management targets the active Oh My Pi agent profile resolved by the runtime; project config remains discovery-only.".to_string(),
            "Model registry and prompt-template files are reported as native extensions rather than guessed into config.yml fields.".to_string(),
        ],
        AgentSettingsProvider::Opencode => vec![
            "OpenCode configuration uses native JSONC layers and the selected provider/model route. Effort and mode choices are negotiated with the running ACP session.".into(),
            "OAuth entries are native-owned; API-key edits never replace them. Organization/system policy and inline overrides can outrank local edits.".into(),
        ],
        AgentSettingsProvider::DeepseekHarness => vec![
            "DeepSeek Harness is a developer preview. Native ACP/home Cordis patches are read without launching or initializing the CLI.".into(),
            "Only the home patch is writable; project settings are not a native ACP layer. Tagged, aliased or composed YAML stays read-only.".into(),
            "Credentials from the launch environment outrank stored refs. Native changes take effect in a new CLI process; other provider routes remain native-managed.".into(),
        ],
    }
}

fn opencode_descriptors(provider: &str) -> Vec<SettingDescriptor> {
    let mut descriptors = vec![
        descriptor(
            "common",
            "model",
            SettingSection::General,
            "Model",
            "OpenCode provider/model route, preserved verbatim.",
            SettingValueType::String,
            SettingControl::Text,
            &common_locations(&["model"]),
            true,
        ),
        descriptor(
            "opencode",
            "small_model",
            SettingSection::General,
            "Small model",
            "Native route used for lightweight tasks.",
            SettingValueType::String,
            SettingControl::Text,
            &common_locations(&["small_model"]),
            false,
        ),
        descriptor(
            "opencode",
            "default_agent",
            SettingSection::General,
            "Default agent",
            "Native OpenCode agent/mode for new sessions.",
            SettingValueType::String,
            SettingControl::Text,
            &common_locations(&["default_agent"]),
            true,
        ),
        descriptor(
            "opencode",
            "permission",
            SettingSection::PermissionsSandbox,
            "Permissions",
            "Native permission rules, including per-tool and per-skill rules.",
            SettingValueType::Json,
            SettingControl::Json,
            &common_locations(&["permission"]),
            false,
        ),
    ];
    if !provider.is_empty() {
        descriptors.push(api_address_descriptor(
            "Base URL for the provider selected by the native model route.",
            &common_locations(&["provider", provider, "options", "baseURL"]),
        ));
        descriptors.push(credential_descriptor(
            "common",
            "api_key",
            "API key",
            "Native auth.json API entry for the selected route; OAuth entries remain native-owned.",
            &[("user_auth", SettingScope::User, &[provider, "key"])],
        ));
        descriptors.push(credential_descriptor("opencode", "config_api_key", "Configuration API key", "Route-specific config override or literal native env/file reference; references are never expanded by discovery.", &common_locations(&["provider", provider, "options", "apiKey"])));
    }
    descriptors
}

fn deepseek_descriptors(credential_key: &str) -> Vec<SettingDescriptor> {
    let user = |path: &[&str]| {
        descriptor(
            "common",
            "model",
            SettingSection::General,
            "Model",
            "Opaque native ACP route encoded as [provider, model].",
            SettingValueType::String,
            SettingControl::Text,
            &[("user_config", SettingScope::User, path)],
            true,
        )
    };
    let mut model = user(&["acp", "config", "model"]);
    model.validation.pattern = Some("deepseek_model_pair".into());
    vec![
        model,
        descriptor(
            "common",
            "reasoning_effort",
            SettingSection::General,
            "Reasoning effort",
            "Native DeepSeek reasoning default; valid runtime choices depend on the selected ACP route.",
            SettingValueType::String,
            SettingControl::Text,
            &[(
                "user_config",
                SettingScope::User,
                &["llm-deepseek", "config", "reasoningEffort"],
            )],
            true,
        ),
        api_address_descriptor(
            "DeepSeek Messages-compatible base URL, not an OpenAI chat/completions endpoint.",
            &[
                ("home_env", SettingScope::User, &["DEEPSEEK_BASE_URL"]),
                ("project_env", SettingScope::Project, &["DEEPSEEK_BASE_URL"]),
                (
                    "launch_environment",
                    SettingScope::User,
                    &["DEEPSEEK_BASE_URL"],
                ),
                (
                    "user_config",
                    SettingScope::User,
                    &["llm-deepseek", "config", "baseURL"],
                ),
            ],
        ),
        credential_descriptor(
            "common",
            "api_key",
            "API key",
            "Native credential ref selected by llm-deepseek.apiKeyEnv. Environment credentials take precedence.",
            &[
                ("home_env", SettingScope::User, &[credential_key]),
                ("project_env", SettingScope::Project, &[credential_key]),
                (
                    "user_credentials",
                    SettingScope::User,
                    &["refs", credential_key],
                ),
                ("launch_environment", SettingScope::User, &[credential_key]),
            ],
        ),
    ]
}

fn deepseek_model_pair(value: &Value) -> Result<[String; 2], AgentSettingError> {
    let pair: Vec<String> = value
        .as_str()
        .and_then(|value| serde_json::from_str(value).ok())
        .ok_or_else(|| {
            AgentSettingError::ValidationFailed(
                "DeepSeek Harness model must be the opaque advertised [provider, model] value"
                    .into(),
            )
        })?;
    pair.try_into()
        .ok()
        .filter(|pair: &[String; 2]| pair.iter().all(|value| !value.trim().is_empty()))
        .ok_or_else(|| {
            AgentSettingError::ValidationFailed(
                "DeepSeek Harness model requires two nonempty route components".into(),
            )
        })
}

fn validate_deepseek_credentials(value: &Value) -> Result<(), AgentSettingError> {
    let valid = value.as_object().is_some_and(|document| {
        document.get("version") == Some(&Value::from(1))
            && document
                .keys()
                .all(|key| matches!(key.as_str(), "version" | "refs" | "records"))
            && document
                .get("refs")
                .and_then(Value::as_object)
                .is_some_and(|refs| {
                    refs.values()
                        .all(|value| value.as_str().is_some_and(|value| !value.trim().is_empty()))
                })
            && document.get("records").is_none_or(Value::is_object)
    });
    if valid {
        Ok(())
    } else {
        Err(AgentSettingError::InvalidConfiguration(
            "unsupported DeepSeek Harness credential document".into(),
        ))
    }
}

fn read_native_file(spec: NativeFileSpec) -> ParsedNativeFile {
    let bytes = match spec
        .inline_content
        .as_ref()
        .map(|content| Ok(content.as_bytes().to_vec()))
        .unwrap_or_else(|| fs::read(&spec.path))
    {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return ParsedNativeFile {
                spec,
                bytes: None,
                revision: "unreadable".to_string(),
                content: String::new(),
                value: None,
                error: Some(error.to_string()),
            };
        }
    };
    let revision = bytes
        .as_deref()
        .map(revision_bytes)
        .unwrap_or_else(|| "missing".to_string());
    let content = bytes
        .as_deref()
        .map(String::from_utf8_lossy)
        .unwrap_or_default()
        .into_owned();
    let parsed = if bytes.is_some() {
        match spec.dialect {
            NativeDialect::Cordis => native_assets::parse_cordis_patch(&content)
                .map_err(AgentSettingError::InvalidConfiguration),
            NativeDialect::Credentials => native_assets::parse_yaml(&content)
                .map_err(AgentSettingError::InvalidConfiguration)
                .and_then(|value| {
                    validate_deepseek_credentials(&value)?;
                    Ok(value)
                }),
            NativeDialect::Document => parse_document(spec.format, &content),
        }
    } else {
        Ok(Value::Object(Map::new()))
    };
    let (value, error) = match parsed {
        Ok(value) => (Some(value), None),
        Err(error) => (None, Some(error.to_string())),
    };
    ParsedNativeFile {
        spec,
        bytes,
        revision,
        content,
        value,
        error,
    }
}

fn parse_document(format: NativeConfigFormat, content: &str) -> Result<Value, AgentSettingError> {
    if content.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    match format {
        NativeConfigFormat::Toml => {
            let value: toml::Value = toml::from_str(content).map_err(|error| {
                AgentSettingError::InvalidConfiguration(format!("TOML syntax: {error}"))
            })?;
            serde_json::to_value(value).map_err(|error| {
                AgentSettingError::InvalidConfiguration(format!("TOML conversion: {error}"))
            })
        }
        NativeConfigFormat::Json => serde_json::from_str(content).map_err(|error| {
            AgentSettingError::InvalidConfiguration(format!("JSON syntax: {error}"))
        }),
        NativeConfigFormat::Jsonc => {
            jsonc_parser::parse_to_serde_value(content, &ParseOptions::default())
                .map_err(|error| {
                    AgentSettingError::InvalidConfiguration(format!("JSONC syntax: {error}"))
                })?
                .ok_or_else(|| {
                    AgentSettingError::InvalidConfiguration("empty JSONC document".to_string())
                })
        }
        NativeConfigFormat::Yaml => {
            let value: serde_yaml::Value = serde_yaml::from_str(content).map_err(|error| {
                AgentSettingError::InvalidConfiguration(format!("YAML syntax: {error}"))
            })?;
            serde_json::to_value(value).map_err(|error| {
                AgentSettingError::InvalidConfiguration(format!("YAML conversion: {error}"))
            })
        }
        NativeConfigFormat::Dotenv => parse_dotenv(content),
        NativeConfigFormat::Text => Ok(Value::String(content.to_string())),
        NativeConfigFormat::Opaque => Err(AgentSettingError::Unsupported(
            "opaque provider state cannot be parsed as text".to_string(),
        )),
    }
}

fn parse_dotenv(content: &str) -> Result<Value, AgentSettingError> {
    let mut output = Map::new();
    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (key, value) = trimmed.split_once('=').ok_or_else(|| {
            AgentSettingError::InvalidConfiguration(format!("dotenv line {} has no '='", index + 1))
        })?;
        if key.trim().is_empty() {
            return Err(AgentSettingError::InvalidConfiguration(format!(
                "dotenv line {} has an empty key",
                index + 1
            )));
        }
        output.insert(key.trim().to_string(), Value::String(value.to_string()));
    }
    Ok(Value::Object(output))
}

fn render_document(
    format: NativeConfigFormat,
    current: &str,
    operations: impl IntoIterator<Item = (Vec<String>, Option<Value>)>,
) -> Result<String, AgentSettingError> {
    let operations: Vec<_> = operations.into_iter().collect();
    match format {
        NativeConfigFormat::Toml => render_toml(current, &operations),
        NativeConfigFormat::Json | NativeConfigFormat::Jsonc => {
            let mut value = parse_document(format, current)?;
            ensure_object_root(&mut value)?;
            for (path, replacement) in &operations {
                update_json_path(&mut value, path, replacement.clone())?;
            }
            if format == NativeConfigFormat::Jsonc {
                Ok(update_jsonc_content(current, &value))
            } else {
                serde_json::to_string_pretty(&value).map_err(|error| {
                    AgentSettingError::InvalidConfiguration(format!("JSON serialization: {error}"))
                })
            }
        }
        NativeConfigFormat::Yaml => {
            let mut value = parse_document(format, current)?;
            ensure_object_root(&mut value)?;
            for (path, replacement) in &operations {
                update_json_path(&mut value, path, replacement.clone())?;
            }
            serde_yaml::to_string(
                &native_assets::json_to_yaml(&value)
                    .map_err(AgentSettingError::ValidationFailed)?,
            )
            .map_err(|error| {
                AgentSettingError::InvalidConfiguration(format!("YAML serialization: {error}"))
            })
        }
        NativeConfigFormat::Dotenv => {
            let mut value = parse_dotenv(current)?;
            for (path, replacement) in &operations {
                if path.len() != 1 {
                    return Err(AgentSettingError::Unsupported(
                        "dotenv settings must use one path segment".to_string(),
                    ));
                }
                update_json_path(&mut value, path, replacement.clone())?;
            }
            let object = value.as_object().expect("dotenv parser returns object");
            Ok(object
                .iter()
                .map(|(key, value)| format!("{key}={}", value.as_str().unwrap_or_default()))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        NativeConfigFormat::Text | NativeConfigFormat::Opaque => Err(
            AgentSettingError::Unsupported("typed edits are not supported for this format".into()),
        ),
    }
}

fn render_toml(
    current: &str,
    operations: &[(Vec<String>, Option<Value>)],
) -> Result<String, AgentSettingError> {
    let mut document = if current.trim().is_empty() {
        DocumentMut::new()
    } else {
        current.parse::<DocumentMut>().map_err(|error| {
            AgentSettingError::InvalidConfiguration(format!("TOML syntax: {error}"))
        })?
    };
    for (path, replacement) in operations {
        if path.is_empty() {
            return Err(AgentSettingError::InvalidRequest(
                "native setting path cannot be empty".to_string(),
            ));
        }
        let (parents, leaf) = path.split_at(path.len() - 1);
        let mut table = document.as_table_mut();
        for segment in parents {
            if !table.contains_key(segment) {
                table.insert(segment, Item::Table(toml_edit::Table::new()));
            }
            table = table
                .get_mut(segment)
                .and_then(Item::as_table_mut)
                .ok_or_else(|| {
                    AgentSettingError::InvalidConfiguration(format!(
                        "{} must be a TOML table",
                        parents.join(".")
                    ))
                })?;
        }
        if let Some(replacement) = replacement {
            let mut value = replacement
                .serialize(toml_edit::ser::ValueSerializer::new())
                .map_err(|error| {
                    AgentSettingError::ValidationFailed(format!(
                        "{} cannot be represented in TOML: {error}",
                        path.join(".")
                    ))
                })?;
            if let Some(existing) = table.get_mut(&leaf[0]) {
                let existing_value = existing.as_value_mut().ok_or_else(|| {
                    AgentSettingError::InvalidConfiguration(format!(
                        "{} has a TOML table where a value is required",
                        path.join(".")
                    ))
                })?;
                // Mutate the Item in place so comments attached to its TOML
                // key remain intact. Re-inserting the key discards that CST
                // decoration even if the value decoration is copied.
                *value.decor_mut() = existing_value.decor().clone();
                *existing_value = value;
            } else {
                table.insert(&leaf[0], Item::Value(value));
            }
        } else {
            table.remove(&leaf[0]);
        }
    }
    let rendered = document.to_string();
    parse_document(NativeConfigFormat::Toml, &rendered)?;
    Ok(rendered)
}

fn ensure_object_root(value: &mut Value) -> Result<(), AgentSettingError> {
    if value.is_null() {
        *value = Value::Object(Map::new());
    }
    if !value.is_object() {
        return Err(AgentSettingError::InvalidConfiguration(
            "native configuration root must be an object".to_string(),
        ));
    }
    Ok(())
}

fn update_json_path(
    root: &mut Value,
    path: &[String],
    replacement: Option<Value>,
) -> Result<(), AgentSettingError> {
    if path.is_empty() {
        return Err(AgentSettingError::InvalidRequest(
            "native setting path cannot be empty".to_string(),
        ));
    }
    let mut current = root;
    for segment in &path[..path.len() - 1] {
        let object = current.as_object_mut().ok_or_else(|| {
            AgentSettingError::InvalidConfiguration(format!("{} must be an object", path.join(".")))
        })?;
        if replacement.is_none() && !object.contains_key(segment) {
            return Ok(());
        }
        current = object
            .entry(segment.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    let object = current.as_object_mut().ok_or_else(|| {
        AgentSettingError::InvalidConfiguration(format!(
            "{} parent must be an object",
            path.join(".")
        ))
    })?;
    if let Some(replacement) = replacement {
        object.insert(path[path.len() - 1].clone(), replacement);
    } else {
        object.remove(&path[path.len() - 1]);
    }
    Ok(())
}

fn value_at_path<'a>(value: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, segment| current.get(segment))
}

fn value_at_segments<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, segment| current.get(*segment))
}

fn effective_string_at_path(files: &[ParsedNativeFile], path: &[&str]) -> Option<String> {
    for file in files.iter().rev() {
        let value = file
            .value
            .as_ref()
            .and_then(|value| value_at_segments(value, path))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if value.is_some() {
            return value;
        }
        if file.spec.dialect == NativeDialect::Cordis
            && path.get(1) == Some(&"config")
            && file
                .value
                .as_ref()
                .and_then(|value| value.get(path[0]))
                .and_then(|row| row.get("config"))
                .is_some()
        {
            // A present later Cordis config replaces the complete earlier
            // config. A missing/empty field uses the native default, not a
            // value from the shadowed profile's config.
            return None;
        }
    }
    None
}

fn managed_descendant_paths(
    descriptor: &SettingDescriptor,
    descriptors: &[SettingDescriptor],
    file_id: &str,
    scope: SettingScope,
    parent_path: &[String],
) -> Vec<Vec<String>> {
    descriptors
        .iter()
        .filter(|candidate| candidate.key != descriptor.key)
        .flat_map(|candidate| candidate.native_locations.iter())
        .filter(|location| {
            location.file_id == file_id
                && location.scope == scope
                && location.native_path.len() > parent_path.len()
                && location.native_path.starts_with(parent_path)
        })
        .map(|location| location.native_path[parent_path.len()..].to_vec())
        .collect()
}

fn remove_value_at_path(root: &mut Value, path: &[String]) -> bool {
    let Some((leaf, parents)) = path.split_last() else {
        return false;
    };
    let mut current = root;
    for segment in parents {
        let Some(next) = current
            .as_object_mut()
            .and_then(|object| object.get_mut(segment))
        else {
            return false;
        };
        current = next;
    }
    current
        .as_object_mut()
        .and_then(|object| object.remove(leaf))
        .is_some()
}

fn projected_setting_value(
    descriptor: &SettingDescriptor,
    descriptors: &[SettingDescriptor],
    file_id: &str,
    scope: SettingScope,
    native_path: &[String],
    value: &Value,
) -> Option<Value> {
    let mut projected = value.clone();
    let mut removed_managed_value = false;
    for path in managed_descendant_paths(descriptor, descriptors, file_id, scope, native_path) {
        removed_managed_value |= remove_value_at_path(&mut projected, &path);
    }
    if removed_managed_value && projected.as_object().is_some_and(serde_json::Map::is_empty) {
        None
    } else {
        Some(projected)
    }
}

fn preserve_managed_descendants(
    descriptor: &SettingDescriptor,
    descriptors: &[SettingDescriptor],
    file_id: &str,
    scope: SettingScope,
    parent_path: &[String],
    original_root: Option<&Value>,
    explicitly_mutated_paths: &BTreeSet<Vec<String>>,
    replacement: Option<Value>,
) -> Result<Option<Value>, AgentSettingError> {
    let managed_paths =
        managed_descendant_paths(descriptor, descriptors, file_id, scope, parent_path);
    if managed_paths.is_empty() {
        return Ok(replacement);
    }

    let mut replacement = replacement;
    for relative_path in managed_paths {
        // A parent map payload never owns a field with its own descriptor.
        // Strip such values from the parent operation, then retain the native
        // value unless this patch contains an explicit child operation.
        if let Some(value) = replacement.as_mut() {
            remove_value_at_path(value, &relative_path);
        }
        let mut absolute_path = parent_path.to_vec();
        absolute_path.extend(relative_path.iter().cloned());
        if explicitly_mutated_paths.contains(&absolute_path) {
            continue;
        }
        let Some(existing) = original_root.and_then(|value| value_at_path(value, &absolute_path))
        else {
            continue;
        };
        let target = replacement.get_or_insert_with(|| Value::Object(Map::new()));
        update_json_path(target, &relative_path, Some(existing.clone()))?;
    }
    Ok(replacement)
}

fn effective_setting(
    descriptor: &SettingDescriptor,
    descriptors: &[SettingDescriptor],
    files: &[ParsedNativeFile],
    include_sensitive_values: bool,
) -> EffectiveSetting {
    let mut sources = Vec::new();
    let mut warnings = Vec::new();
    for location in &descriptor.native_locations {
        let Some(file) = files.iter().find(|file| file.spec.id == location.file_id) else {
            continue;
        };
        if file.error.is_some() {
            warnings.push(format!(
                "{} could not be read from {}",
                descriptor.key.id(),
                file.spec.id
            ));
            continue;
        }
        if file.spec.dialect == NativeDialect::Cordis
            && location
                .native_path
                .get(1)
                .is_some_and(|key| key == "config")
        {
            let id = &location.native_path[0];
            // A later Cordis config replaces the entire earlier config. A
            // missing key in that later row is not inherited from this file.
            let later = files
                .iter()
                .skip_while(|candidate| candidate.spec.id != file.spec.id)
                .skip(1)
                .any(|candidate| {
                    candidate.spec.dialect == NativeDialect::Cordis
                        && candidate
                            .value
                            .as_ref()
                            .and_then(|value| value.get(id))
                            .and_then(|row| row.get("config"))
                            .is_some()
                });
            if later {
                continue;
            }
        }
        if let Some(value) = file
            .value
            .as_ref()
            .and_then(|value| value_at_path(value, &location.native_path))
        {
            let value = if descriptor.key.id() == "common.model"
                && file.spec.dialect == NativeDialect::Cordis
            {
                file.value
                    .as_ref()
                    .and_then(|value| value_at_segments(value, &["acp", "config", "provider"]))
                    .and_then(Value::as_str)
                    .zip(value.as_str())
                    .map(|(provider, model)| {
                        Value::String(serde_json::json!([provider, model]).to_string())
                    })
            } else if descriptor.sensitive {
                include_sensitive_values.then(|| value.clone())
            } else {
                projected_setting_value(
                    descriptor,
                    descriptors,
                    &file.spec.id,
                    location.scope,
                    &location.native_path,
                    value,
                )
            };
            if !descriptor.sensitive && value.is_none() {
                continue;
            }
            sources.push(SettingSourceValue {
                source: format!("native_{}", location.scope.label()),
                scope: location.scope,
                file_id: file.spec.id.clone(),
                value,
                configured: true,
                revision: file.revision.clone(),
            });
        }
    }
    // Location order is provider-owned and intentionally encodes effective
    // precedence. Current Codex/Claude/Gemini descriptors list user then
    // project; OMP exposes only verified user-writable settings.
    let effective = sources.last();
    EffectiveSetting {
        key: descriptor.key.clone(),
        effective_value: effective.and_then(|source| source.value.clone()),
        effective_source: effective.map(|source| source.source.clone()),
        configured: effective.is_some(),
        sources,
        warnings,
    }
}

fn unknown_native_nodes(
    descriptors: &[SettingDescriptor],
    files: &[ParsedNativeFile],
) -> Vec<UnknownNativeNode> {
    let known: BTreeSet<_> = descriptors
        .iter()
        .flat_map(|descriptor| descriptor.native_locations.iter())
        .map(|location| (location.file_id.clone(), location.native_path.join(".")))
        .collect();
    let mut output = Vec::new();
    for file in files {
        let Some(value) = &file.value else {
            continue;
        };
        let mut leaves = Vec::new();
        collect_leaf_nodes(value, String::new(), &mut leaves);
        for (path, value) in leaves {
            let managed = known.iter().any(|(file_id, known_path)| {
                file_id == &file.spec.id
                    && (path == *known_path || path.starts_with(&format!("{known_path}.")))
            });
            if !managed {
                output.push(UnknownNativeNode {
                    file_id: file.spec.id.clone(),
                    scope: file.spec.scope,
                    field_path: path,
                    value_kind: json_value_kind(&value).to_string(),
                });
            }
        }
    }
    output
}

fn json_value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn collect_leaf_nodes(value: &Value, prefix: String, output: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(object) if !object.is_empty() => {
            for (key, value) in object {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                collect_leaf_nodes(value, path, output);
            }
        }
        _ if !prefix.is_empty() => output.push((prefix, value.clone())),
        _ => {}
    }
}

fn validate_setting_value(
    descriptor: &SettingDescriptor,
    value: &Value,
) -> Result<(), AgentSettingError> {
    let type_matches = match descriptor.value_type {
        SettingValueType::String => value.is_string(),
        SettingValueType::Boolean => value.is_boolean(),
        SettingValueType::Number => value.is_number(),
        SettingValueType::StringList => value
            .as_array()
            .is_some_and(|values| values.iter().all(Value::is_string)),
        SettingValueType::StringMap => value
            .as_object()
            .is_some_and(|values| values.values().all(Value::is_string)),
        SettingValueType::Json => true,
    };
    if !type_matches {
        return Err(AgentSettingError::ValidationFailed(format!(
            "{} expects {:?}",
            descriptor.key.id(),
            descriptor.value_type
        )));
    }
    if descriptor.key.id() == "claude_code.model_settings" {
        let models = value.as_object().ok_or_else(|| {
            AgentSettingError::ValidationFailed(
                "Per-model settings must be an object keyed by canonical model name".into(),
            )
        })?;
        for (model, settings) in models {
            if model.trim().is_empty() || !settings.is_object() {
                return Err(AgentSettingError::ValidationFailed(
                    "Each model setting needs a model name and an object".into(),
                ));
            }
            for key in ["effortLevel", "maxEffortLevel"] {
                if let Some(effort) = settings.get(key)
                    && !matches!(effort.as_str(), Some("low" | "medium" | "high" | "xhigh"))
                {
                    return Err(AgentSettingError::ValidationFailed(format!(
                        "{model}.{key} must be low, medium, high or xhigh; max is session-only"
                    )));
                }
            }
        }
    }
    if descriptor.key.id() == "gemini.model_overrides" {
        let rules = value.as_array().ok_or_else(|| {
            AgentSettingError::ValidationFailed("Model overrides must be an array".into())
        })?;
        for rule in rules {
            if !rule.is_object() {
                return Err(AgentSettingError::ValidationFailed(
                    "Each model override must be an object".into(),
                ));
            }
            let Some(budget) =
                rule.pointer("/modelConfig/generateContentConfig/thinkingConfig/thinkingBudget")
            else {
                // Native overrides also configure generation settings and
                // aliases. A thinking-budget editor must not reject or erase
                // those independently valid rules.
                continue;
            };
            let model = rule.pointer("/match/model").and_then(Value::as_str);
            if model.is_none_or(|model| model.trim().is_empty())
                || budget.as_i64().is_none_or(|budget| budget < -1)
            {
                return Err(AgentSettingError::ValidationFailed("Each thinking budget override requires an explicit match.model and an integer thinkingBudget >= -1; model-specific limits are checked by Gemini".into()));
            }
        }
    }
    if descriptor.validation.pattern.as_deref() == Some("deepseek_model_pair") {
        deepseek_model_pair(value)?;
    }
    if descriptor.sensitive {
        let credential = value.as_str().ok_or_else(|| {
            AgentSettingError::ValidationFailed(format!(
                "{} expects a credential string",
                descriptor.key.id()
            ))
        })?;
        let trimmed = credential.trim();
        if trimmed.is_empty() {
            return Err(AgentSettingError::ValidationFailed(format!(
                "{} cannot be replaced with an empty value; use clear instead",
                descriptor.key.id()
            )));
        }
        if trimmed.chars().count() >= 3
            && trimmed
                .chars()
                .all(|character| matches!(character, '*' | '•'))
        {
            return Err(AgentSettingError::ValidationFailed(format!(
                "{} cannot be replaced with a redacted display value",
                descriptor.key.id()
            )));
        }
    }
    if descriptor.key.namespace == "common" && descriptor.key.name == "api_address" {
        validate_api_address(value.as_str().expect("API address descriptor is a string"))?;
    }
    if !descriptor.options.is_empty()
        && !descriptor
            .options
            .iter()
            .any(|option| option.value == *value)
    {
        return Err(AgentSettingError::ValidationFailed(format!(
            "{} is not one of the advertised options",
            descriptor.key.id()
        )));
    }
    if let Some(string) = value.as_str()
        && let Some(max_length) = descriptor.validation.max_length
        && string.chars().count() > max_length as usize
    {
        return Err(AgentSettingError::ValidationFailed(format!(
            "{} exceeds {max_length} characters",
            descriptor.key.id()
        )));
    }
    if let Some(number) = value.as_f64() {
        if descriptor
            .validation
            .minimum
            .is_some_and(|minimum| number < minimum)
        {
            return Err(AgentSettingError::ValidationFailed(format!(
                "{} is below its minimum",
                descriptor.key.id()
            )));
        }
        if descriptor
            .validation
            .maximum
            .is_some_and(|maximum| number > maximum)
        {
            return Err(AgentSettingError::ValidationFailed(format!(
                "{} exceeds its maximum",
                descriptor.key.id()
            )));
        }
    }
    Ok(())
}

fn validate_api_address(value: &str) -> Result<(), AgentSettingError> {
    let parsed = reqwest::Url::parse(value).map_err(|_| {
        AgentSettingError::ValidationFailed(
            "API address must be a complete HTTP or HTTPS URL".to_string(),
        )
    })?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(AgentSettingError::ValidationFailed(
            "API address must be a complete HTTP or HTTPS URL".to_string(),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AgentSettingError::ValidationFailed(
            "API address must not contain embedded credentials".to_string(),
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(AgentSettingError::ValidationFailed(
            "API address must not contain a query or fragment".to_string(),
        ));
    }
    Ok(())
}

fn require_expected_revision(
    expected: &BTreeMap<String, String>,
    file_id: &str,
    current: &str,
) -> Result<(), AgentSettingError> {
    let expected = expected.get(file_id).ok_or_else(|| {
        AgentSettingError::InvalidRequest(format!(
            "mutation requires the observed revision for {file_id}"
        ))
    })?;
    if expected != current {
        return Err(AgentSettingError::StaleRevision);
    }
    Ok(())
}

fn verify_operations(
    snapshot: &SettingsSnapshot,
    operations: &[SettingOperation],
    rendered: &[RenderedFile],
) -> Result<(), AgentSettingError> {
    for operation in operations {
        let (key, scope, expected) = match operation {
            SettingOperation::Preserve { .. } => continue,
            SettingOperation::Replace { key, scope, value } => (key, scope, Some(value)),
            SettingOperation::Clear { key, scope } => (key, scope, None),
        };
        let observed = snapshot
            .effective_settings
            .iter()
            .find(|setting| setting.key == *key)
            .and_then(|setting| {
                setting.sources.iter().rev().find(|source| {
                    source.scope == *scope
                        && rendered.iter().any(|file| file.spec.id == source.file_id)
                })
            });
        let matches = match expected {
            Some(expected) => observed
                .and_then(|source| source.value.as_ref())
                .is_some_and(|value| value == expected),
            None => observed.is_none(),
        };
        if !matches {
            return Err(AgentSettingError::VerificationFailed(format!(
                "{} did not round-trip at {} scope",
                key.id(),
                scope.label()
            )));
        }
    }
    Ok(())
}

fn write_all_or_restore(files: &[RenderedFile]) -> Result<(), AgentSettingError> {
    for file in files {
        crate::agent_tools::ensure_native_root_safe(&file.spec.path).map_err(|_| {
            AgentSettingError::UnsafePath(
                "native configuration path contains a symbolic link or reparse point".into(),
            )
        })?;
        let observed = match fs::read(&file.spec.path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if observed != file.before {
            return Err(AgentSettingError::StaleRevision);
        }
    }
    let mut written = Vec::new();
    for file in files {
        if file.before.as_deref() == Some(file.after.as_slice()) {
            continue;
        }
        if let Err(error) = atomic_write(&file.spec.path, &file.after) {
            if let Err(rollback_error) = rollback_files(&written) {
                return Err(AgentSettingError::RollbackFailed(format!(
                    "write failed: {error}; {rollback_error}"
                )));
            }
            return Err(error);
        }
        written.push(file.clone());
    }
    Ok(())
}

fn rollback_files(files: &[RenderedFile]) -> Result<(), AgentSettingError> {
    let mut errors = Vec::new();
    for file in files.iter().rev() {
        let result = if let Some(before) = &file.before {
            atomic_write(&file.spec.path, before)
        } else {
            match fs::remove_file(&file.spec.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            }
        };
        if let Err(error) = result {
            errors.push(format!("{}: {error}", file.spec.path.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(AgentSettingError::RollbackFailed(errors.join("; ")))
    }
}

fn revision_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), AgentSettingError> {
    let parent = path.parent().ok_or_else(|| {
        AgentSettingError::UnsafePath("write target has no parent directory".to_string())
    })?;
    fs::create_dir_all(parent)?;
    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| AgentSettingError::Io(error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::executors::BaseCodingAgent;

    struct Harness {
        _root: TempDir,
        home: PathBuf,
        project: PathBuf,
        service: AgentSettingsService,
    }

    fn harness() -> Harness {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let project = root.path().join("project");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&project).unwrap();
        let service = AgentSettingsService::new(
            home.clone(),
            root.path().join("assets/agent-settings/profiles-v1.json"),
        );
        Harness {
            _root: root,
            home,
            project,
            service,
        }
    }

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn snapshot<'a>(
        inventory: &'a AgentSettingsInventory,
        provider: AgentSettingsProvider,
    ) -> &'a SettingsSnapshot {
        inventory
            .providers
            .iter()
            .find(|snapshot| snapshot.provider == provider)
            .unwrap()
    }

    fn setting<'a>(snapshot: &'a SettingsSnapshot, key: &str) -> &'a EffectiveSetting {
        snapshot
            .effective_settings
            .iter()
            .find(|setting| setting.key.id() == key)
            .unwrap()
    }

    fn descriptor_for<'a>(snapshot: &'a SettingsSnapshot, key: &str) -> &'a SettingDescriptor {
        snapshot
            .descriptors
            .iter()
            .find(|descriptor| descriptor.key.id() == key)
            .unwrap()
    }

    fn native_patch(
        snapshot: &SettingsSnapshot,
        operations: Vec<SettingOperation>,
    ) -> SettingsPatch {
        SettingsPatch {
            provider: snapshot.provider,
            project_path: None,
            expected_file_revisions: snapshot
                .native_files
                .iter()
                .filter_map(|file| {
                    file.revision
                        .as_ref()
                        .map(|revision| (file.file_id.clone(), revision.clone()))
                })
                .collect(),
            operations,
        }
    }

    #[test]
    fn native_model_settings_validate_scope_without_fabricating_per_attempt_support() {
        let claude = claude_descriptors()
            .into_iter()
            .find(|descriptor| descriptor.key.id() == "claude_code.model_settings")
            .unwrap();
        assert!(!claude.capabilities.run_override);
        assert!(
            validate_setting_value(
                &claude,
                &json!({"claude-model":{"effortLevel":"xhigh", "futureOption":true}})
            )
            .is_ok()
        );
        for value in [
            json!({"claude-model":{"effortLevel":"max"}}),
            json!({"claude-model":{"maxEffortLevel":"ultracode"}}),
            json!({"claude-model":{"effortLevel":true}}),
            json!({" ":{"effortLevel":"high"}}),
        ] {
            assert!(validate_setting_value(&claude, &value).is_err());
        }

        let gemini = gemini_descriptors()
            .into_iter()
            .find(|descriptor| descriptor.key.id() == "gemini.model_overrides")
            .unwrap();
        assert!(!gemini.capabilities.run_override);
        for budget in [json!(-1), json!(0), json!(1024)] {
            assert!(validate_setting_value(&gemini, &json!([{
                "match":{"model":"gemini-2.5-pro"},
                "modelConfig":{"generateContentConfig":{"thinkingConfig":{"thinkingBudget":budget}}}
            }])).is_ok());
        }
        for budget in [
            json!(-2),
            json!(1.5),
            json!("1024"),
            json!(null),
            json!(true),
        ] {
            assert!(validate_setting_value(&gemini, &json!([{
                "match":{"model":"gemini-2.5-pro"},
                "modelConfig":{"generateContentConfig":{"thinkingConfig":{"thinkingBudget":budget}}}
            }])).is_err());
        }
        assert!(validate_setting_value(&gemini, &json!([{
            "match":{"model":" "},
            "modelConfig":{"generateContentConfig":{"thinkingConfig":{"thinkingBudget":1024}}}
        }])).is_err());
    }

    #[test]
    fn gemini_budget_edit_preserves_non_budget_overrides_aliases_and_unknown_fields() {
        let harness = harness();
        let native = harness.home.join(".gemini/settings.json");
        let ordinary_rule = json!({
            "match":{"overrideScope":"agent"},
            "modelConfig":{"generateContentConfig":{"temperature":0.2}},
            "futureMetadata":{"keep":true}
        });
        let original = json!({
            "modelConfigs":{
                "customAliases":{"local-alias":{"extends":"native-alias"}},
                "overrides":[ordinary_rule.clone()]
            },
            "futureNativeSettings":{"keep":true}
        });
        write(&native, &serde_json::to_string_pretty(&original).unwrap());
        let manager = harness.service.manager(AgentSettingsProvider::Gemini, None);
        let before = manager.discover().unwrap();
        let overrides = json!([
            ordinary_rule,
            {"match":{"model":"gemini-2.5-pro"}, "modelConfig":{"generateContentConfig":{"thinkingConfig":{"thinkingBudget":1024}}}}
        ]);
        let patch = native_patch(
            &before,
            vec![SettingOperation::Replace {
                key: SettingKey::new("gemini", "model_overrides"),
                scope: SettingScope::User,
                value: overrides.clone(),
            }],
        );
        manager.diff(&patch).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&native).unwrap()).unwrap(),
            original
        );
        manager.apply(&patch).unwrap();
        let after: Value = serde_json::from_slice(&fs::read(&native).unwrap()).unwrap();
        assert_eq!(after["modelConfigs"]["overrides"], overrides);
        assert_eq!(
            after["modelConfigs"]["customAliases"],
            original["modelConfigs"]["customAliases"]
        );
        assert_eq!(
            after["futureNativeSettings"],
            original["futureNativeSettings"]
        );
        assert!(
            !after
                .to_string()
                .contains("GEMINI_CLI_SYSTEM_SETTINGS_PATH")
        );
        let bytes = fs::read(&native).unwrap();
        let invalid = native_patch(
            &manager.discover().unwrap(),
            vec![SettingOperation::Replace {
                key: SettingKey::new("gemini", "model_overrides"),
                scope: SettingScope::User,
                value: json!([{"match":{"model":"gemini-2.5-pro"}, "modelConfig":{"generateContentConfig":{"thinkingConfig":{"thinkingBudget":-2}}}}]),
            }],
        );
        assert!(matches!(
            manager.apply(&invalid),
            Err(AgentSettingError::ValidationFailed(_))
        ));
        assert_eq!(fs::read(native).unwrap(), bytes);
    }

    #[test]
    fn claude_per_model_effort_edit_preserves_other_models_and_unknown_native_options() {
        let harness = harness();
        let native = harness.home.join(".claude/settings.json");
        let original = json!({
            "modelSettings":{
                "claude-model":{"effortLevel":"medium", "futureModelFlag":true},
                "other-model":{"futureSetting":{"keep":true}}
            },
            "env":{"ANTHROPIC_BASE_URL":"https://example.test/v1"},
            "futureNativeOption":{"keep":true}
        });
        write(&native, &serde_json::to_string_pretty(&original).unwrap());
        let manager = harness
            .service
            .manager(AgentSettingsProvider::ClaudeCode, None);
        let mut settings = original["modelSettings"].clone();
        settings["claude-model"]["effortLevel"] = json!("xhigh");
        let patch = native_patch(
            &manager.discover().unwrap(),
            vec![SettingOperation::Replace {
                key: SettingKey::new("claude_code", "model_settings"),
                scope: SettingScope::User,
                value: settings.clone(),
            }],
        );
        manager.apply(&patch).unwrap();
        let after: Value = serde_json::from_slice(&fs::read(native).unwrap()).unwrap();
        assert_eq!(after["modelSettings"], settings);
        assert_eq!(after["env"], original["env"]);
        assert_eq!(after["futureNativeOption"], original["futureNativeOption"]);
    }

    #[test]
    fn opencode_json_extensions_use_lossless_jsonc_and_selected_route() {
        let harness = harness();
        let config = harness.home.join(".config/opencode/opencode.json");
        write(
            &config,
            "{\n// preserved comment\n\"model\": \"vendor/old\",\n\"provider\": {\"vendor\": {\"options\": {\"baseURL\": \"https://example.test/v1\"}}},\n\"unknown\": {\"retained\": true}\n}\n",
        );
        let manager = harness
            .service
            .manager(AgentSettingsProvider::Opencode, None);
        let snapshot = manager.discover().unwrap();
        assert_eq!(
            setting(&snapshot, "common.model").effective_value,
            Some(json!("vendor/old"))
        );
        assert_eq!(
            setting(&snapshot, "common.api_address").effective_value,
            Some(json!("https://example.test/v1"))
        );
        let patch = native_patch(
            &snapshot,
            vec![SettingOperation::Replace {
                key: SettingKey::new("common", "model"),
                scope: SettingScope::User,
                value: json!("vendor/new"),
            }],
        );
        manager.apply(&patch).unwrap();
        let observed = fs::read_to_string(&config).unwrap();
        assert!(observed.contains("// preserved comment"));
        assert!(observed.contains("retained"));
        assert!(matches!(
            manager.apply(&patch),
            Err(AgentSettingError::StaleRevision)
        ));
        assert!(
            !harness
                .home
                .join(".config/opencode/opencode.jsonc")
                .exists()
        );
    }

    #[test]
    fn opencode_credentials_are_redacted_and_oauth_entries_are_protected() {
        let harness = harness();
        write(
            &harness.home.join(".config/opencode/opencode.jsonc"),
            "{\"model\":\"vendor/model\"}",
        );
        let auth = harness.home.join(".local/share/opencode/auth.json");
        write(
            &auth,
            r#"{"vendor":{"type":"api","key":"fixture-key"},"other":{"type":"oauth","refresh":"keep-refresh"}}"#,
        );
        let manager = harness
            .service
            .manager(AgentSettingsProvider::Opencode, None);
        let snapshot = manager.discover().unwrap();
        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert!(!serialized.contains("fixture-key"));
        assert!(!serialized.contains("keep-refresh"));
        let patch = native_patch(
            &snapshot,
            vec![SettingOperation::Replace {
                key: SettingKey::new("common", "api_key"),
                scope: SettingScope::User,
                value: json!("replacement-key"),
            }],
        );
        manager.apply(&patch).unwrap();
        let value: Value = serde_json::from_slice(&fs::read(&auth).unwrap()).unwrap();
        assert_eq!(value["vendor"]["type"], "api");
        assert_eq!(value["other"]["refresh"], "keep-refresh");
        write(
            &auth,
            r#"{"vendor":{"type":"oauth","refresh":"native-token"}}"#,
        );
        let before = fs::read(&auth).unwrap();
        let patch = native_patch(
            &manager.discover().unwrap(),
            vec![SettingOperation::Replace {
                key: SettingKey::new("common", "api_key"),
                scope: SettingScope::User,
                value: json!("replacement-key"),
            }],
        );
        assert!(matches!(
            manager.apply(&patch),
            Err(AgentSettingError::Unsupported(_))
        ));
        assert_eq!(fs::read(&auth).unwrap(), before);
    }

    #[test]
    fn dsh_model_pair_write_preserves_entire_inherited_config_and_comments() {
        let harness = harness();
        let home = harness.home.join(".dsh");
        let profile = home.join("profiles/acp/cordis.patch.yml");
        let original = "# native profile\n- id: acp\n  config:\n    provider: deepseek-official\n    model: old\n    sessionListPageSize: 23\n- id: llm-deepseek\n  config:\n    reasoningEffort: high\n    apiKeyEnv: VK_DSH_FIXTURE_CRED\n";
        write(&profile, original);
        let patch_file = home.join("cordis.patch.yml");
        write(
            &patch_file,
            "# home comment\n- id: unrelated\n  config:\n    keep: true\n",
        );
        let manager = harness
            .service
            .manager(AgentSettingsProvider::DeepseekHarness, None);
        let snapshot = manager.discover().unwrap();
        assert_eq!(
            setting(&snapshot, "common.model").effective_value,
            Some(json!("[\"deepseek-official\",\"old\"]"))
        );
        assert_eq!(
            setting(&snapshot, "common.reasoning_effort").effective_value,
            Some(json!("high"))
        );
        let patch = native_patch(
            &snapshot,
            vec![SettingOperation::Replace {
                key: SettingKey::new("common", "model"),
                scope: SettingScope::User,
                value: json!("[\"deepseek-official\",\"new\"]"),
            }],
        );
        let after = manager.apply(&patch).unwrap();
        assert_eq!(
            setting(&after, "common.model").effective_value,
            Some(json!("[\"deepseek-official\",\"new\"]"))
        );
        let output = fs::read_to_string(&patch_file).unwrap();
        assert!(output.contains("# home comment"));
        let native = native_assets::parse_cordis_patch(&output).unwrap();
        assert_eq!(native["acp"]["config"]["sessionListPageSize"], 23);
        assert_eq!(native["unrelated"]["config"]["keep"], true);
        assert_eq!(fs::read_to_string(profile).unwrap(), original);
        let invalid = native_patch(
            &after,
            vec![SettingOperation::Replace {
                key: SettingKey::new("common", "model"),
                scope: SettingScope::User,
                value: json!("unadvertised-model"),
            }],
        );
        assert!(matches!(
            manager.diff(&invalid),
            Err(AgentSettingError::ValidationFailed(_))
        ));
    }

    #[test]
    fn dsh_new_home_config_rejects_unsafe_inherited_profiles_without_writes() {
        for original in [
            "- id: acp\n  config: !expr native-config\n",
            "- id: acp\n  config: &route\n    provider: route\n    model: old\n- id: other\n  config: *route\n",
            "- include: native-profile\n",
        ] {
            for existing_home in [
                None,
                Some("# keep\n- id: unrelated\n  config:\n    keep: true\n"),
            ] {
                let harness = harness();
                let home = harness.home.join(".dsh");
                let profile = home.join("profiles/acp/cordis.patch.yml");
                let patch_file = home.join("cordis.patch.yml");
                write(&profile, original);
                if let Some(content) = existing_home {
                    write(&patch_file, content);
                }
                let manager = harness
                    .service
                    .manager(AgentSettingsProvider::DeepseekHarness, None);
                let snapshot = manager.discover().unwrap();
                let patch = native_patch(
                    &snapshot,
                    vec![SettingOperation::Replace {
                        key: SettingKey::new("common", "model"),
                        scope: SettingScope::User,
                        value: json!("[\"route\",\"new\"]"),
                    }],
                );
                assert!(matches!(
                    manager.diff(&patch),
                    Err(AgentSettingError::InvalidConfiguration(_))
                ));
                assert!(matches!(
                    manager.apply(&patch),
                    Err(AgentSettingError::InvalidConfiguration(_))
                ));
                assert_eq!(fs::read_to_string(&profile).unwrap(), original);
                if let Some(content) = existing_home {
                    assert_eq!(fs::read_to_string(&patch_file).unwrap(), content);
                } else {
                    assert!(!patch_file.exists());
                }
            }
        }
    }

    #[test]
    fn dsh_home_config_seeding_preserves_row_metadata_without_copying_plugin_insert() {
        let harness = harness();
        let home = harness.home.join(".dsh");
        let profile = home.join("profiles/acp/cordis.patch.yml");
        let original = "- insert:\n  - id: acp\n    name: native-acp\n    config:\n      provider: route\n      model: old\n      sessionListPageSize: 23\n";
        write(&profile, original);
        let patch_file = home.join("cordis.patch.yml");
        write(
            &patch_file,
            "# home metadata\n- id: acp\n  disabled: true\n  customMetadata: retained\n",
        );
        let manager = harness
            .service
            .manager(AgentSettingsProvider::DeepseekHarness, None);
        let patch = native_patch(
            &manager.discover().unwrap(),
            vec![SettingOperation::Replace {
                key: SettingKey::new("common", "model"),
                scope: SettingScope::User,
                value: json!("[\"route\",\"new\"]"),
            }],
        );
        manager.apply(&patch).unwrap();
        let output = fs::read_to_string(&patch_file).unwrap();
        let native = native_assets::parse_cordis_patch(&output).unwrap();
        assert!(output.contains("# home metadata"));
        assert!(!output.contains("insert:"));
        assert_eq!(native["acp"]["disabled"], true);
        assert_eq!(native["acp"]["customMetadata"], "retained");
        assert_eq!(native["acp"]["config"]["model"], "new");
        assert_eq!(native["acp"]["config"]["sessionListPageSize"], 23);
        assert!(native["acp"].get("name").is_none());
        assert_eq!(fs::read_to_string(profile).unwrap(), original);
    }

    #[test]
    fn dsh_replaced_config_without_api_key_env_selects_default_credential() {
        let harness = harness();
        let home = harness.home.join(".dsh");
        write(
            &home.join("profiles/acp/cordis.patch.yml"),
            "- id: llm-deepseek\n  config:\n    apiKeyEnv: VK_DSH_SHADOWED_CRED\n",
        );
        write(
            &home.join("cordis.patch.yml"),
            "- id: llm-deepseek\n  config:\n    baseURL: https://example.test/anthropic\n",
        );
        write(
            &home.join(".env"),
            "VK_DSH_SHADOWED_CRED=shadowed-env\nDEEPSEEK_API_KEY=default-env\n",
        );
        let manager = harness
            .service
            .manager(AgentSettingsProvider::DeepseekHarness, None);
        let snapshot = manager.discover().unwrap();
        let key = descriptor_for(&snapshot, "common.api_key");
        assert!(!key.native_locations.is_empty());
        assert!(key.native_locations.iter().all(|location| {
            location.native_path.last().map(String::as_str) == Some("DEEPSEEK_API_KEY")
        }));
        let resolve_native_credential = || {
            // Keep this fixture independent from credentials in the test
            // runner's real environment; native launch specs still use the
            // same credential-key resolver as the descriptors.
            let files: Vec<_> = manager
                .file_specs()
                .into_iter()
                .filter(|file| file.id != "launch_environment")
                .map(read_native_file)
                .collect();
            let descriptors = manager.descriptors_for_files(&files);
            let key = descriptors
                .iter()
                .find(|descriptor| descriptor.key.id() == "common.api_key")
                .unwrap();
            effective_setting(key, &descriptors, &files, true)
        };
        assert_eq!(
            resolve_native_credential().effective_value,
            Some(json!("default-env"))
        );
        write(
            &home.join(".credentials.yaml"),
            "version: 1\nrefs:\n  VK_DSH_SHADOWED_CRED: shadowed-key\n  DEEPSEEK_API_KEY: default-key\nrecords: {}\n",
        );
        assert_eq!(
            resolve_native_credential().effective_value,
            Some(json!("default-key"))
        );
        for file in manager
            .file_specs()
            .into_iter()
            .filter(|file| file.id == "launch_environment")
        {
            let value = read_native_file(file).value.unwrap();
            assert!(value.get("VK_DSH_SHADOWED_CRED").is_none());
        }
    }

    #[test]
    fn dsh_whole_config_shadowing_and_credential_clear_follow_native_semantics() {
        let harness = harness();
        let home = harness.home.join(".dsh");
        write(
            &home.join("profiles/acp/cordis.patch.yml"),
            "- id: llm-deepseek\n  config:\n    reasoningEffort: high\n    apiKeyEnv: VK_DSH_FIXTURE_CRED\n",
        );
        write(
            &home.join("cordis.patch.yml"),
            "- id: llm-deepseek\n  config:\n    apiKeyEnv: VK_DSH_FIXTURE_CRED\n    baseURL: https://example.test/anthropic\n",
        );
        let credentials = home.join(".credentials.yaml");
        write(
            &credentials,
            "# credentials\nversion: 1\nrefs:\n  VK_DSH_FIXTURE_CRED: fixture-key # secret comment\nrecords: {} # native grants\n",
        );
        let manager = harness
            .service
            .manager(AgentSettingsProvider::DeepseekHarness, None);
        let snapshot = manager.discover().unwrap();
        assert!(!setting(&snapshot, "common.reasoning_effort").configured);
        assert!(setting(&snapshot, "common.api_key").configured);
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("fixture-key")
        );
        let revision = snapshot
            .native_files
            .iter()
            .find(|file| file.file_id == "user_credentials")
            .unwrap()
            .revision
            .as_deref()
            .unwrap();
        assert_eq!(
            manager
                .reveal(
                    &SettingKey::new("common", "api_key"),
                    SettingScope::User,
                    revision
                )
                .unwrap()
                .value,
            "fixture-key"
        );
        let patch = native_patch(
            &snapshot,
            vec![SettingOperation::Clear {
                key: SettingKey::new("common", "api_key"),
                scope: SettingScope::User,
            }],
        );
        manager.apply(&patch).unwrap();
        let output = fs::read_to_string(credentials).unwrap();
        assert!(output.contains("# credentials"));
        assert!(output.contains("# native grants"));
        assert!(
            native_assets::parse_yaml(&output).unwrap()["refs"]
                .as_object()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn four_provider_discovery_is_read_only_and_isolates_malformed_files() {
        let harness = harness();
        let codex = harness.home.join(".codex/config.toml");
        let claude = harness.home.join(".claude/settings.json");
        let gemini = harness.home.join(".gemini/settings.json");
        let omp = harness.home.join(".omp/agent/config.yml");
        write(&codex, "# keep me\nmodel = \"gpt-5\"\nunknown = 7\n");
        write(&claude, r#"{"model":"sonnet","unknown":{"keep":true}}"#);
        write(&gemini, r#"{"apiKey":"secret-do-not-project", malformed"#);
        write(
            &omp,
            "theme:\n  dark: titanium\ncompaction:\n  enabled: true\nunknown: preserved\n",
        );
        let before = [
            fs::read(&codex).unwrap(),
            fs::read(&claude).unwrap(),
            fs::read(&gemini).unwrap(),
            fs::read(&omp).unwrap(),
        ];

        let inventory = harness.service.discover(None, Some(&harness.project));

        assert_eq!(inventory.providers.len(), 6);
        assert_eq!(
            snapshot(&inventory, AgentSettingsProvider::Codex)
                .errors
                .len(),
            0
        );
        assert_eq!(
            snapshot(&inventory, AgentSettingsProvider::ClaudeCode)
                .errors
                .len(),
            0
        );
        assert_eq!(
            snapshot(&inventory, AgentSettingsProvider::Gemini)
                .errors
                .len(),
            1
        );
        let gemini_public =
            serde_json::to_string(snapshot(&inventory, AgentSettingsProvider::Gemini)).unwrap();
        assert!(!gemini_public.contains("secret-do-not-project"));
        assert!(!gemini_public.contains(&harness.home.display().to_string()));
        assert!(!gemini_public.contains("executable_path"));
        assert_eq!(
            snapshot(&inventory, AgentSettingsProvider::OhMyPi)
                .errors
                .len(),
            0
        );
        assert_eq!(before[0], fs::read(&codex).unwrap());
        assert_eq!(before[1], fs::read(&claude).unwrap());
        assert_eq!(before[2], fs::read(&gemini).unwrap());
        assert_eq!(before[3], fs::read(&omp).unwrap());
        assert!(
            snapshot(&inventory, AgentSettingsProvider::Codex)
                .unknown_native_nodes
                .iter()
                .any(|node| node.field_path == "unknown")
        );
        let codex_snapshot = snapshot(&inventory, AgentSettingsProvider::Codex);
        let public_snapshot = serde_json::to_value(codex_snapshot).unwrap();
        assert!(
            public_snapshot["native_files"]
                .as_array()
                .unwrap()
                .iter()
                .all(|file| file.get("path").is_none())
        );
        assert!(
            public_snapshot["unknown_native_nodes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|node| node.get("value").is_none())
        );
        assert!(
            !serde_json::to_string(codex_snapshot)
                .unwrap()
                .contains(&harness.home.display().to_string())
        );
        let user_config = codex_snapshot
            .native_files
            .iter()
            .find(|file| file.file_id == "user_config")
            .unwrap();
        assert!(
            user_config
                .managed_setting_keys
                .contains(&SettingKey::new("common", "model"))
        );
    }

    #[test]
    fn connection_descriptors_follow_each_provider_native_strategy() {
        let harness = harness();
        write(
            &harness.home.join(".codex/config.toml"),
            "model_provider = \"azure\"\n",
        );
        write(
            &harness.home.join(".omp/agent/config.yml"),
            "modelRoles:\n  default: openrouter/anthropic/claude-sonnet\n",
        );

        let inventory = harness.service.discover(None, Some(&harness.project));
        let expected = [
            (
                AgentSettingsProvider::Codex,
                "user_config",
                vec!["model_providers", "azure", "base_url"],
                "user_auth",
                vec!["OPENAI_API_KEY"],
            ),
            (
                AgentSettingsProvider::ClaudeCode,
                "user_settings",
                vec!["env", "ANTHROPIC_BASE_URL"],
                "user_settings",
                vec!["env", "ANTHROPIC_API_KEY"],
            ),
            (
                AgentSettingsProvider::Gemini,
                "user_env",
                vec!["GOOGLE_GEMINI_BASE_URL"],
                "user_env",
                vec!["GEMINI_API_KEY"],
            ),
            (
                AgentSettingsProvider::OhMyPi,
                "user_models",
                vec!["providers", "openrouter", "baseUrl"],
                "user_models",
                vec!["providers", "openrouter", "apiKey"],
            ),
        ];
        for (provider, address_file, address_path, key_file, key_path) in expected {
            let snapshot = snapshot(&inventory, provider);
            let address = descriptor_for(snapshot, "common.api_address");
            let key = descriptor_for(snapshot, "common.api_key");
            assert!(!address.sensitive);
            assert!(key.sensitive);
            assert_eq!(address.native_locations[0].file_id, address_file);
            assert_eq!(address.native_locations[0].native_path, address_path);
            assert_eq!(key.native_locations[0].file_id, key_file);
            assert_eq!(key.native_locations[0].native_path, key_path);
        }
    }

    #[test]
    fn settings_files_follow_the_runtime_provider_roots() {
        let harness = harness();
        let codex = harness
            .service
            .manager(AgentSettingsProvider::Codex, Some(&harness.project));
        let codex_files = codex.file_specs();
        assert_eq!(
            codex_files
                .iter()
                .find(|file| file.id == "user_config")
                .unwrap()
                .path,
            codex_home_for(&harness.home).join("config.toml")
        );

        let oh_my_pi = harness
            .service
            .manager(AgentSettingsProvider::OhMyPi, Some(&harness.project));
        let oh_my_pi_files = oh_my_pi.file_specs();
        assert_eq!(
            oh_my_pi_files
                .iter()
                .find(|file| file.id == "user_config")
                .unwrap()
                .path,
            oh_my_pi_agent_root_for(&harness.home).join("config.yml")
        );
    }

    #[test]
    fn ordinary_environment_is_visible_while_credentials_require_explicit_reveal() {
        let harness = harness();
        write(
            &harness.home.join(".claude/settings.json"),
            r#"{
                "model":"sonnet",
                "env":{
                    "NORMAL":"visible",
                    "ANTHROPIC_BASE_URL":"https://gateway.example/proxy/v1",
                    "ANTHROPIC_API_KEY":"secret-key",
                    "ANTHROPIC_AUTH_TOKEN":"secret-token"
                }
            }"#,
        );
        let manager = harness
            .service
            .manager(AgentSettingsProvider::ClaudeCode, Some(&harness.project));
        let snapshot = manager.discover().unwrap();

        assert_eq!(
            setting(&snapshot, "common.environment").effective_value,
            Some(json!({"NORMAL": "visible"}))
        );
        assert_eq!(
            setting(&snapshot, "common.api_address").effective_value,
            Some(json!("https://gateway.example/proxy/v1"))
        );
        assert!(setting(&snapshot, "common.api_key").configured);
        assert_eq!(setting(&snapshot, "common.api_key").effective_value, None);
        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert!(!serialized.contains("secret-key"));
        assert!(!serialized.contains("secret-token"));
        let revision = snapshot
            .effective_settings
            .iter()
            .find(|setting| setting.key.id() == "common.api_key")
            .and_then(|setting| setting.sources.first())
            .map(|source| source.revision.clone())
            .unwrap();

        let revealed = manager
            .reveal(
                &SettingKey::new("common", "api_key"),
                SettingScope::User,
                &revision,
            )
            .unwrap();
        assert_eq!(revealed.value, "secret-key");
        assert!(matches!(
            manager.reveal(
                &SettingKey::new("common", "api_key"),
                SettingScope::User,
                "stale-revision",
            ),
            Err(AgentSettingError::StaleRevision)
        ));
        assert!(matches!(
            manager.reveal(
                &SettingKey::new("common", "model"),
                SettingScope::User,
                &revision,
            ),
            Err(AgentSettingError::Unsupported(_))
        ));
    }

    #[test]
    fn environment_edits_preserve_connection_fields_and_key_clear_is_explicit() {
        let harness = harness();
        let path = harness.home.join(".claude/settings.json");
        write(
            &path,
            r#"{"env":{"NORMAL":"old","ANTHROPIC_BASE_URL":"https://old.example/v1","ANTHROPIC_API_KEY":"old-secret"}}"#,
        );
        let manager = harness
            .service
            .manager(AgentSettingsProvider::ClaudeCode, Some(&harness.project));
        let revision = manager.discover().unwrap().native_files[0]
            .revision
            .clone()
            .unwrap();
        let result = manager
            .apply(&SettingsPatch {
                provider: AgentSettingsProvider::ClaudeCode,
                project_path: Some(harness.project.display().to_string()),
                expected_file_revisions: BTreeMap::from([("user_settings".into(), revision)]),
                operations: vec![SettingOperation::Replace {
                    key: SettingKey::new("common", "environment"),
                    scope: SettingScope::User,
                    value: json!({"NORMAL": "new"}),
                }],
            })
            .unwrap();
        assert_eq!(
            setting(&result, "common.environment").effective_value,
            Some(json!({"NORMAL": "new"}))
        );
        let native: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(native["env"]["ANTHROPIC_API_KEY"], "old-secret");
        assert_eq!(
            native["env"]["ANTHROPIC_BASE_URL"],
            "https://old.example/v1"
        );

        let revision = result.native_files[0].revision.clone().unwrap();
        let replaced = manager
            .apply(&SettingsPatch {
                provider: AgentSettingsProvider::ClaudeCode,
                project_path: Some(harness.project.display().to_string()),
                expected_file_revisions: BTreeMap::from([("user_settings".into(), revision)]),
                operations: vec![
                    SettingOperation::Replace {
                        key: SettingKey::new("common", "api_address"),
                        scope: SettingScope::User,
                        value: json!("https://new.example/base/v2"),
                    },
                    SettingOperation::Replace {
                        key: SettingKey::new("common", "api_key"),
                        scope: SettingScope::User,
                        value: json!("new-secret"),
                    },
                ],
            })
            .unwrap();
        assert_eq!(
            setting(&replaced, "common.api_address").effective_value,
            Some(json!("https://new.example/base/v2"))
        );
        assert!(
            !serde_json::to_string(&replaced)
                .unwrap()
                .contains("new-secret")
        );

        let revision = replaced.native_files[0].revision.clone().unwrap();
        manager
            .apply(&SettingsPatch {
                provider: AgentSettingsProvider::ClaudeCode,
                project_path: Some(harness.project.display().to_string()),
                expected_file_revisions: BTreeMap::from([("user_settings".into(), revision)]),
                operations: vec![SettingOperation::Clear {
                    key: SettingKey::new("common", "api_key"),
                    scope: SettingScope::User,
                }],
            })
            .unwrap();
        let native: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert!(native["env"].get("ANTHROPIC_API_KEY").is_none());
        assert_eq!(native["env"]["NORMAL"], "new");
    }

    #[test]
    fn connection_validation_rejects_embedded_credentials_query_and_masked_keys() {
        let harness = harness();
        let path = harness.home.join(".claude/settings.json");
        write(&path, r#"{"env":{}}"#);
        let manager = harness
            .service
            .manager(AgentSettingsProvider::ClaudeCode, Some(&harness.project));
        let revision = manager.discover().unwrap().native_files[0]
            .revision
            .clone()
            .unwrap();
        let patch = |key: SettingKey, value: Value| SettingsPatch {
            provider: AgentSettingsProvider::ClaudeCode,
            project_path: Some(harness.project.display().to_string()),
            expected_file_revisions: BTreeMap::from([("user_settings".into(), revision.clone())]),
            operations: vec![SettingOperation::Replace {
                key,
                scope: SettingScope::User,
                value,
            }],
        };
        for address in [
            "https://user:pass@example.com/v1",
            "https://example.com/v1?token=secret",
            "https://example.com/v1#token",
            "ftp://example.com/v1",
        ] {
            assert!(matches!(
                manager.diff(&patch(
                    SettingKey::new("common", "api_address"),
                    json!(address)
                )),
                Err(AgentSettingError::ValidationFailed(_))
            ));
        }
        for credential in ["", "******", "••••••"] {
            assert!(matches!(
                manager.diff(&patch(
                    SettingKey::new("common", "api_key"),
                    json!(credential)
                )),
                Err(AgentSettingError::ValidationFailed(_))
            ));
        }
        assert_eq!(fs::read_to_string(path).unwrap(), r#"{"env":{}}"#);
    }

    #[test]
    fn codex_targeted_set_and_unset_preserve_comments_and_unknown_fields() {
        let harness = harness();
        let path = harness.home.join(".codex/config.toml");
        write(
            &path,
            "# user comment\nmodel = \"old\"\nunknown = \"keep\"\n\n[features]\nweb_search = false\n",
        );
        let manager = harness
            .service
            .manager(AgentSettingsProvider::Codex, Some(&harness.project));
        let original = manager.discover().unwrap();
        let revision = original.native_files[0].revision.clone().unwrap();
        let patch = SettingsPatch {
            provider: AgentSettingsProvider::Codex,
            project_path: Some(harness.project.display().to_string()),
            expected_file_revisions: BTreeMap::from([("user_config".into(), revision)]),
            operations: vec![
                SettingOperation::Replace {
                    key: SettingKey::new("common", "model"),
                    scope: SettingScope::User,
                    value: json!("gpt-5.6"),
                },
                SettingOperation::Clear {
                    key: SettingKey::new("codex", "web_search"),
                    scope: SettingScope::User,
                },
            ],
        };
        let diff = manager.diff(&patch).unwrap();
        assert!(diff.files[0].changed);
        let public_diff = serde_json::to_string(&diff).unwrap();
        assert!(!public_diff.contains("# user comment"));
        assert!(!public_diff.contains("gpt-5.6"));
        assert!(!public_diff.contains(&path.display().to_string()));
        let public_diff_value = serde_json::to_value(&diff).unwrap();
        assert!(public_diff_value["files"][0].get("path").is_none());
        assert_eq!(public_diff_value["files"][0]["file_id"], "user_config");
        let result = manager.apply(&patch).unwrap();
        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("# user comment"));
        assert!(content.contains("unknown = \"keep\""));
        assert!(!content.contains("web_search"));
        let model = result
            .effective_settings
            .iter()
            .find(|setting| setting.key.id() == "common.model")
            .unwrap();
        assert_eq!(model.effective_value, Some(json!("gpt-5.6")));
    }

    #[test]
    fn stale_revision_rejects_without_overwriting_external_change() {
        let harness = harness();
        let path = harness.home.join(".claude/settings.json");
        write(&path, r#"{"model":"old","unknown":"keep"}"#);
        let manager = harness
            .service
            .manager(AgentSettingsProvider::ClaudeCode, Some(&harness.project));
        let revision = manager.discover().unwrap().native_files[0]
            .revision
            .clone()
            .unwrap();
        write(&path, r#"{"model":"external","unknown":"keep"}"#);
        let patch = SettingsPatch {
            provider: AgentSettingsProvider::ClaudeCode,
            project_path: Some(harness.project.display().to_string()),
            expected_file_revisions: BTreeMap::from([("user_settings".into(), revision)]),
            operations: vec![SettingOperation::Replace {
                key: SettingKey::new("common", "model"),
                scope: SettingScope::User,
                value: json!("requested"),
            }],
        };
        assert!(matches!(
            manager.apply(&patch),
            Err(AgentSettingError::StaleRevision)
        ));
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            r#"{"model":"external","unknown":"keep"}"#
        );
    }

    #[test]
    fn json_and_yaml_writes_preserve_unknown_native_nodes() {
        let harness = harness();
        let gemini = harness.home.join(".gemini/settings.json");
        let omp = harness.home.join(".omp/agent/config.yml");
        write(&gemini, r#"{"model":{"name":"old"},"unknown":{"x":1}}"#);
        write(&omp, "theme:\n  dark: old\nunknown:\n  x: 1\n");
        for (provider, file_id, key, value) in [
            (
                AgentSettingsProvider::Gemini,
                "user_settings",
                SettingKey::new("common", "model"),
                json!("new-model"),
            ),
            (
                AgentSettingsProvider::OhMyPi,
                "user_config",
                SettingKey::new("oh_my_pi", "dark_theme"),
                json!("new-theme"),
            ),
        ] {
            let manager = harness.service.manager(provider, Some(&harness.project));
            let snapshot = manager.discover().unwrap();
            let revision = snapshot
                .native_files
                .iter()
                .find(|file| file.file_id == file_id)
                .unwrap()
                .revision
                .clone()
                .unwrap();
            manager
                .apply(&SettingsPatch {
                    provider,
                    project_path: Some(harness.project.display().to_string()),
                    expected_file_revisions: BTreeMap::from([(file_id.into(), revision)]),
                    operations: vec![SettingOperation::Replace {
                        key,
                        scope: SettingScope::User,
                        value,
                    }],
                })
                .unwrap();
        }
        assert_eq!(
            serde_json::from_str::<Value>(&fs::read_to_string(gemini).unwrap()).unwrap()["unknown"]
                ["x"],
            json!(1)
        );
        assert_eq!(
            parse_document(NativeConfigFormat::Yaml, &fs::read_to_string(omp).unwrap()).unwrap()["unknown"]
                ["x"],
            json!(1)
        );
    }

    #[test]
    fn profile_crud_copy_and_apply_preview_use_common_slots_only() {
        let harness = harness();
        let codex = harness.home.join(".codex/config.toml");
        write(&codex, "model = \"old\"\n");
        let profile = ConfigProfile {
            id: Uuid::new_v4(),
            provider: AgentSettingsProvider::Codex,
            executor_profile: ExecutorProfileId::new(BaseCodingAgent::Codex),
            name: "Review".to_string(),
            schema_version: SETTINGS_PROFILE_STORE_VERSION,
            setting_overrides: BTreeMap::from([
                ("common.model".to_string(), json!("gpt-5.6")),
                ("codex.web_search".to_string(), json!(true)),
            ]),
            provider_extensions: BTreeMap::from([("codex.private".into(), json!(1))]),
            environment: BTreeMap::from([("EXACT".into(), "a=b c".into())]),
            custom_args: vec!["--flag=value".into()],
            updated_at: Utc::now(),
        };
        let saved = harness
            .service
            .save_profile(SaveConfigProfileRequest {
                profile: profile.clone(),
            })
            .unwrap();
        assert!(saved.configured_credential_keys.is_empty());
        assert_eq!(saved.setting_overrides["common.model"], json!("gpt-5.6"));
        assert_eq!(saved.setting_overrides["codex.web_search"], json!(true));
        assert_eq!(
            saved.provider_extension_keys,
            vec!["codex.private".to_string()]
        );
        // Keep the browser boundary safe even if an older/future store
        // contains a credential override that current validation disallows.
        let mut store = harness.service.load_profile_store().unwrap();
        store.profiles[0].setting_overrides.insert(
            "common.api_key".to_string(),
            json!("credential-must-not-leave-service"),
        );
        store.profiles[0].setting_overrides.insert(
            "future.auth_token".to_string(),
            json!("unknown-credential-must-not-leave-service"),
        );
        harness.service.save_profile_store(&store).unwrap();
        let listed = harness
            .service
            .list_profiles(Some(AgentSettingsProvider::Codex))
            .unwrap();
        assert!(
            !serde_json::to_string(&listed)
                .unwrap()
                .contains("credential-must-not-leave-service")
        );
        assert!(
            !serde_json::to_string(&listed)
                .unwrap()
                .contains("unknown-credential-must-not-leave-service")
        );
        assert!(
            !listed[0]
                .setting_overrides
                .contains_key("future.auth_token")
        );
        assert_eq!(
            listed[0].configured_credential_keys,
            vec!["common.api_key".to_string()]
        );
        assert_eq!(
            listed[0].setting_overrides["common.model"],
            json!("gpt-5.6")
        );
        assert_eq!(listed[0].setting_overrides["codex.web_search"], json!(true));

        let updated = harness
            .service
            .update_profile(UpdateConfigProfileRequest {
                id: saved.id,
                name: "Review updated".into(),
                environment: BTreeMap::from([("EXACT".into(), "changed".into())]),
                custom_args: vec!["--updated".into()],
            })
            .unwrap();
        assert_eq!(updated.name, "Review updated");
        assert_eq!(updated.setting_overrides["common.model"], json!("gpt-5.6"));
        let stored = harness.service.load_profile_store().unwrap().profiles;
        let stored = stored
            .iter()
            .find(|profile| profile.id == saved.id)
            .unwrap();
        assert_eq!(
            stored.setting_overrides["common.api_key"],
            json!("credential-must-not-leave-service")
        );
        assert_eq!(
            stored.setting_overrides["future.auth_token"],
            json!("unknown-credential-must-not-leave-service")
        );
        assert_eq!(stored.provider_extensions["codex.private"], json!(1));
        // Restore a currently valid profile for the apply portion of this
        // test; the assertions above already proved that ordinary PUT kept the
        // unknown future field losslessly while the browser view hid it.
        let mut store = harness.service.load_profile_store().unwrap();
        store.profiles[0]
            .setting_overrides
            .remove("future.auth_token");
        harness.service.save_profile_store(&store).unwrap();

        let copy = harness
            .service
            .copy_profile_preview(CopyProfilePreviewRequest {
                id: saved.id,
                target_provider: AgentSettingsProvider::ClaudeCode,
                target_executor_profile: ExecutorProfileId::new(BaseCodingAgent::ClaudeCode),
                target_name: "Copied".into(),
                target_profile_id: None,
            })
            .unwrap();
        assert_eq!(
            copy.added_keys,
            vec!["common.model", "profile.custom_args", "profile.environment"]
        );
        assert!(copy.overwritten_keys.is_empty());
        assert_eq!(
            copy.skipped_keys,
            vec!["codex.private", "codex.web_search", "common.api_key"]
        );
        assert!(copy.profile.provider_extension_keys.is_empty());
        assert!(copy.profile.configured_credential_keys.is_empty());
        assert_eq!(copy.profile.environment["EXACT"], "changed");

        let expected_file_revisions = harness
            .service
            .manager(AgentSettingsProvider::Codex, Some(&harness.project))
            .discover()
            .unwrap()
            .native_files
            .into_iter()
            .filter_map(|file| file.revision.map(|revision| (file.file_id, revision)))
            .collect();
        let apply_preview = ProfileApplyPreviewRequest {
            id: saved.id,
            project_path: Some(harness.project.display().to_string()),
            scope: SettingScope::User,
            expected_file_revisions,
        };
        let preview = harness
            .service
            .preview_profile_apply(&apply_preview)
            .unwrap();
        assert!(preview.files[0].changed);
        assert!(
            preview
                .warnings
                .iter()
                .any(|warning| warning.contains("Custom arguments"))
        );
        assert!(!serde_json::to_string(&preview).unwrap().contains("gpt-5.6"));

        harness
            .service
            .apply_profile(ApplyConfigProfileRequest {
                preview: apply_preview,
                confirmed: true,
            })
            .unwrap();
        let applied = fs::read_to_string(&codex).unwrap();
        assert!(applied.contains("model = \"gpt-5.6\""));
        assert!(applied.contains("EXACT = \"changed\""));

        harness
            .service
            .delete_profile(DeleteConfigProfileRequest { id: saved.id })
            .unwrap();
        assert!(harness.service.list_profiles(None).unwrap().is_empty());
    }

    #[test]
    fn existing_target_profile_copy_is_lossless_revision_checked_and_adapter_owned() {
        let harness = harness();
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let source_updated_at = Utc::now();
        let target_updated_at = source_updated_at + chrono::Duration::seconds(1);
        let mut store = ConfigProfileStore {
            version: SETTINGS_PROFILE_STORE_VERSION,
            profiles: vec![
                ConfigProfile {
                    id: source_id,
                    provider: AgentSettingsProvider::Codex,
                    executor_profile: ExecutorProfileId::new(BaseCodingAgent::Codex),
                    name: "Source".into(),
                    schema_version: SETTINGS_PROFILE_STORE_VERSION,
                    setting_overrides: BTreeMap::from([
                        ("common.model".into(), json!("gpt-5.6")),
                        ("common.api_key".into(), json!("source-secret")),
                        ("codex.web_search".into(), json!(true)),
                    ]),
                    provider_extensions: BTreeMap::from([(
                        "codex.source_extension".into(),
                        json!("source-private"),
                    )]),
                    environment: BTreeMap::from([("SOURCE".into(), "one".into())]),
                    custom_args: vec!["--source".into()],
                    updated_at: source_updated_at,
                },
                ConfigProfile {
                    id: target_id,
                    provider: AgentSettingsProvider::ClaudeCode,
                    executor_profile: ExecutorProfileId::new(BaseCodingAgent::ClaudeCode),
                    name: "Target".into(),
                    schema_version: SETTINGS_PROFILE_STORE_VERSION,
                    setting_overrides: BTreeMap::from([
                        ("common.model".into(), json!("claude-old")),
                        ("common.api_address".into(), json!("https://example.com")),
                        ("common.api_key".into(), json!("target-secret")),
                        ("future.unknown".into(), json!("target-opaque")),
                    ]),
                    provider_extensions: BTreeMap::from([(
                        "claude.target_extension".into(),
                        json!("target-private"),
                    )]),
                    environment: BTreeMap::from([("TARGET".into(), "old".into())]),
                    custom_args: vec!["--target".into()],
                    updated_at: target_updated_at,
                },
            ],
        };
        harness.service.save_profile_store(&store).unwrap();

        let same_provider = harness
            .service
            .copy_profile_preview(CopyProfilePreviewRequest {
                id: source_id,
                target_provider: AgentSettingsProvider::Codex,
                target_executor_profile: ExecutorProfileId::new(BaseCodingAgent::Codex),
                target_name: "Same provider".into(),
                target_profile_id: None,
            });
        assert!(matches!(
            same_provider,
            Err(AgentSettingError::ValidationFailed(_))
        ));

        let mismatched_target = harness
            .service
            .copy_profile_preview(CopyProfilePreviewRequest {
                id: source_id,
                target_provider: AgentSettingsProvider::Gemini,
                target_executor_profile: ExecutorProfileId::new(BaseCodingAgent::Gemini),
                target_name: "Wrong target".into(),
                target_profile_id: Some(target_id),
            });
        assert!(matches!(
            mismatched_target,
            Err(AgentSettingError::ValidationFailed(_))
        ));

        let preview_request = CopyProfilePreviewRequest {
            id: source_id,
            target_provider: AgentSettingsProvider::ClaudeCode,
            target_executor_profile: ExecutorProfileId::new(BaseCodingAgent::ClaudeCode),
            target_name: "ignored for an existing target".into(),
            target_profile_id: Some(target_id),
        };

        store.profiles[0]
            .setting_overrides
            .insert("common.model".into(), json!(42));
        store.profiles[0].updated_at += chrono::Duration::seconds(2);
        harness.service.save_profile_store(&store).unwrap();
        let invalid_value_preview = harness
            .service
            .copy_profile_preview(preview_request.clone())
            .unwrap();
        assert!(
            invalid_value_preview
                .skipped_keys
                .contains(&"common.model".to_string())
        );
        assert_eq!(
            invalid_value_preview.profile.setting_overrides["common.model"],
            json!("claude-old")
        );
        assert!(
            invalid_value_preview
                .warnings
                .iter()
                .any(|warning| warning.contains("target Adapter rejected"))
        );

        store.profiles[0]
            .setting_overrides
            .insert("common.model".into(), json!("gpt-5.6"));
        store.profiles[0].updated_at += chrono::Duration::seconds(1);
        harness.service.save_profile_store(&store).unwrap();
        let preview = harness
            .service
            .copy_profile_preview(preview_request.clone())
            .unwrap();
        assert!(preview.added_keys.is_empty());
        assert_eq!(
            preview.overwritten_keys,
            vec!["common.model", "profile.custom_args", "profile.environment"]
        );
        assert_eq!(
            preview.skipped_keys,
            vec![
                "codex.source_extension",
                "codex.web_search",
                "common.api_key"
            ]
        );
        assert_eq!(preview.profile.name, "Target");
        assert_eq!(
            preview.profile.setting_overrides["common.api_address"],
            json!("https://example.com")
        );
        let serialized = serde_json::to_string(&preview).unwrap();
        assert!(!serialized.contains("source-secret"));
        assert!(!serialized.contains("target-secret"));
        assert!(!serialized.contains("target-opaque"));
        assert!(!serialized.contains("source-private"));
        assert!(!serialized.contains("target-private"));

        let stale_source = harness.service.copy_profile(CopyConfigProfileRequest {
            preview: preview_request.clone(),
            expected_source_updated_at: preview.source_updated_at - chrono::Duration::seconds(1),
            expected_target_updated_at: preview.target_updated_at,
            confirmed: true,
        });
        assert!(matches!(
            stale_source,
            Err(AgentSettingError::StaleRevision)
        ));

        let stale = harness.service.copy_profile(CopyConfigProfileRequest {
            preview: preview_request.clone(),
            expected_source_updated_at: preview.source_updated_at,
            expected_target_updated_at: Some(
                preview.target_updated_at.unwrap() - chrono::Duration::seconds(1),
            ),
            confirmed: true,
        });
        assert!(matches!(stale, Err(AgentSettingError::StaleRevision)));

        let copied = harness
            .service
            .copy_profile(CopyConfigProfileRequest {
                preview: preview_request.clone(),
                expected_source_updated_at: preview.source_updated_at,
                expected_target_updated_at: preview.target_updated_at,
                confirmed: true,
            })
            .unwrap();
        assert_eq!(copied.id, target_id);
        assert_eq!(copied.setting_overrides["common.model"], json!("gpt-5.6"));
        assert_eq!(copied.environment["SOURCE"], "one");
        assert_eq!(copied.custom_args, vec!["--source"]);

        store = harness.service.load_profile_store().unwrap();
        let stored = store
            .profiles
            .iter()
            .find(|profile| profile.id == target_id)
            .unwrap();
        assert_eq!(
            stored.setting_overrides["common.api_key"],
            json!("target-secret")
        );
        assert_eq!(
            stored.setting_overrides["future.unknown"],
            json!("target-opaque")
        );
        assert_eq!(
            stored.provider_extensions["claude.target_extension"],
            json!("target-private")
        );
        assert!(
            !stored
                .provider_extensions
                .contains_key("codex.source_extension")
        );

        let source = store
            .profiles
            .iter_mut()
            .find(|profile| profile.id == source_id)
            .unwrap();
        source.environment.clear();
        source.custom_args.clear();
        source.updated_at = Utc::now() + chrono::Duration::seconds(2);
        harness.service.save_profile_store(&store).unwrap();

        let clear_preview = harness
            .service
            .copy_profile_preview(preview_request.clone())
            .unwrap();
        assert_eq!(
            clear_preview.overwritten_keys,
            vec!["profile.custom_args", "profile.environment"]
        );
        harness
            .service
            .copy_profile(CopyConfigProfileRequest {
                preview: preview_request,
                expected_source_updated_at: clear_preview.source_updated_at,
                expected_target_updated_at: clear_preview.target_updated_at,
                confirmed: true,
            })
            .unwrap();
        let cleared = harness
            .service
            .load_profile_store()
            .unwrap()
            .profiles
            .into_iter()
            .find(|profile| profile.id == target_id)
            .unwrap();
        assert!(cleared.environment.is_empty());
        assert!(cleared.custom_args.is_empty());
    }

    #[test]
    fn raw_edit_requires_current_revision_and_valid_syntax() {
        let harness = harness();
        let path = harness.home.join(".gemini/settings.json");
        write(&path, r#"{"telemetry":{"enabled":false}}"#);
        let manager = harness
            .service
            .manager(AgentSettingsProvider::Gemini, Some(&harness.project));
        let revision = manager.discover().unwrap().native_files[0]
            .revision
            .clone()
            .unwrap();
        let invalid = NativeFilePatch {
            provider: AgentSettingsProvider::Gemini,
            project_path: Some(harness.project.display().to_string()),
            file_id: "user_settings".into(),
            expected_revision: revision.clone(),
            content: "{".into(),
        };
        assert!(matches!(
            manager.diff_native_file(&invalid),
            Err(AgentSettingError::InvalidConfiguration(_))
        ));
        let valid = NativeFilePatch {
            content: r#"{"telemetry":{"enabled":true},"nativeOnly":1}"#.into(),
            ..invalid
        };
        manager.apply_native_file(&valid).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&fs::read_to_string(path).unwrap()).unwrap()["nativeOnly"],
            json!(1)
        );
    }

    #[test]
    fn rollback_restores_exact_bytes_and_removes_new_files() {
        let harness = harness();
        let existing = harness.home.join("existing.json");
        let created = harness.home.join("created.json");
        write(&existing, "{\r\n  \"before\": true\r\n}\r\n");
        write(&created, "new");
        let files = vec![
            RenderedFile {
                spec: file_spec(
                    "existing",
                    existing.clone(),
                    NativeConfigFormat::Json,
                    SettingScope::User,
                ),
                before: Some(b"{\r\n  \"before\": true\r\n}\r\n".to_vec()),
                after: b"changed".to_vec(),
            },
            RenderedFile {
                spec: file_spec(
                    "created",
                    created.clone(),
                    NativeConfigFormat::Json,
                    SettingScope::User,
                ),
                before: None,
                after: b"new".to_vec(),
            },
        ];
        rollback_files(&files).unwrap();
        assert_eq!(
            fs::read(existing).unwrap(),
            b"{\r\n  \"before\": true\r\n}\r\n"
        );
        assert!(!created.exists());
    }
}
