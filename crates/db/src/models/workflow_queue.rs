use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct WorkflowQueueEntry {
    pub sequence: i64,
    pub run_id: Uuid,
    pub project_id: Uuid,
    pub phase: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE local_issues(id BLOB PRIMARY KEY,project_id BLOB);
            CREATE TABLE workflow_runs(id BLOB PRIMARY KEY,issue_id BLOB,status TEXT,finished_at TEXT,updated_at TEXT);
            CREATE TABLE workflow_run_queue(sequence INTEGER PRIMARY KEY AUTOINCREMENT,run_id BLOB UNIQUE,project_id BLOB,phase TEXT DEFAULT 'queued');
            CREATE TABLE workflow_project_slots(project_id BLOB PRIMARY KEY,run_id BLOB UNIQUE);
            CREATE TABLE orchestration_runs(id BLOB,source_definition_id BLOB,product_kind TEXT);
            CREATE TABLE node_executions(run_id BLOB,status TEXT,arena_group_id BLOB,finished_at TEXT);
            CREATE TABLE orchestration_agent_run_links(orchestration_run_id BLOB,agent_run_id BLOB);
            CREATE TABLE agent_run_state(agent_run_id BLOB,status TEXT);
            CREATE TABLE agent_run_attempts(id BLOB,agent_run_id BLOB);
            CREATE TABLE agent_process_registry(run_attempt_id BLOB,registry_status TEXT);
            CREATE TABLE orchestration_outbox(orchestration_run_id BLOB,delivery_status TEXT);")
            .execute(&pool).await.unwrap();
        pool
    }

    async fn enqueue(pool: &SqlitePool, project: Uuid) -> Uuid {
        let run = Uuid::new_v4();
        let issue = Uuid::new_v4();
        sqlx::query("INSERT INTO local_issues VALUES (?,?)")
            .bind(issue)
            .bind(project)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_runs(id,issue_id,status) VALUES (?,?,'pending')")
            .bind(run)
            .bind(issue)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_run_queue(run_id,project_id) VALUES (?,?)")
            .bind(run)
            .bind(project)
            .execute(pool)
            .await
            .unwrap();
        run
    }

    #[tokio::test]
    async fn fifo_occupancy_and_queued_cancellation() {
        let pool = pool().await;
        let project = Uuid::new_v4();
        let first = enqueue(&pool, project).await;
        let canceled = enqueue(&pool, project).await;
        let third = enqueue(&pool, project).await;
        let independent = enqueue(&pool, Uuid::new_v4()).await;
        assert_eq!(
            WorkflowQueueEntry::claim_next(&pool)
                .await
                .unwrap()
                .unwrap()
                .run_id,
            first
        );
        sqlx::query("UPDATE workflow_runs SET status='awaiting_human' WHERE id=?")
            .bind(first)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            WorkflowQueueEntry::claim_next(&pool)
                .await
                .unwrap()
                .unwrap()
                .run_id,
            independent
        );
        assert!(
            WorkflowQueueEntry::claim_next(&pool)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !WorkflowQueueEntry::release_terminal(&pool, first)
                .await
                .unwrap()
        );
        assert!(
            WorkflowQueueEntry::cancel_queued(&pool, canceled)
                .await
                .unwrap()
        );
        assert!(
            !WorkflowQueueEntry::cancel_queued(&pool, first)
                .await
                .unwrap()
        );
        sqlx::query("UPDATE workflow_runs SET status='succeeded' WHERE id=?")
            .bind(first)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            WorkflowQueueEntry::release_terminal(&pool, first)
                .await
                .unwrap()
        );
        assert_eq!(
            WorkflowQueueEntry::claim_next(&pool)
                .await
                .unwrap()
                .unwrap()
                .run_id,
            third
        );
    }

    #[tokio::test]
    async fn release_requires_terminal_child_and_exited_process() {
        let pool = pool().await;
        let run = enqueue(&pool, Uuid::new_v4()).await;
        WorkflowQueueEntry::claim_next(&pool).await.unwrap();
        sqlx::query("UPDATE workflow_runs SET status='failed' WHERE id=?")
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        let agent = Uuid::new_v4();
        let attempt = Uuid::new_v4();
        sqlx::query("INSERT INTO orchestration_agent_run_links VALUES (?,?)")
            .bind(run)
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !WorkflowQueueEntry::release_terminal(&pool, run)
                .await
                .unwrap()
        );
        sqlx::query("INSERT INTO agent_run_state VALUES (?,'cancelled')")
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_run_attempts VALUES (?,?)")
            .bind(attempt)
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_process_registry VALUES (?,'unreachable')")
            .bind(attempt)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !WorkflowQueueEntry::release_terminal(&pool, run)
                .await
                .unwrap()
        );
        sqlx::query("UPDATE agent_process_registry SET registry_status='exited'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO orchestration_outbox VALUES (?,'delivering')")
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !WorkflowQueueEntry::release_terminal(&pool, run)
                .await
                .unwrap()
        );
        sqlx::query("UPDATE orchestration_outbox SET delivery_status='failed'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !WorkflowQueueEntry::release_terminal(&pool, run)
                .await
                .unwrap()
        );
        sqlx::query("UPDATE orchestration_outbox SET delivery_status='delivered'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            WorkflowQueueEntry::release_terminal(&pool, run)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn audit_failure_releases_only_after_confirmed_process_exit() {
        let pool = pool().await;
        let project = Uuid::new_v4();
        let failed = enqueue(&pool, project).await;
        let successor = enqueue(&pool, project).await;
        assert_eq!(
            WorkflowQueueEntry::claim_next(&pool)
                .await
                .unwrap()
                .unwrap()
                .run_id,
            failed
        );
        sqlx::query("UPDATE workflow_runs SET status='failed' WHERE id=?")
            .bind(failed)
            .execute(&pool)
            .await
            .unwrap();
        let agent = Uuid::new_v4();
        let attempt = Uuid::new_v4();
        sqlx::query("INSERT INTO orchestration_agent_run_links VALUES (?,?)")
            .bind(failed)
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_run_state VALUES (?,'audit_failed')")
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_run_attempts VALUES (?,?)")
            .bind(attempt)
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();

        // A terminal audit failure does not establish process absence. Both
        // missing and unreachable registry facts must retain the project slot.
        assert!(
            !WorkflowQueueEntry::release_terminal(&pool, failed)
                .await
                .unwrap()
        );
        sqlx::query("INSERT INTO agent_process_registry VALUES (?,'unreachable')")
            .bind(attempt)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !WorkflowQueueEntry::release_terminal(&pool, failed)
                .await
                .unwrap()
        );
        assert!(
            WorkflowQueueEntry::claim_next(&pool)
                .await
                .unwrap()
                .is_none()
        );

        sqlx::query(
            "UPDATE agent_process_registry SET registry_status='exited' WHERE run_attempt_id=?",
        )
        .bind(attempt)
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            WorkflowQueueEntry::release_terminal(&pool, failed)
                .await
                .unwrap()
        );
        let status: String = sqlx::query_scalar("SELECT status FROM workflow_runs WHERE id=?")
            .bind(failed)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "failed");
        let child_status: String =
            sqlx::query_scalar("SELECT status FROM agent_run_state WHERE agent_run_id=?")
                .bind(agent)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(child_status, "audit_failed");
        assert_eq!(
            WorkflowQueueEntry::claim_next(&pool)
                .await
                .unwrap()
                .unwrap()
                .run_id,
            successor
        );
    }

    #[tokio::test]
    async fn terminal_parent_retains_slot_for_unresolved_human_node() {
        let pool = pool().await;
        let run = enqueue(&pool, Uuid::new_v4()).await;
        WorkflowQueueEntry::claim_next(&pool).await.unwrap();
        sqlx::query("UPDATE workflow_runs SET status='failed' WHERE id=?")
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_executions(run_id,status) VALUES (?,'awaiting_human')")
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !WorkflowQueueEntry::release_terminal(&pool, run)
                .await
                .unwrap()
        );
        sqlx::query("UPDATE node_executions SET status='cancelled' WHERE run_id=?")
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            WorkflowQueueEntry::release_terminal(&pool, run)
                .await
                .unwrap()
        );
    }
}

