use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use db::models::{
    scratch::DraftWorkspaceRepo,
    workflow::{NodeExecutionStatus, WorkflowAttemptStatus, WorkflowRunStatus, WorkflowSource},
};
use serde_json::{Value, json};
use server::{
    error::ApiError,
    routes::workflows::{
        CreateWorkflowAttemptRequest, CreateWorkflowRequest, RunWorkflowAttemptRequest,
        SelectArenaWinnerRequest, TriggerWorkflowRequest, UpdateWorkflowRequest,
        WorkflowActionResponse, WorkflowAttemptListResponse, WorkflowAttemptResponse,
        WorkflowNodeExecutionResponse, WorkflowRunResponse, WorkflowTemplateListResponse,
        WorkflowTemplateResponse, WorkflowUpdateError, create_issue_workflow_attempt,
        create_issue_workflow_attempt_with_resources, create_project_workflow,
        delete_issue_workflow_attempt, delete_workflow_template, fallback_node_executions_payload,
        fallback_workflow_runs_payload, fallback_workflows_payload, get_workflow_template,
        list_project_workflows, sync_attempt_from_run, update_workflow_template,
        workflow_attempt_by_id, workflow_attempt_by_workflow_id,
    },
    workflow_runtime::{
        arena::{
            ArenaNodeExecution, ArenaNodeRequest, ArenaWinnerExecution, ArenaWinnerRequest,
            NoopWorkflowArenaCreator, WorkflowArenaCreator, WorkflowArenaWinnerApplier,
        },
        runner::{
            AgentNodeExecution, AgentNodeRequest, AgentRunReconciliationBoundary,
            AgentRunReconciliationResult, WorkflowAgentExecutor, WorkflowRunCanceller,
            WorkflowWorkspaceRequest, WorkflowWorkspaceResolver, approve_human_node,
            cancel_workflow_run_runtime, reconcile_workflow_run_with_arena_and_boundary,
            recover_stale_workflow_runs, reject_human_node, retry_workflow_node,
            select_arena_winner_with_arena, trigger_workflow_run, trigger_workflow_run_with_arena,
            workflow_event_history,
        },
    },
};
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use ts_rs::TS;
use uuid::Uuid;
use workflow::files::{
    WorkflowFileRef, WorkflowHandoff, control_file_path, readable_file, sha256_bytes,
    write_json_atomic, write_text_atomic,
};

// Production helpers only accept; these fixtures explicitly play the existing
// dispatcher role with fake ports after a successful queue claim.
async fn run_workflow_attempt_runtime<W: WorkflowWorkspaceResolver, A: WorkflowAgentExecutor>(
    pool: &SqlitePool,
    attempt_id: Uuid,
    request: RunWorkflowAttemptRequest,
    resolver: &W,
    agent: &A,
) -> Result<WorkflowRunResponse, ApiError> {
    run_workflow_attempt_runtime_with_arena(
        pool,
        attempt_id,
        request,
        resolver,
        agent,
        &NoopWorkflowArenaCreator,
    )
    .await
}

async fn run_workflow_attempt_runtime_with_arena<
    W: WorkflowWorkspaceResolver,
    A: WorkflowAgentExecutor,
    R: WorkflowArenaCreator,
>(
    pool: &SqlitePool,
    attempt_id: Uuid,
    request: RunWorkflowAttemptRequest,
    resolver: &W,
    agent: &A,
    arena: &R,
) -> Result<WorkflowRunResponse, ApiError> {
    let accepted = server::routes::workflows::run_workflow_attempt_runtime_with_arena(
        pool, attempt_id, request, resolver, agent, arena,
    )
    .await?;
    let entry = db::models::workflow_queue::WorkflowQueueEntry::claim_next(pool)
        .await?
        .expect("fixture dispatcher claims accepted work");
    assert_eq!(entry.run_id, accepted.id);
    server::workflow_runtime::runner::start_accepted_workflow_run(pool, accepted.id, agent, arena)
        .await?;
    let run =
        server::workflow_runtime::runner::get_workflow_run_response(pool, accepted.id).await?;
    sync_attempt_from_run(pool, &run).await?;
    db::models::workflow_queue::WorkflowQueueEntry::release_terminal(pool, run.id).await?;
    Ok(run)
}

