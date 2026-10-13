use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    schemars, tool, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{McpMode, McpServer, ToolCallResult};

#[derive(Debug, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkflowPageRequest {
    cursor: Option<u64>,
    limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkflowGetRequest {
    #[schemars(
        description = "Optional exact Run ID in this Session's bound instance; omit for current facts"
    )]
    run_id: Option<Uuid>,
    cursor: Option<u64>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkflowAction {
    Start,
    Retry,
    Rework,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum WorkflowScope {
    All,
    FromNodes { node_ids: Vec<String> },
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkflowActivePolicy {
    AfterCurrent,
    StopThenRun,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkflowDeliveryIntent {
    Full,
    Partial,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkflowSubmission {
    #[schemars(
        description = "Stable nonempty request key (max 200 characters). Reuse it if the reply is lost; do not submit another key to retry transport."
    )]
    request_id: String,
    action: WorkflowAction,
    input_text: Option<String>,
    #[serde(default)]
    material_paths: Vec<String>,
    #[schemars(
        description = "Exact latest accepted Run required for retry/rework. Read workflow_get first."
    )]
    source_run_id: Option<Uuid>,
    source_node_execution_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<WorkflowScope>,
    #[schemars(
        description = "Required for rework of unfinished work: wait for current work, or stop safely then execute the accepted replacement."
    )]
    active_policy: Option<WorkflowActivePolicy>,
    #[schemars(
        description = "Optional real user input message ID from this turn, never a runtime notification. Backend verifies provenance."
    )]
    source_message_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Use partial only for explicit summary of completed split work. This preserves all failed/missing items and never cancels other work."
    )]
    delivery_intent: Option<WorkflowDeliveryIntent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Exact accepted source split batch required for partial delivery.")]
    source_batch_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkflowDispatchRequest {
    #[schemars(description = "Stable request key; reuse it when a reply is lost")]
    request_id: String,
    batch_id: Uuid,
    #[schemars(description = "Project-relative path to the file-backed dispatch.json")]
    dispatch_path: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkflowStopRequest {
    run_id: Uuid,
    request_id: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkflowRespondRequest {
    run_id: Uuid,
    node_execution_id: Uuid,
    request_id: String,
    action: WorkflowHumanAction,
    reason: Option<String>,
    selected_target_node_ids: Option<Vec<String>>,
    candidate_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkflowHumanAction {
    Approve,
    Reject,
    SelectBranch,
    SelectArenaWinner,
}

#[derive(Debug)]
pub(crate) struct WorkflowToolError {
    pub code: String,
    pub message: String,
    details: Option<Value>,
}

impl WorkflowToolError {
    fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            details: None,
        }
    }

    fn result(self) -> CallToolResult {
        CallToolResult::error(vec![Content::text(
            serde_json::json!({
                "success": false, "code": self.code, "message": self.message,
                "error_data": self.details,
            })
            .to_string(),
        )])
    }
}

#[tool_router(router = workflow_tools_router, vis = "pub")]
impl McpServer {
    #[tool(
        description = "Read verified local Project, main Session, publication and bound Task/instance. This read never creates work or starts an Agent."
    )]
    async fn workflow_context(&self) -> ToolCallResult {
        self.workflow_tool("workflow_context", &serde_json::json!({}))
            .await
    }

    #[tool(
        description = "List callable saved workflows in verified scope. A bound instance cannot switch its workflow. Listing never submits execution."
    )]
    async fn workflow_list(
        &self,
        Parameters(request): Parameters<WorkflowPageRequest>,
    ) -> ToolCallResult {
        self.workflow_tool("workflow_list", &request).await
    }

    #[tool(
        description = "Inspect this Session's instance, frozen graph, exact Node execution/reuse history, outcomes, current human interactions and persisted system notifications. Read only."
    )]
    async fn workflow_get(
        &self,
        Parameters(request): Parameters<WorkflowGetRequest>,
    ) -> ToolCallResult {
        self.workflow_tool("workflow_get", &request).await
    }

    #[tool(
        description = "Accept an explicit user execution request asynchronously: initial start, exact failed-Node retry, or full/partial delivery rework. Runtime validates and schedules affected downstream Nodes. Return is acceptance, not completion. Preserve request_id on uncertain replies."
    )]
    async fn workflow_submit(
        &self,
        Parameters(request): Parameters<WorkflowSubmission>,
    ) -> ToolCallResult {
        self.workflow_tool("workflow_submit", &request).await
    }

    #[tool(
        description = "Accept the file-backed dispatch plan for one task-splitter batch. The runtime validates tasks.json, frozen graph scope, hashes and dependencies before scheduling; writing the file alone never starts work."
    )]
    async fn workflow_dispatch(
        &self,
        Parameters(request): Parameters<WorkflowDispatchRequest>,
    ) -> ToolCallResult {
        self.workflow_tool("workflow_dispatch", &request).await
    }

    #[tool(
        description = "Stop the exact bound Run and cancel its same-instance not-yet-started dependent followups. Other Issues are unaffected. Requested/unreachable stop is not confirmed exit. This never creates replacement work."
    )]
    async fn workflow_stop(
        &self,
        Parameters(request): Parameters<WorkflowStopRequest>,
    ) -> ToolCallResult {
        self.workflow_tool("workflow_stop", &request).await
    }

    #[tool(
        description = "Respond to an authorised current workflow human interaction, naming its exact Run and NodeExecution. Tool possession is not blanket authority to approve; expired or already resolved interactions are rejected."
    )]
    async fn workflow_respond(
        &self,
        Parameters(request): Parameters<WorkflowRespondRequest>,
    ) -> ToolCallResult {
        self.workflow_tool("workflow_respond", &request).await
    }
}

