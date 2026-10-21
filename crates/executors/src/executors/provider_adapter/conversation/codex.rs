use serde_json::{Value, json};

use super::{Decoder, TextChunk, is_terminal, string, tool_status};
use crate::runtime::{
    AgentEventPayload, AgentRunStatus, AgentRuntimeError, AgentRuntimeErrorKind,
    AgentRuntimeMessageRole, ConversationBlockFields, ConversationBlockKind,
    ConversationInteraction, ConversationInteractionKind, ConversationInteractionOption,
    ConversationInteractionStatus, ConversationLocation, ConversationPlanEntry,
    ConversationQuestion, ConversationReasoningSource, ConversationTextField, NativeAuditManifest,
    ProviderEvent,
};

pub(super) fn decode(
    state: &mut Decoder,
    raw: &ProviderEvent,
    payload: &Value,
    manifest: &NativeAuditManifest,
    out: &mut Vec<AgentEventPayload>,
) -> bool {
    // These externally tagged errors are written by the Codex executor when
    // initialization or a request fails, rather than by app-server itself.
    // Preserve the exact diagnostic, including the executor's launch context.
    for (variant, kind) in [
        ("LaunchError", AgentRuntimeErrorKind::StartupFailed),
        ("AuthRequired", AgentRuntimeErrorKind::AuthRequired),
    ] {
        if let Some(message) = payload[variant]["error"].as_str() {
            out.push(error(kind, message.into()));
            return true;
        }
    }
    let Some(method) = payload["method"].as_str() else {
        if payload.get("id").is_some() {
            if payload.get("result").is_some() && payload.get("error").is_none() {
                // Responses to initialize/thread/start/etc are transport
                // acknowledgements. Their full bytes remain in Native Audit.
                return true;
            }
            if let Some(message) = diagnostic(&payload["error"]) {
                // A failed optional request (e.g. a setting/control) does not
                // prove that the business turn failed. The request's caller
                // decides whether to emit a terminal executor error afterward.
                notice(state, raw, manifest, message, out);
                return true;
            }
        }
        return false;
    };
    let params = &payload["params"];
    let item_id = params
        .get("itemId")
        .or_else(|| params.get("item_id"))
        .and_then(Value::as_str);
    match method {
        "error" => {
            match (diagnostic(&params["error"]), params["willRetry"].as_bool()) {
                (Some(message), Some(true)) => notice(
                    state,
                    raw,
                    manifest,
                    format!("Codex is retrying: {message}"),
                    out,
                ),
                (Some(message), Some(false)) => {
                    out.push(error(AgentRuntimeErrorKind::Unknown, message))
                }
                _ => {
                    // Do not invent terminal semantics for an unrecognized
                    // notification shape or let the generic error fallback do so.
                    out.push(AgentEventPayload::ProviderExtension {
                        provider_namespace: "codex".into(),
                        provider_event: method.into(),
                        payload: payload.clone(),
                    });
                }
            }
        }
        "turn/completed" if params["turn"]["status"] == "failed" => {
            if let Some(message) = diagnostic(&params["turn"]["error"]) {
                out.push(error(AgentRuntimeErrorKind::Unknown, message));
            } else {
                // A preceding error notification may already hold the actual
                // cause. Do not replace it with a fabricated generic error.
                out.push(AgentEventPayload::LifecycleChanged {
                    status: AgentRunStatus::Failed,
                });
            }
        }
        "thread/status/changed"
        | "turn/started"
        | "thread/name/updated"
        | "thread/closed"
        | "thread/compacted"
        | "item/reasoning/summaryPartAdded"
        | "rawResponseItem/completed"
        | "thread/tokenUsage/updated"
        | "account/updated"
        | "account/rateLimits/updated"
        | "skills/changed"
        | "app/list/updated" => {
            // Known housekeeping notifications are audit facts, not chat
            // content. Keep this list explicit so future meaningful events
            // retain the extension fallback.
        }
        "mcpServer/startupStatus/updated" => match params["status"].as_str() {
            Some("starting" | "ready" | "cancelled") => {}
            Some("failed") => {
                let name = params["name"].as_str().unwrap_or("unknown");
                let message = params["error"].as_str().unwrap_or("failed to start");
                notice(
                    state,
                    raw,
                    manifest,
                    format!("MCP server {name}: {message}"),
                    out,
                );
            }
            _ => return false,
        },
        "item/agentMessage/delta" => {
            state.text_chunk(
                manifest,
                raw,
                TextChunk {
                    native_id: item_id,
                    text: params["delta"].as_str().unwrap_or_default(),
                    reasoning: false,
                    snapshot: false,
                },
                out,
            );
        }
        "item/plan/delta" => {
            let block = state.block(manifest, raw, ConversationBlockKind::Plan, item_id);
            state.append(
                &block,
                ConversationTextField::Text,
                params["delta"].as_str().unwrap_or_default().into(),
                out,
            );
        }
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            let summary = method.contains("summary");
            let index = params
                .get(if summary {
                    "summaryIndex"
                } else {
                    "contentIndex"
                })
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let id = item_id.map(|id| {
                format!(
                    "{id}:{}:{index}",
                    if summary { "summary" } else { "content" }
                )
            });
            let block = state.block(
                manifest,
                raw,
                ConversationBlockKind::Reasoning,
                id.as_deref(),
            );
            state.patch(
                &block,
                ConversationBlockFields {
                    reasoning_source: Some(if summary {
                        ConversationReasoningSource::Summary
                    } else {
                        ConversationReasoningSource::ExposedText
                    }),
                    ..Default::default()
                },
                out,
            );
            state.append(
                &block,
                ConversationTextField::Text,
                params["delta"].as_str().unwrap_or_default().into(),
                out,
            );
        }
        "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => {
            let block = state.block(manifest, raw, ConversationBlockKind::Tool, item_id);
            state.append(
                &block,
                ConversationTextField::OutputText,
                params["delta"].as_str().unwrap_or_default().into(),
                out,
            );
        }
        "item/started" | "item/completed" => {
            let item = &params["item"];
            let Some(kind) = item["type"].as_str() else {
                return false;
            };
            let id = item["id"].as_str();
            let completed = method == "item/completed";
            match kind {
                "agentMessage" => {
                    let block = state.block(manifest, raw, ConversationBlockKind::Text, id);
                    let text = item["text"].as_str().unwrap_or_default();
                    if completed || !text.is_empty() || !state.text.contains_key(&block.id) {
                        state.snapshot(
                            &block,
                            ConversationBlockFields {
                                text: Some(text.into()),
                                ..Default::default()
                            },
                            out,
                        );
                    }
                    if completed {
                        state.finish(&block, out);
                        if item["phase"] != "commentary" {
                            state.final_output(
                                &block,
                                item["text"].as_str().unwrap_or_default().into(),
                                AgentRuntimeMessageRole::Assistant,
                                out,
                            );
                        }
                    }
                }
                "plan" => {
                    state.boundary();
                    let block = state.block(manifest, raw, ConversationBlockKind::Plan, id);
                    state.snapshot(
                        &block,
                        ConversationBlockFields {
                            text: string(item, "text"),
                            ..Default::default()
                        },
                        out,
                    );
                    if completed {
                        state.finish(&block, out);
                    }
                }
                "reasoning" => {
                    for (field, source) in [
                        ("summary", ConversationReasoningSource::Summary),
                        ("content", ConversationReasoningSource::ExposedText),
                    ] {
                        for (index, part) in
                            item[field].as_array().into_iter().flatten().enumerate()
                        {
                            let Some(text) = part.as_str().or_else(|| part["text"].as_str()) else {
                                continue;
                            };
                            let native = id.map(|id| format!("{id}:{field}:{index}"));
                            let block = state.block(
                                manifest,
                                raw,
                                ConversationBlockKind::Reasoning,
                                native.as_deref(),
                            );
                            state.snapshot(
                                &block,
                                ConversationBlockFields {
                                    text: Some(text.into()),
                                    reasoning_source: Some(source),
                                    ..Default::default()
                                },
                                out,
                            );
                            if completed {
                                state.finish(&block, out);
                            }
                        }
                    }
                }
                "userMessage" => {
                    // Echo of input already persisted by the run writer. The
                    // original content remains available in native audit.
                }
                "commandExecution"
                | "fileChange"
                | "mcpToolCall"
                | "dynamicToolCall"
                | "collabAgentToolCall"
                | "webSearch"
                | "imageGeneration" => {
                    state.boundary();
                    let mut block = state.block(manifest, raw, ConversationBlockKind::Tool, id);
                    if let Some(parent) = item.get("parentToolCallId").and_then(Value::as_str) {
                        block.parent = Some(
                            super::super::canonical_message_id(
                                manifest.run_attempt_id,
                                &format!("tool:{parent}"),
                            )
                            .to_string(),
                        );
                    }
                    let status = item["status"].as_str().and_then(tool_status).or_else(|| {
                        (kind == "webSearch").then_some(if completed {
                            crate::runtime::AgentRuntimeToolStatus::Succeeded
                        } else {
                            crate::runtime::AgentRuntimeToolStatus::Running
                        })
                    });
                    let mut fields = ConversationBlockFields {
                        tool_call_id: id.map(str::to_owned),
                        tool_kind: Some(
                            match kind {
                                "commandExecution" => "shell",
                                "fileChange" => "edit",
                                "mcpToolCall" => "mcp",
                                "collabAgentToolCall" => "subagent",
                                "webSearch" => "search",
                                _ => "other",
                            }
                            .into(),
                        ),
                        status,
                        ..Default::default()
                    };
                    match kind {
                        "commandExecution" => {
                            fields.tool_name = Some("Shell".into());
                            if item.get("command").is_some() {
                                fields.arguments =
                                    Some(json!({"command":item["command"], "cwd":item["cwd"]}));
                            }
                            fields.output_text = string(item, "aggregatedOutput");
                            if completed {
                                fields.result = Some(
                                    json!({"output":item["aggregatedOutput"], "exit_code":item["exitCode"], "duration_ms":item["durationMs"]}),
                                );
                                if item["exitCode"].as_i64().is_some_and(|code| code != 0) {
                                    fields.status =
                                        Some(crate::runtime::AgentRuntimeToolStatus::Failed);
                                }
                            }
                        }
                        "fileChange" => {
                            fields.tool_name = Some("File changes".into());
                            fields.arguments = item.get("changes").cloned();
                            fields.locations = item["changes"].as_array().map(|changes| {
                                changes
                                    .iter()
                                    .filter_map(|change| {
                                        Some(ConversationLocation {
                                            path: change["path"].as_str()?.into(),
                                            line: None,
                                        })
                                    })
                                    .collect()
                            });
                        }
                        "mcpToolCall" => {
                            fields.tool_name =
                                string(item, "tool").or_else(|| string(item, "toolName"));
                            fields.arguments = item.get("arguments").cloned();
                            fields.result = item
                                .get("error")
                                .filter(|value| !value.is_null())
                                .cloned()
                                .or_else(|| item.get("result").cloned());
                            fields.extension = Some(
                                json!({"server":item["server"],"app_context":item["appContext"],"plugin_id":item["pluginId"]}),
                            );
                        }
                        "dynamicToolCall" => {
                            fields.tool_name =
                                string(item, "tool").or_else(|| string(item, "toolName"));
                            fields.arguments = item.get("arguments").cloned();
                            fields.result = item.get("contentItems").cloned();
                            fields.extension = Some(
                                json!({"namespace":item["namespace"], "success":item["success"]}),
                            );
                            if item["success"] == false {
                                fields.status =
                                    Some(crate::runtime::AgentRuntimeToolStatus::Failed);
                            }
                        }
                        _ => {
                            fields.tool_name = string(item, "tool")
                                .or_else(|| string(item, "name"))
                                .or_else(|| Some(kind.into()));
                            fields.arguments = item.get("arguments").cloned();
                            fields.result = completed.then(|| item.clone());
                            fields.extension = Some(item.clone());
                        }
                    }
                    let terminal = is_terminal(fields.status);
                    state.patch(&block, fields, out);
                    if completed || terminal {
                        state.finish(&block, out);
                        state.resolve_tool_interactions(id, raw, manifest, out);
                    }
                }
                _ => return false,
            }
        }
        "turn/plan/updated" => {
            state.boundary();
            let id = params["turnId"].as_str().map(|id| format!("plan:{id}"));
            let block = state.block(manifest, raw, ConversationBlockKind::Plan, id.as_deref());
            let entries = params["plan"].as_array().map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        Some(ConversationPlanEntry {
                            content: entry
                                .get("step")
                                .or_else(|| entry.get("content"))?
                                .as_str()?
                                .into(),
                            status: entry["status"].as_str().unwrap_or("pending").into(),
                            priority: None,
                        })
                    })
                    .collect()
            });
            state.snapshot(
                &block,
                ConversationBlockFields {
                    plan_entries: entries,
                    text: string(params, "explanation"),
                    ..Default::default()
                },
                out,
            );
        }
        "item/commandExecution/requestApproval"
        | "item/fileChange/requestApproval"
        | "item/tool/requestUserInput" => {
            if payload.get("id").is_none() {
                return false;
            }
            state.boundary();
            let request = interaction_request(payload);
            let block = state.block(
                manifest,
                raw,
                ConversationBlockKind::Interaction,
                Some(&request.request_id),
            );
            state
                .pending_permissions
                .insert(request.request_id.clone(), request.clone());
            state.snapshot(
                &block,
                ConversationBlockFields {
                    interaction: Some(request.clone()),
                    ..Default::default()
                },
                out,
            );
            if request.kind == ConversationInteractionKind::Permission {
                out.push(AgentEventPayload::ApprovalRequested {
                    approval_id: request.request_id,
                    tool_call_id: request.tool_call_id,
                    tool_name: method.into(),
                });
            } else {
                out.push(AgentEventPayload::InputRequested {
                    input_id: request.request_id,
                    prompt: request.prompt.unwrap_or_default(),
                });
            }
        }
        _ => return false,
    }
    true
}

