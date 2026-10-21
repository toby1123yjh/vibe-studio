//! Provider-independent, replayable conversation facts. These are observations,
//! never instructions to execute a tool or answer an old native request.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use super::{AgentRuntimeMessageRole, AgentRuntimeToolStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ConversationBlockKind {
    Text,
    Reasoning,
    Tool,
    Plan,
    Interaction,
    Notice,
    Resource,
    Extension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ConversationTextField {
    Text,
    OutputText,
    RawArguments,
}

impl ConversationTextField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::OutputText => "output_text",
            Self::RawArguments => "raw_arguments",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ConversationReasoningSource {
    Summary,
    ExposedText,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConversationLocation {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConversationPlanEntry {
    pub content: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ConversationInteractionKind {
    Permission,
    Question,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConversationInteractionOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConversationQuestion {
    pub id: String,
    pub prompt: String,
    #[serde(default)]
    pub options: Vec<ConversationInteractionOption>,
    #[serde(default)]
    pub multiple: bool,
    #[serde(default)]
    pub allow_free_text: bool,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ConversationInteractionStatus {
    Pending,
    Delivering,
    Delivered,
    Resolved,
    DeliveryUnknown,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConversationInteraction {
    pub request_id: String,
    pub kind: ConversationInteractionKind,
    pub status: ConversationInteractionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_request_id: Option<Value>,
    #[serde(default)]
    pub options: Vec<ConversationInteractionOption>,
    #[serde(default)]
    pub questions: Vec<ConversationQuestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Value>,
    /// False for preserved historical/unsupported requests, even if their
    /// original native options are available for inspection.
    pub actionable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ConversationQuestionAnswer {
    pub question_id: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ConversationInteractionReply {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    #[serde(default)]
    pub answers: Vec<ConversationQuestionAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Flat fields have explicit ownership. JSON values/arrays replace one field;
/// they are never recursively merged or concatenated. None means omitted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct ConversationBlockFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_arguments: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AgentRuntimeToolStatus>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_json"
    )]
    pub arguments: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_json"
    )]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<ConversationLocation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_source: Option<ConversationReasoningSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_entries: Option<Vec<ConversationPlanEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction: Option<ConversationInteraction>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_json"
    )]
    pub resource: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_json"
    )]
    pub extension: Option<Value>,
}

fn present_json<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ConversationBlockOperation {
    Append {
        field: ConversationTextField,
        text: String,
    },
    /// Replace the explicitly provided fields, retaining omitted fields.
    Snapshot { fields: ConversationBlockFields },
    Patch {
        fields: ConversationBlockFields,
        /// Explicit top-level field removal; missing fields never mean clear.
        #[serde(default)]
        clear_fields: Vec<String>,
    },
    Finish {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConversationBlockUpdate {
    /// Attempt-scoped deterministic identity. Text blocks use their canonical
    /// message UUID, allowing final-output facts to reference an existing body.
    pub block_id: String,
    pub kind: ConversationBlockKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<AgentRuntimeMessageRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_block_id: Option<String>,
    pub operation: ConversationBlockOperation,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn partial_tool_patch_does_not_clear_omitted_fields() {
        let patch = ConversationBlockOperation::Patch {
            fields: ConversationBlockFields {
                status: Some(AgentRuntimeToolStatus::Succeeded),
                result: Some(Value::Null),
                ..Default::default()
            },
            clear_fields: vec!["raw_arguments".into()],
        };
        let wire = serde_json::to_value(&patch).unwrap();
        assert_eq!(wire["op"], "patch");
        assert!(wire["fields"].get("tool_name").is_none());
        assert_eq!(wire["fields"]["result"], Value::Null);
        assert_eq!(wire["clear_fields"], json!(["raw_arguments"]));
        let restored: ConversationBlockOperation = serde_json::from_value(wire).unwrap();
        assert_eq!(restored, patch);
    }
}
