use serde_json::{Value, json};

use super::{Decoder, TextChunk, string};
use crate::runtime::{
    AgentEventPayload, AgentRuntimeMessageRole, AgentRuntimeToolStatus, ConversationBlockFields,
    ConversationBlockKind, ConversationInteraction, ConversationInteractionKind,
    ConversationInteractionOption, ConversationInteractionStatus, ConversationQuestion,
    ConversationReasoningSource, ConversationTextField, NativeAuditManifest, ProviderEvent,
};

pub(super) fn decode(
    state: &mut Decoder,
    raw: &ProviderEvent,
    payload: &Value,
    manifest: &NativeAuditManifest,
    out: &mut Vec<AgentEventPayload>,
) -> bool {
    match payload["type"].as_str() {
        Some("stream_event") => {
            let event = &payload["event"];
            let context = payload["parent_tool_use_id"]
                .as_str()
                .unwrap_or("main")
                .to_owned();
            match event["type"].as_str() {
                Some("message_start") => {
                    state.boundary();
                    let message = string(&event["message"], "id")
                        .unwrap_or_else(|| format!("claude-message:{}", raw.sequence));
                    state
                        .claude_blocks
                        .retain(|(parent, _), _| parent != &context);
                    state.claude_messages.insert(context, message);
                }
                Some("content_block_start") => {
                    let index = event["index"].as_u64().unwrap_or(0);
                    let part = &event["content_block"];
                    let message = state
                        .claude_messages
                        .entry(context.clone())
                        .or_insert_with(|| format!("claude-message:{}", raw.sequence));
                    let native = format!("{message}:{index}");
                    let kind = match part["type"].as_str() {
                        Some("text") => ConversationBlockKind::Text,
                        Some("thinking") => ConversationBlockKind::Reasoning,
                        Some("tool_use" | "server_tool_use") => ConversationBlockKind::Tool,
                        Some("redacted_thinking") => return true,
                        _ => return false,
                    };
                    if kind == ConversationBlockKind::Tool {
                        state.boundary();
                    }
                    let mut block = state.block(
                        manifest,
                        raw,
                        kind,
                        if kind == ConversationBlockKind::Tool {
                            part["id"].as_str()
                        } else {
                            Some(&native)
                        },
                    );
                    if let Some(parent) = payload["parent_tool_use_id"].as_str() {
                        block.parent = Some(
                            super::super::canonical_message_id(
                                manifest.run_attempt_id,
                                &format!("tool:{parent}"),
                            )
                            .to_string(),
                        );
                    }
                    state.claude_blocks.insert((context, index), block.clone());
                    let mut fields = part_fields(part, kind, false);
                    if fields.text.as_ref().is_some_and(String::is_empty)
                        && state.text.contains_key(&block.id)
                    {
                        fields.text = None;
                    }
                    state.snapshot(&block, fields, out);
                }
                Some("content_block_delta") => {
                    let index = event["index"].as_u64().unwrap_or(0);
                    let delta = &event["delta"];
                    let (field, text, kind) = match delta["type"].as_str() {
                        Some("text_delta") => (
                            ConversationTextField::Text,
                            delta["text"].as_str(),
                            ConversationBlockKind::Text,
                        ),
                        Some("thinking_delta") => (
                            ConversationTextField::Text,
                            delta["thinking"].as_str(),
                            ConversationBlockKind::Reasoning,
                        ),
                        Some("input_json_delta") => (
                            ConversationTextField::RawArguments,
                            delta["partial_json"].as_str(),
                            ConversationBlockKind::Tool,
                        ),
                        Some("signature_delta") => return true,
                        _ => return false,
                    };
                    let Some(text) = text else {
                        return false;
                    };
                    let key = (context.clone(), index);
                    let block = if let Some(block) = state.claude_blocks.get(&key) {
                        block.clone()
                    } else {
                        let message = state
                            .claude_messages
                            .entry(context)
                            .or_insert_with(|| format!("claude-message:{}", raw.sequence));
                        let native = format!("{message}:{index}");
                        let block = state.block(manifest, raw, kind, Some(&native));
                        state.claude_blocks.insert(key, block.clone());
                        block
                    };
                    state.append(&block, field, text.into(), out);
                }
                Some("content_block_stop") => {
                    if let Some(block) = state
                        .claude_blocks
                        .get(&(context, event["index"].as_u64().unwrap_or(0)))
                        .cloned()
                    {
                        state.finish(&block, out);
                    }
                }
                Some("message_stop" | "message_delta") => {}
                _ => return false,
            }
            true
        }
        Some("assistant")
            if payload.get("error").is_none()
                && payload["is_api_error_message"] != true
                && payload["is_error"] != true =>
        {
            let message = &payload["message"];
            let Some(parts) = message["content"].as_array() else {
                return false;
            };
            let context = payload["parent_tool_use_id"]
                .as_str()
                .unwrap_or("main")
                .to_owned();
            let message_id = string(message, "id")
                .or_else(|| state.claude_messages.get(&context).cloned())
                .unwrap_or_else(|| format!("claude-message:{}", raw.sequence));
            state.claude_messages.insert(context, message_id.clone());
            let mut full_text = Vec::new();
            let mut last_text = None;
            for (index, part) in parts.iter().enumerate() {
                let native = format!("{message_id}:{index}");
                let kind = match part["type"].as_str() {
                    Some("text") => ConversationBlockKind::Text,
                    Some("thinking") => ConversationBlockKind::Reasoning,
                    Some("tool_use" | "server_tool_use") => ConversationBlockKind::Tool,
                    Some("redacted_thinking") => continue,
                    _ => {
                        let block = state.block(
                            manifest,
                            raw,
                            ConversationBlockKind::Resource,
                            Some(&native),
                        );
                        state.snapshot(
                            &block,
                            ConversationBlockFields {
                                resource: Some(part.clone()),
                                ..Default::default()
                            },
                            out,
                        );
                        continue;
                    }
                };
                if kind == ConversationBlockKind::Tool {
                    state.boundary();
                }
                let mut block = state.block(
                    manifest,
                    raw,
                    kind,
                    if kind == ConversationBlockKind::Tool {
                        part["id"].as_str()
                    } else {
                        Some(&native)
                    },
                );
                if let Some(parent) = payload["parent_tool_use_id"].as_str() {
                    block.parent = Some(
                        super::super::canonical_message_id(
                            manifest.run_attempt_id,
                            &format!("tool:{parent}"),
                        )
                        .to_string(),
                    );
                }
                state.snapshot(&block, part_fields(part, kind, true), out);
                if kind != ConversationBlockKind::Tool {
                    state.finish(&block, out);
                }
                if kind == ConversationBlockKind::Text {
                    full_text.push(part["text"].as_str().unwrap_or_default());
                    last_text = Some(block);
                }
            }
            if let Some(block) = last_text.filter(|_| payload["parent_tool_use_id"].is_null()) {
                state.final_output(
                    &block,
                    full_text.join(""),
                    AgentRuntimeMessageRole::Assistant,
                    out,
                );
            }
            true
        }
        Some("user") if payload["message"]["content"].is_array() => {
            let mut handled = false;
            for part in payload["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if part["type"] != "tool_result" {
                    continue;
                }
                handled = true;
                state.boundary();
                let block = state.block(
                    manifest,
                    raw,
                    ConversationBlockKind::Tool,
                    part["tool_use_id"].as_str(),
                );
                state.patch(
                    &block,
                    ConversationBlockFields {
                        tool_call_id: string(part, "tool_use_id"),
                        status: Some(if part["is_error"] == true {
                            AgentRuntimeToolStatus::Failed
                        } else {
                            AgentRuntimeToolStatus::Succeeded
                        }),
                        result: part.get("content").cloned(),
                        ..Default::default()
                    },
                    out,
                );
                state.finish(&block, out);
                state.resolve_tool_interactions(part["tool_use_id"].as_str(), raw, manifest, out);
            }
            handled
        }
        Some("control_request") if payload["request"]["subtype"] == "can_use_tool" => {
            let Some(request_id) = payload["request_id"].as_str() else {
                return false;
            };
            state.boundary();
            let interaction = interaction_request(payload);
            state
                .pending_permissions
                .insert(request_id.into(), interaction.clone());
            let block = state.block(
                manifest,
                raw,
                ConversationBlockKind::Interaction,
                Some(request_id),
            );
            state.snapshot(
                &block,
                ConversationBlockFields {
                    interaction: Some(interaction),
                    ..Default::default()
                },
                out,
            );
            true
        }
        Some("control_cancel_request") => {
            if let Some(request_id) = payload["request_id"].as_str() {
                state.close_interaction(
                    request_id,
                    ConversationInteractionStatus::Cancelled,
                    raw,
                    manifest,
                    out,
                );
            }
            true
        }
        Some("result") if payload["is_error"] != true => {
            // The CLI's `result` repeats the completed assistant body. It is a
            // final-output fact, not a second visible text block.
            if let Some((block, text)) = state.latest_final.clone() {
                state.final_output(&block, text, AgentRuntimeMessageRole::Assistant, out);
                false // existing classifier preserves the terminal lifecycle
            } else if let Some(text) = payload["result"].as_str() {
                let block = state.text_chunk(
                    manifest,
                    raw,
                    TextChunk {
                        native_id: None,
                        text,
                        reasoning: false,
                        snapshot: true,
                    },
                    out,
                );
                state.finish(&block, out);
                state.final_output(&block, text.into(), AgentRuntimeMessageRole::Assistant, out);
                false
            } else {
                false
            }
        }
        _ => false,
    }
}

