use std::{error::Error, fmt};

use crate::{
    graph::{WorkflowEdge, WorkflowNode, WorkflowNodeKind},
    transform::{TransformError, apply_transform},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeHandlerStatus {
    Succeeded,
    AwaitingHuman,
    AwaitingArena,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamOutput {
    pub node_id: String,
    pub output_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHandlerContext {
    pub run_input_text: String,
    pub upstream_outputs: Vec<UpstreamOutput>,
}

impl NodeHandlerContext {
    pub fn from_run_input(run_input_text: impl Into<String>) -> Self {
        Self {
            run_input_text: run_input_text.into(),
            upstream_outputs: Vec::new(),
        }
    }

    pub fn with_upstream_outputs(
        run_input_text: impl Into<String>,
        upstream_outputs: Vec<UpstreamOutput>,
    ) -> Self {
        Self {
            run_input_text: run_input_text.into(),
            upstream_outputs,
        }
    }

    pub fn upstream_text(&self) -> String {
        if self.upstream_outputs.is_empty() {
            return self.run_input_text.clone();
        }

        self.upstream_outputs
            .iter()
            .map(|output| output.output_text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHandlerOutcome {
    pub status: NodeHandlerStatus,
    pub output_text: Option<String>,
    pub selected_target_node_ids: Vec<String>,
    pub skipped_target_node_ids: Vec<String>,
    pub prompt: Option<String>,
}

impl NodeHandlerOutcome {
    fn succeeded(output_text: impl Into<String>) -> Self {
        Self {
            status: NodeHandlerStatus::Succeeded,
            output_text: Some(output_text.into()),
            selected_target_node_ids: Vec::new(),
            skipped_target_node_ids: Vec::new(),
            prompt: None,
        }
    }

    fn waiting(status: NodeHandlerStatus, prompt: Option<String>) -> Self {
        Self {
            status,
            output_text: None,
            selected_target_node_ids: Vec::new(),
            skipped_target_node_ids: Vec::new(),
            prompt,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerError {
    message: String,
}

impl HandlerError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<TransformError> for HandlerError {
    fn from(error: TransformError) -> Self {
        Self::new(error.to_string())
    }
}

impl fmt::Display for HandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for HandlerError {}

pub fn handle_pure_node(
    node: &WorkflowNode,
    _outgoing_edges: &[WorkflowEdge],
    context: &NodeHandlerContext,
) -> Result<NodeHandlerOutcome, HandlerError> {
    match node.kind {
        WorkflowNodeKind::Start => Ok(handle_start(context)),
        WorkflowNodeKind::End => Ok(handle_end(context)),
        WorkflowNodeKind::Condition => Err(HandlerError::new(
            "condition nodes require the workflow condition router and are not pure handlers",
        )),
        WorkflowNodeKind::Transform => handle_transform(node, context),
        WorkflowNodeKind::HumanGate => Ok(handle_human_gate(node)),
        WorkflowNodeKind::Arena => Ok(handle_arena(node)),
        WorkflowNodeKind::Agent | WorkflowNodeKind::TaskSplitter => Err(HandlerError::new(
            "agent nodes require an executor port and are not pure handlers",
        )),
    }
}

pub fn handle_start(context: &NodeHandlerContext) -> NodeHandlerOutcome {
    NodeHandlerOutcome::succeeded(context.run_input_text.clone())
}

pub fn handle_end(context: &NodeHandlerContext) -> NodeHandlerOutcome {
    NodeHandlerOutcome::succeeded(context.upstream_text())
}

pub fn handle_transform(
    node: &WorkflowNode,
    context: &NodeHandlerContext,
) -> Result<NodeHandlerOutcome, HandlerError> {
    Ok(NodeHandlerOutcome::succeeded(apply_transform(
        &node.data,
        &context.upstream_text(),
    )?))
}

pub fn handle_human_gate(node: &WorkflowNode) -> NodeHandlerOutcome {
    NodeHandlerOutcome::waiting(
        NodeHandlerStatus::AwaitingHuman,
        node.data.prompt_to_human.clone(),
    )
}

pub fn handle_arena(node: &WorkflowNode) -> NodeHandlerOutcome {
    NodeHandlerOutcome::waiting(
        NodeHandlerStatus::AwaitingArena,
        node.data.prompt_template.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{HumanGateAction, TransformMode, WorkflowNodeData};

    fn node(id: &str, kind: WorkflowNodeKind, data: WorkflowNodeData) -> WorkflowNode {
        WorkflowNode {
            id: id.to_string(),
            kind,
            data,
            position: None,
        }
    }

    fn upstream(node_id: &str, output_text: &str) -> UpstreamOutput {
        UpstreamOutput {
            node_id: node_id.to_string(),
            output_text: output_text.to_string(),
        }
    }

    #[test]
    fn start_outputs_run_input_text() {
        let context = NodeHandlerContext::from_run_input("implement issue");

        let outcome = handle_start(&context);

        assert_eq!(outcome.status, NodeHandlerStatus::Succeeded);
        assert_eq!(outcome.output_text.as_deref(), Some("implement issue"));
    }

    #[test]
    fn end_combines_upstream_outputs() {
        let context = NodeHandlerContext::with_upstream_outputs(
            "run input",
            vec![upstream("a", "first"), upstream("b", "second")],
        );

        let outcome = handle_end(&context);

        assert_eq!(outcome.output_text.as_deref(), Some("first\n\nsecond"));
    }

    #[test]
    fn transform_template_wraps_upstream_text() {
        let transform = node(
            "transform",
            WorkflowNodeKind::Transform,
            WorkflowNodeData {
                mode: Some(TransformMode::Template),
                template: Some("Summary: {{input}}".to_string()),
                ..WorkflowNodeData::default()
            },
        );
        let context = NodeHandlerContext::with_upstream_outputs(
            "run input",
            vec![upstream("agent", "agent output")],
        );

        let outcome = handle_transform(&transform, &context).unwrap();

        assert_eq!(
            outcome.output_text.as_deref(),
            Some("Summary: agent output")
        );
    }

    #[test]
    fn transform_regex_extract_returns_first_capture() {
        let transform = node(
            "transform",
            WorkflowNodeKind::Transform,
            WorkflowNodeData {
                mode: Some(TransformMode::RegexExtract),
                regex: Some("PR-(\\d+)".to_string()),
                ..WorkflowNodeData::default()
            },
        );
        let context = NodeHandlerContext::with_upstream_outputs(
            "run input",
            vec![upstream("agent", "created PR-123")],
        );

        let outcome = handle_transform(&transform, &context).unwrap();

        assert_eq!(outcome.output_text.as_deref(), Some("123"));
    }

    #[test]
    fn transform_truncate_respects_character_limit() {
        let transform = node(
            "transform",
            WorkflowNodeKind::Transform,
            WorkflowNodeData {
                mode: Some(TransformMode::Truncate),
                max_chars: Some(3),
                ..WorkflowNodeData::default()
            },
        );
        let context =
            NodeHandlerContext::with_upstream_outputs("run input", vec![upstream("agent", "åbcd")]);

        let outcome = handle_transform(&transform, &context).unwrap();

        assert_eq!(outcome.output_text.as_deref(), Some("åbc"));
    }

    #[test]
    fn human_gate_returns_paused_state() {
        let gate = node(
            "gate",
            WorkflowNodeKind::HumanGate,
            WorkflowNodeData {
                prompt_to_human: Some("Approve?".to_string()),
                required_action: Some(HumanGateAction::ApproveOrReject),
                ..WorkflowNodeData::default()
            },
        );

        let outcome = handle_human_gate(&gate);

        assert_eq!(outcome.status, NodeHandlerStatus::AwaitingHuman);
        assert_eq!(outcome.prompt.as_deref(), Some("Approve?"));
    }

    #[test]
    fn arena_returns_awaiting_arena_state() {
        let arena = node(
            "arena",
            WorkflowNodeKind::Arena,
            WorkflowNodeData {
                prompt_template: Some("Build candidates".to_string()),
                ..WorkflowNodeData::default()
            },
        );

        let outcome = handle_arena(&arena);

        assert_eq!(outcome.status, NodeHandlerStatus::AwaitingArena);
        assert_eq!(outcome.prompt.as_deref(), Some("Build candidates"));
    }
}
