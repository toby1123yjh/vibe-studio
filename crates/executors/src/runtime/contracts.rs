use api_types::SelectedSkill;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use ts_rs::TS;
use uuid::Uuid;

use super::{AgentRuntimeError, AgentRuntimeMessageRole, AgentRuntimeToolStatus};
use crate::profile::{ExecutorConfig, runtime_profile_ids_match};

pub const AGENT_REQUEST_SCHEMA_VERSION: u16 = 1;
pub const AGENT_REQUEST_PAYLOAD_VERSION: u16 = 1;
pub const CAPABILITY_SNAPSHOT_SCHEMA_VERSION: u16 = 1;
pub const PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION: u16 = 1;
pub const AGENT_EVENT_SCHEMA_VERSION: u16 = 1;
pub const AGENT_EVENT_PAYLOAD_VERSION: u16 = 1;
pub const RUN_STATE_SCHEMA_VERSION: u16 = 1;
pub const RUN_STATE_REDUCER_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum AgentRunIntent {
    Initial,
    FollowUp,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum RunAttemptMode {
    Launch,
    Resume,
    Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum WorkspaceMode {
    SharedWorkspace,
    IsolatedWorktree,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct WorkspaceReference {
    pub workspace_id: Uuid,
    pub mode: WorkspaceMode,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum AgentCapability {
    SessionResume,
    Steering,
    Approval,
    Images,
    Review,
    Mcp,
    Subagents,
    TokenUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum CapabilityState {
    Native,
    Emulated,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum CapabilitySource {
    RuntimeProfile,
    VersionProbe,
    RuntimeProbe,
    RunPolicy,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CapabilitySnapshotEntry {
    pub capability: AgentCapability,
    pub state: CapabilityState,
    pub source: CapabilitySource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emulation_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CapabilitySnapshot {
    pub schema_version: u16,
    pub runtime_profile_id: String,
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    pub adapter_version: String,
    pub resolved_at: DateTime<Utc>,
    pub capabilities: Vec<CapabilitySnapshotEntry>,
}

impl CapabilitySnapshot {
    pub fn resolve(&self, capability: AgentCapability) -> CapabilityState {
        self.capabilities
            .iter()
            .find(|entry| entry.capability == capability)
            .map(|entry| entry.state)
            .unwrap_or(CapabilityState::Unknown)
    }

    pub fn validate(&self) -> Result<(), CapabilitySnapshotError> {
        validate_current_version(
            "capability_snapshot.schema",
            self.schema_version,
            CAPABILITY_SNAPSHOT_SCHEMA_VERSION,
        )?;

        for (index, entry) in self.capabilities.iter().enumerate() {
            if self.capabilities[..index]
                .iter()
                .any(|candidate| candidate.capability == entry.capability)
            {
                return Err(CapabilitySnapshotError::Duplicate(entry.capability));
            }

            match (&entry.state, &entry.emulation_policy) {
                (CapabilityState::Emulated, Some(policy)) if !policy.trim().is_empty() => {}
                (CapabilityState::Emulated, _) => {
                    return Err(CapabilitySnapshotError::MissingEmulationPolicy(
                        entry.capability,
                    ));
                }
                (_, Some(_)) => {
                    return Err(CapabilitySnapshotError::UnexpectedEmulationPolicy(
                        entry.capability,
                    ));
                }
                _ => {}
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilitySnapshotError {
    #[error(transparent)]
    Version(#[from] ContractVersionError),
    #[error("capability {0:?} appears more than once")]
    Duplicate(AgentCapability),
    #[error("emulated capability {0:?} requires an explicit policy")]
    MissingEmulationPolicy(AgentCapability),
    #[error("non-emulated capability {0:?} cannot carry an emulation policy")]
    UnexpectedEmulationPolicy(AgentCapability),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ProviderSessionReference {
    pub schema_version: u16,
    pub provider_id: String,
    pub runtime_profile_id: String,
    pub provider_session_id: String,
    pub observed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl ProviderSessionReference {
    pub fn validate_current(&self) -> Result<(), ContractVersionError> {
        validate_current_version(
            "provider_session_reference.schema",
            self.schema_version,
            PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CanonicalMessage {
    pub message_id: Uuid,
    pub role: AgentRuntimeMessageRole,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AgentRunRequestEnvelope {
    pub schema_version: u16,
    pub payload_version: u16,
    pub request_id: Uuid,
    pub idempotency_key: String,
    pub session_id: Uuid,
    pub agent_run_id: Uuid,
    pub turn_id: Uuid,
    pub correlation_id: Uuid,
    pub intent: AgentRunIntent,
    pub runtime_profile_id: String,
    pub provider_id: String,
    pub workspace: WorkspaceReference,
    pub input: CanonicalMessage,
    pub created_at: DateTime<Utc>,
}

impl AgentRunRequestEnvelope {
    pub fn validate_current(&self) -> Result<(), ContractVersionError> {
        validate_current_version(
            "agent_run_request.schema",
            self.schema_version,
            AGENT_REQUEST_SCHEMA_VERSION,
        )?;
        validate_current_version(
            "agent_run_request.payload",
            self.payload_version,
            AGENT_REQUEST_PAYLOAD_VERSION,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum AgentTransportKind {
    StdioCli,
    StdioRpc,
    Acp,
    AppServerJsonrpc,
    HttpSidecar,
    InProcess,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct RunAttemptRequest {
    pub schema_version: u16,
    pub payload_version: u16,
    pub request_id: Uuid,
    pub idempotency_key: String,
    pub session_id: Uuid,
    pub agent_run_id: Uuid,
    pub turn_id: Uuid,
    pub run_attempt_id: Uuid,
    pub attempt_number: u32,
    pub correlation_id: Uuid,
    pub mode: RunAttemptMode,
    pub transport: AgentTransportKind,
    pub runtime_profile_id: String,
    pub provider_id: String,
    pub workspace: WorkspaceReference,
    pub capability_snapshot: CapabilitySnapshot,
    pub executor_config: ExecutorConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_skills: Option<Vec<SelectedSkill>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_to_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session: Option<ProviderSessionReference>,
    pub created_at: DateTime<Utc>,
}

impl RunAttemptRequest {
    pub fn validate_for_run(
        &self,
        request: &AgentRunRequestEnvelope,
    ) -> Result<(), RunAttemptRequestError> {
        self.validate_current()?;
        request.validate_current()?;
        self.capability_snapshot.validate()?;

        if self.session_id != request.session_id
            || self.agent_run_id != request.agent_run_id
            || self.turn_id != request.turn_id
            || self.correlation_id != request.correlation_id
        {
            return Err(RunAttemptRequestError::IdentityMismatch);
        }
        if self.runtime_profile_id != request.runtime_profile_id
            || self.provider_id != request.provider_id
            || self.workspace != request.workspace
            || self.executor_config.profile_id().cache_key() != self.runtime_profile_id
            || self
                .executor_config
                .executor
                .to_string()
                .to_ascii_lowercase()
                != self.provider_id
            || self.capability_snapshot.runtime_profile_id != self.runtime_profile_id
            || self.capability_snapshot.provider_id != self.provider_id
        {
            return Err(RunAttemptRequestError::ExecutionContextMismatch);
        }
        if self
            .selected_skills
            .as_ref()
            .is_some_and(|selected_skills| selected_skills.is_empty())
        {
            return Err(RunAttemptRequestError::EmptySelectedSkills);
        }
        if self.selected_skills.is_some() && request.intent == AgentRunIntent::Review {
            return Err(RunAttemptRequestError::SelectedSkillsOutsideCodingRun);
        }
        if self.reset_to_message_id.is_some() && request.intent != AgentRunIntent::FollowUp {
            return Err(RunAttemptRequestError::ResetOutsideFollowUp);
        }
        if let Some(provider_session) = &self.provider_session {
            provider_session.validate_current()?;
            if !runtime_profile_ids_match(
                &provider_session.runtime_profile_id,
                &self.runtime_profile_id,
            ) || provider_session.provider_id != self.provider_id
            {
                return Err(RunAttemptRequestError::ExecutionContextMismatch);
            }
        }

        Ok(())
    }

    pub fn validate_current(&self) -> Result<(), ContractVersionError> {
        validate_current_version(
            "run_attempt_request.schema",
            self.schema_version,
            AGENT_REQUEST_SCHEMA_VERSION,
        )?;
        validate_current_version(
            "run_attempt_request.payload",
            self.payload_version,
            AGENT_REQUEST_PAYLOAD_VERSION,
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunAttemptRequestError {
    #[error(transparent)]
    Version(#[from] ContractVersionError),
    #[error(transparent)]
    Capability(#[from] CapabilitySnapshotError),
    #[error("run attempt identity does not match its agent run request")]
    IdentityMismatch,
    #[error("run attempt execution context does not match its agent run request")]
    ExecutionContextMismatch,
    #[error("run attempt selected skills must be absent instead of empty")]
    EmptySelectedSkills,
    #[error("run attempt selected skills are only valid for initial and follow-up coding runs")]
    SelectedSkillsOutsideCodingRun,
    #[error("run attempt reset target is only valid for a follow-up")]
    ResetOutsideFollowUp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct NativeAuditReference {
    pub stream_id: Uuid,
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
}

/// A confirmed provider file operation, never an inferred directory difference.
/// Native paths are resolved against the captured Agent working directory by
/// the project projection; these are not public download targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AgentFileChange {
    pub path: String,
    pub change_type: AgentFileChangeType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileChangeType {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AgentEventPayload {
    LifecycleChanged {
        status: AgentRunStatus,
    },
    SessionObserved {
        provider_session: ProviderSessionReference,
    },
    Message {
        message: CanonicalMessage,
        final_output: bool,
    },
    Thinking {
        content: String,
    },
    /// Emitted only for completed native file operations with an explicit kind.
    /// Does not imply that other writes made by this Agent were observed.
    FileChanges {
        tool_call_id: String,
        changes: Vec<AgentFileChange>,
    },
    ToolCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        tool_name: String,
        status: AgentRuntimeToolStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arguments: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
    },
    ApprovalRequested {
        approval_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        tool_name: String,
    },
    ApprovalResolved {
        approval_id: String,
        approved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    InputRequested {
        input_id: String,
        prompt: String,
    },
    InputResolved {
        input_id: String,
        answered: bool,
    },
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cached_input_tokens: Option<u64>,
    },
    Error {
        error: AgentRuntimeError,
    },
    ProjectionDegraded {
        reason: String,
    },
    ProviderExtension {
        provider_namespace: String,
        provider_event: String,
        payload: serde_json::Value,
    },
    Unknown {
        event_type: String,
        payload: serde_json::Value,
    },
}

impl AgentEventPayload {
    pub fn upcast(
        payload_version: u16,
        payload: serde_json::Value,
    ) -> Result<Self, ContractDecodeError> {
        if payload_version < AGENT_EVENT_PAYLOAD_VERSION {
            return Err(ContractDecodeError::MissingUpcaster {
                contract: "agent_event.payload",
                from: payload_version,
                to: AGENT_EVENT_PAYLOAD_VERSION,
            });
        }

        if payload_version > AGENT_EVENT_PAYLOAD_VERSION {
            return Ok(Self::Unknown {
                event_type: payload_type(&payload),
                payload,
            });
        }

        let event_type = payload_type(&payload);
        if !KNOWN_AGENT_EVENT_TYPES.contains(&event_type.as_str()) {
            return Ok(Self::Unknown {
                event_type,
                payload,
            });
        }

        serde_json::from_value(payload).map_err(ContractDecodeError::MalformedCurrentPayload)
    }
}

const KNOWN_AGENT_EVENT_TYPES: &[&str] = &[
    "lifecycle_changed",
    "session_observed",
    "message",
    "thinking",
    "file_changes",
    "tool_call",
    "approval_requested",
    "approval_resolved",
    "input_requested",
    "input_resolved",
    "token_usage",
    "error",
    "projection_degraded",
    "provider_extension",
    "unknown",
];

fn payload_type(payload: &serde_json::Value) -> String {
    payload
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AgentEventEnvelope {
    pub schema_version: u16,
    pub payload_version: u16,
    pub event_id: Uuid,
    pub session_id: Uuid,
    pub agent_run_id: Uuid,
    pub turn_id: Uuid,
    pub run_attempt_id: Uuid,
    pub run_attempt_number: u32,
    pub sequence: u64,
    pub correlation_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestration_run_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestration_node_execution_id: Option<Uuid>,
    pub timestamp: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_refs: Vec<NativeAuditReference>,
    pub payload: AgentEventPayload,
}

impl AgentEventEnvelope {
    pub fn validate_for_projection(&self) -> Result<(), ContractVersionError> {
        validate_current_version(
            "agent_event.schema",
            self.schema_version,
            AGENT_EVENT_SCHEMA_VERSION,
        )?;
        match self.payload_version.cmp(&AGENT_EVENT_PAYLOAD_VERSION) {
            std::cmp::Ordering::Less => Err(ContractVersionError::MissingUpcaster {
                contract: "agent_event.payload",
                provided: self.payload_version,
                current: AGENT_EVENT_PAYLOAD_VERSION,
            }),
            std::cmp::Ordering::Greater
                if !matches!(&self.payload, AgentEventPayload::Unknown { .. }) =>
            {
                Err(ContractVersionError::UnsupportedFutureVersion {
                    contract: "agent_event.payload",
                    provided: self.payload_version,
                    current: AGENT_EVENT_PAYLOAD_VERSION,
                })
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize, TS)]
#[sqlx(type_name = "agent_run_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum AgentRunStatus {
    Pending,
    Starting,
    Running,
    AwaitingInput,
    AwaitingApproval,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    Crashed,
    AuditFailed,
}

impl AgentRunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Crashed | Self::AuditFailed
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize, TS)]
#[sqlx(type_name = "projection_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum ProjectionStatus {
    Current,
    ProjectionDegraded,
    Rebuilding,
}

/// Values reported by the native runtime, distinct from the frozen request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ObservedRuntimeConfiguration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_id: Option<String>,
    pub source: String,
    pub event_id: Uuid,
    pub run_attempt_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct RunState {
    pub state_schema_version: u16,
    pub reducer_version: u16,
    pub session_id: Uuid,
    pub agent_run_id: Uuid,
    pub turn_id: Uuid,
    pub status: AgentRunStatus,
    pub projection_status: ProjectionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_attempt_id: Option<Uuid>,
    pub last_run_attempt_number: u32,
    pub last_event_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session: Option<ProviderSessionReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_output: Option<CanonicalMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<AgentRuntimeError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_configuration: Option<ObservedRuntimeConfiguration>,
    pub unknown_event_count: u64,
    pub updated_at: DateTime<Utc>,
}

impl RunState {
    pub fn pending(request: &AgentRunRequestEnvelope) -> Self {
        Self {
            state_schema_version: RUN_STATE_SCHEMA_VERSION,
            reducer_version: RUN_STATE_REDUCER_VERSION,
            session_id: request.session_id,
            agent_run_id: request.agent_run_id,
            turn_id: request.turn_id,
            status: AgentRunStatus::Pending,
            projection_status: ProjectionStatus::Current,
            last_run_attempt_id: None,
            last_run_attempt_number: 0,
            last_event_sequence: 0,
            last_event_id: None,
            provider_session: None,
            terminal_output: None,
            last_error: None,
            observed_configuration: None,
            unknown_event_count: 0,
            updated_at: request.created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContractVersionError {
    #[error(
        "{contract} version {provided} is older than {current}; an explicit upcaster is required"
    )]
    MissingUpcaster {
        contract: &'static str,
        provided: u16,
        current: u16,
    },
    #[error("{contract} version {provided} is newer than supported version {current}")]
    UnsupportedFutureVersion {
        contract: &'static str,
        provided: u16,
        current: u16,
    },
}

pub(crate) fn validate_current_version(
    contract: &'static str,
    provided: u16,
    current: u16,
) -> Result<(), ContractVersionError> {
    match provided.cmp(&current) {
        std::cmp::Ordering::Less => Err(ContractVersionError::MissingUpcaster {
            contract,
            provided,
            current,
        }),
        std::cmp::Ordering::Greater => Err(ContractVersionError::UnsupportedFutureVersion {
            contract,
            provided,
            current,
        }),
        std::cmp::Ordering::Equal => Ok(()),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContractDecodeError {
    #[error("{contract} version {from} requires an explicit upcaster to version {to}")]
    MissingUpcaster {
        contract: &'static str,
        from: u16,
        to: u16,
    },
    #[error("current event payload is malformed: {0}")]
    MalformedCurrentPayload(serde_json::Error),
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::executors::BaseCodingAgent;

    fn run_attempt_requests(
        intent: AgentRunIntent,
    ) -> (AgentRunRequestEnvelope, RunAttemptRequest) {
        let now = Utc::now();
        let workspace = WorkspaceReference {
            workspace_id: Uuid::new_v4(),
            mode: WorkspaceMode::SharedWorkspace,
            path: "C:/workspace".to_string(),
        };
        let executor_config = ExecutorConfig {
            executor: BaseCodingAgent::Codex,
            variant: Some("default".to_string()),
            model_id: Some(Some("openai/gpt-5.6-codex".to_string())),
            agent_id: None,
            reasoning_id: Some(Some("high".to_string())),
            permission_policy: None,
            provider_options: None,
        };
        let runtime_profile_id = executor_config.profile_id().cache_key();
        let request = AgentRunRequestEnvelope {
            schema_version: AGENT_REQUEST_SCHEMA_VERSION,
            payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
            request_id: Uuid::new_v4(),
            idempotency_key: "contract-test".to_string(),
            session_id: Uuid::new_v4(),
            agent_run_id: Uuid::new_v4(),
            turn_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            intent,
            runtime_profile_id: runtime_profile_id.clone(),
            provider_id: "codex".to_string(),
            workspace: workspace.clone(),
            input: CanonicalMessage {
                message_id: Uuid::new_v4(),
                role: AgentRuntimeMessageRole::User,
                content: "Implement the runtime contract".to_string(),
            },
            created_at: now,
        };
        let attempt = RunAttemptRequest {
            schema_version: AGENT_REQUEST_SCHEMA_VERSION,
            payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
            request_id: Uuid::new_v4(),
            idempotency_key: "contract-test:attempt:1".to_string(),
            session_id: request.session_id,
            agent_run_id: request.agent_run_id,
            turn_id: request.turn_id,
            run_attempt_id: Uuid::new_v4(),
            attempt_number: 1,
            correlation_id: request.correlation_id,
            mode: RunAttemptMode::Launch,
            transport: AgentTransportKind::AppServerJsonrpc,
            runtime_profile_id: runtime_profile_id.clone(),
            provider_id: request.provider_id.clone(),
            workspace,
            capability_snapshot: CapabilitySnapshot {
                schema_version: CAPABILITY_SNAPSHOT_SCHEMA_VERSION,
                runtime_profile_id,
                provider_id: request.provider_id.clone(),
                runtime_version: Some("1.0.0".to_string()),
                protocol_version: Some("app-server-v2".to_string()),
                adapter_version: "1".to_string(),
                resolved_at: now,
                capabilities: Vec::new(),
            },
            executor_config,
            selected_skills: Some(vec![SelectedSkill {
                name: "runtime-contracts".to_string(),
                path: PathBuf::from("C:/skills/runtime-contracts/SKILL.md"),
            }]),
            reset_to_message_id: None,
            provider_session: None,
            created_at: now,
        };
        (request, attempt)
    }

    #[test]
    fn resumed_default_alias_preserves_native_session_and_rejects_other_profiles() {
        let (mut request, mut attempt) = run_attempt_requests(AgentRunIntent::FollowUp);
        request.runtime_profile_id = "CODEX".to_string();
        attempt.runtime_profile_id = "CODEX".to_string();
        attempt.capability_snapshot.runtime_profile_id = "CODEX".to_string();
        attempt.executor_config.variant = None;
        attempt.provider_session = Some(ProviderSessionReference {
            schema_version: PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION,
            provider_id: "codex".to_string(),
            runtime_profile_id: "CODEX:DEFAULT".to_string(),
            provider_session_id: "native-session".to_string(),
            observed_at: Utc::now(),
            metadata: None,
        });
        assert!(attempt.validate_for_run(&request).is_ok());
        attempt
            .provider_session
            .as_mut()
            .unwrap()
            .runtime_profile_id = "CODEX:PLAN".to_string();
        assert!(matches!(
            attempt.validate_for_run(&request),
            Err(RunAttemptRequestError::ExecutionContextMismatch)
        ));
    }

    #[test]
    fn run_attempt_freezes_and_round_trips_provider_launch_spec() {
        let (request, attempt) = run_attempt_requests(AgentRunIntent::Initial);

        attempt
            .validate_for_run(&request)
            .expect("coherent frozen launch spec should validate");
        let round_trip: RunAttemptRequest = serde_json::from_value(
            serde_json::to_value(&attempt).expect("launch spec should serialize"),
        )
        .expect("launch spec should deserialize");

        assert_eq!(round_trip, attempt);
        assert_eq!(
            round_trip
                .executor_config
                .model_id
                .as_ref()
                .and_then(|value| value.as_deref()),
            Some("openai/gpt-5.6-codex")
        );
        assert_eq!(
            round_trip
                .executor_config
                .reasoning_id
                .as_ref()
                .and_then(|value| value.as_deref()),
            Some("high")
        );
        assert_eq!(round_trip.selected_skills, attempt.selected_skills);
    }

    #[test]
    fn run_attempt_rejects_launch_spec_drift() {
        let (request, mut attempt) = run_attempt_requests(AgentRunIntent::Initial);
        attempt.executor_config.executor = BaseCodingAgent::Gemini;

        assert!(matches!(
            attempt.validate_for_run(&request),
            Err(RunAttemptRequestError::ExecutionContextMismatch)
        ));

        let (empty_skills_request, mut empty_skills) =
            run_attempt_requests(AgentRunIntent::Initial);
        empty_skills.selected_skills = Some(Vec::new());
        assert!(matches!(
            empty_skills.validate_for_run(&empty_skills_request),
            Err(RunAttemptRequestError::EmptySelectedSkills)
        ));

        let (request, mut reset) = run_attempt_requests(AgentRunIntent::Initial);
        reset.reset_to_message_id = Some("message-1".to_string());
        assert!(matches!(
            reset.validate_for_run(&request),
            Err(RunAttemptRequestError::ResetOutsideFollowUp)
        ));

        let (request, attempt) = run_attempt_requests(AgentRunIntent::Review);
        assert!(matches!(
            attempt.validate_for_run(&request),
            Err(RunAttemptRequestError::SelectedSkillsOutsideCodingRun)
        ));
    }

    #[test]
    fn capability_snapshot_requires_explicit_emulation_policy() {
        let snapshot = CapabilitySnapshot {
            schema_version: 1,
            runtime_profile_id: "codex:default".to_string(),
            provider_id: "codex".to_string(),
            runtime_version: Some("1.0.0".to_string()),
            protocol_version: Some("app-server-v2".to_string()),
            adapter_version: "1".to_string(),
            resolved_at: Utc::now(),
            capabilities: vec![CapabilitySnapshotEntry {
                capability: AgentCapability::SessionResume,
                state: CapabilityState::Emulated,
                source: CapabilitySource::RunPolicy,
                emulation_policy: None,
                evidence: None,
            }],
        };

        assert_eq!(
            snapshot.validate(),
            Err(CapabilitySnapshotError::MissingEmulationPolicy(
                AgentCapability::SessionResume
            ))
        );
    }

    #[test]
    fn missing_capability_is_unknown_instead_of_enabled() {
        let snapshot = CapabilitySnapshot {
            schema_version: 1,
            runtime_profile_id: "claude-code:default".to_string(),
            provider_id: "claude-code".to_string(),
            runtime_version: None,
            protocol_version: None,
            adapter_version: "1".to_string(),
            resolved_at: Utc::now(),
            capabilities: Vec::new(),
        };

        assert_eq!(
            snapshot.resolve(AgentCapability::SessionResume),
            CapabilityState::Unknown
        );
    }

    #[test]
    fn capability_snapshot_requires_current_schema() {
        let snapshot = CapabilitySnapshot {
            schema_version: 0,
            runtime_profile_id: "codex:default".to_string(),
            provider_id: "codex".to_string(),
            runtime_version: None,
            protocol_version: None,
            adapter_version: "1".to_string(),
            resolved_at: Utc::now(),
            capabilities: Vec::new(),
        };

        assert!(matches!(
            snapshot.validate(),
            Err(CapabilitySnapshotError::Version(
                ContractVersionError::MissingUpcaster {
                    contract: "capability_snapshot.schema",
                    ..
                }
            ))
        ));
    }

    #[test]
    fn unknown_current_payload_is_preserved() {
        let raw = json!({
            "type": "codex_future_event",
            "data": { "nested": true }
        });

        let decoded = AgentEventPayload::upcast(AGENT_EVENT_PAYLOAD_VERSION, raw.clone())
            .expect("unknown event should be preserved");

        assert_eq!(
            decoded,
            AgentEventPayload::Unknown {
                event_type: "codex_future_event".to_string(),
                payload: raw
            }
        );
    }

    #[test]
    fn future_payload_version_degrades_to_unknown() {
        let raw = json!({ "type": "message", "data": { "new_shape": true } });

        let decoded = AgentEventPayload::upcast(AGENT_EVENT_PAYLOAD_VERSION + 1, raw.clone())
            .expect("future payload should remain replayable as unknown");

        assert_eq!(
            decoded,
            AgentEventPayload::Unknown {
                event_type: "message".to_string(),
                payload: raw
            }
        );
    }

    #[test]
    fn historical_payload_requires_registered_upcaster() {
        let error = AgentEventPayload::upcast(0, json!({ "type": "message" }))
            .expect_err("clean v1 has no historical canonical upcaster");

        assert!(matches!(
            error,
            ContractDecodeError::MissingUpcaster {
                contract: "agent_event.payload",
                from: 0,
                to: AGENT_EVENT_PAYLOAD_VERSION
            }
        ));
    }

    #[test]
    fn event_envelope_versions_serialize_independently() {
        let value = serde_json::to_value(AgentEventEnvelope {
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload_version: AGENT_EVENT_PAYLOAD_VERSION,
            event_id: Uuid::nil(),
            session_id: Uuid::nil(),
            agent_run_id: Uuid::nil(),
            turn_id: Uuid::nil(),
            run_attempt_id: Uuid::nil(),
            run_attempt_number: 1,
            sequence: 1,
            correlation_id: Uuid::nil(),
            orchestration_run_id: None,
            orchestration_node_execution_id: None,
            timestamp: Utc::now(),
            native_refs: Vec::new(),
            payload: AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::AwaitingApproval,
            },
        })
        .expect("event should serialize");

        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["payload_version"], 1);
        assert_eq!(value["payload"]["type"], "lifecycle_changed");
        assert_eq!(value["payload"]["data"]["status"], "awaiting_approval");
    }

    #[test]
    fn future_unknown_event_is_valid_for_degraded_projection() {
        let event = AgentEventEnvelope {
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload_version: AGENT_EVENT_PAYLOAD_VERSION + 1,
            event_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            agent_run_id: Uuid::new_v4(),
            turn_id: Uuid::new_v4(),
            run_attempt_id: Uuid::new_v4(),
            run_attempt_number: 1,
            sequence: 1,
            correlation_id: Uuid::new_v4(),
            orchestration_run_id: None,
            orchestration_node_execution_id: None,
            timestamp: Utc::now(),
            native_refs: Vec::new(),
            payload: AgentEventPayload::Unknown {
                event_type: "future_event".to_string(),
                payload: json!({ "type": "future_event", "data": {} }),
            },
        };

        event
            .validate_for_projection()
            .expect("future unknown payload must remain replayable");
    }

    #[test]
    fn future_known_event_payload_is_rejected_before_projection() {
        let event = AgentEventEnvelope {
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload_version: AGENT_EVENT_PAYLOAD_VERSION + 1,
            event_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            agent_run_id: Uuid::new_v4(),
            turn_id: Uuid::new_v4(),
            run_attempt_id: Uuid::new_v4(),
            run_attempt_number: 1,
            sequence: 1,
            correlation_id: Uuid::new_v4(),
            orchestration_run_id: None,
            orchestration_node_execution_id: None,
            timestamp: Utc::now(),
            native_refs: Vec::new(),
            payload: AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Running,
            },
        };

        assert!(matches!(
            event.validate_for_projection(),
            Err(ContractVersionError::UnsupportedFutureVersion {
                contract: "agent_event.payload",
                ..
            })
        ));
    }

    #[test]
    fn run_state_serializes_degraded_projection_explicitly() {
        let value = serde_json::to_value(ProjectionStatus::ProjectionDegraded)
            .expect("projection status should serialize");

        assert_eq!(value, json!("projection_degraded"));
    }
}
