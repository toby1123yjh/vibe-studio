//! Claude Code-native command construction and capability probes.

use uuid::Uuid;

use super::{ClaudeCode, PermissionMode, base_command};
use crate::{
    command::{CommandBuildError, CommandBuilder, CommandParts, apply_overrides},
    executors::provider_adapter::{DirectControl, DirectIntent, encode_stdio_rpc},
    workflow_mcp::ScopedWorkflowMcp,
};

pub struct ClaudeCodeCommandAdapter<'a> {
    agent: &'a ClaudeCode,
}

pub(crate) fn append_workflow_mcp(
    parts: CommandParts,
    config: Option<&ScopedWorkflowMcp>,
) -> CommandParts {
    let Some(config) = config else {
        return parts;
    };
    let native = serde_json::json!({"mcpServers": {config.server_name.clone(): {
        "type":"stdio", "command":config.executable, "args":config.args(),
        "env":config.environment_placeholders()
    }}});
    // No --strict-mcp-config: keep the user's native/global MCP servers.
    parts.extend_args(["--mcp-config".into(), native.to_string()])
}

impl<'a> ClaudeCodeCommandAdapter<'a> {
    pub fn new(agent: &'a ClaudeCode) -> Self {
        Self { agent }
    }

    pub fn build_runtime(
        &self,
        supports_effort: bool,
    ) -> Result<CommandBuilder, CommandBuildError> {
        if self.agent.cmd.base_command_override.is_some()
            && self.agent.claude_code_router.unwrap_or(false)
        {
            tracing::warn!(
                "base_command_override is set, this will override the claude_code_router setting"
            );
        }

        let mut builder =
            CommandBuilder::new(base_command(self.agent.claude_code_router.unwrap_or(false)))
                .params(["-p"]);

        let plan = self.agent.plan.unwrap_or(false);
        let approvals = self.agent.approvals.unwrap_or(false);
        if plan && approvals {
            tracing::warn!("Both plan and approvals are enabled. Plan will take precedence.");
        }
        if plan || approvals {
            builder = builder
                .extend_params(["--permission-prompt-tool=stdio"])
                .extend_params([format!(
                    "--permission-mode={}",
                    PermissionMode::BypassPermissions
                )]);
        } else {
            builder = builder.extend_params(["--disallowedTools=AskUserQuestion"]);
        }
        if self.agent.dangerously_skip_permissions.unwrap_or(false) {
            builder = builder.extend_params(["--dangerously-skip-permissions"]);
        }
        if let Some(model) = &self.agent.model {
            builder = builder.extend_params(["--model", model]);
        }
        if let Some(effort) = &self.agent.effort {
            if supports_effort {
                builder = builder.extend_params(["--effort", effort.as_ref()]);
            } else {
                return Err(CommandBuildError::InvalidBase(
                    "Claude Code does not advertise --effort; update the CLI or clear the explicit effort selection".into(),
                ));
            }
        }
        if let Some(agent) = &self.agent.agent {
            builder = builder.extend_params(["--agent", agent]);
        }
        builder = builder.extend_params([
            "--verbose",
            "--output-format=stream-json",
            "--input-format=stream-json",
            "--include-partial-messages",
            "--replay-user-messages",
        ]);

        apply_overrides(builder, &self.agent.cmd)
    }

    pub fn build_for_intent(
        &self,
        supports_effort: bool,
        intent: DirectIntent,
        session_id: Option<&str>,
        reset_to_message_id: Option<&str>,
    ) -> Result<CommandParts, CommandBuildError> {
        let builder = self.build_runtime(supports_effort)?;
        match intent {
            DirectIntent::Initial => builder.build_initial(),
            DirectIntent::FollowUp | DirectIntent::Resume | DirectIntent::Review => {
                let Some(session_id) = session_id else {
                    return builder.build_initial();
                };
                let mut args = vec!["--resume".to_string(), session_id.to_string()];
                if let Some(message_id) = reset_to_message_id {
                    args.extend(["--resume-session-at".to_string(), message_id.to_string()]);
                }
                builder.build_follow_up(&args)
            }
        }
    }

