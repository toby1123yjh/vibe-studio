//! Provider-neutral entry points for the direct V1 runtime adapters.
//!
//! Legacy product paths may still use `StandardCodingAgentExecutor`, while the
//! V1 runtime launches the same concrete providers through the narrow API in
//! this module. Native bytes are decoded once here, then mapped to canonical
//! events. Consumers must not parse provider output themselves.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    actions::SelectedSkill,
    approvals::ExecutorApprovalService,
    env::ExecutionEnv,
    executors::{CodingAgent, ExecutorError, SpawnedChild},
    profile::{ExecutorConfig, ExecutorConfigs, runtime_profile_ids_match},
    runtime::{
        AGENT_EVENT_PAYLOAD_VERSION, AGENT_EVENT_SCHEMA_VERSION, AgentEvent, AgentEventEnvelope,
        AgentEventPayload, AgentRunStatus, AgentRuntimeError, AgentRuntimeErrorKind,
        AgentRuntimeMessageRole, AgentRuntimeToolStatus, AgentTransportKind, CapabilitySnapshot,
        CapabilitySnapshotEntry, CapabilitySource, CapabilityState, NativeAuditError,
        NativeAuditFrame, NativeAuditManifest, NativeAuditReplayMapper, NativeAuditVersionSet,
        ProviderEvent, ProviderSessionReference,
    },
};

mod codex_items;
pub(crate) mod conversation;
mod file_evidence;
pub use conversation::StatefulProviderMapper;

/// Everything needed to launch one real provider process without constructing
/// a legacy `ExecutorAction` or routing through `StandardCodingAgentExecutor`.
pub struct DirectProviderLaunchRequest<'a> {
    pub provider: DirectProvider,
    pub executor_config: &'a ExecutorConfig,
    pub intent: DirectIntent,
    pub prompt: &'a str,
    pub provider_session: Option<&'a ProviderSessionReference>,
    pub reset_to_message_id: Option<&'a str>,
    pub selected_skills: &'a [SelectedSkill],
    pub approvals: Arc<dyn ExecutorApprovalService>,
    pub current_dir: &'a Path,
    pub env: &'a ExecutionEnv,
}

/// Resolve the requested profile and launch the provider through its concrete
/// adapter. Profile overrides, approvals, reviews, and Codex selected skills
/// retain the same behavior as the existing product launch path.
pub async fn launch_direct_provider(
    request: DirectProviderLaunchRequest<'_>,
) -> Result<SpawnedChild, ExecutorError> {
    validate_direct_launch(&request)?;

    let profile_id = request.executor_config.profile_id();
    let mut agent = ExecutorConfigs::get_cached()
        .get_coding_agent(&profile_id)
        .ok_or_else(|| ExecutorError::UnknownExecutorType(profile_id.to_string()))?;
    if DirectProvider::from_agent(&agent) != Some(request.provider) {
        return Err(profile_provider_mismatch(
            request.provider,
            request.executor_config,
        ));
    }

    apply_attempt_model_selection(&mut agent, request.executor_config)?;

    match &mut agent {
        CodingAgent::Gemini(agent) => {
            agent.use_direct_approvals(request.approvals);
            agent
                .launch_direct(
                    request.intent,
                    request.current_dir,
                    request.prompt,
                    provider_session_id(request.provider_session),
                    request.env,
                )
                .await
        }
        CodingAgent::Codex(agent) => {
            agent.use_direct_approvals(request.approvals);
            agent
                .launch_direct(
                    request.intent,
                    request.current_dir,
                    request.prompt,
                    provider_session_id(request.provider_session),
                    request.selected_skills,
                    request.env,
                )
                .await
        }
        CodingAgent::ClaudeCode(agent) => {
            agent.use_direct_approvals(request.approvals);
            agent
                .launch_direct(
                    request.intent,
                    request.current_dir,
                    request.prompt,
                    provider_session_id(request.provider_session),
                    request.reset_to_message_id,
                    request.env,
                )
                .await
        }
        CodingAgent::OhMyPi(agent) => {
            agent
                .launch_direct(
                    request.intent,
                    request.current_dir,
                    request.prompt,
                    provider_session_id(request.provider_session),
                    request.env,
                )
                .await
        }
        CodingAgent::Opencode(agent) => {
            agent.use_direct_approvals(request.approvals);
            agent
                .launch_direct(
                    request.intent,
                    request.current_dir,
                    request.prompt,
                    provider_session_id(request.provider_session),
                    request.env,
                )
                .await
        }
        CodingAgent::DeepseekHarness(agent) => {
            agent.use_direct_approvals(request.approvals);
            agent
                .launch_direct(
                    request.intent,
                    request.current_dir,
                    request.prompt,
                    provider_session_id(request.provider_session),
                    request.env,
                )
                .await
        }
        #[cfg(feature = "qa-mode")]
        CodingAgent::QaMock(_) => Err(profile_provider_mismatch(
            request.provider,
            request.executor_config,
        )),
    }
}

fn apply_attempt_model_selection(
    agent: &mut CodingAgent,
    config: &ExecutorConfig,
) -> Result<(), ExecutorError> {
    // Preserve omitted fields for profile-only workflow requests. Explicit null
    // clears that preset field and delegates to the native CLI/session.
    match agent {
        CodingAgent::Gemini(agent) => agent.apply_direct_overrides(config),
        CodingAgent::Codex(agent) => agent.apply_direct_overrides(config),
        CodingAgent::ClaudeCode(agent) => agent.apply_direct_overrides(config),
        CodingAgent::OhMyPi(agent) => agent.apply_direct_overrides(config),
        CodingAgent::Opencode(agent) => agent.apply_direct_overrides(config),
        CodingAgent::DeepseekHarness(agent) => agent.apply_direct_overrides(config),
        #[cfg(feature = "qa-mode")]
        CodingAgent::QaMock(_) => Ok(()),
    }
}

fn validate_direct_launch(request: &DirectProviderLaunchRequest<'_>) -> Result<(), ExecutorError> {
    request
        .executor_config
        .validate_provider_options()
        .map_err(ExecutorError::InvalidConfiguration)?;
    let configured_provider = DirectProvider::from_base_agent(request.executor_config.executor)
        .ok_or_else(|| profile_provider_mismatch(request.provider, request.executor_config))?;
    if configured_provider != request.provider {
        return Err(profile_provider_mismatch(
            request.provider,
            request.executor_config,
        ));
    }

    let profile_id = request.executor_config.profile_id().cache_key();
    if let Some(session) = request.provider_session {
        session.validate_current().map_err(|error| {
            ExecutorError::FollowUpNotSupported(format!(
                "invalid provider session for {}: {error}",
                request.provider.id()
            ))
        })?;
        if session.provider_id != request.provider.id()
            || !runtime_profile_ids_match(&session.runtime_profile_id, &profile_id)
        {
            return Err(ExecutorError::FollowUpNotSupported(format!(
                "provider session does not match {} profile {profile_id}",
                request.provider.id()
            )));
        }
        validate_native_session_id(request.provider, &session.provider_session_id).map_err(
            |error| {
                ExecutorError::FollowUpNotSupported(format!(
                    "invalid provider session for {}: {error}",
                    request.provider.id()
                ))
            },
        )?;
    }

    match request.intent {
        DirectIntent::Initial if request.provider_session.is_some() => {
            return Err(ExecutorError::FollowUpNotSupported(
                "initial launch cannot attach a provider session".to_string(),
            ));
        }
        DirectIntent::FollowUp | DirectIntent::Resume if request.provider_session.is_none() => {
            return Err(ExecutorError::FollowUpNotSupported(format!(
                "{} requires an explicit provider session",
                request.provider.id()
            )));
        }
        _ => {}
    }

    if request.reset_to_message_id.is_some()
        && (request.provider != DirectProvider::ClaudeCode
            || !matches!(
                request.intent,
                DirectIntent::FollowUp | DirectIntent::Resume
            ))
    {
        return Err(ExecutorError::ResetToMessageNotSupported(format!(
            "{} does not support message-level reset for {:?}",
            request.provider.id(),
            request.intent
        )));
    }

    Ok(())
}

fn provider_session_id(session: Option<&ProviderSessionReference>) -> Option<&str> {
    session.map(|session| session.provider_session_id.as_str())
}

fn profile_provider_mismatch(
    provider: DirectProvider,
    executor_config: &ExecutorConfig,
) -> ExecutorError {
    ExecutorError::UnknownExecutorType(format!(
        "direct provider {} does not match executor profile {}",
        provider.id(),
        executor_config.profile_id()
    ))
}

/// Validate an opaque provider-native session identifier before it is persisted
/// or handed to a provider process. Session IDs are intentionally not parsed
/// as UUIDs (providers choose their own format), but control characters and
/// option-like values would be ambiguous on the wire or alter CLI parsing.
/// Provider adapters remain authoritative for the actual existence check.
pub fn validate_native_session_id(
    provider: DirectProvider,
    session_id: &str,
) -> Result<(), String> {
    let trimmed = session_id.trim();
    if trimmed != session_id {
        return Err("native session id cannot have leading or trailing whitespace".to_string());
    }
    let session_id = trimmed;
    if session_id.is_empty() {
        return Err("native session id cannot be blank".to_string());
    }
    if session_id.chars().any(char::is_control) {
        return Err("native session id contains control characters".to_string());
    }
    if matches!(provider, DirectProvider::Gemini | DirectProvider::OhMyPi)
        && session_id.starts_with('-')
    {
        return Err(format!(
            "{} native session id cannot start with '-'",
            provider.id()
        ));
    }
    Ok(())
}

/// The only provider products included in the V1 adapter gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectProvider {
    Gemini,
    Codex,
    ClaudeCode,
    OhMyPi,
    Opencode,
    DeepseekHarness,
}

