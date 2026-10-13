use std::{
    collections::{HashMap, HashSet, VecDeque},
    error::Error,
    fmt,
};

use crate::graph::{WorkflowEdge, WorkflowGraph, WorkflowNodeKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedGraph {
    pub version: u32,
    pub start_node_id: String,
    pub end_node_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    message: String,
}

impl ValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for ValidationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Visited,
}

pub fn validate_graph(graph: &WorkflowGraph) -> Result<ValidatedGraph, ValidationError> {
    if graph.version != 1 && graph.version != 2 {
        return Err(ValidationError::new(format!(
            "unsupported workflow graph version {}",
            graph.version
        )));
    }

    let mut node_ids = HashSet::new();
    for node in &graph.nodes {
        if !node_ids.insert(node.id.as_str()) {
            return Err(ValidationError::new(format!(
                "duplicate node id `{}`",
                node.id
            )));
        }
    }

    let start_nodes: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == WorkflowNodeKind::Start)
        .collect();
    if start_nodes.is_empty() {
        return Err(ValidationError::new("missing start node"));
    }
    if start_nodes.len() > 1 {
        return Err(ValidationError::new(
            "workflow graph must contain exactly one start node",
        ));
    }

    let end_node_ids: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == WorkflowNodeKind::End)
        .map(|node| node.id.clone())
        .collect();
    if end_node_ids.is_empty() {
        return Err(ValidationError::new("missing end node"));
    }

    for edge in &graph.edges {
        if !node_ids.contains(edge.source.as_str()) {
            return Err(ValidationError::new(format!(
                "edge `{}` has missing source node `{}`",
                edge.id, edge.source
            )));
        }
        if !node_ids.contains(edge.target.as_str()) {
            return Err(ValidationError::new(format!(
                "edge `{}` has missing target node `{}`",
                edge.id, edge.target
            )));
        }
    }

    let adjacency = adjacency_map(graph);
    reject_duplicate_condition_targets(graph)?;
    reject_cycles(graph, &adjacency)?;
    crate::dispatch::dispatch_scopes(graph).map_err(ValidationError::new)?;

    let start_node_id = start_nodes[0].id.as_str();
    let reachable = reachable_nodes(start_node_id, &adjacency);
    for node in &graph.nodes {
        if node.kind != WorkflowNodeKind::Start && !reachable.contains(node.id.as_str()) {
            return Err(ValidationError::new(format!(
                "unreachable node `{}`",
                node.id
            )));
        }
    }

    Ok(ValidatedGraph {
        version: graph.version,
        start_node_id: start_node_id.to_string(),
        end_node_ids,
    })
}

pub fn validate_graph_for_run(graph: &WorkflowGraph) -> Result<ValidatedGraph, ValidationError> {
    let validated = validate_graph(graph)?;
    validate_condition_routing_for_run(graph)?;
    Ok(validated)
}

fn adjacency_map(graph: &WorkflowGraph) -> HashMap<&str, Vec<&str>> {
    let mut adjacency: HashMap<&str, Vec<&str>> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), Vec::new()))
        .collect();

    for edge in &graph.edges {
        adjacency
            .entry(edge.source.as_str())
            .or_default()
            .push(edge.target.as_str());
    }

    adjacency
}

fn condition_outgoing_edges<'a>(
    graph: &'a WorkflowGraph,
    condition_node_id: &str,
) -> Vec<&'a WorkflowEdge> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.source == condition_node_id)
        .collect()
}

fn reject_duplicate_condition_targets(graph: &WorkflowGraph) -> Result<(), ValidationError> {
    for node in graph
        .nodes
        .iter()
        .filter(|node| node.kind == WorkflowNodeKind::Condition)
    {
        let mut targets = HashSet::new();
        for edge in condition_outgoing_edges(graph, &node.id) {
            if !targets.insert(edge.target.as_str()) {
                return Err(ValidationError::new(format!(
                    "condition node `{}` has duplicate outgoing target `{}`",
                    node.id, edge.target
                )));
            }
        }
    }
    Ok(())
}

