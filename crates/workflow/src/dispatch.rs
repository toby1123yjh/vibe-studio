//! Fixed-graph batch semantics. This module plans; it never starts processes.
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{
    files::{DispatchPlan, SplitTaskManifest, WORKFLOW_FILE_SCHEMA_VERSION, validate_manifest},
    graph::{WorkflowEdgeKind, WorkflowGraph, WorkflowNodeKind},
    planner::NodeExecutionStatus,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchScope {
    pub splitter_node_id: String,
    pub candidate_node_ids: Vec<String>,
    pub summary_node_id: String,
}

/// A splitter's immediate Agent children all join at one fixed exit. Reject
/// competing ordinary triggers so `each` and batch dispatch cannot both run it.
pub fn dispatch_scope(graph: &WorkflowGraph, splitter_id: &str) -> Result<DispatchScope, String> {
    if !graph
        .nodes
        .iter()
        .any(|n| n.id == splitter_id && n.kind == WorkflowNodeKind::TaskSplitter)
    {
        return Err(format!("`{splitter_id}` is not a task splitter"));
    }
    let outgoing: Vec<_> = graph
        .edges
        .iter()
        .filter(|e| e.source == splitter_id)
        .collect();
    if outgoing.is_empty() {
        return Err(format!(
            "Splitter `{splitter_id}` needs at least one executor node"
        ));
    }
    let mut candidate_node_ids = Vec::new();
    let mut summary_node_id: Option<String> = None;
    for edge in outgoing {
        if edge.kind != WorkflowEdgeKind::Default || candidate_node_ids.contains(&edge.target) {
            return Err(format!(
                "Splitter `{splitter_id}` has duplicate or non-default executor edges"
            ));
        }
        let candidate = graph
            .nodes
            .iter()
            .find(|n| n.id == edge.target)
            .ok_or_else(|| format!("Missing executor `{}`", edge.target))?;
        if candidate.kind != WorkflowNodeKind::Agent {
            return Err(format!(
                "Splitter executor `{}` must be an Agent node",
                candidate.id
            ));
        }
        if graph
            .edges
            .iter()
            .filter(|e| e.target == candidate.id)
            .count()
            != 1
        {
            return Err(format!(
                "Splitter executor `{}` has competing incoming triggers",
                candidate.id
            ));
        }
        let exits: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.source == candidate.id)
            .collect();
        if exits.len() != 1 || exits[0].kind != WorkflowEdgeKind::Default {
            return Err(format!(
                "Splitter executor `{}` must connect to exactly one summary exit",
                candidate.id
            ));
        }
        let exit = &exits[0].target;
        if summary_node_id
            .as_ref()
            .is_some_and(|current| current != exit)
        {
            return Err(format!(
                "Splitter `{splitter_id}` executor nodes must share one summary exit"
            ));
        }
        summary_node_id = Some(exit.clone());
        candidate_node_ids.push(candidate.id.clone());
    }
    let summary_node_id = summary_node_id.expect("nonempty candidates have an exit");
    let summary = graph
        .nodes
        .iter()
        .find(|n| n.id == summary_node_id)
        .ok_or_else(|| format!("Missing summary exit `{summary_node_id}`"))?;
    if !matches!(
        summary.kind,
        WorkflowNodeKind::Agent | WorkflowNodeKind::Transform | WorkflowNodeKind::End
    ) || candidate_node_ids.contains(&summary_node_id)
    {
        return Err(format!(
            "Splitter summary `{summary_node_id}` must be a separate Agent, Transform or End"
        ));
    }
    if graph
        .edges
        .iter()
        .filter(|e| e.target == summary_node_id)
        .any(|e| !candidate_node_ids.contains(&e.source) || e.kind != WorkflowEdgeKind::Default)
    {
        return Err(format!(
            "Splitter summary `{summary_node_id}` has competing incoming triggers"
        ));
    }
    Ok(DispatchScope {
        splitter_node_id: splitter_id.to_owned(),
        candidate_node_ids,
        summary_node_id,
    })
}

pub fn dispatch_scopes(graph: &WorkflowGraph) -> Result<Vec<DispatchScope>, String> {
    let scopes = graph
        .nodes
        .iter()
        .filter(|n| n.kind == WorkflowNodeKind::TaskSplitter)
        .map(|n| dispatch_scope(graph, &n.id))
        .collect::<Result<Vec<_>, _>>()?;
    let mut owners = HashSet::new();
    for scope in &scopes {
        for id in scope
            .candidate_node_ids
            .iter()
            .chain(std::iter::once(&scope.summary_node_id))
        {
            if !owners.insert(id) {
                return Err(format!("Node `{id}` belongs to overlapping split batches"));
            }
        }
    }
    Ok(scopes)
}

