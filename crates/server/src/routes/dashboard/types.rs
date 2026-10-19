use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum DashboardWorkKind {
    Task,
    Workflow,
    Session,
    Arena,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
pub struct DashboardWorkRef {
    pub kind: DashboardWorkKind,
    pub id: Uuid,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum DashboardStatusFilter {
    #[default]
    Current,
    Waiting,
    Running,
    Failed,
    Finished,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum DashboardStatusGroup {
    Waiting,
    Running,
    Failed,
    Finished,
    Idle,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct DashboardFilters {
    pub status: DashboardStatusFilter,
    pub kinds: Vec<DashboardWorkKind>,
    pub project_id: Option<Uuid>,
    pub provider_id: Option<String>,
    /// direct_folder, worktree, or unknown.
    pub directory_kind: Option<String>,
    pub search: String,
    /// Supported windows are 24 hours, 7 days, and 30 days. Active work is never aged out.
    pub history_hours: Option<u32>,
    pub group_by_project: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardExpandedPage {
    pub work: DashboardWorkRef,
    pub workflow_run_id: Option<Uuid>,
    pub snapshot_id: Option<Uuid>,
    pub page: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct DashboardQuery {
    pub filters: DashboardFilters,
    pub snapshot_id: Option<Uuid>,
    pub page: u32,
    pub page_size: Option<u32>,
    pub expanded: Vec<DashboardExpandedPage>,
    pub visible: Vec<DashboardWorkRef>,
    pub refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardIssue {
    pub code: String,
    pub message: String,
    pub work: Option<DashboardWorkRef>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct DashboardCounts {
    pub work_count: u32,
    pub active_session_count: u32,
    pub active_run_count: u32,
    pub waiting: u32,
    pub running: u32,
    pub failed: u32,
    pub finished: u32,
    pub current: u32,
    pub all: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardNamedRef {
    pub id: Uuid,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardAgent {
    pub provider_id: String,
    pub runtime_profile_id: Option<String>,
    pub model_id: Option<String>,
    pub reasoning_id: Option<String>,
    /// observed, requested, or unavailable.
    pub configuration_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(tag = "kind", rename_all = "snake_case")]
pub enum DashboardOpenTarget {
    Session {
        workspace_id: Uuid,
        session_id: Uuid,
        agent_run_id: Option<Uuid>,
        workflow_run_id: Option<Uuid>,
        node_execution_id: Option<Uuid>,
        workflow_node_id: Option<String>,
    },
    Workflow {
        attempt_id: Uuid,
        workflow_id: Uuid,
        workflow_run_id: Option<Uuid>,
        project_id: Option<Uuid>,
    },
    Arena {
        arena_group_id: Uuid,
        project_id: Option<Uuid>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardReplyPreview {
    pub text: String,
    pub truncated: bool,
    pub occurred_at: DateTime<Utc>,
    pub message_id: String,
    pub session_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_run_id: Uuid,
    pub run_attempt_id: Option<Uuid>,
    pub event_id: Option<Uuid>,
    pub source_role: String,
    pub source_title: String,
    pub caught_up: bool,
    pub open_target: DashboardOpenTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardActivity {
    pub kind: String,
    pub description: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub status: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardDirectorySummary {
    pub path: Option<String>,
    pub kind: String,
    pub git_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardWorkRow {
    pub work: DashboardWorkRef,
    pub title: String,
    pub project: Option<DashboardNamedRef>,
    pub task: Option<DashboardNamedRef>,
    pub execution_id: Option<Uuid>,
    pub workflow_run_id: Option<Uuid>,
    pub raw_status: String,
    pub status_group: DashboardStatusGroup,
    pub directory: DashboardDirectorySummary,
    pub agents: Vec<DashboardAgent>,
    pub active_session_count: u32,
    pub active_run_count: u32,
    pub matching_session_count: u32,
    pub waiting_count: u32,
    pub child_count: u32,
    pub preview: Option<DashboardReplyPreview>,
    /// ready, catching_up, rebuilding, missing, or unavailable.
    pub preview_status: String,
    pub activity: Option<DashboardActivity>,
    pub open_target: Option<DashboardOpenTarget>,
    pub updated_at: DateTime<Utc>,
    pub issues: Vec<DashboardIssue>,
    pub stale_membership: bool,
    pub invalidated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardChildRow {
    pub id: String,
    pub parent: DashboardWorkRef,
    /// main_agent, node, subtask, arena_candidate, other_active_run, or session.
    pub role: String,
    pub title: String,
    pub workflow_run_id: Option<Uuid>,
    pub node_execution_id: Option<Uuid>,
    pub subtask_id: Option<Uuid>,
    pub arena_candidate_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
    pub raw_status: String,
    /// Canonical process status is independent of Node control-plane waiting.
    #[serde(default)]
    #[ts(optional)]
    pub agent_run_status: Option<String>,
    pub status_group: DashboardStatusGroup,
    pub agent: Option<DashboardAgent>,
    pub directory: DashboardDirectorySummary,
    pub preview: Option<DashboardReplyPreview>,
    pub preview_status: String,
    pub activity: Option<DashboardActivity>,
    pub open_target: Option<DashboardOpenTarget>,
    pub reused: bool,
    pub matches_filter: bool,
    pub issues: Vec<DashboardIssue>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardChildrenPage {
    pub work: DashboardWorkRef,
    pub workflow_run_id: Option<Uuid>,
    pub snapshot_id: Option<Uuid>,
    pub page: u32,
    pub page_size: u32,
    pub total: u32,
    pub rows: Vec<DashboardChildRow>,
    pub has_updates: bool,
    pub invalidation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardPageResponse {
    pub generated_at: DateTime<Utc>,
    pub snapshot_id: Option<Uuid>,
    pub snapshot_status: String,
    pub page: u32,
    pub page_size: u32,
    pub total: u32,
    pub latest_total: u32,
    pub counts: DashboardCounts,
    pub rows: Vec<DashboardWorkRow>,
    pub expanded: Vec<DashboardChildrenPage>,
    pub has_updates: bool,
    pub issues: Vec<DashboardIssue>,
    pub projects: Vec<DashboardNamedRef>,
    pub providers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardGitRemote {
    pub name: String,
    pub url: String,
    pub web_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardGitRepository {
    pub path: String,
    /// git, not_git, missing, or unreadable.
    pub status: String,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub remotes: Vec<DashboardGitRemote>,
    pub issue: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardSessionContext {
    pub session_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_run_id: Option<Uuid>,
    pub directory: DashboardDirectorySummary,
    pub agent: Option<DashboardAgent>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DashboardContext {
    pub work: DashboardWorkRef,
    pub generated_at: DateTime<Utc>,
    pub sessions: Vec<DashboardSessionContext>,
    pub repositories: Vec<DashboardGitRepository>,
    pub issues: Vec<DashboardIssue>,
}