async fn setup_workflow_pool() -> SqlitePool {
    let project_directories = Arc::new(tempfile::tempdir().expect("project fixture directories"));
    let retained = project_directories.clone();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .after_release(move |_, _| {
            let retained = retained.clone();
            Box::pin(async move {
                drop(retained);
                Ok(true)
            })
        })
        .connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");

    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .expect("enable workflow test foreign keys");

    for statement in [
        r#"
        CREATE TABLE projects (
            id BLOB PRIMARY KEY,
            name TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE local_issues (
            id BLOB PRIMARY KEY,
            project_id BLOB NOT NULL,
            issue_number INTEGER NOT NULL,
            simple_id TEXT NOT NULL,
            status_id BLOB NOT NULL,
            title TEXT NOT NULL,
            description TEXT,
            priority TEXT,
            start_date TEXT,
            target_date TEXT,
            completed_at TEXT,
            sort_order REAL NOT NULL DEFAULT 0,
            parent_issue_id BLOB,
            parent_issue_sort_order REAL,
            extension_metadata TEXT NOT NULL DEFAULT 'null',
            creator_user_id BLOB,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE workspaces (
            id BLOB PRIMARY KEY,
            container_ref TEXT,
            workspace_kind TEXT NOT NULL DEFAULT 'worktree',
            container_ownership TEXT NOT NULL DEFAULT 'managed',
            branch TEXT NOT NULL,
            setup_completed_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            archived BOOLEAN NOT NULL DEFAULT FALSE,
            pinned BOOLEAN NOT NULL DEFAULT FALSE,
            name TEXT,
            worktree_deleted BOOLEAN NOT NULL DEFAULT FALSE
        )
        "#,
        r#"
        CREATE TABLE tasks (
            id             BLOB PRIMARY KEY,
            project_id     BLOB NOT NULL,
            issue_id       BLOB NOT NULL,
            parent_task_id BLOB,
            title          TEXT NOT NULL CHECK (length(trim(title)) > 0),
            execution_kind TEXT NOT NULL CHECK (execution_kind IN ('agent','workflow','arena')),
            created_at     TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at     TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE,
            FOREIGN KEY (issue_id) REFERENCES local_issues(id) ON DELETE CASCADE,
            FOREIGN KEY (parent_task_id) REFERENCES tasks(id) ON DELETE CASCADE
        )
        "#,
        r#"
        CREATE TABLE workflows (
            id          BLOB PRIMARY KEY,
            source      TEXT NOT NULL CHECK (source IN ('system','project')),
            project_id  BLOB,
            name        TEXT NOT NULL,
            description TEXT,
            graph_json  TEXT NOT NULL,
            revision    INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
            created_at  TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            CHECK (
                (source = 'system' AND project_id IS NULL) OR
                (source = 'project' AND project_id IS NOT NULL)
            )
        )
        "#,
        r#"
        CREATE TABLE workflow_runs (
            id             BLOB PRIMARY KEY,
            workflow_id    BLOB NOT NULL,
            attempt_id     BLOB,
            issue_id       BLOB NOT NULL,
            workspace_id   BLOB,
            trigger_source TEXT NOT NULL DEFAULT 'manual',
            input_text     TEXT NOT NULL,
            graph_snapshot TEXT,
            output_text    TEXT,
            status         TEXT NOT NULL DEFAULT 'pending',
            orchestration_run_id BLOB,
            started_at     TEXT,
            finished_at    TEXT,
            error_text     TEXT,
            created_at     TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at     TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE workflow_attempts (
            id             BLOB PRIMARY KEY,
            task_id        BLOB NOT NULL UNIQUE,
            workflow_id    BLOB NOT NULL,
            latest_run_id  BLOB,
            workspace_id   BLOB,
            status         TEXT NOT NULL DEFAULT 'draft',
            created_at     TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at     TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            UNIQUE (workflow_id),
            FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
        )
        "#,
        r#"
        CREATE TABLE node_executions (
            id             BLOB PRIMARY KEY,
            run_id         BLOB NOT NULL,
            task_id        BLOB,
            node_id        TEXT NOT NULL,
            node_type      TEXT NOT NULL,
            iteration      INTEGER NOT NULL DEFAULT 0,
            status         TEXT NOT NULL DEFAULT 'pending',
            input_text     TEXT,
            output_text    TEXT,
            session_id     BLOB,
            execution_process_id BLOB,
            orchestration_node_execution_id BLOB,
            agent_run_id BLOB,
            arena_group_id BLOB,
            tokens_used    INTEGER,
            cost_estimate  REAL,
            started_at     TEXT,
            finished_at    TEXT,
            error_text     TEXT,
            created_at     TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at     TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            UNIQUE (run_id, node_id, iteration)
        )
        "#,
        r#"
        CREATE TABLE execution_processes (
            id BLOB PRIMARY KEY,
            session_id BLOB NOT NULL,
            run_reason TEXT NOT NULL,
            executor_action TEXT NOT NULL,
            status TEXT NOT NULL,
            exit_code INTEGER,
            dropped BOOLEAN NOT NULL DEFAULT FALSE,
            started_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            completed_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE repos (
            id BLOB PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            display_name TEXT NOT NULL,
            setup_script TEXT,
            cleanup_script TEXT,
            archive_script TEXT,
            copy_files TEXT,
            parallel_setup_script BOOLEAN NOT NULL DEFAULT FALSE,
            dev_server_script TEXT,
            default_target_branch TEXT,
            default_working_dir TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE workspace_repos (
            id BLOB PRIMARY KEY,
            workspace_id BLOB NOT NULL,
            repo_id BLOB NOT NULL,
            target_branch TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE sessions (
            id BLOB PRIMARY KEY,
            workspace_id BLOB NOT NULL,
            name TEXT,
            executor TEXT,
            agent_working_dir TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE agent_task_bindings (
            task_id    BLOB PRIMARY KEY,
            session_id BLOB NOT NULL UNIQUE,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        )
        "#,
        r#"
        CREATE TABLE session_project_memberships (
            session_id BLOB PRIMARY KEY,
            project_id BLOB NOT NULL
        )
        "#,
        r#"
        CREATE TABLE arena_groups (
            id BLOB PRIMARY KEY,
            task_id BLOB NOT NULL UNIQUE,
            prompt TEXT NOT NULL,
            base_branch TEXT NOT NULL,
            mode TEXT NOT NULL DEFAULT 'implementation',
            lifecycle_status TEXT NOT NULL DEFAULT 'open',
            winner_candidate_id BLOB,
            promoted_at TEXT,
            closed_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
        )
        "#,
        r#"
        CREATE TABLE arena_candidates (
            id BLOB PRIMARY KEY,
            arena_group_id BLOB NOT NULL,
            workspace_id BLOB NOT NULL UNIQUE,
            purpose TEXT NOT NULL CHECK (purpose IN ('attempt', 'synthesis')),
            sort_order INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            UNIQUE (arena_group_id, sort_order),
            FOREIGN KEY (arena_group_id) REFERENCES arena_groups(id) ON DELETE CASCADE,
            FOREIGN KEY (workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE
        )
        "#,
        r#"
        CREATE TABLE orchestration_runs (
            id BLOB PRIMARY KEY,
            request_id BLOB NOT NULL UNIQUE,
            idempotency_key TEXT NOT NULL UNIQUE,
            correlation_id BLOB NOT NULL,
            product_kind TEXT NOT NULL,
            source_definition_id BLOB NOT NULL,
            source_definition_version TEXT NOT NULL,
            plan_schema_version INTEGER NOT NULL,
            plan_snapshot TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            projection_status TEXT NOT NULL DEFAULT 'current',
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE orchestration_state (
            orchestration_run_id BLOB PRIMARY KEY,
            state_schema_version INTEGER NOT NULL,
            reducer_version INTEGER NOT NULL,
            last_event_sequence INTEGER NOT NULL DEFAULT 0,
            last_event_id BLOB,
            status TEXT NOT NULL DEFAULT 'pending',
            projection_status TEXT NOT NULL DEFAULT 'current',
            state_json TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE orchestration_node_executions (
            id BLOB PRIMARY KEY,
            orchestration_run_id BLOB NOT NULL,
            node_key TEXT NOT NULL,
            iteration INTEGER NOT NULL DEFAULT 0,
            stable_order INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            UNIQUE (orchestration_run_id, node_key, iteration)
        )
        "#,
        r#"
        CREATE TABLE orchestration_events (
            event_id BLOB PRIMARY KEY,
            orchestration_run_id BLOB NOT NULL,
            sequence INTEGER NOT NULL,
            correlation_id BLOB NOT NULL,
            schema_version INTEGER NOT NULL,
            payload_version INTEGER NOT NULL,
            event_envelope TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            UNIQUE (orchestration_run_id, sequence)
        )
        "#,
        r#"
        CREATE TABLE orchestration_leases (
            resource_kind TEXT NOT NULL,
            resource_id BLOB NOT NULL,
            owner_id TEXT NOT NULL,
            fencing_token INTEGER NOT NULL,
            acquired_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (resource_kind, resource_id)
        )
        "#,
        r#"
        CREATE TABLE orchestration_agent_run_links (
            id BLOB PRIMARY KEY,
            orchestration_run_id BLOB NOT NULL,
            node_execution_id BLOB NOT NULL,
            agent_run_id BLOB NOT NULL,
            dispatch_idempotency_key TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            UNIQUE (node_execution_id, agent_run_id)
        )
        "#,
        r#"
        CREATE TABLE orchestration_outbox (
            id BLOB PRIMARY KEY,
            orchestration_run_id BLOB NOT NULL,
            node_execution_id BLOB,
            command_id BLOB NOT NULL UNIQUE,
            idempotency_key TEXT NOT NULL UNIQUE,
            command_schema_version INTEGER NOT NULL,
            command_envelope TEXT NOT NULL,
            delivery_status TEXT NOT NULL DEFAULT 'pending',
            delivery_attempts INTEGER NOT NULL DEFAULT 0,
            available_at TEXT NOT NULL,
            delivered_at TEXT,
            last_error TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE orchestration_inbox (
            id BLOB PRIMARY KEY,
            orchestration_run_id BLOB NOT NULL,
            source_event_id BLOB NOT NULL,
            source_agent_run_id BLOB NOT NULL,
            source_sequence INTEGER NOT NULL,
            event_envelope TEXT NOT NULL,
            consumption_status TEXT NOT NULL DEFAULT 'pending',
            received_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            consumed_at TEXT
        )
        "#,
        r#"
        CREATE TABLE orchestration_consumption (
            id BLOB PRIMARY KEY,
            orchestration_run_id BLOB NOT NULL,
            join_node_execution_id BLOB NOT NULL,
            source_node_execution_id BLOB NOT NULL,
            source_agent_run_id BLOB NOT NULL,
            source_event_id BLOB NOT NULL,
            target_node_execution_id BLOB,
            consumed_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            UNIQUE (join_node_execution_id, source_node_execution_id)
        )
        "#,
        r#"
        CREATE TABLE agent_runs (
            id BLOB PRIMARY KEY,
            session_id BLOB NOT NULL,
            workspace_id BLOB NOT NULL,
            request_id BLOB NOT NULL UNIQUE,
            idempotency_key TEXT NOT NULL UNIQUE,
            correlation_id BLOB NOT NULL,
            schema_version INTEGER NOT NULL,
            payload_version INTEGER NOT NULL,
            runtime_profile_id TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            workspace_mode TEXT NOT NULL,
            workspace_path TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            projection_status TEXT NOT NULL DEFAULT 'current',
            request_envelope TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE agent_turns (
            id BLOB PRIMARY KEY,
            agent_run_id BLOB NOT NULL UNIQUE,
            request_id BLOB NOT NULL UNIQUE,
            turn_number INTEGER NOT NULL,
            intent TEXT NOT NULL,
            input_message TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE agent_run_attempts (
            id BLOB PRIMARY KEY,
            agent_run_id BLOB NOT NULL,
            turn_id BLOB NOT NULL,
            request_id BLOB NOT NULL UNIQUE,
            idempotency_key TEXT NOT NULL UNIQUE,
            attempt_number INTEGER NOT NULL,
            mode TEXT NOT NULL,
            transport TEXT NOT NULL,
            schema_version INTEGER NOT NULL,
            payload_version INTEGER NOT NULL,
            capability_snapshot TEXT NOT NULL,
            request_envelope TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            projection_status TEXT NOT NULL DEFAULT 'current',
            started_at TEXT,
            finished_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            UNIQUE (agent_run_id, attempt_number)
        )
        "#,
        r#"
        CREATE TABLE agent_process_registry (
            id BLOB PRIMARY KEY,
            run_attempt_id BLOB NOT NULL UNIQUE,
            registry_status TEXT NOT NULL DEFAULT 'reserved',
            pid INTEGER,
            process_group_id INTEGER,
            process_started_at TEXT,
            executable TEXT,
            command_fingerprint TEXT,
            exit_code INTEGER,
            observed_exited_at TEXT,
            lease_owner TEXT,
            lease_expires_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE agent_run_state (
            agent_run_id BLOB PRIMARY KEY,
            state_schema_version INTEGER NOT NULL,
            reducer_version INTEGER NOT NULL,
            last_run_attempt_id BLOB,
            last_run_attempt_number INTEGER NOT NULL DEFAULT 0,
            last_event_sequence INTEGER NOT NULL DEFAULT 0,
            last_event_id BLOB,
            status TEXT NOT NULL DEFAULT 'pending',
            projection_status TEXT NOT NULL DEFAULT 'current',
            state_json TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
        )
        "#,
        r#"
        CREATE TABLE agent_events (
            event_id BLOB PRIMARY KEY,
            session_id BLOB NOT NULL,
            agent_run_id BLOB NOT NULL,
            turn_id BLOB NOT NULL,
            run_attempt_id BLOB NOT NULL,
            run_attempt_number INTEGER NOT NULL,
            sequence INTEGER NOT NULL,
            correlation_id BLOB NOT NULL,
            schema_version INTEGER NOT NULL,
            payload_version INTEGER NOT NULL,
            event_envelope TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            UNIQUE (run_attempt_id, sequence)
        )
        "#,
    ] {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .expect("create workflow test schema");
    }

    sqlx::query(
        "CREATE UNIQUE INDEX idx_node_executions_task_id ON node_executions(task_id) WHERE task_id IS NOT NULL",
    )
    .execute(&pool)
    .await
    .expect("create pre-integration Node Task index");
    sqlx::raw_sql(include_str!(
        "../../db/migrations/20260929000000_external_integrations.sql"
    ))
    .execute(&pool)
    .await
    .expect("apply external integration fixture schema");
    sqlx::raw_sql(include_str!(
        "../../db/migrations/20260929010000_workflow_integration_queue.sql"
    ))
    .execute(&pool)
    .await
    .expect("apply workflow integration schema");
    sqlx::raw_sql(include_str!(
        "../../db/migrations/20261003000000_workflow_management.sql"
    ))
    .execute(&pool)
    .await
    .expect("apply workflow management schema");

    for statement in [
        "CREATE TABLE local_project_metadata(project_id BLOB PRIMARY KEY,organization_id BLOB NOT NULL,color TEXT NOT NULL,sort_order INTEGER NOT NULL)",
        "CREATE TABLE local_project_statuses(id BLOB PRIMARY KEY,project_id BLOB NOT NULL,name TEXT NOT NULL,color TEXT NOT NULL,sort_order INTEGER NOT NULL,hidden INTEGER NOT NULL,created_at TEXT NOT NULL DEFAULT (datetime('now','subsec')))",
        "CREATE TABLE local_tags(id BLOB PRIMARY KEY,project_id BLOB NOT NULL,name TEXT NOT NULL,color TEXT NOT NULL)",
        "CREATE TABLE local_issue_tags(id BLOB PRIMARY KEY,issue_id BLOB NOT NULL,tag_id BLOB NOT NULL)",
        "CREATE TABLE local_issue_assignees(id BLOB PRIMARY KEY,issue_id BLOB NOT NULL,user_id BLOB NOT NULL,assigned_at TEXT NOT NULL)",
        "CREATE TABLE local_issue_followers(id BLOB PRIMARY KEY,issue_id BLOB NOT NULL,user_id BLOB NOT NULL)",
        "CREATE TABLE local_issue_relationships(id BLOB PRIMARY KEY,issue_id BLOB NOT NULL,related_issue_id BLOB NOT NULL,relationship_type TEXT NOT NULL,created_at TEXT NOT NULL)",
        "CREATE TABLE scratch(id BLOB NOT NULL,scratch_type TEXT NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(id,scratch_type))",
        "CREATE TABLE project_repos(project_id BLOB NOT NULL,repo_id BLOB NOT NULL)",
        "CREATE TABLE fixture_project_root(directory_path TEXT NOT NULL)",
    ] {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .expect("portable task fixture schema");
    }
    sqlx::raw_sql(include_str!(
        "../../db/migrations/20261008120000_project_task_mounts.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../db/migrations/20261013000000_workflow_file_driven.sql"
    ))
    .execute(&pool)
    .await
    .expect("apply file-driven workflow fixture schema");
    sqlx::query("INSERT INTO fixture_project_root(directory_path) VALUES (?)")
        .bind(project_directories.path().to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap();

    pool
}

async fn insert_project(pool: &SqlitePool, project_id: Uuid) {
    sqlx::query("INSERT INTO projects (id, name) VALUES (?, ?)")
        .bind(project_id)
        .bind("Workflow Project")
        .execute(pool)
        .await
        .expect("insert project");
    let root: String = sqlx::query_scalar("SELECT directory_path FROM fixture_project_root")
        .fetch_one(pool)
        .await
        .unwrap();
    let path = std::path::PathBuf::from(root).join(project_id.to_string());
    tokio::fs::create_dir_all(&path).await.unwrap();
    sqlx::query("INSERT INTO scratch(id,scratch_type,payload) VALUES (?,'PROJECT_REPO_DEFAULTS',?)").bind(project_id).bind(json!({"type":"PROJECT_REPO_DEFAULTS","data":{"repos":[],"directory_path":path.to_string_lossy()}}).to_string()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO local_project_statuses(id,project_id,name,color,sort_order,hidden) VALUES (?,?,'Todo','210 80% 52%',0,0)").bind(Uuid::new_v4()).bind(project_id).execute(pool).await.unwrap();
}

async fn insert_local_issue(pool: &SqlitePool, project_id: Uuid, issue_id: Uuid, title: &str) {
    sqlx::query("INSERT INTO local_issues (id, project_id, title,issue_number,simple_id,status_id) SELECT ?,?,?,COALESCE((SELECT MAX(issue_number) FROM local_issues WHERE project_id=?),0)+1,'TASK-'||?,(SELECT id FROM local_project_statuses WHERE project_id=? LIMIT 1)")
        .bind(issue_id)
        .bind(project_id)
        .bind(title)
        .bind(project_id)
        .bind(issue_id.to_string())
        .bind(project_id)
        .execute(pool)
        .await
        .expect("insert local issue");
    let mounted: Option<String> =
        sqlx::query_scalar("SELECT directory_path FROM project_task_mounts WHERE project_id=?")
            .bind(project_id)
            .fetch_optional(pool)
            .await
            .unwrap();
    if let Some(root) = mounted {
        let path = std::path::PathBuf::from(root).join(".vibe-kanban/project.json");
        let mut document: Value =
            serde_json::from_slice(&tokio::fs::read(&path).await.unwrap()).unwrap();
        let number: i64 = sqlx::query_scalar("SELECT issue_number FROM local_issues WHERE id=?")
            .bind(issue_id)
            .fetch_one(pool)
            .await
            .unwrap();
        let status: Uuid = sqlx::query_scalar("SELECT status_id FROM local_issues WHERE id=?")
            .bind(issue_id)
            .fetch_one(pool)
            .await
            .unwrap();
        let now = chrono::Utc::now();
        document["tasks"].as_array_mut().unwrap().push(json!({"id":issue_id,"project_id":project_id,"task_number":number,"simple_id":format!("TASK-{issue_id}"),"status_id":status,"title":title,"description":null,"priority":null,"start_date":null,"target_date":null,"completed_at":null,"sort_order":0.0,"parent_task_id":null,"parent_task_sort_order":null,"extension_metadata":null,"creator_user_id":null,"created_at":now,"updated_at":now}));
        document["revision"] = json!(document["revision"].as_i64().unwrap() + 1);
        tokio::fs::write(path, serde_json::to_vec_pretty(&document).unwrap())
            .await
            .unwrap();
    }
}

async fn link_canonical_orchestration_fixture(
    pool: &SqlitePool,
    run_id: Uuid,
    node_execution_id: Uuid,
    node_id: &str,
) {
    let orchestration_run_id = Uuid::new_v4();
    let orchestration_node_execution_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO orchestration_runs (
            id, request_id, idempotency_key, correlation_id, product_kind,
            source_definition_id, source_definition_version,
            plan_schema_version, plan_snapshot, status
        ) VALUES (?, ?, ?, ?, 'workflow', ?, '1', 1, '{}', 'running')
        "#,
    )
    .bind(orchestration_run_id)
    .bind(Uuid::new_v4())
    .bind(format!("workflow-fixture-{orchestration_run_id}"))
    .bind(Uuid::new_v4())
    .bind(run_id)
    .execute(pool)
    .await
    .expect("insert canonical orchestration run");
    sqlx::query(
        r#"
        INSERT INTO orchestration_node_executions (
            id, orchestration_run_id, node_key, iteration, stable_order, status
        ) VALUES (?, ?, ?, 0, 0, 'running')
        "#,
    )
    .bind(orchestration_node_execution_id)
    .bind(orchestration_run_id)
    .bind(node_id)
    .execute(pool)
    .await
    .expect("insert canonical orchestration node execution");
    sqlx::query("UPDATE workflow_runs SET orchestration_run_id = ? WHERE id = ?")
        .bind(orchestration_run_id)
        .bind(run_id)
        .execute(pool)
        .await
        .expect("link Workflow run to canonical orchestration run");
    sqlx::query("UPDATE node_executions SET orchestration_node_execution_id = ? WHERE id = ?")
        .bind(orchestration_node_execution_id)
        .bind(node_execution_id)
        .execute(pool)
        .await
        .expect("link Workflow node to canonical orchestration node");
}

async fn insert_project_workflow(pool: &SqlitePool, project_id: Uuid) -> Uuid {
    let workflow_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Project Flow', 'project template', ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(valid_graph_json())
    .execute(pool)
    .await
    .expect("insert project workflow");
    workflow_id
}

fn valid_graph_json() -> String {
    json!({
        "version": 1,
        "nodes": [
            { "id": "start", "type": "start", "data": { "display_name": "Start" } },
            { "id": "end", "type": "end", "data": { "display_name": "End" } }
        ],
        "edges": [
            { "id": "e1", "source": "start", "target": "end", "type": "default" }
        ]
    })
    .to_string()
}

fn agent_graph_json() -> String {
    json!({
        "version": 1,
        "nodes": [
            { "id": "start", "type": "start", "data": { "display_name": "Start" } },
            {
                "id": "agent",
                "type": "agent",
                "data": {
                    "display_name": "Implementer",
                    "prompt_template": "Implement this: {{upstream}}"
                }
            },
            { "id": "end", "type": "end", "data": { "display_name": "End" } }
        ],
        "edges": [
            { "id": "e1", "source": "start", "target": "agent", "type": "default" },
            { "id": "e2", "source": "agent", "target": "end", "type": "default" }
        ]
    })
    .to_string()
}

fn two_agent_graph_json() -> String {
    json!({
        "version": 1,
        "nodes": [
            { "id": "start", "type": "start", "data": { "display_name": "Start" } },
            {
                "id": "agent-a",
                "type": "agent",
                "data": {
                    "display_name": "Agent A",
                    "prompt_template": "Agent A prompt"
                }
            },
            {
                "id": "agent-b",
                "type": "agent",
                "data": {
                    "display_name": "Agent B",
                    "prompt_template": "Agent B prompt"
                }
            },
            { "id": "end", "type": "end", "data": { "display_name": "End" } }
        ],
        "edges": [
            { "id": "e1", "source": "start", "target": "agent-a", "type": "default" },
            { "id": "e2", "source": "agent-a", "target": "agent-b", "type": "default" },
            { "id": "e3", "source": "agent-b", "target": "end", "type": "default" }
        ]
    })
    .to_string()
}

fn human_gate_graph_json() -> String {
    json!({
        "version": 1,
        "nodes": [
            { "id": "start", "type": "start", "data": { "display_name": "Start" } },
            {
                "id": "gate",
                "type": "human_gate",
                "data": {
                    "display_name": "Approve plan",
                    "prompt_to_human": "Approve this plan?",
                    "required_action": "approve_or_reject"
                }
            },
            {
                "id": "agent",
                "type": "agent",
                "data": {
                    "display_name": "Implementer",
                    "prompt_template": "After approval: {{upstream}}"
                }
            },
            { "id": "end", "type": "end", "data": { "display_name": "End" } }
        ],
        "edges": [
            { "id": "e1", "source": "start", "target": "gate", "type": "default" },
            { "id": "e2", "source": "gate", "target": "agent", "type": "default" },
            { "id": "e3", "source": "agent", "target": "end", "type": "default" }
        ]
    })
    .to_string()
}

fn arena_graph_json() -> String {
    json!({
        "version": 1,
        "nodes": [
            { "id": "start", "type": "start", "data": { "display_name": "Start" } },
            {
                "id": "plan",
                "type": "transform",
                "data": {
                    "display_name": "Plan",
                    "mode": "template",
                    "template": "Plan: {{input}}"
                }
            },
            {
                "id": "arena",
                "type": "arena",
                "data": {
                    "display_name": "Arena implementation",
                    "prompt_template": "Implement candidates from {{upstream}}",
                    "attempts": [
                        { "id": "a", "display_name": "Candidate A" },
                        { "id": "b", "display_name": "Candidate B" },
                        { "id": "c", "display_name": "Candidate C" }
                    ],
                    "promote_strategy": "manual",
                    "apply_strategy": "diff_apply"
                }
            },
            { "id": "end", "type": "end", "data": { "display_name": "End" } }
        ],
        "edges": [
            { "id": "e1", "source": "start", "target": "plan", "type": "default" },
            { "id": "e2", "source": "plan", "target": "arena", "type": "default" },
            { "id": "e3", "source": "arena", "target": "end", "type": "arena_winner" }
        ]
    })
    .to_string()
}

fn structural_graph_json() -> String {
    json!({
        "version": 2,
        "router_executor_config": { "executor": "codex" },
        "nodes": [
            { "id": "start", "type": "start", "data": { "display_name": "Start" } },
            {
                "id": "transform",
                "type": "transform",
                "data": {
                    "display_name": "Prepare",
                    "mode": "template",
                    "template": "Prepared: {{input}}"
                }
            },
            {
                "id": "gate",
                "type": "human_gate",
                "data": {
                    "display_name": "Approve",
                    "prompt_to_human": "Continue?",
                    "required_action": "approve_or_reject"
                }
            },
            {
                "id": "condition",
                "type": "condition",
                "data": {
                    "display_name": "Route",
                    "routing_mode": "single",
                    "branches": [{
                        "target_node_id": "end",
                        "condition": "Continue to the end."
                    }]
                }
            },
            { "id": "end", "type": "end", "data": { "display_name": "End" } }
        ],
        "edges": [
            { "id": "e1", "source": "start", "target": "transform", "type": "default" },
            { "id": "e2", "source": "transform", "target": "gate", "type": "default" },
            { "id": "e3", "source": "gate", "target": "condition", "type": "approval" },
            { "id": "e4", "source": "condition", "target": "end", "type": "condition_branch" }
        ]
    })
    .to_string()
}

fn fan_in_agent_graph_json() -> String {
    json!({
        "version": 2,
        "nodes": [
            { "id": "start", "type": "start", "data": { "display_name": "Start" } },
            {
                "id": "scan",
                "type": "transform",
                "data": {
                    "display_name": "Scan code",
                    "mode": "template",
                    "template": "Scan: {{input}}"
                }
            },
            {
                "id": "summarize",
                "type": "transform",
                "data": {
                    "display_name": "Summarize task",
                    "mode": "template",
                    "template": "Summary: {{input}}"
                }
            },
            {
                "id": "review",
                "type": "agent",
                "data": {
                    "display_name": "Review",
                    "prompt_template": "Review the current worktree."
                }
            },
            { "id": "end", "type": "end", "data": { "display_name": "End" } }
        ],
        "edges": [
            { "id": "e1", "source": "start", "target": "scan", "type": "default" },
            { "id": "e2", "source": "start", "target": "summarize", "type": "default" },
            { "id": "e3", "source": "scan", "target": "review", "type": "default" },
            { "id": "e4", "source": "summarize", "target": "review", "type": "default" },
            { "id": "e5", "source": "review", "target": "end", "type": "default" }
        ]
    })
    .to_string()
}

fn failing_transform_graph_json() -> String {
    json!({
        "version": 1,
        "nodes": [
            { "id": "start", "type": "start", "data": { "display_name": "Start" } },
            {
                "id": "transform",
                "type": "transform",
                "data": {
                    "display_name": "Extract ticket",
                    "mode": "regex_extract",
                    "regex": "NO_MATCH-(\\d+)"
                }
            },
            { "id": "end", "type": "end", "data": { "display_name": "End" } }
        ],
        "edges": [
            { "id": "e1", "source": "start", "target": "transform", "type": "default" },
            { "id": "e2", "source": "transform", "target": "end", "type": "default" }
        ]
    })
    .to_string()
}

fn fixed_transform_graph_json() -> String {
    json!({
        "version": 1,
        "nodes": [
            { "id": "start", "type": "start", "data": { "display_name": "Start" } },
            {
                "id": "transform",
                "type": "transform",
                "data": {
                    "display_name": "Summarize",
                    "mode": "template",
                    "template": "Fixed: {{input}}"
                }
            },
            { "id": "end", "type": "end", "data": { "display_name": "End" } }
        ],
        "edges": [
            { "id": "e1", "source": "start", "target": "transform", "type": "default" },
            { "id": "e2", "source": "transform", "target": "end", "type": "default" }
        ]
    })
    .to_string()
}

fn failing_condition_graph_json() -> String {
    json!({
        "version": 1,
        "nodes": [
            { "id": "start", "type": "start", "data": { "display_name": "Start" } },
            {
                "id": "condition",
                "type": "condition",
                "data": {
                    "display_name": "Condition router",
                    "routing_mode": "single",
                    "branches": [
                        {
                            "target_node_id": "end",
                            "condition": "Continue when the router selects this branch."
                        }
                    ]
                }
            },
            { "id": "end", "type": "end", "data": { "display_name": "End" } }
        ],
        "edges": [
            { "id": "e1", "source": "start", "target": "condition", "type": "default" },
            { "id": "e2", "source": "condition", "target": "end", "type": "condition_branch" }
        ]
    })
    .to_string()
}

fn graph_without_start_json() -> String {
    json!({
        "version": 1,
        "nodes": [
            { "id": "agent", "type": "agent", "data": { "display_name": "Agent" } },
            { "id": "end", "type": "end", "data": { "display_name": "End" } }
        ],
        "edges": [
            { "id": "e1", "source": "agent", "target": "end", "type": "default" }
        ]
    })
    .to_string()
}

fn unreachable_draft_graph_json() -> String {
    json!({
        "version": 2,
        "nodes": [
            { "id": "start", "type": "start", "data": { "display_name": "Start" } },
            { "id": "end", "type": "end", "data": { "display_name": "End" } },
            {
                "id": "agent-draft",
                "type": "agent",
                "data": {
                    "display_name": "Draft Agent",
                    "prompt_template": "Draft prompt"
                }
            }
        ],
        "edges": [
            {
                "id": "start-end",
                "source": "start",
                "source_handle": "output-right",
                "target": "end",
                "target_handle": "input-left",
                "type": "default"
            }
        ]
    })
    .to_string()
}

#[derive(Debug)]
struct FakeWorkspaceResolver {
    pool: SqlitePool,
    workspace_id: Uuid,
    requests: Mutex<Vec<WorkflowWorkspaceRequest>>,
}

impl FakeWorkspaceResolver {
    fn new(pool: &SqlitePool, workspace_id: Uuid) -> Self {
        Self {
            pool: pool.clone(),
            workspace_id,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<WorkflowWorkspaceRequest> {
        self.requests.lock().expect("workspace requests").clone()
    }
}

#[async_trait::async_trait]
impl WorkflowWorkspaceResolver for FakeWorkspaceResolver {
    async fn create_or_bind_main_workspace(
        &self,
        request: WorkflowWorkspaceRequest,
    ) -> Result<Uuid, ApiError> {
        self.requests
            .lock()
            .expect("workspace requests")
            .push(request.clone());

        let root: String = sqlx::query_scalar("SELECT directory_path FROM fixture_project_root")
            .fetch_one(&self.pool)
            .await?;
        let workspace_id = request.existing_workspace_id.unwrap_or(self.workspace_id);
        let directory = PathBuf::from(root).join(format!("workspace-{workspace_id}"));
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| ApiError::BadRequest(format!("create fixture workspace: {error}")))?;

        sqlx::query(
            r#"
            INSERT INTO workspaces (id, container_ref, workspace_kind, container_ownership, branch)
            VALUES (?, ?, 'worktree', 'managed', ?)
            ON CONFLICT(id) DO NOTHING
            "#,
        )
        .bind(workspace_id)
        .bind(directory.to_string_lossy().as_ref())
        .bind(format!("workflow-{}", request.run_id))
        .execute(&self.pool)
        .await
        .map_err(ApiError::Database)?;

        Ok(workspace_id)
    }

    async fn cleanup_created_main_workspace(&self, workspace_id: Uuid) -> Result<(), ApiError> {
        sqlx::query("DELETE FROM workspaces WHERE id = ?")
            .bind(workspace_id)
            .execute(&self.pool)
            .await
            .map_err(ApiError::Database)?;
        Ok(())
    }
}

async fn fixture_workspace_root(
    pool: &SqlitePool,
    workspace_id: Uuid,
) -> Result<PathBuf, ApiError> {
    let root: String = sqlx::query_scalar("SELECT container_ref FROM workspaces WHERE id=?")
        .bind(workspace_id)
        .fetch_one(pool)
        .await?;
    Ok(PathBuf::from(root))
}

// A fake Agent is still a real file producer. Its chat reply does not create
// an accepted handoff, and the workflow execution id is not the orchestration id.
async fn write_fake_agent_delivery(
    pool: &SqlitePool,
    request: &AgentNodeRequest,
    output: &str,
) -> Result<(), ApiError> {
    let execution_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM node_executions WHERE run_id=? AND node_id=? AND iteration=?",
    )
    .bind(request.run_id)
    .bind(&request.node_id)
    .bind(request.iteration)
    .fetch_one(pool)
    .await?;
    write_fake_delivery(
        pool,
        request.run_id,
        request.workspace_id,
        execution_id,
        output,
    )
    .await?;
    Ok(())
}

async fn write_fake_delivery(
    pool: &SqlitePool,
    run_id: Uuid,
    workspace_id: Uuid,
    execution_id: Uuid,
    output: &str,
) -> Result<(WorkflowFileRef, WorkflowFileRef), ApiError> {
    let root = fixture_workspace_root(pool, workspace_id).await?;
    let summary_path = format!(".vibe-kanban/workflows/{run_id}/results/{execution_id}.md");
    let index_path = format!(".vibe-kanban/workflows/{run_id}/handoffs/{execution_id}.json");
    let handoff = WorkflowHandoff {
        schema_version: 1,
        run_id: run_id.to_string(),
        node_execution_id: execution_id.to_string(),
        summary_path: Some(summary_path.clone()),
        artifact_paths: Vec::new(),
    };
    let fixture_error = |error: workflow::files::WorkflowFileError| {
        ApiError::BadRequest(format!("write fixture workflow delivery: {error}"))
    };
    let summary = control_file_path(&root, &summary_path)
        .await
        .map_err(fixture_error)?;
    write_text_atomic(&summary, output)
        .await
        .map_err(fixture_error)?;
    let index = control_file_path(&root, &index_path)
        .await
        .map_err(fixture_error)?;
    write_json_atomic(&index, &handoff)
        .await
        .map_err(fixture_error)?;
    let bytes = serde_json::to_vec_pretty(&handoff)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    Ok((
        WorkflowFileRef {
            path: index_path,
            sha256: sha256_bytes(&bytes),
        },
        WorkflowFileRef {
            path: summary_path,
            sha256: sha256_bytes(output.as_bytes()),
        },
    ))
}

async fn accepted_delivery_contents(
    pool: &SqlitePool,
    run_id: Uuid,
    execution_id: Uuid,
) -> Vec<String> {
    let (accepted_run_id, path, hash, files): (Uuid, String, String, String) = sqlx::query_as(
        "SELECT run_id,handoff_path,handoff_sha256,files_json FROM workflow_node_handoffs WHERE node_execution_id=?",
    )
    .bind(execution_id)
    .fetch_one(pool)
    .await
    .expect("successful execution has an accepted handoff");
    assert_eq!(accepted_run_id, run_id);
    assert_eq!(
        path,
        format!(".vibe-kanban/workflows/{run_id}/handoffs/{execution_id}.json")
    );
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM workflow_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(pool)
            .await
            .unwrap();
    let root = fixture_workspace_root(pool, workspace_id).await.unwrap();
    let bytes = tokio::fs::read(readable_file(&root, &path).await.unwrap())
        .await
        .unwrap();
    assert_eq!(sha256_bytes(&bytes), hash);
    let handoff: WorkflowHandoff = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(handoff.schema_version, 1);
    assert_eq!(handoff.run_id, run_id.to_string());
    assert_eq!(handoff.node_execution_id, execution_id.to_string());
    let references: Vec<WorkflowFileRef> = serde_json::from_str(&files).unwrap();
    let mut paths = handoff.artifact_paths;
    if let Some(summary) = handoff.summary_path {
        paths.insert(0, summary);
    }
    paths.dedup();
    assert_eq!(
        references
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>(),
        paths
    );
    let mut contents = Vec::new();
    for reference in references {
        let path = readable_file(&root, &reference.path).await.unwrap();
        let bytes = tokio::fs::read(path).await.unwrap();
        assert_eq!(sha256_bytes(&bytes), reference.sha256);
        contents.push(String::from_utf8(bytes).expect("fixture delivery is UTF-8 text"));
    }
    contents
}

async fn assert_run_delivery(pool: &SqlitePool, run: &WorkflowRunResponse, expected: &str) {
    let end = run
        .nodes
        .iter()
        .filter(|node| node.node_id == "end" && node.status == NodeExecutionStatus::Succeeded)
        .min_by_key(|node| node.iteration)
        .expect("successful workflow has a completed End execution");
    let handoff = format!(
        "Read workflow handoff: `.vibe-kanban/workflows/{}/handoffs/{}.json`",
        run.id, end.id
    );
    assert_eq!(run.output_text.as_deref(), Some(handoff.as_str()));
    assert_eq!(
        accepted_delivery_contents(pool, run.id, end.id).await,
        vec![expected]
    );
}

#[derive(Debug)]
struct FakeAgentExecutor {
    pool: SqlitePool,
    session_id: Uuid,
    execution_process_id: Uuid,
    output_text: String,
    requests: Mutex<Vec<AgentNodeRequest>>,
}

impl FakeAgentExecutor {
    fn new(pool: &SqlitePool, session_id: Uuid, output_text: impl Into<String>) -> Self {
        Self::with_execution_process(pool, session_id, Uuid::new_v4(), output_text)
    }

    fn with_execution_process(
        pool: &SqlitePool,
        session_id: Uuid,
        execution_process_id: Uuid,
        output_text: impl Into<String>,
    ) -> Self {
        Self {
            pool: pool.clone(),
            session_id,
            execution_process_id,
            output_text: output_text.into(),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<AgentNodeRequest> {
        self.requests.lock().expect("agent requests").clone()
    }
}

#[async_trait::async_trait]
impl WorkflowAgentExecutor for FakeAgentExecutor {
    async fn run_agent(&self, request: AgentNodeRequest) -> Result<AgentNodeExecution, ApiError> {
        write_fake_agent_delivery(&self.pool, &request, &self.output_text).await?;
        self.requests.lock().expect("agent requests").push(request);

        Ok(AgentNodeExecution::Completed {
            session_id: self.session_id,
            orchestration_node_execution_id: self.execution_process_id,
            agent_run_id: self.execution_process_id,
            output_text: self.output_text.clone(),
        })
    }
}

#[derive(Debug)]
struct BoundSessionAgentExecutor {
    pool: SqlitePool,
}

impl BoundSessionAgentExecutor {
    fn new(pool: &SqlitePool) -> Self {
        Self { pool: pool.clone() }
    }
}

#[async_trait::async_trait]
impl WorkflowAgentExecutor for BoundSessionAgentExecutor {
    async fn run_agent(&self, request: AgentNodeRequest) -> Result<AgentNodeExecution, ApiError> {
        write_fake_agent_delivery(&self.pool, &request, "bound session completed").await?;
        Ok(AgentNodeExecution::Completed {
            session_id: request.session_id.ok_or_else(|| {
                ApiError::BadRequest("Workflow Agent Task has no bound Session".to_string())
            })?,
            orchestration_node_execution_id: Uuid::new_v4(),
            agent_run_id: Uuid::new_v4(),
            output_text: "bound session completed".to_string(),
        })
    }
}

#[derive(Debug)]
struct StartedAgentExecutor {
    session_id: Uuid,
    execution_process_id: Uuid,
    requests: Mutex<Vec<AgentNodeRequest>>,
}

impl StartedAgentExecutor {
    fn new(session_id: Uuid) -> Self {
        Self {
            session_id,
            execution_process_id: Uuid::new_v4(),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn execution_process_id(&self) -> Uuid {
        self.execution_process_id
    }
}

#[async_trait::async_trait]
impl WorkflowAgentExecutor for StartedAgentExecutor {
    async fn run_agent(&self, request: AgentNodeRequest) -> Result<AgentNodeExecution, ApiError> {
        self.requests.lock().expect("agent requests").push(request);

        Ok(AgentNodeExecution::Started {
            session_id: self.session_id,
            orchestration_node_execution_id: self.execution_process_id,
            agent_run_id: self.execution_process_id,
            output_text: Some("agent still running".to_string()),
        })
    }
}

#[derive(Debug)]
struct AsyncThenCompleteAgentExecutor {
    pool: SqlitePool,
    first_session_id: Uuid,
    first_execution_process_id: Uuid,
    second_session_id: Uuid,
    second_execution_process_id: Uuid,
    requests: Mutex<Vec<AgentNodeRequest>>,
}

impl AsyncThenCompleteAgentExecutor {
    fn new(
        pool: &SqlitePool,
        first_session_id: Uuid,
        first_execution_process_id: Uuid,
        second_session_id: Uuid,
        second_execution_process_id: Uuid,
    ) -> Self {
        Self {
            pool: pool.clone(),
            first_session_id,
            first_execution_process_id,
            second_session_id,
            second_execution_process_id,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<AgentNodeRequest> {
        self.requests.lock().expect("agent requests").clone()
    }
}

#[async_trait::async_trait]
impl WorkflowAgentExecutor for AsyncThenCompleteAgentExecutor {
    async fn run_agent(&self, request: AgentNodeRequest) -> Result<AgentNodeExecution, ApiError> {
        let node_id = request.node_id.clone();
        self.requests
            .lock()
            .expect("agent requests")
            .push(request.clone());

        if node_id == "agent-a" {
            return Ok(AgentNodeExecution::Started {
                session_id: self.first_session_id,
                orchestration_node_execution_id: self.first_execution_process_id,
                agent_run_id: self.first_execution_process_id,
                output_text: Some("agent A started".to_string()),
            });
        }

        write_fake_agent_delivery(&self.pool, &request, "agent B done").await?;
        Ok(AgentNodeExecution::Completed {
            session_id: self.second_session_id,
            orchestration_node_execution_id: self.second_execution_process_id,
            agent_run_id: self.second_execution_process_id,
            output_text: "agent B done".to_string(),
        })
    }
}

#[derive(Debug)]
struct FailOnceAgentExecutor {
    pool: SqlitePool,
    session_id: Uuid,
    output_text: String,
    remaining_failures: Mutex<usize>,
    requests: Mutex<Vec<AgentNodeRequest>>,
}

impl FailOnceAgentExecutor {
    fn new(pool: &SqlitePool, session_id: Uuid, output_text: impl Into<String>) -> Self {
        Self {
            pool: pool.clone(),
            session_id,
            output_text: output_text.into(),
            remaining_failures: Mutex::new(1),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<AgentNodeRequest> {
        self.requests.lock().expect("agent requests").clone()
    }
}

#[async_trait::async_trait]
impl WorkflowAgentExecutor for FailOnceAgentExecutor {
    async fn run_agent(&self, request: AgentNodeRequest) -> Result<AgentNodeExecution, ApiError> {
        self.requests
            .lock()
            .expect("agent requests")
            .push(request.clone());

        {
            let mut remaining_failures =
                self.remaining_failures.lock().expect("remaining failures");
            if *remaining_failures > 0 {
                *remaining_failures -= 1;
                return Err(ApiError::BadRequest("agent failed once".to_string()));
            }
        }

        write_fake_agent_delivery(&self.pool, &request, &self.output_text).await?;
        Ok(AgentNodeExecution::Completed {
            session_id: self.session_id,
            orchestration_node_execution_id: Uuid::new_v4(),
            agent_run_id: Uuid::new_v4(),
            output_text: self.output_text.clone(),
        })
    }
}

#[derive(Debug)]
struct FakeArenaCreator {
    group_id: Uuid,
    pool: Option<SqlitePool>,
    requests: Mutex<Vec<ArenaNodeRequest>>,
}

impl FakeArenaCreator {
    fn new(group_id: Uuid) -> Self {
        Self {
            group_id,
            pool: None,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn persisting(pool: &SqlitePool, group_id: Uuid) -> Self {
        Self {
            group_id,
            pool: Some(pool.clone()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<ArenaNodeRequest> {
        self.requests.lock().expect("arena requests").clone()
    }
}

#[async_trait::async_trait]
impl WorkflowArenaCreator for FakeArenaCreator {
    async fn create_arena(
        &self,
        request: ArenaNodeRequest,
    ) -> Result<ArenaNodeExecution, ApiError> {
        self.requests
            .lock()
            .expect("arena requests")
            .push(request.clone());

        if let Some(pool) = &self.pool {
            let task_id: Uuid = sqlx::query_scalar(
                r#"
                SELECT task_id
                FROM node_executions
                WHERE run_id = ? AND node_id = ? AND iteration = ?
                "#,
            )
            .bind(request.run_id)
            .bind(&request.node_id)
            .bind(request.iteration)
            .fetch_one(pool)
            .await
            .map_err(ApiError::Database)?;
            sqlx::query(
                r#"
                INSERT INTO arena_groups (id, task_id, prompt, base_branch, mode)
                VALUES (?, ?, ?, 'main', 'implementation')
                "#,
            )
            .bind(self.group_id)
            .bind(task_id)
            .bind(&request.prompt)
            .execute(pool)
            .await
            .map_err(ApiError::Database)?;
        }

        Ok(ArenaNodeExecution {
            arena_group_id: self.group_id,
        })
    }
}

#[derive(Debug)]
struct FakeArenaWinnerApplier {
    result: Mutex<Result<String, String>>,
    requests: Mutex<Vec<ArenaWinnerRequest>>,
}

impl FakeArenaWinnerApplier {
    fn succeeds(output_text: impl Into<String>) -> Self {
        Self {
            result: Mutex::new(Ok(output_text.into())),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn fails(message: impl Into<String>) -> Self {
        Self {
            result: Mutex::new(Err(message.into())),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<ArenaWinnerRequest> {
        self.requests.lock().expect("winner requests").clone()
    }
}

#[async_trait::async_trait]
impl WorkflowArenaWinnerApplier for FakeArenaWinnerApplier {
    async fn apply_winner(
        &self,
        request: ArenaWinnerRequest,
    ) -> Result<ArenaWinnerExecution, ApiError> {
        self.requests.lock().expect("winner requests").push(request);

        match self.result.lock().expect("winner result").clone() {
            Ok(output_text) => Ok(ArenaWinnerExecution {
                output_text,
                artifact_paths: Vec::new(),
            }),
            Err(message) => Err(ApiError::BadRequest(message)),
        }
    }
}

#[derive(Debug, Default)]
struct FakeRunCanceller {
    stopped_sessions: Mutex<Vec<Uuid>>,
    cancelled_orchestration_runs: Mutex<Vec<Uuid>>,
}

impl FakeRunCanceller {
    fn stopped_sessions(&self) -> Vec<Uuid> {
        self.stopped_sessions
            .lock()
            .expect("stopped sessions")
            .clone()
    }

    fn cancelled_orchestration_runs(&self) -> Vec<Uuid> {
        self.cancelled_orchestration_runs
            .lock()
            .expect("cancelled orchestration runs")
            .clone()
    }
}

#[async_trait::async_trait]
impl WorkflowRunCanceller for FakeRunCanceller {
    async fn cancel_session(&self, session_id: Uuid) -> Result<(), ApiError> {
        self.stopped_sessions
            .lock()
            .expect("stopped sessions")
            .push(session_id);
        Ok(())
    }

    async fn cancel_orchestration_run(
        &self,
        _pool: &SqlitePool,
        orchestration_run_id: Uuid,
    ) -> Result<(), ApiError> {
        self.cancelled_orchestration_runs
            .lock()
            .expect("cancelled orchestration runs")
            .push(orchestration_run_id);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum FakeCanonicalOutcome {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
struct FakeCanonicalReconciliationBoundary {
    outcome: FakeCanonicalOutcome,
    output_text: Option<String>,
}

impl FakeCanonicalReconciliationBoundary {
    fn completed(output_text: impl Into<String>) -> Self {
        Self {
            outcome: FakeCanonicalOutcome::Completed,
            output_text: Some(output_text.into()),
        }
    }

    fn failed(error_text: impl Into<String>) -> Self {
        Self {
            outcome: FakeCanonicalOutcome::Failed,
            output_text: Some(error_text.into()),
        }
    }

    fn cancelled() -> Self {
        Self {
            outcome: FakeCanonicalOutcome::Cancelled,
            output_text: None,
        }
    }
}

#[async_trait::async_trait]
impl AgentRunReconciliationBoundary for FakeCanonicalReconciliationBoundary {
    async fn reconcile_workflow_run(
        &self,
        pool: &SqlitePool,
        run_id: Uuid,
    ) -> Result<AgentRunReconciliationResult, ApiError> {
        let (status, finished, error_text) = match self.outcome {
            FakeCanonicalOutcome::Completed => ("succeeded", true, None),
            FakeCanonicalOutcome::Failed => ("failed", true, self.output_text.as_deref()),
            FakeCanonicalOutcome::Cancelled => ("cancelled", true, None),
        };
        if matches!(self.outcome, FakeCanonicalOutcome::Completed) {
            // This fake replaces the whole canonical reconciliation boundary,
            // including acceptance after the successful native terminal event.
            let executions: Vec<(Uuid, Uuid)> = sqlx::query_as(
                "SELECT n.id,r.workspace_id FROM node_executions n JOIN workflow_runs r ON r.id=n.run_id WHERE n.run_id=? AND n.status IN ('running','cancelling')",
            )
            .bind(run_id)
            .fetch_all(pool)
            .await?;
            for (execution_id, workspace_id) in executions {
                let (index, summary) = write_fake_delivery(
                    pool,
                    run_id,
                    workspace_id,
                    execution_id,
                    self.output_text
                        .as_deref()
                        .expect("completed fixture has output"),
                )
                .await?;
                sqlx::query(
                    "INSERT INTO workflow_node_handoffs(node_execution_id,run_id,handoff_path,handoff_sha256,files_json) VALUES (?,?,?,?,?)",
                )
                .bind(execution_id)
                .bind(run_id)
                .bind(index.path)
                .bind(index.sha256)
                .bind(serde_json::to_string(&vec![summary]).unwrap())
                .execute(pool)
                .await?;
            }
        }
        sqlx::query(
            r#"
            UPDATE node_executions
            SET status = ?, output_text = COALESCE(?, output_text),
                error_text = ?, finished_at = CASE WHEN ? THEN datetime('now', 'subsec') ELSE finished_at END,
                updated_at = datetime('now', 'subsec')
            WHERE run_id = ? AND status IN ('running', 'cancelling')
            "#,
        )
        .bind(status)
        .bind(self.output_text.as_deref().filter(|_| matches!(self.outcome, FakeCanonicalOutcome::Completed)))
        .bind(error_text)
        .bind(finished)
        .bind(run_id)
        .execute(pool)
        .await
        .map_err(ApiError::Database)?;

        Ok(match self.outcome {
            FakeCanonicalOutcome::Completed => AgentRunReconciliationResult {
                completed: true,
                ..Default::default()
            },
            FakeCanonicalOutcome::Failed => AgentRunReconciliationResult {
                failed: true,
                ..Default::default()
            },
            FakeCanonicalOutcome::Cancelled => AgentRunReconciliationResult {
                cancelled: true,
                ..Default::default()
            },
        })
    }
}

#[test]
fn workflow_route_dtos_export_stable_type_names() {
    let declarations = [
        ("WorkflowTemplateResponse", WorkflowTemplateResponse::decl()),
        (
            "WorkflowTemplateListResponse",
            WorkflowTemplateListResponse::decl(),
        ),
        ("CreateWorkflowRequest", CreateWorkflowRequest::decl()),
        ("UpdateWorkflowRequest", UpdateWorkflowRequest::decl()),
        (
            "WorkflowRevisionConflict",
            server::routes::workflows::WorkflowRevisionConflict::decl(),
        ),
        ("TriggerWorkflowRequest", TriggerWorkflowRequest::decl()),
        (
            "CreateWorkflowAttemptRequest",
            CreateWorkflowAttemptRequest::decl(),
        ),
        (
            "RunWorkflowAttemptRequest",
            RunWorkflowAttemptRequest::decl(),
        ),
        ("SelectArenaWinnerRequest", SelectArenaWinnerRequest::decl()),
        ("WorkflowAttemptResponse", WorkflowAttemptResponse::decl()),
        (
            "WorkflowAttemptListResponse",
            WorkflowAttemptListResponse::decl(),
        ),
        ("WorkflowRunResponse", WorkflowRunResponse::decl()),
        (
            "WorkflowNodeExecutionResponse",
            WorkflowNodeExecutionResponse::decl(),
        ),
        ("WorkflowActionResponse", WorkflowActionResponse::decl()),
    ];

    for (name, declaration) in declarations {
        assert!(
            declaration.contains(name),
            "{name} declaration should include its exported type name"
        );
    }
}

#[test]
fn workflow_router_function_is_public() {
    let _router = server::routes::workflows::router;
}

#[test]
fn workflow_interaction_requests_require_an_execution_identity() {
    use server::routes::workflows::{RespondWorkflowNodeRequest, SelectConditionBranchRequest};

    let execution_id = Uuid::new_v4();
    let candidate_id = Uuid::new_v4();
    let response: RespondWorkflowNodeRequest = serde_json::from_value(json!({
        "node_execution_id": execution_id,
        "message": "Approved"
    }))
    .expect("human response identifies an execution");
    assert_eq!(response.node_execution_id, execution_id);
    assert!(serde_json::from_value::<RespondWorkflowNodeRequest>(json!({})).is_err());

    let branch: SelectConditionBranchRequest = serde_json::from_value(json!({
        "node_execution_id": execution_id,
        "selected_target_node_ids": ["approved"]
    }))
    .expect("branch response identifies an execution");
    assert_eq!(branch.node_execution_id, execution_id);
    assert!(
        serde_json::from_value::<SelectConditionBranchRequest>(json!({
            "selected_target_node_ids": ["approved"]
        }))
        .is_err()
    );

    let winner: SelectArenaWinnerRequest = serde_json::from_value(json!({
        "node_execution_id": execution_id,
        "candidate_id": candidate_id
    }))
    .expect("arena response identifies an execution");
    assert_eq!(winner.node_execution_id, execution_id);
    assert!(
        serde_json::from_value::<SelectArenaWinnerRequest>(json!({
            "candidate_id": candidate_id
        }))
        .is_err()
    );
}

#[tokio::test]
async fn list_project_workflows_seeds_system_templates_and_returns_project_templates() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    let project_workflow_id = insert_project_workflow(&pool, project_id).await;

    let workflows = list_project_workflows(&pool, project_id)
        .await
        .expect("list workflows");

    let built_in_count = workflow::templates::built_in_templates().len();
    assert_eq!(workflows.len(), built_in_count + 1);
    assert!(
        workflows
            .iter()
            .any(|workflow| workflow.id == project_workflow_id
                && workflow.source == WorkflowSource::Project
                && workflow.project_id == Some(project_id))
    );
    assert_eq!(
        workflows
            .iter()
            .filter(|workflow| workflow.source == WorkflowSource::System
                && workflow.project_id.is_none())
            .count(),
        built_in_count
    );
}

#[tokio::test]
async fn list_project_workflows_hides_removed_system_templates_kept_for_history() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let removed_system_workflow_id =
        Uuid::parse_str("8f1f2f0c-0e58-4c7c-8dc1-000000000001").unwrap();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'system', NULL, 'Old system template', NULL, ?)
        "#,
    )
    .bind(removed_system_workflow_id)
    .bind(valid_graph_json())
    .execute(&pool)
    .await
    .expect("insert removed system workflow");

    sqlx::query(
        r#"
        INSERT INTO workflow_runs (id, workflow_id, issue_id, input_text, status)
        VALUES (?, ?, ?, 'Historical run', 'succeeded')
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(removed_system_workflow_id)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .expect("insert historical run");

    let workflows = list_project_workflows(&pool, project_id)
        .await
        .expect("list workflows");

    assert!(
        workflows
            .iter()
            .all(|workflow| workflow.id != removed_system_workflow_id),
        "removed system templates retained for historical runs must stay hidden"
    );

    let retained_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflows WHERE id = ?")
        .bind(removed_system_workflow_id)
        .fetch_one(&pool)
        .await
        .expect("count retained workflow");
    assert_eq!(retained_count, 1);
}

#[tokio::test]
async fn create_workflow_attempt_creates_issue_bound_draft() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Build attempt model").await;

    let attempt = create_issue_workflow_attempt(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Workflow attempt".to_string()),
            graph_json: valid_graph_json(),
            repos: None,
        },
    )
    .await
    .expect("create workflow attempt");

    assert_eq!(attempt.project_id, project_id);
    assert_eq!(attempt.issue_id, issue_id);
    assert_eq!(attempt.status, WorkflowAttemptStatus::Draft);
    assert!(attempt.latest_run_id.is_none());
    assert!(attempt.workspace_id.is_none());

    let canonical_task: (Uuid, Uuid, Uuid, String, String) = sqlx::query_as(
        r#"
        SELECT attempt.task_id, task.project_id, task.issue_id,
               task.title, task.execution_kind
        FROM workflow_attempts attempt
        JOIN tasks task ON task.id = attempt.task_id
        WHERE attempt.id = ?
        "#,
    )
    .bind(attempt.id)
    .fetch_one(&pool)
    .await
    .expect("load canonical workflow Task");
    assert_eq!(canonical_task.0, attempt.id);
    assert_eq!(canonical_task.1, project_id);
    assert_eq!(canonical_task.2, issue_id);
    assert_eq!(canonical_task.3, "Workflow attempt");
    assert_eq!(canonical_task.4, "workflow");
}

#[tokio::test]
async fn create_workflow_attempt_with_resources_binds_ready_workspace() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let repo_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Build workflow attempt").await;

    let workspace_resolver = FakeWorkspaceResolver::new(&pool, workspace_id);
    let attempt = create_issue_workflow_attempt_with_resources(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Workflow attempt".to_string()),
            graph_json: valid_graph_json(),
            repos: Some(vec![DraftWorkspaceRepo {
                repo_id,
                target_branch: "main".to_string(),
            }]),
        },
        &workspace_resolver,
    )
    .await
    .expect("create workflow attempt with workspace");

    assert_eq!(attempt.project_id, project_id);
    assert_eq!(attempt.issue_id, issue_id);
    assert_eq!(attempt.status, WorkflowAttemptStatus::Ready);
    assert_eq!(attempt.workspace_id, Some(workspace_id));

    let requests = workspace_resolver.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].issue_id, issue_id);
    assert_eq!(requests[0].project_id, Some(project_id));
    assert!(requests[0].existing_workspace_id.is_none());
    assert_eq!(requests[0].repo_overrides.len(), 1);
    assert_eq!(requests[0].repo_overrides[0].repo_id, repo_id);
    assert_eq!(requests[0].repo_overrides[0].target_branch, "main");
}

#[tokio::test]
async fn create_workflow_attempt_passes_direct_folder_to_workspace_resolver() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Folder workflow").await;
    let resolver = FakeWorkspaceResolver::new(&pool, workspace_id);
    let attempt = create_issue_workflow_attempt_with_resources(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: Some("F:/notes".to_string()),
            name: Some("Folder workflow".to_string()),
            graph_json: valid_graph_json(),
            repos: None,
        },
        &resolver,
    )
    .await
    .unwrap();
    assert_eq!(attempt.workspace_id, Some(workspace_id));
    let requests = resolver.requests();
    assert_eq!(requests[0].directory_path.as_deref(), Some("F:/notes"));
    assert!(requests[0].repo_overrides.is_empty());
}

#[tokio::test]
async fn run_unbound_workflow_attempt_passes_direct_folder_to_workspace_resolver() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Folder workflow").await;
    let attempt = create_issue_workflow_attempt(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Folder workflow".to_string()),
            graph_json: valid_graph_json(),
            repos: None,
        },
    )
    .await
    .unwrap();
    let resolver = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, Uuid::new_v4(), "unused");
    let run = run_workflow_attempt_runtime(
        &pool,
        attempt.id,
        RunWorkflowAttemptRequest {
            directory_path: Some("F:/notes".to_string()),
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Run".to_string(),
            repos: None,
        },
        &resolver,
        &agent,
    )
    .await
    .unwrap();
    assert_eq!(run.attempt_id, Some(attempt.id));
    let requests = resolver.requests();
    assert_eq!(requests[0].directory_path.as_deref(), Some("F:/notes"));
    assert!(requests[0].repo_overrides.is_empty());
}

#[tokio::test]
async fn workflow_attempt_resource_failure_compensates_all_created_resources() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Reject partial resources").await;
    list_project_workflows(&pool, project_id)
        .await
        .expect("seed canonical system workflows before measuring baselines");

    let tables = [
        "workflows",
        "workflow_attempts",
        "tasks",
        "workspaces",
        "sessions",
        "agent_task_bindings",
    ];
    let mut baselines = Vec::with_capacity(tables.len());
    for table in tables {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("count baseline {table}: {error}"));
        baselines.push(count);
    }

    sqlx::query(
        r#"
        CREATE TRIGGER reject_workflow_session_resource
        BEFORE INSERT ON sessions
        BEGIN
            SELECT RAISE(ABORT, 'reject Workflow Session resource');
        END
        "#,
    )
    .execute(&pool)
    .await
    .expect("install Session failure trigger");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let result = create_issue_workflow_attempt_with_resources(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Compensated attempt".to_string()),
            graph_json: agent_graph_json(),
            repos: None,
        },
        &workspace,
    )
    .await;

    assert!(result.is_err(), "resource initialization must fail");
    for (table, baseline) in tables.into_iter().zip(baselines) {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("count {table}: {error}"));
        assert_eq!(
            count, baseline,
            "{table} must return to its baseline cardinality"
        );
    }
}

#[tokio::test]
async fn list_project_workflows_excludes_attempt_owned_backing_workflows() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Draft hidden attempt").await;

    let attempt = create_issue_workflow_attempt(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Hidden attempt graph".to_string()),
            graph_json: valid_graph_json(),
            repos: None,
        },
    )
    .await
    .expect("create workflow attempt");

    let workflows = list_project_workflows(&pool, project_id)
        .await
        .expect("list workflows");

    assert!(
        workflows
            .iter()
            .all(|workflow| workflow.id != attempt.workflow_id),
        "attempt-owned backing graph must not appear as reusable template"
    );
}

#[tokio::test]
async fn workflow_attempt_can_be_resolved_from_backing_workflow() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Open attempt canvas").await;

    let attempt = create_issue_workflow_attempt(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Canvas-owned attempt".to_string()),
            graph_json: valid_graph_json(),
            repos: None,
        },
    )
    .await
    .expect("create workflow attempt");

    let resolved = workflow_attempt_by_workflow_id(&pool, attempt.workflow_id)
        .await
        .expect("query attempt by workflow")
        .expect("attempt exists for backing workflow");

    assert_eq!(resolved.id, attempt.id);
    assert_eq!(resolved.issue_id, issue_id);
    assert_eq!(resolved.workflow_id, attempt.workflow_id);
}

#[tokio::test]
async fn delete_workflow_attempt_removes_backing_graph_runs_and_nodes() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Delete attempt").await;

    let attempt = create_issue_workflow_attempt(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Delete me".to_string()),
            graph_json: valid_graph_json(),
            repos: None,
        },
    )
    .await
    .expect("create workflow attempt");
    let run_id = Uuid::new_v4();
    let node_execution_id = Uuid::new_v4();

    sqlx::query(
        r#"
        INSERT INTO workflow_runs
            (id, workflow_id, attempt_id, issue_id, trigger_source, input_text, status)
        VALUES (?, ?, ?, ?, 'manual', 'run it', 'failed')
        "#,
    )
    .bind(run_id)
    .bind(attempt.workflow_id)
    .bind(attempt.id)
    .bind(issue_id)
    .execute(&pool)
    .await
    .expect("insert workflow run");
    sqlx::query("UPDATE workflow_attempts SET latest_run_id = ?, status = 'failed' WHERE id = ?")
        .bind(run_id)
        .bind(attempt.id)
        .execute(&pool)
        .await
        .expect("link latest run");
    sqlx::query(
        r#"
        INSERT INTO node_executions
            (id, run_id, node_id, node_type, iteration, status)
        VALUES (?, ?, 'agent', 'agent', 0, 'failed')
        "#,
    )
    .bind(node_execution_id)
    .bind(run_id)
    .execute(&pool)
    .await
    .expect("insert node execution");

    delete_issue_workflow_attempt(&pool, attempt.id)
        .await
        .expect("delete workflow attempt");

    let attempt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_attempts WHERE id = ?")
            .bind(attempt.id)
            .fetch_one(&pool)
            .await
            .expect("count attempts");
    let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE id = ?")
        .bind(attempt.id)
        .fetch_one(&pool)
        .await
        .expect("count canonical Tasks");
    let workflow_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflows WHERE id = ?")
        .bind(attempt.workflow_id)
        .fetch_one(&pool)
        .await
        .expect("count workflows");
    let run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_runs WHERE id = ?")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("count runs");
    let node_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM node_executions WHERE id = ?")
        .bind(node_execution_id)
        .fetch_one(&pool)
        .await
        .expect("count node executions");

    assert_eq!(attempt_count, 0);
    assert_eq!(task_count, 0);
    assert_eq!(workflow_count, 0);
    assert_eq!(run_count, 0);
    assert_eq!(node_count, 0);
}

#[tokio::test]
async fn workflow_attempt_create_rejects_issue_from_another_project() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let other_project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_project(&pool, other_project_id).await;
    insert_local_issue(&pool, other_project_id, issue_id, "Wrong project").await;

    let err = create_issue_workflow_attempt(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Invalid".to_string()),
            graph_json: valid_graph_json(),
            repos: None,
        },
    )
    .await
    .expect_err("issue must belong to project");

    assert!(err.to_string().contains("Task not found for project"));
}

#[tokio::test]
async fn create_project_workflow_rejects_invalid_graph() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    let result = create_project_workflow(
        &pool,
        project_id,
        CreateWorkflowRequest {
            name: "Invalid Flow".to_string(),
            description: None,
            graph_json: graph_without_start_json(),
        },
    )
    .await;

    assert!(
        matches!(result, Err(ApiError::BadRequest(message)) if message.contains("start")),
        "invalid graph should be rejected with a start-node validation error"
    );

    let project_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflows WHERE source = 'project'")
            .fetch_one(&pool)
            .await
            .expect("count project workflows");
    assert_eq!(project_count, 0);
}

#[tokio::test]
async fn update_system_template_returns_forbidden() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    let workflows = list_project_workflows(&pool, project_id)
        .await
        .expect("seed system workflows");
    let system_workflow_id = workflows
        .iter()
        .find(|workflow| workflow.source == WorkflowSource::System)
        .expect("system workflow")
        .id;

    let result = update_workflow_template(
        &pool,
        system_workflow_id,
        UpdateWorkflowRequest {
            expected_revision: 1,
            name: Some("Changed".to_string()),
            description: None,
            graph_json: None,
            main_agent_config: None,
            main_agent_prompt: None,
        },
    )
    .await;

    assert!(
        matches!(result, Err(WorkflowUpdateError::Api(ApiError::Forbidden(message))) if message.contains("system")),
        "system workflows must not be editable"
    );
}

#[tokio::test]
async fn update_project_workflow_accepts_parseable_draft_graph() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    let workflow_id = insert_project_workflow(&pool, project_id).await;

    let result = update_workflow_template(
        &pool,
        workflow_id,
        UpdateWorkflowRequest {
            expected_revision: 1,
            name: None,
            description: None,
            graph_json: Some(unreachable_draft_graph_json()),
            main_agent_config: None,
            main_agent_prompt: None,
        },
    )
    .await
    .expect("draft graph update should be persisted before run validation");

    assert!(
        result.graph_json.contains("agent-draft"),
        "draft node configuration should survive workflow update"
    );
    assert_eq!(result.revision, 2);
}

#[tokio::test]
async fn concurrent_workflow_updates_from_one_revision_allow_one_writer() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    let workflow_id = insert_project_workflow(&pool, project_id).await;

    let first = update_workflow_template(
        &pool,
        workflow_id,
        UpdateWorkflowRequest {
            expected_revision: 1,
            name: Some("First writer".to_string()),
            description: None,
            graph_json: None,
            main_agent_config: None,
            main_agent_prompt: None,
        },
    )
    .await
    .expect("first writer should advance the revision");
    assert_eq!(first.revision, 2);

    let second = update_workflow_template(
        &pool,
        workflow_id,
        UpdateWorkflowRequest {
            expected_revision: 1,
            name: Some("Second writer".to_string()),
            description: None,
            graph_json: None,
            main_agent_config: None,
            main_agent_prompt: None,
        },
    )
    .await
    .expect_err("stale writer must not overwrite the first writer");

    match second {
        WorkflowUpdateError::RevisionConflict(conflict) => {
            assert_eq!(conflict.workflow_id, workflow_id);
            assert_eq!(conflict.expected_revision, 1);
            assert_eq!(conflict.current_revision, 2);
        }
        other => panic!("expected typed revision conflict, got {other:?}"),
    }

    let persisted = get_workflow_template(&pool, workflow_id)
        .await
        .expect("read persisted workflow");
    assert_eq!(persisted.name, "First writer");
    assert_eq!(persisted.revision, 2);
}

#[tokio::test]
async fn revision_conflict_does_not_create_agent_sessions() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(
        &pool,
        project_id,
        issue_id,
        "Keep losing save side-effect free",
    )
    .await;

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let attempt = create_issue_workflow_attempt_with_resources(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Concurrent Agent workflow".to_string()),
            graph_json: agent_graph_json(),
            repos: None,
        },
        &workspace,
    )
    .await
    .expect("create ready workflow attempt");
    let before = get_workflow_template(&pool, attempt.workflow_id)
        .await
        .expect("load workflow before concurrent saves");
    let baseline_sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .expect("count baseline Sessions");

    update_workflow_template(
        &pool,
        attempt.workflow_id,
        UpdateWorkflowRequest {
            expected_revision: before.revision,
            name: Some("Winning writer".to_string()),
            description: None,
            graph_json: None,
            main_agent_config: None,
            main_agent_prompt: None,
        },
    )
    .await
    .expect("winning writer advances revision");

    let losing_result = update_workflow_template(
        &pool,
        attempt.workflow_id,
        UpdateWorkflowRequest {
            expected_revision: before.revision,
            name: Some("Losing writer".to_string()),
            description: None,
            graph_json: Some(agent_graph_json()),
            main_agent_config: None,
            main_agent_prompt: None,
        },
    )
    .await;
    assert!(matches!(
        losing_result,
        Err(WorkflowUpdateError::RevisionConflict(_))
    ));

    let session_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .expect("count Sessions after revision conflict");
    assert_eq!(session_count, baseline_sessions);
}

#[tokio::test]
async fn delete_system_template_returns_forbidden() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    let workflows = list_project_workflows(&pool, project_id)
        .await
        .expect("seed system workflows");
    let system_workflow_id = workflows
        .iter()
        .find(|workflow| workflow.source == WorkflowSource::System)
        .expect("system workflow")
        .id;

    let result = delete_workflow_template(&pool, system_workflow_id).await;

    assert!(
        matches!(result, Err(ApiError::Forbidden(message)) if message.contains("system")),
        "system workflows must not be deletable"
    );
}

#[tokio::test]
async fn fallback_payloads_return_workflow_table_keys() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let node_execution_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Runnable Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(valid_graph_json())
    .execute(&pool)
    .await
    .expect("insert workflow");

    sqlx::query(
        r#"
        INSERT INTO workflow_runs
            (id, workflow_id, issue_id, workspace_id, trigger_source, input_text, status)
        VALUES (?, ?, ?, NULL, 'manual', 'Run it', 'pending')
        "#,
    )
    .bind(run_id)
    .bind(workflow_id)
    .bind(issue_id)
    .execute(&pool)
    .await
    .expect("insert workflow run");

    sqlx::query(
        r#"
        INSERT INTO node_executions
            (id, run_id, node_id, node_type, iteration, status, input_text)
        VALUES (?, ?, 'start', 'start', 0, 'pending', 'Run it')
        "#,
    )
    .bind(node_execution_id)
    .bind(run_id)
    .execute(&pool)
    .await
    .expect("insert node execution");

    let workflows_payload = fallback_workflows_payload(&pool, Some(project_id))
        .await
        .expect("workflow fallback payload");
    let workflow_runs_payload = fallback_workflow_runs_payload(&pool, Some(issue_id), None)
        .await
        .expect("workflow run fallback payload");
    let node_executions_payload = fallback_node_executions_payload(&pool, Some(run_id))
        .await
        .expect("node execution fallback payload");

    assert_payload_array_key(&workflows_payload, "workflows");
    assert_payload_array_key(&workflow_runs_payload, "workflow_runs");
    assert_payload_array_key(&node_executions_payload, "node_executions");
}

#[tokio::test]
async fn workflow_runner_trigger_creates_run_workspace_and_node_executions() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Agent Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(agent_graph_json())
    .execute(&pool)
    .await
    .expect("insert agent workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "agent final output");

    let run = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Build workflow runner".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger workflow run");

    assert_eq!(run.workflow_id, workflow_id);
    assert_eq!(run.issue_id, issue_id);
    assert_eq!(run.workspace_id, Some(workspace_id));
    assert_eq!(run.nodes.len(), 3);
    assert_eq!(
        run.nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<Vec<_>>(),
        vec!["start", "agent", "end"]
    );

    let workspace_requests = workspace.requests();
    assert_eq!(workspace_requests.len(), 1);
    assert_eq!(workspace_requests[0].existing_workspace_id, None);
    assert_eq!(workspace_requests[0].issue_id, issue_id);
    assert_eq!(workspace_requests[0].run_id, run.id);
    assert_eq!(workspace_requests[0].project_id, Some(project_id));
    assert_eq!(
        workspace_requests[0].branch_name,
        format!("vk/{issue_id}-wf-{}", short_run_id(run.id))
    );

    let run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_runs")
        .fetch_one(&pool)
        .await
        .expect("count workflow runs");
    let node_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM node_executions")
        .fetch_one(&pool)
        .await
        .expect("count node executions");
    assert_eq!(run_count, 1);
    assert_eq!(node_count, 3);
}

