//! Stateful native decoding shared by live Process Host readers and audit
//! replay. No UI/storage consumer should rediscover these protocol semantics.

use std::collections::HashMap;

use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    DecodedProviderEvent, DirectProvider, TypedProviderEvent, canonical_message_id,
    classify_payload, envelope_for_payload, file_evidence, map_typed_event,
};
use crate::runtime::{
    AgentEvent, AgentEventPayload, AgentRuntimeMessageRole, CanonicalMessage,
    ConversationBlockFields, ConversationBlockKind, ConversationBlockOperation,
    ConversationBlockUpdate, ConversationInteraction, ConversationReasoningSource,
    ConversationTextField, DefaultNativeAuditMapper, NativeAuditDirection, NativeAuditError,
    NativeAuditFrame, NativeAuditManifest, NativeAuditReplayMapper, ProviderEvent,
};

mod acp;
pub(crate) mod claude;
pub(crate) mod codex;
mod omp;
#[cfg(test)]
mod tests;

#[derive(Debug)]
pub struct StatefulProviderMapper {
    provider: DirectProvider,
    attempt: Option<Uuid>,
    sequence: u64,
    decoder: Decoder,
}

impl StatefulProviderMapper {
    pub fn new(provider: DirectProvider) -> Self {
        Self {
            provider,
            attempt: None,
            sequence: 0,
            decoder: Decoder::default(),
        }
    }

    pub fn map_frame(
        &mut self,
        frame: &NativeAuditFrame,
        manifest: &NativeAuditManifest,
    ) -> Result<Vec<AgentEvent>, NativeAuditError> {
        let mut raw = DefaultNativeAuditMapper::for_manifest(manifest).decode(frame)?;
        raw.native_ref.stream_id = manifest.run_attempt_id;
        self.map_event(&raw, manifest)
    }

    pub(super) fn map_event(
        &mut self,
        raw: &ProviderEvent,
        manifest: &NativeAuditManifest,
    ) -> Result<Vec<AgentEvent>, NativeAuditError> {
        if self.attempt != Some(manifest.run_attempt_id) {
            self.attempt = Some(manifest.run_attempt_id);
            self.sequence = 0;
            self.decoder = Decoder::default();
        }
        let mut payloads = Vec::new();
        if raw.direction == NativeAuditDirection::Input {
            // Native control/input audit records are not assistant messages.
            // Canonical user input is already appended by the run writer.
            return Ok(Vec::new());
        }
        let handled = raw
            .payload_json
            .as_ref()
            .is_some_and(|payload| match self.provider {
                DirectProvider::Codex => {
                    codex::decode(&mut self.decoder, raw, payload, manifest, &mut payloads)
                }
                DirectProvider::ClaudeCode => {
                    claude::decode(&mut self.decoder, raw, payload, manifest, &mut payloads)
                }
                DirectProvider::OhMyPi => {
                    omp::decode(&mut self.decoder, raw, payload, manifest, &mut payloads)
                }
                DirectProvider::Gemini
                | DirectProvider::Opencode
                | DirectProvider::DeepseekHarness => {
                    acp::decode(&mut self.decoder, raw, payload, manifest, &mut payloads)
                }
            });
        if !handled {
            self.fallback(raw, manifest, &mut payloads)?;
        }
        // File evidence is an independent fact; never replace the tool's own
        // lifecycle/body with a file list from the same native frame.
        if let Some(evidence) = file_evidence::completed_file_changes(self.provider, raw) {
            payloads.push(evidence);
        }
        // Process hosts stop consuming after terminal lifecycle/error. All
        // content, final-output and file facts must be persisted before it.
        payloads.sort_by_key(|payload| {
            matches!(
                payload,
                AgentEventPayload::LifecycleChanged { .. } | AgentEventPayload::Error { .. }
            )
        });
        let mut events = Vec::with_capacity(payloads.len());
        for (subindex, payload) in payloads.into_iter().enumerate() {
            self.sequence += 1;
            let mut envelope = envelope_for_payload(raw, manifest, payload);
            envelope.sequence = self.sequence;
            envelope.event_id = frame_event_id(manifest.run_attempt_id, raw.sequence, subindex);
            envelope.native_refs = vec![crate::runtime::NativeAuditReference {
                stream_id: manifest.run_attempt_id,
                sequence: raw.sequence,
                checksum: raw.native_ref.checksum.clone(),
            }];
            events.push(envelope);
        }
        Ok(events)
    }

