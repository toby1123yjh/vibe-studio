use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use db::models::{
    agent_run_preview::{AGENT_RUN_PREVIEW_VERSION, AgentRunPreviewRecord},
    dashboard::{DashboardFacts, DashboardRunFact, DashboardSessionFact},
    project::DEFAULT_PROJECT_ID,
    task::ExecutionKind,
};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::{snapshot, types::*};
use crate::{error::ApiError, routes::project_store};

pub(super) struct Work {
    pub row: DashboardWorkRow,
    pub children: Vec<DashboardChildRow>,
}

pub(super) struct DashboardRead {
    pub facts: DashboardFacts,
    pub works: Vec<Work>,
    pub issues: Vec<DashboardIssue>,
}

fn add_child(
    works: &mut HashMap<DashboardWorkRef, Work>,
    owners: &mut HashMap<Uuid, HashSet<DashboardWorkRef>>,
    work: &DashboardWorkRef,
    child: DashboardChildRow,
) {
    if let Some(entry) = works.get_mut(work) {
        if !child.reused
            && let Some(id) = child.session_id
        {
            owners.entry(id).or_default().insert(work.clone());
        }
        if !entry.children.iter().any(|c| c.id == child.id) {
            entry.children.push(child);
        }
    }
}

fn set_node_target(child: &mut DashboardChildRow, node_id: &str) {
    if let Some(DashboardOpenTarget::Session {
        workflow_node_id, ..
    }) = &mut child.open_target
    {
        *workflow_node_id = Some(node_id.into());
    }
    if let Some(preview) = &mut child.preview
        && let DashboardOpenTarget::Session {
            workflow_node_id, ..
        } = &mut preview.open_target
    {
        *workflow_node_id = Some(node_id.into());
    }
}

pub(super) fn issue(
    code: &str,
    message: impl Into<String>,
    work: Option<DashboardWorkRef>,
) -> DashboardIssue {
    DashboardIssue {
        code: code.into(),
        message: message.into(),
        work,
    }
}

pub(super) fn work_key(work: &DashboardWorkRef) -> String {
    format!("{}:{}", kind_name(work.kind), work.id)
}

pub(super) fn kind_name(kind: DashboardWorkKind) -> &'static str {
    match kind {
        DashboardWorkKind::Task => "task",
        DashboardWorkKind::Workflow => "workflow",
        DashboardWorkKind::Session => "session",
        DashboardWorkKind::Arena => "arena",
        DashboardWorkKind::Unresolved => "unresolved",
    }
}

pub(super) fn status_group(status: &str) -> DashboardStatusGroup {
    match status {
        "awaiting_input" | "awaiting_approval" | "awaiting_human" | "awaiting_arena"
        | "blocked" => DashboardStatusGroup::Waiting,
        "pending" | "starting" | "running" | "cancelling" | "queued" => {
            DashboardStatusGroup::Running
        }
        "failed" | "crashed" | "audit_failed" => DashboardStatusGroup::Failed,
        "succeeded"
        | "cancelled"
        | "canceled"
        | "skipped"
        | "closed"
        | "adopted"
        | "implementation_started" => DashboardStatusGroup::Finished,
        _ => DashboardStatusGroup::Idle,
    }
}

pub(super) fn is_active(status: &str) -> bool {
    matches!(
        status_group(status),
        DashboardStatusGroup::Running | DashboardStatusGroup::Waiting
    )
}

pub(super) fn agent(run: &DashboardRunFact) -> DashboardAgent {
    let observed = run.observed_model.is_some() || run.observed_reasoning.is_some();
    DashboardAgent {
        provider_id: run.provider_id.clone(),
        runtime_profile_id: Some(run.runtime_profile_id.clone()),
        model_id: if observed {
            run.observed_model.clone()
        } else {
            run.requested_model.clone()
        },
        reasoning_id: if observed {
            run.observed_reasoning.clone()
        } else {
            run.requested_reasoning.clone()
        },
        configuration_source: if observed {
            "observed"
        } else if run.requested_model.is_some() || run.requested_reasoning.is_some() {
            "requested"
        } else {
            "unavailable"
        }
        .into(),
    }
}

fn directory(
    session: Option<&DashboardSessionFact>,
    run: Option<&DashboardRunFact>,
) -> DashboardDirectorySummary {
    DashboardDirectorySummary {
        path: run
            .map(|r| r.workspace_path.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| session.and_then(|s| s.directory.clone())),
        kind: session
            .map(|s| s.workspace_kind.clone())
            .or_else(|| run.map(|r| r.workspace_mode.clone()))
            .unwrap_or_else(|| "unknown".into()),
        git_branch: session
            .and_then(|s| s.workspace_branch.clone())
            .filter(|s| s != "direct-folder"),
    }
}

fn empty_row(work: DashboardWorkRef, title: String, updated_at: DateTime<Utc>) -> DashboardWorkRow {
    DashboardWorkRow {
        work,
        title,
        project: None,
        task: None,
        execution_id: None,
        workflow_run_id: None,
        raw_status: "not_started".into(),
        status_group: DashboardStatusGroup::Idle,
        directory: directory(None, None),
        agents: vec![],
        active_session_count: 0,
        active_run_count: 0,
        matching_session_count: 0,
        waiting_count: 0,
        child_count: 0,
        preview: None,
        preview_status: "missing".into(),
        activity: None,
        open_target: None,
        updated_at,
        issues: vec![],
        stale_membership: false,
        invalidated: false,
    }
}