#[tokio::test]
async fn running_workflow_attempt_updates_latest_run_workspace_and_status() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let repo_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Run workflow attempt").await;

    let attempt = create_issue_workflow_attempt(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Attempt run".to_string()),
            graph_json: valid_graph_json(),
            repos: None,
        },
    )
    .await
    .expect("create workflow attempt");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, Uuid::new_v4(), "unused");
    let run = run_workflow_attempt_runtime(
        &pool,
        attempt.id,
        RunWorkflowAttemptRequest {
            directory_path: None,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Implement by workflow".to_string(),
            repos: Some(vec![DraftWorkspaceRepo {
                repo_id,
                target_branch: "develop".to_string(),
            }]),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("run workflow attempt");

    assert_eq!(run.attempt_id, Some(attempt.id));
    let refreshed = workflow_attempt_by_id(&pool, attempt.id)
        .await
        .expect("query attempt")
        .expect("attempt exists");
    assert_eq!(refreshed.latest_run_id, Some(run.id));
    assert_eq!(refreshed.workspace_id, Some(workspace_id));
    assert_eq!(refreshed.status, WorkflowAttemptStatus::Succeeded);

    let workspace_requests = workspace.requests();
    assert_eq!(workspace_requests.len(), 1);
    assert_eq!(workspace_requests[0].repo_overrides.len(), 1);
    assert_eq!(workspace_requests[0].repo_overrides[0].repo_id, repo_id);
    assert_eq!(
        workspace_requests[0].repo_overrides[0].target_branch,
        "develop"
    );
}

#[tokio::test]
async fn accepted_workflow_runs_keep_snapshot_fifo_and_stable_node_sessions() {
    use db::models::workflow_queue::WorkflowQueueEntry;
    use server::{
        routes::workflows::accept_workflow_attempt,
        workflow_runtime::{
            management::resolve_waiting_submissions,
            runner::{get_workflow_run_response, start_accepted_workflow_run},
        },
    };

    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Queued workflow").await;
    let attempt = create_issue_workflow_attempt(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Queued work".into()),
            graph_json: agent_graph_json(),
            repos: None,
        },
    )
    .await
    .unwrap();
    let workspace = FakeWorkspaceResolver::new(&pool, Uuid::new_v4());
    let request = |input: &str| RunWorkflowAttemptRequest {
        directory_path: None,
        workspace_id: None,
        trigger_source: "manual".into(),
        input_text: input.into(),
        repos: None,
    };
    let first_id = Uuid::new_v4();
    let first = accept_workflow_attempt(
        &pool,
        first_id,
        attempt.id,
        request("Original input"),
        &workspace,
    )
    .await
    .unwrap();
    assert_eq!(first.status, WorkflowRunStatus::Pending);
    assert_eq!(first.queue_phase.as_deref(), Some("queued"));
    assert!(first.started_at.is_none());
    assert!(first.orchestration_run_id.is_none());
    let second_id = Uuid::new_v4();
    let second = accept_workflow_attempt(
        &pool,
        second_id,
        attempt.id,
        request("Follow-up input"),
        &workspace,
    )
    .await
    .unwrap();
    assert_eq!(second.status, WorkflowRunStatus::Pending);
    assert_eq!(second.queue_phase.as_deref(), Some("waiting_for_source"));
    assert!(second.nodes.is_empty());
    assert!(second.started_at.is_none());
    assert!(second.orchestration_run_id.is_none());
    let snapshot: String =
        sqlx::query_scalar("SELECT graph_snapshot FROM workflow_runs WHERE id=?")
            .bind(first_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let first_sequence: i64 =
        sqlx::query_scalar("SELECT sequence FROM workflow_run_queue WHERE run_id=?")
            .bind(first_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let second_sequence: i64 =
        sqlx::query_scalar("SELECT sequence FROM workflow_run_queue WHERE run_id=?")
            .bind(second_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(first_sequence < second_sequence);
    let first_node = first
        .nodes
        .iter()
        .find(|node| node.node_id == "agent")
        .unwrap();
    assert!(first_node.session_id.is_some());

    // Editing/turning off the source after acceptance must not rewrite queued work.
    sqlx::query("UPDATE workflows SET graph_json=?, external_enabled=0 WHERE id=?")
        .bind(valid_graph_json())
        .bind(attempt.workflow_id)
        .execute(&pool)
        .await
        .unwrap();
    let retry = accept_workflow_attempt(
        &pool,
        first_id,
        attempt.id,
        request("Original input"),
        &workspace,
    )
    .await
    .unwrap();
    assert_eq!(retry.id, first_id);
    assert_eq!(retry.nodes.len(), first.nodes.len());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_run_queue")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );

    assert_eq!(
        WorkflowQueueEntry::claim_next(&pool)
            .await
            .unwrap()
            .unwrap()
            .run_id,
        first_id
    );
    assert!(
        WorkflowQueueEntry::claim_next(&pool)
            .await
            .unwrap()
            .is_none()
    );
    start_accepted_workflow_run(
        &pool,
        first_id,
        &BoundSessionAgentExecutor::new(&pool),
        &NoopWorkflowArenaCreator,
    )
    .await
    .unwrap();
    let finished = get_workflow_run_response(&pool, first_id).await.unwrap();
    assert_eq!(finished.status, WorkflowRunStatus::Succeeded);
    assert!(
        finished
            .nodes
            .iter()
            .any(|node| node.node_id == "agent" && node.status == NodeExecutionStatus::Succeeded)
    );
    assert!(
        WorkflowQueueEntry::release_terminal(&pool, first_id)
            .await
            .unwrap()
    );
    // The accepted dependent has no actual work until the source outcome and
    // release boundary settle. Readiness fixes its plan without changing FIFO.
    assert!(
        WorkflowQueueEntry::claim_next(&pool)
            .await
            .unwrap()
            .is_none()
    );
    resolve_waiting_submissions(&pool).await.unwrap();
    let second = get_workflow_run_response(&pool, second_id).await.unwrap();
    assert_eq!(second.queue_phase.as_deref(), Some("queued"));
    assert_eq!(finished.input_text, "Original input");
    assert_eq!(second.input_text, "Follow-up input");
    assert!(second.started_at.is_none());
    assert!(second.orchestration_run_id.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT sequence FROM workflow_run_queue WHERE run_id=?")
            .bind(second_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        second_sequence
    );
    for run_id in [first_id, second_id] {
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT graph_snapshot FROM workflow_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            snapshot
        );
    }
    let second_node = second
        .nodes
        .iter()
        .find(|node| node.node_id == "agent")
        .expect("ready full rework materializes an actual Agent Node");
    assert_eq!(first_node.session_id, second_node.session_id);
    assert_eq!(first_node.task_id, second_node.task_id);
    assert_ne!(first_node.id, second_node.id);
    assert_eq!(
        WorkflowQueueEntry::claim_next(&pool)
            .await
            .unwrap()
            .unwrap()
            .run_id,
        second_id
    );
    start_accepted_workflow_run(
        &pool,
        second_id,
        &BoundSessionAgentExecutor::new(&pool),
        &NoopWorkflowArenaCreator,
    )
    .await
    .unwrap();
    assert_eq!(
        get_workflow_run_response(&pool, second_id)
            .await
            .unwrap()
            .status,
        WorkflowRunStatus::Succeeded
    );
    let children: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE parent_task_id=?")
        .bind(attempt.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(children, 1);
}

#[tokio::test]
async fn external_acceptance_rechecks_grants_and_template_before_committing_queue() {
    use server::routes::workflows::accept_workflow_attempt;
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let caller = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "External acceptance").await;
    let template_id = insert_project_workflow(&pool, project_id).await;
    sqlx::query("UPDATE workflows SET external_enabled=1 WHERE id=?")
        .bind(template_id)
        .execute(&pool)
        .await
        .unwrap();
    let attempt = create_issue_workflow_attempt(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("External work".into()),
            graph_json: valid_graph_json(),
            repos: None,
        },
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO workflow_attempt_sources(attempt_id,template_id,template_revision) VALUES (?,?,1)").bind(attempt.id).bind(template_id).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO external_integrations(id,name,key_digest) VALUES (?,'Caller','test-digest')",
    )
    .bind(caller)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO external_integration_projects(integration_id,project_id) VALUES (?,?)",
    )
    .bind(caller)
    .bind(project_id)
    .execute(&pool)
    .await
    .unwrap();
    let workspace = FakeWorkspaceResolver::new(&pool, Uuid::new_v4());
    let request = || RunWorkflowAttemptRequest {
        directory_path: None,
        workspace_id: None,
        trigger_source: "external".into(),
        input_text: "Process documents".into(),
        repos: None,
    };
    let accepted_id = Uuid::new_v4();
    let rejected_id = Uuid::new_v4();
    for id in [accepted_id, rejected_id] {
        sqlx::query("INSERT INTO external_integration_requests(integration_id,operation,scope,request_key,request_hash,resource_id) VALUES (?,'workflow_run',?,?, 'hash',?)")
            .bind(caller).bind(attempt.id.to_string()).bind(id.to_string()).bind(id).execute(&pool).await.unwrap();
    }
    let accepted = accept_workflow_attempt(&pool, accepted_id, attempt.id, request(), &workspace)
        .await
        .unwrap();
    assert_eq!(accepted.status, WorkflowRunStatus::Pending);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM external_integration_requests WHERE resource_id=?"
        )
        .bind(accepted_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "complete"
    );
    sqlx::query("UPDATE workflows SET external_enabled=0 WHERE id=?")
        .bind(template_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        accept_workflow_attempt(&pool, rejected_id, attempt.id, request(), &workspace).await,
        Err(ApiError::Conflict(_))
    ));
    assert_eq!(
        accept_workflow_attempt(&pool, accepted_id, attempt.id, request(), &workspace)
            .await
            .unwrap()
            .id,
        accepted_id
    );
    sqlx::query("UPDATE workflows SET external_enabled=1 WHERE id=?")
        .bind(template_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM external_integration_projects WHERE integration_id=?")
        .bind(caller)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        accept_workflow_attempt(&pool, rejected_id, attempt.id, request(), &workspace).await,
        Err(ApiError::Forbidden(_))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_runs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_run_queue")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM external_integration_requests WHERE resource_id=?"
        )
        .bind(rejected_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "preparing"
    );
}

