use serde_json::{Value, json};

use super::{Decoder, TextChunk, is_terminal, string, tool_status};
use crate::runtime::{
    AgentEventPayload, AgentRuntimeMessageRole, ConversationBlockFields, ConversationBlockKind,
    ConversationInteraction, ConversationInteractionKind, ConversationInteractionOption,
    ConversationInteractionStatus, ConversationLocation, ConversationPlanEntry,
    NativeAuditManifest, ProviderEvent,
};

pub(super) fn decode(
    state: &mut Decoder,
    raw: &ProviderEvent,
    payload: &Value,
    manifest: &NativeAuditManifest,
    out: &mut Vec<AgentEventPayload>,
) -> bool {
    let update = if payload["method"] == "session/update" {
        &payload["params"]["update"]
    } else {
        payload
    };
    // Both audited harness events and native ACP notifications feed the same
    // semantics. No per-provider copy of the common ACP stream is necessary.
    if let Some(content) = update.get("Message").or_else(|| {
        (update["sessionUpdate"] == "agent_message_chunk").then_some(&update["content"])
    }) {
        content_block(state, raw, content, manifest, false, out);
        return true;
    }
    if let Some(content) = update.get("Thought").or_else(|| {
        (update["sessionUpdate"] == "agent_thought_chunk").then_some(&update["content"])
    }) {
        content_block(state, raw, content, manifest, true, out);
        return true;
    }
    if let Some(call) = update
        .get("ToolCall")
        .or_else(|| update.get("ToolUpdate"))
        .or_else(|| {
            matches!(
                update["sessionUpdate"].as_str(),
                Some("tool_call" | "tool_call_update")
            )
            .then_some(update)
        })
    {
        state.boundary();
        let id = call["toolCallId"].as_str();
        let block = state.block(manifest, raw, ConversationBlockKind::Tool, id);
        let fields = tool_fields(call);
        let terminal = is_terminal(fields.status);
        state.patch(&block, fields, out);
        if terminal {
            state.finish(&block, out);
            state.resolve_tool_interactions(id, raw, manifest, out);
        }
        return true;
    }
    if let Some(plan) = update
        .get("Plan")
        .or_else(|| (update["sessionUpdate"] == "plan").then_some(update))
    {
        state.boundary();
        let block = state.block(manifest, raw, ConversationBlockKind::Plan, Some("acp-plan"));
        let entries = plan["entries"].as_array().map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    Some(ConversationPlanEntry {
                        content: entry["content"].as_str()?.into(),
                        status: entry["status"].as_str().unwrap_or("pending").into(),
                        priority: string(entry, "priority"),
                    })
                })
                .collect()
        });
        state.snapshot(
            &block,
            ConversationBlockFields {
                plan_entries: entries,
                ..Default::default()
            },
            out,
        );
        return true;
    }
    if let Some(request) = payload.get("RequestPermission") {
        let call = &request["toolCall"];
        let Some(id) = call["toolCallId"].as_str() else {
            return false;
        };
        // This event precedes register_permission's platform ID. Wait for the
        // subsequent ApprovalRequested; never expose an unanswerable request.
        state.pending_permissions.insert(
            format!("acp-tool:{id}"),
            ConversationInteraction {
                request_id: String::new(),
                kind: ConversationInteractionKind::Permission,
                status: ConversationInteractionStatus::Pending,
                title: string(call, "title"),
                prompt: string(call, "title"),
                tool_call_id: Some(id.into()),
                native_request_id: None,
                options: request["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|option| {
                        Some(ConversationInteractionOption {
                            id: option["optionId"].as_str()?.into(),
                            label: option["name"].as_str().unwrap_or_default().into(),
                            kind: string(option, "kind"),
                            description: None,
                        })
                    })
                    .collect(),
                questions: vec![],
                scope: Some(request.clone()),
                actionable: true,
            },
        );
        return true;
    }
    if let Some(request) = payload.get("ApprovalRequested") {
        let Some(id) = request["approval_id"].as_str() else {
            return false;
        };
        let tool = request["tool_call_id"].as_str().unwrap_or_default();
        if let Some(mut interaction) = state
            .pending_permissions
            .remove(&format!("acp-tool:{tool}"))
        {
            interaction.request_id = id.into();
            state
                .pending_permissions
                .insert(id.into(), interaction.clone());
            state.boundary();
            let block = state.block(manifest, raw, ConversationBlockKind::Interaction, Some(id));
            state.snapshot(
                &block,
                ConversationBlockFields {
                    interaction: Some(interaction),
                    ..Default::default()
                },
                out,
            );
        }
        out.push(AgentEventPayload::ApprovalRequested {
            approval_id: id.into(),
            tool_call_id: Some(tool.into()),
            tool_name: tool.into(),
        });
        return true;
    }
    if let Some(response) = payload.get("ApprovalResponse") {
        let id = response["approval_id"]
            .as_str()
            .or_else(|| response["tool_call_id"].as_str());
        if let Some(id) = id
            && let Some(mut interaction) = state.pending_permissions.get(id).cloned()
        {
            // ACP callback returns a native decision, but it has not yet
            // reached the writer. Host records Delivered after real flush.
            interaction.status = ConversationInteractionStatus::Delivering;
            interaction.actionable = false;
            state
                .pending_permissions
                .insert(id.into(), interaction.clone());
            let block = state.block(manifest, raw, ConversationBlockKind::Interaction, Some(id));
            state.patch(
                &block,
                ConversationBlockFields {
                    interaction: Some(interaction),
                    ..Default::default()
                },
                out,
            );
        }
        return true;
    }
    if payload.get("Done").is_some() {
        let blocks = state.acp_texts.clone();
        if let Some(last) = blocks.last() {
            let text = blocks
                .iter()
                .filter_map(|block| state.text.get(&block.id))
                .cloned()
                .collect::<Vec<_>>()
                .join("");
            for block in &blocks {
                state.finish(block, out);
            }
            state.final_output(last, text, AgentRuntimeMessageRole::Assistant, out);
        }
        if let Some(block) = state.active_reasoning.clone() {
            state.finish(&block, out);
        }
        state.boundary();
        return false; // retain native stop-reason lifecycle through classifier
    }
    false
}

