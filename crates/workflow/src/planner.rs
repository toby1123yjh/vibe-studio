use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::graph::{WorkflowEdge, WorkflowGraph, WorkflowNode, WorkflowNodeKind};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeExecutionStatus {
    #[default]
    Pending,
    Running,
    AwaitingHuman,
    AwaitingArena,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeExecutionSnapshot {
    pub node_id: String,
    #[serde(default)]
    pub iteration: i64,
    pub status: NodeExecutionStatus,
    pub output_text: Option<String>,
    pub error_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub run_id: String,
    pub nodes: Vec<NodeExecutionSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadyNode {
    pub node_id: String,
    pub iteration: i64,
    pub kind: WorkflowNodeKind,
    pub writes_main_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlannerWarning {
    SerializedMainWorktreeAgents { node_ids: Vec<String> },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReadyPlan {
    pub ready_nodes: Vec<ReadyNode>,
    pub warnings: Vec<PlannerWarning>,
}

pub fn plan_ready_nodes(graph: &WorkflowGraph, snapshot: &RunSnapshot) -> ReadyPlan {
    let batch_managed = crate::dispatch::batch_managed_nodes(graph);
    let executions = execution_snapshots(graph, snapshot);
    let incoming = incoming_edges(graph);
    let nodes_by_id = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();

    let ready_nodes = executions
        .iter()
        .filter(|execution| execution.status == NodeExecutionStatus::Pending)
        .filter(|execution| !batch_managed.contains(&execution.node_id))
        .filter_map(|execution| {
            let node = nodes_by_id.get(execution.node_id.as_str())?;
            if execution_is_ready(
                node,
                execution.iteration,
                incoming.get(node.id.as_str()).map(Vec::as_slice),
                graph,
                &executions,
            ) {
                Some(ready_node(node, execution.iteration))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    ReadyPlan {
        ready_nodes,
        warnings: Vec::new(),
    }
}

fn execution_snapshots(
    graph: &WorkflowGraph,
    snapshot: &RunSnapshot,
) -> Vec<NodeExecutionSnapshot> {
    let mut executions = snapshot.nodes.clone();
    let known_nodes = executions
        .iter()
        .map(|execution| execution.node_id.clone())
        .collect::<HashSet<_>>();

    for node in &graph.nodes {
        if known_nodes.contains(node.id.as_str()) {
            continue;
        }
        executions.push(NodeExecutionSnapshot {
            node_id: node.id.clone(),
            iteration: 0,
            status: NodeExecutionStatus::Pending,
            output_text: None,
            error_text: None,
        });
    }

    executions
}

pub fn triggered_execution_count(
    graph: &WorkflowGraph,
    snapshot: &RunSnapshot,
    target_node_id: &str,
) -> i64 {
    if crate::dispatch::batch_managed_nodes(graph).contains(target_node_id) {
        return 0;
    }
    let executions = execution_snapshots(graph, snapshot);
    let incoming = incoming_edges(graph);
    incoming
        .get(target_node_id)
        .map(|edges| {
            edges
                .iter()
                .map(|edge| succeeded_trigger_count_for_edge(graph, edge, &executions))
                .sum()
        })
        .unwrap_or_default()
}

fn incoming_edges(graph: &WorkflowGraph) -> HashMap<&str, Vec<&WorkflowEdge>> {
    let mut incoming: HashMap<&str, Vec<&WorkflowEdge>> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), Vec::new()))
        .collect();

    for edge in &graph.edges {
        incoming.entry(edge.target.as_str()).or_default().push(edge);
    }

    incoming
}

fn execution_is_ready(
    node: &WorkflowNode,
    iteration: i64,
    incoming: Option<&[&WorkflowEdge]>,
    graph: &WorkflowGraph,
    executions: &[NodeExecutionSnapshot],
) -> bool {
    let Some(incoming) = incoming else {
        return node.kind == WorkflowNodeKind::Start && iteration == 0;
    };
    if incoming.is_empty() {
        return node.kind == WorkflowNodeKind::Start && iteration == 0;
    }

    incoming
        .iter()
        .any(|edge| succeeded_trigger_count_for_edge(graph, edge, executions) > 0)
}

fn succeeded_trigger_count_for_edge(
    graph: &WorkflowGraph,
    edge: &WorkflowEdge,
    executions: &[NodeExecutionSnapshot],
) -> i64 {
    executions
        .iter()
        .filter(|execution| {
            execution.status == NodeExecutionStatus::Succeeded
                && execution.node_id == edge.source
                && execution_triggers_edge(graph, edge, execution)
        })
        .count() as i64
}

fn execution_triggers_edge(
    graph: &WorkflowGraph,
    edge: &WorkflowEdge,
    execution: &NodeExecutionSnapshot,
) -> bool {
    let Some(source_node) = graph.nodes.iter().find(|node| node.id == edge.source) else {
        return false;
    };
    if source_node.kind != WorkflowNodeKind::Condition {
        return true;
    }

    condition_output_selects_target(execution.output_text.as_deref(), &edge.target)
}

pub(crate) fn condition_output_selects_target(
    output_text: Option<&str>,
    target_node_id: &str,
) -> bool {
    let Some(output_text) = output_text else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output_text) else {
        return false;
    };

    json_string_array_contains(value.get("selected_target_node_ids"), target_node_id)
        || json_string_array_contains(
            value
                .get("decision")
                .and_then(|decision| decision.get("selected_target_node_ids")),
            target_node_id,
        )
}

fn json_string_array_contains(value: Option<&serde_json::Value>, needle: &str) -> bool {
    value
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(needle)))
}

fn ready_node(node: &WorkflowNode, iteration: i64) -> ReadyNode {
    ReadyNode {
        node_id: node.id.clone(),
        iteration,
        kind: node.kind.clone(),
        writes_main_worktree: matches!(
            node.kind,
            WorkflowNodeKind::Agent | WorkflowNodeKind::TaskSplitter
        ),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        graph::{
            WorkflowEdge, WorkflowEdgeKind, WorkflowGraph, WorkflowNode, WorkflowNodeData,
            WorkflowNodeKind,
        },
        planner::{
            NodeExecutionSnapshot, NodeExecutionStatus, RunSnapshot, plan_ready_nodes,
            triggered_execution_count,
        },
    };

    fn node(id: &str, kind: WorkflowNodeKind) -> WorkflowNode {
        WorkflowNode {
            id: id.to_string(),
            kind,
            data: WorkflowNodeData::default(),
            position: None,
        }
    }

    fn edge(id: &str, source: &str, target: &str) -> WorkflowEdge {
        WorkflowEdge {
            id: id.to_string(),
            source: source.to_string(),
            source_handle: Some("output-right".to_string()),
            target: target.to_string(),
            target_handle: Some("input-left".to_string()),
            kind: WorkflowEdgeKind::Default,
            data: None,
        }
    }

    fn condition_edge(id: &str, source: &str, target: &str) -> WorkflowEdge {
        WorkflowEdge {
            kind: WorkflowEdgeKind::ConditionBranch,
            ..edge(id, source, target)
        }
    }

    fn graph(nodes: Vec<WorkflowNode>, edges: Vec<WorkflowEdge>) -> WorkflowGraph {
        WorkflowGraph {
            version: 2,
            nodes,
            edges,
            router_executor_config: None,
            canvas: None,
        }
    }

    fn snapshot(nodes: &[(&str, NodeExecutionStatus)]) -> RunSnapshot {
        RunSnapshot {
            run_id: "run-1".to_string(),
            nodes: nodes
                .iter()
                .map(|(node_id, status)| NodeExecutionSnapshot {
                    node_id: (*node_id).to_string(),
                    iteration: 0,
                    status: *status,
                    output_text: None,
                    error_text: None,
                })
                .collect(),
        }
    }

    fn condition_output(selected: &[&str], skipped: &[&str]) -> String {
        serde_json::json!({
            "type": "condition_router_decision",
            "source": "router",
            "status": "selected",
            "schema_version": 1,
            "decision": {
                "schema_version": 1,
                "status": "selected",
                "selected_target_node_ids": selected,
                "skipped_target_node_ids": skipped,
                "confidence": "high",
                "reason": "Matched",
                "question": null
            },
            "raw_output": "",
            "selected_target_node_ids": selected,
            "skipped_target_node_ids": skipped,
            "validation": { "result": "auto_route" }
        })
        .to_string()
    }

    #[test]
    fn start_is_ready_first() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("end", WorkflowNodeKind::End),
            ],
            vec![edge("e1", "start", "end")],
        );

        let plan = plan_ready_nodes(&graph, &snapshot(&[]));

        assert_eq!(
            plan.ready_nodes
                .iter()
                .map(|node| node.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["start"]
        );
    }

    #[test]
    fn downstream_node_becomes_ready_after_upstream_succeeds() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("agent", WorkflowNodeKind::Agent),
                node("end", WorkflowNodeKind::End),
            ],
            vec![edge("e1", "start", "agent"), edge("e2", "agent", "end")],
        );

        let plan = plan_ready_nodes(
            &graph,
            &snapshot(&[("start", NodeExecutionStatus::Succeeded)]),
        );

        assert_eq!(plan.ready_nodes[0].node_id, "agent");
    }

    #[test]
    fn fan_in_is_ready_after_any_upstream_succeeds() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("a", WorkflowNodeKind::Transform),
                node("b", WorkflowNodeKind::Transform),
                node("join", WorkflowNodeKind::Agent),
                node("end", WorkflowNodeKind::End),
            ],
            vec![
                edge("e1", "start", "a"),
                edge("e2", "start", "b"),
                edge("e3", "a", "join"),
                edge("e4", "b", "join"),
                edge("e5", "join", "end"),
            ],
        );

        let ready_after_one = plan_ready_nodes(
            &graph,
            &snapshot(&[
                ("start", NodeExecutionStatus::Succeeded),
                ("a", NodeExecutionStatus::Succeeded),
            ]),
        );
        assert_eq!(ready_after_one.ready_nodes[0].node_id, "b");
        assert_eq!(ready_after_one.ready_nodes[1].node_id, "join");

        let ready = plan_ready_nodes(
            &graph,
            &snapshot(&[
                ("start", NodeExecutionStatus::Succeeded),
                ("a", NodeExecutionStatus::Succeeded),
                ("b", NodeExecutionStatus::Succeeded),
            ]),
        );
        assert_eq!(
            ready
                .ready_nodes
                .iter()
                .map(|node| node.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["join"]
        );
    }

    #[test]
    fn skipped_branch_unblocks_join_when_another_upstream_succeeded() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("selected", WorkflowNodeKind::Transform),
                node("skipped", WorkflowNodeKind::Transform),
                node("join", WorkflowNodeKind::Agent),
                node("end", WorkflowNodeKind::End),
            ],
            vec![
                edge("e1", "start", "selected"),
                edge("e2", "start", "skipped"),
                edge("e3", "selected", "join"),
                edge("e4", "skipped", "join"),
                edge("e5", "join", "end"),
            ],
        );

        let plan = plan_ready_nodes(
            &graph,
            &snapshot(&[
                ("start", NodeExecutionStatus::Succeeded),
                ("selected", NodeExecutionStatus::Succeeded),
                ("skipped", NodeExecutionStatus::Skipped),
            ]),
        );

        assert_eq!(plan.ready_nodes[0].node_id, "join");
    }

    #[test]
    fn skipped_only_upstream_does_not_make_node_ready() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("skipped", WorkflowNodeKind::Transform),
                node("downstream", WorkflowNodeKind::Agent),
                node("end", WorkflowNodeKind::End),
            ],
            vec![
                edge("e1", "start", "skipped"),
                edge("e2", "skipped", "downstream"),
                edge("e3", "downstream", "end"),
            ],
        );

        let plan = plan_ready_nodes(
            &graph,
            &snapshot(&[
                ("start", NodeExecutionStatus::Succeeded),
                ("skipped", NodeExecutionStatus::Skipped),
            ]),
        );

        assert!(plan.ready_nodes.is_empty());
    }

    #[test]
    fn fan_out_keeps_multiple_ready_agent_nodes() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("agent-a", WorkflowNodeKind::Agent),
                node("agent-b", WorkflowNodeKind::Agent),
                node("end", WorkflowNodeKind::End),
            ],
            vec![
                edge("e1", "start", "agent-a"),
                edge("e2", "start", "agent-b"),
                edge("e3", "agent-a", "end"),
                edge("e4", "agent-b", "end"),
            ],
        );

        let plan = plan_ready_nodes(
            &graph,
            &snapshot(&[("start", NodeExecutionStatus::Succeeded)]),
        );

        assert_eq!(
            plan.ready_nodes
                .iter()
                .map(|node| node.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["agent-a", "agent-b"]
        );
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn condition_success_only_readies_selected_targets() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("condition", WorkflowNodeKind::Condition),
                node("ui", WorkflowNodeKind::Agent),
                node("api", WorkflowNodeKind::Agent),
                node("end", WorkflowNodeKind::End),
            ],
            vec![
                edge("e1", "start", "condition"),
                condition_edge("e2", "condition", "ui"),
                condition_edge("e3", "condition", "api"),
                edge("e4", "ui", "end"),
                edge("e5", "api", "end"),
            ],
        );
        let snapshot = RunSnapshot {
            run_id: "run-1".to_string(),
            nodes: vec![
                NodeExecutionSnapshot {
                    node_id: "start".to_string(),
                    iteration: 0,
                    status: NodeExecutionStatus::Succeeded,
                    output_text: None,
                    error_text: None,
                },
                NodeExecutionSnapshot {
                    node_id: "condition".to_string(),
                    iteration: 0,
                    status: NodeExecutionStatus::Succeeded,
                    output_text: Some(condition_output(&["ui"], &["api"])),
                    error_text: None,
                },
                NodeExecutionSnapshot {
                    node_id: "ui".to_string(),
                    iteration: 0,
                    status: NodeExecutionStatus::Pending,
                    output_text: None,
                    error_text: None,
                },
                NodeExecutionSnapshot {
                    node_id: "api".to_string(),
                    iteration: 0,
                    status: NodeExecutionStatus::Pending,
                    output_text: None,
                    error_text: None,
                },
            ],
        };

        let plan = plan_ready_nodes(&graph, &snapshot);

        assert_eq!(
            plan.ready_nodes
                .iter()
                .map(|node| node.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["ui"]
        );
        assert_eq!(triggered_execution_count(&graph, &snapshot, "ui"), 1);
        assert_eq!(triggered_execution_count(&graph, &snapshot, "api"), 0);
    }

    #[test]
    fn fan_in_can_ready_later_iterations_for_same_node() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("a", WorkflowNodeKind::Transform),
                node("b", WorkflowNodeKind::Transform),
                node("join", WorkflowNodeKind::Agent),
                node("end", WorkflowNodeKind::End),
            ],
            vec![
                edge("e1", "start", "a"),
                edge("e2", "start", "b"),
                edge("e3", "a", "join"),
                edge("e4", "b", "join"),
                edge("e5", "join", "end"),
            ],
        );

        let plan = plan_ready_nodes(
            &graph,
            &RunSnapshot {
                run_id: "run-1".to_string(),
                nodes: vec![
                    NodeExecutionSnapshot {
                        node_id: "start".to_string(),
                        iteration: 0,
                        status: NodeExecutionStatus::Succeeded,
                        output_text: None,
                        error_text: None,
                    },
                    NodeExecutionSnapshot {
                        node_id: "a".to_string(),
                        iteration: 0,
                        status: NodeExecutionStatus::Succeeded,
                        output_text: None,
                        error_text: None,
                    },
                    NodeExecutionSnapshot {
                        node_id: "b".to_string(),
                        iteration: 0,
                        status: NodeExecutionStatus::Succeeded,
                        output_text: None,
                        error_text: None,
                    },
                    NodeExecutionSnapshot {
                        node_id: "join".to_string(),
                        iteration: 0,
                        status: NodeExecutionStatus::Succeeded,
                        output_text: None,
                        error_text: None,
                    },
                    NodeExecutionSnapshot {
                        node_id: "join".to_string(),
                        iteration: 1,
                        status: NodeExecutionStatus::Pending,
                        output_text: None,
                        error_text: None,
                    },
                ],
            },
        );

        assert!(
            plan.ready_nodes
                .iter()
                .any(|node| { node.node_id == "join" && node.iteration == 1 })
        );
    }
}
