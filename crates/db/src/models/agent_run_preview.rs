//! Disposable summaries, separate from the authoritative AgentRun reducer.

use chrono::{DateTime, Utc};
use executors::runtime::{AgentRuntimeMessageRole, AgentRuntimeToolStatus};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool, types::Json};
use uuid::Uuid;

pub const AGENT_RUN_PREVIEW_VERSION: i64 = 1;
pub const AGENT_RUN_PREVIEW_CHAR_LIMIT: usize = 600;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantReplyPreview {
    pub message_id: Uuid,
    pub run_attempt_id: Uuid,
    pub run_attempt_number: u32,
    pub sequence: u64,
    pub event_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPreviewActivityKind {
    Message,
    Thinking,
    Tool,
    ApprovalRequested,
    ApprovalResolved,
    InputRequested,
    InputResolved,
    Error,
    Lifecycle,
    FileChanges,
    ProjectionDegraded,
}

impl AgentPreviewActivityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Thinking => "thinking",
            Self::Tool => "tool",
            Self::ApprovalRequested => "approval_requested",
            Self::ApprovalResolved => "approval_resolved",
            Self::InputRequested => "input_requested",
            Self::InputResolved => "input_resolved",
            Self::Error => "error",
            Self::Lifecycle => "lifecycle",
            Self::FileChanges => "file_changes",
            Self::ProjectionDegraded => "projection_degraded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunActivity {
    pub kind: AgentPreviewActivityKind,
    pub timestamp: DateTime<Utc>,
    pub event_id: Uuid,
    pub run_attempt_id: Uuid,
    pub run_attempt_number: u32,
    pub sequence: u64,
    pub label: String,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentToolPreview {
    pub tool_name: String,
    pub status: AgentRuntimeToolStatus,
    /// A terminal lifecycle event clears this even if a tool-end was missing.
    pub active: bool,
    pub timestamp: DateTime<Utc>,
    pub event_id: Uuid,
    pub run_attempt_id: Uuid,
    pub run_attempt_number: u32,
    pub sequence: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentRunPreview {
    pub assistant: Option<AssistantReplyPreview>,
    pub activity: Option<AgentRunActivity>,
    pub latest_tool: Option<AgentToolPreview>,
    /// Only adjacency, not a second message history, is retained for replay.
    pub merge_message_id: Option<Uuid>,
    pub merge_role: Option<AgentRuntimeMessageRole>,
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentRunPreviewRecord {
    pub agent_run_id: Uuid,
    pub algorithm_version: i64,
    pub last_run_attempt_id: Option<Uuid>,
    pub last_run_attempt_number: i64,
    pub last_sequence: i64,
    pub last_event_id: Option<Uuid>,
    #[sqlx(rename = "preview_json")]
    pub preview: Json<AgentRunPreview>,
    pub updated_at: DateTime<Utc>,
}

impl AgentRunPreviewRecord {
    pub fn empty(agent_run_id: Uuid) -> Self {
        Self {
            agent_run_id,
            algorithm_version: AGENT_RUN_PREVIEW_VERSION,
            last_run_attempt_id: None,
            last_run_attempt_number: 0,
            last_sequence: 0,
            last_event_id: None,
            preview: Json(AgentRunPreview::default()),
            updated_at: Utc::now(),
        }
    }

    pub async fn find(pool: &SqlitePool, agent_run_id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        // An older algorithm may have an entirely different JSON shape. Its
        // cursor remains available for compare-and-swap, but its body must not
        // be decoded as the current projection before replay can replace it.
        sqlx::query_as(
            r#"SELECT agent_run_id, algorithm_version, last_run_attempt_id,
                last_run_attempt_number, last_sequence, last_event_id,
                CASE WHEN algorithm_version = ? THEN preview_json ELSE '{}' END AS preview_json,
                updated_at FROM agent_run_previews WHERE agent_run_id = ?"#,
        )
        .bind(AGENT_RUN_PREVIEW_VERSION)
        .bind(agent_run_id)
        .fetch_optional(pool)
        .await
    }

    /// Chunking stays below SQLite's bind limit without dropping any Run IDs.
    pub async fn find_for_runs(
        pool: &SqlitePool,
        agent_run_ids: &[Uuid],
    ) -> Result<Vec<Self>, sqlx::Error> {
        let mut rows = Vec::new();
        for chunk in agent_run_ids.chunks(200) {
            let mut query = QueryBuilder::<Sqlite>::new(
                "SELECT agent_run_id, algorithm_version, last_run_attempt_id, \
                    last_run_attempt_number, last_sequence, last_event_id, \
                    CASE WHEN algorithm_version = ",
            );
            query.push_bind(AGENT_RUN_PREVIEW_VERSION).push(
                " THEN preview_json ELSE '{}' END AS preview_json, updated_at \
                    FROM agent_run_previews WHERE agent_run_id IN (",
            );
            let mut values = query.separated(", ");
            for id in chunk {
                values.push_bind(*id);
            }
            values.push_unseparated(")");
            rows.extend(query.build_query_as::<Self>().fetch_all(pool).await?);
        }
        Ok(rows)
    }

    /// A durable sweep repairs missed/lagged broadcasts, old algorithms and
    /// process restarts. Only cursor metadata is read, never event bodies.
    pub async fn pending_run_ids(pool: &SqlitePool, limit: u32) -> Result<Vec<Uuid>, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            SELECT state.agent_run_id
            FROM agent_run_state state
            LEFT JOIN agent_run_previews preview
                ON preview.agent_run_id = state.agent_run_id
            WHERE state.last_event_id IS NOT NULL AND (
                preview.agent_run_id IS NULL
                OR preview.algorithm_version <> ?
                OR preview.last_run_attempt_number <> state.last_run_attempt_number
                OR preview.last_sequence <> state.last_event_sequence
                OR preview.last_event_id IS NOT state.last_event_id
            )
            ORDER BY
                CASE WHEN state.status IN (
                    'pending', 'starting', 'running', 'awaiting_input',
                    'awaiting_approval', 'cancelling'
                ) THEN 0 ELSE 1 END,
                COALESCE(preview.updated_at, '') ASC, state.updated_at DESC,
                state.agent_run_id
            LIMIT ?
            "#,
        )
        .bind(AGENT_RUN_PREVIEW_VERSION)
        .bind(i64::from(limit.clamp(1, 256)))
        .fetch_all(pool)
        .await
    }

    /// The body and cursor move in one SQLite statement. Optimistic checks
    /// prevent another worker or an old algorithm from overwriting newer data.
    /// Event reads/reduction happen before this short, write-only operation.
    pub async fn store_if_unchanged(
        pool: &SqlitePool,
        previous: Option<&Self>,
        next: &Self,
    ) -> Result<bool, sqlx::Error> {
        let result = match previous {
            None => {
                sqlx::query(
                    r#"
                    INSERT INTO agent_run_previews (
                        agent_run_id, algorithm_version, last_run_attempt_id,
                        last_run_attempt_number, last_sequence, last_event_id,
                        preview_json, updated_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                    ON CONFLICT(agent_run_id) DO NOTHING
                    "#,
                )
                .bind(next.agent_run_id)
                .bind(next.algorithm_version)
                .bind(next.last_run_attempt_id)
                .bind(next.last_run_attempt_number)
                .bind(next.last_sequence)
                .bind(next.last_event_id)
                .bind(&next.preview)
                .bind(next.updated_at)
                .execute(pool)
                .await?
            }
            Some(previous) => {
                sqlx::query(
                    r#"
                    UPDATE agent_run_previews SET
                        algorithm_version = ?, last_run_attempt_id = ?,
                        last_run_attempt_number = ?, last_sequence = ?,
                        last_event_id = ?, preview_json = ?, updated_at = ?
                    WHERE agent_run_id = ? AND algorithm_version = ?
                        AND last_run_attempt_number = ? AND last_sequence = ?
                        AND last_event_id IS ?
                    "#,
                )
                .bind(next.algorithm_version)
                .bind(next.last_run_attempt_id)
                .bind(next.last_run_attempt_number)
                .bind(next.last_sequence)
                .bind(next.last_event_id)
                .bind(&next.preview)
                .bind(next.updated_at)
                .bind(next.agent_run_id)
                .bind(previous.algorithm_version)
                .bind(previous.last_run_attempt_number)
                .bind(previous.last_sequence)
                .bind(previous.last_event_id)
                .execute(pool)
                .await?
            }
        };
        Ok(result.rows_affected() == 1)
    }
}