impl WorkflowQueueEntry {
    /// Claim is a single write transaction. A second dispatcher cannot overtake
    /// the oldest request or acquire a project already held by another run.
    pub async fn claim_next(pool: &SqlitePool) -> Result<Option<Self>, sqlx::Error> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        let claimed: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO workflow_project_slots(project_id, run_id)
             SELECT q.project_id, q.run_id FROM workflow_run_queue q
             JOIN workflow_runs r ON r.id = q.run_id
             WHERE q.phase = 'queued' AND r.status = 'pending'
               AND NOT EXISTS (SELECT 1 FROM workflow_project_slots s WHERE s.project_id = q.project_id)
               AND NOT EXISTS (
                   SELECT 1 FROM workflow_run_queue earlier
                   WHERE earlier.project_id=q.project_id AND earlier.sequence<q.sequence
                     AND earlier.phase IN ('queued','waiting_for_source','stopping_source','starting','active')
               )
               AND NOT EXISTS (
                   SELECT 1 FROM workflow_runs other JOIN local_issues i ON i.id = other.issue_id
                   WHERE i.project_id = q.project_id AND other.id <> q.run_id
                     AND other.status IN ('running','awaiting_human','awaiting_arena','cancelling')
               )
             ORDER BY q.sequence LIMIT 1
             ON CONFLICT(project_id) DO NOTHING RETURNING run_id",
        ).fetch_optional(&mut *tx).await?;
        let entry = if let Some(run_id) = claimed {
            sqlx::query_as::<_, Self>(
                "UPDATE workflow_run_queue SET phase = 'starting' WHERE run_id = ? AND phase = 'queued'
                 RETURNING sequence,run_id,project_id,phase",
            ).bind(run_id).fetch_optional(&mut *tx).await?
        } else {
            None
        };
        tx.commit().await?;
        Ok(entry)
    }

    pub async fn cancel_queued(pool: &SqlitePool, run_id: Uuid) -> Result<bool, sqlx::Error> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        let changed = sqlx::query(
            "UPDATE workflow_run_queue SET phase = 'finished' WHERE run_id = ? AND phase IN ('queued','waiting_for_source','stopping_source') AND EXISTS(SELECT 1 FROM workflow_runs WHERE id=? AND status='pending')",
        ).bind(run_id).bind(run_id).execute(&mut *tx).await?.rows_affected() == 1;
        if changed {
            sqlx::query("UPDATE workflow_runs SET status = 'canceled', finished_at = datetime('now','subsec'), updated_at = datetime('now','subsec') WHERE id = ? AND status = 'pending'")
                .bind(run_id).execute(&mut *tx).await?;
            sqlx::query("UPDATE node_executions SET status='cancelled',finished_at=datetime('now','subsec') WHERE run_id=? AND status='pending'")
                .bind(run_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(changed)
    }

    /// Caller must first reconcile the process boundary. SQL remains a second
    /// guard against releasing an unknown/missing or nonterminal child state.
    pub async fn release_terminal(pool: &SqlitePool, run_id: Uuid) -> Result<bool, sqlx::Error> {
        let mut tx = pool.begin().await?;
        let released = sqlx::query(
            "WITH owned_runs(id) AS (
                 SELECT ?1 UNION SELECT o.id FROM orchestration_runs o
                 JOIN node_executions n ON n.arena_group_id=o.source_definition_id
                 WHERE n.run_id=?1 AND o.product_kind='arena'
             )
             DELETE FROM workflow_project_slots WHERE run_id = ?1
             AND EXISTS (SELECT 1 FROM workflow_runs r WHERE r.id = ?1 AND r.status IN ('succeeded','failed','canceled'))
             AND NOT EXISTS (
                 SELECT 1 FROM orchestration_agent_run_links l
                 LEFT JOIN agent_run_state a ON a.agent_run_id = l.agent_run_id
                 WHERE l.orchestration_run_id IN (SELECT id FROM owned_runs)
                   AND (a.agent_run_id IS NULL OR a.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed'))
             )
             AND NOT EXISTS (
                 SELECT 1 FROM orchestration_agent_run_links l
                 JOIN agent_run_attempts a ON a.agent_run_id=l.agent_run_id
                 LEFT JOIN agent_process_registry p ON p.run_attempt_id=a.id
                 WHERE l.orchestration_run_id IN (SELECT id FROM owned_runs)
                   AND (p.run_attempt_id IS NULL OR p.registry_status<>'exited')
             )
             AND NOT EXISTS (
                 SELECT 1 FROM orchestration_outbox o WHERE o.orchestration_run_id IN (SELECT id FROM owned_runs)
                   AND o.delivery_status IN ('pending','delivering','failed')
             )
             AND NOT EXISTS (SELECT 1 FROM node_executions n WHERE n.run_id = ?1 AND n.status IN ('running','cancelling','awaiting_human','awaiting_arena'))",
        ).bind(run_id).execute(&mut *tx).await?.rows_affected() == 1;
        if released {
            sqlx::query("UPDATE workflow_run_queue SET phase = 'finished' WHERE run_id = ?")
                .bind(run_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(released)
    }
}
