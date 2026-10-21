//! Bounded canonical-event previews. Failure here never changes an AgentRun.

use std::{
    collections::{HashSet, VecDeque},
    sync::OnceLock,
    time::Duration,
};

use db::models::{
    agent_run_preview::{
        AGENT_RUN_PREVIEW_CHAR_LIMIT, AGENT_RUN_PREVIEW_VERSION, AgentPreviewActivityKind,
        AgentRunActivity, AgentRunPreview, AgentRunPreviewRecord, AgentToolPreview,
        AssistantReplyPreview,
    },
    agent_runtime::subscribe_agent_event_changes,
};
use executors::runtime::{
    AgentEventEnvelope, AgentEventPayload, AgentRuntimeMessageRole, AgentRuntimeToolStatus,
    ConversationBlockKind, ConversationBlockOperation, ConversationBlockUpdate,
    ConversationInteractionKind, ConversationInteractionStatus, ConversationTextField,
};
use futures::TryStreamExt;
use sqlx::{SqlitePool, types::Json};
use tokio::{sync::broadcast, task::JoinHandle, time::MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const EVENT_BATCH: u32 = 128;
const RUNS_PER_TICK: usize = 16;
const PENDING_LIMIT: usize = 1_024;
const ACTIVITY_CHAR_LIMIT: usize = 160;

fn requested_previews() -> &'static broadcast::Sender<Uuid> {
    static REQUESTS: OnceLock<broadcast::Sender<Uuid>> = OnceLock::new();
    REQUESTS.get_or_init(|| broadcast::channel(256).0)
}

/// A read request can prioritize its missing/stale summaries without replaying
/// events or writing database state in the request's path. Hints are optional.
pub fn prioritize_agent_run_previews(agent_run_ids: &[Uuid]) {
    for id in agent_run_ids.iter().take(128) {
        let _ = requested_previews().send(*id);
    }
}

#[derive(Clone)]
pub struct AgentRunPreviewService {
    pool: SqlitePool,
}

#[derive(Debug, Clone, Copy)]
pub struct PreviewBatchResult {
    pub processed_events: usize,
    pub has_more: bool,
}

impl AgentRunPreviewService {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// One worker serializes a Run's updates; hints coalesce and a durable
    /// sweep recovers overflow, missed notifications and existing history.
    pub fn spawn(pool: SqlitePool, shutdown: CancellationToken) -> JoinHandle<()> {
        let changes = subscribe_agent_event_changes();
        let requests = requested_previews().subscribe();
        tokio::spawn(Self::new(pool).run(changes, requests, shutdown))
    }

    async fn run(
        self,
        mut changes: broadcast::Receiver<Uuid>,
        mut requests: broadcast::Receiver<Uuid>,
        shutdown: CancellationToken,
    ) {
        let mut pending = VecDeque::new();
        let mut pending_ids = HashSet::new();
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut sweep = tokio::time::interval(Duration::from_secs(2));
        sweep.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                _ = tick.tick() => {
                    // Limit total processing and at most one write per Run in
                    // this tick, even when a long history still needs replay.
                    let count = pending.len().min(RUNS_PER_TICK);
                    for _ in 0..count {
                        let Some(id) = pending.pop_front() else { break };
                        pending_ids.remove(&id);
                        let result = tokio::select! {
                            biased;
                            _ = shutdown.cancelled() => return,
                            result = self.project_batch(id, EVENT_BATCH) => result,
                        };
                        match result {
                            Ok(result) if result.has_more => {
                                enqueue(&mut pending, &mut pending_ids, id);
                            }
                            Ok(_) => {}
                            Err(error) => {
                                // Sweep retries after a delay. Never touch the
                                // canonical reducer's status or projection flag.
                                tracing::warn!(%id, %error, "Agent reply preview will retry");
                            }
                        }
                    }
                }
                _ = sweep.tick() => {
                    let result = tokio::select! {
                        biased;
                        _ = shutdown.cancelled() => return,
                        result = AgentRunPreviewRecord::pending_run_ids(&self.pool, 128) => result,
                    };
                    match result {
                        Ok(ids) => {
                            for id in ids {
                                enqueue(&mut pending, &mut pending_ids, id);
                            }
                        }
                        Err(error) => tracing::warn!(%error, "Agent reply preview sweep will retry"),
                    }
                }
                change = changes.recv() => {
                    match change {
                        Ok(id) => enqueue(&mut pending, &mut pending_ids, id),
                        // The next periodic sweep reads authoritative cursors.
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                request = requests.recv() => {
                    if let Ok(id) = request {
                        enqueue_priority(&mut pending, &mut pending_ids, id);
                    }
                }
            }
        }
    }

