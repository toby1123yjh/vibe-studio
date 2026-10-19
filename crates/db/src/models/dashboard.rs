//! Read-only Dashboard facts. No business Task titles, native frames, or event
//! histories are loaded here; optional ownership never removes a live Session.
use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

use super::{
    arena_group::{ArenaCandidate, ArenaGroup},
    task::Execution,
};

#[derive(Debug, Clone, FromRow)]
pub struct DashboardSessionFact {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: Option<String>,
    pub executor: Option<String>,
    pub directory: Option<String>,
    pub workspace_kind: String,
    pub workspace_branch: Option<String>,
    pub project_id: Option<Uuid>,
    pub execution_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct DashboardRunFact {
    pub id: Uuid,
    pub session_id: Uuid,
    pub workspace_id: Uuid,
    pub provider_id: String,
    pub runtime_profile_id: String,
    pub workspace_path: String,
    pub workspace_mode: String,
    pub status: String,
    pub projection_status: String,
    pub last_run_attempt_number: i64,
    pub last_event_sequence: i64,
    pub last_event_id: Option<Uuid>,
    pub observed_model: Option<String>,
    pub observed_reasoning: Option<String>,
    pub requested_model: Option<String>,
    pub requested_reasoning: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct DashboardWorkflowFact {
    pub id: Uuid,
    pub execution_id: Uuid,
    pub workflow_id: Uuid,
    pub title: String,
    pub project_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub latest_run_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub main_session_id: Option<Uuid>,
    pub main_agent_run_id: Option<Uuid>,
    pub status: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct DashboardNodeFact {
    pub id: Uuid,
    pub run_id: Uuid,
    pub attempt_id: Option<Uuid>,
    pub execution_id: Option<Uuid>,
    pub node_id: String,
    pub node_type: String,
    pub title: Option<String>,
    pub iteration: i64,
    pub status: String,
    pub session_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
    pub arena_group_id: Option<Uuid>,
    pub reused: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct DashboardSubtaskFact {
    pub id: Uuid,
    pub run_id: Uuid,
    pub attempt_id: Option<Uuid>,
    pub title: String,
    pub node_execution_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
    pub status: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct DashboardRepoFact {
    pub workspace_id: Uuid,
    pub path: String,
    pub name: String,
    pub workspace_path: Option<String>,
    pub workspace_kind: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct DashboardArenaRunFact {
    pub arena_group_id: Uuid,
    pub candidate_id: Uuid,
    pub session_id: Uuid,
    pub agent_run_id: Uuid,
    pub latest: bool,
}

pub struct DashboardFacts {
    pub sessions: Vec<DashboardSessionFact>,
    pub runs: Vec<DashboardRunFact>,
    pub executions: Vec<Execution>,
    pub workflows: Vec<DashboardWorkflowFact>,
    pub nodes: Vec<DashboardNodeFact>,
    pub subtasks: Vec<DashboardSubtaskFact>,
    pub arenas: Vec<ArenaGroup>,
    pub candidates: Vec<ArenaCandidate>,
    pub arena_runs: Vec<DashboardArenaRunFact>,
}

impl DashboardFacts {
    /// A fixed set of batch reads: no per-row history scans or writer locks.
    pub async fn load(pool: &SqlitePool) -> Result<Self, sqlx::Error> {
        let sessions = sqlx::query_as::<_, DashboardSessionFact>(
            r#"
            SELECT s.id,s.workspace_id,
                   COALESCE(NULLIF(TRIM(s.name),''),NULLIF(TRIM(w.name),'')) AS name,
                   s.executor,
                   COALESCE(NULLIF(s.agent_working_dir,''),w.container_ref) AS directory,
                   COALESCE(w.workspace_kind,'unknown') AS workspace_kind,
                   w.branch AS workspace_branch,m.project_id,b.task_id AS execution_id,
                   s.created_at,s.updated_at
            FROM sessions s
            LEFT JOIN workspaces w ON w.id=s.workspace_id
            LEFT JOIN session_project_memberships m ON m.session_id=s.id
            LEFT JOIN agent_task_bindings b ON b.session_id=s.id
        "#,
        )
        .fetch_all(pool)
        .await?;
        let runs = sqlx::query_as::<_, DashboardRunFact>(r#"
            WITH ranked AS (
                SELECT r.id,ROW_NUMBER() OVER (
                    PARTITION BY r.session_id ORDER BY julianday(r.created_at) DESC,r.id ASC
                ) AS position FROM agent_runs r
            )
            SELECT r.id,r.session_id,r.workspace_id,r.provider_id,r.runtime_profile_id,
                   r.workspace_path,r.workspace_mode,COALESCE(s.status,r.status) AS status,
                   COALESCE(s.projection_status,r.projection_status) AS projection_status,
                   COALESCE(s.last_run_attempt_number,0) AS last_run_attempt_number,
                   COALESCE(s.last_event_sequence,0) AS last_event_sequence,s.last_event_id,
                   json_extract(s.state_json,'$.observed_configuration.model_id') AS observed_model,
                   json_extract(s.state_json,'$.observed_configuration.reasoning_id') AS observed_reasoning,
                   json_extract(a.request_envelope,'$.executor_config.model_id') AS requested_model,
                   json_extract(a.request_envelope,'$.executor_config.reasoning_id') AS requested_reasoning,
                   a.started_at,a.finished_at,r.created_at,r.updated_at
            FROM agent_runs r JOIN ranked ON ranked.id=r.id
            LEFT JOIN agent_run_state s ON s.agent_run_id=r.id
            LEFT JOIN agent_run_attempts a ON a.id=s.last_run_attempt_id
            WHERE ranked.position=1 OR COALESCE(s.status,r.status) IN
                ('pending','starting','running','awaiting_input','awaiting_approval','cancelling')
                OR r.id IN (SELECT agent_run_id FROM node_executions WHERE agent_run_id IS NOT NULL)
                OR r.id IN (SELECT actual_main_agent_run_id FROM workflow_main_session_bindings)
                OR r.session_id IN (SELECT main_session_id FROM workflow_attempts)
                OR r.id IN (SELECT agent_run_id FROM orchestration_agent_run_links)
            ORDER BY julianday(r.created_at) DESC,r.id ASC
        "#).fetch_all(pool).await?;
        let executions = sqlx::query_as::<_, Execution>(
            "SELECT id,project_id,issue_id,parent_task_id,title,execution_kind,created_at,updated_at FROM tasks"
        ).fetch_all(pool).await?;
        let workflows = sqlx::query_as::<_, DashboardWorkflowFact>(r#"
            SELECT a.id,a.task_id AS execution_id,a.workflow_id,w.name AS title,
                   t.project_id,t.issue_id AS task_id,a.latest_run_id,a.workspace_id,a.main_session_id,
                   COALESCE(
                       (SELECT h.agent_run_id FROM workflow_handover_events h WHERE h.run_id=a.latest_run_id
                         AND h.session_id=a.main_session_id AND h.agent_run_id IS NOT NULL
                         ORDER BY julianday(h.created_at) DESC,h.id ASC LIMIT 1),
                       (SELECT turn.agent_run_id FROM workflow_run_submissions sub
                         JOIN agent_turns turn ON replace(json_extract(turn.input_message,'$.message_id'),'-','')=lower(hex(sub.source_message_id))
                         JOIN agent_runs main_run ON main_run.id=turn.agent_run_id AND main_run.session_id=a.main_session_id
                         WHERE sub.run_id=a.latest_run_id ORDER BY julianday(main_run.created_at) DESC,main_run.id ASC LIMIT 1),
                       CASE WHEN a.latest_run_id IS NULL THEN b.actual_main_agent_run_id END
                   ) AS main_agent_run_id,
                   COALESCE(r.status,a.status) AS status,
                   CASE WHEN julianday(r.updated_at)>julianday(a.updated_at) THEN r.updated_at ELSE a.updated_at END AS updated_at
            FROM workflow_attempts a
            LEFT JOIN tasks t ON t.id=a.task_id
            JOIN workflows w ON w.id=a.workflow_id
            LEFT JOIN workflow_runs r ON r.id=a.latest_run_id
            LEFT JOIN workflow_main_session_bindings b ON b.session_id=a.main_session_id
        "#).fetch_all(pool).await?;
        let nodes = sqlx::query_as::<_, DashboardNodeFact>(r#"
            SELECT n.id,n.run_id,r.attempt_id,n.task_id AS execution_id,n.node_id,n.node_type,
                   t.title,n.iteration,n.status,n.session_id,n.agent_run_id,n.arena_group_id,
                   0 AS reused,n.updated_at
            FROM node_executions n JOIN workflow_runs r ON r.id=n.run_id
            LEFT JOIN tasks t ON t.id=n.task_id
            WHERE r.id IN (SELECT latest_run_id FROM workflow_attempts)
               OR n.agent_run_id IN (SELECT id FROM agent_runs WHERE status IN
                 ('pending','starting','running','awaiting_input','awaiting_approval','cancelling'))
               OR n.arena_group_id IN (
                   SELECT o.source_definition_id FROM orchestration_runs o
                   JOIN orchestration_agent_run_links l ON l.orchestration_run_id=o.id
                   JOIN agent_runs ar ON ar.id=l.agent_run_id
                   WHERE o.product_kind='arena' AND ar.status IN
                     ('pending','starting','running','awaiting_input','awaiting_approval','cancelling')
               )
            UNION ALL
            SELECT n.id,reuse.run_id,r.attempt_id,n.task_id,n.node_id,n.node_type,
                   t.title,reuse.iteration,n.status,n.session_id,n.agent_run_id,n.arena_group_id,
                   1 AS reused,n.updated_at
            FROM workflow_result_reuse reuse JOIN node_executions n ON n.id=reuse.source_node_execution_id
            JOIN workflow_runs r ON r.id=reuse.run_id LEFT JOIN tasks t ON t.id=n.task_id
            WHERE r.id IN (SELECT latest_run_id FROM workflow_attempts)
        "#).fetch_all(pool).await?;
        let subtasks = sqlx::query_as::<_, DashboardSubtaskFact>(r#"
            SELECT s.id,b.run_id,r.attempt_id,s.title,s.node_execution_id,s.session_id,
                   n.agent_run_id,s.state AS status,s.updated_at
            FROM workflow_split_subtasks s JOIN workflow_split_batches b ON b.id=s.batch_id
            JOIN workflow_runs r ON r.id=b.run_id LEFT JOIN node_executions n ON n.id=s.node_execution_id
            WHERE b.run_id IN (SELECT latest_run_id FROM workflow_attempts)
               OR s.state IN ('running','queued','blocked')
        "#).fetch_all(pool).await?;
        let arenas = sqlx::query_as::<_, ArenaGroup>("SELECT * FROM arena_groups")
            .fetch_all(pool)
            .await?;
        let candidates = sqlx::query_as::<_, ArenaCandidate>(
            "SELECT * FROM arena_candidates ORDER BY sort_order,id",
        )
        .fetch_all(pool)
        .await?;
        let arena_runs = sqlx::query_as::<_,DashboardArenaRunFact>(r#"
            WITH candidate_runs AS (
                SELECT o.source_definition_id AS arena_group_id,c.id AS candidate_id,
                       r.session_id,r.id AS agent_run_id,
                       ROW_NUMBER() OVER (PARTITION BY c.id ORDER BY julianday(o.created_at) DESC,julianday(r.created_at) DESC,r.id ASC) AS position,
                       COALESCE(s.status,r.status) AS status
                FROM orchestration_runs o
                JOIN orchestration_agent_run_links l ON l.orchestration_run_id=o.id
                JOIN agent_runs r ON r.id=l.agent_run_id
                LEFT JOIN agent_run_state s ON s.agent_run_id=r.id
                JOIN arena_candidates c ON c.arena_group_id=o.source_definition_id AND c.workspace_id=r.workspace_id
                WHERE o.product_kind='arena'
            )
            SELECT arena_group_id,candidate_id,session_id,agent_run_id,position=1 AS latest FROM candidate_runs
            WHERE position=1 OR status IN ('pending','starting','running','awaiting_input','awaiting_approval','cancelling')
        "#).fetch_all(pool).await?;
        Ok(Self {
            sessions,
            runs,
            executions,
            workflows,
            nodes,
            subtasks,
            arenas,
            candidates,
            arena_runs,
        })
    }

    pub async fn repositories(
        pool: &SqlitePool,
        workspace_id: Uuid,
    ) -> Result<Vec<DashboardRepoFact>, sqlx::Error> {
        sqlx::query_as(r#"SELECT wr.workspace_id,r.path,r.name,w.container_ref AS workspace_path,
            w.workspace_kind FROM workspace_repos wr JOIN repos r ON r.id=wr.repo_id
            JOIN workspaces w ON w.id=wr.workspace_id WHERE wr.workspace_id=? ORDER BY r.name,r.id"#)
            .bind(workspace_id).fetch_all(pool).await
    }
}
