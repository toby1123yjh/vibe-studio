use serde_json::Value;

use super::{Decoder, TextChunk, string};
use crate::runtime::{
    AgentEventPayload, AgentRuntimeMessageRole, AgentRuntimeToolStatus, ConversationBlockFields,
    ConversationBlockKind, ConversationReasoningSource, ConversationTextField, NativeAuditManifest,
    ProviderEvent,
};

pub(super) fn decode(
    state: &mut Decoder,
    raw: &ProviderEvent,
    payload: &Value,
    manifest: &NativeAuditManifest,
    out: &mut Vec<AgentEventPayload>,
) -> bool {
    match payload["type"].as_str() {
        Some("message_start") => {
            state.boundary();
            state.block_indices.clear();
            state.current_message = string(payload, "messageId")
                .or_else(|| string(&payload["message"], "id"))
                .or_else(|| Some(format!("omp-message:{}", raw.sequence)));
            true
        }
        Some("message_update") => {
            let event = &payload["assistantMessageEvent"];
            let index = event["contentIndex"].as_u64().unwrap_or(0);
            if let Some(message) = string(payload, "messageId") {
                state.current_message = Some(message);
            }
            let message = state
                .current_message
                .get_or_insert_with(|| format!("omp-message:{}", raw.sequence))
                .clone();
            let native = format!("{message}:{index}");
            match event["type"].as_str() {
                Some("text_start" | "thinking_start") => {
                    let kind = if event["type"] == "thinking_start" {
                        ConversationBlockKind::Reasoning
                    } else {
                        ConversationBlockKind::Text
                    };
                    let block = state.block(manifest, raw, kind, Some(&native));
                    state.block_indices.insert(index, block.clone());
                    state.snapshot(
                        &block,
                        ConversationBlockFields {
                            text: Some(String::new()),
                            ..Default::default()
                        },
                        out,
                    );
                }
                Some("text_delta" | "thinking_delta") => {
                    let reasoning = event["type"] == "thinking_delta";
                    let block = state.text_chunk(
                        manifest,
                        raw,
                        TextChunk {
                            native_id: Some(&native),
                            text: event["delta"].as_str().unwrap_or_default(),
                            reasoning,
                            snapshot: false,
                        },
                        out,
                    );
                    state.block_indices.insert(index, block);
                }
                Some("text_end" | "thinking_end") => {
                    if let Some(block) = state.block_indices.get(&index).cloned() {
                        if let Some(content) = event["content"].as_str() {
                            state.snapshot(
                                &block,
                                ConversationBlockFields {
                                    text: Some(content.into()),
                                    ..Default::default()
                                },
                                out,
                            );
                        }
                        state.finish(&block, out);
                    }
                }
                Some("toolcall_start" | "toolcall_delta" | "toolcall_end") => {
                    let partial = event
                        .get("partial")
                        .and_then(|value| value.get("content"))
                        .and_then(Value::as_array)
                        .and_then(|parts| parts.get(index as usize));
                    let call = event.get("toolCall").or(partial).unwrap_or(&Value::Null);
                    let block = if let Some(block) = state.block_indices.get(&index) {
                        block.clone()
                    } else {
                        state.boundary();
                        let block = state.block(
                            manifest,
                            raw,
                            ConversationBlockKind::Tool,
                            call["id"].as_str(),
                        );
                        state.block_indices.insert(index, block.clone());
                        block
                    };
                    let block = state.bind_native(block, call["id"].as_str());
                    state.block_indices.insert(index, block.clone());
                    state.patch(
                        &block,
                        ConversationBlockFields {
                            tool_call_id: string(call, "id"),
                            tool_name: string(call, "name"),
                            tool_kind: tool_kind(call["name"].as_str()),
                            arguments: call.get("arguments").cloned(),
                            ..Default::default()
                        },
                        out,
                    );
                    if let Some(delta) = event["delta"].as_str() {
                        state.append(
                            &block,
                            ConversationTextField::RawArguments,
                            delta.into(),
                            out,
                        );
                    }
                }
                _ => return false,
            }
            true
        }
        Some("message_end") if payload["message"]["role"] == "assistant" => {
            let message = string(payload, "messageId")
                .or_else(|| string(&payload["message"], "id"))
                .or_else(|| state.current_message.clone())
                .unwrap_or_else(|| format!("omp-message:{}", raw.sequence));
            let mut full_text = Vec::new();
            let mut last_text = None;
            for (index, part) in payload["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                let native = format!("{message}:{index}");
                let kind = match part["type"].as_str() {
                    Some("text") => ConversationBlockKind::Text,
                    Some("thinking") => ConversationBlockKind::Reasoning,
                    Some("toolCall" | "tool_call") => ConversationBlockKind::Tool,
                    _ => continue,
                };
                let block = if kind == ConversationBlockKind::Tool {
                    if let Some(block) = state.block_indices.get(&(index as u64)).cloned() {
                        state.bind_native(block, part["id"].as_str())
                    } else {
                        state.block(manifest, raw, kind, part["id"].as_str())
                    }
                } else {
                    state.block(manifest, raw, kind, Some(&native))
                };
                if kind == ConversationBlockKind::Tool {
                    state.patch(
                        &block,
                        ConversationBlockFields {
                            tool_call_id: string(part, "id"),
                            tool_name: string(part, "name"),
                            tool_kind: tool_kind(part["name"].as_str()),
                            arguments: part.get("arguments").cloned(),
                            ..Default::default()
                        },
                        out,
                    );
                } else {
                    let text = part[if kind == ConversationBlockKind::Reasoning {
                        "thinking"
                    } else {
                        "text"
                    }]
                    .as_str()
                    .unwrap_or_default();
                    state.snapshot(
                        &block,
                        ConversationBlockFields {
                            text: Some(text.into()),
                            reasoning_source: (kind == ConversationBlockKind::Reasoning)
                                .then_some(ConversationReasoningSource::ExposedText),
                            ..Default::default()
                        },
                        out,
                    );
                    state.finish(&block, out);
                    if kind == ConversationBlockKind::Text {
                        full_text.push(text);
                        last_text = Some(block);
                    }
                }
            }
            if let Some(block) = last_text {
                state.final_output(
                    &block,
                    full_text.join(""),
                    AgentRuntimeMessageRole::Assistant,
                    out,
                );
            }
            true
        }
        Some("tool_execution_start" | "tool_execution_update" | "tool_execution_end") => {
            state.boundary();
            let block = state.block(
                manifest,
                raw,
                ConversationBlockKind::Tool,
                payload["toolCallId"].as_str(),
            );
            let ended = payload["type"] == "tool_execution_end";
            let status = if ended {
                Some(if payload["isError"] == true {
                    AgentRuntimeToolStatus::Failed
                } else {
                    AgentRuntimeToolStatus::Succeeded
                })
            } else if payload["type"] == "tool_execution_start" {
                Some(AgentRuntimeToolStatus::Running)
            } else {
                None
            };
            state.patch(
                &block,
                ConversationBlockFields {
                    tool_call_id: string(payload, "toolCallId"),
                    tool_name: string(payload, "toolName"),
                    tool_kind: tool_kind(payload["toolName"].as_str()),
                    status,
                    arguments: payload.get("args").cloned(),
                    result: payload
                        .get("result")
                        .or_else(|| payload.get("partialResult"))
                        .cloned(),
                    ..Default::default()
                },
                out,
            );
            if ended {
                state.finish(&block, out);
            }
            true
        }
        _ => false,
    }
}

fn tool_kind(name: Option<&str>) -> Option<String> {
    name.map(|name| {
        match name {
            "bash" => "shell",
            "read" => "read",
            "edit" | "write" => "edit",
            "grep" | "find" | "ls" => "search",
            "fetch" => "fetch",
            "subagent" => "subagent",
            _ => "other",
        }
        .into()
    })
}