    /// Replay one bounded page, then atomically store the summary and cursor.
    /// Queries read persisted summaries only; they never call this method.
    pub async fn project_batch(
        &self,
        agent_run_id: Uuid,
        limit: u32,
    ) -> Result<PreviewBatchResult, sqlx::Error> {
        let previous = AgentRunPreviewRecord::find(&self.pool, agent_run_id).await?;
        let mut next = match &previous {
            Some(record) if record.algorithm_version == AGENT_RUN_PREVIEW_VERSION => record.clone(),
            _ => AgentRunPreviewRecord::empty(agent_run_id),
        };
        let limit = i64::from(limit.clamp(1, EVENT_BATCH));
        let query = sqlx::query_scalar::<_, Json<AgentEventEnvelope>>(
            r#"
            SELECT event_envelope FROM agent_events
            WHERE agent_run_id = ? AND (
                run_attempt_number > ?
                OR (run_attempt_number = ? AND sequence > ?)
            )
            ORDER BY run_attempt_number, sequence LIMIT ?
            "#,
        )
        .bind(agent_run_id)
        .bind(next.last_run_attempt_number)
        .bind(next.last_run_attempt_number)
        .bind(next.last_sequence)
        .bind(limit + 1);
        // Tool results and cumulative messages can be large. Hold only one
        // canonical event at a time instead of 129 full envelopes. Release the
        // read connection before the separate, write-only summary statement.
        let mut events = query.fetch(&self.pool);
        let mut has_more = false;
        let mut processed_events = 0;
        while let Some(event) = events.try_next().await? {
            if processed_events == limit as usize {
                has_more = true;
                break;
            }
            if apply_preview_event(&mut next, &event.0)? {
                processed_events += 1;
            }
        }
        drop(events);
        if processed_events > 0 {
            next.updated_at = chrono::Utc::now();
            if !AgentRunPreviewRecord::store_if_unchanged(&self.pool, previous.as_ref(), &next)
                .await?
            {
                // Another worker advanced or replaced this cursor. Re-read on
                // the next tick rather than overwriting a newer projection.
                return Ok(PreviewBatchResult {
                    processed_events: 0,
                    has_more: true,
                });
            }
        }
        Ok(PreviewBatchResult {
            processed_events,
            has_more,
        })
    }
}

fn enqueue(pending: &mut VecDeque<Uuid>, pending_ids: &mut HashSet<Uuid>, id: Uuid) {
    if pending.len() < PENDING_LIMIT && pending_ids.insert(id) {
        pending.push_back(id);
    }
}

fn enqueue_priority(pending: &mut VecDeque<Uuid>, pending_ids: &mut HashSet<Uuid>, id: Uuid) {
    if pending_ids.contains(&id) {
        pending.retain(|queued| *queued != id);
    } else if pending.len() >= PENDING_LIMIT
        && let Some(tail) = pending.pop_back()
    {
        pending_ids.remove(&tail);
    }
    pending_ids.insert(id);
    pending.push_front(id);
}

fn bounded(text: &str, limit: usize) -> (String, bool) {
    let mut characters = text.chars();
    let prefix = characters.by_ref().take(limit).collect();
    (prefix, characters.next().is_some())
}

fn serialized_status<T: serde::Serialize>(status: T) -> Option<String> {
    serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
}

fn activity(
    event: &AgentEventEnvelope,
    kind: AgentPreviewActivityKind,
    label: &str,
    status: Option<String>,
) -> AgentRunActivity {
    AgentRunActivity {
        kind,
        timestamp: event.timestamp,
        event_id: event.event_id,
        run_attempt_id: event.run_attempt_id,
        run_attempt_number: event.run_attempt_number,
        sequence: event.sequence,
        label: bounded(label, ACTIVITY_CHAR_LIMIT).0,
        status,
    }
}

fn tool_active(status: AgentRuntimeToolStatus) -> bool {
    matches!(
        status,
        AgentRuntimeToolStatus::Created
            | AgentRuntimeToolStatus::Running
            | AgentRuntimeToolStatus::WaitingApproval
            | AgentRuntimeToolStatus::Approved
    )
}

