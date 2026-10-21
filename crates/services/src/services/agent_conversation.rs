//! The conversation is a derived read model of committed canonical events.
//! No provider parsing, process control, or tool execution belongs here.
use chrono::{DateTime, Utc};
use db::models::{
    agent_conversation::{ConversationBlockRecord, ConversationSessionRecord},
    agent_runtime::AgentEventRecord,
};
use executors::runtime::{
    AgentEventEnvelope, AgentEventPayload, AgentRuntimeMessageRole, ConversationBlockFields,
    ConversationBlockKind, ConversationBlockOperation, ConversationBlockUpdate,
    ConversationInteraction, ConversationInteractionKind, ConversationInteractionStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, SqliteConnection, SqlitePool, types::Json};
use thiserror::Error;
use ts_rs::TS;
use uuid::Uuid;

use super::agent_runtime::AgentRunSummary;

const PROJECTION_BATCH_SIZE: i64 = 256;
const BODY_PREVIEW_BYTES: usize = 16 * 1024;
const BODY_FRAGMENT_BYTES: usize = 16 * 1024;
const MAX_PAGE_SIZE: u32 = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ConversationCursor {
    pub epoch: Uuid,
    /// Decimal string: JavaScript must not round a SQLite integer cursor.
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ConversationBodyReference {
    /// text/output_text/raw_arguments, or a large JSON field (e.g. result).
    pub field: String,
    #[ts(type = "number")]
    pub byte_length: u64,
    #[ts(type = "number")]
    pub preview_offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ConversationBlock {
    pub block_id: String,
    pub agent_run_id: Uuid,
    pub run_attempt_id: Uuid,
    pub kind: ConversationBlockKind,
    pub role: Option<AgentRuntimeMessageRole>,
    pub native_id: Option<String>,
    pub parent_block_id: Option<String>,
    /// Opaque first-appearance position. Updates never change this value.
    pub position: String,
    pub revision: String,
    pub fields: ConversationBlockFields,
    pub bodies: Vec<ConversationBodyReference>,
    pub finished: bool,
    pub finish_reason: Option<String>,
    pub source_event_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ConversationPage {
    pub session_id: Uuid,
    pub cursor: ConversationCursor,
    pub blocks: Vec<ConversationBlock>,
    pub runs: Vec<AgentRunSummary>,
    pub pending_interactions: Vec<ConversationBlock>,
    pub previous_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ConversationChangesPage {
    pub session_id: Uuid,
    pub cursor: ConversationCursor,
    pub blocks: Vec<ConversationBlock>,
    pub tombstones: Vec<String>,
    pub runs: Vec<AgentRunSummary>,
    pub pending_interactions: Vec<ConversationBlock>,
    pub has_more: bool,
    pub reset_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ConversationBlockDetail {
    pub block: ConversationBlock,
    pub field: String,
    pub content: String,
    #[ts(type = "number")]
    pub offset: u64,
    #[ts(type = "number | null")]
    pub next_offset: Option<u64>,
    #[ts(type = "number")]
    pub total_length: u64,
}

/// Optional live optimization only. A client must have base_revision before
/// applying this delta; changes()/block_detail() always recover a full baseline.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ConversationBlockAppend {
    pub block_id: String,
    pub base_revision: String,
    pub revision: String,
    pub field: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ConversationStreamMessage {
    Snapshot(ConversationPage),
    Changes(ConversationChangesPage),
    Append {
        session_id: Uuid,
        epoch: Uuid,
        appends: Vec<ConversationBlockAppend>,
    },
    Ready(ConversationCursor),
    Error {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct ConversationChangeNotification {
    pub session_id: Uuid,
    pub epoch: Uuid,
    pub appends: Vec<ConversationBlockAppend>,
}

fn changes_bus() -> &'static tokio::sync::broadcast::Sender<ConversationChangeNotification> {
    static BUS: std::sync::OnceLock<
        tokio::sync::broadcast::Sender<ConversationChangeNotification>,
    > = std::sync::OnceLock::new();
    BUS.get_or_init(|| tokio::sync::broadcast::channel(256).0)
}

pub fn subscribe_conversation_changes()
-> tokio::sync::broadcast::Receiver<ConversationChangeNotification> {
    changes_bus().subscribe()
}

#[derive(Debug, Error)]
pub enum ConversationReadError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error("session {0} was not found")]
    SessionNotFound(Uuid),
    #[error("conversation block {0} was not found")]
    BlockNotFound(String),
    #[error("invalid conversation cursor: {0}")]
    InvalidCursor(String),
    #[error("conversation block identity changed: {0}")]
    IdentityConflict(String),
}

#[derive(Debug, FromRow)]
struct RunSummaryRow {
    agent_run_id: Uuid,
    session_id: Uuid,
    turn_id: Uuid,
    state_json: Json<executors::runtime::RunState>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct SourceEvent {
    #[sqlx(flatten)]
    event: AgentEventRecord,
    run_created_at: DateTime<Utc>,
    projected_attempt: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct AgentConversationService<'a> {
    pool: &'a SqlitePool,
}

impl<'a> AgentConversationService<'a> {
    pub const fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn project_run(&self, agent_run_id: Uuid) -> Result<(), ConversationReadError> {
        let session =
            sqlx::query_scalar::<_, Uuid>("SELECT session_id FROM agent_runs WHERE id = ?")
                .bind(agent_run_id)
                .fetch_optional(self.pool)
                .await?;
        if let Some(session_id) = session {
            self.project_session(session_id).await?;
        }
        Ok(())
    }

    /// BEGIN IMMEDIATE serializes independent projectors before reading their
    /// checkpoint. No provider/file/network operation is performed in a tx.
    pub async fn project_session(&self, session_id: Uuid) -> Result<(), ConversationReadError> {
        loop {
            let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
            ensure_session(&mut tx, session_id).await?;
            let state = session_state(&mut tx, session_id).await?;
            let events = sqlx::query_as::<_, SourceEvent>(
                "SELECT e.*, r.created_at AS run_created_at, c.run_attempt_number AS projected_attempt
                 FROM agent_events e JOIN agent_runs r ON r.id = e.agent_run_id
                 LEFT JOIN agent_conversation_checkpoints c ON c.agent_run_id = e.agent_run_id
                 WHERE e.session_id = ? AND
                    (c.agent_run_id IS NULL OR e.run_attempt_number > c.run_attempt_number OR
                     (e.run_attempt_number = c.run_attempt_number AND e.sequence > c.sequence))
                 ORDER BY r.created_at, r.id, e.run_attempt_number, e.sequence LIMIT ?",
            )
            .bind(session_id).bind(PROJECTION_BATCH_SIZE).fetch_all(&mut *tx).await?;
            if events.is_empty() {
                tx.commit().await?;
                return Ok(());
            }
            let count = events.len();
            let mut revision = state.revision;
            let mut appends = Vec::new();
            let mut projected_attempts = std::collections::HashMap::new();
            for source in events {
                revision = revision.checked_add(1).ok_or_else(|| {
                    ConversationReadError::InvalidCursor("revision overflow".into())
                })?;
                let event = &source.event.event_envelope.0;
                let previous_attempt = projected_attempts
                    .entry(event.agent_run_id)
                    .or_insert(source.projected_attempt.unwrap_or(0));
                if i64::from(event.run_attempt_number) > *previous_attempt {
                    if event.run_attempt_number > 1 {
                        settle_previous_attempts(&mut tx, event, revision).await?;
                    }
                    *previous_attempt = i64::from(event.run_attempt_number);
                }
                for (subindex, update) in legacy_or_current_updates(event).into_iter().enumerate() {
                    if let Some(append) = apply_update(
                        &mut tx,
                        event,
                        source.run_created_at,
                        subindex,
                        revision,
                        &update,
                    )
                    .await?
                    {
                        appends.push(append);
                    }
                }
                apply_interaction_resolution(&mut tx, event, revision).await?;
                sqlx::query(
                    "INSERT INTO agent_conversation_checkpoints (agent_run_id,session_id,run_attempt_number,sequence)
                     VALUES (?,?,?,?) ON CONFLICT(agent_run_id) DO UPDATE SET
                     run_attempt_number=excluded.run_attempt_number, sequence=excluded.sequence",
                )
                .bind(event.agent_run_id).bind(session_id).bind(i64::from(event.run_attempt_number))
                .bind(source.event.sequence).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE agent_conversation_sessions SET revision = ? WHERE session_id = ?")
                .bind(revision)
                .bind(session_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            if appends
                .iter()
                .map(|append| append.text.len())
                .sum::<usize>()
                > 65536
            {
                appends.clear();
            }
            let _ = changes_bus().send(ConversationChangeNotification {
                session_id,
                epoch: state.epoch,
                appends,
            });
            if count < PROJECTION_BATCH_SIZE as usize {
                return Ok(());
            }
        }
    }

    pub async fn page(
        &self,
        session_id: Uuid,
        before: Option<&str>,
        limit: u32,
    ) -> Result<ConversationPage, ConversationReadError> {
        self.project_session(session_id).await?;
        let limit = limit.clamp(1, MAX_PAGE_SIZE);
        let mut tx = self.pool.begin().await?;
        let state = session_state(&mut tx, session_id).await?;
        let mut records = sqlx::query_as::<_, ConversationBlockRecord>(
            "SELECT * FROM agent_conversation_blocks WHERE session_id = ? AND tombstone = 0
             AND (? IS NULL OR position < ?) ORDER BY position DESC LIMIT ?",
        )
        .bind(session_id)
        .bind(before)
        .bind(before)
        .bind(i64::from(limit) + 1)
        .fetch_all(&mut *tx)
        .await?;
        let has_more = records.len() > limit as usize;
        records.truncate(limit as usize);
        records.reverse();
        let previous_cursor = records.first().map(|record| record.position.clone());
        let blocks = records
            .into_iter()
            .map(block_from_record)
            .collect::<Result<Vec<_>, _>>()?;
        let runs = run_summaries(&mut tx, session_id).await?;
        let pending_interactions = pending_interactions(&mut tx, session_id).await?;
        tx.commit().await?;
        Ok(ConversationPage {
            session_id,
            cursor: cursor(&state),
            blocks,
            runs,
            pending_interactions,
            previous_cursor,
            has_more,
        })
    }

    pub async fn changes(
        &self,
        session_id: Uuid,
        after: &ConversationCursor,
        limit: u32,
    ) -> Result<ConversationChangesPage, ConversationReadError> {
        let after_revision = parse_revision(&after.revision)?;
        self.project_session(session_id).await?;
        let mut tx = self.pool.begin().await?;
        let state = session_state(&mut tx, session_id).await?;
        let reset_required = state.epoch != after.epoch || after_revision > state.revision;
        let mut blocks = Vec::new();
        let mut tombstones = Vec::new();
        let mut covered_revision = state.revision;
        let mut has_more = false;
        if !reset_required {
            // Page by revision groups, not row count, so a multi-block event is
            // never half-acknowledged. Newer updates can reappear on later pages.
            let revisions: Vec<i64> = sqlx::query_scalar(
                "SELECT DISTINCT revision FROM agent_conversation_blocks
                 WHERE session_id = ? AND revision > ? ORDER BY revision LIMIT ?",
            )
            .bind(session_id)
            .bind(after_revision)
            .bind(i64::from(limit.clamp(1, MAX_PAGE_SIZE)) + 1)
            .fetch_all(&mut *tx)
            .await?;
            if revisions.len() > limit.clamp(1, MAX_PAGE_SIZE) as usize {
                has_more = true;
                covered_revision = revisions[limit.clamp(1, MAX_PAGE_SIZE) as usize - 1];
            }
            let records = sqlx::query_as::<_, ConversationBlockRecord>(
                "SELECT * FROM agent_conversation_blocks WHERE session_id = ?
                 AND revision > ? AND revision <= ? ORDER BY revision, position",
            )
            .bind(session_id)
            .bind(after_revision)
            .bind(covered_revision)
            .fetch_all(&mut *tx)
            .await?;
            for record in records {
                if record.tombstone {
                    tombstones.push(record.block_id);
                } else {
                    blocks.push(block_from_record(record)?);
                }
            }
        }
        let runs = run_summaries(&mut tx, session_id).await?;
        let pending_interactions = pending_interactions(&mut tx, session_id).await?;
        tx.commit().await?;
        Ok(ConversationChangesPage {
            session_id,
            cursor: ConversationCursor {
                epoch: state.epoch,
                revision: covered_revision.to_string(),
            },
            blocks,
            tombstones,
            runs,
            pending_interactions,
            has_more,
            reset_required,
        })
    }

    pub async fn block_detail(
        &self,
        session_id: Uuid,
        block_id: &str,
        field: &str,
        offset: u64,
        limit: u32,
    ) -> Result<ConversationBlockDetail, ConversationReadError> {
        self.project_session(session_id).await?;
        let mut tx = self.pool.begin().await?;
        let record = sqlx::query_as::<_, ConversationBlockRecord>(
            "SELECT * FROM agent_conversation_blocks WHERE session_id = ? AND block_id = ? AND tombstone = 0",
        ).bind(session_id).bind(block_id).fetch_optional(&mut *tx).await?
            .ok_or_else(|| ConversationReadError::BlockNotFound(block_id.to_owned()))?;
        let total_length = record.body_lengths_json.0.get(field).copied().unwrap_or(0);
        if offset > total_length {
            return Err(ConversationReadError::InvalidCursor(
                "body offset exceeds length".into(),
            ));
        }
        let limit = u64::from(limit.clamp(1, 65536));
        let end = offset.saturating_add(limit).min(total_length);
        let fragments = sqlx::query_as::<_, BodyFragment>(
            "SELECT byte_offset, content FROM agent_conversation_body_fragments
             WHERE session_id = ? AND block_id = ? AND field = ?
             AND byte_offset < ? AND byte_offset + byte_length > ? ORDER BY byte_offset",
        )
        .bind(session_id)
        .bind(block_id)
        .bind(field)
        .bind(end as i64)
        .bind(offset as i64)
        .fetch_all(&mut *tx)
        .await?;
        let (content, actual_offset) = read_fragment_range(fragments, offset, end);
        let next = actual_offset + content.len() as u64;
        let block = block_from_record(record)?;
        tx.commit().await?;
        Ok(ConversationBlockDetail {
            block,
            field: field.into(),
            content,
            offset: actual_offset,
            next_offset: (next < total_length).then_some(next),
            total_length,
        })
    }

    pub async fn interaction(
        &self,
        agent_run_id: Uuid,
        run_attempt_id: Uuid,
        request_id: &str,
    ) -> Result<Option<ConversationInteraction>, ConversationReadError> {
        Ok(self
            .interaction_block(agent_run_id, run_attempt_id, request_id)
            .await?
            .map(|(_, interaction)| interaction))
    }

    pub async fn interaction_block(
        &self,
        agent_run_id: Uuid,
        run_attempt_id: Uuid,
        request_id: &str,
    ) -> Result<Option<(String, ConversationInteraction)>, ConversationReadError> {
        self.project_run(agent_run_id).await?;
        let record = sqlx::query_as::<_, ConversationBlockRecord>(
            "SELECT * FROM agent_conversation_blocks WHERE agent_run_id = ? AND run_attempt_id = ?
             AND tombstone = 0 AND json_extract(fields_json,'$.interaction.request_id') = ? LIMIT 1",
        ).bind(agent_run_id).bind(run_attempt_id).bind(request_id).fetch_optional(self.pool).await?;
        record.map(block_from_record).transpose().map(|block| {
            block.and_then(|block| {
                block
                    .fields
                    .interaction
                    .map(|interaction| (block.block_id, interaction))
            })
        })
    }

    /// Only derived rows are cleared; canonical/audit data and execution remain
    /// untouched. Epoch forces connected readers to acquire a fresh baseline.
    pub async fn rebuild(&self, session_id: Uuid) -> Result<(), ConversationReadError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        ensure_session(&mut tx, session_id).await?;
        sqlx::query("DELETE FROM agent_conversation_blocks WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM agent_conversation_checkpoints WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        let epoch = Uuid::new_v4();
        sqlx::query(
            "UPDATE agent_conversation_sessions SET epoch = ?, revision = 0 WHERE session_id = ?",
        )
        .bind(epoch)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.project_session(session_id).await?;
        let _ = changes_bus().send(ConversationChangeNotification {
            session_id,
            epoch,
            appends: Vec::new(),
        });
        Ok(())
    }
}

#[derive(Debug, FromRow)]
struct BodyFragment {
    byte_offset: i64,
    content: String,
}

fn parse_revision(value: &str) -> Result<i64, ConversationReadError> {
    value
        .parse::<i64>()
        .ok()
        .filter(|revision| *revision >= 0)
        .ok_or_else(|| ConversationReadError::InvalidCursor(value.into()))
}

fn cursor(state: &ConversationSessionRecord) -> ConversationCursor {
    ConversationCursor {
        epoch: state.epoch,
        revision: state.revision.to_string(),
    }
}

async fn ensure_session(
    conn: &mut SqliteConnection,
    session_id: Uuid,
) -> Result<(), ConversationReadError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?)")
        .bind(session_id)
        .fetch_one(&mut *conn)
        .await?;
    if !exists {
        return Err(ConversationReadError::SessionNotFound(session_id));
    }
    sqlx::query("INSERT INTO agent_conversation_sessions(session_id,epoch) VALUES(?,?) ON CONFLICT(session_id) DO NOTHING")
        .bind(session_id).bind(Uuid::new_v4()).execute(conn).await?;
    Ok(())
}

async fn session_state(
    conn: &mut SqliteConnection,
    session_id: Uuid,
) -> Result<ConversationSessionRecord, ConversationReadError> {
    sqlx::query_as("SELECT * FROM agent_conversation_sessions WHERE session_id = ?")
        .bind(session_id)
        .fetch_optional(conn)
        .await?
        .ok_or(ConversationReadError::SessionNotFound(session_id))
}

async fn run_summaries(
    conn: &mut SqliteConnection,
    session_id: Uuid,
) -> Result<Vec<AgentRunSummary>, ConversationReadError> {
    let rows=sqlx::query_as::<_,RunSummaryRow>(
        "SELECT r.id AS agent_run_id,r.session_id,t.id AS turn_id,s.state_json,r.created_at,r.updated_at
         FROM agent_runs r JOIN agent_turns t ON t.agent_run_id=r.id
         JOIN agent_run_state s ON s.agent_run_id=r.id WHERE r.session_id = ? ORDER BY r.created_at,r.id",
    ).bind(session_id).fetch_all(conn).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            // This endpoint serves a summary, not workflow handoff output. The
            // canonical RunState remains unchanged; the answer is already a block.
            let mut state = row.state_json.0;
            state.terminal_output = None;
            AgentRunSummary {
                agent_run_id: row.agent_run_id,
                session_id: row.session_id,
                turn_id: row.turn_id,
                state,
                created_at: row.created_at,
                updated_at: row.updated_at,
            }
        })
        .collect())
}

async fn pending_interactions(
    conn: &mut SqliteConnection,
    session_id: Uuid,
) -> Result<Vec<ConversationBlock>, ConversationReadError> {
    sqlx::query_as::<_,ConversationBlockRecord>(
        "SELECT b.* FROM agent_conversation_blocks b JOIN agent_runs r ON r.id=b.agent_run_id
         JOIN agent_run_state s ON s.agent_run_id=r.id AND s.last_run_attempt_id=b.run_attempt_id
         WHERE b.session_id=? AND b.tombstone=0
         AND json_extract(b.fields_json,'$.interaction.status') IN ('pending','delivering','delivered','delivery_unknown','failed')
         AND r.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed') ORDER BY b.position",
    ).bind(session_id).fetch_all(conn).await?.into_iter().map(block_from_record).collect()
}

fn block_from_record(
    record: ConversationBlockRecord,
) -> Result<ConversationBlock, ConversationReadError> {
    let bodies = record
        .body_lengths_json
        .0
        .into_iter()
        .map(|(field, byte_length)| ConversationBodyReference {
            field,
            byte_length,
            preview_offset: 0,
        })
        .collect();
    Ok(ConversationBlock {
        block_id: record.block_id,
        agent_run_id: record.agent_run_id,
        run_attempt_id: record.run_attempt_id,
        kind: serde_json::from_value(Value::String(record.kind))?,
        role: record
            .role
            .map(|role| serde_json::from_value(Value::String(role)))
            .transpose()?,
        native_id: record.native_id,
        parent_block_id: record.parent_block_id,
        position: record.position,
        revision: record.revision.to_string(),
        fields: serde_json::from_value(record.fields_json.0)?,
        bodies,
        finished: record.finished,
        finish_reason: record.finish_reason,
        source_event_id: record.source_event_id,
        created_at: record.created_at,
        updated_at: record.updated_at,
    })
}

fn read_fragment_range(fragments: Vec<BodyFragment>, offset: u64, end: u64) -> (String, u64) {
    let mut result = String::new();
    let mut actual_offset = offset;
    for fragment in fragments {
        let mut start = offset.saturating_sub(fragment.byte_offset as u64) as usize;
        let mut stop =
            (end.saturating_sub(fragment.byte_offset as u64) as usize).min(fragment.content.len());
        start = start.min(fragment.content.len());
        while start > 0 && !fragment.content.is_char_boundary(start) {
            start -= 1;
        }
        while stop < fragment.content.len() && !fragment.content.is_char_boundary(stop) {
            stop += 1;
        }
        if result.is_empty() {
            actual_offset = fragment.byte_offset as u64 + start as u64;
        }
        if stop > start {
            result.push_str(&fragment.content[start..stop]);
        }
    }
    (result, actual_offset)
}

fn legacy_or_current_updates(event: &AgentEventEnvelope) -> Vec<ConversationBlockUpdate> {
    let snapshot = |block_id: String, kind, role, fields| ConversationBlockUpdate {
        block_id,
        kind,
        role,
        native_id: None,
        parent_block_id: None,
        operation: ConversationBlockOperation::Snapshot { fields },
    };
    let update = match &event.payload {
        AgentEventPayload::Conversation { update } => return vec![update.as_ref().clone()],
        AgentEventPayload::Message { message, .. } => snapshot(
            message.message_id.to_string(),
            ConversationBlockKind::Text,
            Some(message.role),
            ConversationBlockFields {
                text: Some(message.content.clone()),
                ..Default::default()
            },
        ),
        AgentEventPayload::Thinking { content } => snapshot(
            format!("{}:thinking:{}", event.run_attempt_id, event.event_id),
            ConversationBlockKind::Reasoning,
            None,
            ConversationBlockFields {
                text: Some(content.clone()),
                ..Default::default()
            },
        ),
        AgentEventPayload::ToolCall {
            tool_call_id,
            tool_name,
            status,
            arguments,
            result,
        } => snapshot(
            format!(
                "{}:tool:{}",
                event.run_attempt_id,
                tool_call_id
                    .clone()
                    .unwrap_or_else(|| event.event_id.to_string())
            ),
            ConversationBlockKind::Tool,
            None,
            ConversationBlockFields {
                tool_call_id: tool_call_id.clone(),
                tool_name: Some(tool_name.clone()),
                status: Some(*status),
                arguments: arguments.clone(),
                result: result.clone(),
                ..Default::default()
            },
        ),
        // New providers emit a rich Conversation interaction. V1 requests have
        // no native option/control evidence, so their preserved view is inert.
        AgentEventPayload::ApprovalRequested {
            approval_id,
            tool_call_id,
            tool_name,
        } if event.payload_version == 1 => snapshot(
            format!("{}:interaction:{approval_id}", event.run_attempt_id),
            ConversationBlockKind::Interaction,
            None,
            ConversationBlockFields {
                interaction: Some(ConversationInteraction {
                    request_id: approval_id.clone(),
                    kind: ConversationInteractionKind::Permission,
                    status: ConversationInteractionStatus::Pending,
                    title: Some(tool_name.clone()),
                    prompt: None,
                    tool_call_id: tool_call_id.clone(),
                    native_request_id: None,
                    options: Vec::new(),
                    questions: Vec::new(),
                    scope: None,
                    actionable: false,
                }),
                ..Default::default()
            },
        ),
        AgentEventPayload::InputRequested { input_id, prompt } if event.payload_version == 1 => {
            snapshot(
                format!("{}:interaction:{input_id}", event.run_attempt_id),
                ConversationBlockKind::Interaction,
                None,
                ConversationBlockFields {
                    interaction: Some(ConversationInteraction {
                        request_id: input_id.clone(),
                        kind: ConversationInteractionKind::Question,
                        status: ConversationInteractionStatus::Pending,
                        title: None,
                        prompt: Some(prompt.clone()),
                        tool_call_id: None,
                        native_request_id: None,
                        options: Vec::new(),
                        questions: Vec::new(),
                        scope: None,
                        actionable: false,
                    }),
                    ..Default::default()
                },
            )
        }
        AgentEventPayload::Error { error } => snapshot(
            format!("{}:error:{}", event.run_attempt_id, event.event_id),
            ConversationBlockKind::Notice,
            Some(AgentRuntimeMessageRole::System),
            ConversationBlockFields {
                text: Some(error.message.clone()),
                extension: Some(json!({"type":"error","error":error})),
                ..Default::default()
            },
        ),
        AgentEventPayload::ProjectionDegraded { reason } => snapshot(
            format!("{}:degraded:{}", event.run_attempt_id, event.event_id),
            ConversationBlockKind::Notice,
            Some(AgentRuntimeMessageRole::System),
            ConversationBlockFields {
                text: Some(reason.clone()),
                extension: Some(json!({"type":"projection_degraded"})),
                ..Default::default()
            },
        ),
        AgentEventPayload::ProviderExtension {
            provider_namespace,
            provider_event,
            payload,
        } if provider_event != "runtime_configuration" => snapshot(
            format!("{}:extension:{}", event.run_attempt_id, event.event_id),
            ConversationBlockKind::Extension,
            None,
            ConversationBlockFields {
                extension: Some(
                    json!({"provider_namespace":provider_namespace,"provider_event":provider_event,"payload":payload}),
                ),
                ..Default::default()
            },
        ),
        AgentEventPayload::Unknown {
            event_type,
            payload,
        } => snapshot(
            format!("{}:unknown:{}", event.run_attempt_id, event.event_id),
            ConversationBlockKind::Extension,
            None,
            ConversationBlockFields {
                extension: Some(json!({"type":event_type,"payload":payload,"unrecognized":true})),
                ..Default::default()
            },
        ),
        _ => return Vec::new(),
    };
    vec![update]
}

async fn apply_update(
    conn: &mut SqliteConnection,
    event: &AgentEventEnvelope,
    run_created_at: DateTime<Utc>,
    subindex: usize,
    revision: i64,
    update: &ConversationBlockUpdate,
) -> Result<Option<ConversationBlockAppend>, ConversationReadError> {
    let existing = sqlx::query_as::<_, ConversationBlockRecord>(
        "SELECT * FROM agent_conversation_blocks WHERE session_id=? AND block_id=?",
    )
    .bind(event.session_id)
    .bind(&update.block_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(ref existing) = existing {
        if existing.agent_run_id != event.agent_run_id
            || existing.run_attempt_id != event.run_attempt_id
        {
            return Err(ConversationReadError::IdentityConflict(
                update.block_id.clone(),
            ));
        }
        // V2 final Message is orchestration output, not a second body snapshot.
        // Its text can aggregate several native blocks around tool boundaries.
        if event.payload_version >= 2
            && matches!(
                &event.payload,
                AgentEventPayload::Message {
                    final_output: true,
                    ..
                }
            )
        {
            return Ok(None);
        }
    }
    let kind = enum_string(&update.kind)?;
    let role = update.role.as_ref().map(enum_string).transpose()?;
    let base_revision = existing.as_ref().map(|record| record.revision).unwrap_or(0);
    let position = existing
        .as_ref()
        .map(|record| record.position.clone())
        .unwrap_or_else(|| {
            format!(
                "{}|{}|{:010}|{:020}|{:04}",
                run_created_at.format("%Y-%m-%dT%H:%M:%S%.9fZ"),
                event.agent_run_id,
                event.run_attempt_number,
                event.sequence,
                subindex,
            )
        });
    // The parent row must precede body fragments for FK integrity. Its final
    // fields/cursor are still committed atomically at the end of this tx.
    sqlx::query(
        "INSERT INTO agent_conversation_blocks(session_id,block_id,agent_run_id,run_attempt_id,kind,role,
         native_id,parent_block_id,position,revision,fields_json,source_event_id,created_at,updated_at)
         VALUES(?,?,?,?,?,?,?,?,?,?,'{}',?,?,?) ON CONFLICT(session_id,block_id) DO NOTHING",
    ).bind(event.session_id).bind(&update.block_id).bind(event.agent_run_id).bind(event.run_attempt_id)
        .bind(&kind).bind(&role).bind(&update.native_id).bind(&update.parent_block_id).bind(&position).bind(revision)
        .bind(event.event_id).bind(event.timestamp).bind(event.timestamp).execute(&mut *conn).await?;
    let mut fields = existing
        .as_ref()
        .map(|record| record.fields_json.0.clone())
        .unwrap_or_else(|| json!({}));
    let mut lengths = existing
        .as_ref()
        .map(|record| record.body_lengths_json.0.clone())
        .unwrap_or_default();
    let mut finished = existing.as_ref().is_some_and(|record| record.finished);
    let mut finish_reason = existing
        .as_ref()
        .and_then(|record| record.finish_reason.clone());
    let mut append = None;
    match &update.operation {
        ConversationBlockOperation::Append { field, text } => {
            let key = field.as_str();
            let offset = lengths.get(key).copied().unwrap_or(0);
            insert_fragments(conn, event.session_id, &update.block_id, key, offset, text).await?;
            lengths.insert(key.to_owned(), offset + text.len() as u64);
            let preview = fields[key].as_str().unwrap_or("");
            if preview.len() < BODY_PREVIEW_BYTES && preview.len() as u64 == offset {
                let mut preview = preview.to_owned();
                preview.push_str(utf8_prefix(text, BODY_PREVIEW_BYTES - preview.len()));
                fields[key] = Value::String(preview);
            }
            append = Some(ConversationBlockAppend {
                block_id: update.block_id.clone(),
                base_revision: base_revision.to_string(),
                revision: revision.to_string(),
                field: key.to_owned(),
                text: text.clone(),
            });
        }
        ConversationBlockOperation::Snapshot {
            fields: replacement,
        }
        | ConversationBlockOperation::Patch {
            fields: replacement,
            ..
        } => {
            if let ConversationBlockOperation::Patch { clear_fields, .. } = &update.operation {
                for key in clear_fields {
                    if let Some(object) = fields.as_object_mut() {
                        object.remove(key);
                    }
                    lengths.remove(key);
                    delete_body(conn, event.session_id, &update.block_id, key).await?;
                }
            }
            let replacements = serde_json::to_value(replacement)?;
            if let Some(replacements) = replacements.as_object() {
                for (key, value) in replacements {
                    if key == "status"
                        && tool_terminal(fields.get(key))
                        && fields.get(key) != Some(value)
                    {
                        tracing::warn!(agent_run_id=%event.agent_run_id,block_id=%update.block_id,
                            previous=?fields.get(key),incoming=?value,"ignoring tool status after a terminal fact");
                        continue;
                    }
                    let text = if matches!(key.as_str(), "text" | "output_text" | "raw_arguments") {
                        value.as_str().map(str::to_owned)
                    } else if matches!(
                        key.as_str(),
                        "arguments" | "result" | "resource" | "extension"
                    ) {
                        let json = value.to_string();
                        (json.len() > BODY_PREVIEW_BYTES).then_some(json)
                    } else {
                        None
                    };
                    delete_body(conn, event.session_id, &update.block_id, key).await?;
                    lengths.remove(key);
                    if let Some(text) = text {
                        insert_fragments(conn, event.session_id, &update.block_id, key, 0, &text)
                            .await?;
                        lengths.insert(key.clone(), text.len() as u64);
                        if matches!(key.as_str(), "text" | "output_text" | "raw_arguments") {
                            fields[key] =
                                Value::String(utf8_prefix(&text, BODY_PREVIEW_BYTES).to_owned());
                        } else if let Some(object) = fields.as_object_mut() {
                            // Full JSON stays recoverable through the body API,
                            // rather than swelling every live block snapshot.
                            object.remove(key);
                        }
                    } else if key == "interaction" {
                        let mut interaction = value.clone();
                        let old_status = fields
                            .pointer("/interaction/status")
                            .and_then(Value::as_str);
                        let next_status = interaction.get("status").and_then(Value::as_str);
                        if matches!(old_status, Some("resolved" | "cancelled"))
                            || (matches!(old_status, Some("delivered" | "delivery_unknown"))
                                && matches!(next_status, Some("pending" | "delivering")))
                        {
                            interaction["status"] = Value::String(old_status.unwrap().into());
                            interaction["actionable"] = Value::Bool(false);
                        }
                        // Replay uses facts preceding this update, not today's
                        // RunState (which may already be terminal on rebuild).
                        let terminal:bool=sqlx::query_scalar(
                            "SELECT EXISTS(SELECT 1 FROM agent_events WHERE run_attempt_id=? AND sequence<=?
                             AND json_extract(event_envelope,'$.payload.type')='lifecycle_changed'
                             AND json_extract(event_envelope,'$.payload.data.status')
                                 IN ('succeeded','failed','cancelled','crashed','audit_failed'))",
                        ).bind(event.run_attempt_id).bind(event.sequence as i64).fetch_one(&mut *conn).await?;
                        if terminal {
                            if interaction["status"] != "resolved" {
                                interaction["status"] = json!("cancelled");
                            }
                            interaction["actionable"] = Value::Bool(false);
                        }
                        fields[key] = interaction;
                    } else {
                        fields[key] = value.clone();
                    }
                }
            }
        }
        ConversationBlockOperation::Finish { reason } => {
            finished = true;
            finish_reason = reason.clone();
        }
    }
    sqlx::query(
        "UPDATE agent_conversation_blocks SET kind=?,role=COALESCE(?,role),native_id=COALESCE(?,native_id),
         parent_block_id=COALESCE(?,parent_block_id),revision=?,fields_json=?,body_lengths_json=?,finished=?,
         finish_reason=?,source_event_id=?,updated_at=? WHERE session_id=? AND block_id=?",
    ).bind(kind).bind(role).bind(&update.native_id).bind(&update.parent_block_id).bind(revision)
        .bind(Json(fields)).bind(Json(lengths)).bind(finished).bind(finish_reason).bind(event.event_id).bind(event.timestamp)
        .bind(event.session_id).bind(&update.block_id).execute(&mut *conn).await?;
    Ok(append)
}

fn enum_string(value: &impl Serialize) -> Result<String, ConversationReadError> {
    Ok(serde_json::to_value(value)?
        .as_str()
        .unwrap_or("unknown")
        .to_owned())
}

fn tool_terminal(value: Option<&Value>) -> bool {
    matches!(
        value.and_then(Value::as_str),
        Some("succeeded" | "failed" | "denied" | "cancelled" | "timed_out")
    )
}

fn utf8_prefix(value: &str, limit: usize) -> &str {
    let mut end = limit.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

async fn delete_body(
    conn: &mut SqliteConnection,
    session_id: Uuid,
    block_id: &str,
    field: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM agent_conversation_body_fragments WHERE session_id=? AND block_id=? AND field=?")
        .bind(session_id).bind(block_id).bind(field).execute(conn).await?;
    Ok(())
}

async fn insert_fragments(
    conn: &mut SqliteConnection,
    session_id: Uuid,
    block_id: &str,
    field: &str,
    offset: u64,
    text: &str,
) -> Result<(), sqlx::Error> {
    let mut consumed = 0;
    if offset > 0 {
        let tail: Option<(i64, i64)> = sqlx::query_as(
            "SELECT byte_offset,byte_length FROM agent_conversation_body_fragments
             WHERE session_id=? AND block_id=? AND field=? AND byte_offset+byte_length=? LIMIT 1",
        )
        .bind(session_id)
        .bind(block_id)
        .bind(field)
        .bind(offset as i64)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((tail_offset, tail_length)) = tail {
            let fragment = utf8_prefix(
                text,
                BODY_FRAGMENT_BYTES.saturating_sub(tail_length as usize),
            );
            if !fragment.is_empty() {
                sqlx::query("UPDATE agent_conversation_body_fragments SET content=content||?,byte_length=byte_length+? WHERE session_id=? AND block_id=? AND field=? AND byte_offset=?")
                    .bind(fragment).bind(fragment.len() as i64).bind(session_id).bind(block_id).bind(field).bind(tail_offset).execute(&mut *conn).await?;
                consumed = fragment.len();
            }
        }
    }
    while consumed < text.len() {
        let fragment = utf8_prefix(&text[consumed..], BODY_FRAGMENT_BYTES);
        sqlx::query("INSERT INTO agent_conversation_body_fragments(session_id,block_id,field,byte_offset,content,byte_length) VALUES(?,?,?,?,?,?)")
            .bind(session_id).bind(block_id).bind(field).bind((offset+consumed as u64) as i64)
            .bind(fragment).bind(fragment.len() as i64).execute(&mut *conn).await?;
        consumed += fragment.len();
    }
    Ok(())
}

async fn apply_interaction_resolution(
    conn: &mut SqliteConnection,
    event: &AgentEventEnvelope,
    revision: i64,
) -> Result<(), ConversationReadError> {
    if let AgentEventPayload::Conversation { update } = &event.payload {
        let fields = match &update.operation {
            ConversationBlockOperation::Snapshot { fields }
            | ConversationBlockOperation::Patch { fields, .. } => Some(fields),
            _ => None,
        };
        if let Some(interaction) = fields.and_then(|fields| fields.interaction.as_ref())
            && matches!(
                interaction.status,
                ConversationInteractionStatus::Resolved | ConversationInteractionStatus::Cancelled
            )
        {
            // Read the effective projected status: a late native update cannot
            // undo a prior cancellation or reopen a resolved request.
            sqlx::query(
                "UPDATE agent_conversation_interaction_replies SET
                 delivery_status=(SELECT json_extract(fields_json,'$.interaction.status')
                     FROM agent_conversation_blocks WHERE session_id=? AND block_id=?),updated_at=?
                 WHERE agent_run_id=? AND run_attempt_id=? AND request_id=?
                 AND delivery_status IN ('pending','delivering','delivered','delivery_unknown','failed')
                 AND EXISTS(SELECT 1 FROM agent_conversation_blocks WHERE session_id=? AND block_id=?
                     AND json_extract(fields_json,'$.interaction.status') IN ('resolved','cancelled'))",
            ).bind(event.session_id).bind(&update.block_id).bind(event.timestamp)
                .bind(event.agent_run_id).bind(event.run_attempt_id).bind(&interaction.request_id)
                .bind(event.session_id).bind(&update.block_id).execute(&mut *conn).await?;
        }
        return Ok(());
    }
    let (request_id, status) = match &event.payload {
        AgentEventPayload::ApprovalResolved { approval_id, .. } => {
            (Some(approval_id.as_str()), "resolved")
        }
        AgentEventPayload::InputResolved { input_id, answered } => (
            Some(input_id.as_str()),
            if *answered { "resolved" } else { "cancelled" },
        ),
        AgentEventPayload::LifecycleChanged { status } if status.is_terminal() => {
            (None, "cancelled")
        }
        _ => return Ok(()),
    };
    sqlx::query(
        "UPDATE agent_conversation_blocks SET
         fields_json=json_set(fields_json,'$.interaction.status',?,'$.interaction.actionable',json('false')),
         revision=?,source_event_id=?,updated_at=?
         WHERE agent_run_id=? AND run_attempt_id=? AND tombstone=0
         AND json_type(fields_json,'$.interaction')='object'
         AND (? IS NULL OR json_extract(fields_json,'$.interaction.request_id')=?)
         AND json_extract(fields_json,'$.interaction.status') IN ('pending','delivering','delivered','delivery_unknown','failed')",
    ).bind(status).bind(revision).bind(event.event_id).bind(event.timestamp).bind(event.agent_run_id).bind(event.run_attempt_id)
        .bind(request_id).bind(request_id).execute(&mut *conn).await?;
    sqlx::query(
        "UPDATE agent_conversation_interaction_replies SET delivery_status=?,updated_at=?
         WHERE agent_run_id=? AND run_attempt_id=? AND (? IS NULL OR request_id=?)
         AND delivery_status IN ('pending','delivering','delivered','delivery_unknown','failed')",
    )
    .bind(status)
    .bind(event.timestamp)
    .bind(event.agent_run_id)
    .bind(event.run_attempt_id)
    .bind(request_id)
    .bind(request_id)
    .execute(&mut *conn)
    .await?;
    if request_id.is_none() {
        sqlx::query(
            "UPDATE agent_conversation_blocks SET finished=1,finish_reason=COALESCE(finish_reason,'run_terminal'),
             revision=?,source_event_id=?,updated_at=? WHERE agent_run_id=? AND run_attempt_id=? AND finished=0 AND tombstone=0",
        ).bind(revision).bind(event.event_id).bind(event.timestamp).bind(event.agent_run_id).bind(event.run_attempt_id).execute(conn).await?;
    }
    Ok(())
}

async fn settle_previous_attempts(
    conn: &mut SqliteConnection,
    event: &AgentEventEnvelope,
    revision: i64,
) -> Result<(), ConversationReadError> {
    sqlx::query(
        "UPDATE agent_conversation_blocks SET
         fields_json=json_set(fields_json,'$.interaction.status','cancelled','$.interaction.actionable',json('false')),
         revision=?,source_event_id=?,updated_at=?
         WHERE agent_run_id=? AND tombstone=0
         AND run_attempt_id IN (SELECT id FROM agent_run_attempts WHERE agent_run_id=? AND attempt_number<?)
         AND json_extract(fields_json,'$.interaction.status') IN ('pending','delivering','delivered','delivery_unknown','failed')",
    ).bind(revision).bind(event.event_id).bind(event.timestamp).bind(event.agent_run_id)
        .bind(event.agent_run_id).bind(i64::from(event.run_attempt_number)).execute(&mut *conn).await?;
    sqlx::query(
        "UPDATE agent_conversation_interaction_replies SET delivery_status='cancelled',updated_at=?
         WHERE agent_run_id=?
         AND run_attempt_id IN (SELECT id FROM agent_run_attempts WHERE agent_run_id=? AND attempt_number<?)
         AND delivery_status IN ('pending','delivering','delivered','delivery_unknown','failed')",
    ).bind(event.timestamp).bind(event.agent_run_id).bind(event.agent_run_id)
        .bind(i64::from(event.run_attempt_number)).execute(conn).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use db::models::{
        agent_conversation::ConversationInteractionReplyRecord,
        agent_runtime::{AgentRunCommandRecord, AgentRuntimePersistenceError},
    };
    use executors::runtime::{
        AGENT_EVENT_PAYLOAD_VERSION, AGENT_EVENT_SCHEMA_VERSION, AgentRunPortCommand,
        AgentRunPortCommandEnvelope, AgentRunStatus, AgentRuntimeToolStatus, CanonicalMessage,
        ConversationInteractionReply, ConversationTextField, ORCHESTRATION_COMMAND_SCHEMA_VERSION,
        ProjectionStatus, RUN_STATE_REDUCER_VERSION, RUN_STATE_SCHEMA_VERSION, RunState,
    };
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    struct History {
        pool: SqlitePool,
        session_id: Uuid,
        run_id: Uuid,
        turn_id: Uuid,
        attempt_id: Uuid,
        attempt_number: u32,
        sequence: u64,
    }

    impl History {
        async fn new() -> Self {
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();
            Self::with_pool(pool).await
        }

        async fn with_pool(pool: SqlitePool) -> Self {
            sqlx::raw_sql(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE workspaces(id BLOB PRIMARY KEY);
                 CREATE TABLE sessions(id BLOB PRIMARY KEY,workspace_id BLOB NOT NULL REFERENCES workspaces(id));
                 CREATE TABLE execution_processes(id BLOB PRIMARY KEY);",
            ).execute(&pool).await.unwrap();
            sqlx::raw_sql(include_str!(
                "../../../db/migrations/20260811000000_agent_runtime_v1.sql"
            ))
            .execute(&pool)
            .await
            .unwrap();
            sqlx::raw_sql(include_str!(
                "../../../db/migrations/20261020000000_agent_conversation.sql"
            ))
            .execute(&pool)
            .await
            .unwrap();
            let workspace_id = Uuid::new_v4();
            let session_id = Uuid::new_v4();
            let run_id = Uuid::new_v4();
            let turn_id = Uuid::new_v4();
            sqlx::query("INSERT INTO workspaces(id) VALUES(?)")
                .bind(workspace_id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO sessions(id,workspace_id) VALUES(?,?)")
                .bind(session_id)
                .bind(workspace_id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO agent_runs(id,session_id,workspace_id,request_id,idempotency_key,correlation_id,
                 schema_version,payload_version,runtime_profile_id,provider_id,workspace_mode,workspace_path,request_envelope)
                 VALUES(?,?,?,?,?,?,1,1,'CODEX:DEFAULT','codex','shared_workspace','test-workspace','{}')",
            ).bind(run_id).bind(session_id).bind(workspace_id).bind(Uuid::new_v4())
                .bind(run_id.to_string()).bind(Uuid::new_v4()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO agent_turns(id,agent_run_id,request_id,intent,input_message) VALUES(?,?,?,'initial','{}')")
                .bind(turn_id).bind(run_id).bind(Uuid::new_v4()).execute(&pool).await.unwrap();
            let state = RunState {
                state_schema_version: RUN_STATE_SCHEMA_VERSION,
                reducer_version: RUN_STATE_REDUCER_VERSION,
                session_id,
                agent_run_id: run_id,
                turn_id,
                status: AgentRunStatus::Pending,
                projection_status: ProjectionStatus::Current,
                last_run_attempt_id: None,
                last_run_attempt_number: 0,
                last_event_sequence: 0,
                last_event_id: None,
                provider_session: None,
                terminal_output: None,
                last_error: None,
                observed_configuration: None,
                pending_interactions: Vec::new(),
                closed_interaction_ids: Vec::new(),
                unknown_event_count: 0,
                updated_at: Utc::now(),
            };
            sqlx::query("INSERT INTO agent_run_state(agent_run_id,state_schema_version,reducer_version,state_json) VALUES(?,?,?,?)")
                .bind(run_id).bind(i64::from(RUN_STATE_SCHEMA_VERSION)).bind(i64::from(RUN_STATE_REDUCER_VERSION))
                .bind(Json(state)).execute(&pool).await.unwrap();
            let mut history = Self {
                pool,
                session_id,
                run_id,
                turn_id,
                attempt_id: Uuid::nil(),
                attempt_number: 0,
                sequence: 0,
            };
            history.next_attempt().await;
            history
        }

        async fn next_attempt(&mut self) {
            self.attempt_id = Uuid::new_v4();
            self.attempt_number += 1;
            self.sequence = 0;
            sqlx::query(
                "INSERT INTO agent_run_attempts(id,agent_run_id,turn_id,request_id,idempotency_key,attempt_number,
                 mode,transport,schema_version,payload_version,capability_snapshot,request_envelope)
                 VALUES(?,?,?,?,?,?,'launch','app_server_jsonrpc',1,1,'{}','{}')",
            ).bind(self.attempt_id).bind(self.run_id).bind(self.turn_id).bind(Uuid::new_v4())
                .bind(self.attempt_id.to_string()).bind(i64::from(self.attempt_number)).execute(&self.pool).await.unwrap();
        }

        fn service(&self) -> AgentConversationService<'_> {
            AgentConversationService::new(&self.pool)
        }

        async fn emit(&mut self, payload: AgentEventPayload) -> AgentEventEnvelope {
            self.sequence += 1;
            let event = AgentEventEnvelope {
                schema_version: AGENT_EVENT_SCHEMA_VERSION,
                payload_version: AGENT_EVENT_PAYLOAD_VERSION,
                event_id: Uuid::new_v4(),
                session_id: self.session_id,
                agent_run_id: self.run_id,
                turn_id: self.turn_id,
                run_attempt_id: self.attempt_id,
                run_attempt_number: self.attempt_number,
                sequence: self.sequence,
                correlation_id: self.run_id,
                orchestration_run_id: None,
                orchestration_node_execution_id: None,
                timestamp: Utc::now(),
                native_refs: Vec::new(),
                payload,
            };
            AgentEventRecord::append_and_project(&self.pool, &event)
                .await
                .unwrap();
            event
        }

        async fn block(
            &mut self,
            id: &str,
            kind: ConversationBlockKind,
            operation: ConversationBlockOperation,
        ) {
            self.emit(AgentEventPayload::Conversation {
                update: Box::new(ConversationBlockUpdate {
                    block_id: id.into(),
                    kind,
                    role: None,
                    native_id: None,
                    parent_block_id: None,
                    operation,
                }),
            })
            .await;
        }

        async fn text(&mut self, id: &str, text: &str) {
            self.block(
                id,
                ConversationBlockKind::Text,
                ConversationBlockOperation::Snapshot {
                    fields: ConversationBlockFields {
                        text: Some(text.into()),
                        ..Default::default()
                    },
                },
            )
            .await;
        }

        async fn append(&mut self, id: &str, text: &str) {
            self.block(
                id,
                ConversationBlockKind::Text,
                ConversationBlockOperation::Append {
                    field: ConversationTextField::Text,
                    text: text.into(),
                },
            )
            .await;
        }

        async fn request(&mut self, status: ConversationInteractionStatus) {
            self.block(
                "permission",
                ConversationBlockKind::Interaction,
                ConversationBlockOperation::Snapshot {
                    fields: ConversationBlockFields {
                        interaction: Some(permission(status)),
                        ..Default::default()
                    },
                },
            )
            .await;
        }
    }

    fn permission(status: ConversationInteractionStatus) -> ConversationInteraction {
        ConversationInteraction {
            request_id: "native-request".into(),
            kind: ConversationInteractionKind::Permission,
            status,
            title: Some("Run a command".into()),
            prompt: None,
            tool_call_id: Some("tool-1".into()),
            native_request_id: Some(json!(42)),
            options: Vec::new(),
            questions: Vec::new(),
            scope: None,
            actionable: matches!(status, ConversationInteractionStatus::Pending),
        }
    }

    fn semantic_blocks(blocks: &[ConversationBlock]) -> Value {
        let mut blocks = serde_json::to_value(blocks).unwrap();
        for block in blocks.as_array_mut().unwrap() {
            block.as_object_mut().unwrap().remove("revision");
        }
        blocks
    }

    #[tokio::test]
    async fn snapshots_final_output_and_rebuild_share_one_visible_body() {
        let mut history = History::new().await;
        let message_id = Uuid::new_v4();
        let block_id = message_id.to_string();
        history.append(&block_id, "hel").await;
        history.append(&block_id, "lo").await;
        history.text(&block_id, "hello").await;
        history
            .emit(AgentEventPayload::Message {
                message: CanonicalMessage {
                    message_id,
                    role: AgentRuntimeMessageRole::Assistant,
                    content: "earlier block\nhello".into(),
                },
                final_output: true,
            })
            .await;
        history
            .emit(AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Succeeded,
            })
            .await;
        let first = history
            .service()
            .page(history.session_id, None, 50)
            .await
            .unwrap();
        assert_eq!(first.blocks.len(), 1);
        assert_eq!(first.blocks[0].fields.text.as_deref(), Some("hello"));
        assert!(first.blocks[0].finished);
        assert!(first.runs[0].state.terminal_output.is_none());
        let stored: Json<RunState> =
            sqlx::query_scalar("SELECT state_json FROM agent_run_state WHERE agent_run_id=?")
                .bind(history.run_id)
                .fetch_one(&history.pool)
                .await
                .unwrap();
        assert_eq!(
            stored.0.terminal_output.unwrap().content,
            "earlier block\nhello"
        );
        let repeated = history
            .service()
            .page(history.session_id, None, 50)
            .await
            .unwrap();
        assert_eq!(first.cursor, repeated.cursor);
        assert_eq!(
            semantic_blocks(&first.blocks),
            semantic_blocks(&repeated.blocks)
        );
        history.service().rebuild(history.session_id).await.unwrap();
        let rebuilt = history
            .service()
            .page(history.session_id, None, 50)
            .await
            .unwrap();
        assert_ne!(first.cursor.epoch, rebuilt.cursor.epoch);
        assert_eq!(
            semantic_blocks(&first.blocks),
            semantic_blocks(&rebuilt.blocks)
        );
        assert!(
            history
                .service()
                .changes(history.session_id, &first.cursor, 50)
                .await
                .unwrap()
                .reset_required
        );
    }

    #[tokio::test]
    async fn tool_patches_retain_fields_nulls_and_terminal_status() {
        let mut history = History::new().await;
        history
            .block(
                "tool",
                ConversationBlockKind::Tool,
                ConversationBlockOperation::Snapshot {
                    fields: ConversationBlockFields {
                        tool_name: Some("shell".into()),
                        tool_call_id: Some("native-call".into()),
                        status: Some(AgentRuntimeToolStatus::Running),
                        arguments: Some(json!({"command":"pwd"})),
                        raw_arguments: Some("{\"command\":\"pwd\"}".into()),
                        ..Default::default()
                    },
                },
            )
            .await;
        history
            .block(
                "tool",
                ConversationBlockKind::Tool,
                ConversationBlockOperation::Patch {
                    fields: ConversationBlockFields {
                        status: Some(AgentRuntimeToolStatus::Succeeded),
                        result: Some(Value::Null),
                        ..Default::default()
                    },
                    clear_fields: vec!["raw_arguments".into()],
                },
            )
            .await;
        history
            .block(
                "tool",
                ConversationBlockKind::Tool,
                ConversationBlockOperation::Patch {
                    fields: ConversationBlockFields {
                        status: Some(AgentRuntimeToolStatus::Running),
                        output_text: Some("late output".into()),
                        ..Default::default()
                    },
                    clear_fields: Vec::new(),
                },
            )
            .await;
        let page = history
            .service()
            .page(history.session_id, None, 50)
            .await
            .unwrap();
        let fields = &page.blocks[0].fields;
        assert_eq!(fields.tool_name.as_deref(), Some("shell"));
        assert_eq!(fields.arguments, Some(json!({"command":"pwd"})));
        assert_eq!(fields.status, Some(AgentRuntimeToolStatus::Succeeded));
        assert_eq!(fields.result, Some(Value::Null));
        assert!(fields.raw_arguments.is_none());
        assert_eq!(fields.output_text.as_deref(), Some("late output"));
        assert!(
            !page.blocks[0]
                .bodies
                .iter()
                .any(|body| body.field == "raw_arguments")
        );
    }

    #[tokio::test]
    async fn projection_conflict_rolls_back_blocks_checkpoints_and_revision() {
        let mut history = History::new().await;
        let empty = history
            .service()
            .page(history.session_id, None, 50)
            .await
            .unwrap();
        history.text("reused-id", "attempt one").await;
        history.next_attempt().await;
        history.text("reused-id", "attempt two").await;
        assert!(matches!(
            history.service().project_session(history.session_id).await,
            Err(ConversationReadError::IdentityConflict(_))
        ));
        let mut conn = history.pool.acquire().await.unwrap();
        assert_eq!(
            session_state(&mut conn, history.session_id)
                .await
                .unwrap()
                .revision,
            0
        );
        assert_eq!(
            session_state(&mut conn, history.session_id)
                .await
                .unwrap()
                .epoch,
            empty.cursor.epoch
        );
        let counts:(i64,i64,i64)=sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM agent_conversation_blocks),
             (SELECT COUNT(*) FROM agent_conversation_checkpoints),(SELECT COUNT(*) FROM agent_events)",
        ).fetch_one(&mut *conn).await.unwrap();
        assert_eq!(counts, (0, 0, 2));
    }

    #[tokio::test]
    async fn independent_projector_waits_for_writer_then_reuses_the_committed_checkpoint() {
        use std::time::Duration;

        use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};

        let directory = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(directory.path().join("conversation.sqlite"))
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(2));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        let mut history = History::with_pool(pool.clone()).await;
        history.append("concurrent", "once").await;
        let writer = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let session_id = history.session_id;
        let other_pool = pool.clone();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let mut projection = tokio::spawn(async move {
            let _ = started.send(());
            AgentConversationService::new(&other_pool)
                .project_session(session_id)
                .await
        });
        waiting.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut projection)
                .await
                .is_err()
        );
        writer.commit().await.unwrap();
        projection.await.unwrap().unwrap();
        // Independent consumers must observe the checkpoint under the same
        // write lock, so a racing/restarted projection never appends twice.
        let reader = history.service();
        let other_reader = AgentConversationService::new(&pool);
        let (first, second) = tokio::join!(
            reader.project_session(session_id),
            other_reader.project_session(session_id)
        );
        first.unwrap();
        second.unwrap();
        let detail = history
            .service()
            .block_detail(session_id, "concurrent", "text", 0, 100)
            .await
            .unwrap();
        assert_eq!(detail.content, "once");
        assert_eq!(detail.block.revision, "1");
        pool.close().await;
    }

    #[tokio::test]
    async fn stable_backward_pages_and_old_block_changes_do_not_drop_revision_groups() {
        let mut history = History::new().await;
        for index in 0..5 {
            history
                .text(&format!("block-{index}"), &index.to_string())
                .await;
        }
        let tail = history
            .service()
            .page(history.session_id, None, 2)
            .await
            .unwrap();
        assert_eq!(
            tail.blocks
                .iter()
                .map(|block| block.block_id.as_str())
                .collect::<Vec<_>>(),
            vec!["block-3", "block-4"]
        );
        let older = history
            .service()
            .page(history.session_id, tail.previous_cursor.as_deref(), 2)
            .await
            .unwrap();
        assert_eq!(
            older
                .blocks
                .iter()
                .map(|block| block.block_id.as_str())
                .collect::<Vec<_>>(),
            vec!["block-1", "block-2"]
        );
        assert!(older.has_more);
        history.append("block-0", " updated").await;
        let changed = history
            .service()
            .changes(history.session_id, &tail.cursor, 1)
            .await
            .unwrap();
        assert_eq!(changed.blocks.len(), 1);
        assert_eq!(changed.blocks[0].block_id, "block-0");
        assert_eq!(changed.blocks[0].fields.text.as_deref(), Some("0 updated"));
        history
            .emit(AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Cancelled,
            })
            .await;
        let terminal = history
            .service()
            .changes(history.session_id, &changed.cursor, 1)
            .await
            .unwrap();
        assert_eq!(
            terminal.blocks.len(),
            5,
            "all blocks changed by one event share one revision"
        );
        assert!(
            terminal
                .blocks
                .iter()
                .all(|block| block.finished && block.revision == terminal.cursor.revision)
        );
        assert!(!terminal.has_more);
        assert_eq!(tail.blocks[0].position, terminal.blocks[3].position);
    }

    #[tokio::test]
    async fn updated_block_reappears_after_an_earlier_changes_page() {
        let mut history = History::new().await;
        let initial = history
            .service()
            .page(history.session_id, None, 50)
            .await
            .unwrap();
        for id in ["a", "b", "c"] {
            history.text(id, id).await;
        }
        let first = history
            .service()
            .changes(history.session_id, &initial.cursor, 1)
            .await
            .unwrap();
        assert_eq!(first.blocks[0].block_id, "a");
        history.append("a", " again").await;
        let second = history
            .service()
            .changes(history.session_id, &first.cursor, 1)
            .await
            .unwrap();
        assert_eq!(second.blocks[0].block_id, "b");
        let third = history
            .service()
            .changes(history.session_id, &second.cursor, 1)
            .await
            .unwrap();
        assert_eq!(third.blocks[0].block_id, "c");
        let fourth = history
            .service()
            .changes(history.session_id, &third.cursor, 1)
            .await
            .unwrap();
        assert_eq!(fourth.blocks[0].block_id, "a");
        assert_eq!(fourth.blocks[0].fields.text.as_deref(), Some("a again"));
        assert!(!fourth.has_more);
    }

    #[tokio::test]
    async fn long_unicode_bodies_and_large_json_load_in_bounded_segments() {
        let mut history = History::new().await;
        let head = "a".repeat(BODY_PREVIEW_BYTES - 1);
        history.text("long", &head).await;
        history.append("long", "🧪").await;
        history.append("long", "b").await;
        let rest = "汉字🧪\n".repeat(5000);
        history.append("long", &rest).await;
        let expected = format!("{head}🧪b{rest}");
        let json_result = json!({"output":"x".repeat(BODY_PREVIEW_BYTES*2)});
        history
            .block(
                "large-json",
                ConversationBlockKind::Tool,
                ConversationBlockOperation::Snapshot {
                    fields: ConversationBlockFields {
                        result: Some(json_result.clone()),
                        ..Default::default()
                    },
                },
            )
            .await;
        let page = history
            .service()
            .page(history.session_id, None, 50)
            .await
            .unwrap();
        assert_eq!(
            page.blocks[0].fields.text.as_deref(),
            Some(head.as_str()),
            "preview must stay a contiguous UTF-8 prefix"
        );
        assert!(page.blocks[1].fields.result.is_none());
        let mut offset = 0;
        let mut loaded = String::new();
        loop {
            let detail = history
                .service()
                .block_detail(history.session_id, "long", "text", offset, 1021)
                .await
                .unwrap();
            assert_eq!(detail.offset, offset);
            assert!(
                detail.content.len() <= 1024,
                "UTF-8 boundary expansion is at most three bytes"
            );
            loaded.push_str(&detail.content);
            match detail.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
        assert_eq!(loaded, expected);
        let json_detail = history
            .service()
            .block_detail(history.session_id, "large-json", "result", 0, 65536)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&json_detail.content).unwrap(),
            json_result
        );
        let fragments: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_conversation_body_fragments WHERE block_id='long'",
        )
        .fetch_one(&history.pool)
        .await
        .unwrap();
        assert!(fragments <= 6);
    }

    #[tokio::test]
    async fn resolved_interaction_cannot_reopen_and_rebuild_preserves_its_history() {
        let mut history = History::new().await;
        history
            .request(ConversationInteractionStatus::Pending)
            .await;
        let pending = history
            .service()
            .page(history.session_id, None, 50)
            .await
            .unwrap();
        assert_eq!(pending.pending_interactions.len(), 1);
        let reply_id = Uuid::new_v4();
        let payload = json!({"request_id":"native-request","option_id":"allow"});
        ConversationInteractionReplyRecord::claim(
            &history.pool,
            history.session_id,
            history.run_id,
            history.attempt_id,
            "native-request",
            reply_id,
            &payload,
        )
        .await
        .unwrap();
        history
            .request(ConversationInteractionStatus::Resolved)
            .await;
        history
            .service()
            .project_session(history.session_id)
            .await
            .unwrap();
        assert_eq!(
            ConversationInteractionReplyRecord::for_request(
                &history.pool,
                history.attempt_id,
                "native-request"
            )
            .await
            .unwrap()
            .unwrap()
            .delivery_status,
            "resolved"
        );
        history
            .request(ConversationInteractionStatus::Delivered)
            .await;
        history
            .emit(AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Succeeded,
            })
            .await;
        history
            .request(ConversationInteractionStatus::Pending)
            .await;
        let live = history
            .service()
            .page(history.session_id, None, 50)
            .await
            .unwrap();
        assert!(live.pending_interactions.is_empty());
        let interaction = live.blocks[0].fields.interaction.as_ref().unwrap();
        assert_eq!(interaction.status, ConversationInteractionStatus::Resolved);
        assert!(!interaction.actionable);
        history.service().rebuild(history.session_id).await.unwrap();
        let rebuilt = history
            .service()
            .page(history.session_id, None, 50)
            .await
            .unwrap();
        assert_eq!(
            semantic_blocks(&live.blocks),
            semantic_blocks(&rebuilt.blocks)
        );
    }

    #[tokio::test]
    async fn old_attempt_requests_are_not_pending_after_retry() {
        let mut history = History::new().await;
        history
            .request(ConversationInteractionStatus::Pending)
            .await;
        assert_eq!(
            history
                .service()
                .page(history.session_id, None, 50)
                .await
                .unwrap()
                .pending_interactions
                .len(),
            1
        );
        history.next_attempt().await;
        history
            .emit(AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Running,
            })
            .await;
        let page = history
            .service()
            .page(history.session_id, None, 50)
            .await
            .unwrap();
        assert_eq!(page.blocks.len(), 1);
        assert!(page.pending_interactions.is_empty());
        assert_eq!(
            page.blocks[0].fields.interaction.as_ref().unwrap().status,
            ConversationInteractionStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn semantic_stats_count_blocks_and_requests_once_across_updates_and_attempts() {
        use crate::services::agent_runtime::AgentRuntimeReadService;

        let mut history = History::new().await;
        let message_id = Uuid::new_v4();
        let text_id = message_id.to_string();
        history.text(&text_id, "first").await;
        for token in [" second", " third"] {
            history.append(&text_id, token).await;
        }
        history
            .block(
                "tool",
                ConversationBlockKind::Tool,
                ConversationBlockOperation::Snapshot {
                    fields: ConversationBlockFields {
                        tool_name: Some("Read".into()),
                        status: Some(AgentRuntimeToolStatus::Running),
                        ..Default::default()
                    },
                },
            )
            .await;
        for token in ["one", "two", "three"] {
            history
                .block(
                    "tool",
                    ConversationBlockKind::Tool,
                    ConversationBlockOperation::Append {
                        field: ConversationTextField::OutputText,
                        text: token.into(),
                    },
                )
                .await;
        }
        history
            .block(
                "tool",
                ConversationBlockKind::Tool,
                ConversationBlockOperation::Patch {
                    fields: ConversationBlockFields {
                        status: Some(AgentRuntimeToolStatus::Succeeded),
                        ..Default::default()
                    },
                    clear_fields: Vec::new(),
                },
            )
            .await;
        history
            .block(
                "reasoning",
                ConversationBlockKind::Reasoning,
                ConversationBlockOperation::Append {
                    field: ConversationTextField::Text,
                    text: "consider".into(),
                },
            )
            .await;
        history
            .block(
                "reasoning",
                ConversationBlockKind::Reasoning,
                ConversationBlockOperation::Append {
                    field: ConversationTextField::Text,
                    text: " options".into(),
                },
            )
            .await;
        history
            .emit(AgentEventPayload::Message {
                message: CanonicalMessage {
                    message_id,
                    role: AgentRuntimeMessageRole::Assistant,
                    content: "first second third".into(),
                },
                final_output: true,
            })
            .await;
        for status in [
            ConversationInteractionStatus::Pending,
            ConversationInteractionStatus::Delivering,
            ConversationInteractionStatus::Delivered,
        ] {
            history.request(status).await;
        }
        let delivered = AgentRuntimeReadService::new(&history.pool)
            .stats(history.run_id)
            .await
            .unwrap();
        assert_eq!(delivered.message_count, 1);
        assert_eq!(delivered.thinking_count, 1);
        assert_eq!(delivered.tool_call_count, 1);
        assert_eq!(delivered.approval_request_count, 1);
        assert_eq!(
            delivered.approval_resolution_count, 0,
            "transport delivery is not native resolution"
        );
        for status in [
            ConversationInteractionStatus::Resolved,
            ConversationInteractionStatus::Resolved,
        ] {
            history.request(status).await;
        }
        let question = |status| ConversationInteraction {
            request_id: "question".into(),
            kind: ConversationInteractionKind::Question,
            status,
            title: None,
            prompt: Some("Which option?".into()),
            tool_call_id: None,
            native_request_id: None,
            options: Vec::new(),
            questions: Vec::new(),
            scope: None,
            actionable: status == ConversationInteractionStatus::Pending,
        };
        for status in [
            ConversationInteractionStatus::Pending,
            ConversationInteractionStatus::Delivering,
            ConversationInteractionStatus::DeliveryUnknown,
            ConversationInteractionStatus::Cancelled,
        ] {
            history
                .block(
                    "question",
                    ConversationBlockKind::Interaction,
                    ConversationBlockOperation::Patch {
                        fields: ConversationBlockFields {
                            interaction: Some(question(status)),
                            ..Default::default()
                        },
                        clear_fields: Vec::new(),
                    },
                )
                .await;
        }
        let resolved = AgentRuntimeReadService::new(&history.pool)
            .stats(history.run_id)
            .await
            .unwrap();
        assert_eq!(resolved.approval_request_count, 1);
        assert_eq!(resolved.approval_resolution_count, 1);
        assert_eq!(resolved.input_request_count, 1);
        assert_eq!(resolved.input_resolution_count, 1);

        // Legacy histories can reuse native message/tool/request identities on
        // retry. Their counts remain distinct because attempt owns identity.
        history.next_attempt().await;
        history
            .emit(AgentEventPayload::Message {
                message: CanonicalMessage {
                    message_id,
                    role: AgentRuntimeMessageRole::Assistant,
                    content: "retry".into(),
                },
                final_output: false,
            })
            .await;
        for status in [
            AgentRuntimeToolStatus::Running,
            AgentRuntimeToolStatus::Succeeded,
        ] {
            history
                .emit(AgentEventPayload::ToolCall {
                    tool_call_id: Some("tool".into()),
                    tool_name: "Read".into(),
                    status,
                    arguments: None,
                    result: None,
                })
                .await;
        }
        history
            .emit(AgentEventPayload::ApprovalRequested {
                approval_id: "native-request".into(),
                tool_call_id: Some("tool".into()),
                tool_name: "Read".into(),
            })
            .await;
        let retried = AgentRuntimeReadService::new(&history.pool)
            .stats(history.run_id)
            .await
            .unwrap();
        assert_eq!(retried.message_count, 2);
        assert_eq!(retried.tool_call_count, 2);
        assert_eq!(retried.approval_request_count, 2);
        assert_eq!(retried.approval_resolution_count, 1);
        assert_eq!(retried.input_request_count, 1);
        assert_eq!(retried.input_resolution_count, 1);
        assert_eq!(retried.event_count, resolved.event_count + 4);
    }

    #[tokio::test]
    async fn reply_claim_is_exact_and_restart_never_retries_an_uncertain_answer() {
        let mut history = History::new().await;
        history
            .request(ConversationInteractionStatus::Pending)
            .await;
        let reply_id = Uuid::new_v4();
        let reply = ConversationInteractionReply {
            request_id: "native-request".into(),
            option_id: Some("allow".into()),
            answers: Vec::new(),
            text: None,
        };
        let payload = serde_json::to_value(&reply).unwrap();
        let (_, claimed) = ConversationInteractionReplyRecord::claim(
            &history.pool,
            history.session_id,
            history.run_id,
            history.attempt_id,
            "native-request",
            reply_id,
            &payload,
        )
        .await
        .unwrap();
        assert!(claimed);
        let (_, claimed) = ConversationInteractionReplyRecord::claim(
            &history.pool,
            history.session_id,
            history.run_id,
            history.attempt_id,
            "native-request",
            reply_id,
            &payload,
        )
        .await
        .unwrap();
        assert!(!claimed);
        for (id, payload) in [
            (Uuid::new_v4(), payload.clone()),
            (
                reply_id,
                json!({"request_id":"native-request","option_id":"deny"}),
            ),
        ] {
            assert!(matches!(
                ConversationInteractionReplyRecord::claim(
                    &history.pool,
                    history.session_id,
                    history.run_id,
                    history.attempt_id,
                    "native-request",
                    id,
                    &payload
                )
                .await,
                Err(AgentRuntimePersistenceError::IdempotencyConflict { .. })
            ));
        }
        let command = |command| AgentRunPortCommandEnvelope {
            schema_version: ORCHESTRATION_COMMAND_SCHEMA_VERSION,
            command_id: Uuid::new_v4(),
            idempotency_key: Uuid::new_v4().to_string(),
            agent_run_id: history.run_id,
            orchestration_run_id: None,
            orchestration_node_execution_id: None,
            correlation_id: history.run_id,
            created_at: Utc::now(),
            command,
        };
        let answer = command(AgentRunPortCommand::ReplyInteraction {
            run_attempt_id: history.attempt_id,
            reply_id,
            reply,
        });
        let cancel = command(AgentRunPortCommand::Cancel {
            reason: "user stop".into(),
        });
        for command in [&answer, &cancel] {
            AgentRunCommandRecord::enqueue(&history.pool, command)
                .await
                .unwrap();
            assert!(
                AgentRunCommandRecord::claim(&history.pool, command.command_id)
                    .await
                    .unwrap()
                    .is_some()
            );
        }
        assert_eq!(
            AgentRunCommandRecord::reconcile_inflight(&history.pool)
                .await
                .unwrap(),
            1
        );
        let recovered = ConversationInteractionReplyRecord::for_request(
            &history.pool,
            history.attempt_id,
            "native-request",
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(recovered.delivery_status, "delivery_unknown");
        assert!(
            !ConversationInteractionReplyRecord::mark_delivery(
                &history.pool,
                reply_id,
                "delivered",
                None
            )
            .await
            .unwrap()
        );
        let pending = AgentRunCommandRecord::pending(&history.pool).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].command_id, cancel.command_id);
    }
}
