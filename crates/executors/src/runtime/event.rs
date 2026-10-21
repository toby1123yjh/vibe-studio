use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::runtime::{AgentRunLifecycle, AgentRuntimeError};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentRuntimeEvent {
    LifecycleChanged {
        lifecycle: AgentRunLifecycle,
    },
    SessionObserved {
        session_id: String,
    },
    Message {
        role: AgentRuntimeMessageRole,
        content: String,
    },
    Thinking {
        content: String,
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
        approval_id: String,
        prompt: String,
    },
    InputResolved {
        approval_id: String,
        answered: bool,
    },
    TokenUsage {
        total_tokens: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_context_window: Option<u32>,
    },
    Warning {
        message: String,
    },
    Error {
        error: AgentRuntimeError,
    },
    ProcessExited {
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum AgentRuntimeMessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum AgentRuntimeToolStatus {
    Created,
    Running,
    WaitingApproval,
    Approved,
    Denied,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn serializes_event_with_snake_case_type() {
        let encoded = serde_json::to_value(AgentRuntimeEvent::LifecycleChanged {
            lifecycle: AgentRunLifecycle::WaitingInput,
        })
        .expect("event should serialize");

        assert_eq!(
            encoded,
            json!({
                "type": "lifecycle_changed",
                "lifecycle": "waiting_input"
            })
        );
    }

    #[test]
    fn serializes_tool_status_as_snake_case() {
        let encoded = serde_json::to_value(AgentRuntimeEvent::ToolCall {
            tool_call_id: Some("tool-1".to_string()),
            tool_name: "Edit".to_string(),
            status: AgentRuntimeToolStatus::WaitingApproval,
            arguments: Some(json!({ "path": "src/lib.rs" })),
            result: None,
        })
        .expect("tool event should serialize");

        assert_eq!(
            encoded,
            json!({
                "type": "tool_call",
                "tool_call_id": "tool-1",
                "tool_name": "Edit",
                "status": "waiting_approval",
                "arguments": { "path": "src/lib.rs" }
            })
        );
    }
}