fn error(kind: AgentRuntimeErrorKind, message: String) -> AgentEventPayload {
    AgentEventPayload::Error {
        error: AgentRuntimeError::new(kind, message).with_provider(Some("codex")),
    }
}

fn diagnostic(error: &Value) -> Option<String> {
    let message = error["message"].as_str()?;
    match error["additionalDetails"].as_str() {
        Some(details) if !details.is_empty() => Some(format!("{message}\n\n{details}")),
        _ => Some(message.into()),
    }
}

fn notice(
    state: &mut Decoder,
    raw: &ProviderEvent,
    manifest: &NativeAuditManifest,
    message: String,
    out: &mut Vec<AgentEventPayload>,
) {
    state.boundary();
    let block = state.block(manifest, raw, ConversationBlockKind::Notice, None);
    state.snapshot(
        &block,
        ConversationBlockFields {
            text: Some(message),
            ..Default::default()
        },
        out,
    );
    state.finish(&block, out);
}

pub(crate) fn request_key(native: &Value) -> String {
    format!("codex:{native}")
}

pub(crate) fn approval_decisions(params: &Value) -> Vec<Value> {
    params
        .get("availableDecisions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            vec![
                json!("accept"),
                json!("acceptForSession"),
                json!("decline"),
                json!("cancel"),
            ]
        })
}