#[tokio::test]
async fn workflow_agent_node_materializes_one_child_task_and_agent_binding() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Materialize Agent Task").await;

    let attempt = create_issue_workflow_attempt(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Agent Task workflow".to_string()),
            graph_json: agent_graph_json(),
            repos: None,
        },
    )
    .await
    .expect("create Agent workflow attempt");
    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let run = run_workflow_attempt_runtime(
        &pool,
        attempt.id,
        RunWorkflowAttemptRequest {
            directory_path: None,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Run the Agent Task".to_string(),
            repos: None,
        },
        &workspace,
        &BoundSessionAgentExecutor::new(&pool),
    )
    .await
    .expect("run Agent workflow attempt");

    let child: (Uuid, String, Uuid) = sqlx::query_as(
        r#"
        SELECT task.id, task.execution_kind, binding.session_id
        FROM tasks task
        JOIN agent_task_bindings binding ON binding.task_id = task.id
        WHERE task.parent_task_id = ?
        "#,
    )
    .bind(attempt.id)
    .fetch_one(&pool)
    .await
    .expect("load Agent child Task and binding");
    let agent_node = run
        .nodes
        .iter()
        .find(|node| node.node_id == "agent")
        .expect("Agent node execution");
    assert_eq!(child.1, "agent");
    assert_eq!(agent_node.task_id, Some(child.0));
    assert_eq!(agent_node.session_id, Some(child.2));

    let child_task_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE parent_task_id = ?")
            .bind(attempt.id)
            .fetch_one(&pool)
            .await
            .expect("count Agent child Tasks");
    let binding_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM agent_task_bindings binding
        JOIN tasks task ON task.id = binding.task_id
        WHERE task.parent_task_id = ?
        "#,
    )
    .bind(attempt.id)
    .fetch_one(&pool)
    .await
    .expect("count Agent Task bindings");
    assert_eq!(child_task_count, 1);
    assert_eq!(binding_count, 1);
}

