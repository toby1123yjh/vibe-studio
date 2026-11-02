use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
};

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
    let rounds = ExecutionRounds::new(graph, &executions);
    let mut ready_nodes = Vec::new();
    for node in &graph.nodes {
        if batch_managed.contains(&node.id) {
            continue;
        }
        for (round, execution) in rounds.node_executions(&node.id).enumerate() {
            if execution.status == NodeExecutionStatus::Pending
                && (node.kind == WorkflowNodeKind::Start && round == 0
                    || matches!(rounds.dependencies(&node.id, round), Dependencies::Ready(_)))
            {
                ready_nodes.push(ready_node(node, execution.iteration));
            }
        }
    }

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
    let rounds = ExecutionRounds::new(graph, &executions);
    let count = graph
        .edges
        .iter()
        .filter(|edge| edge.target == target_node_id)
        .map(|edge| rounds.node_executions(&edge.source).count())
        .max()
        .unwrap_or_default();
    // One completed set of required inputs consumes one round. Counting edges
    // would run a join twice when its two parallel inputs finish together.
    (0..count)
        .take_while(|round| {
            matches!(
                rounds.dependencies(target_node_id, *round),
                Dependencies::Ready(_)
            )
        })
        .count() as i64
}

/// Select the exact effective input executions for this target's round. The
/// runtime resolves these (node, iteration) pairs to immutable source IDs and
/// verifies their accepted files before launch. Reused iteration numbers are
/// intentionally not renumbered: their ordered position is the logical round.
pub fn upstream_execution_rounds(
    graph: &WorkflowGraph,
    snapshot: &RunSnapshot,
    target_node_id: &str,
    target_iteration: i64,
) -> Vec<(String, i64)> {
    let executions = execution_snapshots(graph, snapshot);
    let rounds = ExecutionRounds::new(graph, &executions);
    let Some(round) = rounds
        .node_executions(target_node_id)
        .position(|execution| execution.iteration == target_iteration)
    else {
        return Vec::new();
    };
    match rounds.dependencies(target_node_id, round) {
        Dependencies::Ready(inputs) => inputs
            .into_iter()
            .map(|execution| (execution.node_id.clone(), execution.iteration))
            .collect(),
        Dependencies::Inactive | Dependencies::Waiting => Vec::new(),
    }
}

/// Only recorded Condition choices can exclude a path. A failed run may also
/// contain `skipped` rows; those are not proof that a required input is optional.
pub fn skipped_node_rounds(graph: &WorkflowGraph, snapshot: &RunSnapshot) -> Vec<(String, i64)> {
    let executions = execution_snapshots(graph, snapshot);
    let rounds = ExecutionRounds::new(graph, &executions);
    let batch_managed = crate::dispatch::batch_managed_nodes(graph);
    let mut skipped = Vec::new();
    for node in &graph.nodes {
        if node.kind == WorkflowNodeKind::Start || batch_managed.contains(&node.id) {
            continue;
        }
        for (round, execution) in rounds.node_executions(&node.id).enumerate() {
            if execution.status == NodeExecutionStatus::Pending
                && rounds.activation(&node.id, round, &mut HashSet::new()) == Activation::Inactive
            {
                skipped.push((node.id.clone(), execution.iteration));
            }
        }
    }
    skipped
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Activation {
    Active,
    Inactive,
    Unknown,
}

enum Dependencies<'a> {
    Ready(Vec<&'a NodeExecutionSnapshot>),
    Inactive,
    Waiting,
}

struct ExecutionRounds<'a> {
    graph: &'a WorkflowGraph,
    executions: HashMap<&'a str, Vec<&'a NodeExecutionSnapshot>>,
    activations: RefCell<HashMap<(String, usize), Activation>>,
}

impl<'a> ExecutionRounds<'a> {
    fn new(graph: &'a WorkflowGraph, executions: &'a [NodeExecutionSnapshot]) -> Self {
        let mut grouped: HashMap<&str, Vec<&NodeExecutionSnapshot>> = HashMap::new();
        for execution in executions {
            grouped
                .entry(&execution.node_id)
                .or_default()
                .push(execution);
        }
        for executions in grouped.values_mut() {
            executions.sort_by_key(|execution| execution.iteration);
        }
        Self {
            graph,
            executions: grouped,
            activations: RefCell::default(),
        }
    }

