use std::collections::HashMap;

use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;

use super::*;
use crate::runtime::{
    AgentRuntimeToolStatus, AuditBundle, NativeAuditChannel, NativeAuditMetadata, NativeAuditWriter,
};

fn transcript(provider: DirectProvider, frames: Vec<Value>) -> Vec<AgentEvent> {
    let root = tempfile::tempdir().unwrap();
    let versions = provider.versions();
    let attempt = Uuid::new_v4();
    let mut writer = NativeAuditWriter::create_in(
        root.path(),
        NativeAuditMetadata {
            session_id: Uuid::new_v4(),
            agent_run_id: Uuid::new_v4(),
            turn_id: Uuid::new_v4(),
            run_attempt_id: attempt,
            run_attempt_number: 1,
            provider_id: provider.id().into(),
            runtime_profile_id: "default".into(),
            workspace_path: root.path().display().to_string(),
            runtime_version: versions.runtime.map(str::to_owned),
            protocol_version: versions.protocol.map(str::to_owned),
            adapter_version: versions.adapter.into(),
            mapper_version: versions.mapper.into(),
            created_at: Utc::now(),
        },
    )
    .unwrap();
    for frame in frames {
        writer
            .append_native_output(
                NativeAuditChannel::Stdout,
                "application/json",
                attempt,
                serde_json::to_vec(&frame).unwrap().as_slice(),
            )
            .unwrap();
    }
    let manifest = writer.close().unwrap();
    let path = root.path().join(&manifest.manifest_relative_path);
    let bundle = AuditBundle::read(path.parent().unwrap()).unwrap();
    let mut live = StatefulProviderMapper::new(provider);
    let actual: Vec<_> = bundle
        .frames()
        .iter()
        .flat_map(|frame| live.map_frame(frame, bundle.manifest()).unwrap())
        .collect();
    let replay = provider.mapper();
    assert_eq!(actual, bundle.replay(&replay).unwrap().agent_events);
    assert_eq!(
        actual,
        bundle.replay(&replay).unwrap().agent_events,
        "reusing a replay mapper must reset its decoder"
    );
    actual
}

fn blocks(events: &[AgentEvent]) -> HashMap<String, (ConversationBlockKind, Value)> {
    let mut blocks = HashMap::new();
    for event in events {
        if let AgentEventPayload::Conversation { update } = &event.payload {
            let entry = blocks
                .entry(update.block_id.clone())
                .or_insert_with(|| (update.kind, json!({})));
            match &update.operation {
                ConversationBlockOperation::Append { field, text } => {
                    let value = entry.1[field.as_str()]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned()
                        + text;
                    entry.1[field.as_str()] = Value::String(value);
                }
                ConversationBlockOperation::Patch { fields, .. }
                | ConversationBlockOperation::Snapshot { fields } => {
                    for (name, value) in serde_json::to_value(fields).unwrap().as_object().unwrap()
                    {
                        entry.1[name] = value.clone();
                    }
                }
                ConversationBlockOperation::Finish { .. } => {}
            }
        }
    }
    blocks
}

#[test]
fn codex_deltas_and_final_snapshot_share_one_attempt_scoped_block() {
    let native = Uuid::new_v4().to_string();
    let events = transcript(
        DirectProvider::Codex,
        vec![
            json!({"method":"item/started", "params":{"item":{"type":"agentMessage","id":native,"text":""}}}),
            json!({"method":"item/agentMessage/delta", "params":{"itemId":native,"delta":"Hello "}}),
            json!({"method":"item/agentMessage/delta", "params":{"itemId":native,"delta":"world"}}),
            json!({"method":"item/completed", "params":{"item":{"type":"agentMessage","id":native,"text":"Hello world"}}}),
        ],
    );
    let visible = blocks(&events);
    assert_eq!(visible.len(), 1);
    let (id, (_, fields)) = visible.iter().next().unwrap();
    assert_ne!(id, &native, "native UUIDs also require attempt scoping");
    assert_eq!(fields["text"], "Hello world");
    let final_output = events
        .iter()
        .find_map(|event| match &event.payload {
            AgentEventPayload::Message {
                message,
                final_output: true,
            } => Some(message),
            _ => None,
        })
        .unwrap();
    assert_eq!(final_output.message_id.to_string(), *id);
    assert_eq!(final_output.content, "Hello world");
}

#[test]
fn codex_summary_and_exposed_reasoning_preserve_their_native_boundaries() {
    let events = transcript(
        DirectProvider::Codex,
        vec![
            json!({"method":"item/reasoning/summaryTextDelta","params":{"itemId":"r","summaryIndex":0,"delta":"visible summary"}}),
            json!({"method":"item/reasoning/textDelta","params":{"itemId":"r","contentIndex":0,"delta":"exposed text"}}),
            json!({"method":"item/completed","params":{"item":{"type":"reasoning","id":"r","summary":["visible summary"],"content":["exposed text"]}}}),
        ],
    );
    let visible = blocks(&events);
    assert_eq!(visible.len(), 2);
    let summary = &visible
        .values()
        .find(|(_, fields)| fields["reasoning_source"] == "summary")
        .unwrap()
        .1;
    assert_eq!(summary["text"], "visible summary");
    let exposed = &visible
        .values()
        .find(|(_, fields)| fields["reasoning_source"] == "exposed_text")
        .unwrap()
        .1;
    assert_eq!(exposed["text"], "exposed text");
}

