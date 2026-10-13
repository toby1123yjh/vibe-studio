//! Durable main-Agent handover claims for file-driven workflow dispatch.
//!
//! A handover is an intent to ask the original main Session to read a control
//! file and submit dispatch. It is deliberately separate from conversation
//! messages: notifications and page reads never create a new Agent turn.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct WorkflowHandoverEvent {
    pub id: Uuid,
    pub batch_id: Uuid,
    pub run_id: Uuid,
    pub session_id: Uuid,
    pub state: String,
    pub agent_run_id: Option<Uuid>,
    pub run_attempt_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub error_text: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (SqlitePool, Uuid, Uuid, Uuid) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE sessions(id BLOB PRIMARY KEY); CREATE TABLE workflow_runs(id BLOB PRIMARY KEY,attempt_id BLOB,status TEXT,error_text TEXT); CREATE TABLE workflow_attempts(id BLOB PRIMARY KEY,main_session_id BLOB); CREATE TABLE node_executions(id BLOB PRIMARY KEY,run_id BLOB,status TEXT); CREATE TABLE workflow_run_submissions(id BLOB PRIMARY KEY); CREATE TABLE workflow_stop_intents(run_id BLOB); CREATE TABLE workflow_project_slots(run_id BLOB UNIQUE); CREATE TABLE agent_runs(id BLOB PRIMARY KEY); CREATE TABLE agent_run_attempts(id BLOB PRIMARY KEY,agent_run_id BLOB,status TEXT); CREATE TABLE agent_process_registry(run_attempt_id BLOB PRIMARY KEY,registry_status TEXT);")
            .execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/20261013000000_workflow_file_driven.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let run_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        sqlx::query("INSERT INTO sessions VALUES (?)")
            .bind(session_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_attempts VALUES (?,?)")
            .bind(attempt_id)
            .bind(session_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_runs(id,attempt_id,status) VALUES (?,?,'running')")
            .bind(run_id)
            .bind(attempt_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_project_slots VALUES (?)")
            .bind(run_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_executions VALUES (?,?,'succeeded')")
            .bind(batch_id)
            .bind(run_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_split_batches(id,run_id,node_execution_id,manifest_path,manifest_sha256,state) VALUES (?,?,?,'tasks.json',?,'accepted')")
            .bind(batch_id).bind(run_id).bind(batch_id).bind("a".repeat(64)).execute(&pool).await.unwrap();
        (pool, run_id, batch_id, session_id)
    }

    #[tokio::test]
    async fn event_replay_keeps_the_original_session_and_one_canonical_identity() {
        let (pool, run_id, batch_id, session_id) = fixture().await;
        let event = WorkflowHandoverEvent::create(&pool, batch_id, run_id, session_id)
            .await
            .unwrap();
        let replay = WorkflowHandoverEvent::create(&pool, batch_id, run_id, Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(replay.id, event.id);
        assert_eq!(replay.session_id, session_id);
        let identity = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        assert!(
            WorkflowHandoverEvent::claim(&pool, event.id, identity.0, identity.1, identity.2)
                .await
                .unwrap()
        );
        assert!(
            !WorkflowHandoverEvent::claim(
                &pool,
                event.id,
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4()
            )
            .await
            .unwrap()
        );
        let claimed = WorkflowHandoverEvent::find_by_id(&pool, event.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                claimed.agent_run_id,
                claimed.run_attempt_id,
                claimed.turn_id
            ),
            (Some(identity.0), Some(identity.1), Some(identity.2))
        );
    }

    #[tokio::test]
    async fn concurrent_claims_have_exactly_one_winner() {
        let (pool, run_id, batch_id, session_id) = fixture().await;
        let event = WorkflowHandoverEvent::create(&pool, batch_id, run_id, session_id)
            .await
            .unwrap();
        let first = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let second = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let (a, b) = tokio::join!(
            WorkflowHandoverEvent::claim(&pool, event.id, first.0, first.1, first.2),
            WorkflowHandoverEvent::claim(&pool, event.id, second.0, second.1, second.2)
        );
        let a = a.unwrap();
        let b = b.unwrap();
        assert_ne!(a, b);
        let winner = if a { first } else { second };
        let event = WorkflowHandoverEvent::find_by_id(&pool, event.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (event.agent_run_id, event.run_attempt_id, event.turn_id),
            (Some(winner.0), Some(winner.1), Some(winner.2))
        );
    }

    #[tokio::test]
    async fn claim_requires_source_success_project_slot_and_no_stop_or_dispatch() {
        let (pool, run_id, batch_id, session_id) = fixture().await;
        let event = WorkflowHandoverEvent::create(&pool, batch_id, run_id, session_id)
            .await
            .unwrap();
        sqlx::query("UPDATE node_executions SET status='running'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !WorkflowHandoverEvent::claim(
                &pool,
                event.id,
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4()
            )
            .await
            .unwrap()
        );
        sqlx::query("UPDATE node_executions SET status='succeeded'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM workflow_project_slots")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !WorkflowHandoverEvent::claim(
                &pool,
                event.id,
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4()
            )
            .await
            .unwrap()
        );
        sqlx::query("INSERT INTO workflow_project_slots VALUES (?)")
            .bind(run_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_stop_intents VALUES (?)")
            .bind(run_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !WorkflowHandoverEvent::claim(
                &pool,
                event.id,
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4()
            )
            .await
            .unwrap()
        );
        sqlx::query("DELETE FROM workflow_stop_intents")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE workflow_split_batches SET dispatch_sha256=?")
            .bind("b".repeat(64))
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !WorkflowHandoverEvent::claim(
                &pool,
                event.id,
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4()
            )
            .await
            .unwrap()
        );
        assert_eq!(
            WorkflowHandoverEvent::find_by_id(&pool, event.id)
                .await
                .unwrap()
                .unwrap()
                .state,
            "pending"
        );
    }

    #[tokio::test]
    async fn claim_rejects_a_running_run_already_settling_failure() {
        let (pool, run_id, batch_id, session_id) = fixture().await;
        let event = WorkflowHandoverEvent::create(&pool, batch_id, run_id, session_id)
            .await
            .unwrap();
        sqlx::query("UPDATE workflow_runs SET error_text='required sibling failed' WHERE id=?")
            .bind(run_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !WorkflowHandoverEvent::claim(
                &pool,
                event.id,
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4()
            )
            .await
            .unwrap()
        );
        let retained = WorkflowHandoverEvent::find_by_id(&pool, event.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.state, "pending");
        assert!(retained.agent_run_id.is_none());
    }

    #[tokio::test]
    async fn session_deletion_preserves_the_pending_destination_without_replacement() {
        let (pool, run_id, batch_id, session_id) = fixture().await;
        let event = WorkflowHandoverEvent::create(&pool, batch_id, run_id, session_id)
            .await
            .unwrap();
        sqlx::query("DELETE FROM sessions WHERE id=?")
            .bind(session_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE workflow_attempts SET main_session_id=NULL")
            .execute(&pool)
            .await
            .unwrap();
        let retained = WorkflowHandoverEvent::find_by_id(&pool, event.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.session_id, session_id);
        assert_eq!(retained.state, "pending");
        assert!(
            !WorkflowHandoverEvent::claim(
                &pool,
                event.id,
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4()
            )
            .await
            .unwrap()
        );
        assert!(
            WorkflowHandoverEvent::finish(&pool, event.id, "cancelled", None)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn terminal_event_still_observes_an_unsettled_process() {
        let (pool, run_id, batch_id, session_id) = fixture().await;
        let event = WorkflowHandoverEvent::create(&pool, batch_id, run_id, session_id)
            .await
            .unwrap();
        let run = Uuid::new_v4();
        let attempt = Uuid::new_v4();
        WorkflowHandoverEvent::claim(&pool, event.id, run, attempt, Uuid::new_v4())
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_runs VALUES (?)")
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        WorkflowHandoverEvent::finish(&pool, event.id, "completed", None)
            .await
            .unwrap();
        // Missing canonical attempts remain visible to recovery, rather than
        // becoming an occupancy fence that no worker can ever observe.
        assert_eq!(
            WorkflowHandoverEvent::list_unsettled(&pool)
                .await
                .unwrap()
                .len(),
            1
        );
        sqlx::query("INSERT INTO agent_run_attempts VALUES (?,?,'succeeded')")
            .bind(attempt)
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            WorkflowHandoverEvent::list_unsettled(&pool)
                .await
                .unwrap()
                .len(),
            1
        );
        sqlx::query("INSERT INTO agent_process_registry VALUES (?,'unreachable')")
            .bind(attempt)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            WorkflowHandoverEvent::list_unsettled(&pool)
                .await
                .unwrap()
                .len(),
            1
        );
        sqlx::query("UPDATE agent_process_registry SET registry_status='exited'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            WorkflowHandoverEvent::list_unsettled(&pool)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

impl WorkflowHandoverEvent {
    pub async fn create(
        pool: &SqlitePool,
        batch_id: Uuid,
        run_id: Uuid,
        session_id: Uuid,
    ) -> Result<Self, sqlx::Error> {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO workflow_handover_events (id,batch_id,run_id,session_id,state) VALUES (?,?,?,?,'pending') ON CONFLICT(batch_id) DO NOTHING",
        )
        .bind(id)
        .bind(batch_id)
        .bind(run_id)
        .bind(session_id)
        .execute(pool)
        .await?;
        sqlx::query_as("SELECT * FROM workflow_handover_events WHERE batch_id=?")
            .bind(batch_id)
            .fetch_one(pool)
            .await
    }

    pub async fn list_pending(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(
            "SELECT * FROM workflow_handover_events WHERE state='pending' ORDER BY created_at,id",
        )
        .fetch_all(pool)
        .await
    }

    pub async fn find_for_batch(
        pool: &SqlitePool,
        batch_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM workflow_handover_events WHERE batch_id=?")
            .bind(batch_id)
            .fetch_optional(pool)
            .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM workflow_handover_events WHERE id=?")
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// Terminal turn and process lifetime are separate. Recovery observes an
    /// unsettled process even if the event's business outcome was recorded.
    pub async fn list_unsettled(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(
            "SELECT h.* FROM workflow_handover_events h WHERE h.state IN ('pending','claimed') OR EXISTS(SELECT 1 FROM agent_runs r LEFT JOIN agent_run_attempts a ON a.agent_run_id=r.id LEFT JOIN agent_process_registry p ON p.run_attempt_id=a.id WHERE r.id=h.agent_run_id AND (a.id IS NULL OR a.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed') OR p.run_attempt_id IS NULL OR p.registry_status<>'exited')) ORDER BY h.created_at,h.id",
        )
        .fetch_all(pool)
        .await
    }

    pub async fn set_error(
        pool: &SqlitePool,
        id: Uuid,
        error_text: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE workflow_handover_events SET error_text=?,updated_at=datetime('now','subsec') WHERE id=? AND state IN ('pending','claimed') AND error_text IS NOT ?")
            .bind(error_text).bind(id).bind(error_text).execute(pool).await?;
        Ok(())
    }

    /// Claim is compare-and-set. A restart or a duplicate queue tick cannot
    /// launch a second manager turn for the same accepted split.
    pub async fn claim(
        pool: &SqlitePool,
        id: Uuid,
        agent_run_id: Uuid,
        run_attempt_id: Uuid,
        turn_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE workflow_handover_events SET state='claimed',agent_run_id=?,run_attempt_id=?,turn_id=?,error_text=NULL,updated_at=datetime('now','subsec') WHERE id=? AND state='pending' AND EXISTS(SELECT 1 FROM workflow_split_batches b JOIN workflow_runs r ON r.id=b.run_id JOIN workflow_attempts a ON a.id=r.attempt_id JOIN workflow_project_slots p ON p.run_id=r.id JOIN node_executions n ON n.id=b.node_execution_id AND n.run_id=b.run_id WHERE b.id=workflow_handover_events.batch_id AND b.run_id=workflow_handover_events.run_id AND b.state='accepted' AND b.dispatch_sha256 IS NULL AND n.status='succeeded' AND r.status IN ('running','awaiting_human','awaiting_arena') AND r.error_text IS NULL AND a.main_session_id=workflow_handover_events.session_id) AND NOT EXISTS(SELECT 1 FROM workflow_stop_intents s WHERE s.run_id=workflow_handover_events.run_id)",
        )
        .bind(agent_run_id)
        .bind(run_attempt_id)
        .bind(turn_id)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn finish(
        pool: &SqlitePool,
        id: Uuid,
        state: &str,
        error_text: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        if !matches!(state, "completed" | "cancelled" | "failed") {
            return Err(sqlx::Error::Protocol("invalid handover state".into()));
        }
        let result = sqlx::query(
            "UPDATE workflow_handover_events SET state=?,error_text=?,updated_at=datetime('now','subsec') WHERE id=? AND state IN ('pending','claimed')",
        )
        .bind(state)
        .bind(error_text)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}