#[tokio::test]
async fn workflow_structural_nodes_never_materialize_tasks() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(
        &pool,
        project_id,
        issue_id,
        "Keep structural nodes structural",
    )
    .await;

    let attempt = create_issue_workflow_attempt(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Structural workflow".to_string()),
            graph_json: structural_graph_json(),
            repos: None,
        },
    )
    .await
    .expect("create structural workflow attempt");
    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let run = run_workflow_attempt_runtime(
        &pool,
        attempt.id,
        RunWorkflowAttemptRequest {
            directory_path: None,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Pause before routing".to_string(),
            repos: None,
        },
        &workspace,
        &FakeAgentExecutor::new(&pool, Uuid::new_v4(), "unused"),
    )
    .await
    .expect("initialize structural workflow");

    assert_eq!(run.status, WorkflowRunStatus::AwaitingHuman);
    for node_type in ["start", "end", "condition", "human_gate", "transform"] {
        let matching = run
            .nodes
            .iter()
            .filter(|node| node.node_type == node_type)
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 1, "expected one {node_type} node");
        assert!(
            matching[0].task_id.is_none(),
            "{node_type} must not own a canonical Task"
        );
    }
    let child_task_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE parent_task_id = ?")
            .bind(attempt.id)
            .fetch_one(&pool)
            .await
            .expect("count structural child Tasks");
    assert_eq!(child_task_count, 0);
}

