use std::collections::HashSet;

use chrono::{DateTime, Utc};
use db::models::agent_runtime::{
    AgentEventRecord, AgentRunCommandRecord, AgentRunStateRecord, AgentRuntimePersistenceError,
};
use executors::runtime::{
    AgentEventEnvelope, AgentEventPayload, AgentRunPort, AgentRunPortCommandEnvelope,
    AgentRunPortError, AgentRunStatus, ProjectionStatus, RunState,
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool, types::Json};
use thiserror::Error;
use ts_rs::TS;
use uuid::Uuid;

/// The cursor used by both history and live subscriptions. Sequences restart
/// for every attempt, so the attempt number is part of the ordering key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AgentEventCursor {
    pub run_attempt_number: u32,
    pub sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AgentRunSummary {
    pub agent_run_id: Uuid,
    pub session_id: Uuid,
    pub turn_id: Uuid,
    pub state: RunState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AgentRunHistoryPage {
    pub agent_run_id: Uuid,
    pub state: RunState,
    pub events: Vec<AgentEventEnvelope>,
    pub next_cursor: Option<AgentEventCursor>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AgentRunStats {
    pub event_count: u64,
    pub message_count: u64,
    pub thinking_count: u64,
    pub tool_call_count: u64,
    pub approval_request_count: u64,
    pub approval_resolution_count: u64,
    pub input_request_count: u64,
    pub input_resolution_count: u64,
    pub error_count: u64,
    pub provider_extension_count: u64,
    pub unknown_event_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub first_event_at: Option<DateTime<Utc>>,
    pub last_event_at: Option<DateTime<Utc>>,
    pub status: AgentRunStatus,
    pub projection_status: ProjectionStatus,
}

#[derive(Debug, Error)]
pub enum AgentRuntimeReadError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("agent run {0} was not found")]
    NotFound(Uuid),
    #[error("agent event {event_id} has invalid sequence {sequence}")]
    InvalidSequence { event_id: Uuid, sequence: i64 },
    #[error("agent event {event_id} has invalid attempt number {attempt_number}")]
    InvalidAttemptNumber { event_id: Uuid, attempt_number: i64 },
}

#[derive(Debug, Error)]
pub enum AgentRunCommandError {
    #[error(transparent)]
    Persistence(#[from] AgentRuntimePersistenceError),
    #[error(transparent)]
    Port(#[from] AgentRunPortError),
    #[error("AgentRun command {0} is already being delivered")]
    DeliveryInProgress(Uuid),
    #[error(
        "Native interaction command {0} has an unconfirmed delivery; it cannot be resent automatically"
    )]
    ReplyDeliveryUnconfirmed(Uuid),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentRunCommandReconciliationReport {
    pub recovered_inflight: u64,
    pub delivered: u64,
    pub failed: u64,
}

pub struct AgentRunCommandService<'a, P> {
    pool: &'a SqlitePool,
    port: &'a P,
}

impl<'a, P> AgentRunCommandService<'a, P>
where
    P: AgentRunPort,
{
    pub const fn new(pool: &'a SqlitePool, port: &'a P) -> Self {
        Self { pool, port }
    }

    pub async fn dispatch(
        &self,
        command: AgentRunPortCommandEnvelope,
    ) -> Result<(), AgentRunCommandError> {
        let (record, _) = AgentRunCommandRecord::enqueue(self.pool, &command).await?;
        if record.delivery_status == "delivered" {
            return Ok(());
        }
        if record.delivery_status == "failed"
            && matches!(
                &record.command_envelope.0.command,
                executors::runtime::AgentRunPortCommand::ReplyInteraction { .. }
            )
        {
            return Err(AgentRunCommandError::ReplyDeliveryUnconfirmed(
                record.command_id,
            ));
        }
        self.deliver(record.command_id).await
    }

    pub async fn reconcile_startup(
        &self,
    ) -> Result<AgentRunCommandReconciliationReport, AgentRunCommandError> {
        let mut report = AgentRunCommandReconciliationReport {
            recovered_inflight: AgentRunCommandRecord::reconcile_inflight(self.pool).await?,
            ..AgentRunCommandReconciliationReport::default()
        };
        for command in AgentRunCommandRecord::pending(self.pool).await? {
            match self.deliver(command.command_id).await {
                Ok(()) => report.delivered += 1,
                Err(error) => {
                    report.failed += 1;
                    tracing::warn!(
                        command_id = %command.command_id,
                        agent_run_id = %command.agent_run_id,
                        %error,
                        "durable AgentRun command remains pending after startup reconciliation"
                    );
                }
            }
        }
        Ok(report)
    }

    async fn deliver(&self, command_id: Uuid) -> Result<(), AgentRunCommandError> {
        let Some(command) = AgentRunCommandRecord::claim(self.pool, command_id).await? else {
            return Err(AgentRunCommandError::DeliveryInProgress(command_id));
        };
        match self.port.control(command.command_envelope.0).await {
            Ok(()) => {
                AgentRunCommandRecord::mark_delivered(self.pool, command_id).await?;
                Ok(())
            }
            Err(error) => {
                AgentRunCommandRecord::mark_failed(self.pool, command_id, &error.to_string())
                    .await?;
                Err(error.into())
            }
        }
    }
}