fn preview_for(
    record: Option<&AgentRunPreviewRecord>,
    run: &DashboardRunFact,
    session: &DashboardSessionFact,
    role: &str,
    title: &str,
    target: &DashboardOpenTarget,
) -> (
    Option<DashboardReplyPreview>,
    Option<DashboardActivity>,
    String,
) {
    let Some(record) = record else {
        return (None, None, "missing".into());
    };
    if record.algorithm_version != AGENT_RUN_PREVIEW_VERSION {
        return (None, None, "rebuilding".into());
    }
    let caught_up = record.last_run_attempt_number == run.last_run_attempt_number
        && record.last_sequence == run.last_event_sequence
        && record.last_event_id == run.last_event_id;
    let preview = record
        .preview
        .0
        .assistant
        .as_ref()
        .map(|reply| DashboardReplyPreview {
            text: reply.text.clone(),
            truncated: reply.truncated,
            occurred_at: reply.timestamp,
            message_id: reply.message_id.to_string(),
            session_id: session.id,
            workspace_id: session.workspace_id,
            agent_run_id: run.id,
            run_attempt_id: Some(reply.run_attempt_id),
            event_id: Some(reply.event_id),
            source_role: role.into(),
            source_title: title.into(),
            caught_up,
            open_target: target.clone(),
        });
    let activity = record
        .preview
        .0
        .activity
        .as_ref()
        .map(|a| DashboardActivity {
            kind: a.kind.as_str().into(),
            description: Some(a.label.clone()),
            status: a.status.clone(),
            occurred_at: a.timestamp,
        });
    (
        preview,
        activity,
        if caught_up { "ready" } else { "catching_up" }.into(),
    )
}

struct Builder<'a> {
    sessions: HashMap<Uuid, &'a DashboardSessionFact>,
    runs: HashMap<Uuid, &'a DashboardRunFact>,
    session_runs: HashMap<Uuid, Vec<&'a DashboardRunFact>>,
    previews: HashMap<Uuid, AgentRunPreviewRecord>,
}

impl<'a> Builder<'a> {
    fn selected_runs(&self, session_id: Uuid) -> Vec<&'a DashboardRunFact> {
        let Some(runs) = self.session_runs.get(&session_id) else {
            return vec![];
        };
        let active: Vec<_> = runs
            .iter()
            .copied()
            .filter(|r| is_active(&r.status))
            .collect();
        if active.is_empty() {
            runs.first().copied().into_iter().collect()
        } else {
            active
        }
    }

    // Keep each independently validated runtime identity explicit at call sites.
    #[allow(clippy::too_many_arguments)]
    fn child(
        &self,
        parent: &DashboardWorkRef,
        id: String,
        role: &str,
        title: String,
        session_id: Option<Uuid>,
        run_id: Option<Uuid>,
        workflow_run_id: Option<Uuid>,
        node_execution_id: Option<Uuid>,
        raw_status: &str,
        updated_at: DateTime<Utc>,
        exact: bool,
    ) -> DashboardChildRow {
        let session = session_id.and_then(|id| self.sessions.get(&id).copied());
        let run = run_id
            .and_then(|id| self.runs.get(&id).copied())
            .filter(|r| session_id == Some(r.session_id))
            .or_else(|| {
                if exact {
                    None
                } else {
                    session_id.and_then(|id| self.selected_runs(id).first().copied())
                }
            });
        let target = session.map(|s| DashboardOpenTarget::Session {
            workspace_id: s.workspace_id,
            session_id: s.id,
            agent_run_id: run.map(|r| r.id),
            workflow_run_id,
            node_execution_id,
            workflow_node_id: None,
        });
        let (preview, activity, preview_status) = match (run, session, target.as_ref()) {
            (Some(run), Some(session), Some(target)) => preview_for(
                self.previews.get(&run.id),
                run,
                session,
                role,
                &title,
                target,
            ),
            _ => (None, None, "missing".into()),
        };
        // A Condition may finish its AgentRun and then wait for a human branch
        // choice. Its Node lifecycle remains authoritative for that wait.
        let status = if node_execution_id.is_some()
            && status_group(raw_status) == DashboardStatusGroup::Waiting
        {
            raw_status
        } else {
            run.map(|r| r.status.as_str()).unwrap_or(raw_status)
        };
        let mut issues = vec![];
        if session_id.is_some() && session.is_none() {
            issues.push(issue(
                "session_missing",
                "The bound session no longer exists",
                Some(parent.clone()),
            ));
        }
        if run_id.is_some() && run.is_none() {
            issues.push(issue(
                "run_binding_missing",
                "The exact bound AgentRun is missing or belongs to another session",
                Some(parent.clone()),
            ));
        }
        if let Some(run) = run
            && run.projection_status != "current"
        {
            issues.push(issue(
                "projection_degraded",
                format!("Runtime projection: {}", run.projection_status),
                Some(parent.clone()),
            ));
        }
        let agent = run.map(agent).or_else(|| {
            session
                .and_then(|s| s.executor.as_ref())
                .map(|provider| DashboardAgent {
                    provider_id: provider.to_ascii_lowercase(),
                    runtime_profile_id: None,
                    model_id: None,
                    reasoning_id: None,
                    configuration_source: "unavailable".into(),
                })
        });
        DashboardChildRow {
            id,
            parent: parent.clone(),
            role: role.into(),
            title,
            workflow_run_id,
            node_execution_id,
            subtask_id: None,
            arena_candidate_id: None,
            session_id,
            agent_run_id: run.map(|r| r.id),
            raw_status: status.into(),
            agent_run_status: run.map(|r| r.status.clone()),
            status_group: status_group(status),
            agent,
            directory: directory(session, run),
            preview,
            preview_status,
            activity,
            open_target: target,
            reused: false,
            matches_filter: true,
            issues,
            updated_at: run.map(|r| r.updated_at).unwrap_or(updated_at),
        }
    }
}