    fn fallback(
        &mut self,
        raw: &ProviderEvent,
        manifest: &NativeAuditManifest,
        out: &mut Vec<AgentEventPayload>,
    ) -> Result<(), NativeAuditError> {
        let typed = classify_payload(self.provider, raw)?;
        match typed {
            TypedProviderEvent::Message {
                role,
                content,
                final_output,
                message_id,
            } => {
                let native = message_id.as_deref();
                let mut block =
                    self.decoder
                        .block(manifest, raw, ConversationBlockKind::Text, native);
                block.role = Some(role);
                self.decoder.snapshot(
                    &block,
                    ConversationBlockFields {
                        text: Some(content.clone()),
                        ..Default::default()
                    },
                    out,
                );
                if final_output {
                    self.decoder.finish(&block, out);
                    self.decoder.final_output(&block, content, role, out);
                }
            }
            TypedProviderEvent::Thinking(content) => {
                let block =
                    self.decoder
                        .block(manifest, raw, ConversationBlockKind::Reasoning, None);
                self.decoder
                    .append(&block, ConversationTextField::Text, content, out);
            }
            TypedProviderEvent::ToolCall {
                id,
                name,
                status,
                arguments,
                result,
            } => {
                self.decoder.boundary();
                let block =
                    self.decoder
                        .block(manifest, raw, ConversationBlockKind::Tool, id.as_deref());
                self.decoder.patch(
                    &block,
                    ConversationBlockFields {
                        tool_call_id: id,
                        tool_name: Some(name),
                        status: Some(status),
                        arguments,
                        result,
                        ..Default::default()
                    },
                    out,
                );
            }
            other => {
                // Reuse the existing state/error/session/configuration mapper.
                // Native content has already been claimed by its decoder.
                let decoded = DecodedProviderEvent {
                    raw: raw.clone(),
                    typed: other,
                };
                out.extend(
                    map_typed_event(self.provider, &decoded, manifest)?
                        .into_iter()
                        .map(|event| event.payload)
                        .filter(|payload| {
                            !matches!(payload, AgentEventPayload::FileChanges { .. })
                        }),
                );
            }
        }
        Ok(())
    }
}