#[derive(Debug, FromRow)]
struct AgentRunSummaryRow {
    agent_run_id: Uuid,
    session_id: Uuid,
    turn_id: Uuid,
    state_json: Json<RunState>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Read-only canonical projection access for product consumers.
///
/// This service intentionally has no methods that read `MsgStore`, provider
/// events, native audit frames, or `execution_processes` status. Consumers use
/// the same page/cursor contract for initial history and live recovery.
#[derive(Debug, Clone, Copy)]
pub struct AgentRuntimeReadService<'a> {
    pool: &'a SqlitePool,
}

impl<'a> AgentRuntimeReadService<'a> {
    pub const fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn state(&self, agent_run_id: Uuid) -> Result<RunState, AgentRuntimeReadError> {
        let state = sqlx::query_as::<_, AgentRunStateRecord>(
            "SELECT * FROM agent_run_state WHERE agent_run_id = ?",
        )
        .bind(agent_run_id)
        .fetch_optional(self.pool)
        .await?
        .ok_or(AgentRuntimeReadError::NotFound(agent_run_id))?;
        Ok(state.state_json.0)
    }

    pub async fn list_for_session(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<AgentRunSummary>, AgentRuntimeReadError> {
        let rows = sqlx::query_as::<_, AgentRunSummaryRow>(
            r#"
            SELECT ar.id AS agent_run_id,
                   ar.session_id,
                   at.id AS turn_id,
                   state.state_json,
                   ar.created_at,
                   ar.updated_at
            FROM agent_runs ar
            JOIN agent_turns at ON at.agent_run_id = ar.id
            JOIN agent_run_state state ON state.agent_run_id = ar.id
            WHERE ar.session_id = ?
            ORDER BY ar.created_at, ar.id
            "#,
        )
        .bind(session_id)
        .fetch_all(self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| AgentRunSummary {
                agent_run_id: row.agent_run_id,
                session_id: row.session_id,
                turn_id: row.turn_id,
                state: row.state_json.0,
                created_at: row.created_at,
                updated_at: row.updated_at,
            })
            .collect())
    }

