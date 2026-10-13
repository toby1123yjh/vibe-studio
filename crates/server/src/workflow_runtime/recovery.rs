//! Execution recovery keeps accepted identities. Absence of a response is not
//! evidence that an Agent process never started.
use super::*;

static RUN_LOCKS: LazyLock<Mutex<HashMap<Uuid, Weak<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn workflow_run_lock(run_id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = RUN_LOCKS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&run_id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(run_id, Arc::downgrade(&lock));
    lock
}

pub(super) async fn children_settled(pool: &SqlitePool, run_id: Uuid) -> Result<bool, ApiError> {
    if crate::workflow_runtime::handover::has_unsettled(pool, run_id).await? {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(r#"
        WITH owned_runs(id) AS (
            SELECT orchestration_run_id FROM workflow_runs WHERE id=?1
            UNION SELECT o.id FROM orchestration_runs o JOIN node_executions n
              ON n.arena_group_id=o.source_definition_id
              WHERE n.run_id=?1 AND o.product_kind='arena'
        )
        SELECT NOT EXISTS(SELECT 1 FROM node_executions WHERE run_id=?1
          AND status IN ('running','awaiting_human','awaiting_arena','cancelling'))
        AND NOT EXISTS(SELECT 1 FROM orchestration_agent_run_links l
          LEFT JOIN agent_run_state s ON s.agent_run_id=l.agent_run_id
          WHERE l.orchestration_run_id IN (SELECT id FROM owned_runs)
          AND (s.agent_run_id IS NULL OR s.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed')))
        AND NOT EXISTS(SELECT 1 FROM orchestration_agent_run_links l
          LEFT JOIN agent_run_attempts a ON a.agent_run_id=l.agent_run_id
          LEFT JOIN agent_process_registry p ON p.run_attempt_id=a.id
          WHERE l.orchestration_run_id IN (SELECT id FROM owned_runs)
          AND (a.id IS NULL OR a.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed')
            OR p.run_attempt_id IS NULL OR p.registry_status<>'exited'))
        AND NOT EXISTS(SELECT 1 FROM orchestration_outbox
          WHERE orchestration_run_id IN (SELECT id FROM owned_runs)
          AND delivery_status IN ('pending','delivering','failed'))
    "#).bind(run_id).fetch_one(pool).await?)
}

/// Close only work that was never durably launched. Keep accepted child
/// identities until the process/outbox boundary can establish their outcome.
pub(super) async fn skip_undispatched_in(
    conn: &mut SqliteConnection,
    run_id: Uuid,
    batch_id: Option<Uuid>,
    message: &str,
) -> Result<(), ApiError> {
    sqlx::query(r#"
        UPDATE node_executions SET status='skipped',error_text=?1,
          finished_at=datetime('now','subsec'),updated_at=datetime('now','subsec')
        WHERE run_id=?2 AND status IN ('pending','running')
          AND (?3 IS NULL OR id IN (SELECT node_execution_id FROM workflow_split_subtasks WHERE batch_id=?3)
            OR id IN (SELECT summary_node_execution_id FROM workflow_split_batches WHERE id=?3))
          AND agent_run_id IS NULL AND arena_group_id IS NULL
          AND NOT EXISTS(SELECT 1 FROM orchestration_agent_run_links l
            WHERE l.node_execution_id=node_executions.orchestration_node_execution_id)
          AND NOT EXISTS(SELECT 1 FROM orchestration_outbox o
            WHERE o.node_execution_id=node_executions.orchestration_node_execution_id)
    "#).bind(message).bind(run_id).bind(batch_id).execute(conn).await?;
    Ok(())
}

/// Must run under the per-Run lock. Failure blocks new work but never cancels
/// independent accepted children, nor releases occupancy while they are live.
pub(super) async fn settle_workflow_failure(
    pool: &SqlitePool,
    run_id: Uuid,
    message: &str,
) -> Result<(), ApiError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    // Keep a run-level failure fact even when startup failed outside a Node.
    // Its presence is the dispatch fence; the outcome still waits for actual
    // children. Never inspect the wording or overwrite the original reason.
    let message: Option<String> = sqlx::query_scalar(
        r#"
        UPDATE workflow_runs SET error_text=COALESCE(error_text,?),
          status=CASE WHEN status='pending' THEN 'running' ELSE status END,
          updated_at=datetime('now','subsec')
        WHERE id=? AND status IN ('pending','running','awaiting_human','awaiting_arena')
        RETURNING error_text
    "#,
    )
    .bind(message)
    .bind(run_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(message) = message else {
        tx.commit().await?;
        return Ok(());
    };
    // An interrupted starter must become reconcilable, not remain forever in
    // 'starting'. This transition shares the durable failure transaction.
    sqlx::query("UPDATE workflow_run_queue SET phase='active' WHERE run_id=? AND phase='starting'")
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    batch_runtime::abort_unstarted(pool, run_id, &message).await?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    skip_undispatched_in(&mut tx, run_id, None, &message).await?;
    // Unanswered review/winner choices cannot unlock failed dependencies.
    // Arena process identities remain associated and still count as live.
    sqlx::query(
        r#"
        UPDATE node_executions SET status='skipped',error_text=?,
          finished_at=datetime('now','subsec'),updated_at=datetime('now','subsec')
        WHERE run_id=? AND status IN ('awaiting_human','awaiting_arena')
          AND node_type IN ('human_gate','condition','arena')
          AND NOT EXISTS(SELECT 1 FROM workflow_interaction_responses r
            WHERE r.node_execution_id=node_executions.id)
    "#,
    )
    .bind(&message)
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if children_settled(pool, run_id).await? {
        mark_pending_nodes_skipped(pool, run_id).await?;
        update_run_status(
            pool,
            run_id,
            WorkflowRunStatus::Failed,
            None,
            Some(&message),
            true,
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn recover_undispatched_nodes<
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
>(
    pool: &SqlitePool,
    run_id: Uuid,
    graph: &WorkflowGraph,
    issue_id: Uuid,
    workspace_id: Uuid,
    input: &str,
    executor: &A,
    arena: &R,
) -> Result<(), ApiError> {
    let rows=sqlx::query(r#"
        SELECT n.*,l.agent_run_id AS linked_agent_run_id
        FROM node_executions n
        LEFT JOIN orchestration_agent_run_links l ON l.node_execution_id=n.orchestration_node_execution_id
        WHERE n.run_id=? AND n.status='running' ORDER BY n.rowid
    "#).bind(run_id).fetch_all(pool).await?;
    for row in rows {
        let node_id: String = row.try_get("node_id")?;
        let iteration: i64 = row.try_get("iteration")?;
        let Some(node) = graph.nodes.iter().find(|node| node.id == node_id) else {
            return Err(ApiError::Conflict(
                "Recovering execution has no frozen Node".into(),
            ));
        };
        if let Some(agent_run_id) = row.try_get::<Option<Uuid>, _>("linked_agent_run_id")? {
            if row.try_get::<Option<Uuid>, _>("agent_run_id")?.is_none() {
                update_node_execution(
                    pool,
                    run_id,
                    &node_id,
                    iteration,
                    NodeExecutionUpdate {
                        status: DbNodeExecutionStatus::Running,
                        input_text: None,
                        output_text: None,
                        session_id: None,
                        orchestration_node_execution_id: row
                            .try_get("orchestration_node_execution_id")?,
                        agent_run_id: Some(agent_run_id),
                        arena_group_id: None,
                        error_text: None,
                        finished: false,
                    },
                )
                .await?;
            }
            continue;
        }
        if row.try_get::<Option<Uuid>, _>("agent_run_id")?.is_some() {
            // The pending outbox or an unreachable accepted process owns this
            // identity. Reconciliation must observe it, never replace it.
            continue;
        }
        if matches!(
            node.kind,
            WorkflowNodeKind::Agent | WorkflowNodeKind::TaskSplitter | WorkflowNodeKind::Condition
        ) {
            let prompt: Option<String> = row.try_get("input_text")?;
            let Some(prompt) = prompt else {
                mark_node_failed(pool,run_id,&node_id,iteration,"Interrupted workflow launch has no persisted input; an explicit new Run is required").await?;
                continue;
            };
            let (orchestration_run_id, orchestration_node_execution_id) =
                canonical_node_identity(pool, run_id, &node_id, iteration).await?;
            let executor_config = if node.kind == WorkflowNodeKind::Condition {
                graph.router_executor_config.clone()
            } else {
                node.data.executor_config.clone()
            };
            let result = executor
                .run_agent(AgentNodeRequest {
                    run_id,
                    orchestration_run_id,
                    orchestration_node_execution_id,
                    iteration,
                    node_id: node_id.clone(),
                    session_id: row.try_get("session_id")?,
                    workspace_id,
                    prompt,
                    selected_skills: None,
                    executor_config,
                })
                .await;
            match result {
                Ok(AgentNodeExecution::Started {
                    session_id,
                    orchestration_node_execution_id,
                    agent_run_id,
                    output_text,
                }) => {
                    update_node_execution(
                        pool,
                        run_id,
                        &node_id,
                        iteration,
                        NodeExecutionUpdate {
                            status: DbNodeExecutionStatus::Running,
                            input_text: None,
                            output_text: output_text.as_deref(),
                            session_id: Some(session_id),
                            orchestration_node_execution_id: Some(orchestration_node_execution_id),
                            agent_run_id: Some(agent_run_id),
                            arena_group_id: None,
                            error_text: None,
                            finished: false,
                        },
                    )
                    .await?;
                }
                Ok(AgentNodeExecution::Completed {
                    session_id,
                    orchestration_node_execution_id,
                    agent_run_id,
                    output_text,
                }) => {
                    if node.kind == WorkflowNodeKind::Condition {
                        complete_condition_router(
                            pool,
                            run_id,
                            graph,
                            workspace_id,
                            &node_id,
                            iteration,
                            orchestration_node_execution_id,
                            agent_run_id,
                            row.try_get::<Option<String>, _>("output_text")?.as_deref(),
                            &output_text,
                        )
                        .await?;
                    } else {
                        let acceptance = if node.kind == WorkflowNodeKind::TaskSplitter {
                            accept_split_manifest(
                                pool,
                                run_id,
                                graph,
                                node,
                                iteration,
                                workspace_id,
                            )
                            .await
                        } else {
                            file_handoff::accept(pool, run_id, row.try_get("id")?, true)
                                .await
                                .map(|_| ())
                        };
                        match acceptance {
                            Ok(()) => {
                                mark_canonical_node_succeeded(
                                    pool,
                                    run_id,
                                    &node_id,
                                    iteration,
                                    Some(&output_text),
                                    Some(session_id),
                                    orchestration_node_execution_id,
                                    agent_run_id,
                                )
                                .await?
                            }
                            Err(error) => {
                                mark_canonical_node_failed(
                                    pool,
                                    run_id,
                                    &node_id,
                                    iteration,
                                    Some(session_id),
                                    orchestration_node_execution_id,
                                    agent_run_id,
                                    &error.to_string(),
                                )
                                .await?
                            }
                        }
                    }
                }
                Err(error) => {
                    mark_node_failed(pool, run_id, &node_id, iteration, &error.to_string()).await?
                }
            }
        } else if node.kind == WorkflowNodeKind::Arena {
            // Arena creation can modify repositories. Never repeat an unknown
            // side effect just because its reply was lost.
            mark_node_failed(pool,run_id,&node_id,iteration,
                "Arena creation was interrupted; inspect its retained candidate executions before submitting another Run").await?;
        } else if let Err(error) = execute_ready_node(
            pool,
            run_id,
            graph,
            node,
            iteration,
            issue_id,
            workspace_id,
            input,
            executor,
            arena,
        )
        .await
        {
            mark_node_failed(pool, run_id, &node_id, iteration, &error.to_string()).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (SqlitePool, Uuid) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(r#"
            CREATE TABLE workflow_runs(id BLOB PRIMARY KEY,orchestration_run_id BLOB,status TEXT,output_text TEXT,error_text TEXT,finished_at TEXT,updated_at TEXT);
            CREATE TABLE node_executions(id BLOB PRIMARY KEY,run_id BLOB,node_id TEXT,node_type TEXT NOT NULL DEFAULT 'agent',iteration INTEGER,status TEXT,orchestration_node_execution_id BLOB,agent_run_id BLOB,arena_group_id BLOB,error_text TEXT,finished_at TEXT,updated_at TEXT);
            CREATE TABLE workflow_run_queue(run_id BLOB PRIMARY KEY,phase TEXT);
            CREATE TABLE sessions(id BLOB PRIMARY KEY);
            CREATE TABLE workflow_run_submissions(run_id BLOB PRIMARY KEY);
            CREATE TABLE orchestration_runs(id BLOB PRIMARY KEY,product_kind TEXT,source_definition_id BLOB);
            CREATE TABLE orchestration_agent_run_links(orchestration_run_id BLOB,node_execution_id BLOB,agent_run_id BLOB);
            CREATE TABLE agent_run_state(agent_run_id BLOB PRIMARY KEY,status TEXT);
            CREATE TABLE agent_runs(id BLOB PRIMARY KEY);
            CREATE TABLE agent_run_attempts(id BLOB PRIMARY KEY,agent_run_id BLOB,status TEXT);
            CREATE TABLE agent_process_registry(run_attempt_id BLOB PRIMARY KEY,registry_status TEXT);
            CREATE TABLE orchestration_outbox(id BLOB PRIMARY KEY,orchestration_run_id BLOB,node_execution_id BLOB,delivery_status TEXT);
            CREATE TABLE workflow_interaction_responses(node_execution_id BLOB PRIMARY KEY,response_json TEXT);
        "#).execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../../db/migrations/20261013000000_workflow_file_driven.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let run_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO workflow_runs(id,orchestration_run_id,status) VALUES (?,?,'running')",
        )
        .bind(run_id)
        .bind(run_id)
        .execute(&pool)
        .await
        .unwrap();
        (pool, run_id)
    }

    async fn product_node(pool: &SqlitePool, run: Uuid, name: &str, status: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO node_executions(id,run_id,node_id,iteration,status,orchestration_node_execution_id) VALUES (?,?,?,0,?,?)")
            .bind(id).bind(run).bind(name).bind(status).bind(id).execute(pool).await.unwrap();
        id
    }

    #[tokio::test]
    async fn failure_preserves_live_peer_and_waits_for_actual_process_exit() {
        let (pool, run) = fixture().await;
        product_node(&pool, run, "failed", "failed").await;
        let peer = product_node(&pool, run, "peer", "running").await;
        let undispatched = product_node(&pool, run, "interrupted-launch", "running").await;
        let gate = product_node(&pool, run, "unanswered-review", "awaiting_human").await;
        sqlx::query("UPDATE node_executions SET node_type='human_gate' WHERE id=?")
            .bind(gate)
            .execute(&pool)
            .await
            .unwrap();
        let agent = Uuid::new_v4();
        let attempt = Uuid::new_v4();
        sqlx::query("INSERT INTO orchestration_agent_run_links VALUES (?,?,?)")
            .bind(run)
            .bind(peer)
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_run_state VALUES (?,'running')")
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_run_attempts VALUES (?,?,'running')")
            .bind(attempt)
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_process_registry VALUES (?,'running')")
            .bind(attempt)
            .execute(&pool)
            .await
            .unwrap();

        settle_workflow_failure(&pool, run, "Required work failed")
            .await
            .unwrap();
        let peer_status: String =
            sqlx::query_scalar("SELECT status FROM node_executions WHERE id=?")
                .bind(peer)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(peer_status, "running");
        for id in [undispatched, gate] {
            assert_eq!(
                sqlx::query_scalar::<_, String>("SELECT status FROM node_executions WHERE id=?")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                "skipped"
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM workflow_runs WHERE id=?")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "running"
        );
        sqlx::query("UPDATE node_executions SET status='succeeded' WHERE id=?")
            .bind(peer)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE agent_run_state SET status='succeeded' WHERE agent_run_id=?")
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE agent_run_attempts SET status='succeeded' WHERE id=?")
            .bind(attempt)
            .execute(&pool)
            .await
            .unwrap();
        settle_workflow_failure(&pool, run, "Required work failed")
            .await
            .unwrap();
        assert!(!children_settled(&pool, run).await.unwrap());
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM workflow_runs WHERE id=?")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "running"
        );
        sqlx::query(
            "UPDATE agent_process_registry SET registry_status='exited' WHERE run_attempt_id=?",
        )
        .bind(attempt)
        .execute(&pool)
        .await
        .unwrap();
        settle_workflow_failure(&pool, run, "Required work failed")
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM workflow_runs WHERE id=?")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "failed"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM node_executions WHERE id=?")
                .bind(peer)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "succeeded"
        );
    }

    #[tokio::test]
    async fn startup_failure_remains_failed_after_its_peer_succeeds() {
        let (pool, run) = fixture().await;
        let peer = product_node(&pool, run, "accepted-create", "running").await;
        sqlx::query("INSERT INTO workflow_run_queue VALUES (?,'starting')")
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        // No Node has failed: the interrupted starter itself owns this fact.
        sqlx::query("INSERT INTO orchestration_outbox VALUES (?,?,?,'failed')")
            .bind(Uuid::new_v4())
            .bind(run)
            .bind(peer)
            .execute(&pool)
            .await
            .unwrap();
        fail_accepted_workflow_run(&pool, run, "Startup projection failed")
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT phase FROM workflow_run_queue WHERE run_id=?")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "active"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM node_executions WHERE id=?")
                .bind(peer)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "running"
        );
        assert!(!children_settled(&pool, run).await.unwrap());
        update_run_status(&pool, run, WorkflowRunStatus::Succeeded, None, None, true)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM workflow_runs WHERE id=?")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "running"
        );
        sqlx::query("UPDATE orchestration_outbox SET delivery_status='delivered'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE node_executions SET status='succeeded' WHERE id=?")
            .bind(peer)
            .execute(&pool)
            .await
            .unwrap();
        fail_accepted_workflow_run(&pool, run, "A later error must not replace the cause")
            .await
            .unwrap();
        let result: (String, String) =
            sqlx::query_as("SELECT status,error_text FROM workflow_runs WHERE id=?")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            result,
            ("failed".into(), "Startup projection failed".into())
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM node_executions WHERE id=?")
                .bind(peer)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "succeeded"
        );
    }

    #[tokio::test]
    async fn failure_does_not_hide_a_live_agents_input_request() {
        let (pool, run) = fixture().await;
        let peer = product_node(&pool, run, "agent-input-request", "awaiting_human").await;
        let agent = Uuid::new_v4();
        sqlx::query("INSERT INTO orchestration_agent_run_links VALUES (?,?,?)")
            .bind(run)
            .bind(peer)
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_run_state VALUES (?,'awaiting_input')")
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        settle_workflow_failure(&pool, run, "Another node failed")
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM node_executions WHERE id=?")
                .bind(peer)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "awaiting_human"
        );
        assert!(!children_settled(&pool, run).await.unwrap());
    }

    #[tokio::test]
    async fn lost_launch_reply_keeps_the_original_durable_outbox_identity() {
        let (pool, run) = fixture().await;
        let accepted = product_node(&pool, run, "accepted-create", "running").await;
        let never_started = product_node(&pool, run, "before-create", "running").await;
        sqlx::query("INSERT INTO orchestration_outbox VALUES (?,?,?,'failed')")
            .bind(Uuid::new_v4())
            .bind(run)
            .bind(accepted)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        skip_undispatched_in(&mut tx, run, None, "Runtime interrupted")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM node_executions WHERE id=?")
                .bind(accepted)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "running"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM node_executions WHERE id=?")
                .bind(never_started)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "skipped"
        );
        assert!(!children_settled(&pool, run).await.unwrap());
    }

    #[tokio::test]
    async fn failed_manager_turn_still_blocks_completion_until_its_process_exits() {
        let (pool, run) = fixture().await;
        let split = product_node(&pool, run, "split", "succeeded").await;
        let agent = Uuid::new_v4();
        let attempt = Uuid::new_v4();
        sqlx::query("INSERT INTO workflow_split_batches(id,run_id,node_execution_id,manifest_path,manifest_sha256,state) VALUES (?,?,?,'tasks.json','hash','failed')")
            .bind(split).bind(run).bind(split).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO workflow_handover_events(id,batch_id,run_id,session_id,state,agent_run_id) VALUES (?,?,?,?,'failed',?)")
            .bind(Uuid::new_v4()).bind(split).bind(run).bind(Uuid::new_v4()).bind(agent).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO agent_runs VALUES (?)")
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_run_attempts VALUES (?,?,'failed')")
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
        assert!(!children_settled(&pool, run).await.unwrap());
        sqlx::query(
            "UPDATE agent_process_registry SET registry_status='exited' WHERE run_attempt_id=?",
        )
        .bind(attempt)
        .execute(&pool)
        .await
        .unwrap();
        assert!(children_settled(&pool, run).await.unwrap());
    }
}