#[tokio::test]
async fn workflow_arena_node_and_group_share_one_child_task() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let arena_group_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Materialize Arena Task").await;

    let attempt = create_issue_workflow_attempt(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Arena Task workflow".to_string()),
            graph_json: arena_graph_json(),
            repos: None,
        },
    )
    .await
    .expect("create Arena workflow attempt");
    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let arena = FakeArenaCreator::persisting(&pool, arena_group_id);
    let run = run_workflow_attempt_runtime_with_arena(
        &pool,
        attempt.id,
        RunWorkflowAttemptRequest {
            directory_path: None,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Run one Arena Task".to_string(),
            repos: None,
        },
        &workspace,
        &FakeAgentExecutor::new(&pool, Uuid::new_v4(), "unused"),
        &arena,
    )
    .await
    .expect("run Arena workflow attempt");

    let arena_node = run
        .nodes
        .iter()
        .find(|node| node.node_id == "arena")
        .expect("Arena node execution");
    let task_id = arena_node.task_id.expect("Arena node canonical Task");
    let group_task_id: Uuid = sqlx::query_scalar("SELECT task_id FROM arena_groups WHERE id = ?")
        .bind(arena_group_id)
        .fetch_one(&pool)
        .await
        .expect("load ArenaGroup Task binding");
    assert_eq!(group_task_id, task_id);

    let child_tasks: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, execution_kind FROM tasks WHERE parent_task_id = ? ORDER BY id")
            .bind(attempt.id)
            .fetch_all(&pool)
            .await
            .expect("load Arena child Tasks");
    assert_eq!(child_tasks, vec![(task_id, "arena".to_string())]);
}

#[tokio::test]
async fn workflow_initialization_failure_rolls_back_runtime_materialization() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(
        &pool,
        project_id,
        issue_id,
        "Rollback runtime materialization",
    )
    .await;

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let attempt = create_issue_workflow_attempt_with_resources(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Atomic initialization".to_string()),
            graph_json: agent_graph_json(),
            repos: None,
        },
        &workspace,
    )
    .await
    .expect("create ready workflow attempt");

    let baseline_tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(&pool)
        .await
        .expect("count baseline Tasks");
    let baseline_sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .expect("count baseline Sessions");
    sqlx::query(
        r#"
        CREATE TRIGGER reject_agent_node_materialization
        BEFORE INSERT ON node_executions
        WHEN NEW.node_id = 'agent'
        BEGIN
            SELECT RAISE(ABORT, 'reject Agent node materialization');
        END
        "#,
    )
    .execute(&pool)
    .await
    .expect("install node materialization failure trigger");

    let result = run_workflow_attempt_runtime(
        &pool,
        attempt.id,
        RunWorkflowAttemptRequest {
            directory_path: None,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Trigger atomic rollback".to_string(),
            repos: None,
        },
        &workspace,
        &BoundSessionAgentExecutor::new(&pool),
    )
    .await;
    assert!(result.is_err(), "node materialization must fail");

    let attempt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_attempts WHERE id = ?")
            .bind(attempt.id)
            .fetch_one(&pool)
            .await
            .expect("count retained attempt");
    let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(&pool)
        .await
        .expect("count Tasks after rollback");
    let session_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .expect("count Sessions after rollback");
    let run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_runs")
        .fetch_one(&pool)
        .await
        .expect("count Workflow runs after rollback");
    let node_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM node_executions")
        .fetch_one(&pool)
        .await
        .expect("count NodeExecutions after rollback");
    let binding_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_task_bindings")
        .fetch_one(&pool)
        .await
        .expect("count Agent Task bindings after rollback");
    assert_eq!(attempt_count, 1);
    assert_eq!(task_count, baseline_tasks);
    assert_eq!(session_count, baseline_sessions);
    assert_eq!(run_count, 0);
    assert_eq!(node_count, 0);
    assert_eq!(binding_count, 0);
}

#[tokio::test]
async fn canceling_workflow_attempt_syncs_attempt_status() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Cancel workflow attempt").await;

    let attempt = create_issue_workflow_attempt(
        &pool,
        project_id,
        issue_id,
        CreateWorkflowAttemptRequest {
            directory_path: None,
            name: Some("Cancel attempt".to_string()),
            graph_json: agent_graph_json(),
            repos: None,
        },
    )
    .await
    .expect("create workflow attempt");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = StartedAgentExecutor::new(session_id);
    let running = run_workflow_attempt_runtime(
        &pool,
        attempt.id,
        RunWorkflowAttemptRequest {
            directory_path: None,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Long task".to_string(),
            repos: None,
        },
        &workspace,
        &agent,
    )
    .await
    .expect("run workflow attempt");

    let canceller = FakeRunCanceller::default();
    let canceled = cancel_workflow_run_runtime(&pool, running.id, &canceller)
        .await
        .expect("cancel workflow run");
    sync_attempt_from_run(&pool, &canceled)
        .await
        .expect("sync attempt");

    let refreshed = workflow_attempt_by_id(&pool, attempt.id)
        .await
        .expect("query attempt")
        .expect("attempt exists");
    assert_eq!(canceled.status, WorkflowRunStatus::Cancelling);
    assert_eq!(
        node_status(&canceled.nodes, "agent"),
        NodeExecutionStatus::Cancelling
    );
    assert_eq!(refreshed.status, WorkflowAttemptStatus::Cancelling);
    assert_eq!(refreshed.latest_run_id, Some(running.id));
}