fn content_block(
    state: &mut Decoder,
    raw: &ProviderEvent,
    content: &Value,
    manifest: &NativeAuditManifest,
    reasoning: bool,
    out: &mut Vec<AgentEventPayload>,
) {
    if content["type"] == "text" {
        let block = state.text_chunk(
            manifest,
            raw,
            TextChunk {
                native_id: None,
                text: content["text"].as_str().unwrap_or_default(),
                reasoning,
                snapshot: false,
            },
            out,
        );
        if !reasoning && !state.acp_texts.iter().any(|text| text.id == block.id) {
            state.acp_texts.push(block);
        }
    } else {
        state.boundary();
        let block = state.block(manifest, raw, ConversationBlockKind::Resource, None);
        state.snapshot(
            &block,
            ConversationBlockFields {
                resource: Some(content.clone()),
                ..Default::default()
            },
            out,
        );
    }
}

fn tool_fields(call: &Value) -> ConversationBlockFields {
    let fields = call.get("fields").unwrap_or(call);
    ConversationBlockFields {
        tool_call_id: string(call, "toolCallId"),
        tool_name: string(fields, "title"),
        tool_kind: string(fields, "kind"),
        status: fields["status"].as_str().and_then(tool_status),
        arguments: fields.get("rawInput").cloned(),
        result: fields
            .get("rawOutput")
            .cloned()
            .or_else(|| fields.get("content").cloned()),
        locations: fields["locations"].as_array().map(|locations| {
            locations
                .iter()
                .filter_map(|location| {
                    Some(ConversationLocation {
                        path: location["path"].as_str()?.into(),
                        line: location["line"]
                            .as_u64()
                            .and_then(|line| u32::try_from(line).ok()),
                    })
                })
                .collect()
        }),
        extension: fields
            .get("content")
            .map(|content| json!({"content":content})),
        ..Default::default()
    }
}