/// Keep only bounded display summaries. Large arguments, results and reasoning
/// remain in the conversation read model, never in dashboard preview JSON.
fn apply_conversation_preview(
    preview: &mut AgentRunPreview,
    event: &AgentEventEnvelope,
    update: &ConversationBlockUpdate,
) {
    let fields = match &update.operation {
        ConversationBlockOperation::Snapshot { fields }
        | ConversationBlockOperation::Patch { fields, .. } => Some(fields),
        _ => None,
    };
    let cleared = |name: &str| {
        matches!(&update.operation,
        ConversationBlockOperation::Patch { clear_fields, .. }
            if clear_fields.iter().any(|field| field == name))
    };
    match update.kind {
        ConversationBlockKind::Text
            if update.role.unwrap_or(AgentRuntimeMessageRole::Assistant)
                == AgentRuntimeMessageRole::Assistant =>
        {
            let same = preview.conversation_block_id.as_deref() == Some(&update.block_id)
                && preview
                    .assistant
                    .as_ref()
                    .is_some_and(|previous| previous.run_attempt_id == event.run_attempt_id);
            let replacement = fields.and_then(|fields| fields.text.as_deref());
            let appended = match &update.operation {
                ConversationBlockOperation::Append {
                    field: ConversationTextField::Text,
                    text,
                } => Some(text.as_str()),
                _ => None,
            };
            // A status-only Finish does not invent a new visible answer.
            if same || replacement.is_some() || appended.is_some() || cleared("text") {
                let (text, truncated) = if let Some(text) = replacement {
                    bounded(text, AGENT_RUN_PREVIEW_CHAR_LIMIT)
                } else if cleared("text") {
                    (String::new(), false)
                } else if let Some(text) = appended {
                    match preview.assistant.as_ref().filter(|_| same) {
                        Some(previous) if previous.truncated => (previous.text.clone(), true),
                        Some(previous) => {
                            let remaining = AGENT_RUN_PREVIEW_CHAR_LIMIT
                                .saturating_sub(previous.text.chars().count());
                            let (suffix, truncated) = bounded(text, remaining);
                            let mut merged = previous.text.clone();
                            merged.push_str(&suffix);
                            (merged, truncated)
                        }
                        None => bounded(text, AGENT_RUN_PREVIEW_CHAR_LIMIT),
                    }
                } else {
                    let previous = preview
                        .assistant
                        .as_ref()
                        .expect("same preview has an assistant");
                    (previous.text.clone(), previous.truncated)
                };
                let message_id = Uuid::parse_str(&update.block_id).unwrap_or_else(|_| {
                    preview
                        .assistant
                        .as_ref()
                        .filter(|_| same)
                        .map_or(event.event_id, |previous| previous.message_id)
                });
                preview.assistant = Some(AssistantReplyPreview {
                    message_id,
                    run_attempt_id: event.run_attempt_id,
                    run_attempt_number: event.run_attempt_number,
                    sequence: event.sequence,
                    event_id: event.event_id,
                    timestamp: event.timestamp,
                    text,
                    truncated,
                });
                preview.conversation_block_id = Some(update.block_id.clone());
            }
            preview.activity = Some(activity(event, AgentPreviewActivityKind::Message, "", None));
        }
        ConversationBlockKind::Reasoning => {
            preview.activity = Some(activity(
                event,
                AgentPreviewActivityKind::Thinking,
                "",
                None,
            ));
        }
        ConversationBlockKind::Tool => {
            let same = preview.conversation_tool_block_id.as_deref() == Some(&update.block_id)
                && preview
                    .latest_tool
                    .as_ref()
                    .is_some_and(|previous| previous.run_attempt_id == event.run_attempt_id);
            let previous = preview.latest_tool.as_ref().filter(|_| same);
            let name = fields
                .and_then(|fields| fields.tool_name.as_deref())
                .or_else(|| previous.map(|tool| tool.tool_name.as_str()))
                .unwrap_or("");
            let name = if cleared("tool_name") {
                String::new()
            } else {
                bounded(name, ACTIVITY_CHAR_LIMIT).0
            };
            let requested_status = fields.and_then(|fields| fields.status).unwrap_or_else(|| {
                previous.map_or(AgentRuntimeToolStatus::Created, |tool| tool.status)
            });
            let status = match previous {
                Some(tool) if !tool_active(tool.status) => tool.status,
                _ => requested_status,
            };
            let finished = (same && preview.conversation_tool_finished)
                || matches!(update.operation, ConversationBlockOperation::Finish { .. });
            preview.latest_tool = Some(AgentToolPreview {
                tool_name: name.clone(),
                status,
                active: !finished && tool_active(status),
                timestamp: event.timestamp,
                event_id: event.event_id,
                run_attempt_id: event.run_attempt_id,
                run_attempt_number: event.run_attempt_number,
                sequence: event.sequence,
            });
            preview.conversation_tool_block_id = Some(update.block_id.clone());
            preview.conversation_tool_finished = finished;
            preview.activity = Some(activity(
                event,
                AgentPreviewActivityKind::Tool,
                &name,
                serialized_status(status),
            ));
        }
        ConversationBlockKind::Interaction => {
            if let Some(interaction) = fields.and_then(|fields| fields.interaction.as_ref()) {
                let resolved = matches!(
                    interaction.status,
                    ConversationInteractionStatus::Resolved
                        | ConversationInteractionStatus::Cancelled
                );
                let kind = match (interaction.kind, resolved) {
                    (ConversationInteractionKind::Permission, false) => {
                        AgentPreviewActivityKind::ApprovalRequested
                    }
                    (ConversationInteractionKind::Permission, true) => {
                        AgentPreviewActivityKind::ApprovalResolved
                    }
                    (ConversationInteractionKind::Question, false) => {
                        AgentPreviewActivityKind::InputRequested
                    }
                    (ConversationInteractionKind::Question, true) => {
                        AgentPreviewActivityKind::InputResolved
                    }
                };
                preview.activity = Some(activity(
                    event,
                    kind,
                    interaction
                        .title
                        .as_deref()
                        .or(interaction.prompt.as_deref())
                        .unwrap_or(""),
                    serialized_status(interaction.status),
                ));
            }
        }
        // These carry inspectable conversation content without inventing a
        // dashboard activity type or copying their potentially large bodies.
        ConversationBlockKind::Text
        | ConversationBlockKind::Plan
        | ConversationBlockKind::Notice
        | ConversationBlockKind::Resource
        | ConversationBlockKind::Extension => {}
    }
}