pub(crate) fn interaction_request(payload: &Value) -> ConversationInteraction {
    let params = &payload["params"];
    let question = payload["method"] == "item/tool/requestUserInput";
    ConversationInteraction {
        request_id: request_key(&payload["id"]),
        kind: if question {
            ConversationInteractionKind::Question
        } else {
            ConversationInteractionKind::Permission
        },
        status: ConversationInteractionStatus::Pending,
        title: string(params, "command").or_else(|| string(params, "reason")),
        prompt: string(params, "reason"),
        tool_call_id: string(params, "itemId"),
        native_request_id: payload.get("id").cloned(),
        options: if question {
            vec![]
        } else {
            approval_decisions(params)
                .iter()
                .map(|option| {
                    let id = option
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| option.to_string());
                    ConversationInteractionOption {
                        label: id.clone(),
                        id,
                        kind: None,
                        description: None,
                    }
                })
                .collect()
        },
        questions: params["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                Some(ConversationQuestion {
                    id: entry["id"].as_str()?.into(),
                    prompt: entry["question"].as_str().unwrap_or_default().into(),
                    options: entry["options"]
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
                    multiple: entry["isMultiple"].as_bool().unwrap_or(false),
                    allow_free_text: entry["isOther"].as_bool().unwrap_or(false)
                        || entry["options"].as_array().is_none_or(Vec::is_empty),
                    required: true,
                })
            })
            .collect(),
        scope: Some(params.clone()),
        actionable: true,
    }
}