fn part_fields(
    part: &Value,
    kind: ConversationBlockKind,
    complete: bool,
) -> ConversationBlockFields {
    match kind {
        ConversationBlockKind::Text => ConversationBlockFields {
            text: string(part, "text"),
            ..Default::default()
        },
        ConversationBlockKind::Reasoning => ConversationBlockFields {
            text: string(part, "thinking"),
            reasoning_source: Some(ConversationReasoningSource::ExposedText),
            ..Default::default()
        },
        ConversationBlockKind::Tool => ConversationBlockFields {
            tool_call_id: string(part, "id"),
            tool_name: string(part, "name"),
            tool_kind: part["name"].as_str().map(|name| {
                match name {
                    "Bash" => "shell",
                    "Read" | "NotebookRead" => "read",
                    "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => "edit",
                    "Glob" | "Grep" | "WebSearch" => "search",
                    "WebFetch" => "fetch",
                    "Task" | "Agent" => "subagent",
                    _ => "other",
                }
                .into()
            }),
            arguments: part
                .get("input")
                .filter(|input| complete || **input != json!({}))
                .cloned(),
            status: Some(AgentRuntimeToolStatus::Running),
            ..Default::default()
        },
        _ => Default::default(),
    }
}

pub(crate) fn interaction_request(payload: &Value) -> ConversationInteraction {
    let request = &payload["request"];
    let questions = &request["input"]["questions"];
    let is_question = request["tool_name"] == "AskUserQuestion";
    ConversationInteraction {
        request_id: payload["request_id"].as_str().unwrap_or_default().into(),
        kind: if is_question {
            ConversationInteractionKind::Question
        } else {
            ConversationInteractionKind::Permission
        },
        status: ConversationInteractionStatus::Pending,
        title: string(request, "tool_name"),
        prompt: string(&request["input"], "description"),
        tool_call_id: string(request, "tool_use_id"),
        native_request_id: payload.get("request_id").cloned(),
        options: if is_question {
            vec![]
        } else {
            vec![
                ConversationInteractionOption {
                    id: "allow".into(),
                    label: "Allow".into(),
                    kind: Some("allow_once".into()),
                    description: None,
                },
                ConversationInteractionOption {
                    id: "deny".into(),
                    label: "Deny".into(),
                    kind: Some("reject_once".into()),
                    description: None,
                },
            ]
        },
        questions: questions
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, question)| ConversationQuestion {
                id: question["question"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| index.to_string()),
                prompt: question["question"].as_str().unwrap_or_default().into(),
                options: question["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|option| {
                        Some(ConversationInteractionOption {
                            id: option["label"].as_str()?.into(),
                            label: option["label"].as_str()?.into(),
                            kind: None,
                            description: string(option, "description"),
                        })
                    })
                    .collect(),
                multiple: question["multiSelect"].as_bool().unwrap_or(false),
                allow_free_text: true,
                required: true,
            })
            .collect(),
        scope: Some(request.clone()),
        actionable: true,
    }
}