impl DashboardRead {
    pub async fn load(pool: &SqlitePool, force: bool) -> Result<Self, ApiError> {
        let facts = DashboardFacts::load(pool).await?;
        let run_ids: Vec<_> = facts.runs.iter().map(|r| r.id).collect();
        let mut issues = vec![];
        let previews = match AgentRunPreviewRecord::find_for_runs(pool, &run_ids).await {
            Ok(records) => records.into_iter().map(|r| (r.agent_run_id, r)).collect(),
            Err(error) => {
                tracing::warn!(%error,"Dashboard preview read failed");
                issues.push(issue(
                    "preview_unavailable",
                    "Reply previews are temporarily unavailable",
                    None,
                ));
                HashMap::new()
            }
        };
        let mut session_runs: HashMap<Uuid, Vec<&DashboardRunFact>> = HashMap::new();
        for run in &facts.runs {
            session_runs.entry(run.session_id).or_default().push(run);
        }
        let builder = Builder {
            sessions: facts.sessions.iter().map(|s| (s.id, s)).collect(),
            runs: facts.runs.iter().map(|r| (r.id, r)).collect(),
            session_runs,
            previews,
        };
        let behind: Vec<_> = facts
            .runs
            .iter()
            .filter(|run| {
                builder.previews.get(&run.id).is_none_or(|preview| {
                    preview.algorithm_version != AGENT_RUN_PREVIEW_VERSION
                        || preview.last_run_attempt_number != run.last_run_attempt_number
                        || preview.last_sequence != run.last_event_sequence
                        || preview.last_event_id != run.last_event_id
                })
            })
            .map(|run| run.id)
            .collect();
        services::services::agent_run_preview::prioritize_agent_run_previews(&behind);
        let execution_map: HashMap<_, _> = facts.executions.iter().map(|e| (e.id, e)).collect();
        let mut execution_roots: HashMap<Uuid, DashboardWorkRef> = HashMap::new();
        let mut works: HashMap<DashboardWorkRef, Work> = HashMap::new();
        for workflow in &facts.workflows {
            let work = DashboardWorkRef {
                kind: DashboardWorkKind::Workflow,
                id: workflow.id,
            };
            let mut row = empty_row(work.clone(), workflow.title.clone(), workflow.updated_at);
            row.project = workflow
                .project_id
                .map(|id| DashboardNamedRef { id, title: None });
            row.task = workflow
                .task_id
                .map(|id| DashboardNamedRef { id, title: None });
            row.execution_id = Some(workflow.execution_id);
            row.workflow_run_id = workflow.latest_run_id;
            row.raw_status = workflow.status.clone();
            row.status_group = status_group(&row.raw_status);
            row.open_target = Some(DashboardOpenTarget::Workflow {
                attempt_id: workflow.id,
                workflow_id: workflow.workflow_id,
                workflow_run_id: workflow.latest_run_id,
                project_id: workflow.project_id,
            });
            execution_roots.insert(workflow.execution_id, work.clone());
            works.insert(
                work,
                Work {
                    row,
                    children: vec![],
                },
            );
        }
        for execution in facts
            .executions
            .iter()
            .filter(|e| e.parent_task_id.is_none() && e.execution_kind == ExecutionKind::Agent)
        {
            let work = DashboardWorkRef {
                kind: DashboardWorkKind::Task,
                id: execution.id,
            };
            let mut row = empty_row(work.clone(), execution.title.clone(), execution.updated_at);
            row.project = Some(DashboardNamedRef {
                id: execution.project_id,
                title: None,
            });
            row.task = Some(DashboardNamedRef {
                id: execution.issue_id,
                title: None,
            });
            row.execution_id = Some(execution.id);
            execution_roots.insert(execution.id, work.clone());
            works.insert(
                work,
                Work {
                    row,
                    children: vec![],
                },
            );
        }
        // Resolve parents through explicit Execution links, including nested Arena.
        for _ in 0..facts.executions.len().max(1) {
            let mut changed = false;
            for e in &facts.executions {
                if !execution_roots.contains_key(&e.id)
                    && let Some(root) = e
                        .parent_task_id
                        .and_then(|id| execution_roots.get(&id))
                        .cloned()
                {
                    execution_roots.insert(e.id, root);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let mut arena_roots = HashMap::new();
        for group in &facts.arenas {
            let root = execution_roots.get(&group.task_id).cloned().or_else(|| {
                facts
                    .nodes
                    .iter()
                    .find(|n| n.arena_group_id == Some(group.id))
                    .and_then(|n| n.attempt_id)
                    .map(|id| DashboardWorkRef {
                        kind: DashboardWorkKind::Workflow,
                        id,
                    })
            });
            let root = root.unwrap_or(DashboardWorkRef {
                kind: DashboardWorkKind::Arena,
                id: group.id,
            });
            if root.kind == DashboardWorkKind::Arena {
                let execution = execution_map.get(&group.task_id);
                let mut row = empty_row(
                    root.clone(),
                    execution
                        .map(|e| e.title.clone())
                        .unwrap_or_else(|| "Arena".into()),
                    group.updated_at,
                );
                row.execution_id = Some(group.task_id);
                row.project = execution.map(|e| DashboardNamedRef {
                    id: e.project_id,
                    title: None,
                });
                row.task = execution.map(|e| DashboardNamedRef {
                    id: e.issue_id,
                    title: None,
                });
                row.raw_status = serde_json::to_value(group.lifecycle_status)
                    .unwrap_or_default()
                    .as_str()
                    .unwrap_or("unknown")
                    .into();
                row.status_group = status_group(&row.raw_status);
                row.open_target = Some(DashboardOpenTarget::Arena {
                    arena_group_id: group.id,
                    project_id: row.project.as_ref().map(|p| p.id),
                });
                works.insert(
                    root.clone(),
                    Work {
                        row,
                        children: vec![],
                    },
                );
            }
            execution_roots.insert(group.task_id, root.clone());
            arena_roots.insert(group.id, root);
        }
        for _ in 0..facts.executions.len().max(1) {
            let mut changed = false;
            for execution in &facts.executions {
                if !execution_roots.contains_key(&execution.id)
                    && let Some(root) = execution
                        .parent_task_id
                        .and_then(|id| execution_roots.get(&id))
                        .cloned()
                {
                    execution_roots.insert(execution.id, root);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        // A Session can have multiple references within one work; ownership
        // conflicts across works are visible rather than resolved by guessing.
        let mut owners: HashMap<Uuid, HashSet<DashboardWorkRef>> = HashMap::new();
        for workflow in &facts.workflows {
            let work = DashboardWorkRef {
                kind: DashboardWorkKind::Workflow,
                id: workflow.id,
            };
            if let Some(session_id) = workflow.main_session_id {
                // Main-Agent handovers bind the accepted run explicitly. A
                // main session's unrelated old response never fills this Run.
                add_child(
                    &mut works,
                    &mut owners,
                    &work,
                    builder.child(
                        &work,
                        format!("main:{session_id}"),
                        "main_agent",
                        "Main Agent".into(),
                        Some(session_id),
                        workflow.main_agent_run_id,
                        workflow.latest_run_id,
                        None,
                        "not_started",
                        workflow.updated_at,
                        true,
                    ),
                );
            }
        }
        let split_nodes: HashSet<_> = facts
            .subtasks
            .iter()
            .filter_map(|s| s.node_execution_id)
            .collect();
        // Latest iteration per Node, plus old still-active runs. Reuse remains
        // an explicit history reference and contributes no current run counts.
        let mut latest_nodes: HashMap<(Uuid, String), &db::models::dashboard::DashboardNodeFact> =
            HashMap::new();
        for node in &facts.nodes {
            let key = (node.run_id, node.node_id.clone());
            if latest_nodes.get(&key).is_none_or(|old| {
                (node.iteration, node.updated_at) > (old.iteration, old.updated_at)
            }) {
                latest_nodes.insert(key, node);
            }
        }
        let mut node_rows: Vec<_> = latest_nodes.values().copied().collect();
        for node in &facts.nodes {
            if node
                .agent_run_id
                .and_then(|id| builder.runs.get(&id))
                .is_some_and(|run| is_active(&run.status))
                && !node_rows
                    .iter()
                    .any(|n| n.id == node.id && n.run_id == node.run_id)
            {
                node_rows.push(node);
            }
        }
        for node in node_rows {
            let Some(attempt_id) = node.attempt_id else {
                continue;
            };
            let work = DashboardWorkRef {
                kind: DashboardWorkKind::Workflow,
                id: attempt_id,
            };
            if split_nodes.contains(&node.id) {
                continue;
            }
            let current = facts
                .workflows
                .iter()
                .find(|w| w.id == attempt_id)
                .is_some_and(|w| w.latest_run_id == Some(node.run_id));
            let role = if current { "node" } else { "other_active_run" };
            let mut child = builder.child(
                &work,
                format!("node:{}:{}", node.run_id, node.id),
                role,
                node.title.clone().unwrap_or_else(|| node.node_id.clone()),
                node.session_id,
                node.agent_run_id,
                Some(node.run_id),
                Some(node.id),
                &node.status,
                node.updated_at,
                true,
            );
            if child.agent_run_id.is_none()
                && let Ok(status) =
                    serde_json::from_value(serde_json::Value::String(node.status.clone()))
            {
                let status = crate::routes::workflows::workflow_node_status_from_binding(
                    status,
                    &node.node_type,
                    node.agent_run_id,
                );
                child.raw_status = serde_json::to_value(status)
                    .unwrap_or_default()
                    .as_str()
                    .unwrap_or(&node.status)
                    .into();
                child.status_group = status_group(&child.raw_status);
            }
            child.reused = node.reused;
            if node.reused {
                child.preview = None;
                child.preview_status = "historical".into();
                child.raw_status = "reused".into();
                child.status_group = DashboardStatusGroup::Finished;
            }
            set_node_target(&mut child, &node.node_id);
            if child.open_target.is_none() {
                child.open_target = works.get(&work).and_then(|w| w.row.open_target.clone());
            }
            add_child(&mut works, &mut owners, &work, child);
        }
        for subtask in &facts.subtasks {
            let Some(id) = subtask.attempt_id else {
                continue;
            };
            let work = DashboardWorkRef {
                kind: DashboardWorkKind::Workflow,
                id,
            };
            let mut child = builder.child(
                &work,
                format!("subtask:{}", subtask.id),
                if works
                    .get(&work)
                    .is_some_and(|w| w.row.workflow_run_id == Some(subtask.run_id))
                {
                    "subtask"
                } else {
                    "other_active_run"
                },
                subtask.title.clone(),
                subtask.session_id,
                subtask.agent_run_id,
                Some(subtask.run_id),
                subtask.node_execution_id,
                &subtask.status,
                subtask.updated_at,
                true,
            );
            child.subtask_id = Some(subtask.id);
            if let Some(node) = subtask
                .node_execution_id
                .and_then(|id| facts.nodes.iter().find(|n| n.id == id))
            {
                set_node_target(&mut child, &node.node_id);
            }
            add_child(&mut works, &mut owners, &work, child);
        }
        for candidate in &facts.candidates {
            let Some(root) = arena_roots.get(&candidate.arena_group_id) else {
                continue;
            };
            let bound_node = facts
                .nodes
                .iter()
                .filter(|n| !n.reused && n.arena_group_id == Some(candidate.arena_group_id))
                .max_by_key(|n| {
                    (
                        facts
                            .workflows
                            .iter()
                            .find(|w| w.id == root.id)
                            .is_some_and(|w| w.latest_run_id == Some(n.run_id)),
                        n.iteration,
                        n.updated_at,
                    )
                });
            let workflow_run_id = bound_node.map(|n| n.run_id);
            let linked: Vec<_> = facts
                .arena_runs
                .iter()
                .filter(|r| r.candidate_id == candidate.id)
                .collect();
            for identity in &linked {
                let title = builder
                    .sessions
                    .get(&identity.session_id)
                    .and_then(|s| s.name.clone())
                    .unwrap_or_else(|| format!("Candidate {}", candidate.sort_order + 1));
                let mut child = builder.child(
                    root,
                    format!("candidate:{}:{}", candidate.id, identity.agent_run_id),
                    if identity.latest
                        && (root.kind != DashboardWorkKind::Workflow
                            || works
                                .get(root)
                                .is_some_and(|w| w.row.workflow_run_id == workflow_run_id))
                    {
                        "arena_candidate"
                    } else {
                        "other_active_run"
                    },
                    title,
                    Some(identity.session_id),
                    Some(identity.agent_run_id),
                    if identity.latest {
                        workflow_run_id
                    } else {
                        None
                    },
                    bound_node.map(|n| n.id),
                    "not_started",
                    candidate.updated_at,
                    true,
                );
                child.arena_candidate_id = Some(candidate.id);
                if let Some(node) = bound_node {
                    set_node_target(&mut child, &node.node_id);
                }
                add_child(&mut works, &mut owners, root, child);
            }
            if linked.is_empty() {
                let mut child = builder.child(
                    root,
                    format!("candidate:{}", candidate.id),
                    "arena_candidate",
                    format!("Candidate {}", candidate.sort_order + 1),
                    None,
                    None,
                    workflow_run_id,
                    bound_node.map(|n| n.id),
                    "not_started",
                    candidate.updated_at,
                    true,
                );
                child.arena_candidate_id = Some(candidate.id);
                child.issues.push(issue(
                    "candidate_run_missing",
                    "Candidate has no canonical orchestration AgentRun binding",
                    Some(root.clone()),
                ));
                add_child(&mut works, &mut owners, root, child);
            }
        }
        for session in &facts.sessions {
            if let Some(root) = session.execution_id.and_then(|id| execution_roots.get(&id)) {
                if owners
                    .get(&session.id)
                    .is_some_and(|set| set.contains(root))
                {
                    continue;
                }
                let selected = builder.selected_runs(session.id);
                for run in selected
                    .iter()
                    .copied()
                    .map(Some)
                    .chain(if selected.is_empty() {
                        Some(None)
                    } else {
                        None
                    })
                {
                    add_child(
                        &mut works,
                        &mut owners,
                        root,
                        builder.child(
                            root,
                            format!(
                                "session:{}:{}",
                                session.id,
                                run.map(|r| r.id).unwrap_or(Uuid::nil())
                            ),
                            "session",
                            session.name.clone().unwrap_or_else(|| "Session".into()),
                            Some(session.id),
                            run.map(|r| r.id),
                            None,
                            None,
                            "not_started",
                            session.updated_at,
                            true,
                        ),
                    );
                }
            }
        }
        for session in &facts.sessions {
            let ownership = owners.get(&session.id);
            let conflict = ownership.is_some_and(|o| o.len() > 1);
            if ownership.is_some() && !conflict {
                continue;
            }
            let unbound_broken = session.execution_id.is_some() || conflict;
            let work = DashboardWorkRef {
                kind: if unbound_broken {
                    DashboardWorkKind::Unresolved
                } else {
                    DashboardWorkKind::Session
                },
                id: session.id,
            };
            let mut row = empty_row(
                work.clone(),
                session
                    .name
                    .clone()
                    .unwrap_or_else(|| "Untitled session".into()),
                session.updated_at,
            );
            row.project = session
                .project_id
                .map(|id| DashboardNamedRef { id, title: None });
            if conflict {
                row.issues.push(issue(
                    "ownership_conflict",
                    format!(
                        "Session is referenced by {} works",
                        ownership.map(HashSet::len).unwrap_or(0)
                    ),
                    Some(work.clone()),
                ));
            } else if unbound_broken {
                row.issues.push(issue(
                    "ownership_missing",
                    "The execution owner cannot be resolved",
                    Some(work.clone()),
                ));
            }
            let selected = builder.selected_runs(session.id);
            let mut children = vec![];
            for run in selected
                .iter()
                .copied()
                .map(Some)
                .chain(if selected.is_empty() {
                    Some(None)
                } else {
                    None
                })
            {
                children.push(builder.child(
                    &work,
                    format!(
                        "session:{}:{}",
                        session.id,
                        run.map(|r| r.id).unwrap_or(Uuid::nil())
                    ),
                    "session",
                    row.title.clone(),
                    Some(session.id),
                    run.map(|r| r.id),
                    None,
                    None,
                    "not_started",
                    session.updated_at,
                    true,
                ));
            }
            works.insert(work, Work { row, children });
        }
        // Surface every concurrent canonical Run, even if a Node's exact
        // binding points at another run of that same Session.
        for work in works.values_mut() {
            let owned: HashSet<_> = work
                .children
                .iter()
                .filter(|c| !c.reused)
                .filter_map(|c| c.session_id)
                .collect();
            let bound: HashSet<_> = work
                .children
                .iter()
                .filter_map(|c| c.agent_run_id)
                .collect();
            for id in owned {
                let active: Vec<_> = builder
                    .selected_runs(id)
                    .into_iter()
                    .filter(|r| is_active(&r.status))
                    .collect();
                if active.len() > 1 {
                    work.row.issues.push(issue(
                        "multiple_active_runs",
                        "This session has multiple nonterminal AgentRuns",
                        Some(work.row.work.clone()),
                    ));
                }
                for run in active.into_iter().filter(|r| !bound.contains(&r.id)) {
                    work.children.push(builder.child(
                        &work.row.work,
                        format!("extra:{}", run.id),
                        "other_active_run",
                        "Other active run".into(),
                        Some(id),
                        Some(run.id),
                        None,
                        None,
                        &run.status,
                        run.updated_at,
                        true,
                    ));
                }
            }
            summarize(work, &builder);
            work.children.sort_by(|a, b| {
                let role_rank = |role: &str| match role {
                    "main_agent" => 0,
                    "node" => 1,
                    "subtask" => 2,
                    "arena_candidate" => 3,
                    "session" => 4,
                    _ => 5,
                };
                role_rank(&a.role)
                    .cmp(&role_rank(&b.role))
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
        let project_ids: HashSet<_> = works
            .values()
            .filter_map(|w| w.row.project.as_ref().map(|p| p.id))
            .collect();
        for project_id in project_ids {
            if project_id == DEFAULT_PROJECT_ID {
                for work in works
                    .values_mut()
                    .filter(|w| w.row.project.as_ref().is_some_and(|p| p.id == project_id))
                {
                    if let Some(project) = &mut work.row.project {
                        project.title = Some("Default project".into());
                    }
                }
                continue;
            }
            match project_store::read_snapshot(pool, project_id, force).await {
                Ok(document) => {
                    for work in works
                        .values_mut()
                        .filter(|w| w.row.project.as_ref().is_some_and(|p| p.id == project_id))
                    {
                        if let Some(project) = &mut work.row.project {
                            project.title = Some(document.project.name.clone());
                        }
                        if let Some(task) = &mut work.row.task {
                            task.title = document
                                .tasks
                                .iter()
                                .find(|t| t.id == task.id)
                                .map(|t| t.title.clone());
                            if task.title.is_none() {
                                work.row.issues.push(issue("task_metadata_missing","The business Task is absent from authoritative project metadata",Some(work.row.work.clone())));
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::debug!(%project_id,%error,"Dashboard project metadata unavailable");
                    for work in works
                        .values_mut()
                        .filter(|w| w.row.project.as_ref().is_some_and(|p| p.id == project_id))
                    {
                        work.row.issues.push(issue(
                            "project_metadata_unreadable",
                            "Project metadata is missing or unreadable",
                            Some(work.row.work.clone()),
                        ));
                    }
                }
            }
        }
        let works = works.into_values().collect();
        Ok(Self {
            facts,
            works,
            issues,
        })
    }
}

fn summarize(work: &mut Work, builder: &Builder<'_>) {
    let current = work.row.workflow_run_id;
    let mut active_sessions = HashSet::new();
    let mut active_runs = HashSet::new();
    let mut waiting = HashSet::new();
    let mut agents = HashSet::new();
    for child in &work.children {
        if !child.reused {
            if let Some(run) = child.agent_run_id.and_then(|id| builder.runs.get(&id))
                && is_active(&run.status)
            {
                active_sessions.insert(run.session_id);
                active_runs.insert(run.id);
            }
            if child.status_group == DashboardStatusGroup::Waiting {
                waiting.insert(child.id.clone());
            }
            let is_current = current.is_none() || child.workflow_run_id == current;
            if is_current
                && child.preview.as_ref().is_some_and(|p| {
                    work.row
                        .preview
                        .as_ref()
                        .is_none_or(|old| p.occurred_at > old.occurred_at)
                })
            {
                work.row.preview = child.preview.clone();
            }
            if is_current
                && child.activity.as_ref().is_some_and(|a| {
                    work.row
                        .activity
                        .as_ref()
                        .is_none_or(|old| a.occurred_at > old.occurred_at)
                })
            {
                work.row.activity = child.activity.clone();
            }
        }
        if let Some(agent) = &child.agent
            && agents.insert((
                agent.provider_id.clone(),
                agent.runtime_profile_id.clone(),
                agent.model_id.clone(),
                agent.reasoning_id.clone(),
            ))
        {
            work.row.agents.push(agent.clone());
        }
    }
    work.row.active_session_count = active_sessions.len() as u32;
    work.row.active_run_count = active_runs.len() as u32;
    work.row.waiting_count = waiting.len() as u32;
    work.row.matching_session_count = work
        .children
        .iter()
        .filter_map(|c| c.session_id)
        .collect::<HashSet<_>>()
        .len() as u32;
    work.row.child_count = work.children.len() as u32;
    if let Some(child) = work.children.first() {
        work.row.directory = child.directory.clone();
    }
    if matches!(
        work.row.work.kind,
        DashboardWorkKind::Task | DashboardWorkKind::Session | DashboardWorkKind::Unresolved
    ) {
        if let Some(child) = work.children.iter().min_by_key(|c| match c.status_group {
            DashboardStatusGroup::Waiting => 0,
            DashboardStatusGroup::Running => 1,
            _ => 2,
        }) {
            work.row.raw_status = child.raw_status.clone();
            work.row.status_group = child.status_group;
            work.row.open_target = child.open_target.clone();
            work.row.updated_at = child.updated_at;
        } else {
            work.row.issues.push(issue(
                "session_binding_missing",
                "The Agent Execution has no readable Session binding",
                Some(work.row.work.clone()),
            ));
        }
    } else if work.row.work.kind == DashboardWorkKind::Arena && work.row.raw_status == "open" {
        work.row.status_group = if work.row.waiting_count > 0 {
            DashboardStatusGroup::Waiting
        } else if work.row.active_session_count > 0 {
            DashboardStatusGroup::Running
        } else {
            DashboardStatusGroup::Waiting
        };
    }
    // A Human Gate can wait without any Agent Session.
    if work.row.status_group == DashboardStatusGroup::Waiting && work.row.waiting_count == 0 {
        work.row.waiting_count = 1;
    }
    let relevant: Vec<_> = work
        .children
        .iter()
        .filter(|c| {
            !c.reused
                && (current.is_none() || c.workflow_run_id == current)
                && c.agent_run_id.is_some()
        })
        .collect();
    work.row.preview_status = if relevant.iter().any(|c| c.preview_status == "rebuilding") {
        "rebuilding"
    } else if relevant.iter().any(|c| c.preview_status == "catching_up") {
        "catching_up"
    } else if relevant.iter().any(|c| c.preview_status == "missing") {
        "missing"
    } else {
        "ready"
    }
    .into();
    if work.row.active_session_count > 0 && work.row.status_group == DashboardStatusGroup::Finished
    {
        work.row.issues.push(issue(
            "other_run_active",
            "Other AgentRuns remain active; the parent runtime outcome is preserved",
            Some(work.row.work.clone()),
        ));
    }
}

fn child_matches(child: &DashboardChildRow, filters: &DashboardFilters) -> bool {
    filters.provider_id.as_ref().is_none_or(|p| {
        child
            .agent
            .as_ref()
            .is_some_and(|a| a.provider_id.eq_ignore_ascii_case(p))
    }) && filters
        .directory_kind
        .as_ref()
        .is_none_or(|kind| &child.directory.kind == kind)
}

fn base_matches(work: &Work, filters: &DashboardFilters, now: DateTime<Utc>) -> bool {
    if !filters.kinds.is_empty() && !filters.kinds.contains(&work.row.work.kind) {
        return false;
    }
    if filters
        .project_id
        .is_some_and(|id| work.row.project.as_ref().is_none_or(|p| p.id != id))
    {
        return false;
    }
    if (filters.provider_id.is_some() || filters.directory_kind.is_some())
        && !work.children.iter().any(|c| child_matches(c, filters))
    {
        return false;
    }
    let cutoff = now - Duration::hours(i64::from(filters.history_hours.unwrap_or(24)));
    if work.row.active_session_count == 0
        && !is_active(&work.row.raw_status)
        && work.row.status_group != DashboardStatusGroup::Waiting
        && work.row.updated_at < cutoff
    {
        return false;
    }
    if !filters.search.trim().is_empty() {
        let search = filters.search.trim().to_lowercase();
        let fields = format!(
            "{} {} {} {} {} {}",
            work.row.title,
            work.row
                .project
                .as_ref()
                .and_then(|p| p.title.as_deref())
                .unwrap_or(""),
            work.row
                .task
                .as_ref()
                .and_then(|p| p.title.as_deref())
                .unwrap_or(""),
            work.row.directory.path.as_deref().unwrap_or(""),
            work.row
                .agents
                .iter()
                .map(|a| a.provider_id.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            work.children
                .iter()
                .map(|c| format!("{} {}", c.title, c.directory.path.as_deref().unwrap_or("")))
                .collect::<Vec<_>>()
                .join(" ")
        );
        if !fields.to_lowercase().contains(&search) {
            return false;
        }
    }
    true
}

fn status_matches(work: &Work, status: DashboardStatusFilter) -> bool {
    match status {
        DashboardStatusFilter::Current => {
            work.row.active_session_count > 0
                || matches!(
                    work.row.status_group,
                    DashboardStatusGroup::Waiting | DashboardStatusGroup::Running
                )
        }
        DashboardStatusFilter::Waiting => {
            work.row.waiting_count > 0 || work.row.status_group == DashboardStatusGroup::Waiting
        }
        DashboardStatusFilter::Running => {
            work.row.active_session_count > 0
                || work.row.status_group == DashboardStatusGroup::Running
        }
        DashboardStatusFilter::Failed => work.row.status_group == DashboardStatusGroup::Failed,
        DashboardStatusFilter::Finished => work.row.status_group == DashboardStatusGroup::Finished,
        DashboardStatusFilter::All => true,
    }
}

pub(super) fn data_scope(pool: &SqlitePool) -> String {
    let options = pool.connect_options();
    format!(
        "{}:{:p}",
        options.get_filename().display(),
        std::sync::Arc::as_ptr(&options)
    )
}

pub(super) fn validate(query: &DashboardQuery) -> Result<(), ApiError> {
    if query.filters.search.chars().count() > 256
        || query.expanded.len() > 20
        || query.visible.len() > 100
        || query.page > 100_000
        || query.page_size.is_some_and(|s| s == 0 || s > 100)
        || query.filters.kinds.len() > 5
        || query
            .filters
            .provider_id
            .as_ref()
            .is_some_and(|p| p.len() > 128)
        || query.expanded.iter().any(|p| p.page > 100_000)
        || query
            .filters
            .history_hours
            .is_some_and(|h| ![24, 168, 720].contains(&h))
        || query
            .filters
            .directory_kind
            .as_ref()
            .is_some_and(|k| !["direct_folder", "worktree", "unknown"].contains(&k.as_str()))
    {
        return Err(ApiError::BadRequest(
            "Invalid or excessive Dashboard query".into(),
        ));
    }
    Ok(())
}

pub(super) fn child_page(
    scope: &str,
    work: Option<&Work>,
    request: &DashboardExpandedPage,
    filters: &DashboardFilters,
) -> Result<DashboardChildrenPage, ApiError> {
    let Some(work) = work else {
        return Ok(DashboardChildrenPage {
            work: request.work.clone(),
            workflow_run_id: None,
            snapshot_id: None,
            page: request.page,
            page_size: 20,
            total: 0,
            rows: vec![],
            has_updates: true,
            invalidation: Some("work_deleted".into()),
        });
    };
    if request.workflow_run_id.is_some() && request.workflow_run_id != work.row.workflow_run_id {
        return Ok(DashboardChildrenPage {
            work: request.work.clone(),
            workflow_run_id: work.row.workflow_run_id,
            snapshot_id: None,
            page: request.page,
            page_size: 20,
            total: 0,
            rows: vec![],
            has_updates: true,
            invalidation: Some("run_changed".into()),
        });
    }
    let desired: Vec<_> = work.children.iter().map(|c| c.id.clone()).collect();
    let fingerprint = format!(
        "children:{}:{:?}:{}",
        work_key(&request.work),
        work.row.workflow_run_id,
        serde_json::to_string(filters).unwrap_or_default()
    );
    let selected = snapshot::select(
        scope,
        fingerprint,
        request.snapshot_id,
        &desired,
        &[],
        request.page,
        20,
        filters,
    )
    .map_err(|message| ApiError::BadRequest(message.into()))?;
    let missing_children = selected
        .ids
        .iter()
        .any(|id| !work.children.iter().any(|child| &child.id == id));
    let rows = selected
        .ids
        .iter()
        .filter_map(|id| work.children.iter().find(|c| &c.id == id))
        .cloned()
        .map(|mut child| {
            child.matches_filter = child_matches(&child, filters);
            child
        })
        .collect();
    Ok(DashboardChildrenPage {
        work: request.work.clone(),
        workflow_run_id: work.row.workflow_run_id,
        snapshot_id: selected.snapshot_id,
        page: request.page,
        page_size: 20,
        total: selected.total as u32,
        rows,
        has_updates: selected.has_updates,
        invalidation: if missing_children {
            Some("child_deleted".into())
        } else {
            (selected.status != "ready").then(|| selected.status.into())
        },
    })
}

pub(super) async fn execute(
    pool: &SqlitePool,
    query: DashboardQuery,
) -> Result<DashboardPageResponse, ApiError> {
    validate(&query)?;
    let mut read = DashboardRead::load(pool, query.refresh).await?;
    let now = Utc::now();
    let scope = data_scope(pool);
    let filters = &query.filters;
    let page_size = query.page_size.unwrap_or(20);
    // Rows have immutable sort identities. Polls update values in place while
    // snapshots separately signal changed membership/order.
    read.works.sort_by(|a, b| {
        let group = if filters.group_by_project {
            a.row
                .project
                .as_ref()
                .map(|p| p.id)
                .cmp(&b.row.project.as_ref().map(|p| p.id))
        } else {
            std::cmp::Ordering::Equal
        };
        let rank = |w: &Work| {
            if w.row.waiting_count > 0 {
                0
            } else if w.row.active_session_count > 0
                || w.row.status_group == DashboardStatusGroup::Running
            {
                1
            } else {
                2
            }
        };
        group
            .then_with(|| rank(a).cmp(&rank(b)))
            .then_with(|| {
                b.row
                    .activity
                    .as_ref()
                    .map(|a| a.occurred_at)
                    .unwrap_or(b.row.updated_at)
                    .cmp(
                        &a.row
                            .activity
                            .as_ref()
                            .map(|a| a.occurred_at)
                            .unwrap_or(a.row.updated_at),
                    )
            })
            .then_with(|| work_key(&a.row.work).cmp(&work_key(&b.row.work)))
    });
    let base: Vec<_> = read
        .works
        .iter()
        .filter(|w| base_matches(w, filters, now))
        .collect();
    let mut counts = DashboardCounts {
        all: base.len() as u32,
        waiting: base
            .iter()
            .filter(|w| status_matches(w, DashboardStatusFilter::Waiting))
            .count() as u32,
        running: base
            .iter()
            .filter(|w| status_matches(w, DashboardStatusFilter::Running))
            .count() as u32,
        failed: base
            .iter()
            .filter(|w| status_matches(w, DashboardStatusFilter::Failed))
            .count() as u32,
        finished: base
            .iter()
            .filter(|w| status_matches(w, DashboardStatusFilter::Finished))
            .count() as u32,
        current: base
            .iter()
            .filter(|w| status_matches(w, DashboardStatusFilter::Current))
            .count() as u32,
        ..DashboardCounts::default()
    };
    let matched: Vec<_> = base
        .iter()
        .copied()
        .filter(|w| status_matches(w, filters.status))
        .collect();
    counts.work_count = matched.len() as u32;
    let active_runs: HashSet<_> = matched
        .iter()
        .flat_map(|w| &w.children)
        .filter(|c| {
            !c.reused
                && c.agent_run_status.as_deref().is_some_and(is_active)
                && child_matches(c, filters)
        })
        .filter_map(|c| c.agent_run_id)
        .collect();
    counts.active_run_count = active_runs.len() as u32;
    counts.active_session_count = read
        .facts
        .runs
        .iter()
        .filter(|r| active_runs.contains(&r.id))
        .map(|r| r.session_id)
        .collect::<HashSet<_>>()
        .len() as u32;
    let desired: Vec<_> = matched.iter().map(|w| work_key(&w.row.work)).collect();
    let visible: Vec<_> = query.visible.iter().map(work_key).collect();
    let fingerprint = format!(
        "root:{page_size}:{}",
        serde_json::to_string(filters).unwrap_or_default()
    );
    let selected = snapshot::select(
        &scope,
        fingerprint,
        if query.refresh {
            None
        } else {
            query.snapshot_id
        },
        &desired,
        &visible,
        query.page,
        page_size,
        filters,
    )
    .map_err(|message| ApiError::BadRequest(message.into()))?;
    let rows = selected
        .ids
        .iter()
        .map(|id| {
            if let Some(work) = read.works.iter().find(|w| work_key(&w.row.work) == *id) {
                let mut row = work.row.clone();
                row.stale_membership = !desired.contains(id);
                row.matching_session_count = work
                    .children
                    .iter()
                    .filter(|c| child_matches(c, filters))
                    .filter_map(|c| c.session_id)
                    .collect::<HashSet<_>>()
                    .len() as u32;
                row
            } else {
                let reference = query
                    .visible
                    .iter()
                    .find(|r| work_key(r) == *id)
                    .cloned()
                    .unwrap_or_else(|| parse_key(id));
                let mut row = empty_row(reference, "Unavailable work".into(), now);
                row.invalidated = true;
                row.issues.push(issue(
                    "work_deleted",
                    "Work no longer exists",
                    Some(row.work.clone()),
                ));
                row
            }
        })
        .collect();
    let expanded = query
        .expanded
        .iter()
        .map(|request| {
            child_page(
                &scope,
                read.works.iter().find(|w| w.row.work == request.work),
                request,
                filters,
            )
        })
        .collect::<Result<_, _>>()?;
    if selected.status != "ready" {
        read.issues.push(issue(
            &format!("snapshot_{}", selected.status),
            "Refresh the work list to obtain a new identity snapshot",
            None,
        ));
    }
    let mut projects: Vec<_> = read
        .works
        .iter()
        .filter_map(|w| w.row.project.clone())
        .collect();
    projects.sort_by_key(|p| p.id);
    projects.dedup_by_key(|p| p.id);
    let mut providers: Vec<_> = read
        .works
        .iter()
        .flat_map(|w| w.row.agents.iter().map(|a| a.provider_id.clone()))
        .collect();
    providers.sort();
    providers.dedup();
    Ok(DashboardPageResponse {
        generated_at: now,
        snapshot_id: selected.snapshot_id,
        snapshot_status: selected.status.into(),
        page: query.page,
        page_size,
        total: selected.total as u32,
        latest_total: desired.len() as u32,
        counts,
        rows,
        expanded,
        has_updates: selected.has_updates,
        issues: read.issues,
        projects,
        providers,
    })
}

fn parse_key(key: &str) -> DashboardWorkRef {
    let (kind, id) = key.split_once(':').unwrap_or(("unresolved", ""));
    DashboardWorkRef {
        kind: match kind {
            "task" => DashboardWorkKind::Task,
            "workflow" => DashboardWorkKind::Workflow,
            "session" => DashboardWorkKind::Session,
            "arena" => DashboardWorkKind::Arena,
            _ => DashboardWorkKind::Unresolved,
        },
        id: id.parse().unwrap_or(Uuid::nil()),
    }
}
