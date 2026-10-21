//! Oh My Pi-native command construction.

use std::path::Path;

use serde_json::Value;
use uuid::Uuid;

use super::OhMyPi;
use crate::{
    command::{CommandBuildError, CommandBuilder, apply_overrides},
    executors::provider_adapter::{DirectControl, DirectIntent, encode_stdio_rpc},
    workflow_mcp::ScopedWorkflowMcp,
};

pub struct OhMyPiCommandAdapter<'a> {
    agent: &'a OhMyPi,
}

impl<'a> OhMyPiCommandAdapter<'a> {
    pub fn new(agent: &'a OhMyPi) -> Self {
        Self { agent }
    }

    pub fn build(
        &self,
        intent: DirectIntent,
        session_id: Option<&str>,
    ) -> Result<CommandBuilder, CommandBuildError> {
        let mut builder = CommandBuilder::new(OhMyPi::DEFAULT_BASE_COMMAND)
            .extend_params(["--mode", OhMyPi::RUNTIME_MODE]);
        if matches!(
            intent,
            DirectIntent::FollowUp | DirectIntent::Resume | DirectIntent::Review
        ) && let Some(session_id) = session_id
        {
            builder = builder.extend_params(["--resume", session_id]);
        }
        if let Some(model) = self.agent.model.as_deref() {
            builder = builder.extend_params(["--model", model]);
        }
        apply_overrides(builder, &self.agent.cmd)
    }
}

pub(crate) fn append_workflow_plugin(
    builder: CommandBuilder,
    plugin: Option<&Path>,
) -> CommandBuilder {
    match plugin {
        Some(path) => builder.extend_params([
            "--plugin-dir".to_owned(),
            path.to_string_lossy().into_owned(),
        ]),
        None => builder,
    }
}

/// OMP supports legacy Claude-format execution plugins. The portable Agent
/// Plugins format deliberately forbids absolute commands and treats most env
/// values as literals, so it is NOT appropriate for the bundled executable or
/// credential placeholders. Never write an entry to the user's global config.
pub(crate) fn create_workflow_plugin(
    config: &ScopedWorkflowMcp,
) -> Result<tempfile::TempDir, std::io::Error> {
    let plugin = tempfile::Builder::new()
        .prefix("vk-workflow-mcp-")
        .tempdir()?;
    let manifest_dir = plugin.path().join(".claude-plugin");
    std::fs::create_dir(&manifest_dir)?;
    // OMP namespaces servers as <plugin>:<server>, then mints model-facing
    // mcp__<sanitized-server>_<tool> names with a hard 64-character limit.
    // Retain the full execution UUID while shortening both namespace parts;
    // otherwise the context tool's meaningful name is truncated and hashed.
    let execution_id = config
        .server_name
        .strip_prefix(crate::workflow_mcp::SERVER_NAME_PREFIX)
        .filter(|value| value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| std::io::Error::other("Invalid workflow MCP execution namespace"))?;
    let manifest = serde_json::json!({"name":format!("vk-{execution_id}"), "version":env!("CARGO_PKG_VERSION")});
    let native = serde_json::json!({"mcpServers": {"wf": {
        "type":"stdio", "command":config.executable, "args":config.args(),
        "env":config.environment_placeholders(), "enabled":true
    }}});
    std::fs::write(
        manifest_dir.join("plugin.json"),
        serde_json::to_vec(&manifest)?,
    )?;
    std::fs::write(
        plugin.path().join(".mcp.json"),
        serde_json::to_vec(&native)?,
    )?;
    Ok(plugin)
}

pub(crate) fn encode_control(control: DirectControl) -> Result<Vec<u8>, serde_json::Error> {
    let request = match control {
        DirectControl::Cancel => serde_json::json!({"type":"abort"}),
        DirectControl::Approve { .. }
        | DirectControl::Input { .. }
        | DirectControl::ReplyInteraction { .. } => {
            return Err(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Oh My Pi RPC does not expose host approval/input responses",
            )));
        }
        DirectControl::Steer { text } => serde_json::json!({"type":"steer","message":text}),
    };
    encode_stdio_rpc(&request)
}

