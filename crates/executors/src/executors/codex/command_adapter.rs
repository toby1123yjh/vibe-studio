//! Codex-native command construction for the app-server transport.

use uuid::Uuid;

use super::Codex;
use crate::{
    command::{CommandBuildError, CommandBuilder, CommandParts, apply_overrides},
    executors::provider_adapter::{DirectControl, encode_stdio_rpc},
    workflow_mcp::{FORWARDED_ENV_KEYS, ScopedWorkflowMcp},
};

pub struct CodexCommandAdapter<'a> {
    agent: &'a Codex,
}

/// App-server CLI overrides merge this uniquely named server into native
/// settings. Only environment names, never the scoped credential, reach argv.
pub(crate) fn append_workflow_mcp(
    parts: CommandParts,
    config: Option<&ScopedWorkflowMcp>,
) -> CommandParts {
    let Some(config) = config else {
        return parts;
    };
    let command =
        serde_json::to_string(&config.executable.to_string_lossy()).expect("path serializes");
    let args = serde_json::to_string(&config.args()).expect("arguments serialize");
    let env_vars = serde_json::to_string(&FORWARDED_ENV_KEYS).expect("environment names serialize");
    parts.extend_args(["-c".into(), format!("mcp_servers.{}={{command={command},args={args},env_vars={env_vars},enabled=true,required=true}}", config.server_name)])
}

impl<'a> CodexCommandAdapter<'a> {
    pub fn new(agent: &'a Codex) -> Self {
        Self { agent }
    }

    pub fn build(&self) -> Result<CommandBuilder, CommandBuildError> {
        if matches!(
            self.agent.ask_for_approval,
            Some(super::AskForApproval::UnlessTrusted)
        ) {
            return Err(CommandBuildError::InvalidBase(
                "Codex no longer supports untrusted approval; select on-request explicitly".into(),
            ));
        }
        if self
            .agent
            .service_tier
            .as_deref()
            .is_some_and(|tier| !matches!(tier, "default" | "priority" | "flex"))
        {
            return Err(CommandBuildError::InvalidBase(
                "Unsupported Codex service tier".into(),
            ));
        }
        let mut builder = CommandBuilder::new(Codex::base_command())
            .extend_params(["app-server"])
            .extend_params(Codex::DISABLE_NATIVE_MEMORY_ARGS);
        if self.agent.oss.unwrap_or(false) {
            builder = builder.extend_params(["--oss"]);
        }

        // Profile parameters are applied last so an advanced user can
        // explicitly opt back into native memory for this executor profile.
        apply_overrides(builder, &self.agent.cmd)
    }
}

pub(crate) fn encode_control(control: DirectControl) -> Result<Vec<u8>, serde_json::Error> {
    let request = match control {
        DirectControl::Cancel => serde_json::json!({
            "jsonrpc":"2.0", "id": Uuid::new_v4().to_string(),
            "method":"turn/interrupt",
            "params":{"threadId":"<active-thread>","turnId":"<active-turn>"}
        }),
        DirectControl::Steer { text } => serde_json::json!({
            "jsonrpc":"2.0", "id": Uuid::new_v4().to_string(),
            "method":"turn/steer",
            "params":{"threadId":"<active-thread>","expectedTurnId":"<active-turn>","input":[{"type":"text","text":text}]}
        }),
        DirectControl::Approve { .. } | DirectControl::Input { .. } => {
            return Err(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Codex approval/input responses require the original server request id",
            )));
        }
    };
    encode_stdio_rpc(&request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executors::provider_adapter::DirectControl;

    #[test]
    fn owns_codex_app_server_launch_shape() {
        let agent: Codex = serde_json::from_value(serde_json::json!({"oss": true})).unwrap();
        let builder = CodexCommandAdapter::new(&agent).build().unwrap();
        let params = builder.params.unwrap();

        assert_eq!(builder.base, "codex");
        assert_eq!(params.first().map(String::as_str), Some("app-server"));
        assert_eq!(params.last().map(String::as_str), Some("--oss"));
    }

    #[test]
    fn unsupported_saved_policy_does_not_silently_expand_permissions() {
        let agent: Codex =
            serde_json::from_value(serde_json::json!({"ask_for_approval":"unless-trusted"}))
                .unwrap();
        assert!(CodexCommandAdapter::new(&agent).build().is_err());
    }

    #[test]
    fn owns_codex_control_encoding() {
        let value: serde_json::Value =
            serde_json::from_slice(&encode_control(DirectControl::Cancel).unwrap()).unwrap();

        assert_eq!(value["method"], "turn/interrupt");
        assert!(value["id"].is_string());
        assert!(value["params"]["threadId"].is_string());
    }

    #[test]
    fn applies_codex_command_overrides_after_native_args() {
        let agent: Codex = serde_json::from_value(serde_json::json!({
            "additional_params": ["--profile-flag"]
        }))
        .unwrap();
        let params = CodexCommandAdapter::new(&agent)
            .build()
            .unwrap()
            .params
            .unwrap();

        assert_eq!(params.last().map(String::as_str), Some("--profile-flag"));
    }

    #[test]
    fn workflow_mcp_uses_native_cli_merge_without_secret_values() {
        let executable = tempfile::NamedTempFile::new().unwrap();
        let env = crate::workflow_mcp::test_env(executable.path());
        let config = ScopedWorkflowMcp::from_execution_env(&env)
            .unwrap()
            .unwrap();
        let parts = append_workflow_mcp(
            CommandParts::new("codex".into(), vec!["app-server".into()]),
            Some(&config),
        );
        let debug = format!("{parts:?}");
        assert!(debug.contains("mcp_servers.vk_"));
        assert!(debug.contains("env_vars"));
        assert!(debug.contains(crate::workflow_mcp::TOKEN_ENV));
        assert!(!debug.contains(env.get(crate::workflow_mcp::TOKEN_ENV).unwrap()));
        assert!(debug.contains("required=true"));
    }
}
