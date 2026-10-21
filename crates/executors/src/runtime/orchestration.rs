use std::pin::Pin;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::Stream;
use serde::{Deserialize, Serialize};
use sqlx::Type;
use ts_rs::TS;
use uuid::Uuid;

use super::{
    AgentRunRequestEnvelope, AgentRunStatus, CanonicalMessage, ContractDecodeError,
    ContractVersionError, ProjectionStatus, RunAttemptMode, RunAttemptRequest,
    RunAttemptRequestError, WorkspaceMode, validate_current_version,
};

pub const ORCHESTRATION_PLAN_SCHEMA_VERSION: u16 = 1;
pub const ORCHESTRATION_COMMAND_SCHEMA_VERSION: u16 = 1;
pub const ORCHESTRATION_EVENT_SCHEMA_VERSION: u16 = 1;
pub const ORCHESTRATION_EVENT_PAYLOAD_VERSION: u16 = 1;
pub const ORCHESTRATION_STATE_SCHEMA_VERSION: u16 = 1;
pub const ORCHESTRATION_REDUCER_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum OrchestrationProductKind {
    Workflow,
    Arena,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum OrchestrationFailurePolicy {
    FailFast,
    AllowPartial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum OrchestrationJoinPolicy {
    All,
    Any,
    Each,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum RemainingUpstreamsPolicy {
    Continue,
    CancelRemaining,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum EachDownstreamExecution {
    Parallel,
    Serial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum RetryBackoffKind {
    None,
    Fixed,
    Exponential,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct OrchestrationRetryPolicy {
    pub max_run_attempts: u32,
    pub backoff: RetryBackoffKind,
    pub backoff_ms: u64,
    pub retryable_terminal_statuses: Vec<AgentRunStatus>,
    pub mode: RunAttemptMode,
}

impl Default for OrchestrationRetryPolicy {
    fn default() -> Self {
        Self {
            max_run_attempts: 1,
            backoff: RetryBackoffKind::None,
            backoff_ms: 0,
            retryable_terminal_statuses: Vec::new(),
            mode: RunAttemptMode::Restart,
        }
    }
}

impl OrchestrationRetryPolicy {
    pub fn is_automatic_retry_enabled(&self) -> bool {
        self.max_run_attempts > 1
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct OrchestrationPlanNode {
    pub node_key: String,
    pub stable_order: u32,
    pub dependencies: Vec<String>,
    pub join: OrchestrationJoinPolicy,
    pub failure_policy: OrchestrationFailurePolicy,
    pub remaining_upstreams: RemainingUpstreamsPolicy,
    pub each_downstream_execution: EachDownstreamExecution,
    pub retry: OrchestrationRetryPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct OrchestrationPlanSnapshot {
    pub schema_version: u16,
    pub plan_id: Uuid,
    pub source_definition_id: Uuid,
    pub source_definition_version: String,
    pub product_kind: OrchestrationProductKind,
    pub workspace_mode: WorkspaceMode,
    pub nodes: Vec<OrchestrationPlanNode>,
    pub created_at: DateTime<Utc>,
}

impl OrchestrationPlanSnapshot {
    pub fn validate_current(&self) -> Result<(), ContractVersionError> {
        validate_current_version(
            "orchestration_plan.schema",
            self.schema_version,
            ORCHESTRATION_PLAN_SCHEMA_VERSION,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct UpstreamSourceReference {
    pub orchestration_run_id: Uuid,
    pub node_execution_id: Uuid,
    pub agent_run_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct UpstreamHandoff {
    pub source_ref: UpstreamSourceReference,
    pub initiating_input: CanonicalMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_output: Option<CanonicalMessage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AgentRunPortCommandEnvelope {
    pub schema_version: u16,
    pub command_id: Uuid,
    pub idempotency_key: String,
    pub agent_run_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestration_run_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestration_node_execution_id: Option<Uuid>,
    pub correlation_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub command: AgentRunPortCommand,
}

impl AgentRunPortCommandEnvelope {
    pub fn validate_current(&self) -> Result<(), OrchestrationCommandValidationError> {
        validate_current_version(
            "orchestration_command.schema",
            self.schema_version,
            ORCHESTRATION_COMMAND_SCHEMA_VERSION,
        )?;
        if self.orchestration_run_id.is_some() != self.orchestration_node_execution_id.is_some() {
            return Err(OrchestrationCommandValidationError::IncompleteOrchestrationOrigin);
        }
        if let AgentRunPortCommand::Create { request, attempt } = &self.command {
            attempt.validate_for_run(request)?;
            if request.agent_run_id != self.agent_run_id {
                return Err(OrchestrationCommandValidationError::AgentRunIdentityMismatch);
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OrchestrationCommandValidationError {
    #[error(transparent)]
    Version(#[from] ContractVersionError),
    #[error(transparent)]
    InvalidAttempt(#[from] RunAttemptRequestError),
    #[error("orchestration command origin requires both run and node execution identities")]
    IncompleteOrchestrationOrigin,
    #[error("command AgentRun identity does not match its create request")]
    AgentRunIdentityMismatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum AgentRunPortCommand {
    Create {
        request: AgentRunRequestEnvelope,
        attempt: RunAttemptRequest,
    },
    Cancel {
        reason: String,
    },
    SubmitInput {
        input_id: String,
        content: String,
    },
    ReplyInteraction {
        run_attempt_id: Uuid,
        reply_id: Uuid,
        reply: super::ConversationInteractionReply,
    },
    ResolveApproval {
        approval_id: String,
        approved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Retry {
        mode: RunAttemptMode,
        run_attempt_id: Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AgentRunPortSnapshot {
    pub agent_run_id: Uuid,
    pub state: super::RunState,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentRunPortError {
    #[error("agent run {0} was not found")]
    NotFound(Uuid),
    #[error("agent run port is unavailable: {0}")]
    Unavailable(String),
    #[error("agent run command was rejected: {0}")]
    Rejected(String),
}

pub type AgentEventStream = Pin<Box<dyn Stream<Item = super::AgentEventEnvelope> + Send>>;

/// Narrow orchestration boundary for managed AgentRuns.
///
/// Implementations own provider adapters and process supervision. Callers
/// receive only canonical state/events and durable command outcomes.
#[async_trait]
pub trait AgentRunPort: Send + Sync {
    async fn create(
        &self,
        request: AgentRunRequestEnvelope,
        attempt: RunAttemptRequest,
    ) -> Result<Uuid, AgentRunPortError>;

    async fn query(&self, agent_run_id: Uuid) -> Result<AgentRunPortSnapshot, AgentRunPortError>;

    async fn control(&self, command: AgentRunPortCommandEnvelope) -> Result<(), AgentRunPortError>;

    async fn subscribe(&self, agent_run_id: Uuid) -> Result<AgentEventStream, AgentRunPortError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize, TS)]
#[sqlx(type_name = "orchestration_run_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum OrchestrationRunStatus {
    Pending,
    Running,
    WaitingForInput,
    WaitingForApproval,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
}

impl OrchestrationRunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize, TS)]
#[sqlx(type_name = "orchestration_node_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum OrchestrationNodeStatus {
    Pending,
    Ready,
    Running,
    AwaitingInput,
    AwaitingApproval,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum OrchestrationEventPayload {
    LifecycleChanged {
        status: OrchestrationRunStatus,
    },
    NodeStatusChanged {
        node_execution_id: Uuid,
        status: OrchestrationNodeStatus,
    },
    AgentRunLinked {
        node_execution_id: Uuid,
        agent_run_id: Uuid,
    },
    JoinDecided {
        node_execution_id: Uuid,
        policy: OrchestrationJoinPolicy,
        consumed_source_execution_ids: Vec<Uuid>,
    },
    CommandQueued {
        command_id: Uuid,
        idempotency_key: String,
    },
    ProjectionDegraded {
        reason: String,
    },
    Unknown {
        event_type: String,
        payload: serde_json::Value,
    },
}

impl OrchestrationEventPayload {
    pub fn upcast(
        payload_version: u16,
        payload: serde_json::Value,
    ) -> Result<Self, ContractDecodeError> {
        if payload_version < ORCHESTRATION_EVENT_PAYLOAD_VERSION {
            return Err(ContractDecodeError::MissingUpcaster {
                contract: "orchestration_event.payload",
                from: payload_version,
                to: ORCHESTRATION_EVENT_PAYLOAD_VERSION,
            });
        }
        if payload_version > ORCHESTRATION_EVENT_PAYLOAD_VERSION {
            return Ok(Self::Unknown {
                event_type: orchestration_payload_type(&payload),
                payload,
            });
        }

        let event_type = orchestration_payload_type(&payload);
        if !KNOWN_ORCHESTRATION_EVENT_TYPES.contains(&event_type.as_str()) {
            return Ok(Self::Unknown {
                event_type,
                payload,
            });
        }
        serde_json::from_value(payload).map_err(ContractDecodeError::MalformedCurrentPayload)
    }
}

const KNOWN_ORCHESTRATION_EVENT_TYPES: &[&str] = &[
    "lifecycle_changed",
    "node_status_changed",
    "agent_run_linked",
    "join_decided",
    "command_queued",
    "projection_degraded",
    "unknown",
];

fn orchestration_payload_type(payload: &serde_json::Value) -> String {
    payload
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct OrchestrationEventEnvelope {
    pub schema_version: u16,
    pub payload_version: u16,
    pub event_id: Uuid,
    pub orchestration_run_id: Uuid,
    pub sequence: u64,
    pub correlation_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub payload: OrchestrationEventPayload,
}

impl OrchestrationEventEnvelope {
    pub fn validate_for_projection(&self) -> Result<(), ContractVersionError> {
        validate_current_version(
            "orchestration_event.schema",
            self.schema_version,
            ORCHESTRATION_EVENT_SCHEMA_VERSION,
        )?;
        match self
            .payload_version
            .cmp(&ORCHESTRATION_EVENT_PAYLOAD_VERSION)
        {
            std::cmp::Ordering::Less => Err(ContractVersionError::MissingUpcaster {
                contract: "orchestration_event.payload",
                provided: self.payload_version,
                current: ORCHESTRATION_EVENT_PAYLOAD_VERSION,
            }),
            std::cmp::Ordering::Greater
                if !matches!(&self.payload, OrchestrationEventPayload::Unknown { .. }) =>
            {
                Err(ContractVersionError::UnsupportedFutureVersion {
                    contract: "orchestration_event.payload",
                    provided: self.payload_version,
                    current: ORCHESTRATION_EVENT_PAYLOAD_VERSION,
                })
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct OrchestrationState {
    pub state_schema_version: u16,
    pub reducer_version: u16,
    pub orchestration_run_id: Uuid,
    pub status: OrchestrationRunStatus,
    pub projection_status: ProjectionStatus,
    pub last_event_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_id: Option<Uuid>,
    pub unknown_event_count: u64,
    pub updated_at: DateTime<Utc>,
}

impl OrchestrationState {
    pub fn pending(orchestration_run_id: Uuid, created_at: DateTime<Utc>) -> Self {
        Self {
            state_schema_version: ORCHESTRATION_STATE_SCHEMA_VERSION,
            reducer_version: ORCHESTRATION_REDUCER_VERSION,
            orchestration_run_id,
            status: OrchestrationRunStatus::Pending,
            projection_status: ProjectionStatus::Current,
            last_event_sequence: 0,
            last_event_id: None,
            unknown_event_count: 0,
            updated_at: created_at,
        }
    }

    pub fn validate_current(&self) -> Result<(), ContractVersionError> {
        validate_current_version(
            "orchestration_state.schema",
            self.state_schema_version,
            ORCHESTRATION_STATE_SCHEMA_VERSION,
        )?;
        validate_current_version(
            "orchestration_state.reducer",
            self.reducer_version,
            ORCHESTRATION_REDUCER_VERSION,
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn automatic_retry_is_disabled_by_default() {
        let policy = OrchestrationRetryPolicy::default();

        assert_eq!(policy.max_run_attempts, 1);
        assert!(!policy.is_automatic_retry_enabled());
    }

    #[test]
    fn interaction_reply_requires_and_preserves_the_target_attempt() {
        let run_attempt_id = Uuid::new_v4();
        let command = AgentRunPortCommand::ReplyInteraction {
            run_attempt_id,
            reply_id: Uuid::new_v4(),
            reply: super::super::ConversationInteractionReply {
                request_id: "codex:0".into(),
                option_id: Some("accept".into()),
                answers: Vec::new(),
                text: None,
            },
        };
        let mut wire = serde_json::to_value(&command).unwrap();
        assert_eq!(wire["data"]["run_attempt_id"], json!(run_attempt_id));
        assert_eq!(
            serde_json::from_value::<AgentRunPortCommand>(wire.clone()).unwrap(),
            command
        );
        wire["data"]
            .as_object_mut()
            .unwrap()
            .remove("run_attempt_id");
        assert!(serde_json::from_value::<AgentRunPortCommand>(wire).is_err());
    }

    #[test]
    fn orchestration_contracts_use_explicit_cancelled_and_waiting_states() {
        assert_eq!(
            serde_json::to_value(OrchestrationRunStatus::Cancelled).unwrap(),
            json!("cancelled")
        );
        assert_eq!(
            serde_json::to_value(OrchestrationNodeStatus::AwaitingInput).unwrap(),
            json!("awaiting_input")
        );
        assert_eq!(
            serde_json::to_value(OrchestrationNodeStatus::AwaitingApproval).unwrap(),
            json!("awaiting_approval")
        );
    }

    #[test]
    fn orchestration_event_versions_serialize_independently() {
        let value = serde_json::to_value(OrchestrationEventEnvelope {
            schema_version: ORCHESTRATION_EVENT_SCHEMA_VERSION,
            payload_version: ORCHESTRATION_EVENT_PAYLOAD_VERSION,
            event_id: Uuid::nil(),
            orchestration_run_id: Uuid::nil(),
            sequence: 1,
            correlation_id: Uuid::nil(),
            timestamp: Utc::now(),
            payload: OrchestrationEventPayload::LifecycleChanged {
                status: OrchestrationRunStatus::WaitingForApproval,
            },
        })
        .expect("orchestration event should serialize");

        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["payload_version"], 1);
        assert_eq!(value["payload"]["type"], "lifecycle_changed");
        assert_eq!(value["payload"]["data"]["status"], "waiting_for_approval");
    }

    #[test]
    fn future_orchestration_payload_is_preserved_as_unknown() {
        let raw = json!({ "type": "future_join", "data": { "sources": [] } });
        let payload =
            OrchestrationEventPayload::upcast(ORCHESTRATION_EVENT_PAYLOAD_VERSION + 1, raw.clone())
                .expect("future payload must remain replayable");
        assert_eq!(
            payload,
            OrchestrationEventPayload::Unknown {
                event_type: "future_join".to_string(),
                payload: raw,
            }
        );
    }

    #[test]
    fn historical_orchestration_payload_requires_upcaster() {
        assert!(matches!(
            OrchestrationEventPayload::upcast(0, json!({ "type": "lifecycle_changed" })),
            Err(ContractDecodeError::MissingUpcaster {
                contract: "orchestration_event.payload",
                ..
            })
        ));
    }
}
