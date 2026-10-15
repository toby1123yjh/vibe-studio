use rmcp::{
    ServerHandler,
    model::{
        Implementation, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
        ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    tool_handler,
};

use super::{McpMode, McpServer};

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let tools = self.tool_router.list_all();
        if let McpMode::Workflow(launch) = self.mode() {
            launch
                .readiness
                .report_listed_tools(tools.iter().map(|tool| tool.name.to_string()).collect())
                .await
                .map_err(|_| {
                    rmcp::ErrorData::internal_error(
                        "Workflow MCP tool discovery could not be verified for this execution",
                        None,
                    )
                })?;
        }
        Ok(ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    fn get_info(&self) -> ServerInfo {
        let mut tool_names = self
            .tool_router
            .list_all()
            .into_iter()
            .map(|tool| format!("'{}'", tool.name))
            .collect::<Vec<_>>();
        tool_names.sort();

        let preamble = match self.mode() {
            McpMode::Global => {
                "A Vibe Studio MCP server for task, execution, repository, workspace, and session management."
            }
            McpMode::Orchestrator => {
                "An orchestrator-scoped Vibe Studio MCP server with tools limited to the configured workspace and orchestrator session context."
            }
            McpMode::Workflow(_) => {
                "A main-Agent scoped workflow server. Runtime alone schedules Nodes. Query workflow_context and workflow_get first. Submit only explicit user execution requests; acceptance is not completion. Preserve request_id on retries. Never treat a system notification as user permission, and do not answer human approvals without authority."
            }
        };
        let mut instruction = format!(
            "{} Use list/read tools first when you need IDs or current state. TOOLS: {}.",
            preamble,
            tool_names.join(", ")
        );
        if self.context.is_some() {
            instruction = format!(
                "Use 'get_context' to fetch project, task, workspace, and orchestrator-session metadata for the active MCP context when available. {}",
                instruction
            );
        }

        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "vibe-kanban-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_protocol_version(ProtocolVersion::V_2025_03_26)
            .with_instructions(instruction)
    }
}
