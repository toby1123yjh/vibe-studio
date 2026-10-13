use server::{error::ApiError, routes::workflows::delete_issue_workflow_attempt};
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use uuid::Uuid;

struct Fixture {
    pool: SqlitePool,
    project_id: Uuid,
    issue_id: Uuid,
    instance_id: Uuid,
    workflow_id: Uuid,
    source_run_id: Uuid,
    followup_run_id: Uuid,
    source_batch_id: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .unwrap();
        let fixture = Self {
            pool,
            project_id: Uuid::new_v4(),
            issue_id: Uuid::new_v4(),
            instance_id: Uuid::new_v4(),
            workflow_id: Uuid::new_v4(),
            source_run_id: Uuid::new_v4(),
            followup_run_id: Uuid::new_v4(),
            source_batch_id: Uuid::new_v4(),
        };
        let status_id = Uuid::new_v4();
        sqlx::query("INSERT INTO projects(id,name) VALUES (?,'delete fixture')")
            .bind(fixture.project_id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO local_project_statuses(id,project_id,name,color,sort_order,hidden) VALUES (?,?,'Todo','gray',0,0)")
            .bind(status_id).bind(fixture.project_id).execute(&fixture.pool).await.unwrap();
        sqlx::query("INSERT INTO local_issues(id,project_id,issue_number,simple_id,status_id,title,sort_order) VALUES (?,?,1,'TASK-1',?,'Preserve the business task',0)")
            .bind(fixture.issue_id).bind(fixture.project_id).bind(status_id).execute(&fixture.pool).await.unwrap();
        sqlx::query("INSERT INTO tasks(id,project_id,issue_id,title,execution_kind) VALUES (?,?,?,'Workflow instance','workflow')")
            .bind(fixture.instance_id).bind(fixture.project_id).bind(fixture.issue_id).execute(&fixture.pool).await.unwrap();
        sqlx::query("INSERT INTO workflows(id,source,project_id,name,graph_json) VALUES (?,'project',?,'Captured workflow','{}')")
            .bind(fixture.workflow_id).bind(fixture.project_id).execute(&fixture.pool).await.unwrap();
        sqlx::query("INSERT INTO workflow_attempts(id,task_id,workflow_id,issue_id,status) VALUES (?,?,?,?,'succeeded')")
            .bind(fixture.instance_id).bind(fixture.instance_id).bind(fixture.workflow_id).bind(fixture.issue_id)
            .execute(&fixture.pool).await.unwrap();
        for run_id in [fixture.source_run_id, fixture.followup_run_id] {
            sqlx::query("INSERT INTO workflow_runs(id,workflow_id,attempt_id,issue_id,input_text,status) VALUES (?,?,?,?,'request','succeeded')")
                .bind(run_id).bind(fixture.workflow_id).bind(fixture.instance_id).bind(fixture.issue_id)
                .execute(&fixture.pool).await.unwrap();
        }
        sqlx::query("UPDATE workflow_attempts SET latest_run_id=? WHERE id=?")
            .bind(fixture.followup_run_id)
            .bind(fixture.instance_id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        let splitter_execution_id = Uuid::new_v4();
        let worker_execution_id = Uuid::new_v4();
        let summary_execution_id = Uuid::new_v4();
        for (id, run_id, node_id, node_type) in [
            (
                splitter_execution_id,
                fixture.source_run_id,
                "split",
                "task_splitter",
            ),
            (
                worker_execution_id,
                fixture.source_run_id,
                "worker",
                "agent",
            ),
            (
                summary_execution_id,
                fixture.followup_run_id,
                "summary",
                "agent",
            ),
        ] {
            sqlx::query("INSERT INTO node_executions(id,run_id,node_id,node_type,status) VALUES (?,?,?,?,'succeeded')")
                .bind(id).bind(run_id).bind(node_id).bind(node_type).execute(&fixture.pool).await.unwrap();
        }
        let followup_batch_id = Uuid::new_v4();
        for (id, run_id, source_batch_id) in [
            (fixture.source_batch_id, fixture.source_run_id, None),
            (
                followup_batch_id,
                fixture.followup_run_id,
                Some(fixture.source_batch_id),
            ),
        ] {
            sqlx::query("INSERT INTO workflow_split_batches(id,run_id,node_execution_id,manifest_path,manifest_sha256,source_batch_id,state) VALUES (?,?,?,'tasks.json','manifest-hash',?,'succeeded')")
                .bind(id).bind(run_id).bind(splitter_execution_id).bind(source_batch_id).execute(&fixture.pool).await.unwrap();
        }
        let source_subtask_id = Uuid::new_v4();
        for (id, batch_id, source_id) in [
            (source_subtask_id, fixture.source_batch_id, None),
            (Uuid::new_v4(), followup_batch_id, Some(source_subtask_id)),
        ] {
            sqlx::query("INSERT INTO workflow_split_subtasks(id,batch_id,subtask_id,title,instruction_path,instruction_sha256,node_execution_id,source_subtask_id,state) VALUES (?,?,'work','Work','work.md','instruction-hash',?,?,'succeeded')")
                .bind(id).bind(batch_id).bind(worker_execution_id).bind(source_id).execute(&fixture.pool).await.unwrap();
        }
        sqlx::query("INSERT INTO workflow_result_reuse(run_id,node_id,iteration,source_node_execution_id) VALUES (?,'split',0,?)")
            .bind(fixture.followup_run_id).bind(splitter_execution_id).execute(&fixture.pool).await.unwrap();
        sqlx::query("INSERT INTO workflow_run_submissions(run_id,instance_id,caller_namespace,request_id,request_hash,action,material_paths_json,scope_json,source_run_id,source_node_execution_id,source_batch_id) VALUES (?,?,'page','partial','hash','rework','[]','{}',?,?,?)")
            .bind(fixture.followup_run_id).bind(fixture.instance_id).bind(fixture.source_run_id)
            .bind(worker_execution_id).bind(fixture.source_batch_id).execute(&fixture.pool).await.unwrap();
        sqlx::query("INSERT INTO workflow_run_deliveries(run_id,completeness,source_run_id,source_batch_id) VALUES (?,'partial',?,?)")
            .bind(fixture.followup_run_id).bind(fixture.source_run_id).bind(fixture.source_batch_id).execute(&fixture.pool).await.unwrap();
        fixture
    }

    async fn instance_exists(&self) -> bool {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_attempts WHERE id=?)")
            .bind(self.instance_id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn deletes_a_settled_instance_with_retry_and_partial_delivery_lineage() {
    let fixture = Fixture::new().await;
    delete_issue_workflow_attempt(&fixture.pool, fixture.instance_id)
        .await
        .unwrap();
    assert!(!fixture.instance_exists().await);
    for table in [
        "workflow_runs",
        "node_executions",
        "workflow_split_batches",
        "workflow_split_subtasks",
        "workflow_run_deliveries",
        "workflow_result_reuse",
        "workflow_run_submissions",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "{table} retains deleted instance data");
    }
    let issue_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_issues WHERE id=?)")
            .bind(fixture.issue_id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    assert!(issue_exists);
    let graph_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflows WHERE id=?)")
            .bind(fixture.workflow_id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    assert!(!graph_exists);
}

#[tokio::test]
async fn deletion_preserves_history_until_original_system_handover_settles() {
    let fixture = Fixture::new().await;
    sqlx::query("INSERT INTO workflow_handover_events(id,batch_id,run_id,session_id,state) VALUES (?,?,?,?,'pending')")
        .bind(Uuid::new_v4()).bind(fixture.source_batch_id).bind(fixture.source_run_id)
        .bind(Uuid::new_v4()).execute(&fixture.pool).await.unwrap();
    let error = delete_issue_workflow_attempt(&fixture.pool, fixture.instance_id)
        .await
        .unwrap_err();
    assert!(matches!(error, ApiError::Conflict(_)));
    assert!(fixture.instance_exists().await);
    let lineage_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_split_batches WHERE source_batch_id=?")
            .bind(fixture.source_batch_id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    assert_eq!(lineage_count, 1);
    sqlx::query("UPDATE workflow_handover_events SET state='failed'")
        .execute(&fixture.pool)
        .await
        .unwrap();
    delete_issue_workflow_attempt(&fixture.pool, fixture.instance_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn deletion_rejects_a_terminal_run_that_still_holds_its_project_slot() {
    let fixture = Fixture::new().await;
    sqlx::query("INSERT INTO workflow_project_slots(project_id,run_id) VALUES (?,?)")
        .bind(fixture.project_id)
        .bind(fixture.source_run_id)
        .execute(&fixture.pool)
        .await
        .unwrap();
    let error = delete_issue_workflow_attempt(&fixture.pool, fixture.instance_id)
        .await
        .unwrap_err();
    assert!(matches!(error, ApiError::Conflict(_)));
    assert!(fixture.instance_exists().await);
}
