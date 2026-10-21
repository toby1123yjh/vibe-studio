//! Decode app-server tool items at the native adapter boundary. The canonical
//! conversation must not depend on the separate legacy normalized-log parser.

use codex_app_server_protocol::{CommandExecutionStatus, ThreadItem};
use serde_json::{Value, json};

use super::TypedProviderEvent;
use crate::runtime::AgentRuntimeToolStatus;

pub(super) fn classify(payload: &Value) -> Option<TypedProviderEvent> {
    let completed = match payload.get("method")?.as_str()? {
        "item/started" => false,
        "item/completed" => true,
        _ => return None,
    };
    let ThreadItem::CommandExecution {
        id,
        command,
        cwd,
        status,
        aggregated_output,
        exit_code,
        ..
    } = serde_json::from_value(payload.get("params")?.get("item")?.clone()).ok()?
    else {
        return None;
    };
    if id.is_empty() {
        return None;
    }

    let status = match status {
        CommandExecutionStatus::InProgress => AgentRuntimeToolStatus::Running,
        CommandExecutionStatus::Completed if exit_code.is_some_and(|code| code != 0) => {
            AgentRuntimeToolStatus::Failed
        }
        CommandExecutionStatus::Completed => AgentRuntimeToolStatus::Succeeded,
        CommandExecutionStatus::Failed => AgentRuntimeToolStatus::Failed,
        CommandExecutionStatus::Declined => AgentRuntimeToolStatus::Denied,
    };
    Some(TypedProviderEvent::ToolCall {
        id: Some(id),
        name: "Shell".to_string(),
        status,
        arguments: Some(json!({ "command": command, "cwd": cwd })),
        result: completed.then(|| json!({ "output": aggregated_output, "exit_code": exit_code })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_app_server_commands_preserve_native_identity_status_and_output() {
        for (method, native_status, exit_code, expected) in [
            (
                "item/started",
                "inProgress",
                None,
                AgentRuntimeToolStatus::Running,
            ),
            (
                "item/completed",
                "completed",
                Some(0),
                AgentRuntimeToolStatus::Succeeded,
            ),
            (
                "item/completed",
                "completed",
                Some(1),
                AgentRuntimeToolStatus::Failed,
            ),
            (
                "item/completed",
                "failed",
                Some(1),
                AgentRuntimeToolStatus::Failed,
            ),
            (
                "item/completed",
                "declined",
                None,
                AgentRuntimeToolStatus::Denied,
            ),
        ] {
            let payload = json!({
                "method": method,
                "params": { "threadId": "thread-1", "turnId": "turn-1", "item": {
                    "type": "commandExecution", "id": "exec-1",
                    "command": "echo test", "cwd": std::env::current_dir().unwrap(),
                    "processId": null, "status": native_status, "commandActions": [],
                    "aggregatedOutput": "test", "exitCode": exit_code, "durationMs": null
                }}
            });
            let Some(TypedProviderEvent::ToolCall {
                id,
                name,
                status,
                arguments,
                result,
            }) = classify(&payload)
            else {
                panic!("native command must become a canonical tool call");
            };
            assert_eq!(id.as_deref(), Some("exec-1"));
            assert_eq!(name, "Shell");
            assert_eq!(status, expected);
            assert_eq!(arguments.unwrap()["command"], "echo test");
            if method == "item/completed" {
                assert_eq!(
                    result.unwrap(),
                    json!({ "output": "test", "exit_code": exit_code })
                );
            } else {
                assert_eq!(result, None);
            }
        }
    }

    #[test]
    fn codex_app_server_commands_keep_unsupported_or_malformed_items_unclassified() {
        for payload in [
            json!({ "method": "item/completed", "params": { "item": {
                "type": "futureTool", "id": "future-1"
            }}}),
            json!({ "method": "item/started", "params": { "item": {
                "type": "commandExecution", "id": "broken-1"
            }}}),
            json!({ "method": "other", "params": {} }),
        ] {
            assert!(classify(&payload).is_none());
        }
    }
}