fn frame_event_id(attempt: Uuid, frame: u64, subindex: usize) -> Uuid {
    let mut hash = Sha256::new();
    hash.update(attempt.as_bytes());
    hash.update(b"conversation-event-v2");
    hash.update(frame.to_be_bytes());
    hash.update((subindex as u64).to_be_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash.finalize()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

#[derive(Debug, Clone)]
struct Block {
    id: String,
    native_id: Option<String>,
    kind: ConversationBlockKind,
    role: Option<AgentRuntimeMessageRole>,
    parent: Option<String>,
}

struct TextChunk<'a> {
    native_id: Option<&'a str>,
    text: &'a str,
    reasoning: bool,
    snapshot: bool,
}

#[derive(Debug, Default)]
struct Decoder {
    active_text: Option<Block>,
    active_reasoning: Option<Block>,
    blocks: HashMap<String, Block>,
    text: HashMap<String, String>,
    raw_arguments: HashMap<String, String>,
    current_message: Option<String>,
    block_indices: HashMap<u64, Block>,
    claude_messages: HashMap<String, String>,
    claude_blocks: HashMap<(String, u64), Block>,
    acp_texts: Vec<Block>,
    pending_permissions: HashMap<String, ConversationInteraction>,
    latest_final: Option<(Block, String)>,
}

impl Decoder {
    fn boundary(&mut self) {
        self.active_text = None;
        self.active_reasoning = None;
    }

    fn block(
        &mut self,
        manifest: &NativeAuditManifest,
        raw: &ProviderEvent,
        kind: ConversationBlockKind,
        native: Option<&str>,
    ) -> Block {
        let key = native.map(|native| format!("{kind:?}:{native}"));
        if let Some(existing) = key.as_ref().and_then(|key| self.blocks.get(key)) {
            return existing.clone();
        }
        if native.is_none() {
            let active = match kind {
                ConversationBlockKind::Text => self.active_text.as_ref(),
                ConversationBlockKind::Reasoning => self.active_reasoning.as_ref(),
                _ => None,
            };
            if let Some(active) = active {
                return active.clone();
            }
        }
        let identity = match (kind, native) {
            (ConversationBlockKind::Text, Some(native)) => native.to_owned(),
            (_, Some(native)) => format!("{}:{native}", kind_name(kind)),
            (_, None) => format!(
                "anonymous:{}:{}:{}",
                kind_name(kind),
                raw.sequence,
                self.blocks.len()
            ),
        };
        let block = Block {
            id: canonical_message_id(manifest.run_attempt_id, &identity).to_string(),
            native_id: native.map(str::to_owned),
            kind,
            role: matches!(
                kind,
                ConversationBlockKind::Text | ConversationBlockKind::Reasoning
            )
            .then_some(AgentRuntimeMessageRole::Assistant),
            parent: None,
        };
        self.blocks.insert(key.unwrap_or(identity), block.clone());
        match kind {
            ConversationBlockKind::Text => {
                self.active_text = Some(block.clone());
                self.active_reasoning = None;
            }
            ConversationBlockKind::Reasoning => {
                self.active_reasoning = Some(block.clone());
                self.active_text = None;
            }
            _ => {}
        }
        block
    }

    fn emit(
        &self,
        block: &Block,
        operation: ConversationBlockOperation,
        out: &mut Vec<AgentEventPayload>,
    ) {
        out.push(AgentEventPayload::Conversation {
            update: Box::new(ConversationBlockUpdate {
                block_id: block.id.clone(),
                kind: block.kind,
                role: block.role,
                native_id: block.native_id.clone(),
                parent_block_id: block.parent.clone(),
                operation,
            }),
        });
    }

    fn bind_native(&mut self, mut block: Block, native: Option<&str>) -> Block {
        if let Some(native) = native {
            block.native_id = Some(native.into());
            self.blocks
                .insert(format!("{:?}:{native}", block.kind), block.clone());
        }
        block
    }

    fn close_interaction(
        &mut self,
        request_id: &str,
        status: crate::runtime::ConversationInteractionStatus,
        raw: &ProviderEvent,
        manifest: &NativeAuditManifest,
        out: &mut Vec<AgentEventPayload>,
    ) {
        if let Some(mut interaction) = self.pending_permissions.remove(request_id) {
            interaction.status = status;
            interaction.actionable = false;
            let block = self.block(
                manifest,
                raw,
                ConversationBlockKind::Interaction,
                Some(request_id),
            );
            self.patch(
                &block,
                ConversationBlockFields {
                    interaction: Some(interaction),
                    ..Default::default()
                },
                out,
            );
            self.finish(&block, out);
        }
    }

    fn resolve_tool_interactions(
        &mut self,
        native: Option<&str>,
        raw: &ProviderEvent,
        manifest: &NativeAuditManifest,
        out: &mut Vec<AgentEventPayload>,
    ) {
        let Some(native) = native else {
            return;
        };
        let ids: Vec<_> = self
            .pending_permissions
            .values()
            .filter(|request| {
                !request.request_id.is_empty() && request.tool_call_id.as_deref() == Some(native)
            })
            .map(|request| request.request_id.clone())
            .collect();
        for id in ids {
            self.close_interaction(
                &id,
                crate::runtime::ConversationInteractionStatus::Resolved,
                raw,
                manifest,
                out,
            );
        }
    }

    fn append(
        &mut self,
        block: &Block,
        field: ConversationTextField,
        text: String,
        out: &mut Vec<AgentEventPayload>,
    ) {
        if text.is_empty() {
            return;
        }
        if field == ConversationTextField::Text {
            self.text
                .entry(block.id.clone())
                .or_default()
                .push_str(&text);
        } else if field == ConversationTextField::RawArguments {
            self.raw_arguments
                .entry(block.id.clone())
                .or_default()
                .push_str(&text);
        }
        self.emit(
            block,
            ConversationBlockOperation::Append { field, text },
            out,
        );
    }

    fn snapshot(
        &mut self,
        block: &Block,
        fields: ConversationBlockFields,
        out: &mut Vec<AgentEventPayload>,
    ) {
        if let Some(text) = &fields.text {
            self.text.insert(block.id.clone(), text.clone());
        }
        self.emit(block, ConversationBlockOperation::Snapshot { fields }, out);
    }

    fn patch(
        &self,
        block: &Block,
        fields: ConversationBlockFields,
        out: &mut Vec<AgentEventPayload>,
    ) {
        self.emit(
            block,
            ConversationBlockOperation::Patch {
                fields,
                clear_fields: Vec::new(),
            },
            out,
        );
    }

    fn finish(&mut self, block: &Block, out: &mut Vec<AgentEventPayload>) {
        if let Some(raw) = self.raw_arguments.remove(&block.id)
            && let Ok(arguments) = serde_json::from_str::<Value>(&raw)
        {
            self.patch(
                block,
                ConversationBlockFields {
                    arguments: Some(arguments),
                    ..Default::default()
                },
                out,
            );
        }
        self.emit(
            block,
            ConversationBlockOperation::Finish { reason: None },
            out,
        );
    }

    fn final_output(
        &mut self,
        block: &Block,
        content: String,
        role: AgentRuntimeMessageRole,
        out: &mut Vec<AgentEventPayload>,
    ) {
        self.latest_final = Some((block.clone(), content.clone()));
        out.push(AgentEventPayload::Message {
            message: CanonicalMessage {
                message_id: Uuid::parse_str(&block.id).expect("decoder block ids are UUIDs"),
                role,
                content,
            },
            final_output: true,
        });
    }

    fn text_chunk(
        &mut self,
        manifest: &NativeAuditManifest,
        raw: &ProviderEvent,
        chunk: TextChunk<'_>,
        out: &mut Vec<AgentEventPayload>,
    ) -> Block {
        let kind = if chunk.reasoning {
            ConversationBlockKind::Reasoning
        } else {
            ConversationBlockKind::Text
        };
        let block = self.block(manifest, raw, kind, chunk.native_id);
        if chunk.reasoning {
            self.patch(
                &block,
                ConversationBlockFields {
                    reasoning_source: Some(ConversationReasoningSource::ExposedText),
                    ..Default::default()
                },
                out,
            );
        }
        if chunk.snapshot {
            self.snapshot(
                &block,
                ConversationBlockFields {
                    text: Some(chunk.text.into()),
                    ..Default::default()
                },
                out,
            );
        } else {
            self.append(&block, ConversationTextField::Text, chunk.text.into(), out);
        }
        block
    }
}

fn kind_name(kind: ConversationBlockKind) -> &'static str {
    match kind {
        ConversationBlockKind::Text => "text",
        ConversationBlockKind::Reasoning => "reasoning",
        ConversationBlockKind::Tool => "tool",
        ConversationBlockKind::Plan => "plan",
        ConversationBlockKind::Interaction => "interaction",
        ConversationBlockKind::Notice => "notice",
        ConversationBlockKind::Resource => "resource",
        ConversationBlockKind::Extension => "extension",
    }
}

fn string(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_owned)
}

fn tool_status(status: &str) -> Option<crate::runtime::AgentRuntimeToolStatus> {
    use crate::runtime::AgentRuntimeToolStatus as Status;
    Some(match status {
        "pending" | "created" => Status::Created,
        "in_progress" | "inProgress" | "running" => Status::Running,
        "completed" | "succeeded" | "success" => Status::Succeeded,
        "failed" | "error" => Status::Failed,
        "declined" | "denied" => Status::Denied,
        "cancelled" | "canceled" | "aborted" => Status::Cancelled,
        "waiting_approval" => Status::WaitingApproval,
        _ => return None,
    })
}

fn is_terminal(status: Option<crate::runtime::AgentRuntimeToolStatus>) -> bool {
    use crate::runtime::AgentRuntimeToolStatus as Status;
    matches!(
        status,
        Some(
            Status::Succeeded
                | Status::Failed
                | Status::Denied
                | Status::Cancelled
                | Status::TimedOut
        )
    )
}