#[test]
fn claude_nested_streams_partial_arguments_and_results_preserve_identity() {
    let stream = |parent: Option<&str>, event: Value| json!({"type":"stream_event", "parent_tool_use_id":parent, "event":event});
    let events = transcript(
        DirectProvider::ClaudeCode,
        vec![
            stream(
                None,
                json!({"type":"message_start", "message":{"id":"main-message"}}),
            ),
            stream(
                None,
                json!({"type":"content_block_start", "index":0,"content_block":{"type":"text", "text":""}}),
            ),
            stream(
                None,
                json!({"type":"content_block_delta", "index":0,"delta":{"type":"text_delta", "text":"main "}}),
            ),
            stream(
                Some("delegation"),
                json!({"type":"message_start", "message":{"id":"child-message"}}),
            ),
            stream(
                Some("delegation"),
                json!({"type":"content_block_start", "index":0,"content_block":{"type":"text", "text":"child"}}),
            ),
            stream(
                None,
                json!({"type":"content_block_delta", "index":0,"delta":{"type":"text_delta", "text":"reply"}}),
            ),
            stream(
                None,
                json!({"type":"content_block_start", "index":1,"content_block":{"type":"tool_use", "id":"read-1", "name":"Read", "input":{}}}),
            ),
            stream(
                None,
                json!({"type":"content_block_delta", "index":1,"delta":{"type":"input_json_delta", "partial_json":"{\"path\":"}}),
            ),
            stream(
                None,
                json!({"type":"content_block_delta", "index":1,"delta":{"type":"input_json_delta", "partial_json":"\"a.txt\"}"}}),
            ),
            stream(None, json!({"type":"content_block_stop", "index":1})),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"read-1","content":"file text"}]}}),
            json!({"type":"assistant", "message":{"id":"main-message", "content":[{"type":"text","text":"main reply"},{"type":"tool_use","id":"read-1","name":"Read","input":{"path":"a.txt"}}]}}),
            json!({"type":"assistant", "parent_tool_use_id":"delegation", "message":{"id":"child-message", "content":[{"type":"text","text":"child"}]}}),
            json!({"type":"result", "is_error":false, "result":"main reply"}),
        ],
    );
    let visible = blocks(&events);
    assert_eq!(visible.len(), 3);
    assert_eq!(
        visible
            .values()
            .filter(|(kind, _)| *kind == ConversationBlockKind::Text)
            .count(),
        2
    );
    let tool = &visible
        .values()
        .find(|(kind, _)| *kind == ConversationBlockKind::Tool)
        .unwrap()
        .1;
    assert_eq!(tool["raw_arguments"], "{\"path\":\"a.txt\"}");
    assert_eq!(tool["arguments"], json!({"path":"a.txt"}));
    assert_eq!(tool["result"], "file text");
    let final_output = events
        .iter()
        .rev()
        .find_map(|event| match &event.payload {
            AgentEventPayload::Message {
                message,
                final_output: true,
            } => Some(message),
            _ => None,
        })
        .unwrap();
    assert_eq!(final_output.content, "main reply");
    assert!(matches!(
        events.last().unwrap().payload,
        AgentEventPayload::LifecycleChanged {
            status: crate::runtime::AgentRunStatus::Succeeded
        }
    ));
}

#[test]
fn omp_late_tool_id_and_execution_results_update_original_block() {
    let events = transcript(
        DirectProvider::OhMyPi,
        vec![
            json!({"type":"message_start","messageId":"m"}),
            json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_start","contentIndex":0,"partial":{"content":[{"type":"toolCall","name":"read"}]}}}),
            json!({"type":"message_update","assistantMessageEvent":{"type":"toolcall_delta","contentIndex":0,"delta":"{\"path\":\"file.txt\"}","partial":{"content":[{"type":"toolCall","id":"call-1","name":"read"}]}}}),
            json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"call-1","name":"read","arguments":{"path":"file.txt"}}]}}),
            json!({"type":"tool_execution_start","toolCallId":"call-1","toolName":"read","args":{"path":"file.txt"}}),
            json!({"type":"tool_execution_update","toolCallId":"call-1","partialResult":{"content":[{"type":"text","text":"partial"}]}}),
            json!({"type":"tool_execution_end","toolCallId":"call-1","isError":false,"result":{"content":[{"type":"text","text":"complete"}]}}),
        ],
    );
    let visible = blocks(&events);
    assert_eq!(visible.len(), 1);
    let fields = &visible.values().next().unwrap().1;
    assert_eq!(fields["tool_name"], "read");
    assert_eq!(fields["arguments"], json!({"path":"file.txt"}));
    assert_eq!(fields["result"]["content"][0]["text"], "complete");
    assert_eq!(
        fields["status"],
        serde_json::to_value(AgentRuntimeToolStatus::Succeeded).unwrap()
    );
}