    pub async fn history_page(
        &self,
        agent_run_id: Uuid,
        after: Option<AgentEventCursor>,
        limit: u32,
    ) -> Result<AgentRunHistoryPage, AgentRuntimeReadError> {
        // Clamp at the service boundary so API and WebSocket consumers share
        // the same memory/backpressure contract.
        let limit = i64::from(limit.clamp(1, 1_000));
        let state = self.state(agent_run_id).await?;
        let fetch_limit = limit + 1;
        let records = match after {
            Some(cursor) => {
                sqlx::query_as::<_, AgentEventRecord>(
                    r#"
                SELECT * FROM agent_events
                WHERE agent_run_id = ?
                  AND (run_attempt_number > ?
                    OR (run_attempt_number = ? AND sequence > ?))
                ORDER BY run_attempt_number, sequence
                LIMIT ?
                "#,
                )
                .bind(agent_run_id)
                .bind(i64::from(cursor.run_attempt_number))
                .bind(i64::from(cursor.run_attempt_number))
                .bind(i64::try_from(cursor.sequence).unwrap_or(i64::MAX))
                .bind(fetch_limit)
                .fetch_all(self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, AgentEventRecord>(
                    r#"
                SELECT * FROM agent_events
                WHERE agent_run_id = ?
                ORDER BY run_attempt_number, sequence
                LIMIT ?
                "#,
                )
                .bind(agent_run_id)
                .bind(fetch_limit)
                .fetch_all(self.pool)
                .await?
            }
        };

        let has_more = records.len() as i64 > limit;
        let events = records
            .into_iter()
            .take(limit as usize)
            .map(record_event)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = events.last().map(event_cursor);

        Ok(AgentRunHistoryPage {
            agent_run_id,
            state,
            events,
            next_cursor,
            has_more,
        })
    }

    pub async fn stats(&self, agent_run_id: Uuid) -> Result<AgentRunStats, AgentRuntimeReadError> {
        let mut page = self.history_page(agent_run_id, None, 1_000).await?;
        let mut all_events = std::mem::take(&mut page.events);
        while page.has_more {
            page = self
                .history_page(agent_run_id, page.next_cursor, 1_000)
                .await?;
            all_events.extend(page.events);
        }

        let mut stats = AgentRunStats {
            event_count: 0,
            message_count: 0,
            thinking_count: 0,
            tool_call_count: 0,
            approval_request_count: 0,
            approval_resolution_count: 0,
            input_request_count: 0,
            input_resolution_count: 0,
            error_count: 0,
            provider_extension_count: 0,
            unknown_event_count: 0,
            input_tokens: 0,
            output_tokens: 0,
            cached_input_tokens: 0,
            first_event_at: None,
            last_event_at: None,
            status: page.state.status,
            projection_status: page.state.projection_status,
        };
        stats.event_count = all_events.len() as u64;
        stats.unknown_event_count = page.state.unknown_event_count;
        let mut identities = SemanticEventIdentities::default();
        for event in &all_events {
            identities.observe(event);
            stats.first_event_at = stats.first_event_at.map_or(Some(event.timestamp), |value| {
                Some(value.min(event.timestamp))
            });
            stats.last_event_at = stats.last_event_at.map_or(Some(event.timestamp), |value| {
                Some(value.max(event.timestamp))
            });
            match &event.payload {
                AgentEventPayload::Conversation { .. }
                | AgentEventPayload::Message { .. }
                | AgentEventPayload::Thinking { .. }
                | AgentEventPayload::ToolCall { .. }
                | AgentEventPayload::ApprovalRequested { .. }
                | AgentEventPayload::ApprovalResolved { .. }
                | AgentEventPayload::InputRequested { .. }
                | AgentEventPayload::InputResolved { .. } => {}
                AgentEventPayload::TokenUsage {
                    input_tokens,
                    output_tokens,
                    cached_input_tokens,
                } => {
                    stats.input_tokens = stats.input_tokens.saturating_add(*input_tokens);
                    stats.output_tokens = stats.output_tokens.saturating_add(*output_tokens);
                    stats.cached_input_tokens = stats
                        .cached_input_tokens
                        .saturating_add(cached_input_tokens.unwrap_or_default());
                }
                AgentEventPayload::Error { .. } => stats.error_count += 1,
                AgentEventPayload::ProviderExtension { .. } => stats.provider_extension_count += 1,
                AgentEventPayload::Unknown { .. } => {}
                AgentEventPayload::LifecycleChanged { .. }
                | AgentEventPayload::FileChanges { .. }
                | AgentEventPayload::ProjectionDegraded { .. }
                | AgentEventPayload::SessionObserved { .. } => {}
            }
        }
        stats.message_count = identities.messages.len() as u64;
        stats.thinking_count = identities.thinking.len() as u64;
        stats.tool_call_count = identities.tools.len() as u64;
        stats.approval_request_count = identities.approval_requests.len() as u64;
        stats.approval_resolution_count = identities.approval_resolutions.len() as u64;
        stats.input_request_count = identities.input_requests.len() as u64;
        stats.input_resolution_count = identities.input_resolutions.len() as u64;
        Ok(stats)
    }
}

#[derive(Default)]
struct SemanticEventIdentities {
    messages: HashSet<(Uuid, String)>,
    thinking: HashSet<(Uuid, String)>,
    tools: HashSet<(Uuid, String)>,
    approval_requests: HashSet<(Uuid, String)>,
    approval_resolutions: HashSet<(Uuid, String)>,
    input_requests: HashSet<(Uuid, String)>,
    input_resolutions: HashSet<(Uuid, String)>,
}

impl SemanticEventIdentities {
    fn observe(&mut self, event: &AgentEventEnvelope) {
        use executors::runtime::{
            ConversationBlockKind, ConversationBlockOperation, ConversationInteractionKind,
            ConversationInteractionStatus,
        };

        let key = |id: String| (event.run_attempt_id, id);
        match &event.payload {
            AgentEventPayload::Conversation { update } => {
                let fields = match &update.operation {
                    ConversationBlockOperation::Snapshot { fields }
                    | ConversationBlockOperation::Patch { fields, .. } => Some(fields),
                    _ => None,
                };
                match update.kind {
                    ConversationBlockKind::Text => {
                        self.messages.insert(key(update.block_id.clone()));
                    }
                    ConversationBlockKind::Reasoning => {
                        self.thinking.insert(key(update.block_id.clone()));
                    }
                    ConversationBlockKind::Tool => {
                        // Fields may be omitted on later patches. The stable
                        // block ID, rather than a patch's native ID, owns this.
                        self.tools.insert(key(update.block_id.clone()));
                    }
                    _ => {}
                }
                if let Some(interaction) = fields.and_then(|fields| fields.interaction.as_ref()) {
                    let (requested, resolved) = match interaction.kind {
                        ConversationInteractionKind::Permission => {
                            (&mut self.approval_requests, &mut self.approval_resolutions)
                        }
                        ConversationInteractionKind::Question => {
                            (&mut self.input_requests, &mut self.input_resolutions)
                        }
                    };
                    requested.insert(key(interaction.request_id.clone()));
                    if matches!(
                        interaction.status,
                        ConversationInteractionStatus::Resolved
                            | ConversationInteractionStatus::Cancelled
                    ) {
                        resolved.insert(key(interaction.request_id.clone()));
                    }
                }
            }
            AgentEventPayload::Message { message, .. } => {
                self.messages.insert(key(message.message_id.to_string()));
            }
            AgentEventPayload::Thinking { .. } => {
                self.thinking.insert(key(event.event_id.to_string()));
            }
            AgentEventPayload::ToolCall { tool_call_id, .. } => {
                self.tools.insert(key(tool_call_id
                    .clone()
                    .unwrap_or_else(|| event.event_id.to_string())));
            }
            AgentEventPayload::ApprovalRequested { approval_id, .. } => {
                self.approval_requests.insert(key(approval_id.clone()));
            }
            AgentEventPayload::ApprovalResolved { approval_id, .. } => {
                self.approval_resolutions.insert(key(approval_id.clone()));
            }
            AgentEventPayload::InputRequested { input_id, .. } => {
                self.input_requests.insert(key(input_id.clone()));
            }
            AgentEventPayload::InputResolved { input_id, .. } => {
                self.input_resolutions.insert(key(input_id.clone()));
            }
            _ => {}
        }
    }
}

fn record_event(record: AgentEventRecord) -> Result<AgentEventEnvelope, AgentRuntimeReadError> {
    if record.sequence < 0 {
        return Err(AgentRuntimeReadError::InvalidSequence {
            event_id: record.event_id,
            sequence: record.sequence,
        });
    }
    if record.run_attempt_number < 0 {
        return Err(AgentRuntimeReadError::InvalidAttemptNumber {
            event_id: record.event_id,
            attempt_number: record.run_attempt_number,
        });
    }
    Ok(record.event_envelope.0)
}

fn event_cursor(event: &AgentEventEnvelope) -> AgentEventCursor {
    AgentEventCursor {
        run_attempt_number: event.run_attempt_number,
        sequence: event.sequence,
    }
}

#[cfg(test)]
mod tests {
    use super::AgentEventCursor;

    #[test]
    fn cursor_keeps_attempt_boundary() {
        let first_attempt = AgentEventCursor {
            run_attempt_number: 1,
            sequence: 42,
        };
        let resumed_attempt = AgentEventCursor {
            run_attempt_number: 2,
            sequence: 1,
        };
        assert_ne!(first_attempt, resumed_attempt);
    }
}