#[tokio::test]
async fn workflow_runner_resolves_project_id_from_local_issue_for_system_workflow() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "System workflow issue").await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'system', NULL, 'System Agent Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(agent_graph_json())
    .execute(&pool)
    .await
    .expect("insert system agent workflow");

    // Reference the workflow from a historical run so ensure_system_workflows
    // does not prune this test-only system workflow before triggering.
    sqlx::query(
        r#"
        INSERT INTO workflow_runs (id, workflow_id, issue_id, input_text, status)
        VALUES (?, ?, ?, 'Historical run', 'succeeded')
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(workflow_id)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .expect("insert historical run reference");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "agent final output");

    let run = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Run system workflow".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger system workflow run");

    assert_eq!(run.workspace_id, Some(workspace_id));
    let workspace_requests = workspace.requests();
    assert_eq!(workspace_requests.len(), 1);
    assert_eq!(workspace_requests[0].project_id, Some(project_id));
}

#[tokio::test]
async fn workflow_runner_trigger_binds_existing_workspace() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let existing_workspace_id = Uuid::new_v4();
    let fallback_workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Agent Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(agent_graph_json())
    .execute(&pool)
    .await
    .expect("insert agent workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, fallback_workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "agent final output");

    let run = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: Some(existing_workspace_id),
            trigger_source: "manual".to_string(),
            input_text: "Use current worktree".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger workflow run");

    assert_eq!(run.workspace_id, Some(existing_workspace_id));
    assert_eq!(workspace.requests().len(), 1);
    assert_eq!(workspace.requests()[0].project_id, Some(project_id));
    assert_eq!(
        workspace.requests()[0].existing_workspace_id,
        Some(existing_workspace_id)
    );
}

#[tokio::test]
async fn workflow_runner_agent_node_uses_main_workspace_and_stores_session_output() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    let execution_process_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Agent Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(agent_graph_json())
    .execute(&pool)
    .await
    .expect("insert agent workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::with_execution_process(
        &pool,
        session_id,
        execution_process_id,
        "implemented feature",
    );

    let run = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Add workflow task orchestration".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger workflow run");

    let agent_requests = agent.requests();
    assert_eq!(agent_requests.len(), 1);
    assert_eq!(agent_requests[0].run_id, run.id);
    assert_eq!(agent_requests[0].node_id, "agent");
    assert!(agent_requests[0].session_id.is_some());
    assert_eq!(agent_requests[0].workspace_id, workspace_id);
    assert!(
        agent_requests[0]
            .prompt
            .contains("# Workflow Agent Envelope")
    );
    assert!(
        agent_requests[0]
            .prompt
            .contains("Implement this: Source execution:")
    );
    let start = run
        .nodes
        .iter()
        .find(|node| node.node_id == "start")
        .unwrap();
    assert!(agent_requests[0].prompt.contains(&format!(
        ".vibe-kanban/workflows/{}/handoffs/{}.json",
        run.id, start.id
    )));
    assert!(
        agent_requests[0]
            .prompt
            .contains(&format!(".vibe-kanban/workflows/{}/request.md", run.id))
    );
    assert!(
        !agent_requests[0]
            .prompt
            .contains("Add workflow task orchestration")
    );
    assert_eq!(
        accepted_delivery_contents(&pool, run.id, start.id).await,
        vec!["Add workflow task orchestration"]
    );

    let agent_node = run
        .nodes
        .iter()
        .find(|node| node.node_id == "agent")
        .expect("agent node execution");
    assert_eq!(agent_node.session_id, Some(session_id));
    assert_eq!(agent_node.execution_process_id, None);
    assert_eq!(
        agent_node.orchestration_node_execution_id,
        Some(execution_process_id)
    );
    assert_eq!(agent_node.agent_run_id, Some(execution_process_id));
    assert_eq!(
        agent_node.output_text.as_deref(),
        Some("implemented feature")
    );
    assert!(agent_requests[0].prompt.contains(&format!(
        ".vibe-kanban/workflows/{}/handoffs/{}.json",
        run.id, agent_node.id
    )));
    assert_ne!(agent_node.id, execution_process_id);
    assert_eq!(
        accepted_delivery_contents(&pool, run.id, agent_node.id).await,
        vec!["implemented feature"]
    );
    assert_run_delivery(&pool, &run, "implemented feature").await;
}

#[tokio::test]
async fn workflow_reconcile_completed_agent_process_resumes_downstream_nodes() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let first_session_id = Uuid::new_v4();
    let first_execution_process_id = Uuid::new_v4();
    let second_session_id = Uuid::new_v4();
    let second_execution_process_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Two Agent Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(two_agent_graph_json())
    .execute(&pool)
    .await
    .expect("insert two-agent workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = AsyncThenCompleteAgentExecutor::new(
        &pool,
        first_session_id,
        first_execution_process_id,
        second_session_id,
        second_execution_process_id,
    );

    let running = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Run two agents".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger workflow run");

    assert_eq!(running.status, WorkflowRunStatus::Running);
    assert_eq!(
        node_status(&running.nodes, "agent-a"),
        NodeExecutionStatus::Running
    );
    assert_eq!(agent.requests().len(), 1);

    let boundary = FakeCanonicalReconciliationBoundary::completed("agent A done");
    let reconciled = reconcile_workflow_run_with_arena_and_boundary(
        &pool,
        running.id,
        &agent,
        &NoopWorkflowArenaCreator,
        &boundary,
    )
    .await
    .expect("reconcile workflow run");

    let requests = agent.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].node_id, "agent-b");
    assert!(requests[1].prompt.contains("# Workflow Agent Envelope"));
    assert!(requests[1].prompt.contains("Agent B prompt"));
    let first_node = reconciled
        .nodes
        .iter()
        .find(|node| node.node_id == "agent-a")
        .unwrap();
    assert!(requests[1].prompt.contains(&format!(
        ".vibe-kanban/workflows/{}/handoffs/{}.json",
        running.id, first_node.id
    )));
    assert!(!requests[1].prompt.contains("agent A done"));
    assert_eq!(
        accepted_delivery_contents(&pool, reconciled.id, first_node.id).await,
        vec!["agent A done"]
    );
    assert_eq!(reconciled.status, WorkflowRunStatus::Succeeded);
    assert_eq!(
        node_status(&reconciled.nodes, "agent-a"),
        NodeExecutionStatus::Succeeded
    );
    assert_eq!(
        node_status(&reconciled.nodes, "agent-b"),
        NodeExecutionStatus::Succeeded
    );
    assert_eq!(
        node_status(&reconciled.nodes, "end"),
        NodeExecutionStatus::Succeeded
    );
    assert_run_delivery(&pool, &reconciled, "agent B done").await;
}

#[tokio::test]
async fn workflow_reconcile_failed_agent_process_fails_run_and_skips_downstream() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Agent Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(agent_graph_json())
    .execute(&pool)
    .await
    .expect("insert agent workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = StartedAgentExecutor::new(session_id);
    let running = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Run failing agent".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger workflow run");

    let boundary = FakeCanonicalReconciliationBoundary::failed("agent failed");
    let reconciled = reconcile_workflow_run_with_arena_and_boundary(
        &pool,
        running.id,
        &agent,
        &NoopWorkflowArenaCreator,
        &boundary,
    )
    .await
    .expect("reconcile failed workflow run");

    assert_eq!(reconciled.status, WorkflowRunStatus::Failed);
    assert_eq!(
        node_status(&reconciled.nodes, "agent"),
        NodeExecutionStatus::Failed
    );
    assert_eq!(
        node_status(&reconciled.nodes, "end"),
        NodeExecutionStatus::Skipped
    );
}

#[tokio::test]
async fn workflow_runner_fan_in_triggers_same_agent_session_multiple_times() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Fan In Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(fan_in_agent_graph_json())
    .execute(&pool)
    .await
    .expect("insert fan-in workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "reviewed branch");

    let run = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Fan-in should trigger twice".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger workflow run");

    let agent_requests = agent.requests();
    assert_eq!(agent_requests.len(), 2);
    assert_eq!(agent_requests[0].node_id, "review");
    assert_eq!(agent_requests[1].node_id, "review");
    assert_eq!(agent_requests[0].session_id, agent_requests[1].session_id);
    assert!(
        agent_requests[0]
            .prompt
            .contains("# Workflow Agent Envelope")
    );
    assert!(
        agent_requests[0]
            .prompt
            .contains("Review the current worktree.")
    );
    assert_eq!(
        agent_requests[0]
            .prompt
            .split("\n\n## Required file delivery\n")
            .next(),
        agent_requests[1]
            .prompt
            .split("\n\n## Required file delivery\n")
            .next()
    );
    assert_ne!(agent_requests[0].prompt, agent_requests[1].prompt);
    for request in &agent_requests {
        let execution = run
            .nodes
            .iter()
            .find(|node| node.node_id == "review" && node.iteration == request.iteration)
            .unwrap();
        assert!(request.prompt.contains(&format!(
            ".vibe-kanban/workflows/{}/handoffs/{}.json",
            run.id, execution.id
        )));
        assert_eq!(
            accepted_delivery_contents(&pool, run.id, execution.id).await,
            vec!["reviewed branch"]
        );
    }

    let review_iterations = run
        .nodes
        .iter()
        .filter(|node| node.node_id == "review")
        .map(|node| node.iteration)
        .collect::<Vec<_>>();
    assert_eq!(review_iterations, vec![0, 1]);

    let end_iterations = run
        .nodes
        .iter()
        .filter(|node| node.node_id == "end")
        .map(|node| node.iteration)
        .collect::<Vec<_>>();
    assert_eq!(end_iterations, vec![0, 1]);
}

#[tokio::test]
async fn workflow_human_gate_sets_run_awaiting_human() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Human Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(human_gate_graph_json())
    .execute(&pool)
    .await
    .expect("insert human workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "should not run yet");

    let run = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Build workflow approvals".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger workflow run");

    assert_eq!(run.status, WorkflowRunStatus::AwaitingHuman);
    assert!(agent.requests().is_empty());
    assert_eq!(
        node_status(&run.nodes, "gate"),
        NodeExecutionStatus::AwaitingHuman
    );
    assert_eq!(
        node_status(&run.nodes, "agent"),
        NodeExecutionStatus::Pending
    );
}

#[tokio::test]
async fn workflow_arena_node_creates_group_and_waits_for_winner() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    let arena_group_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Arena Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(arena_graph_json())
    .execute(&pool)
    .await
    .expect("insert arena workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "agent should not run");
    let arena = FakeArenaCreator::new(arena_group_id);

    let run = trigger_workflow_run_with_arena(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Build draw-more workflow".to_string(),
        },
        &workspace,
        &agent,
        &arena,
    )
    .await
    .expect("trigger workflow run");

    assert_eq!(run.status, WorkflowRunStatus::AwaitingArena);
    assert!(agent.requests().is_empty());
    assert_eq!(
        node_status(&run.nodes, "arena"),
        NodeExecutionStatus::AwaitingArena
    );
    assert_eq!(node_status(&run.nodes, "end"), NodeExecutionStatus::Pending);

    let arena_node = run
        .nodes
        .iter()
        .find(|node| node.node_id == "arena")
        .expect("arena node execution");
    assert_eq!(arena_node.arena_group_id, Some(arena_group_id));
    let arena_input = arena_node.input_text.as_deref().expect("arena input text");
    assert!(arena_input.contains("# Workflow Agent Envelope"));
    assert!(arena_input.contains("- Type: Arena step"));
    assert!(arena_input.contains("Implement candidates from Source execution:"));
    let plan = run
        .nodes
        .iter()
        .find(|node| node.node_id == "plan")
        .unwrap();
    assert!(arena_input.contains(&format!(
        ".vibe-kanban/workflows/{}/handoffs/{}.json",
        run.id, plan.id
    )));
    assert!(!arena_input.contains("Plan: Build draw-more workflow"));
    assert_eq!(
        accepted_delivery_contents(&pool, run.id, plan.id).await,
        vec!["Plan: Build draw-more workflow"]
    );

    let arena_requests = arena.requests();
    assert_eq!(arena_requests.len(), 1);
    let request = &arena_requests[0];
    assert_eq!(request.run_id, run.id);
    assert_eq!(request.node_id, "arena");
    assert_eq!(request.issue_id, issue_id);
    assert_eq!(request.main_workspace_id, workspace_id);
    assert_eq!(request.prompt, arena_input);
    assert_eq!(request.attempts.len(), 3);
    assert_eq!(
        request
            .attempts
            .iter()
            .map(|attempt| attempt.branch_name.as_str())
            .collect::<Vec<_>>(),
        vec![
            format!("vk/{issue_id}-wf-{}-arena-1", short_run_id(run.id)),
            format!("vk/{issue_id}-wf-{}-arena-2", short_run_id(run.id)),
            format!("vk/{issue_id}-wf-{}-arena-3", short_run_id(run.id)),
        ]
    );
    assert_eq!(
        request
            .attempts
            .iter()
            .map(|attempt| attempt.prompt.as_str())
            .collect::<Vec<_>>(),
        vec![arena_input, arena_input, arena_input]
    );
    assert_eq!(
        request
            .attempts
            .iter()
            .map(|attempt| attempt.display_name.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("Candidate A"),
            Some("Candidate B"),
            Some("Candidate C")
        ]
    );
}

#[tokio::test]
async fn workflow_arena_winner_selection_applies_winner_and_resumes_downstream_nodes() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let winner_candidate_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    let arena_group_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Arena Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(arena_graph_json())
    .execute(&pool)
    .await
    .expect("insert arena workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "agent should not run");
    let arena = FakeArenaCreator::new(arena_group_id);
    let winner = FakeArenaWinnerApplier::succeeds("Winner applied from Candidate B");

    let awaiting = trigger_workflow_run_with_arena(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Build draw-more workflow".to_string(),
        },
        &workspace,
        &agent,
        &arena,
    )
    .await
    .expect("trigger workflow run");
    assert_eq!(awaiting.status, WorkflowRunStatus::AwaitingArena);

    let completed = select_arena_winner_with_arena(
        &pool,
        awaiting.id,
        "arena",
        winner_candidate_id,
        &agent,
        &arena,
        &winner,
    )
    .await
    .expect("select arena winner");

    assert_eq!(completed.status, WorkflowRunStatus::Succeeded);
    assert_eq!(
        node_status(&completed.nodes, "arena"),
        NodeExecutionStatus::Succeeded
    );
    assert_eq!(
        node_status(&completed.nodes, "end"),
        NodeExecutionStatus::Succeeded
    );
    assert_run_delivery(&pool, &completed, "Winner applied from Candidate B").await;

    let arena_node = completed
        .nodes
        .iter()
        .find(|node| node.node_id == "arena")
        .expect("arena node execution");
    assert_eq!(
        arena_node.output_text.as_deref(),
        Some("Winner applied from Candidate B")
    );
    assert_eq!(
        accepted_delivery_contents(&pool, completed.id, arena_node.id).await,
        vec!["Winner applied from Candidate B"]
    );

    let requests = winner.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].run_id, awaiting.id);
    assert_eq!(requests[0].node_id, "arena");
    assert_eq!(requests[0].arena_group_id, arena_group_id);
    assert_eq!(requests[0].main_workspace_id, workspace_id);
    assert_eq!(requests[0].candidate_id, winner_candidate_id);
}

