//! Gemini-native command construction.
//!
//! Gemini owns its ACP startup flags and profile-specific options here. The
//! runtime only selects the provider and passes the resulting command to the
//! ACP transport.

use super::Gemini;
use crate::{
    command::{CmdOverrides, CommandBuildError, CommandBuilder, apply_overrides},
    env::ExecutionEnv,
    executors::{ExecutorError, provider_adapter::DirectControl},
    workflow_mcp::{
        FORWARDED_ENV_KEYS, ScopedWorkflowMcp, WorkflowMcpReadiness,
        add_gemini_environment_aliases, gemini_env_alias,
    },
};

pub struct GeminiCommandAdapter<'a> {
    agent: &'a Gemini,
}

pub(crate) async fn prepare_workflow_launch(
    env: &ExecutionEnv,
    overrides: &CmdOverrides,
) -> Result<(ExecutionEnv, Option<WorkflowMcpReadiness>), ExecutorError> {
    let mut env = env.clone().with_profile(overrides);
    let readiness = WorkflowMcpReadiness::start(&mut env).await?;
    // Add trusted aliases only after profile merging. Tokens remain in the
    // provider child environment, never in the ACP request or native audit.
    add_gemini_environment_aliases(&mut env)?;
    Ok((env, readiness))
}

pub(crate) fn workflow_mcp_servers(
    env: &ExecutionEnv,
) -> Result<Vec<agent_client_protocol::McpServer>, ExecutorError> {
    let Some(config) = ScopedWorkflowMcp::from_execution_env(env)? else {
        return Ok(Vec::new());
    };
    // Gemini sanitizes TOKEN names even when expanding explicit env values.
    // GEMINI_CLI_* survives both normal and strict sanitizer policies, and
    // placeholders are expanded by Gemini, cross-platform, before MCP spawn.
    Ok(vec![agent_client_protocol::McpServer::Stdio(
        agent_client_protocol::McpServerStdio::new(&config.server_name, &config.executable)
            .args(config.args().into_iter().map(str::to_owned).collect())
            .env(
                FORWARDED_ENV_KEYS
                    .into_iter()
                    .map(|key| {
                        agent_client_protocol::EnvVariable::new(
                            key,
                            format!("${{{}}}", gemini_env_alias(key)),
                        )
                    })
                    .collect(),
            ),
    )])
}

impl<'a> GeminiCommandAdapter<'a> {
    pub fn new(agent: &'a Gemini) -> Self {
        Self { agent }
    }

    pub fn build(&self) -> Result<CommandBuilder, CommandBuildError> {
        let mut builder = CommandBuilder::new("gemini");

        if let Some(model) = &self.agent.model {
            builder = builder.extend_params(["--model", model.as_str()]);
        }

        if self.agent.yolo.unwrap_or(false) {
            builder = builder.extend_params(["--yolo"]);
        }

        apply_overrides(builder.extend_params(["--acp"]), &self.agent.cmd)
    }
}

pub(crate) fn encode_control(control: DirectControl) -> Result<Vec<u8>, serde_json::Error> {
    super::super::acp::provider::encode_control(control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executors::provider_adapter::DirectControl;

    #[test]
    fn owns_gemini_acp_launch_shape() {
        let agent: Gemini = serde_json::from_value(serde_json::json!({
            "model": "gemini-test",
            "yolo": true
        }))
        .unwrap();
        let builder = GeminiCommandAdapter::new(&agent).build().unwrap();

        assert_eq!(builder.base, "gemini");
        assert_eq!(
            builder.params.unwrap(),
            vec![
                "--model".to_string(),
                "gemini-test".to_string(),
                "--yolo".to_string(),
                "--acp".to_string(),
            ]
        );
    }

    #[test]
    fn owns_gemini_control_encoding() {
        assert!(encode_control(DirectControl::Cancel).unwrap().is_empty());
        assert!(
            encode_control(DirectControl::Steer {
                text: "continue".to_string(),
            })
            .is_err()
        );
    }

    #[test]
    fn applies_gemini_command_overrides_after_native_args() {
        let agent: Gemini = serde_json::from_value(serde_json::json!({
            "additional_params": ["--profile-flag"]
        }))
        .unwrap();
        let params = GeminiCommandAdapter::new(&agent)
            .build()
            .unwrap()
            .params
            .unwrap();

        assert_eq!(params.last().map(String::as_str), Some("--profile-flag"));
    }

    #[test]
    fn workflow_mcp_is_carried_in_acp_without_credentials() {
        let executable = tempfile::NamedTempFile::new().unwrap();
        let env = crate::workflow_mcp::test_env(executable.path());
        let servers = workflow_mcp_servers(&env).unwrap();
        assert_eq!(servers.len(), 1);
        let wire = serde_json::to_value(
            agent_client_protocol::NewSessionRequest::new(std::env::temp_dir())
                .mcp_servers(servers.clone()),
        )
        .unwrap();
        assert_eq!(
            wire["mcpServers"][0]["args"],
            serde_json::json!(["--mode", "workflow"])
        );
        let env_vars = wire["mcpServers"][0]["env"].as_array().unwrap();
        assert_eq!(env_vars.len(), FORWARDED_ENV_KEYS.len());
        let token = env_vars
            .iter()
            .find(|entry| entry["name"] == crate::workflow_mcp::TOKEN_ENV)
            .unwrap();
        assert_eq!(token["value"], "${GEMINI_CLI_VK_MCP_WORKFLOW_TOKEN}");
        let loaded = serde_json::to_value(
            agent_client_protocol::LoadSessionRequest::new(
                agent_client_protocol::SessionId::new("native"),
                std::env::temp_dir(),
            )
            .mcp_servers(servers),
        )
        .unwrap();
        assert_eq!(loaded["mcpServers"], wire["mcpServers"]);
        assert!(
            !wire
                .to_string()
                .contains(env.get(crate::workflow_mcp::TOKEN_ENV).unwrap())
        );
    }

    #[tokio::test]
    async fn workflow_gemini_aliases_survive_sanitization_without_native_credential_values() {
        let executable = tempfile::NamedTempFile::new().unwrap();
        let env = crate::workflow_mcp::test_env(executable.path());
        let overrides = CmdOverrides {
            env: Some(std::collections::HashMap::from([
                (
                    gemini_env_alias(crate::workflow_mcp::TOKEN_ENV),
                    "forged".to_owned(),
                ),
                (
                    crate::workflow_mcp::READY_ADDRESS_ENV.to_owned(),
                    "192.0.2.1:1234".to_owned(),
                ),
            ])),
            ..Default::default()
        };
        let (env, readiness) = prepare_workflow_launch(&env, &overrides).await.unwrap();
        assert!(readiness.is_some());
        for key in FORWARDED_ENV_KEYS {
            assert_eq!(env.get(&gemini_env_alias(key)), env.get(key));
        }
        let serialized = serde_json::to_string(&workflow_mcp_servers(&env).unwrap()).unwrap();
        assert!(!serialized.contains(env.get(crate::workflow_mcp::TOKEN_ENV).unwrap()));
        assert!(!format!("{env:?}").contains(env.get(crate::workflow_mcp::TOKEN_ENV).unwrap()));
    }
}