pub fn batch_managed_nodes(graph: &WorkflowGraph) -> HashSet<String> {
    dispatch_scopes(graph)
        .unwrap_or_default()
        .into_iter()
        .flat_map(|scope| {
            scope
                .candidate_node_ids
                .into_iter()
                .chain(std::iter::once(scope.summary_node_id))
        })
        .collect()
}

pub fn validate_dispatch(
    scope: &DispatchScope,
    manifest: &SplitTaskManifest,
    plan: &DispatchPlan,
) -> Result<(), String> {
    validate_manifest(manifest)?;
    if plan.schema_version != WORKFLOW_FILE_SCHEMA_VERSION
        || plan.run_id != manifest.run_id
        || plan.split_execution_id != manifest.split_execution_id
    {
        return Err(
            "Dispatch schema or source identity does not match the accepted split manifest".into(),
        );
    }
    if plan.manifest_sha256.len() != 64
        || !plan.manifest_sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Dispatch manifest_sha256 must identify the accepted manifest bytes".into());
    }
    let known: HashSet<_> = manifest
        .tasks
        .iter()
        .map(|t| t.subtask_id.as_str())
        .collect();
    let mut assigned = HashMap::new();
    for (stage_index, stage) in plan.stages.iter().enumerate() {
        if stage.assignments.is_empty() {
            return Err(format!("Dispatch stage {stage_index} is empty"));
        }
        for item in &stage.assignments {
            if !known.contains(item.subtask_id.as_str())
                || assigned
                    .insert(item.subtask_id.as_str(), stage_index)
                    .is_some()
            {
                return Err(format!("Unknown or repeated subtask `{}`", item.subtask_id));
            }
            if !scope.candidate_node_ids.contains(&item.node_id) {
                return Err(format!(
                    "Node `{}` is outside the frozen dispatch scope",
                    item.node_id
                ));
            }
        }
    }
    if assigned.len() != known.len() {
        return Err("Every split subtask must be assigned exactly once".into());
    }
    for task in &manifest.tasks {
        for dependency in &task.depends_on {
            if assigned[dependency.as_str()] >= assigned[task.subtask_id.as_str()] {
                return Err(format!(
                    "Subtask `{}` must run in a later stage than `{dependency}`",
                    task.subtask_id
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchSubtaskSnapshot {
    pub subtask_id: String,
    pub stage_index: i64,
    pub status: NodeExecutionStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchReadyPlan {
    pub ready_subtask_ids: Vec<String>,
    pub blocked_subtask_ids: Vec<String>,
    pub summary_ready: bool,
    pub has_live_executions: bool,
    pub failed: bool,
}

/// Serial stages, parallel items. A failure never cancels unrelated siblings;
/// the project remains occupied while any sibling can still run or is live.
pub fn plan_batch(items: &[BatchSubtaskSnapshot], stopping: bool) -> BatchReadyPlan {
    let mut result = BatchReadyPlan::default();
    result.has_live_executions = items.iter().any(|i| {
        matches!(
            i.status,
            NodeExecutionStatus::Running
                | NodeExecutionStatus::AwaitingHuman
                | NodeExecutionStatus::AwaitingArena
                | NodeExecutionStatus::Cancelling
        )
    });
    result.failed = items.iter().any(|i| {
        matches!(
            i.status,
            NodeExecutionStatus::Failed
                | NodeExecutionStatus::Cancelled
                | NodeExecutionStatus::Skipped
        )
    });
    if stopping || items.is_empty() {
        return result;
    }
    let first_incomplete = items
        .iter()
        .filter(|i| i.status != NodeExecutionStatus::Succeeded)
        .map(|i| i.stage_index)
        .min();
    let Some(stage) = first_incomplete else {
        result.summary_ready = true;
        return result;
    };
    for item in items
        .iter()
        .filter(|i| i.status == NodeExecutionStatus::Pending)
    {
        if item.stage_index == stage {
            result.ready_subtask_ids.push(item.subtask_id.clone());
        } else if result.failed {
            result.blocked_subtask_ids.push(item.subtask_id.clone());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diamond() -> WorkflowGraph {
        serde_json::from_value(serde_json::json!({"version":2,"nodes":[
            {"id":"start","type":"start","data":{}},{"id":"split","type":"task_splitter","data":{}},
            {"id":"a","type":"agent","data":{}},{"id":"b","type":"agent","data":{}},{"id":"end","type":"end","data":{}}
        ],"edges":[
            {"id":"1","source":"start","target":"split","type":"default"},
            {"id":"2","source":"split","target":"a","type":"default"},
            {"id":"3","source":"split","target":"b","type":"default"},
            {"id":"4","source":"a","target":"end","type":"default"},
            {"id":"5","source":"b","target":"end","type":"default"}
        ]})).unwrap()
    }

    #[test]
    fn frozen_scope_rejects_competing_triggers() {
        let mut graph = diamond();
        assert_eq!(
            dispatch_scope(&graph, "split").unwrap().candidate_node_ids,
            vec!["a", "b"]
        );
        let mut extra = graph.edges[0].clone();
        extra.target = "a".into();
        graph.edges.push(extra);
        assert!(
            dispatch_scope(&graph, "split")
                .unwrap_err()
                .contains("competing")
        );
    }

    #[test]
    fn batch_nodes_never_use_each_trigger_path() {
        use crate::planner::{
            NodeExecutionSnapshot, RunSnapshot, plan_ready_nodes, triggered_execution_count,
        };
        let graph = diamond();
        let snapshot = RunSnapshot {
            run_id: "r".into(),
            nodes: ["start", "split", "a"]
                .into_iter()
                .map(|id| NodeExecutionSnapshot {
                    node_id: id.into(),
                    iteration: 0,
                    status: NodeExecutionStatus::Succeeded,
                    output_text: None,
                    error_text: None,
                })
                .collect(),
        };
        assert!(plan_ready_nodes(&graph, &snapshot).ready_nodes.is_empty());
        assert_eq!(triggered_execution_count(&graph, &snapshot, "a"), 0);
        assert_eq!(triggered_execution_count(&graph, &snapshot, "end"), 0);
    }

    #[test]
    fn failure_keeps_parallel_sibling_ready_but_blocks_next_stage_and_summary() {
        let items = vec![
            BatchSubtaskSnapshot {
                subtask_id: "a".into(),
                stage_index: 0,
                status: NodeExecutionStatus::Failed,
            },
            BatchSubtaskSnapshot {
                subtask_id: "b".into(),
                stage_index: 0,
                status: NodeExecutionStatus::Pending,
            },
            BatchSubtaskSnapshot {
                subtask_id: "c".into(),
                stage_index: 1,
                status: NodeExecutionStatus::Pending,
            },
        ];
        let plan = plan_batch(&items, false);
        assert_eq!(plan.ready_subtask_ids, vec!["b"]);
        assert_eq!(plan.blocked_subtask_ids, vec!["c"]);
        assert!(!plan.summary_ready);
        assert!(plan_batch(&items, true).ready_subtask_ids.is_empty());
    }

    #[test]
    fn dependencies_require_an_earlier_stage_and_all_tasks_exactly_once() {
        use crate::files::*;
        let manifest = SplitTaskManifest {
            schema_version: 1,
            run_id: "r".into(),
            split_execution_id: "s".into(),
            tasks: vec![
                SplitTask {
                    subtask_id: "a".into(),
                    title: "A".into(),
                    instruction_path: "a.md".into(),
                    depends_on: vec![],
                },
                SplitTask {
                    subtask_id: "b".into(),
                    title: "B".into(),
                    instruction_path: "b.md".into(),
                    depends_on: vec!["a".into()],
                },
            ],
        };
        let scope = dispatch_scope(&diamond(), "split").unwrap();
        let mut plan = DispatchPlan {
            schema_version: 1,
            run_id: "r".into(),
            split_execution_id: "s".into(),
            manifest_sha256: "0".repeat(64),
            stages: vec![DispatchStage {
                assignments: vec![
                    DispatchAssignment {
                        subtask_id: "a".into(),
                        node_id: "a".into(),
                    },
                    DispatchAssignment {
                        subtask_id: "b".into(),
                        node_id: "a".into(),
                    },
                ],
            }],
        };
        assert!(
            validate_dispatch(&scope, &manifest, &plan)
                .unwrap_err()
                .contains("later stage")
        );
        let b = plan.stages[0].assignments.pop().unwrap();
        plan.stages.push(DispatchStage {
            assignments: vec![b],
        });
        validate_dispatch(&scope, &manifest, &plan).unwrap();
        plan.stages[1].assignments[0].node_id = "end".into();
        assert!(
            validate_dispatch(&scope, &manifest, &plan)
                .unwrap_err()
                .contains("frozen")
        );
    }
}