#[test]
fn three_acp_providers_keep_parallel_partial_tool_fields_and_final_output() {
    for provider in [
        DirectProvider::Gemini,
        DirectProvider::Opencode,
        DirectProvider::DeepseekHarness,
    ] {
        let events = transcript(
            provider,
            vec![
                json!({"Message":{"type":"text","text":"before "}}),
                json!({"ToolCall":{"toolCallId":"call-a","title":"Read file","kind":"read","status":"in_progress","rawInput":{"path":"a.txt"},"locations":[{"path":"a.txt","line":7}]}}),
                json!({"ToolCall":{"toolCallId":"call-b","title":"Run command","kind":"execute","status":"in_progress","rawInput":{"command":"pwd"}}}),
                json!({"ToolUpdate":{"toolCallId":"call-a","status":"completed","rawOutput":{"text":"a"}}}),
                json!({"ToolUpdate":{"toolCallId":"call-b","status":"failed","rawOutput":{"text":"failure"}}}),
                json!({"Message":{"type":"text","text":"after"}}),
                json!({"Thought":{"type":"text","text":"exposed reasoning"}}),
                json!({"Done":"end_turn"}),
            ],
        );
        let visible = blocks(&events);
        let a = &visible
            .values()
            .find(|(_, fields)| fields["tool_call_id"] == "call-a")
            .unwrap()
            .1;
        assert_eq!(a["tool_name"], "Read file");
        assert_eq!(a["arguments"], json!({"path":"a.txt"}));
        assert_eq!(a["locations"][0]["line"], 7);
        let output = events
            .iter()
            .find_map(|event| match &event.payload {
                AgentEventPayload::Message {
                    message,
                    final_output: true,
                } => Some(&message.content),
                _ => None,
            })
            .unwrap();
        assert_eq!(output, "before after");
        assert!(matches!(
            events.last().unwrap().payload,
            AgentEventPayload::LifecycleChanged { .. }
        ));
    }
}

#[test]
fn codex_file_evidence_does_not_replace_or_duplicate_tool_completion() {
    let events = transcript(
        DirectProvider::Codex,
        vec![json!({
            "method":"item/completed", "params":{"item":{"type":"fileChange","id":"patch-1","status":"completed",
                "changes":[{"path":"a.txt","kind":{"type":"update","move_path":null},"diff":"opaque native patch"}]}}
        })],
    );
    assert_eq!(
        blocks(&events)
            .values()
            .filter(|(kind, _)| *kind == ConversationBlockKind::Tool)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, AgentEventPayload::FileChanges { .. }))
            .count(),
        1
    );
}

#[test]
fn native_tool_completion_resolves_only_its_own_pending_permission() {
    let events = transcript(
        DirectProvider::Codex,
        vec![
            json!({"id":1,"method":"item/commandExecution/requestApproval","params":{"itemId":"tool-a","availableDecisions":["accept","decline"]}}),
            json!({"id":2,"method":"item/commandExecution/requestApproval","params":{"itemId":"tool-b","availableDecisions":["accept","decline"]}}),
            json!({"method":"item/completed","params":{"item":{"id":"tool-a","type":"commandExecution","command":"pwd","cwd":"/repo","status":"completed","exitCode":0}}}),
        ],
    );
    let visible = blocks(&events);
    let a = &visible
        .values()
        .find(|(_, fields)| fields["interaction"]["request_id"] == "codex:1")
        .unwrap()
        .1;
    let b = &visible
        .values()
        .find(|(_, fields)| fields["interaction"]["request_id"] == "codex:2")
        .unwrap()
        .1;
    assert_eq!(a["interaction"]["status"], "resolved");
    assert_eq!(b["interaction"]["status"], "pending");
}

#[test]
fn missing_tool_ids_are_never_paired_with_the_nearest_tool() {
    let events = transcript(
        DirectProvider::OhMyPi,
        vec![
            json!({"type":"tool_execution_start", "toolCallId":"known", "toolName":"read", "args":{"path":"a"}}),
            json!({"type":"tool_execution_end", "isError":false, "result":{"content":"unassociated"}}),
        ],
    );
    let visible = blocks(&events);
    assert_eq!(visible.len(), 2);
    let known = &visible
        .values()
        .find(|(_, fields)| fields["tool_call_id"] == "known")
        .unwrap()
        .1;
    assert!(known.get("result").is_none());
    assert_eq!(known["status"], "running");
}

#[test]
fn unknown_events_remain_extensions_and_do_not_degrade_known_text() {
    let events = transcript(
        DirectProvider::ClaudeCode,
        vec![json!({"type":"future_native_capability", "payload":{"opaque":true}})],
    );
    assert!(events.iter().any(|event| matches!(&event.payload,
        AgentEventPayload::ProviderExtension { payload, .. } if payload["payload"]["opaque"] == true
    )));
}