impl DirectProvider {
    pub const ALL: [Self; 6] = [
        Self::Gemini,
        Self::Codex,
        Self::ClaudeCode,
        Self::OhMyPi,
        Self::Opencode,
        Self::DeepseekHarness,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Gemini => "gemini",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude_code",
            Self::OhMyPi => "oh_my_pi",
            Self::Opencode => "opencode",
            Self::DeepseekHarness => "deepseek_harness",
        }
    }

    /// Construct this adapter's native Tool Manager. Provider path and
    /// encoding decisions remain owned by the direct adapter boundary.
    pub fn tool_manager(
        self,
        home_dir: PathBuf,
        project_path: Option<PathBuf>,
        disabled_root: PathBuf,
    ) -> crate::agent_tools::ProviderToolManager {
        crate::agent_tools::ProviderToolManager::new(
            self.into(),
            home_dir,
            project_path,
            disabled_root,
        )
    }

    /// Construct this adapter's native Settings Manager. Provider paths,
    /// schema descriptors, precedence, and native encodings stay behind the
    /// direct-provider boundary.
    pub fn settings_manager(
        self,
        home_dir: PathBuf,
        project_path: Option<PathBuf>,
    ) -> crate::agent_settings::ProviderSettingsManager {
        crate::agent_settings::ProviderSettingsManager::new(self.into(), home_dir, project_path)
    }

    pub const fn from_base_agent(agent: crate::executors::BaseCodingAgent) -> Option<Self> {
        match agent {
            crate::executors::BaseCodingAgent::Gemini => Some(Self::Gemini),
            crate::executors::BaseCodingAgent::Codex => Some(Self::Codex),
            crate::executors::BaseCodingAgent::ClaudeCode => Some(Self::ClaudeCode),
            crate::executors::BaseCodingAgent::OhMyPi => Some(Self::OhMyPi),
            crate::executors::BaseCodingAgent::Opencode => Some(Self::Opencode),
            crate::executors::BaseCodingAgent::DeepseekHarness => Some(Self::DeepseekHarness),
            #[cfg(feature = "qa-mode")]
            crate::executors::BaseCodingAgent::QaMock => None,
        }
    }

    pub fn from_agent(agent: &CodingAgent) -> Option<Self> {
        Self::from_base_agent(crate::executors::BaseCodingAgent::from(agent))
    }

    pub const fn transport(self) -> AgentTransportKind {
        match self {
            Self::Gemini | Self::Opencode | Self::DeepseekHarness => AgentTransportKind::Acp,
            Self::Codex => AgentTransportKind::AppServerJsonrpc,
            Self::ClaudeCode => AgentTransportKind::StdioRpc,
            Self::OhMyPi => AgentTransportKind::StdioRpc,
        }
    }

    pub const fn versions(self) -> DirectAdapterVersions {
        match self {
            Self::Gemini => DirectAdapterVersions {
                executable: "gemini",
                runtime: None,
                protocol: Some("acp-0.8"),
                adapter: "gemini-adapter-v1",
                mapper: "gemini-conversation-v2",
            },
            Self::Codex => DirectAdapterVersions {
                executable: "codex",
                // The runtime version is reported by app-server initialize;
                // static adapter metadata must not claim a required release.
                runtime: None,
                protocol: Some("rust-v0.144.1"),
                adapter: "codex-adapter-v1",
                mapper: "codex-conversation-v2",
            },
            Self::ClaudeCode => DirectAdapterVersions {
                executable: "claude",
                runtime: None,
                protocol: Some("stream-json-v1"),
                adapter: "claude-code-adapter-v1",
                mapper: "claude-code-conversation-v2",
            },
            Self::OhMyPi => DirectAdapterVersions {
                executable: "omp",
                runtime: None,
                protocol: Some("stdio-rpc-ndjson-v1"),
                adapter: "oh-my-pi-adapter-v1",
                mapper: "oh-my-pi-conversation-v2",
            },
            Self::Opencode => DirectAdapterVersions {
                executable: "opencode",
                runtime: None,
                protocol: Some("acp-v1"),
                adapter: "opencode-adapter-v1",
                mapper: "opencode-conversation-v2",
            },
            Self::DeepseekHarness => DirectAdapterVersions {
                executable: "dsh",
                runtime: None,
                protocol: Some("acp-v1"),
                adapter: "deepseek-harness-adapter-v1",
                mapper: "deepseek-harness-conversation-v2",
            },
        }
    }

    pub fn version_set(self) -> NativeAuditVersionSet {
        let versions = self.versions();
        NativeAuditVersionSet {
            audit_schema_version: crate::runtime::NATIVE_AUDIT_SCHEMA_VERSION,
            runtime_version: versions.runtime.map(str::to_owned),
            adapter_version: versions.adapter.to_owned(),
            protocol_version: versions.protocol.map(str::to_owned),
            mapper_version: versions.mapper.to_owned(),
        }
    }

    pub fn capabilities(self, runtime_profile_id: impl Into<String>) -> CapabilitySnapshot {
        let versions = self.versions();
        let states = match self {
            Self::Opencode | Self::DeepseekHarness => capability_states(&[
                (
                    crate::runtime::AgentCapability::SessionResume,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Steering,
                    CapabilityState::Unsupported,
                ),
                (
                    crate::runtime::AgentCapability::Approval,
                    CapabilityState::Native,
                ),
                // The current prompt bridge only sends text. Do not claim
                // image support until an actual content mapping is present.
                (
                    crate::runtime::AgentCapability::Images,
                    CapabilityState::Unsupported,
                ),
                (
                    crate::runtime::AgentCapability::Review,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Mcp,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Subagents,
                    CapabilityState::Unknown,
                ),
                (
                    crate::runtime::AgentCapability::TokenUsage,
                    CapabilityState::Unknown,
                ),
            ]),
            Self::Gemini => capability_states(&[
                (
                    crate::runtime::AgentCapability::SessionResume,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Steering,
                    CapabilityState::Unsupported,
                ),
                (
                    crate::runtime::AgentCapability::Approval,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Images,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Review,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Mcp,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Subagents,
                    CapabilityState::Unknown,
                ),
                (
                    crate::runtime::AgentCapability::TokenUsage,
                    CapabilityState::Native,
                ),
            ]),
            Self::Codex => capability_states(&[
                (
                    crate::runtime::AgentCapability::SessionResume,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Steering,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Approval,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Images,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Review,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Mcp,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Subagents,
                    CapabilityState::Unknown,
                ),
                (
                    crate::runtime::AgentCapability::TokenUsage,
                    CapabilityState::Native,
                ),
            ]),
            Self::ClaudeCode => capability_states(&[
                (
                    crate::runtime::AgentCapability::SessionResume,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Steering,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Approval,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Images,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Review,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Mcp,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Subagents,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::TokenUsage,
                    CapabilityState::Native,
                ),
            ]),
            Self::OhMyPi => capability_states(&[
                (
                    crate::runtime::AgentCapability::SessionResume,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Steering,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Approval,
                    CapabilityState::Unsupported,
                ),
                (
                    crate::runtime::AgentCapability::Images,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Review,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Mcp,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::Subagents,
                    CapabilityState::Native,
                ),
                (
                    crate::runtime::AgentCapability::TokenUsage,
                    CapabilityState::Native,
                ),
            ]),
        };
        CapabilitySnapshot {
            schema_version: crate::runtime::CAPABILITY_SNAPSHOT_SCHEMA_VERSION,
            runtime_profile_id: runtime_profile_id.into(),
            provider_id: self.id().to_string(),
            runtime_version: versions.runtime.map(str::to_owned),
            protocol_version: versions.protocol.map(str::to_owned),
            adapter_version: versions.adapter.to_string(),
            resolved_at: Utc::now(),
            capabilities: states,
        }
    }

    pub fn mapper(self) -> DirectProviderMapper {
        DirectProviderMapper::new(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectAdapterVersions {
    pub executable: &'static str,
    pub runtime: Option<&'static str>,
    pub protocol: Option<&'static str>,
    pub adapter: &'static str,
    pub mapper: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectIntent {
    Initial,
    FollowUp,
    Review,
    Resume,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DirectControl {
    Cancel,
    Approve {
        request_id: String,
        approved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Input {
        request_id: String,
        text: String,
    },
    ReplyInteraction {
        reply: crate::runtime::ConversationInteractionReply,
    },
    Steer {
        text: String,
    },
}

impl DirectControl {
    fn capability(&self) -> Option<crate::runtime::AgentCapability> {
        match self {
            Self::Cancel => None,
            Self::Approve { .. } => Some(crate::runtime::AgentCapability::Approval),
            Self::Input { .. } => None,
            Self::ReplyInteraction { .. } => None,
            Self::Steer { .. } => Some(crate::runtime::AgentCapability::Steering),
        }
    }
}

/// Encode one provider-native control frame after checking the frozen attempt
/// capability snapshot.  Unsupported/unknown capabilities never fall through
/// to another control shape.
pub fn encode_control(
    provider: DirectProvider,
    snapshot: &CapabilitySnapshot,
    control: DirectControl,
) -> Result<Vec<u8>, DirectControlError> {
    if let Some(capability) = control.capability() {
        require_capability(provider, snapshot, capability, false)?;
    }
    match provider {
        DirectProvider::Gemini => super::gemini::command_adapter::encode_control(control),
        DirectProvider::Codex => super::codex::command_adapter::encode_control(control),
        DirectProvider::ClaudeCode => super::claude::command_adapter::encode_control(control),
        DirectProvider::OhMyPi => super::oh_my_pi::command_adapter::encode_control(control),
        DirectProvider::Opencode => super::opencode::command_adapter::encode_control(control),
        DirectProvider::DeepseekHarness => {
            super::deepseek_harness::command_adapter::encode_control(control)
        }
    }
    .map_err(DirectControlError::Json)
}

#[derive(Debug, thiserror::Error)]
pub enum DirectControlError {
    #[error(transparent)]
    Capability(#[from] CapabilityGateError),
    #[error(transparent)]
    Json(serde_json::Error),
}

fn capability_states(
    values: &[(crate::runtime::AgentCapability, CapabilityState)],
) -> Vec<CapabilitySnapshotEntry> {
    values
        .iter()
        .map(|(capability, state)| CapabilitySnapshotEntry {
            capability: *capability,
            state: *state,
            source: CapabilitySource::RuntimeProfile,
            emulation_policy: None,
            evidence: Some(serde_json::json!({ "provider_profile": "v1" })),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityGateError {
    #[error("capability {capability:?} is unresolved for {provider}")]
    Unresolved {
        provider: String,
        capability: crate::runtime::AgentCapability,
    },
    #[error("capability {capability:?} is unsupported for {provider}")]
    Unsupported {
        provider: String,
        capability: crate::runtime::AgentCapability,
    },
    #[error("emulated capability {capability:?} requires explicit policy approval")]
    EmulationNotAllowed {
        capability: crate::runtime::AgentCapability,
    },
}

pub fn require_capability(
    provider: DirectProvider,
    snapshot: &CapabilitySnapshot,
    capability: crate::runtime::AgentCapability,
    allow_emulated: bool,
) -> Result<(), CapabilityGateError> {
    match snapshot.resolve(capability) {
        CapabilityState::Native => Ok(()),
        CapabilityState::Emulated if allow_emulated => Ok(()),
        CapabilityState::Emulated => Err(CapabilityGateError::EmulationNotAllowed { capability }),
        CapabilityState::Unsupported => Err(CapabilityGateError::Unsupported {
            provider: provider.id().to_string(),
            capability,
        }),
        CapabilityState::Unknown => Err(CapabilityGateError::Unresolved {
            provider: provider.id().to_string(),
            capability,
        }),
    }
}

/// Typed provider semantics produced by the one native-frame decode.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedProviderEvent {
    Lifecycle(AgentRunStatus),
    SessionObserved(String),
    Message {
        role: AgentRuntimeMessageRole,
        content: String,
        final_output: bool,
        /// Provider-native item/message id when the protocol exposes one.
        /// Keeping it lets streaming deltas and the completed item project
        /// into one canonical conversation entry.
        message_id: Option<String>,
    },
    Thinking(String),
    ToolCall {
        id: Option<String>,
        name: String,
        status: AgentRuntimeToolStatus,
        arguments: Option<Value>,
        result: Option<Value>,
    },
    ApprovalRequested {
        id: String,
        tool_call_id: Option<String>,
        tool_name: String,
    },
    ApprovalResolved {
        id: String,
        approved: bool,
        reason: Option<String>,
    },
    InputRequested {
        id: String,
        prompt: String,
    },
    InputResolved {
        id: String,
        answered: bool,
    },
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: Option<u64>,
    },
    Error(AgentRuntimeError),
    Unknown {
        event_type: String,
        payload: Value,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedProviderEvent {
    pub raw: ProviderEvent,
    pub typed: TypedProviderEvent,
}

impl DirectProvider {
    pub fn decode_native_frame(
        self,
        frame: &NativeAuditFrame,
    ) -> Result<DecodedProviderEvent, NativeAuditError> {
        let raw = crate::runtime::DefaultNativeAuditMapper::for_manifest(&fixture_manifest(self))
            .decode(frame)?;
        let typed = classify_payload(self, &raw)?;
        Ok(DecodedProviderEvent { raw, typed })
    }

    pub fn map_provider_event(
        self,
        event: &DecodedProviderEvent,
        manifest: &NativeAuditManifest,
    ) -> Result<Vec<AgentEvent>, NativeAuditError> {
        map_typed_event(self, event, manifest)
    }
}

/// Replay mapper used by `AuditBundle::replay`; all six adapters share the
/// canonical envelope construction while retaining provider-specific names.
#[derive(Debug)]
pub struct DirectProviderMapper {
    pub provider: DirectProvider,
    state: std::sync::Mutex<StatefulProviderMapper>,
}

impl DirectProviderMapper {
    pub fn new(provider: DirectProvider) -> Self {
        Self {
            provider,
            state: std::sync::Mutex::new(StatefulProviderMapper::new(provider)),
        }
    }
}

impl Clone for DirectProviderMapper {
    fn clone(&self) -> Self {
        Self::new(self.provider)
    }
}

impl NativeAuditReplayMapper for DirectProviderMapper {
    fn versions(&self) -> NativeAuditVersionSet {
        self.provider.version_set()
    }

    fn decode(&self, frame: &NativeAuditFrame) -> Result<ProviderEvent, NativeAuditError> {
        crate::runtime::DefaultNativeAuditMapper::new(self.versions()).decode(frame)
    }

    fn begin_replay(&self) {
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            StatefulProviderMapper::new(self.provider);
    }

    fn validate_manifest(&self, manifest: &NativeAuditManifest) -> Result<(), NativeAuditError> {
        let mut expected = self.versions();
        // Explicit supported historical mapper identities, not a generic
        // accept-any-version bypass. Raw facts remain immutable.
        let historical = match self.provider {
            DirectProvider::Codex => "codex-mapper-v2",
            DirectProvider::Gemini => "gemini-mapper-v2",
            DirectProvider::ClaudeCode => "claude-code-mapper-v1",
            DirectProvider::OhMyPi => "oh-my-pi-mapper-v1",
            DirectProvider::Opencode => "opencode-mapper-v1",
            DirectProvider::DeepseekHarness => "deepseek-harness-mapper-v1",
        };
        if manifest.provider_id == self.provider.id() && manifest.mapper_version == historical {
            expected.mapper_version = historical.into();
        }
        crate::runtime::DefaultNativeAuditMapper::new(expected).validate_manifest(manifest)
    }

    fn map(
        &self,
        event: &ProviderEvent,
        manifest: &NativeAuditManifest,
    ) -> Result<Vec<AgentEvent>, NativeAuditError> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .map_event(event, manifest)
    }
}

fn fixture_manifest(provider: DirectProvider) -> NativeAuditManifest {
    // Only used to invoke the existing lossless frame decoder.  The returned
    // ProviderEvent's native reference is replaced with the real manifest by
    // AuditBundle::replay.
    let now = Utc::now();
    NativeAuditManifest {
        audit_schema_version: crate::runtime::NATIVE_AUDIT_SCHEMA_VERSION,
        session_id: Uuid::nil(),
        agent_run_id: Uuid::nil(),
        turn_id: Uuid::nil(),
        run_attempt_id: Uuid::nil(),
        run_attempt_number: 1,
        provider_id: provider.id().to_string(),
        runtime_profile_id: "fixture".to_string(),
        workspace_path: String::new(),
        runtime_version: provider.versions().runtime.map(str::to_owned),
        protocol_version: provider.versions().protocol.map(str::to_owned),
        adapter_version: provider.versions().adapter.to_string(),
        mapper_version: provider.versions().mapper.to_string(),
        frame_count: 0,
        first_sequence: None,
        last_sequence: None,
        final_checksum: None,
        integrity_status: crate::runtime::NativeAuditIntegrityStatus::Open,
        created_at: now,
        closed_at: None,
        manifest_relative_path: String::new(),
        frames_relative_path: String::new(),
        raw_content_trusted: true,
    }
}

/// Extract displayable text from provider-native values. Claude's stream-json
/// assistant events put the actual text under `message.content[]`, while
/// other providers commonly emit a string directly. Keeping this extraction
/// here prevents each consumer from having to understand provider envelopes.
fn text_from_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(values) => {
            let parts = values
                .iter()
                .filter_map(text_from_value)
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join(""))
        }
        Value::Object(map) => ["text", "content", "message", "delta", "output", "result"]
            .iter()
            .find_map(|key| map.get(*key).and_then(text_from_value)),
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

fn text_or_number_from_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn error_marker(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(true)) => true,
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Number(_)) => true,
        Some(Value::Object(map)) => !map.is_empty(),
        Some(Value::Array(values)) => !values.is_empty(),
        _ => false,
    }
}

fn is_provider_error(object: Option<&serde_json::Map<String, Value>>, event_type: &str) -> bool {
    let Some(object) = object else {
        return false;
    };
    object
        .get("is_api_error_message")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || object
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || error_marker(object.get("api_error_status"))
        || error_marker(object.get("error"))
        || object
            .get("subtype")
            .and_then(Value::as_str)
            .is_some_and(|subtype| {
                let subtype = subtype.to_ascii_lowercase();
                subtype.contains("error") || subtype.contains("fail")
            })
        || (event_type == "result"
            && object
                .get("result")
                .and_then(Value::as_str)
                .is_some_and(|result| result.starts_with("API Error:")))
}

fn provider_error(
    provider: DirectProvider,
    object: Option<&serde_json::Map<String, Value>>,
    params: Option<&serde_json::Map<String, Value>>,
) -> AgentRuntimeError {
    let text = |map: Option<&serde_json::Map<String, Value>>| {
        map.and_then(|map| {
            ["result", "message", "reason", "output", "error"]
                .iter()
                .find_map(|key| map.get(*key).and_then(text_from_value))
        })
    };
    let message = text(object)
        .or_else(|| text(params))
        .or_else(|| {
            object
                .and_then(|map| map.get("api_error_status"))
                .map(|status| format!("provider API error (status {status})"))
        })
        .unwrap_or_else(|| "provider error".to_string());
    AgentRuntimeError::new(AgentRuntimeErrorKind::Unknown, message)
        .with_provider(Some(provider.id()))
}

fn claude_api_retry_message(object: &serde_json::Map<String, Value>) -> String {
    let message = ["message", "error", "reason", "detail", "details", "cause"]
        .iter()
        .find_map(|key| object.get(*key).and_then(text_from_value));
    let attempt = [
        "attempt",
        "attemptNumber",
        "attempt_number",
        "retryAttempt",
        "retry_attempt",
        "retryCount",
        "retry_count",
    ]
    .iter()
    .find_map(|key| object.get(*key))
    .and_then(text_or_number_from_value);
    let max_retries = ["max_retries", "maxRetries", "max_attempts", "maxAttempts"]
        .iter()
        .find_map(|key| object.get(*key))
        .and_then(text_or_number_from_value);
    let delay_ms = ["retry_delay_ms", "retryDelayMs", "delay_ms", "delayMs"]
        .iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_u64);

    let mut details = Vec::new();
    if let Some(attempt) = attempt {
        details.push(match max_retries {
            Some(max_retries) => format!("attempt {attempt}/{max_retries}"),
            None => format!("attempt {attempt}"),
        });
    }
    if let Some(delay_ms) = delay_ms {
        details.push(if delay_ms >= 1_000 && delay_ms % 1_000 == 0 {
            format!("next in {}s", delay_ms / 1_000)
        } else {
            format!("next in {delay_ms}ms")
        });
    }

    let mut content = "Claude Code is retrying a temporary API failure".to_string();
    if let Some(message) = message.filter(|message| !message.trim().is_empty()) {
        content.push_str(": ");
        content.push_str(&message);
    }
    if !details.is_empty() {
        content.push_str(" (");
        content.push_str(&details.join(", "));
        content.push(')');
    }
    content
}

fn classify_payload(
    provider: DirectProvider,
    event: &ProviderEvent,
) -> Result<TypedProviderEvent, NativeAuditError> {
    let Some(payload) = event.payload_json.as_ref() else {
        let text = String::from_utf8_lossy(&event.payload).trim().to_string();
        if text.is_empty() {
            return Ok(TypedProviderEvent::Unknown {
                event_type: "empty".to_string(),
                payload: Value::Null,
            });
        }
        return Ok(TypedProviderEvent::Message {
            role: AgentRuntimeMessageRole::Assistant,
            content: text,
            final_output: false,
            message_id: None,
        });
    };

    if provider == DirectProvider::OhMyPi
        && let Some(typed) = super::oh_my_pi::rpc::classify(payload)
    {
        return Ok(typed);
    }

    if provider == DirectProvider::Codex
        && let Some(typed) = codex_items::classify(payload)
    {
        return Ok(typed);
    }

    if provider.transport() == AgentTransportKind::Acp
        && let Ok(acp_event) = serde_json::from_value::<super::acp::AcpEvent>(payload.clone())
    {
        return Ok(classify_acp_event(provider, acp_event, payload));
    }

    let object = payload.as_object();
    let event_type = object
        .and_then(|map| map.get("type").or_else(|| map.get("method")))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_ascii_lowercase();
    let params = object
        .and_then(|map| map.get("params"))
        .and_then(Value::as_object);
    let value = |keys: &[&str]| -> Option<&Value> {
        object
            .and_then(|map| keys.iter().find_map(|key| map.get(*key)))
            .or_else(|| params.and_then(|map| keys.iter().find_map(|key| map.get(*key))))
    };
    let text =
        || value(&["text", "content", "message", "delta", "output"]).and_then(text_from_value);
    let message_id = || {
        object
            .and_then(|map| map.get("message"))
            .and_then(Value::as_object)
            .and_then(|message| message.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                value(&["message_id", "messageId", "item_id", "itemId"])
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
    };

    match event_type.as_str() {
        "init" | "initialize" | "session" | "session_started" | "thread_started"
        | "session_observed" => value(&["session_id", "sessionId", "thread_id", "threadId"])
            .and_then(Value::as_str)
            .map(|id| TypedProviderEvent::SessionObserved(id.to_string()))
            .ok_or(NativeAuditError::MalformedFrame(event.sequence)),
        "thinking" | "reasoning" | "thought" => {
            Ok(TypedProviderEvent::Thinking(text().unwrap_or_default()))
        }
        "system"
            if provider == DirectProvider::ClaudeCode
                && object
                    .and_then(|map| map.get("subtype"))
                    .and_then(Value::as_str)
                    == Some("api_retry") =>
        {
            let object = object.expect("api_retry classification requires an object");
            Ok(TypedProviderEvent::Message {
                role: AgentRuntimeMessageRole::System,
                content: claude_api_retry_message(object),
                final_output: false,
                // All retries in one process attempt update one visible status row.
                message_id: Some("claude-api-retry".to_string()),
            })
        }
        "assistant" if is_provider_error(object, &event_type) => Ok(TypedProviderEvent::Error(
            provider_error(provider, object, params),
        )),
        "assistant" | "message" | "text" | "content" | "delta" | "assistant_message"
        | "text_delta" => Ok(TypedProviderEvent::Message {
            role: AgentRuntimeMessageRole::Assistant,
            content: text().unwrap_or_default(),
            final_output: value(&["final", "is_final", "done"])
                .and_then(Value::as_bool)
                .unwrap_or(false),
            message_id: message_id(),
        }),
        "user" | "user_message" => Ok(TypedProviderEvent::Message {
            role: AgentRuntimeMessageRole::User,
            content: text().unwrap_or_default(),
            final_output: false,
            message_id: message_id(),
        }),
        // Codex app-server emits JSON-RPC notifications rather than the
        // generic `assistant`/`completed` event names handled above.
        "item/agentmessage/delta" => Ok(TypedProviderEvent::Message {
            role: AgentRuntimeMessageRole::Assistant,
            content: value(&["delta", "text", "content"])
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            final_output: false,
            message_id: value(&["item_id", "itemId", "message_id", "messageId"])
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "item/completed" => {
            let item = params
                .and_then(|map| map.get("item"))
                .and_then(Value::as_object);
            let item_type = item
                .and_then(|map| map.get("type"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if item_type == "agentmessage" {
                Ok(TypedProviderEvent::Message {
                    role: AgentRuntimeMessageRole::Assistant,
                    content: item
                        .and_then(|map| {
                            ["text", "content", "message"]
                                .iter()
                                .find_map(|key| map.get(*key))
                        })
                        .and_then(text_from_value)
                        .unwrap_or_default(),
                    final_output: true,
                    message_id: item
                        .and_then(|map| map.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                })
            } else {
                Ok(TypedProviderEvent::Unknown {
                    event_type,
                    payload: payload.clone(),
                })
            }
        }
        "turn/completed" => {
            let status = params
                .and_then(|map| map.get("turn"))
                .and_then(Value::as_object)
                .and_then(|turn| turn.get("status"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            match status.as_str() {
                "completed" => Ok(TypedProviderEvent::Lifecycle(AgentRunStatus::Succeeded)),
                "interrupted" | "cancelled" | "canceled" => {
                    Ok(TypedProviderEvent::Lifecycle(AgentRunStatus::Cancelled))
                }
                "failed" => Ok(TypedProviderEvent::Lifecycle(AgentRunStatus::Failed)),
                _ => Ok(TypedProviderEvent::Unknown {
                    event_type,
                    payload: payload.clone(),
                }),
            }
        }
        "thread/started" | "thread/created" => {
            let thread = params
                .and_then(|map| map.get("thread"))
                .and_then(Value::as_object);
            thread
                .and_then(|map| map.get("id"))
                .or_else(|| value(&["thread_id", "threadId", "id"]))
                .and_then(Value::as_str)
                .map(|id| TypedProviderEvent::SessionObserved(id.to_string()))
                .ok_or(NativeAuditError::MalformedFrame(event.sequence))
        }
        "tool_call" | "tool_use" | "function_call" | "tool_start" => {
            Ok(TypedProviderEvent::ToolCall {
                id: value(&["id", "call_id", "tool_call_id"])
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                name: value(&["name", "tool_name", "tool"])
                    .and_then(Value::as_str)
                    .unwrap_or("unknown_tool")
                    .to_string(),
                status: if value(&["approval_required", "requires_approval"])
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    AgentRuntimeToolStatus::WaitingApproval
                } else {
                    AgentRuntimeToolStatus::Running
                },
                arguments: value(&["arguments", "input", "args"]).cloned(),
                result: None,
            })
        }
        "tool_result" | "tool_end" | "function_result" => Ok(TypedProviderEvent::ToolCall {
            id: value(&["id", "call_id", "tool_call_id"])
                .and_then(Value::as_str)
                .map(str::to_owned),
            name: value(&["name", "tool_name", "tool"])
                .and_then(Value::as_str)
                .unwrap_or("unknown_tool")
                .to_string(),
            status: if value(&["error", "failed"])
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                AgentRuntimeToolStatus::Failed
            } else {
                AgentRuntimeToolStatus::Succeeded
            },
            arguments: None,
            result: value(&["result", "output", "content"]).cloned(),
        }),
        "approval_requested" | "permission_request" | "tool_approval" => {
            Ok(TypedProviderEvent::ApprovalRequested {
                id: value(&["approval_id", "id", "request_id"])
                    .and_then(Value::as_str)
                    .unwrap_or("unknown-approval")
                    .to_string(),
                tool_call_id: value(&["tool_call_id", "call_id"])
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                tool_name: value(&["tool_name", "name", "tool"])
                    .and_then(Value::as_str)
                    .unwrap_or("unknown_tool")
                    .to_string(),
            })
        }
        "approval_resolved" | "permission_response" | "tool_approval_response" => {
            Ok(TypedProviderEvent::ApprovalResolved {
                id: value(&["approval_id", "id", "request_id"])
                    .and_then(Value::as_str)
                    .unwrap_or("unknown-approval")
                    .to_string(),
                approved: value(&["approved", "allow"])
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                reason: value(&["reason", "message"])
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        }
        "input_requested" | "input_request" | "question" | "ask_user" => {
            Ok(TypedProviderEvent::InputRequested {
                id: value(&["input_id", "id", "request_id"])
                    .and_then(Value::as_str)
                    .unwrap_or("unknown-input")
                    .to_string(),
                prompt: value(&["prompt", "question", "message"])
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        }
        "input_resolved" | "question_answered" | "answer" => {
            Ok(TypedProviderEvent::InputResolved {
                id: value(&["input_id", "id", "request_id"])
                    .and_then(Value::as_str)
                    .unwrap_or("unknown-input")
                    .to_string(),
                answered: value(&["answered", "ok"])
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            })
        }
        "usage" | "token_usage" | "tokens" => Ok(TypedProviderEvent::TokenUsage {
            input_tokens: value(&["input_tokens", "prompt_tokens"])
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: value(&["output_tokens", "completion_tokens"])
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cached_input_tokens: value(&["cached_input_tokens", "cache_read_tokens"])
                .and_then(Value::as_u64),
        }),
        "result" | "completed" | "done" | "success" => {
            if is_provider_error(object, &event_type) {
                Ok(TypedProviderEvent::Error(provider_error(
                    provider, object, params,
                )))
            } else {
                Ok(TypedProviderEvent::Lifecycle(AgentRunStatus::Succeeded))
            }
        }
        "cancelled" | "canceled" | "abort" => {
            Ok(TypedProviderEvent::Lifecycle(AgentRunStatus::Cancelled))
        }
        "error" | "failure" | "failed" | "protocol_error" => Ok(TypedProviderEvent::Error(
            AgentRuntimeError::new(
                if event_type.contains("protocol") {
                    AgentRuntimeErrorKind::ProtocolFailed
                } else {
                    AgentRuntimeErrorKind::Unknown
                },
                value(&["message", "error", "reason"])
                    .and_then(Value::as_str)
                    .unwrap_or("provider error"),
            )
            .with_provider(Some(provider.id())),
        )),
        "eof" | "exit" | "terminal" => Err(NativeAuditError::MalformedFrame(event.sequence)),
        _ => Ok(TypedProviderEvent::Unknown {
            event_type,
            payload: payload.clone(),
        }),
    }
}

fn classify_acp_event(
    provider: DirectProvider,
    event: super::acp::AcpEvent,
    payload: &Value,
) -> TypedProviderEvent {
    use agent_client_protocol::{ContentBlock, ToolCallStatus};

    use super::acp::AcpEvent;

    let unknown = || TypedProviderEvent::Unknown {
        event_type: "acp_notification".to_string(),
        payload: payload.clone(),
    };
    let text = |content: ContentBlock| match content {
        ContentBlock::Text(text) => Some(text.text),
        _ => None,
    };
    let status = |status: ToolCallStatus| match status {
        ToolCallStatus::Completed => AgentRuntimeToolStatus::Succeeded,
        ToolCallStatus::Failed => AgentRuntimeToolStatus::Failed,
        _ => AgentRuntimeToolStatus::Running,
    };
    match event {
        AcpEvent::SessionStart(session) => TypedProviderEvent::SessionObserved(session),
        AcpEvent::CatalogObserved(observation) => TypedProviderEvent::Unknown {
            event_type: super::acp::session_config::ACP_CATALOG_EVENT.to_string(),
            payload: serde_json::to_value(observation).unwrap_or(Value::Null),
        },
        AcpEvent::ConfigurationObserved(observation) => TypedProviderEvent::Unknown {
            event_type: "runtime_configuration".into(),
            payload: serde_json::to_value(observation).unwrap_or(Value::Null),
        },
        AcpEvent::User(content) => TypedProviderEvent::Message {
            role: AgentRuntimeMessageRole::User,
            content,
            final_output: false,
            message_id: None,
        },
        AcpEvent::Message(content) => text(content)
            .map(|content| TypedProviderEvent::Message {
                role: AgentRuntimeMessageRole::Assistant,
                content,
                final_output: false,
                message_id: None,
            })
            .unwrap_or_else(unknown),
        AcpEvent::Thought(content) => text(content)
            .map(TypedProviderEvent::Thinking)
            .unwrap_or_else(unknown),
        AcpEvent::ToolCall(call) => TypedProviderEvent::ToolCall {
            id: Some(call.tool_call_id.0.to_string()),
            name: call.title,
            status: status(call.status),
            arguments: call.raw_input,
            result: call.raw_output,
        },
        AcpEvent::ToolUpdate(update) => TypedProviderEvent::ToolCall {
            id: Some(update.tool_call_id.0.to_string()),
            name: update.fields.title.unwrap_or_else(|| "tool".to_string()),
            status: update
                .fields
                .status
                .map(status)
                .unwrap_or(AgentRuntimeToolStatus::Running),
            arguments: update.fields.raw_input,
            result: update.fields.raw_output,
        },
        AcpEvent::RequestPermission(request) => TypedProviderEvent::ToolCall {
            id: Some(request.tool_call.tool_call_id.0.to_string()),
            name: request
                .tool_call
                .fields
                .title
                .unwrap_or_else(|| "tool".to_string()),
            status: AgentRuntimeToolStatus::WaitingApproval,
            arguments: request.tool_call.fields.raw_input,
            result: None,
        },
        AcpEvent::ApprovalRequested {
            tool_call_id,
            approval_id,
        } => TypedProviderEvent::ApprovalRequested {
            id: approval_id,
            tool_call_id: Some(tool_call_id),
            tool_name: "tool".to_string(),
        },
        AcpEvent::ApprovalResponse(response) => TypedProviderEvent::ApprovalResolved {
            id: response.approval_id.unwrap_or(response.tool_call_id),
            approved: matches!(
                response.status,
                workspace_utils::approvals::ApprovalStatus::Approved
            ),
            reason: match response.status {
                workspace_utils::approvals::ApprovalStatus::Denied { reason } => reason,
                _ => None,
            },
        },
        AcpEvent::Usage(usage) => TypedProviderEvent::TokenUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_input_tokens: usage.cached_read_tokens,
        },
        AcpEvent::Error(message) => TypedProviderEvent::Error(
            AgentRuntimeError::new(AgentRuntimeErrorKind::Unknown, message)
                .with_provider(Some(provider.id())),
        ),
        AcpEvent::Done(reason) => {
            let reason = serde_json::from_str::<String>(&reason).unwrap_or(reason);
            match reason.as_str() {
                "cancelled" | "canceled" => {
                    TypedProviderEvent::Lifecycle(AgentRunStatus::Cancelled)
                }
                "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" => {
                    TypedProviderEvent::Lifecycle(AgentRunStatus::Succeeded)
                }
                _ => unknown(),
            }
        }
        _ => unknown(),
    }
}

fn map_typed_event(
    provider: DirectProvider,
    event: &DecodedProviderEvent,
    manifest: &NativeAuditManifest,
) -> Result<Vec<AgentEvent>, NativeAuditError> {
    let payload = if let Some(evidence) =
        file_evidence::completed_file_changes(provider, &event.raw)
    {
        evidence
    } else {
        match &event.typed {
            TypedProviderEvent::Lifecycle(status) => {
                AgentEventPayload::LifecycleChanged { status: *status }
            }
            TypedProviderEvent::SessionObserved(session_id) => AgentEventPayload::SessionObserved {
                provider_session: ProviderSessionReference {
                    schema_version: crate::runtime::PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION,
                    provider_id: provider.id().to_string(),
                    runtime_profile_id: manifest.runtime_profile_id.clone(),
                    provider_session_id: session_id.clone(),
                    observed_at: event.raw.timestamp,
                    metadata: Some(serde_json::json!({ "source": "native_frame" })),
                },
            },
            TypedProviderEvent::Message {
                role,
                content,
                final_output,
                message_id,
            } => AgentEventPayload::Message {
                message: crate::runtime::CanonicalMessage {
                    message_id: message_id
                        .clone()
                        .map(|id| canonical_message_id(manifest.run_attempt_id, &id))
                        .unwrap_or_else(|| event_id(manifest.run_attempt_id, event.raw.sequence)),
                    role: *role,
                    content: content.clone(),
                },
                final_output: *final_output,
            },
            TypedProviderEvent::Thinking(content) => AgentEventPayload::Thinking {
                content: content.clone(),
            },
            TypedProviderEvent::ToolCall {
                id,
                name,
                status,
                arguments,
                result,
            } => AgentEventPayload::ToolCall {
                tool_call_id: id.clone(),
                tool_name: name.clone(),
                status: *status,
                arguments: arguments.clone(),
                result: result.clone(),
            },
            TypedProviderEvent::ApprovalRequested {
                id,
                tool_call_id,
                tool_name,
            } => AgentEventPayload::ApprovalRequested {
                approval_id: id.clone(),
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
            },
            TypedProviderEvent::ApprovalResolved {
                id,
                approved,
                reason,
            } => AgentEventPayload::ApprovalResolved {
                approval_id: id.clone(),
                approved: *approved,
                reason: reason.clone(),
            },
            TypedProviderEvent::InputRequested { id, prompt } => {
                AgentEventPayload::InputRequested {
                    input_id: id.clone(),
                    prompt: prompt.clone(),
                }
            }
            TypedProviderEvent::InputResolved { id, answered } => {
                AgentEventPayload::InputResolved {
                    input_id: id.clone(),
                    answered: *answered,
                }
            }
            TypedProviderEvent::TokenUsage {
                input_tokens,
                output_tokens,
                cached_input_tokens,
            } => AgentEventPayload::TokenUsage {
                input_tokens: *input_tokens,
                output_tokens: *output_tokens,
                cached_input_tokens: *cached_input_tokens,
            },
            TypedProviderEvent::Error(error) => AgentEventPayload::Error {
                error: error.clone(),
            },
            TypedProviderEvent::Unknown {
                event_type,
                payload,
            } => AgentEventPayload::ProviderExtension {
                provider_namespace: provider.id().to_string(),
                provider_event: event_type.clone(),
                payload: payload.clone(),
            },
        }
    };
    Ok(vec![envelope_for_payload(&event.raw, manifest, payload)])
}

fn envelope_for_payload(
    event: &ProviderEvent,
    manifest: &NativeAuditManifest,
    payload: AgentEventPayload,
) -> AgentEventEnvelope {
    AgentEventEnvelope {
        schema_version: AGENT_EVENT_SCHEMA_VERSION,
        payload_version: AGENT_EVENT_PAYLOAD_VERSION,
        event_id: event_id(manifest.run_attempt_id, event.sequence),
        session_id: manifest.session_id,
        agent_run_id: manifest.agent_run_id,
        turn_id: manifest.turn_id,
        run_attempt_id: manifest.run_attempt_id,
        run_attempt_number: manifest.run_attempt_number,
        sequence: event.sequence,
        correlation_id: event.correlation_id,
        orchestration_run_id: None,
        orchestration_node_execution_id: None,
        timestamp: event.timestamp,
        native_refs: vec![event.native_ref.clone()],
        payload,
    }
}

fn event_id(run_attempt_id: Uuid, sequence: u64) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(run_attempt_id.as_bytes());
    hasher.update(sequence.to_be_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hasher.finalize()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn canonical_message_id(run_attempt_id: Uuid, native_id: &str) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(run_attempt_id.as_bytes());
    hasher.update(b"canonical-message:");
    hasher.update(native_id.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hasher.finalize()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// Serialize one NDJSON request without appending a second frame or accepting
/// an embedded SDK.  Callers write the returned bytes directly to `omp` stdin.
pub fn encode_stdio_rpc(request: &Value) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = serde_json::to_vec(request)?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::*;
    use crate::{
        approvals::NoopExecutorApprovalService,
        env::RepoContext,
        executors::BaseCodingAgent,
        runtime::{NativeAuditChannel, PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION},
    };

    #[test]
    fn attempt_model_snapshot_does_not_resurrect_cleared_preset_values() {
        for (name, executor, effort_key) in [
            ("GEMINI", BaseCodingAgent::Gemini, None),
            (
                "CODEX",
                BaseCodingAgent::Codex,
                Some("model_reasoning_effort"),
            ),
            ("CLAUDE_CODE", BaseCodingAgent::ClaudeCode, Some("effort")),
            ("OH_MY_PI", BaseCodingAgent::OhMyPi, Some("thinking_level")),
            (
                "OPENCODE",
                BaseCodingAgent::Opencode,
                Some("reasoning_effort"),
            ),
            (
                "DEEPSEEK_HARNESS",
                BaseCodingAgent::DeepseekHarness,
                Some("reasoning_effort"),
            ),
        ] {
            let mut settings = serde_json::json!({
                "model": "preset-model",
                "env": {"PRESERVED_PROFILE_OPTION": "value"},
            });
            if let Some(key) = effort_key {
                settings[key] = serde_json::json!("high");
            }
            let mut agent: CodingAgent =
                serde_json::from_value(serde_json::json!({(name): settings})).unwrap();
            let inherited = serde_json::to_value(&agent).unwrap();
            let profile_only = ExecutorConfig::new(executor);
            apply_attempt_model_selection(&mut agent, &profile_only).unwrap();
            assert_eq!(serde_json::to_value(&agent).unwrap(), inherited, "{name}");

            for (field, native_key) in std::iter::once(("model_id", "model"))
                .chain(effort_key.map(|key| ("reasoning_id", key)))
            {
                let mut isolated = agent.clone();
                let partial: ExecutorConfig = serde_json::from_value(serde_json::json!({
                    "executor": executor,
                    (field): null,
                }))
                .unwrap();
                apply_attempt_model_selection(&mut isolated, &partial).unwrap();
                let mut expected = inherited.clone();
                expected[name].as_object_mut().unwrap().remove(native_key);
                assert_eq!(
                    serde_json::to_value(isolated).unwrap(),
                    expected,
                    "{name}/{field}"
                );
            }

            let cleared: ExecutorConfig = serde_json::from_value(serde_json::json!({
                "executor": executor,
                "model_id": null,
                "reasoning_id": null,
            }))
            .unwrap();
            cleared.validate_provider_options().unwrap();
            // Audit/persistence must retain null rather than turning it back into omission.
            let cleared: ExecutorConfig =
                serde_json::from_value(serde_json::to_value(&cleared).unwrap()).unwrap();
            let mut expected = inherited;
            expected[name].as_object_mut().unwrap().remove("model");
            if let Some(key) = effort_key {
                expected[name].as_object_mut().unwrap().remove(key);
            }

            apply_attempt_model_selection(&mut agent, &cleared).unwrap();
            assert_eq!(serde_json::to_value(&agent).unwrap(), expected, "{name}");

            let mut explicit = ExecutorConfig::new(executor);
            explicit.model_id = Some(Some("selected-model".into()));
            explicit.reasoning_id = effort_key.map(|_| Some("low".into()));
            apply_attempt_model_selection(&mut agent, &explicit).unwrap();
            expected[name]["model"] = serde_json::json!("selected-model");
            if let Some(key) = effort_key {
                expected[name][key] = serde_json::json!("low");
            }
            assert_eq!(serde_json::to_value(&agent).unwrap(), expected, "{name}");
        }
    }

    fn frame(provider: DirectProvider, payload: Value) -> NativeAuditFrame {
        NativeAuditFrame::from_bytes(
            1,
            DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            crate::runtime::NativeAuditDirection::Output,
            NativeAuditChannel::Stdout,
            "application/json",
            Uuid::from_u128(10),
            serde_json::to_string(&payload).unwrap().as_bytes(),
            Some(serde_json::json!({ "provider": provider.id() })),
        )
    }

    fn launch_env() -> ExecutionEnv {
        ExecutionEnv::new(
            RepoContext::new(".".into(), Vec::new()),
            false,
            String::new(),
        )
    }

    fn provider_session(
        provider: DirectProvider,
        config: &ExecutorConfig,
    ) -> ProviderSessionReference {
        ProviderSessionReference {
            schema_version: PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION,
            provider_id: provider.id().to_string(),
            runtime_profile_id: config.profile_id().cache_key(),
            provider_session_id: "native-session".to_string(),
            observed_at: Utc::now(),
            metadata: None,
        }
    }

    fn validate_launch(
        provider: DirectProvider,
        config: &ExecutorConfig,
        intent: DirectIntent,
        session: Option<&ProviderSessionReference>,
        reset_to_message_id: Option<&str>,
    ) -> Result<(), ExecutorError> {
        let env = launch_env();
        validate_direct_launch(&DirectProviderLaunchRequest {
            provider,
            executor_config: config,
            intent,
            prompt: "prompt",
            provider_session: session,
            reset_to_message_id,
            selected_skills: &[],
            approvals: Arc::new(NoopExecutorApprovalService),
            current_dir: Path::new("."),
            env: &env,
        })
    }

    #[test]
    fn follow_up_accepts_default_alias_but_rejects_named_profile_changes() {
        let config = ExecutorConfig::new(BaseCodingAgent::Codex);
        let mut session = provider_session(DirectProvider::Codex, &config);
        session.runtime_profile_id = "CODEX:DEFAULT".to_string();
        assert!(
            validate_launch(
                DirectProvider::Codex,
                &config,
                DirectIntent::FollowUp,
                Some(&session),
                None
            )
            .is_ok()
        );
        session.runtime_profile_id = "CODEX:PLAN".to_string();
        assert!(
            validate_launch(
                DirectProvider::Codex,
                &config,
                DirectIntent::FollowUp,
                Some(&session),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn all_six_versions_use_local_executables() {
        let versions = DirectProvider::ALL.map(DirectProvider::versions);

        assert_eq!(
            versions.map(|version| version.executable),
            ["gemini", "codex", "claude", "omp", "opencode", "dsh"]
        );
        assert!(versions.iter().all(|version| version.runtime.is_none()));
        assert_eq!(versions[1].protocol, Some("rust-v0.144.1"));
        assert!(
            versions
                .windows(2)
                .all(|pair| pair[0].adapter != pair[1].adapter)
        );
    }

    #[test]
    fn all_six_profile_identities_map_to_their_direct_provider() {
        let mappings = [
            (BaseCodingAgent::Gemini, DirectProvider::Gemini),
            (BaseCodingAgent::Codex, DirectProvider::Codex),
            (BaseCodingAgent::ClaudeCode, DirectProvider::ClaudeCode),
            (BaseCodingAgent::OhMyPi, DirectProvider::OhMyPi),
            (BaseCodingAgent::Opencode, DirectProvider::Opencode),
            (
                BaseCodingAgent::DeepseekHarness,
                DirectProvider::DeepseekHarness,
            ),
        ];

        for (agent, provider) in mappings {
            assert_eq!(DirectProvider::from_base_agent(agent), Some(provider));
            let config = ExecutorConfig::new(agent);
            assert!(validate_launch(provider, &config, DirectIntent::Initial, None, None).is_ok());
        }
    }

    #[test]
    fn profile_provider_mismatch_is_rejected_before_spawn() {
        let error = validate_launch(
            DirectProvider::Gemini,
            &ExecutorConfig::new(BaseCodingAgent::Codex),
            DirectIntent::Initial,
            None,
            None,
        )
        .expect_err("mismatched profile must not launch");

        assert!(matches!(error, ExecutorError::UnknownExecutorType(_)));
    }

    #[test]
    fn follow_up_requires_a_matching_non_empty_provider_session() {
        let config = ExecutorConfig::new(BaseCodingAgent::Codex);
        let error = validate_launch(
            DirectProvider::Codex,
            &config,
            DirectIntent::FollowUp,
            None,
            None,
        )
        .expect_err("follow-up without a session must not launch");
        assert!(matches!(error, ExecutorError::FollowUpNotSupported(_)));

        let mut session = provider_session(DirectProvider::Codex, &config);
        session.runtime_profile_id = "CODEX:OTHER".to_string();
        let error = validate_launch(
            DirectProvider::Codex,
            &config,
            DirectIntent::FollowUp,
            Some(&session),
            None,
        )
        .expect_err("session from another profile must not launch");
        assert!(matches!(error, ExecutorError::FollowUpNotSupported(_)));
    }

    #[test]
    fn native_session_ids_reject_ambiguous_whitespace_and_option_values() {
        assert!(validate_native_session_id(DirectProvider::Gemini, " session ").is_err());
        assert!(validate_native_session_id(DirectProvider::OhMyPi, "-resume").is_err());
        assert!(validate_native_session_id(DirectProvider::Codex, "thread-1").is_ok());
    }

    #[test]
    fn initial_launch_rejects_an_attached_provider_session() {
        let config = ExecutorConfig::new(BaseCodingAgent::Gemini);
        let session = provider_session(DirectProvider::Gemini, &config);

        let error = validate_launch(
            DirectProvider::Gemini,
            &config,
            DirectIntent::Initial,
            Some(&session),
            None,
        )
        .expect_err("initial launch must not accidentally resume");

        assert!(matches!(error, ExecutorError::FollowUpNotSupported(_)));
    }

    #[test]
    fn reset_is_only_allowed_for_claude_follow_up_or_resume() {
        for provider in [
            DirectProvider::Gemini,
            DirectProvider::Codex,
            DirectProvider::OhMyPi,
            DirectProvider::Opencode,
            DirectProvider::DeepseekHarness,
        ] {
            let agent = match provider {
                DirectProvider::Gemini => BaseCodingAgent::Gemini,
                DirectProvider::Codex => BaseCodingAgent::Codex,
                DirectProvider::OhMyPi => BaseCodingAgent::OhMyPi,
                DirectProvider::Opencode => BaseCodingAgent::Opencode,
                DirectProvider::DeepseekHarness => BaseCodingAgent::DeepseekHarness,
                DirectProvider::ClaudeCode => unreachable!(),
            };
            let config = ExecutorConfig::new(agent);
            let session = provider_session(provider, &config);
            let error = validate_launch(
                provider,
                &config,
                DirectIntent::FollowUp,
                Some(&session),
                Some("message-id"),
            )
            .expect_err("unsupported reset must not silently downgrade");
            assert!(matches!(
                error,
                ExecutorError::ResetToMessageNotSupported(_)
            ));
        }

        let config = ExecutorConfig::new(BaseCodingAgent::ClaudeCode);
        let session = provider_session(DirectProvider::ClaudeCode, &config);
        assert!(
            validate_launch(
                DirectProvider::ClaudeCode,
                &config,
                DirectIntent::Resume,
                Some(&session),
                Some("message-id"),
            )
            .is_ok()
        );
    }

    #[test]
    fn unknown_and_terminal_frames_fail_closed() {
        let unknown = DirectProvider::Gemini
            .decode_native_frame(&frame(
                DirectProvider::Gemini,
                serde_json::json!({"type":"future_event"}),
            ))
            .unwrap();
        assert!(matches!(unknown.typed, TypedProviderEvent::Unknown { .. }));
        assert!(
            DirectProvider::Gemini
                .decode_native_frame(&frame(
                    DirectProvider::Gemini,
                    serde_json::json!({"type":"eof"})
                ))
                .is_err()
        );
    }

    #[test]
    fn runtime_configuration_frames_cross_native_audit_as_extensions() {
        for provider in [
            DirectProvider::ClaudeCode,
            DirectProvider::Codex,
            DirectProvider::Gemini,
            DirectProvider::OhMyPi,
            DirectProvider::Opencode,
            DirectProvider::DeepseekHarness,
        ] {
            let value = serde_json::json!({
                "type":"runtime_configuration", "model_id":"native-resolved-model", "source":"native.fixture"
            });
            let decoded = provider
                .decode_native_frame(&frame(provider, value.clone()))
                .unwrap();
            assert!(
                matches!(&decoded.typed, TypedProviderEvent::Unknown { event_type, payload } if event_type == "runtime_configuration" && payload == &value)
            );
            let manifest = fixture_manifest(provider);
            let events = provider.map_provider_event(&decoded, &manifest).unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].run_attempt_id, manifest.run_attempt_id);
            assert_eq!(events[0].session_id, manifest.session_id);
            assert!(
                matches!(&events[0].payload, AgentEventPayload::ProviderExtension { provider_namespace, provider_event, payload } if provider_namespace == provider.id() && provider_event == "runtime_configuration" && payload["model_id"] == "native-resolved-model" && payload.get("reasoning_id").is_none())
            );
        }
    }

    #[test]
    fn codex_app_server_messages_and_turn_completion_are_canonical() {
        let delta = DirectProvider::Codex
            .decode_native_frame(&frame(
                DirectProvider::Codex,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "item/agentMessage/delta",
                    "params": {
                        "threadId": "thread-1",
                        "turnId": "turn-1",
                        "itemId": "item-1",
                        "delta": "hello"
                    }
                }),
            ))
            .unwrap();
        assert!(matches!(
            delta.typed,
            TypedProviderEvent::Message {
                role: AgentRuntimeMessageRole::Assistant,
                ref content,
                message_id: Some(ref id),
                final_output: false,
            } if content == "hello" && id == "item-1"
        ));

        let completed = DirectProvider::Codex
            .decode_native_frame(&frame(
                DirectProvider::Codex,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "turn/completed",
                    "params": {
                        "turn": { "id": "turn-1", "status": "completed" }
                    }
                }),
            ))
            .unwrap();
        assert_eq!(
            completed.typed,
            TypedProviderEvent::Lifecycle(AgentRunStatus::Succeeded)
        );
    }

    #[test]
    fn codex_completed_agent_message_reuses_item_identity() {
        let event = DirectProvider::Codex
            .decode_native_frame(&frame(
                DirectProvider::Codex,
                serde_json::json!({
                    "method": "item/completed",
                    "params": {
                        "item": {
                            "type": "agentMessage",
                            "id": "item-1",
                            "text": "hello world"
                        }
                    }
                }),
            ))
            .unwrap();
        let manifest = NativeAuditManifest {
            audit_schema_version: crate::runtime::NATIVE_AUDIT_SCHEMA_VERSION,
            session_id: Uuid::from_u128(1),
            agent_run_id: Uuid::from_u128(2),
            turn_id: Uuid::from_u128(3),
            run_attempt_id: Uuid::from_u128(4),
            run_attempt_number: 1,
            provider_id: "codex".to_string(),
            runtime_profile_id: "default".to_string(),
            workspace_path: ".".to_string(),
            runtime_version: None,
            protocol_version: None,
            adapter_version: "codex-adapter-v1".to_string(),
            mapper_version: DirectProvider::Codex.versions().mapper.to_string(),
            frame_count: 1,
            first_sequence: Some(1),
            last_sequence: Some(1),
            final_checksum: None,
            integrity_status: crate::runtime::NativeAuditIntegrityStatus::Complete,
            created_at: Utc::now(),
            closed_at: None,
            manifest_relative_path: "manifest.json".to_string(),
            frames_relative_path: "frames.jsonl".to_string(),
            raw_content_trusted: true,
        };
        let mapped = DirectProvider::Codex
            .map_provider_event(&event, &manifest)
            .unwrap();
        let AgentEventPayload::Message {
            message,
            final_output,
        } = &mapped[0].payload
        else {
            panic!("completed Codex agent message must map to canonical message");
        };
        assert_eq!(message.content, "hello world");
        assert!(final_output);
        assert_eq!(
            message.message_id,
            canonical_message_id(manifest.run_attempt_id, "item-1")
        );
    }

    #[test]
    fn claude_nested_assistant_content_is_projected() {
        let event = DirectProvider::ClaudeCode
            .decode_native_frame(&frame(
                DirectProvider::ClaudeCode,
                serde_json::json!({
                    "type": "assistant",
                    "message": {
                        "id": "msg-1",
                        "role": "assistant",
                        "content": [{"type": "text", "text": "hello from Claude"}]
                    },
                    "session_id": "session-1"
                }),
            ))
            .unwrap();
        assert!(matches!(
            event.typed,
            TypedProviderEvent::Message {
                role: AgentRuntimeMessageRole::Assistant,
                ref content,
                message_id: Some(ref id),
                ..
            } if content == "hello from Claude" && id == "msg-1"
        ));
    }

    #[test]
    fn claude_api_error_events_are_not_success() {
        let assistant_error = DirectProvider::ClaudeCode
            .decode_native_frame(&frame(
                DirectProvider::ClaudeCode,
                serde_json::json!({
                    "type": "assistant",
                    "message": {
                        "id": "msg-error",
                        "role": "assistant",
                        "content": [{
                            "type": "text",
                            "text": "API Error: 503 No available channel"
                        }]
                    },
                    "is_api_error_message": true,
                    "error": "server_error"
                }),
            ))
            .unwrap();
        let TypedProviderEvent::Error(error) = assistant_error.typed else {
            panic!("Claude assistant API errors must map to an error event");
        };
        assert_eq!(error.provider.as_deref(), Some("claude_code"));
        assert!(error.message.contains("API Error: 503"));

        let result_error = DirectProvider::ClaudeCode
            .decode_native_frame(&frame(
                DirectProvider::ClaudeCode,
                serde_json::json!({
                    "type": "result",
                    "subtype": "success",
                    "is_error": true,
                    "api_error_status": 503,
                    "result": "API Error: 503 No available channel"
                }),
            ))
            .unwrap();
        let TypedProviderEvent::Error(error) = result_error.typed else {
            panic!("Claude result API errors must map to an error event");
        };
        assert!(error.message.contains("API Error: 503"));

        let success = DirectProvider::ClaudeCode
            .decode_native_frame(&frame(
                DirectProvider::ClaudeCode,
                serde_json::json!({
                    "type": "result",
                    "subtype": "success",
                    "is_error": false,
                    "result": "completed"
                }),
            ))
            .unwrap();
        assert_eq!(
            success.typed,
            TypedProviderEvent::Lifecycle(AgentRunStatus::Succeeded)
        );
    }

    #[test]
    fn claude_success_with_null_api_error_status_is_not_an_error() {
        let event = DirectProvider::ClaudeCode
            .decode_native_frame(&frame(
                DirectProvider::ClaudeCode,
                serde_json::json!({
                    "type": "result",
                    "subtype": "success",
                    "is_error": false,
                    "api_error_status": null,
                    "terminal_reason": "completed",
                    "result": "Files delivered successfully"
                }),
            ))
            .unwrap();
        assert_eq!(
            event.typed,
            TypedProviderEvent::Lifecycle(AgentRunStatus::Succeeded)
        );
    }

    #[test]
    fn claude_api_retry_is_a_visible_non_terminal_status() {
        let event = DirectProvider::ClaudeCode
            .decode_native_frame(&frame(
                DirectProvider::ClaudeCode,
                serde_json::json!({
                    "type": "system",
                    "subtype": "api_retry",
                    "attempt": 3,
                    "max_retries": 10,
                    "retry_delay_ms": 4000,
                    "error": "server_error"
                }),
            ))
            .unwrap();

        assert!(matches!(
            event.typed,
            TypedProviderEvent::Message {
                role: AgentRuntimeMessageRole::System,
                ref content,
                final_output: false,
                message_id: Some(ref message_id),
            } if content == "Claude Code is retrying a temporary API failure: server_error (attempt 3/10, next in 4s)"
                && message_id == "claude-api-retry"
        ));
    }

    #[test]
    fn maps_approval_and_waiting_input_to_canonical_events() {
        let manifest = NativeAuditManifest {
            audit_schema_version: crate::runtime::NATIVE_AUDIT_SCHEMA_VERSION,
            session_id: Uuid::from_u128(1),
            agent_run_id: Uuid::from_u128(2),
            turn_id: Uuid::from_u128(3),
            run_attempt_id: Uuid::from_u128(4),
            run_attempt_number: 1,
            provider_id: "gemini".to_string(),
            runtime_profile_id: "default".to_string(),
            workspace_path: ".".to_string(),
            runtime_version: None,
            protocol_version: Some("acp-0.8".to_string()),
            adapter_version: "gemini-adapter-v1".to_string(),
            mapper_version: DirectProvider::Gemini.versions().mapper.to_string(),
            frame_count: 1,
            first_sequence: Some(1),
            last_sequence: Some(1),
            final_checksum: None,
            integrity_status: crate::runtime::NativeAuditIntegrityStatus::Complete,
            created_at: Utc::now(),
            closed_at: None,
            manifest_relative_path: "manifest.json".to_string(),
            frames_relative_path: "frames.jsonl".to_string(),
            raw_content_trusted: true,
        };
        let event = DirectProvider::Gemini
            .decode_native_frame(&frame(
                DirectProvider::Gemini,
                serde_json::json!({"type":"approval_requested","approval_id":"a1","tool_name":"shell"}),
            ))
            .unwrap();
        let mapped = DirectProvider::Gemini
            .map_provider_event(&event, &manifest)
            .unwrap();
        assert!(matches!(
            mapped[0].payload,
            AgentEventPayload::ApprovalRequested { .. }
        ));
    }

    #[test]
    fn omp_rpc_is_ndjson_and_external() {
        let request = super::super::oh_my_pi::command_adapter::session_request("hello", None);
        let bytes = encode_stdio_rpc(&request).unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert!(
            String::from_utf8(bytes)
                .unwrap()
                .contains("\"type\":\"prompt\"")
        );
        assert_eq!(DirectProvider::OhMyPi.versions().executable, "omp");
    }

    #[test]
    fn controls_are_provider_native_and_fail_closed_on_unknown_capability() {
        let snapshot = DirectProvider::Gemini.capabilities("default");
        assert_eq!(
            snapshot.resolve(crate::runtime::AgentCapability::Approval),
            CapabilityState::Native
        );
        let marker = encode_control(
            DirectProvider::Gemini,
            &snapshot,
            DirectControl::Approve {
                request_id: "a1".to_string(),
                approved: true,
                reason: None,
            },
        )
        .expect("ACP approvals use a typed route through the active connection");
        assert!(
            marker.is_empty(),
            "the active peer owns the actual request id and frame"
        );
        assert_eq!(
            snapshot.resolve(crate::runtime::AgentCapability::Steering),
            CapabilityState::Unsupported
        );

        let mut unknown = DirectProvider::Gemini.capabilities("default");
        unknown
            .capabilities
            .retain(|entry| entry.capability != crate::runtime::AgentCapability::Steering);
        let error = encode_control(
            DirectProvider::Gemini,
            &unknown,
            DirectControl::Steer {
                text: "stop".to_string(),
            },
        )
        .expect_err("missing steering capability must not silently fall back");
        assert!(matches!(
            error,
            DirectControlError::Capability(CapabilityGateError::Unresolved { .. })
        ));

        let omp_snapshot = DirectProvider::OhMyPi.capabilities("default");
        assert_eq!(
            omp_snapshot.resolve(crate::runtime::AgentCapability::Approval),
            CapabilityState::Unsupported
        );
        let error = encode_control(
            DirectProvider::OhMyPi,
            &omp_snapshot,
            DirectControl::Approve {
                request_id: "a1".to_string(),
                approved: true,
                reason: None,
            },
        )
        .expect_err("Oh My Pi approval is not exposed by its RPC adapter");
        assert!(matches!(
            error,
            DirectControlError::Capability(CapabilityGateError::Unsupported { .. })
        ));
    }

    #[test]
    fn all_provider_replays_use_the_same_decoder_mapper_reducer_path() {
        for provider in DirectProvider::ALL {
            let root = tempfile::tempdir().unwrap();
            let ids = [
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
            ];
            let versions = provider.versions();
            let mut writer = crate::runtime::NativeAuditWriter::create_in(
                root.path(),
                crate::runtime::NativeAuditMetadata {
                    session_id: ids[0],
                    agent_run_id: ids[1],
                    turn_id: ids[2],
                    run_attempt_id: ids[3],
                    run_attempt_number: 1,
                    provider_id: provider.id().to_string(),
                    runtime_profile_id: "default".to_string(),
                    workspace_path: root.path().display().to_string(),
                    runtime_version: versions.runtime.map(str::to_owned),
                    protocol_version: versions.protocol.map(str::to_owned),
                    adapter_version: versions.adapter.to_string(),
                    mapper_version: versions.mapper.to_string(),
                    created_at: Utc::now(),
                },
            )
            .unwrap();
            for payload in [
                serde_json::json!({"type":"session_started","session_id":"native-1"}),
                serde_json::json!({"type":"message","content":"done","final":true}),
                serde_json::json!({"type":"result"}),
            ] {
                writer
                    .append_native_output(
                        NativeAuditChannel::Stdout,
                        "application/json",
                        ids[0],
                        serde_json::to_string(&payload).unwrap().as_bytes(),
                    )
                    .unwrap();
            }
            let manifest = writer.close().unwrap();
            let directory = root
                .path()
                .join(&manifest.manifest_relative_path)
                .parent()
                .unwrap()
                .to_path_buf();
            let bundle = crate::runtime::AuditBundle::read(&directory).unwrap();
            let first = bundle.replay(&provider.mapper()).unwrap();
            let second = bundle.replay(&provider.mapper()).unwrap();
            assert_eq!(first.provider_events, second.provider_events);
            assert_eq!(first.agent_events, second.agent_events);
            assert_eq!(first.state, second.state);
            assert_eq!(first.state.status, AgentRunStatus::Succeeded);
            assert_eq!(
                first.state.last_event_sequence,
                first.agent_events.len() as u64
            );
        }
    }

    #[test]
    fn checked_in_native_fixtures_replay_for_every_v1_provider() {
        for provider in DirectProvider::ALL {
            let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures/native-audit")
                .join(provider.id());
            let bundle = crate::runtime::AuditBundle::read(&directory).unwrap_or_else(|error| {
                panic!("failed to read {} fixture: {error}", provider.id())
            });
            let replay = bundle
                .replay_fixture(&provider.mapper())
                .unwrap_or_else(|error| {
                    panic!("failed to replay {} fixture: {error}", provider.id())
                });

            assert_eq!(bundle.manifest().provider_id, provider.id());
            assert_eq!(replay.provider_events.len(), 3);
            assert!(replay.agent_events.len() >= 3);
            assert_eq!(replay.state.status, AgentRunStatus::Succeeded);
            assert_eq!(
                replay.state.last_event_sequence,
                replay.agent_events.len() as u64
            );
        }
    }
}