fn apply_preview_event(
    record: &mut AgentRunPreviewRecord,
    event: &AgentEventEnvelope,
) -> Result<bool, sqlx::Error> {
    if event.agent_run_id != record.agent_run_id {
        return Err(sqlx::Error::Protocol(
            "preview event belongs to another Run".into(),
        ));
    }
    let attempt = i64::from(event.run_attempt_number);
    let sequence = i64::try_from(event.sequence)
        .map_err(|_| sqlx::Error::Protocol("preview event sequence exceeds SQLite range".into()))?;
    if (attempt, sequence) <= (record.last_run_attempt_number, record.last_sequence) {
        return Ok(false);
    }
    let preview = &mut record.preview.0;
    if record.last_run_attempt_id != Some(event.run_attempt_id) {
        preview.conversation_block_id = None;
        preview.conversation_tool_block_id = None;
        preview.conversation_tool_finished = false;
        preview.merge_message_id = None;
        preview.merge_role = None;
    }
    // Match canonicalAgentConversation's event-boundary rules. FileChanges
    // has no rendered conversation entry and therefore preserves adjacency.
    if !matches!(
        event.payload,
        AgentEventPayload::Message { .. } | AgentEventPayload::FileChanges { .. }
    ) {
        preview.merge_message_id = None;
        preview.merge_role = None;
    }
    match &event.payload {
        AgentEventPayload::Conversation { update } => {
            apply_conversation_preview(preview, event, update)
        }
        AgentEventPayload::Message {
            message,
            final_output,
        } => {
            if event.payload_version >= 2
                && *final_output
                && preview.conversation_block_id.is_some()
                && preview
                    .assistant
                    .as_ref()
                    .is_some_and(|previous| previous.run_attempt_id == event.run_attempt_id)
            {
                // The final output may aggregate text around tools; it is an
                // orchestration fact referencing an already rendered body.
                record.last_run_attempt_id = Some(event.run_attempt_id);
                record.last_run_attempt_number = attempt;
                record.last_sequence = sequence;
                record.last_event_id = Some(event.event_id);
                return Ok(true);
            }
            if message.role == AgentRuntimeMessageRole::Assistant {
                preview.conversation_block_id = None;
                let can_merge = preview.merge_message_id == Some(message.message_id)
                    && preview.merge_role == Some(message.role);
                let (text, truncated) = match &preview.assistant {
                    Some(previous) if can_merge && previous.truncated => {
                        // Both supported operations (append and a cumulative
                        // replacement that starts with the complete previous
                        // value) preserve its first 600 characters. We do not
                        // guess whether an incomplete prefix matches a frame.
                        (previous.text.clone(), true)
                    }
                    Some(previous) if can_merge => {
                        if message.content.starts_with(&previous.text) {
                            bounded(&message.content, AGENT_RUN_PREVIEW_CHAR_LIMIT)
                        } else {
                            // Never allocate the complete incoming long answer.
                            let mut merged = previous.text.clone();
                            let remaining =
                                AGENT_RUN_PREVIEW_CHAR_LIMIT.saturating_sub(merged.chars().count());
                            let (suffix, truncated) = bounded(&message.content, remaining);
                            merged.push_str(&suffix);
                            (merged, truncated)
                        }
                    }
                    _ => bounded(&message.content, AGENT_RUN_PREVIEW_CHAR_LIMIT),
                };
                preview.assistant = Some(AssistantReplyPreview {
                    message_id: message.message_id,
                    run_attempt_id: event.run_attempt_id,
                    run_attempt_number: event.run_attempt_number,
                    sequence: event.sequence,
                    event_id: event.event_id,
                    timestamp: event.timestamp,
                    text,
                    truncated,
                });
            }
            preview.merge_message_id = Some(message.message_id);
            preview.merge_role = Some(message.role);
            preview.activity = Some(activity(event, AgentPreviewActivityKind::Message, "", None));
        }
        AgentEventPayload::Thinking { .. } => {
            // Thinking content is deliberately not copied into the summary.
            preview.activity = Some(activity(
                event,
                AgentPreviewActivityKind::Thinking,
                "",
                None,
            ));
        }
        AgentEventPayload::ToolCall {
            tool_name, status, ..
        } => {
            preview.conversation_tool_block_id = None;
            preview.conversation_tool_finished = false;
            preview.latest_tool = Some(AgentToolPreview {
                tool_name: bounded(tool_name, ACTIVITY_CHAR_LIMIT).0,
                status: *status,
                active: matches!(
                    status,
                    AgentRuntimeToolStatus::Created
                        | AgentRuntimeToolStatus::Running
                        | AgentRuntimeToolStatus::WaitingApproval
                        | AgentRuntimeToolStatus::Approved
                ),
                timestamp: event.timestamp,
                event_id: event.event_id,
                run_attempt_id: event.run_attempt_id,
                run_attempt_number: event.run_attempt_number,
                sequence: event.sequence,
            });
            preview.activity = Some(activity(
                event,
                AgentPreviewActivityKind::Tool,
                tool_name,
                serialized_status(status),
            ));
        }
        AgentEventPayload::ApprovalRequested { tool_name, .. } => {
            preview.activity = Some(activity(
                event,
                AgentPreviewActivityKind::ApprovalRequested,
                tool_name,
                None,
            ));
        }
        AgentEventPayload::ApprovalResolved { approved, .. } => {
            preview.activity = Some(activity(
                event,
                AgentPreviewActivityKind::ApprovalResolved,
                "",
                Some(if *approved { "approved" } else { "denied" }.into()),
            ));
        }
        AgentEventPayload::InputRequested { prompt, .. } => {
            preview.activity = Some(activity(
                event,
                AgentPreviewActivityKind::InputRequested,
                prompt,
                None,
            ));
        }
        AgentEventPayload::InputResolved { answered, .. } => {
            preview.activity = Some(activity(
                event,
                AgentPreviewActivityKind::InputResolved,
                "",
                Some(if *answered { "answered" } else { "dismissed" }.into()),
            ));
        }
        AgentEventPayload::Error { error } => {
            preview.activity = Some(activity(
                event,
                AgentPreviewActivityKind::Error,
                &error.message,
                None,
            ));
        }
        AgentEventPayload::LifecycleChanged { status } => {
            if status.is_terminal()
                && let Some(tool) = &mut preview.latest_tool
            {
                tool.active = false;
                preview.conversation_tool_finished = true;
            }
            preview.activity = Some(activity(
                event,
                AgentPreviewActivityKind::Lifecycle,
                "",
                serialized_status(status),
            ));
        }
        AgentEventPayload::FileChanges { .. } => {
            preview.activity = Some(activity(
                event,
                AgentPreviewActivityKind::FileChanges,
                "",
                None,
            ));
        }
        AgentEventPayload::ProjectionDegraded { reason } => {
            preview.activity = Some(activity(
                event,
                AgentPreviewActivityKind::ProjectionDegraded,
                reason,
                None,
            ));
        }
        // Configuration/session observations, accounting and unknown provider
        // extensions advance the cursor without inventing user-visible work.
        AgentEventPayload::TokenUsage { .. }
        | AgentEventPayload::SessionObserved { .. }
        | AgentEventPayload::ProviderExtension { .. }
        | AgentEventPayload::Unknown { .. } => {}
    }
    record.last_run_attempt_id = Some(event.run_attempt_id);
    record.last_run_attempt_number = attempt;
    record.last_sequence = sequence;
    record.last_event_id = Some(event.event_id);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use executors::runtime::{
        AGENT_EVENT_PAYLOAD_VERSION, AGENT_EVENT_SCHEMA_VERSION, AgentRunStatus, CanonicalMessage,
        ConversationBlockFields,
    };
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    fn event(run: Uuid, sequence: u64, payload: AgentEventPayload) -> AgentEventEnvelope {
        AgentEventEnvelope {
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload_version: AGENT_EVENT_PAYLOAD_VERSION,
            event_id: Uuid::new_v4(),
            session_id: Uuid::from_u128(10),
            agent_run_id: run,
            turn_id: Uuid::from_u128(20),
            run_attempt_id: Uuid::from_u128(30),
            run_attempt_number: 1,
            sequence,
            correlation_id: Uuid::from_u128(40),
            orchestration_run_id: None,
            orchestration_node_execution_id: None,
            timestamp: Utc::now(),
            native_refs: Vec::new(),
            payload,
        }
    }

    fn message(run: Uuid, id: Uuid, sequence: u64, content: &str) -> AgentEventEnvelope {
        event(
            run,
            sequence,
            AgentEventPayload::Message {
                message: CanonicalMessage {
                    message_id: id,
                    role: AgentRuntimeMessageRole::Assistant,
                    content: content.into(),
                },
                final_output: false,
            },
        )
    }

    fn assistant(record: &AgentRunPreviewRecord) -> &AssistantReplyPreview {
        record
            .preview
            .0
            .assistant
            .as_ref()
            .expect("assistant preview")
    }

    #[test]
    fn rich_preview_replaces_snapshots_preserves_tool_details_and_bounds_final_output() {
        let run = Uuid::new_v4();
        let message_id = Uuid::new_v4();
        let block_id = message_id.to_string();
        let mut record = AgentRunPreviewRecord::empty(run);
        let rich = |sequence, id: &str, kind, operation| {
            event(
                run,
                sequence,
                AgentEventPayload::Conversation {
                    update: Box::new(ConversationBlockUpdate {
                        block_id: id.into(),
                        kind,
                        role: None,
                        native_id: None,
                        parent_block_id: None,
                        operation,
                    }),
                },
            )
        };
        apply_preview_event(
            &mut record,
            &rich(
                1,
                &block_id,
                ConversationBlockKind::Text,
                ConversationBlockOperation::Append {
                    field: ConversationTextField::Text,
                    text: "before ".into(),
                },
            ),
        )
        .unwrap();
        apply_preview_event(
            &mut record,
            &rich(
                2,
                "tool",
                ConversationBlockKind::Tool,
                ConversationBlockOperation::Snapshot {
                    fields: ConversationBlockFields {
                        tool_name: Some("Read".into()),
                        status: Some(AgentRuntimeToolStatus::Running),
                        arguments: Some(serde_json::json!({"path": "private-arguments"})),
                        ..Default::default()
                    },
                },
            ),
        )
        .unwrap();
        assert!(record.preview.0.latest_tool.as_ref().unwrap().active);
        apply_preview_event(
            &mut record,
            &rich(
                3,
                "tool",
                ConversationBlockKind::Tool,
                ConversationBlockOperation::Patch {
                    fields: ConversationBlockFields {
                        result: Some(serde_json::json!("private-result".repeat(10_000))),
                        ..Default::default()
                    },
                    clear_fields: Vec::new(),
                },
            ),
        )
        .unwrap();
        let tool = record.preview.0.latest_tool.as_ref().unwrap();
        assert_eq!(tool.tool_name, "Read");
        assert_eq!(tool.status, AgentRuntimeToolStatus::Running);
        apply_preview_event(
            &mut record,
            &rich(
                4,
                "tool",
                ConversationBlockKind::Tool,
                ConversationBlockOperation::Finish {
                    reason: Some("completed".into()),
                },
            ),
        )
        .unwrap();
        assert!(!record.preview.0.latest_tool.as_ref().unwrap().active);
        // A partial update after Finish neither erases the label nor restarts
        // the spinner, and Finish alone never fabricates a success status.
        apply_preview_event(
            &mut record,
            &rich(
                5,
                "tool",
                ConversationBlockKind::Tool,
                ConversationBlockOperation::Patch {
                    fields: ConversationBlockFields {
                        status: Some(AgentRuntimeToolStatus::Running),
                        ..Default::default()
                    },
                    clear_fields: Vec::new(),
                },
            ),
        )
        .unwrap();
        let tool = record.preview.0.latest_tool.as_ref().unwrap();
        assert_eq!(tool.tool_name, "Read");
        assert_eq!(tool.status, AgentRuntimeToolStatus::Running);
        assert!(!tool.active);
        apply_preview_event(
            &mut record,
            &rich(
                6,
                &block_id,
                ConversationBlockKind::Text,
                ConversationBlockOperation::Append {
                    field: ConversationTextField::Text,
                    text: "after".into(),
                },
            ),
        )
        .unwrap();
        assert_eq!(assistant(&record).text, "before after");

        let prefix = "\u{1f9ea}".repeat(AGENT_RUN_PREVIEW_CHAR_LIMIT);
        apply_preview_event(
            &mut record,
            &rich(
                7,
                &block_id,
                ConversationBlockKind::Text,
                ConversationBlockOperation::Snapshot {
                    fields: ConversationBlockFields {
                        text: Some(format!("{prefix}tail")),
                        ..Default::default()
                    },
                },
            ),
        )
        .unwrap();
        apply_preview_event(
            &mut record,
            &rich(
                8,
                &block_id,
                ConversationBlockKind::Text,
                ConversationBlockOperation::Append {
                    field: ConversationTextField::Text,
                    text: "more".into(),
                },
            ),
        )
        .unwrap();
        assert_eq!(assistant(&record).text, prefix);
        assert_eq!(assistant(&record).text.chars().count(), 600);
        assert!(assistant(&record).truncated);
        // An explicit shorter snapshot can replace an already truncated body.
        apply_preview_event(
            &mut record,
            &rich(
                9,
                &block_id,
                ConversationBlockKind::Text,
                ConversationBlockOperation::Snapshot {
                    fields: ConversationBlockFields {
                        text: Some("final answer".into()),
                        ..Default::default()
                    },
                },
            ),
        )
        .unwrap();
        apply_preview_event(
            &mut record,
            &rich(
                10,
                &block_id,
                ConversationBlockKind::Text,
                ConversationBlockOperation::Finish { reason: None },
            ),
        )
        .unwrap();
        let displayed = assistant(&record).clone();
        assert_eq!(displayed.text, "final answer");
        assert!(!displayed.truncated);
        let final_output = event(
            run,
            11,
            AgentEventPayload::Message {
                message: CanonicalMessage {
                    message_id,
                    role: AgentRuntimeMessageRole::Assistant,
                    content: "before after\nfinal answer".into(),
                },
                final_output: true,
            },
        );
        apply_preview_event(&mut record, &final_output).unwrap();
        assert_eq!(*assistant(&record), displayed);
        assert_eq!(record.last_sequence, 11);
        let summary = serde_json::to_string(&record.preview.0).unwrap();
        assert!(!summary.contains("private-arguments"));
        assert!(!summary.contains("private-result"));
    }

    #[test]
    fn cumulative_delta_unicode_and_replayed_event_match_conversation() {
        let run = Uuid::new_v4();
        let id = Uuid::new_v4();
        let mut record = AgentRunPreviewRecord::empty(run);
        let first = message(run, id, 1, "你🙂");
        apply_preview_event(&mut record, &first).unwrap();
        let cumulative = message(run, id, 2, "你🙂好");
        apply_preview_event(&mut record, &cumulative).unwrap();
        let delta = message(run, id, 3, "世界");
        apply_preview_event(&mut record, &delta).unwrap();
        assert_eq!(assistant(&record).text, "你🙂好世界");
        assert_eq!(assistant(&record).event_id, delta.event_id);
        assert_eq!(assistant(&record).timestamp, delta.timestamp);
        assert!(!assistant(&record).truncated);
        assert!(!apply_preview_event(&mut record, &delta).unwrap());
        assert!(!apply_preview_event(&mut record, &first).unwrap());
        assert_eq!(assistant(&record).text, "你🙂好世界");
    }

    #[test]
    fn long_prefix_remains_exact_for_later_cumulative_and_delta_frames() {
        let run = Uuid::new_v4();
        let id = Uuid::new_v4();
        let mut record = AgentRunPreviewRecord::empty(run);
        let prefix = "🙂".repeat(600);
        apply_preview_event(&mut record, &message(run, id, 1, &prefix)).unwrap();
        assert!(!assistant(&record).truncated);
        let cumulative = format!("{prefix}尾");
        apply_preview_event(&mut record, &message(run, id, 2, &cumulative)).unwrap();
        apply_preview_event(&mut record, &message(run, id, 3, "delta")).unwrap();
        // A shorter same-prefix frame cannot replace the full previous value
        // in canonical chat; whether append or cumulative, its prefix is exact.
        apply_preview_event(&mut record, &message(run, id, 4, "🙂")).unwrap();
        assert_eq!(assistant(&record).text, prefix);
        assert_eq!(assistant(&record).text.chars().count(), 600);
        assert!(assistant(&record).truncated);
        apply_preview_event(&mut record, &message(run, Uuid::new_v4(), 5, "new answer")).unwrap();
        assert_eq!(assistant(&record).text, "new answer");
        assert!(!assistant(&record).truncated);
    }

    #[test]
    fn event_boundaries_and_retry_cursor_match_existing_chat() {
        let run = Uuid::new_v4();
        let id = Uuid::new_v4();
        let mut record = AgentRunPreviewRecord::empty(run);
        apply_preview_event(&mut record, &message(run, id, 1, "before")).unwrap();
        let file_change = event(
            run,
            2,
            AgentEventPayload::FileChanges {
                tool_call_id: "edit".into(),
                changes: Vec::new(),
            },
        );
        apply_preview_event(&mut record, &file_change).unwrap();
        apply_preview_event(&mut record, &message(run, id, 3, "after")).unwrap();
        assert_eq!(assistant(&record).text, "beforeafter");
        let thinking = event(
            run,
            4,
            AgentEventPayload::Thinking {
                content: "private thinking".into(),
            },
        );
        apply_preview_event(&mut record, &thinking).unwrap();
        apply_preview_event(&mut record, &message(run, id, 5, "new segment")).unwrap();
        assert_eq!(assistant(&record).text, "new segment");

        let mut starting = event(
            run,
            1,
            AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Starting,
            },
        );
        starting.run_attempt_id = Uuid::from_u128(31);
        starting.run_attempt_number = 2;
        apply_preview_event(&mut record, &starting).unwrap();
        // Keep the exact previous reply/source until this attempt speaks.
        assert_eq!(assistant(&record).run_attempt_number, 1);
        let mut retried = message(run, id, 2, "retried");
        retried.run_attempt_id = starting.run_attempt_id;
        retried.run_attempt_number = 2;
        apply_preview_event(&mut record, &retried).unwrap();
        assert_eq!(assistant(&record).text, "retried");
        assert_eq!(assistant(&record).run_attempt_number, 2);
        assert_eq!(record.last_sequence, 2);
        assert_eq!(record.last_run_attempt_number, 2);
        assert!(!apply_preview_event(&mut record, &thinking).unwrap());
    }

    #[test]
    fn accounting_breaks_merge_without_becoming_activity_and_terminal_stops_tool() {
        let run = Uuid::new_v4();
        let id = Uuid::new_v4();
        let mut record = AgentRunPreviewRecord::empty(run);
        let reply = message(run, id, 1, "before");
        apply_preview_event(&mut record, &reply).unwrap();
        let usage = event(
            run,
            2,
            AgentEventPayload::TokenUsage {
                input_tokens: 1,
                output_tokens: 1,
                cached_input_tokens: None,
            },
        );
        apply_preview_event(&mut record, &usage).unwrap();
        assert_eq!(
            record.preview.0.activity.as_ref().unwrap().event_id,
            reply.event_id
        );
        apply_preview_event(&mut record, &message(run, id, 3, "after")).unwrap();
        assert_eq!(assistant(&record).text, "after");
        let tool = event(
            run,
            4,
            AgentEventPayload::ToolCall {
                tool_call_id: Some("tool".into()),
                tool_name: "Read".into(),
                status: AgentRuntimeToolStatus::Running,
                arguments: Some(serde_json::json!({"secret": "do not copy"})),
                result: Some(serde_json::json!("long tool output")),
            },
        );
        apply_preview_event(&mut record, &tool).unwrap();
        assert_eq!(assistant(&record).text, "after");
        assert!(record.preview.0.latest_tool.as_ref().unwrap().active);
        let finished = event(
            run,
            5,
            AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Succeeded,
            },
        );
        apply_preview_event(&mut record, &finished).unwrap();
        assert!(!record.preview.0.latest_tool.as_ref().unwrap().active);
        let json = serde_json::to_string(&record.preview.0).unwrap();
        assert!(!json.contains("do not copy"));
        assert!(!json.contains("long tool output"));
    }

    #[test]
    fn rendered_control_and_provider_events_break_assistant_adjacency() {
        use executors::runtime::{
            AgentRuntimeError, AgentRuntimeErrorKind, ProviderSessionReference,
        };

        let boundaries = [
            AgentEventPayload::ToolCall {
                tool_call_id: None,
                tool_name: "Read".into(),
                status: AgentRuntimeToolStatus::Succeeded,
                arguments: None,
                result: None,
            },
            AgentEventPayload::ApprovalRequested {
                approval_id: "approve".into(),
                tool_call_id: None,
                tool_name: "Read".into(),
            },
            AgentEventPayload::ApprovalResolved {
                approval_id: "approve".into(),
                approved: true,
                reason: None,
            },
            AgentEventPayload::InputRequested {
                input_id: "input".into(),
                prompt: "continue?".into(),
            },
            AgentEventPayload::InputResolved {
                input_id: "input".into(),
                answered: true,
            },
            AgentEventPayload::Error {
                error: AgentRuntimeError::new(AgentRuntimeErrorKind::Unknown, "failed"),
            },
            AgentEventPayload::ProjectionDegraded {
                reason: "unknown".into(),
            },
            AgentEventPayload::SessionObserved {
                provider_session: ProviderSessionReference {
                    schema_version: 1,
                    provider_id: "codex".into(),
                    runtime_profile_id: "CODEX:default".into(),
                    provider_session_id: "native".into(),
                    observed_at: Utc::now(),
                    metadata: None,
                },
            },
            AgentEventPayload::ProviderExtension {
                provider_namespace: "codex".into(),
                provider_event: "heartbeat".into(),
                payload: serde_json::Value::Null,
            },
            AgentEventPayload::Unknown {
                event_type: "future".into(),
                payload: serde_json::Value::Null,
            },
        ];
        for boundary in boundaries {
            let run = Uuid::new_v4();
            let id = Uuid::new_v4();
            let mut record = AgentRunPreviewRecord::empty(run);
            apply_preview_event(&mut record, &message(run, id, 1, "before")).unwrap();
            let boundary_event = event(run, 2, boundary);
            apply_preview_event(&mut record, &boundary_event).unwrap();
            apply_preview_event(&mut record, &message(run, id, 3, "after")).unwrap();
            assert_eq!(
                assistant(&record).text,
                "after",
                "boundary {:?}",
                boundary_event.payload
            );
        }
    }

    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // Existing canonical data is created before applying the new migration.
        sqlx::raw_sql(
            r#"
            PRAGMA foreign_keys = ON;
            CREATE TABLE agent_runs(id BLOB PRIMARY KEY, status TEXT NOT NULL);
            CREATE TABLE agent_run_state(
                agent_run_id BLOB PRIMARY KEY, status TEXT NOT NULL,
                last_run_attempt_number INTEGER, last_event_sequence INTEGER,
                last_event_id BLOB, updated_at TEXT NOT NULL
            );
            CREATE TABLE agent_events(
                agent_run_id BLOB, run_attempt_number INTEGER, sequence INTEGER,
                event_envelope TEXT NOT NULL,
                UNIQUE(agent_run_id, run_attempt_number, sequence)
            );
            CREATE INDEX idx_agent_events_agent_run_attempt_sequence
                ON agent_events(agent_run_id, run_attempt_number, sequence);
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../db/migrations/20261019000000_agent_run_previews.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn save_event(pool: &SqlitePool, event: &AgentEventEnvelope) {
        sqlx::query("INSERT OR IGNORE INTO agent_runs(id,status) VALUES (?,'running')")
            .bind(event.agent_run_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_events VALUES (?,?,?,?)")
            .bind(event.agent_run_id)
            .bind(i64::from(event.run_attempt_number))
            .bind(event.sequence as i64)
            .bind(Json(event))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_run_state VALUES (?,'running',?,?,?,?) ON CONFLICT(agent_run_id) DO UPDATE SET last_run_attempt_number=excluded.last_run_attempt_number,last_event_sequence=excluded.last_event_sequence,last_event_id=excluded.last_event_id")
            .bind(event.agent_run_id).bind(i64::from(event.run_attempt_number))
            .bind(event.sequence as i64).bind(event.event_id).bind(event.timestamp)
            .execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn durable_replay_recovers_restart_missing_hints_version_and_stale_writer() {
        let pool = pool().await;
        let run = Uuid::new_v4();
        let id = Uuid::new_v4();
        let events = [
            message(run, id, 1, "hello"),
            message(run, id, 2, "hello world"),
            message(run, id, 3, "!"),
        ];
        for event in &events {
            save_event(&pool, event).await;
        }
        assert_eq!(
            AgentRunPreviewRecord::pending_run_ids(&pool, 10)
                .await
                .unwrap(),
            vec![run]
        );
        let service = AgentRunPreviewService::new(pool.clone());
        assert!(service.project_batch(run, 1).await.unwrap().has_more);
        let old = AgentRunPreviewRecord::find(&pool, run)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(assistant(&old).text, "hello");
        // A fresh service has no in-memory cursor or broadcast history.
        let restarted = AgentRunPreviewService::new(pool.clone());
        assert!(!restarted.project_batch(run, 128).await.unwrap().has_more);
        let completed = AgentRunPreviewRecord::find(&pool, run)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(assistant(&completed).text, "hello world!");
        assert_eq!(completed.last_event_id, Some(events[2].event_id));
        assert!(
            AgentRunPreviewRecord::pending_run_ids(&pool, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            !AgentRunPreviewRecord::store_if_unchanged(&pool, Some(&old), &old)
                .await
                .unwrap()
        );
        assert_eq!(
            restarted
                .project_batch(run, 128)
                .await
                .unwrap()
                .processed_events,
            0
        );

        sqlx::query("UPDATE agent_run_previews SET algorithm_version = ?, preview_json = '{\"assistant\":42}'")
            .bind(AGENT_RUN_PREVIEW_VERSION + 1).execute(&pool).await.unwrap();
        assert_eq!(
            AgentRunPreviewRecord::pending_run_ids(&pool, 10)
                .await
                .unwrap(),
            vec![run]
        );
        restarted.project_batch(run, 128).await.unwrap();
        let rebuilt = AgentRunPreviewRecord::find(&pool, run)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rebuilt.algorithm_version, AGENT_RUN_PREVIEW_VERSION);
        assert_eq!(assistant(&rebuilt).text, "hello world!");

        sqlx::query("DELETE FROM agent_runs WHERE id=?")
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            AgentRunPreviewRecord::find(&pool, run)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn preview_write_failure_never_rewrites_canonical_state_and_can_retry() {
        let pool = pool().await;
        let run = Uuid::new_v4();
        save_event(&pool, &message(run, Uuid::new_v4(), 1, "reply")).await;
        sqlx::raw_sql("CREATE TRIGGER reject_preview BEFORE INSERT ON agent_run_previews BEGIN SELECT RAISE(FAIL, 'preview storage unavailable'); END;")
            .execute(&pool).await.unwrap();
        let service = AgentRunPreviewService::new(pool.clone());
        assert!(service.project_batch(run, 128).await.is_err());
        let status: String = sqlx::query_scalar("SELECT status FROM agent_runs WHERE id=?")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "running");
        assert!(
            AgentRunPreviewRecord::find(&pool, run)
                .await
                .unwrap()
                .is_none()
        );
        sqlx::query("DROP TRIGGER reject_preview")
            .execute(&pool)
            .await
            .unwrap();
        service.project_batch(run, 128).await.unwrap();
        assert_eq!(
            assistant(
                &AgentRunPreviewRecord::find(&pool, run)
                    .await
                    .unwrap()
                    .unwrap()
            )
            .text,
            "reply"
        );
    }

    #[tokio::test]
    async fn worker_replays_without_notifications_and_obeys_shutdown() {
        let pool = pool().await;
        let run = Uuid::new_v4();
        save_event(
            &pool,
            &message(run, Uuid::new_v4(), 1, "before service started"),
        )
        .await;
        let cancel = CancellationToken::new();
        let worker = AgentRunPreviewService::spawn(pool.clone(), cancel.clone());
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if AgentRunPreviewRecord::find(&pool, run)
                    .await
                    .unwrap()
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("periodic durable recovery");
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap();
    }
}