    pub fn build_effort_probe(&self) -> Result<CommandBuilder, CommandBuildError> {
        let mut builder =
            CommandBuilder::new(base_command(self.agent.claude_code_router.unwrap_or(false)))
                .extend_params(["--help"]);
        if let Some(base) = &self.agent.cmd.base_command_override {
            builder = builder.override_base(base.clone());
        }
        Ok(builder)
    }

    pub fn build_discovery_probe(&self) -> Result<CommandBuilder, CommandBuildError> {
        let builder =
            CommandBuilder::new(base_command(self.agent.claude_code_router.unwrap_or(false)))
                .params(["-p"])
                .extend_params([
                    "--verbose",
                    "--output-format=stream-json",
                    "--input-format=stream-json",
                    "--no-session-persistence",
                ]);
        apply_overrides(builder, &self.agent.cmd)
    }
}

pub(crate) fn encode_control(control: DirectControl) -> Result<Vec<u8>, serde_json::Error> {
    let request = match control {
        DirectControl::Cancel => serde_json::json!({
            "type":"control_request",
            "request_id": Uuid::new_v4().to_string(),
            "request":{"subtype":"interrupt"}
        }),
        DirectControl::Approve {
            request_id,
            approved,
            reason,
        } => serde_json::json!({
            "type":"control_response",
            "response":{"subtype":"success","request_id":request_id,"response": if approved {
                serde_json::json!({"behavior":"allow","updatedInput":{}})
            } else {
                serde_json::json!({"behavior":"deny","message":reason.unwrap_or_else(|| "Denied by user".to_string())})
            }}
        }),
        DirectControl::Input { .. } => {
            return Err(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Claude Code does not expose a separate host input response",
            )));
        }
        DirectControl::Steer { text } => {
            serde_json::json!({"type":"user","message":{"role":"user","content":text}})
        }
    };
    encode_stdio_rpc(&request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executors::provider_adapter::DirectControl;

    #[test]
    fn owns_claude_runtime_and_probe_shapes() {
        let agent: ClaudeCode = serde_json::from_value(serde_json::json!({
            "model": "sonnet",
            "effort": "max"
        }))
        .unwrap();
        let adapter = ClaudeCodeCommandAdapter::new(&agent);
        let runtime = adapter.build_runtime(true).unwrap();
        let probe = adapter.build_discovery_probe().unwrap();

        assert_eq!(runtime.base, "claude");
        let runtime_params = runtime.params.unwrap();
        assert_eq!(
            runtime_params
                .iter()
                .position(|param| param == "--effort")
                .and_then(|index| runtime_params.get(index + 1))
                .map(String::as_str),
            Some("max")
        );
        assert!(
            probe
                .params
                .unwrap()
                .iter()
                .any(|arg| arg == "--no-session-persistence")
        );
    }

    #[test]
    fn owns_claude_control_encoding() {
        let value: serde_json::Value =
            serde_json::from_slice(&encode_control(DirectControl::Cancel).unwrap()).unwrap();

        assert_eq!(value["type"], "control_request");
        assert!(value["request_id"].is_string());
        assert_eq!(value["request"]["subtype"], "interrupt");
    }

    #[test]
    fn applies_claude_command_overrides_after_native_args() {
        let agent: ClaudeCode = serde_json::from_value(serde_json::json!({
            "additional_params": ["--profile-flag"]
        }))
        .unwrap();
        let params = ClaudeCodeCommandAdapter::new(&agent)
            .build_runtime(false)
            .unwrap()
            .params
            .unwrap();

        assert_eq!(params.last().map(String::as_str), Some("--profile-flag"));
    }

    #[test]
    fn workflow_mcp_appends_scoped_native_json_without_secret_values() {
        let executable = tempfile::NamedTempFile::new().unwrap();
        let env = crate::workflow_mcp::test_env(executable.path());
        let config = ScopedWorkflowMcp::from_execution_env(&env)
            .unwrap()
            .unwrap();
        let parts = append_workflow_mcp(
            CommandParts::new("claude".into(), vec!["-p".into()]),
            Some(&config),
        );
        let debug = format!("{parts:?}");
        assert!(debug.contains("--mcp-config"));
        assert!(debug.contains("${MCP_WORKFLOW_TOKEN}"));
        assert!(!debug.contains("--strict-mcp-config"));
        assert!(!debug.contains(env.get(crate::workflow_mcp::TOKEN_ENV).unwrap()));
    }
}