pub(crate) fn session_request(prompt: &str, session_id: Option<&str>) -> Value {
    let _ = session_id;
    serde_json::json!({"id": Uuid::new_v4().to_string(), "type":"prompt", "message":prompt})
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::executors::provider_adapter::{DirectControl, DirectIntent};

    #[test]
    fn owns_omp_rpc_launch_shape() {
        let agent = OhMyPi::default();
        let builder = OhMyPiCommandAdapter::new(&agent)
            .build(DirectIntent::Resume, Some("session-1"))
            .unwrap();

        assert_eq!(builder.base, "omp");
        assert_eq!(
            builder.params.unwrap(),
            vec![
                "--mode".to_string(),
                "rpc".to_string(),
                "--resume".to_string(),
                "session-1".to_string()
            ]
        );
    }

    #[test]
    fn owns_omp_session_and_control_encoding() {
        let resume = session_request("continue", Some("session-1"));
        let cancel: Value =
            serde_json::from_slice(&encode_control(DirectControl::Cancel).unwrap()).unwrap();

        assert_eq!(resume["type"], "prompt");
        assert_eq!(resume["message"], "continue");
        assert_eq!(cancel["type"], "abort");
    }

    #[test]
    fn applies_omp_command_overrides_after_native_args() {
        let agent: OhMyPi = serde_json::from_value(serde_json::json!({
            "additional_params": ["--profile-flag"]
        }))
        .unwrap();
        let params = OhMyPiCommandAdapter::new(&agent)
            .build(DirectIntent::Initial, None)
            .unwrap()
            .params
            .unwrap();

        assert_eq!(params.last().map(String::as_str), Some("--profile-flag"));
    }

    #[test]
    fn workflow_plugin_is_ephemeral_native_config_without_secret_material() {
        let executable = tempfile::NamedTempFile::new().unwrap();
        let env = crate::workflow_mcp::test_env(executable.path());
        let config = ScopedWorkflowMcp::from_execution_env(&env)
            .unwrap()
            .unwrap();
        let plugin = create_workflow_plugin(&config).unwrap();
        let path = plugin.path().to_owned();
        let native = std::fs::read_to_string(path.join(".mcp.json")).unwrap();
        assert!(native.contains("${MCP_WORKFLOW_TOKEN}"));
        assert!(!native.contains(env.get(crate::workflow_mcp::TOKEN_ENV).unwrap()));
        assert!(path.join(".claude-plugin/plugin.json").is_file());
        assert!(!path.join("plugin.json").exists());
        let manifest: Value = serde_json::from_slice(
            &std::fs::read(path.join(".claude-plugin/plugin.json")).unwrap(),
        )
        .unwrap();
        let plugin_name = manifest["name"].as_str().unwrap();
        assert_eq!(
            plugin_name,
            format!(
                "vk-{}",
                config
                    .server_name
                    .strip_prefix(crate::workflow_mcp::SERVER_NAME_PREFIX)
                    .unwrap()
            )
        );
        let native_value: Value = serde_json::from_str(&native).unwrap();
        assert!(native_value["mcpServers"].get("wf").is_some());
        let namespace = format!("{}_wf", plugin_name.replace('-', "_"));
        for tool in crate::workflow_mcp::WORKFLOW_TOOL_NAMES {
            // Also allow the documented double-separator alias: neither native
            // spelling needs OMP's truncation/hash fallback.
            assert!(format!("mcp__{namespace}__{tool}").len() <= 64);
        }
        let builder = append_workflow_plugin(CommandBuilder::new("omp"), Some(&path));
        assert_eq!(builder.params.as_ref().unwrap()[0], "--plugin-dir");
        drop(plugin);
        assert!(!path.exists());
    }
}