fn validate_condition_routing_for_run(graph: &WorkflowGraph) -> Result<(), ValidationError> {
    let condition_nodes: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == WorkflowNodeKind::Condition)
        .collect();
    if condition_nodes.is_empty() {
        return Ok(());
    }

    if graph
        .router_executor_config
        .as_ref()
        .is_none_or(|config| !router_executor_config_has_executor(config))
    {
        return Err(ValidationError::new(
            "workflow with condition nodes requires router executor config",
        ));
    }

    for node in condition_nodes {
        let outgoing_targets: HashSet<&str> = condition_outgoing_edges(graph, &node.id)
            .into_iter()
            .map(|edge| edge.target.as_str())
            .collect();
        let branches = node.data.branches.as_deref().unwrap_or_default();
        let mut branch_targets = HashSet::new();

        for branch in branches {
            let Some(target_node_id) = branch.target_node_id.as_deref() else {
                return Err(ValidationError::new(format!(
                    "condition node `{}` has a branch without target",
                    node.id
                )));
            };
            if !branch_targets.insert(target_node_id) {
                return Err(ValidationError::new(format!(
                    "condition node `{}` has duplicate branch target `{}`",
                    node.id, target_node_id
                )));
            }
            if !outgoing_targets.contains(target_node_id) {
                return Err(ValidationError::new(format!(
                    "condition node `{}` has stale branch target `{}`",
                    node.id, target_node_id
                )));
            }
            if branch
                .condition
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
            {
                return Err(ValidationError::new(format!(
                    "condition node `{}` has empty branch condition for target `{}`",
                    node.id, target_node_id
                )));
            }
        }

        for target in outgoing_targets {
            if !branch_targets.contains(target) {
                return Err(ValidationError::new(format!(
                    "condition node `{}` is missing branch config for target `{}`",
                    node.id, target
                )));
            }
        }
    }

    Ok(())
}

fn router_executor_config_has_executor(config: &serde_json::Value) -> bool {
    config
        .get("executor")
        .and_then(serde_json::Value::as_str)
        .is_some_and(is_known_executor)
}

fn is_known_executor(executor: &str) -> bool {
    let normalized = executor.trim().replace('-', "_").to_ascii_uppercase();
    matches!(
        normalized.as_str(),
        "CLAUDE_CODE" | "GEMINI" | "CODEX" | "OH_MY_PI" | "OPENCODE" | "DEEPSEEK_HARNESS"
    )
}

fn reject_cycles(
    graph: &WorkflowGraph,
    adjacency: &HashMap<&str, Vec<&str>>,
) -> Result<(), ValidationError> {
    let mut states = HashMap::new();

    for node in &graph.nodes {
        if !states.contains_key(node.id.as_str()) {
            visit_for_cycle(node.id.as_str(), adjacency, &mut states)?;
        }
    }

    Ok(())
}

fn visit_for_cycle<'a>(
    node_id: &'a str,
    adjacency: &HashMap<&'a str, Vec<&'a str>>,
    states: &mut HashMap<&'a str, VisitState>,
) -> Result<(), ValidationError> {
    match states.get(node_id) {
        Some(VisitState::Visiting) => {
            return Err(ValidationError::new(format!(
                "workflow graph contains a cycle at `{node_id}`"
            )));
        }
        Some(VisitState::Visited) => return Ok(()),
        None => {}
    }

    states.insert(node_id, VisitState::Visiting);

    if let Some(targets) = adjacency.get(node_id) {
        for target in targets {
            visit_for_cycle(target, adjacency, states)?;
        }
    }

    states.insert(node_id, VisitState::Visited);
    Ok(())
}