impl McpServer {
    async fn workflow_tool(&self, tool: &str, body: &impl Serialize) -> ToolCallResult {
        match self.workflow_request(tool, body).await {
            Ok(data) => Self::success(&data),
            Err(error) => Ok(error.result()),
        }
    }

    pub(crate) async fn workflow_request(
        &self,
        tool: &str,
        body: &impl Serialize,
    ) -> Result<Value, WorkflowToolError> {
        let McpMode::Workflow(launch) = self.mode() else {
            return Err(WorkflowToolError::new(
                "workflow_context_unavailable",
                "An explicit workflow execution binding is required",
            ));
        };
        let response = self.client.post(self.url(&format!("/api/workflow-management/mcp/{tool}")))
            .bearer_auth(&launch.token)
            .header("X-Workflow-Session-Id", launch.session_id.to_string())
            .header("X-Workflow-Agent-Run-Id", launch.agent_run_id.to_string())
            .header("X-Workflow-Turn-Id", launch.turn_id.to_string())
            .json(body).send().await.map_err(|_| WorkflowToolError::new(
                "workflow_backend_unavailable", "Workflow backend reply unavailable; execution outcome may be unknown. Retry with the same request_id."
            ))?;
        let status = response.status();
        let value = response.json::<Value>().await.map_err(|_| WorkflowToolError::new(
            "workflow_backend_invalid_reply", "Workflow backend returned an invalid reply; do not assume an execution was rejected. Retry with the same request_id."
        ))?;
        decode_workflow_reply(tool, status.is_success(), value)
    }
}

fn decode_workflow_reply(
    tool: &str,
    http_success: bool,
    value: Value,
) -> Result<Value, WorkflowToolError> {
    if http_success && value.get("success").and_then(Value::as_bool) == Some(true) {
        // Before the first formal submission, this Session has no instance.
        // That is a valid workflow_get result, not an unavailable binding.
        return value
            .get("data")
            .filter(|data| !data.is_null() || tool == "workflow_get")
            .cloned()
            .ok_or_else(|| {
                WorkflowToolError::new(
                    "workflow_backend_invalid_reply",
                    "Workflow backend reply is missing data",
                )
            });
    }
    let detail = value.get("error_data").filter(|value| !value.is_null());
    let error = detail.unwrap_or(&value);
    Err(WorkflowToolError {
        code: error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("workflow_backend_error")
            .to_owned(),
        message: error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| value.get("message").and_then(Value::as_str))
            .unwrap_or("Workflow backend rejected this operation")
            .to_owned(),
        details: detail.cloned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_structured_errors_even_on_non_success_http() {
        let error = decode_workflow_reply("workflow_submit", false, serde_json::json!({
            "success": false, "error_data": {"code":"stale_execution_basis", "message":"Read current execution", "run_id":"current"}
        })).unwrap_err();
        assert_eq!(error.code, "stale_execution_basis");
        assert_eq!(error.message, "Read current execution");
        assert_eq!(error.details.unwrap()["run_id"], "current");
        assert!(
            decode_workflow_reply(
                "workflow_submit",
                false,
                serde_json::json!({"success":true,"data":{}})
            )
            .is_err()
        );
    }

    #[test]
    fn submission_cannot_replace_caller_identity() {
        assert!(serde_json::from_value::<WorkflowSubmission>(serde_json::json!({
            "request_id":"test", "action":"start", "input_text":"work", "project_id":Uuid::new_v4()
        })).is_err());
        let request = serde_json::from_value::<WorkflowSubmission>(serde_json::json!({
            "request_id":"test", "action":"rework", "scope":{"type":"from_nodes","node_ids":["report"]},
            "active_policy":"stop_then_run"
        })).unwrap();
        assert_eq!(
            serde_json::to_value(request).unwrap()["scope"]["node_ids"][0],
            "report"
        );
    }

    #[test]
    fn minimal_submissions_omit_scope_instead_of_sending_null() {
        for action in ["start", "retry"] {
            let request: WorkflowSubmission = serde_json::from_value(serde_json::json!({
                "request_id":"transport-replay", "action":action,
            }))
            .unwrap();
            let value = serde_json::to_value(request).unwrap();
            assert!(value.get("scope").is_none());
            assert_eq!(value["material_paths"], serde_json::json!([]));
        }
    }

    #[test]
    fn uses_runtime_interaction_and_pagination_shapes() {
        let request: WorkflowRespondRequest = serde_json::from_value(serde_json::json!({
            "run_id":Uuid::new_v4(), "node_execution_id":Uuid::new_v4(), "request_id":"select",
            "action":"select_branch", "selected_target_node_ids":["report"]
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(request).unwrap()["selected_target_node_ids"][0],
            "report"
        );
        assert!(
            serde_json::from_value::<WorkflowPageRequest>(
                serde_json::json!({"cursor":12,"limit":20})
            )
            .is_ok()
        );
        assert!(
            serde_json::from_value::<WorkflowPageRequest>(serde_json::json!({"cursor":"12"}))
                .is_err()
        );
    }

    #[test]
    fn get_before_submission_is_empty_but_missing_context_still_fails_closed() {
        let reply = serde_json::json!({"success":true,"data":null});
        assert_eq!(
            decode_workflow_reply("workflow_get", true, reply.clone()).unwrap(),
            Value::Null
        );
        for tool in [
            "workflow_context",
            "workflow_list",
            "workflow_submit",
            "workflow_dispatch",
            "workflow_stop",
            "workflow_respond",
        ] {
            assert!(decode_workflow_reply(tool, true, reply.clone()).is_err());
        }
        assert!(
            decode_workflow_reply("workflow_get", true, serde_json::json!({"success":true}))
                .is_err()
        );
        assert!(decode_workflow_reply("workflow_get", false, reply).is_err());
    }
}