    fn node_executions(
        &self,
        node_id: &str,
    ) -> impl Iterator<Item = &'a NodeExecutionSnapshot> + '_ {
        self.executions.get(node_id).into_iter().flatten().copied()
    }

    fn execution(&self, node_id: &str, round: usize) -> Option<&'a NodeExecutionSnapshot> {
        self.executions.get(node_id)?.get(round).copied()
    }

    fn edge_activation(
        &self,
        edge: &WorkflowEdge,
        round: usize,
        visiting: &mut HashSet<String>,
    ) -> Activation {
        let source = self.activation(&edge.source, round, visiting);
        if source != Activation::Active {
            return source;
        }
        let Some(node) = self.graph.nodes.iter().find(|node| node.id == edge.source) else {
            return Activation::Unknown;
        };
        if node.kind != WorkflowNodeKind::Condition {
            return Activation::Active;
        }
        let Some(execution) = self.execution(&edge.source, round) else {
            return Activation::Unknown;
        };
        if execution.status != NodeExecutionStatus::Succeeded {
            return Activation::Unknown;
        }
        let Some(selected) = condition_selected_targets(execution.output_text.as_deref()) else {
            return Activation::Unknown;
        };
        if selected.is_empty()
            || selected.iter().any(|target| {
                !self
                    .graph
                    .edges
                    .iter()
                    .any(|candidate| candidate.source == edge.source && candidate.target == *target)
            })
        {
            return Activation::Unknown;
        }
        if selected.iter().any(|target| target == &edge.target) {
            Activation::Active
        } else {
            Activation::Inactive
        }
    }

    fn activation(
        &self,
        node_id: &str,
        round: usize,
        visiting: &mut HashSet<String>,
    ) -> Activation {
        let key = (node_id.to_string(), round);
        if let Some(state) = self.activations.borrow().get(&key) {
            return *state;
        }
        let Some(node) = self.graph.nodes.iter().find(|node| node.id == node_id) else {
            return Activation::Unknown;
        };
        if node.kind == WorkflowNodeKind::Start {
            return Activation::Active;
        }
        if !visiting.insert(node_id.to_string()) {
            return Activation::Unknown;
        }
        let mut incoming = self
            .graph
            .edges
            .iter()
            .filter(|edge| edge.target == node_id)
            .peekable();
        let state = if incoming.peek().is_none() {
            Activation::Unknown
        } else {
            let mut state = Activation::Inactive;
            for edge in incoming {
                match self.edge_activation(edge, round, visiting) {
                    Activation::Active => {
                        state = Activation::Active;
                        break;
                    }
                    Activation::Unknown => state = Activation::Unknown,
                    Activation::Inactive => {}
                }
            }
            state
        };
        visiting.remove(node_id);
        self.activations.borrow_mut().insert(key, state);
        state
    }

    fn dependencies(&self, node_id: &str, round: usize) -> Dependencies<'a> {
        let mut inputs = Vec::new();
        let mut waiting = false;
        for edge in self
            .graph
            .edges
            .iter()
            .filter(|edge| edge.target == node_id)
        {
            match self.edge_activation(edge, round, &mut HashSet::new()) {
                Activation::Inactive => continue,
                Activation::Unknown => {
                    waiting = true;
                    continue;
                }
                Activation::Active => {}
            }
            match self.execution(&edge.source, round) {
                Some(execution) if execution.status == NodeExecutionStatus::Succeeded => {
                    if !inputs.iter().any(|input: &&NodeExecutionSnapshot| {
                        input.node_id == execution.node_id && input.iteration == execution.iteration
                    }) {
                        inputs.push(execution);
                    }
                }
                _ => waiting = true,
            }
        }
        if waiting {
            Dependencies::Waiting
        } else if inputs.is_empty() {
            Dependencies::Inactive
        } else {
            Dependencies::Ready(inputs)
        }
    }
}