#[tokio::test]
async fn workflow_arena_winner_apply_failure_fails_run_with_conflict_text() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let winner_candidate_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    let arena_group_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Arena Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(arena_graph_json())
    .execute(&pool)
    .await
    .expect("insert arena workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "agent should not run");
    let arena = FakeArenaCreator::new(arena_group_id);
    let winner = FakeArenaWinnerApplier::fails("winner diff conflict in src/main.rs");

    let awaiting = trigger_workflow_run_with_arena(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Build draw-more workflow".to_string(),
        },
        &workspace,
        &agent,
        &arena,
    )
    .await
    .expect("trigger workflow run");

    let failed = select_arena_winner_with_arena(
        &pool,
        awaiting.id,
        "arena",
        winner_candidate_id,
        &agent,
        &arena,
        &winner,
    )
    .await
    .expect("select arena winner reports failed run");

    assert_eq!(failed.status, WorkflowRunStatus::Failed);
    assert_eq!(
        node_status(&failed.nodes, "arena"),
        NodeExecutionStatus::Failed
    );
    assert_eq!(
        node_status(&failed.nodes, "end"),
        NodeExecutionStatus::Skipped
    );
    assert!(
        failed
            .error_text
            .as_deref()
            .is_some_and(|message| message.contains("winner diff conflict"))
    );
}

#[tokio::test]
async fn workflow_arena_winner_selection_requires_awaiting_arena_node() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let winner_candidate_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    let arena_group_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Arena Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(arena_graph_json())
    .execute(&pool)
    .await
    .expect("insert arena workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "agent should not run");
    let arena = FakeArenaCreator::new(arena_group_id);
    let winner = FakeArenaWinnerApplier::succeeds("Winner should not apply");

    let awaiting = trigger_workflow_run_with_arena(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Build draw-more workflow".to_string(),
        },
        &workspace,
        &agent,
        &arena,
    )
    .await
    .expect("trigger workflow run");
    sqlx::query(
        r#"
        UPDATE node_executions
        SET status = 'succeeded'
        WHERE run_id = ? AND node_id = 'arena'
        "#,
    )
    .bind(awaiting.id)
    .execute(&pool)
    .await
    .expect("force arena node succeeded");

    let result = select_arena_winner_with_arena(
        &pool,
        awaiting.id,
        "arena",
        winner_candidate_id,
        &agent,
        &arena,
        &winner,
    )
    .await;

    assert!(matches!(
        result.expect_err("winner selection must reject a consumed interaction"),
        ApiError::Conflict(message) if message.contains("stale or unavailable")
    ));
    assert!(winner.requests().is_empty());
}

#[tokio::test]
async fn workflow_human_approve_resumes_downstream_nodes() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Human Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(human_gate_graph_json())
    .execute(&pool)
    .await
    .expect("insert human workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "approved implementation");

    let run = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Ship the plan".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger workflow run");

    let resumed = approve_human_node(&pool, run.id, "gate", &agent)
        .await
        .expect("approve human gate");

    assert_eq!(resumed.status, WorkflowRunStatus::Succeeded);
    assert_eq!(
        node_status(&resumed.nodes, "gate"),
        NodeExecutionStatus::Succeeded
    );
    assert_eq!(
        node_status(&resumed.nodes, "agent"),
        NodeExecutionStatus::Succeeded
    );
    assert_eq!(agent.requests().len(), 1);
    let approved_prompt = agent.requests()[0].prompt.clone();
    assert!(approved_prompt.contains("# Workflow Agent Envelope"));
    assert!(approved_prompt.contains("After approval: Source execution:"));
    let gate = resumed
        .nodes
        .iter()
        .find(|node| node.node_id == "gate")
        .unwrap();
    assert!(approved_prompt.contains(&format!(
        ".vibe-kanban/workflows/{}/handoffs/{}.json",
        resumed.id, gate.id
    )));
    assert!(!approved_prompt.contains("Ship the plan"));
    assert_eq!(
        accepted_delivery_contents(&pool, resumed.id, gate.id).await,
        vec!["Ship the plan"]
    );
    assert_run_delivery(&pool, &resumed, "approved implementation").await;
}

#[tokio::test]
async fn workflow_human_reject_fails_run() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Human Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(human_gate_graph_json())
    .execute(&pool)
    .await
    .expect("insert human workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "should not run");

    let run = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Review before coding".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger workflow run");

    let rejected = reject_human_node(&pool, run.id, "gate")
        .await
        .expect("reject human gate");

    assert_eq!(rejected.status, WorkflowRunStatus::Failed);
    assert_eq!(
        node_status(&rejected.nodes, "gate"),
        NodeExecutionStatus::Failed
    );
    assert!(agent.requests().is_empty());
    assert!(
        rejected
            .error_text
            .as_deref()
            .is_some_and(|message| message.contains("rejected"))
    );
}

#[tokio::test]
async fn workflow_human_cancel_marks_run_canceled_and_stops_running_session() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Agent Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(agent_graph_json())
    .execute(&pool)
    .await
    .expect("insert agent workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = StartedAgentExecutor::new(session_id);

    let run = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Long-running task".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger workflow run");
    assert_eq!(run.status, WorkflowRunStatus::Running);

    let canceller = FakeRunCanceller::default();
    let cancelling = cancel_workflow_run_runtime(&pool, run.id, &canceller)
        .await
        .expect("cancel workflow run");

    assert_eq!(cancelling.status, WorkflowRunStatus::Cancelling);
    assert!(canceller.stopped_sessions().is_empty());
    assert_eq!(
        canceller.cancelled_orchestration_runs(),
        vec![run.orchestration_run_id.expect("orchestration run")]
    );
    assert_eq!(
        node_status(&cancelling.nodes, "agent"),
        NodeExecutionStatus::Cancelling
    );

    let boundary = FakeCanonicalReconciliationBoundary::cancelled();
    let canceled = reconcile_workflow_run_with_arena_and_boundary(
        &pool,
        run.id,
        &agent,
        &NoopWorkflowArenaCreator,
        &boundary,
    )
    .await
    .expect("reconcile cancelled workflow run");
    assert_eq!(canceled.status, WorkflowRunStatus::Canceled);
    assert_eq!(
        node_status(&canceled.nodes, "agent"),
        NodeExecutionStatus::Cancelled
    );
}

#[tokio::test]
async fn standalone_agent_retry_is_rejected_without_resetting_historical_execution() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Agent Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(agent_graph_json())
    .execute(&pool)
    .await
    .expect("insert agent workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FailOnceAgentExecutor::new(&pool, session_id, "retry succeeded");

    let failed = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Retry just the agent".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger workflow run");
    assert_eq!(failed.status, WorkflowRunStatus::Failed);
    assert_eq!(
        node_status(&failed.nodes, "start"),
        NodeExecutionStatus::Succeeded
    );
    assert_eq!(
        node_status(&failed.nodes, "agent"),
        NodeExecutionStatus::Failed
    );

    let before = serde_json::to_value(&failed).unwrap();
    let rejected = retry_workflow_node(&pool, failed.id, "agent", &agent).await;
    assert!(
        matches!(rejected,Err(ApiError::Conflict(message)) if message.contains("stable Workflow instance"))
    );
    assert_eq!(agent.requests().len(), 1);
    let preserved = server::workflow_runtime::runner::get_workflow_run_response(&pool, failed.id)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(preserved).unwrap(), before);
}

#[tokio::test]
async fn standalone_transform_retry_never_falls_back_to_a_mutated_publication() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Transform Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(failing_transform_graph_json())
    .execute(&pool)
    .await
    .expect("insert transform workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "unused");

    let failed = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "No ticket here".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger workflow run");
    assert_eq!(failed.status, WorkflowRunStatus::Failed);
    assert_eq!(
        node_status(&failed.nodes, "transform"),
        NodeExecutionStatus::Failed
    );
    let original_snapshot: String =
        sqlx::query_scalar("SELECT graph_snapshot FROM workflow_runs WHERE id = ?")
            .bind(failed.id)
            .fetch_one(&pool)
            .await
            .expect("load immutable graph snapshot");
    assert_eq!(
        serde_json::from_str::<Value>(&original_snapshot).expect("parse stored snapshot"),
        serde_json::from_str::<Value>(&failing_transform_graph_json())
            .expect("parse failing graph")
    );

    sqlx::query("UPDATE workflows SET graph_json = ? WHERE id = ?")
        .bind(fixed_transform_graph_json())
        .bind(workflow_id)
        .execute(&pool)
        .await
        .expect("fix transform workflow");

    let rejected = retry_workflow_node(&pool, failed.id, "transform", &agent).await;
    assert!(
        matches!(rejected,Err(ApiError::Conflict(message)) if message.contains("stable Workflow instance"))
    );
    let snapshot_after_retry: String =
        sqlx::query_scalar("SELECT graph_snapshot FROM workflow_runs WHERE id = ?")
            .bind(failed.id)
            .fetch_one(&pool)
            .await
            .expect("reload immutable graph snapshot");
    assert_eq!(snapshot_after_retry, original_snapshot);
}

#[tokio::test]
async fn workflow_retry_rejects_run_without_immutable_snapshot() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Mutable fallback must not run', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(fixed_transform_graph_json())
    .execute(&pool)
    .await
    .expect("insert mutable workflow graph");
    sqlx::query(
        r#"
        INSERT INTO workflow_runs
            (id, workflow_id, issue_id, workspace_id, trigger_source, input_text, status)
        VALUES (?, ?, ?, ?, 'manual', 'Do not use mutable graph', 'failed')
        "#,
    )
    .bind(run_id)
    .bind(workflow_id)
    .bind(issue_id)
    .bind(workspace_id)
    .execute(&pool)
    .await
    .expect("insert run without snapshot");

    let result = retry_workflow_node(
        &pool,
        run_id,
        "transform",
        &FakeAgentExecutor::new(&pool, Uuid::new_v4(), "unused"),
    )
    .await;

    assert!(matches!(
        result,
        Err(ApiError::Conflict(message)) if message.contains("no immutable graph snapshot")
    ));
}

#[tokio::test]
async fn workflow_run_rejects_condition_without_router_config() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Condition Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(failing_condition_graph_json())
    .execute(&pool)
    .await
    .expect("insert condition workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "unused");

    let result = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "LGTM".to_string(),
        },
        &workspace,
        &agent,
    )
    .await;

    assert!(
        matches!(
            result,
            Err(ApiError::BadRequest(message))
                if message.contains("workflow with condition nodes requires router executor config")
        ),
        "condition workflows without an explicit router config must be blocked before run"
    );
}

#[tokio::test]
async fn workflow_human_recovery_marks_stale_running_nodes_failed() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let node_execution_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Agent Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(agent_graph_json())
    .execute(&pool)
    .await
    .expect("insert workflow");
    sqlx::query(
        r#"
        INSERT INTO workflow_runs
            (id, workflow_id, issue_id, workspace_id, trigger_source, input_text, status)
        VALUES (?, ?, ?, NULL, 'manual', 'Recover this', 'running')
        "#,
    )
    .bind(run_id)
    .bind(workflow_id)
    .bind(issue_id)
    .execute(&pool)
    .await
    .expect("insert run");
    sqlx::query(
        r#"
        INSERT INTO node_executions (id, run_id, node_id, node_type, iteration, status)
        VALUES (?, ?, 'agent', 'agent', 0, 'running')
        "#,
    )
    .bind(node_execution_id)
    .bind(run_id)
    .execute(&pool)
    .await
    .expect("insert stale node");
    link_canonical_orchestration_fixture(&pool, run_id, node_execution_id, "agent").await;

    let recovered = recover_stale_workflow_runs(&pool)
        .await
        .expect("recover stale workflow runs");
    let run = server::workflow_runtime::runner::get_workflow_run_response(&pool, run_id)
        .await
        .expect("get recovered run");

    assert_eq!(recovered, 1);
    assert_eq!(run.status, WorkflowRunStatus::Running);
    assert_eq!(
        node_status(&run.nodes, "agent"),
        NodeExecutionStatus::Running
    );
    assert!(run.error_text.is_none());
}

#[tokio::test]
async fn recovery_syncs_attempt_status_for_stale_running_run() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let attempt_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let node_execution_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;
    insert_local_issue(&pool, project_id, issue_id, "Recover attempt").await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Recover Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(agent_graph_json())
    .execute(&pool)
    .await
    .expect("insert workflow");

    sqlx::query(
        r#"
        INSERT INTO tasks (id, project_id, issue_id, title, execution_kind)
        VALUES (?, ?, ?, 'Recover attempt', 'workflow')
        "#,
    )
    .bind(attempt_id)
    .bind(project_id)
    .bind(issue_id)
    .execute(&pool)
    .await
    .expect("insert canonical workflow Task");

    sqlx::query(
        r#"
        INSERT INTO workflow_attempts (id, task_id, workflow_id, status)
        VALUES (?, ?, ?, 'running')
        "#,
    )
    .bind(attempt_id)
    .bind(attempt_id)
    .bind(workflow_id)
    .execute(&pool)
    .await
    .expect("insert attempt");

    sqlx::query(
        r#"
        INSERT INTO workflow_runs
            (id, workflow_id, attempt_id, issue_id, workspace_id, trigger_source, input_text, status)
        VALUES (?, ?, ?, ?, NULL, 'manual', 'Recover this', 'running')
        "#,
    )
    .bind(run_id)
    .bind(workflow_id)
    .bind(attempt_id)
    .bind(issue_id)
    .execute(&pool)
    .await
    .expect("insert run");

    sqlx::query("UPDATE workflow_attempts SET latest_run_id = ? WHERE id = ?")
        .bind(run_id)
        .bind(attempt_id)
        .execute(&pool)
        .await
        .expect("link attempt latest run");

    sqlx::query(
        r#"
        INSERT INTO node_executions (id, run_id, node_id, node_type, iteration, status)
        VALUES (?, ?, 'agent', 'agent', 0, 'running')
        "#,
    )
    .bind(node_execution_id)
    .bind(run_id)
    .execute(&pool)
    .await
    .expect("insert stale node");
    link_canonical_orchestration_fixture(&pool, run_id, node_execution_id, "agent").await;

    let recovered = recover_stale_workflow_runs(&pool)
        .await
        .expect("recover stale workflow runs");
    assert_eq!(recovered, 1);

    let attempt_status: String =
        sqlx::query_scalar("SELECT status FROM workflow_attempts WHERE id = ?")
            .bind(attempt_id)
            .fetch_one(&pool)
            .await
            .expect("fetch attempt status");
    assert_eq!(attempt_status, "running");
}

#[tokio::test]
async fn workflow_events_records_run_and_node_status_changes() {
    let pool = setup_workflow_pool().await;
    let project_id = Uuid::new_v4();
    let issue_id = Uuid::new_v4();
    let workflow_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    insert_project(&pool, project_id).await;

    sqlx::query(
        r#"
        INSERT INTO workflows (id, source, project_id, name, description, graph_json)
        VALUES (?, 'project', ?, 'Human Flow', NULL, ?)
        "#,
    )
    .bind(workflow_id)
    .bind(project_id)
    .bind(human_gate_graph_json())
    .execute(&pool)
    .await
    .expect("insert human workflow");

    let workspace = FakeWorkspaceResolver::new(&pool, workspace_id);
    let agent = FakeAgentExecutor::new(&pool, session_id, "approved implementation");
    let run = trigger_workflow_run(
        &pool,
        workflow_id,
        TriggerWorkflowRequest {
            issue_id,
            workspace_id: None,
            trigger_source: "manual".to_string(),
            input_text: "Emit workflow events".to_string(),
        },
        &workspace,
        &agent,
    )
    .await
    .expect("trigger workflow run");

    approve_human_node(&pool, run.id, "gate", &agent)
        .await
        .expect("approve human gate");

    let events = workflow_event_history(run.id);
    assert!(
        events
            .iter()
            .any(|event| event.kind == workflow::WorkflowEventKind::RunStatus
                && event.status.as_deref() == Some("awaiting_human"))
    );
    assert!(events.iter().any(
        |event| event.kind == workflow::WorkflowEventKind::NodeWaitingHuman
            && event.node_id.as_deref() == Some("gate")
    ));
    assert!(events.iter().any(
        |event| event.kind == workflow::WorkflowEventKind::NodeStatus
            && event.node_id.as_deref() == Some("agent")
            && event.status.as_deref() == Some("succeeded")
    ));
    assert!(
        events
            .iter()
            .any(|event| event.kind == workflow::WorkflowEventKind::RunStatus
                && event.status.as_deref() == Some("succeeded"))
    );
}

fn short_run_id(run_id: Uuid) -> String {
    run_id.simple().to_string()[..8].to_string()
}

fn assert_payload_array_key(payload: &Value, key: &str) {
    assert!(
        payload.get(key).and_then(Value::as_array).is_some(),
        "fallback payload should include an array at `{key}`"
    );
}

fn node_status(nodes: &[WorkflowNodeExecutionResponse], node_id: &str) -> NodeExecutionStatus {
    nodes
        .iter()
        .find(|node| node.node_id == node_id)
        .unwrap_or_else(|| panic!("missing node execution `{node_id}`"))
        .status
}
