use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, SqlitePool, types::Json};
use uuid::Uuid;

use super::agent_runtime::AgentRuntimePersistenceError;

#[derive(Debug, Clone, FromRow)]
pub struct ConversationSessionRecord {
    pub session_id: Uuid,
    pub epoch: Uuid,
    pub revision: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct ConversationBlockRecord {
    pub session_id: Uuid,
    pub block_id: String,
    pub agent_run_id: Uuid,
    pub run_attempt_id: Uuid,
    pub kind: String,
    pub role: Option<String>,
    pub native_id: Option<String>,
    pub parent_block_id: Option<String>,
    pub position: String,
    pub revision: i64,
    pub fields_json: Json<Value>,
    pub body_lengths_json: Json<std::collections::BTreeMap<String, u64>>,
    pub finished: bool,
    pub finish_reason: Option<String>,
    pub tombstone: bool,
    pub source_event_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct ConversationInteractionReplyRecord {
    pub reply_id: Uuid,
    pub session_id: Uuid,
    pub agent_run_id: Uuid,
    pub run_attempt_id: Uuid,
    pub request_id: String,
    pub payload_json: Json<Value>,
    pub delivery_status: String,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ConversationInteractionReplyRecord {
    /// Only one answer may own a native request. Repeated identity is accepted
    /// only with the identical target and payload; it never claims stdin twice.
    pub async fn claim(
        pool: &SqlitePool,
        session_id: Uuid,
        agent_run_id: Uuid,
        run_attempt_id: Uuid,
        request_id: &str,
        reply_id: Uuid,
        payload: &Value,
    ) -> Result<(Self, bool), AgentRuntimePersistenceError> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing = sqlx::query_as::<_, Self>(
            "SELECT * FROM agent_conversation_interaction_replies
             WHERE reply_id = ? OR (run_attempt_id = ? AND request_id = ?)",
        )
        .bind(reply_id)
        .bind(run_attempt_id)
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(existing) = existing {
            if existing.reply_id != reply_id
                || existing.session_id != session_id
                || existing.agent_run_id != agent_run_id
                || existing.run_attempt_id != run_attempt_id
                || existing.request_id != request_id
                || existing.payload_json.0 != *payload
            {
                return Err(AgentRuntimePersistenceError::IdempotencyConflict {
                    entity: "native interaction reply",
                    key: request_id.to_owned(),
                });
            }
            tx.commit().await?;
            return Ok((existing, false));
        }
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO agent_conversation_interaction_replies
             (reply_id, session_id, agent_run_id, run_attempt_id, request_id,
              payload_json, delivery_status, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, 'delivering', ?, ?)",
        )
        .bind(reply_id)
        .bind(session_id)
        .bind(agent_run_id)
        .bind(run_attempt_id)
        .bind(request_id)
        .bind(Json(payload))
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let record = sqlx::query_as::<_, Self>(
            "SELECT * FROM agent_conversation_interaction_replies WHERE reply_id = ?",
        )
        .bind(reply_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((record, true))
    }

    pub async fn mark_delivery(
        pool: &SqlitePool,
        reply_id: Uuid,
        status: &str,
        error: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        if !matches!(
            status,
            "delivered" | "resolved" | "delivery_unknown" | "failed" | "cancelled"
        ) {
            return Err(sqlx::Error::Protocol(
                "invalid interaction delivery status".into(),
            ));
        }
        Ok(sqlx::query(
            "UPDATE agent_conversation_interaction_replies
             SET delivery_status = ?, last_error = ?, updated_at = ?
             WHERE reply_id = ? AND delivery_status IN ('pending', 'delivering')",
        )
        .bind(status)
        .bind(error)
        .bind(Utc::now())
        .bind(reply_id)
        .execute(pool)
        .await?
        .rows_affected()
            == 1)
    }

    pub async fn for_request(
        pool: &SqlitePool,
        run_attempt_id: Uuid,
        request_id: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM agent_conversation_interaction_replies WHERE run_attempt_id = ? AND request_id = ?")
            .bind(run_attempt_id)
            .bind(request_id)
            .fetch_optional(pool)
            .await
    }
}