fn condition_selected_targets(output_text: Option<&str>) -> Option<Vec<String>> {
    let output: serde_json::Value = serde_json::from_str(output_text?).ok()?;
    let selected = output
        .get("selected_target_node_ids")
        .or_else(|| output.get("decision")?.get("selected_target_node_ids"))?
        .as_array()?;
    selected
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
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
            skipped_node_rounds, triggered_execution_count, upstream_execution_rounds,
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
    fn fan_in_waits_for_all_required_upstream_deliveries_and_consumes_one_round() {
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
        assert_eq!(ready_after_one.ready_nodes.len(), 1);
        assert_eq!(
            triggered_execution_count(
                &graph,
                &snapshot(&[
                    ("start", NodeExecutionStatus::Succeeded),
                    ("a", NodeExecutionStatus::Succeeded),
                ]),
                "join"
            ),
            0
        );

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
        assert_eq!(
            triggered_execution_count(
                &graph,
                &snapshot(&[
                    ("start", NodeExecutionStatus::Succeeded),
                    ("a", NodeExecutionStatus::Succeeded),
                    ("b", NodeExecutionStatus::Succeeded),
                ]),
                "join"
            ),
            1
        );
    }

    #[test]
    fn failure_skip_does_not_make_a_required_input_optional() {
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

        assert!(plan.ready_nodes.is_empty());
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
    fn completed_join_cannot_consume_the_same_inputs_a_second_time() {
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
                .all(|node| { node.node_id != "join" || node.iteration != 1 })
        );
    }

    #[test]
    fn unselected_nested_branch_is_excluded_without_waiting_for_phantom_deliveries() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("route", WorkflowNodeKind::Condition),
                node("selected", WorkflowNodeKind::Agent),
                node("unused", WorkflowNodeKind::Agent),
                node("unused-child", WorkflowNodeKind::Agent),
                node("join", WorkflowNodeKind::Agent),
            ],
            vec![
                edge("s-r", "start", "route"),
                condition_edge("r-s", "route", "selected"),
                condition_edge("r-u", "route", "unused"),
                edge("u-c", "unused", "unused-child"),
                edge("s-j", "selected", "join"),
                edge("c-j", "unused-child", "join"),
            ],
        );
        let mut state = snapshot(&[
            ("start", NodeExecutionStatus::Succeeded),
            ("route", NodeExecutionStatus::Succeeded),
            ("selected", NodeExecutionStatus::Succeeded),
        ]);
        state.nodes[1].output_text = Some(condition_output(&["selected"], &["unused"]));
        let plan = plan_ready_nodes(&graph, &state);
        assert_eq!(
            plan.ready_nodes
                .iter()
                .map(|node| node.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["join"]
        );
        assert_eq!(
            skipped_node_rounds(&graph, &state),
            vec![("unused".into(), 0), ("unused-child".into(), 0)]
        );
        assert_eq!(
            upstream_execution_rounds(&graph, &state, "join", 0),
            vec![("selected".into(), 0)]
        );
    }

    #[test]
    fn unresolved_condition_blocks_a_join_even_when_an_independent_input_succeeded() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("ready", WorkflowNodeKind::Agent),
                node("route", WorkflowNodeKind::Condition),
                node("possible", WorkflowNodeKind::Agent),
                node("other", WorkflowNodeKind::Agent),
                node("join", WorkflowNodeKind::Agent),
            ],
            vec![
                edge("s-a", "start", "ready"),
                edge("s-r", "start", "route"),
                condition_edge("r-p", "route", "possible"),
                condition_edge("r-o", "route", "other"),
                edge("a-j", "ready", "join"),
                edge("p-j", "possible", "join"),
            ],
        );
        let mut state = snapshot(&[
            ("start", NodeExecutionStatus::Succeeded),
            ("ready", NodeExecutionStatus::Succeeded),
            ("route", NodeExecutionStatus::Running),
        ]);
        assert!(plan_ready_nodes(&graph, &state).ready_nodes.is_empty());
        assert!(skipped_node_rounds(&graph, &state).is_empty());
        state.nodes[2].status = NodeExecutionStatus::Succeeded;
        state.nodes[2].output_text = Some(condition_output(&["other"], &["possible"]));
        let plan = plan_ready_nodes(&graph, &state);
        assert!(plan.ready_nodes.iter().any(|node| node.node_id == "join"));
        assert_eq!(
            upstream_execution_rounds(&graph, &state, "join", 0),
            vec![("ready".into(), 0)]
        );
    }

    #[test]
    fn all_selected_condition_branches_are_required_at_the_join() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("route", WorkflowNodeKind::Condition),
                node("a", WorkflowNodeKind::Agent),
                node("b", WorkflowNodeKind::Agent),
                node("join", WorkflowNodeKind::Agent),
            ],
            vec![
                edge("s-r", "start", "route"),
                condition_edge("r-a", "route", "a"),
                condition_edge("r-b", "route", "b"),
                edge("a-j", "a", "join"),
                edge("b-j", "b", "join"),
            ],
        );
        let mut state = snapshot(&[
            ("start", NodeExecutionStatus::Succeeded),
            ("route", NodeExecutionStatus::Succeeded),
            ("a", NodeExecutionStatus::Succeeded),
            ("b", NodeExecutionStatus::Failed),
        ]);
        state.nodes[1].output_text = Some(condition_output(&["a", "b"], &[]));
        assert!(plan_ready_nodes(&graph, &state).ready_nodes.is_empty());
        assert!(skipped_node_rounds(&graph, &state).is_empty());
        assert_eq!(triggered_execution_count(&graph, &state, "join"), 0);
    }

    #[test]
    fn inputs_bind_to_one_ordered_round_without_mixing_earlier_successes() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("a", WorkflowNodeKind::Agent),
                node("b", WorkflowNodeKind::Agent),
                node("join", WorkflowNodeKind::Agent),
            ],
            vec![
                edge("s-a", "start", "a"),
                edge("s-b", "start", "b"),
                edge("a-j", "a", "join"),
                edge("b-j", "b", "join"),
            ],
        );
        let mut state = snapshot(&[
            ("start", NodeExecutionStatus::Succeeded),
            ("a", NodeExecutionStatus::Succeeded),
            ("b", NodeExecutionStatus::Succeeded),
            ("join", NodeExecutionStatus::Succeeded),
        ]);
        for (node_id, iteration, status) in [
            ("a", 4, NodeExecutionStatus::Succeeded),
            ("b", 7, NodeExecutionStatus::Running),
            ("join", 2, NodeExecutionStatus::Pending),
        ] {
            state.nodes.push(NodeExecutionSnapshot {
                node_id: node_id.into(),
                iteration,
                status,
                output_text: None,
                error_text: None,
            });
        }
        assert!(
            plan_ready_nodes(&graph, &state)
                .ready_nodes
                .iter()
                .all(|node| node.node_id != "join")
        );
        assert!(upstream_execution_rounds(&graph, &state, "join", 2).is_empty());
        state
            .nodes
            .iter_mut()
            .find(|node| node.node_id == "b" && node.iteration == 7)
            .unwrap()
            .status = NodeExecutionStatus::Succeeded;
        assert_eq!(triggered_execution_count(&graph, &state, "join"), 2);
        assert_eq!(
            upstream_execution_rounds(&graph, &state, "join", 2),
            vec![("a".into(), 4), ("b".into(), 7)]
        );
        assert_eq!(
            plan_ready_nodes(&graph, &state)
                .ready_nodes
                .iter()
                .filter(|node| node.node_id == "join")
                .count(),
            1
        );
    }

    #[test]
    fn retry_run_reuses_the_verified_sibling_execution_without_replaying_its_round() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("a", WorkflowNodeKind::Agent),
                node("b", WorkflowNodeKind::Agent),
                node("join", WorkflowNodeKind::Agent),
            ],
            vec![
                edge("s-a", "start", "a"),
                edge("s-b", "start", "b"),
                edge("a-j", "a", "join"),
                edge("b-j", "b", "join"),
            ],
        );
        let mut state = snapshot(&[
            ("start", NodeExecutionStatus::Succeeded),
            ("a", NodeExecutionStatus::Succeeded),
            ("b", NodeExecutionStatus::Running),
        ]);
        state.run_id = "retry-run".into();
        state.nodes[1].iteration = 5; // Exact reused source iteration, not a new Agent execution.
        assert!(plan_ready_nodes(&graph, &state).ready_nodes.is_empty());
        state.nodes[2].status = NodeExecutionStatus::Succeeded;
        assert_eq!(
            upstream_execution_rounds(&graph, &state, "join", 0),
            vec![("a".into(), 5), ("b".into(), 0)]
        );
        assert_eq!(triggered_execution_count(&graph, &state, "join"), 1);
    }
}
