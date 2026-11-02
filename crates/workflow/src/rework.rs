//! Pure planning for a new delivery. Reuse points to real prior executions;
//! neither successful executions nor historical file evidence are copied.
use std::collections::{BTreeSet, HashSet};

use crate::{WorkflowGraph, WorkflowNodeKind, planner::NodeExecutionStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReworkSourceExecution {
    pub execution_id: String,
    pub node_id: String,
    pub iteration: i64,
    pub status: NodeExecutionStatus,
    pub output_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReuseBinding {
    pub node_id: String,
    pub iteration: i64,
    pub source_execution_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReworkPlan {
    pub affected_node_ids: BTreeSet<String>,
    pub reuse: Vec<ReuseBinding>,
    pub skipped_node_ids: BTreeSet<String>,
}

/// None means all nodes. Explicit roots always include their potentially
/// affected downstream closure, but actual dispatch still uses the runner's
/// recorded Condition routes and complete effective inputs for each round.
pub fn plan_rework(
    graph: &WorkflowGraph,
    roots: Option<&[String]>,
    source: &[ReworkSourceExecution],
) -> Result<ReworkPlan, String> {
    let all: BTreeSet<_> = graph.nodes.iter().map(|node| node.id.clone()).collect();
    let affected = if let Some(roots) = roots {
        if roots.is_empty() || roots.iter().collect::<HashSet<_>>().len() != roots.len() {
            return Err("Partial rework requires distinct, non-empty Node IDs".into());
        }
        if let Some(id) = roots.iter().find(|id| !all.contains(*id)) {
            return Err(format!("Node `{id}` is not in the frozen workflow"));
        }
        downstream_closure(graph, roots.iter().cloned().collect())
    } else {
        all.clone()
    };
    let permitted = permitted_nodes(graph, &affected, source);
    if let Some(root) = roots.and_then(|roots| roots.iter().find(|id| !permitted.contains(*id))) {
        return Err(format!(
            "Node `{root}` is outside the recorded Condition route; include an earlier Condition in the rework scope"
        ));
    }

    let mut reuse = Vec::new();
    let mut skipped = BTreeSet::new();
    for id in all.difference(&affected) {
        // Only an untouched Condition's recorded route can exclude an input.
        // Failure/cancellation also skips pending nodes, so a Skipped status
        // alone is not evidence that an otherwise permitted input is optional.
        if !permitted.contains(id) {
            skipped.insert(id.clone());
            continue;
        }
        let records: Vec<_> = source
            .iter()
            .filter(|record| &record.node_id == id)
            .collect();
        // Preserve every consumed execution, not iteration zero / global
        // latest. A failed or still-pending permitted input is not reusable.
        let succeeded: Vec<_> = records
            .iter()
            .filter(|record| record.status == NodeExecutionStatus::Succeeded)
            .collect();
        if succeeded.is_empty()
            || records.iter().any(|record| {
                !matches!(
                    record.status,
                    NodeExecutionStatus::Succeeded | NodeExecutionStatus::Skipped
                )
            })
        {
            return Err(format!(
                "Cannot reuse Node `{id}`: the settled source has no complete successful input; choose an earlier rework Node"
            ));
        }
        for record in succeeded {
            reuse.push(ReuseBinding {
                node_id: id.clone(),
                iteration: record.iteration,
                source_execution_id: record.execution_id.clone(),
            });
        }
    }
    Ok(ReworkPlan {
        affected_node_ids: affected,
        reuse,
        skipped_node_ids: skipped,
    })
}

fn downstream_closure(graph: &WorkflowGraph, mut nodes: BTreeSet<String>) -> BTreeSet<String> {
    loop {
        let next: Vec<_> = graph
            .edges
            .iter()
            .filter(|edge| nodes.contains(&edge.source))
            .map(|edge| edge.target.clone())
            .collect();
        let before = nodes.len();
        nodes.extend(next);
        if before == nodes.len() {
            return nodes;
        }
    }
}

fn permitted_nodes(
    graph: &WorkflowGraph,
    affected: &BTreeSet<String>,
    source: &[ReworkSourceExecution],
) -> BTreeSet<String> {
    let mut reachable: BTreeSet<_> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == WorkflowNodeKind::Start)
        .map(|node| node.id.clone())
        .collect();
    loop {
        let next: Vec<_> = graph
            .edges
            .iter()
            .filter(|edge| {
                if !reachable.contains(&edge.source) {
                    return false;
                }
                let is_condition = graph
                    .nodes
                    .iter()
                    .any(|node| node.id == edge.source && node.kind == WorkflowNodeKind::Condition);
                !is_condition
                    || affected.contains(&edge.source)
                    || source.iter().any(|record| {
                        record.node_id == edge.source
                            && record.status == NodeExecutionStatus::Succeeded
                            && crate::planner::condition_output_selects_target(
                                record.output_text.as_deref(),
                                &edge.target,
                            )
                    })
            })
            .map(|edge| edge.target.clone())
            .collect();
        let before = reachable.len();
        reachable.extend(next);
        if before == reachable.len() {
            return reachable;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{WorkflowEdge, WorkflowEdgeKind, WorkflowNode, WorkflowNodeData};

    fn graph() -> WorkflowGraph {
        WorkflowGraph {
            version: 2,
            router_executor_config: None,
            canvas: None,
            nodes: [
                ("start", WorkflowNodeKind::Start),
                ("condition", WorkflowNodeKind::Condition),
                ("left", WorkflowNodeKind::Agent),
                ("right", WorkflowNodeKind::Agent),
                ("end", WorkflowNodeKind::End),
            ]
            .into_iter()
            .map(|(id, kind)| WorkflowNode {
                id: id.into(),
                kind,
                data: WorkflowNodeData::default(),
                position: None,
            })
            .collect(),
            edges: [
                ("start", "condition"),
                ("condition", "left"),
                ("condition", "right"),
                ("left", "end"),
                ("right", "end"),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (source, target))| WorkflowEdge {
                id: i.to_string(),
                source: source.into(),
                target: target.into(),
                source_handle: None,
                target_handle: None,
                kind: WorkflowEdgeKind::Default,
                data: None,
            })
            .collect(),
        }
    }
    fn source() -> Vec<ReworkSourceExecution> {
        ["start", "condition", "left", "end"]
            .into_iter()
            .map(|id| ReworkSourceExecution {
                execution_id: format!("actual-{id}"),
                node_id: id.into(),
                iteration: 0,
                status: NodeExecutionStatus::Succeeded,
                output_text: Some(if id == "condition" {
                    r#"{"selected_target_node_ids":["left"]}"#.into()
                } else {
                    id.into()
                }),
            })
            .collect()
    }
    #[test]
    fn rework_expands_downstream_and_reuses_exact_multiple_inputs() {
        let mut records = source();
        let mut second = records[1].clone();
        second.iteration = 1;
        second.execution_id = "condition-iteration-1".into();
        records.push(second);
        let plan = plan_rework(&graph(), Some(&["left".into()]), &records).unwrap();
        assert_eq!(
            plan.affected_node_ids,
            BTreeSet::from(["left".into(), "end".into()])
        );
        assert_eq!(
            plan.reuse
                .iter()
                .filter(|binding| binding.node_id == "condition")
                .count(),
            2
        );
        assert!(
            plan.reuse
                .iter()
                .any(|binding| binding.source_execution_id == "condition-iteration-1")
        );
        assert!(plan.skipped_node_ids.contains("right"));
    }
    #[test]
    fn unselected_branch_requires_earlier_condition_root() {
        assert!(
            plan_rework(&graph(), Some(&["right".into()]), &source())
                .unwrap_err()
                .contains("recorded Condition route")
        );
        let plan = plan_rework(&graph(), Some(&["condition".into()]), &source()).unwrap();
        assert!(plan.affected_node_ids.contains("right"));
        assert!(!plan.reuse.iter().any(|binding| binding.node_id == "left"));
    }
    #[test]
    fn missing_input_fails_without_fabricated_execution() {
        let mut records = source();
        records.remove(0);
        assert!(
            plan_rework(&graph(), Some(&["left".into()]), &records)
                .unwrap_err()
                .contains("Cannot reuse")
        );
        assert!(
            plan_rework(&graph(), None, &records)
                .unwrap()
                .reuse
                .is_empty()
        );
    }

    #[test]
    fn failure_induced_skips_cannot_replace_required_upstream_results() {
        let mut linear = graph();
        linear.nodes.retain(|node| node.id != "right");
        linear
            .nodes
            .iter_mut()
            .find(|node| node.id == "condition")
            .unwrap()
            .kind = WorkflowNodeKind::Agent;
        linear
            .edges
            .retain(|edge| edge.source != "right" && edge.target != "right");
        let records: Vec<_> = linear
            .nodes
            .iter()
            .map(|node| ReworkSourceExecution {
                execution_id: format!("actual-{}", node.id),
                node_id: node.id.clone(),
                iteration: 0,
                status: NodeExecutionStatus::Skipped,
                output_text: None,
            })
            .collect();

        assert!(
            plan_rework(&linear, Some(&["left".into()]), &records)
                .unwrap_err()
                .contains("Cannot reuse")
        );
        assert!(
            plan_rework(&linear, None, &records)
                .unwrap()
                .reuse
                .is_empty()
        );
    }

    #[test]
    fn recorded_unselected_branch_remains_skipped_without_reuse() {
        let mut records = source();
        records.push(ReworkSourceExecution {
            execution_id: "actual-right".into(),
            node_id: "right".into(),
            iteration: 0,
            status: NodeExecutionStatus::Skipped,
            output_text: None,
        });

        let plan = plan_rework(&graph(), Some(&["left".into()]), &records).unwrap();
        assert_eq!(plan.skipped_node_ids, BTreeSet::from(["right".into()]));
        assert!(!plan.reuse.iter().any(|binding| binding.node_id == "right"));
    }
}