fn reachable_nodes<'a>(
    start_node_id: &'a str,
    adjacency: &HashMap<&'a str, Vec<&'a str>>,
) -> HashSet<&'a str> {
    let mut reachable = HashSet::new();
    let mut queue = VecDeque::from([start_node_id]);

    while let Some(node_id) = queue.pop_front() {
        if !reachable.insert(node_id) {
            continue;
        }

        if let Some(targets) = adjacency.get(node_id) {
            queue.extend(targets.iter().copied());
        }
    }

    reachable
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{
        ConditionBranch, WorkflowEdge, WorkflowEdgeKind, WorkflowGraph, WorkflowNode,
        WorkflowNodeData, WorkflowNodeKind,
    };

    fn node(id: &str, kind: WorkflowNodeKind) -> WorkflowNode {
        WorkflowNode {
            id: id.to_string(),
            kind,
            data: WorkflowNodeData {
                display_name: Some(id.to_string()),
                ..WorkflowNodeData::default()
            },
            position: None,
        }
    }

    fn condition_node(id: &str, branches: Vec<ConditionBranch>) -> WorkflowNode {
        WorkflowNode {
            id: id.to_string(),
            kind: WorkflowNodeKind::Condition,
            data: WorkflowNodeData {
                display_name: Some(id.to_string()),
                branches: Some(branches),
                ..WorkflowNodeData::default()
            },
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

    fn graph(nodes: Vec<WorkflowNode>, edges: Vec<WorkflowEdge>) -> WorkflowGraph {
        WorkflowGraph {
            version: 2,
            nodes,
            edges,
            router_executor_config: None,
            canvas: None,
        }
    }

    #[test]
    fn accepts_graph_with_single_start_to_end_path() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("end", WorkflowNodeKind::End),
            ],
            vec![edge("e1", "start", "end")],
        );

        validate_graph(&graph).unwrap();
    }

    #[test]
    fn rejects_graph_without_start() {
        let graph = graph(
            vec![
                node("agent", WorkflowNodeKind::Agent),
                node("end", WorkflowNodeKind::End),
            ],
            vec![edge("e1", "agent", "end")],
        );

        let err = validate_graph(&graph).unwrap_err();
        assert!(err.to_string().contains("start"));
    }

    #[test]
    fn rejects_graph_with_multiple_start_nodes() {
        let graph = graph(
            vec![
                node("start-a", WorkflowNodeKind::Start),
                node("start-b", WorkflowNodeKind::Start),
                node("end", WorkflowNodeKind::End),
            ],
            vec![edge("e1", "start-a", "end"), edge("e2", "start-b", "end")],
        );

        let err = validate_graph(&graph).unwrap_err();
        assert!(err.to_string().contains("start"));
    }

    #[test]
    fn rejects_graph_with_cycle() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("agent", WorkflowNodeKind::Agent),
                node("end", WorkflowNodeKind::End),
            ],
            vec![
                edge("e1", "start", "agent"),
                edge("e2", "agent", "start"),
                edge("e3", "agent", "end"),
            ],
        );

        let err = validate_graph(&graph).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn rejects_unreachable_executable_node() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("agent", WorkflowNodeKind::Agent),
                node("end", WorkflowNodeKind::End),
            ],
            vec![edge("e1", "start", "end")],
        );

        let err = validate_graph(&graph).unwrap_err();
        assert!(err.to_string().contains("unreachable"));
    }

    #[test]
    fn rejects_edge_with_missing_endpoint() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                node("end", WorkflowNodeKind::End),
            ],
            vec![edge("e1", "start", "missing")],
        );

        let err = validate_graph(&graph).unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn draft_validation_allows_condition_without_router_config() {
        let mut graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                condition_node("condition", vec![]),
                node("end", WorkflowNodeKind::End),
            ],
            vec![
                edge("e1", "start", "condition"),
                edge("e2", "condition", "end"),
            ],
        );

        validate_graph(&graph).unwrap();
        let err = validate_graph_for_run(&graph).unwrap_err();
        assert!(err.to_string().contains("router executor config"));

        graph.router_executor_config = Some(serde_json::json!({}));
        let err = validate_graph_for_run(&graph).unwrap_err();
        assert!(err.to_string().contains("router executor config"));

        graph.router_executor_config = Some(serde_json::json!({"executor": "unknown"}));
        let err = validate_graph_for_run(&graph).unwrap_err();
        assert!(err.to_string().contains("router executor config"));
    }

    #[test]
    fn run_validation_accepts_complete_agentic_condition_runtime_config() {
        let mut graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                condition_node(
                    "condition",
                    vec![
                        ConditionBranch {
                            id: Some("branch-agent".to_string()),
                            target_node_id: Some("agent".to_string()),
                            condition: Some("Needs implementation".to_string()),
                        },
                        ConditionBranch {
                            id: Some("branch-end".to_string()),
                            target_node_id: Some("end".to_string()),
                            condition: Some("No work needed".to_string()),
                        },
                    ],
                ),
                node("agent", WorkflowNodeKind::Agent),
                node("end", WorkflowNodeKind::End),
            ],
            vec![
                edge("e1", "start", "condition"),
                edge("e2", "condition", "agent"),
                edge("e3", "condition", "end"),
                edge("e4", "agent", "end"),
            ],
        );
        for executor in [
            "CODEX",
            "CLAUDE_CODE",
            "GEMINI",
            "OH_MY_PI",
            "OPENCODE",
            "DEEPSEEK_HARNESS",
        ] {
            graph.router_executor_config = Some(serde_json::json!({
                "executor": executor,
                "variant": null
            }));
            validate_graph_for_run(&graph).unwrap();
        }
    }

    #[test]
    fn rejects_duplicate_condition_outgoing_targets() {
        let graph = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                condition_node("condition", vec![]),
                node("end", WorkflowNodeKind::End),
            ],
            vec![
                edge("e1", "start", "condition"),
                edge("e2", "condition", "end"),
                edge("e3", "condition", "end"),
            ],
        );

        let err = validate_graph(&graph).unwrap_err();
        assert!(err.to_string().contains("duplicate outgoing target"));
    }

    #[test]
    fn run_validation_rejects_missing_stale_and_empty_condition_branches() {
        let mut missing = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                condition_node("condition", vec![]),
                node("end", WorkflowNodeKind::End),
            ],
            vec![
                edge("e1", "start", "condition"),
                edge("e2", "condition", "end"),
            ],
        );
        missing.router_executor_config = Some(serde_json::json!({"executor": "CODEX"}));

        let err = validate_graph_for_run(&missing).unwrap_err();
        assert!(err.to_string().contains("missing branch config"));

        let mut stale = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                condition_node(
                    "condition",
                    vec![ConditionBranch {
                        target_node_id: Some("missing".to_string()),
                        condition: Some("Go missing".to_string()),
                        ..ConditionBranch::default()
                    }],
                ),
                node("end", WorkflowNodeKind::End),
            ],
            vec![
                edge("e1", "start", "condition"),
                edge("e2", "condition", "end"),
            ],
        );
        stale.router_executor_config = Some(serde_json::json!({"executor": "CODEX"}));

        let err = validate_graph_for_run(&stale).unwrap_err();
        assert!(err.to_string().contains("stale branch target"));

        let mut empty = graph(
            vec![
                node("start", WorkflowNodeKind::Start),
                condition_node(
                    "condition",
                    vec![ConditionBranch {
                        target_node_id: Some("end".to_string()),
                        condition: Some(" ".to_string()),
                        ..ConditionBranch::default()
                    }],
                ),
                node("end", WorkflowNodeKind::End),
            ],
            vec![
                edge("e1", "start", "condition"),
                edge("e2", "condition", "end"),
            ],
        );
        empty.router_executor_config = Some(serde_json::json!({"executor": "CODEX"}));

        let err = validate_graph_for_run(&empty).unwrap_err();
        assert!(err.to_string().contains("empty branch condition"));
    }
}
