//! Execution-only workflow MCP context. No credential is serialized into a
//! provider configuration: adapters forward names/placeholders through native
//! MCP launch settings while the secret travels only in the child environment.
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{env::ExecutionEnv, executors::ExecutorError, profile::ExecutorConfig};

mod readiness;
pub use readiness::{WorkflowMcpReadiness, WorkflowMcpReadyReporter, WorkflowPromptGate};

pub const SESSION_ID_ENV: &str = "MCP_WORKFLOW_SESSION_ID";
pub const AGENT_RUN_ID_ENV: &str = "MCP_WORKFLOW_AGENT_RUN_ID";
pub const TURN_ID_ENV: &str = "MCP_WORKFLOW_TURN_ID";
pub const TOKEN_ENV: &str = "MCP_WORKFLOW_TOKEN";
pub const EXECUTABLE_ENV: &str = "VIBE_WORKFLOW_MCP_EXECUTABLE";
pub const BACKEND_URL_ENV: &str = "VIBE_BACKEND_URL";
pub const READY_ADDRESS_ENV: &str = "MCP_WORKFLOW_READY_ADDRESS";
pub(crate) const SERVER_NAME_PREFIX: &str = "vk_";
pub const CONTEXT_ENV_KEYS: [&str; 6] = [
    SESSION_ID_ENV,
    AGENT_RUN_ID_ENV,
    TURN_ID_ENV,
    TOKEN_ENV,
    EXECUTABLE_ENV,
    READY_ADDRESS_ENV,
];
pub const FORWARDED_ENV_KEYS: [&str; 6] = [
    BACKEND_URL_ENV,
    SESSION_ID_ENV,
    AGENT_RUN_ID_ENV,
    TURN_ID_ENV,
    TOKEN_ENV,
    READY_ADDRESS_ENV,
];
pub const WORKFLOW_TOOL_NAMES: [&str; 7] = [
    "workflow_context",
    "workflow_list",
    "workflow_get",
    "workflow_submit",
    "workflow_dispatch",
    "workflow_stop",
    "workflow_respond",
];

/// Gemini's sanitizer explicitly preserves GEMINI_CLI_* variables, including
/// under its strict GitHub policy. Native MCP env expansion uses that sanitized
/// environment, so an original TOKEN name cannot be forwarded directly.
pub fn gemini_env_alias(key: &str) -> String {
    format!("GEMINI_CLI_VK_{key}")
}

pub fn all_context_env_keys() -> impl Iterator<Item = String> {
    CONTEXT_ENV_KEYS
        .into_iter()
        .map(str::to_owned)
        .chain(FORWARDED_ENV_KEYS.into_iter().map(gemini_env_alias))
}

pub fn is_context_env_key(key: &str) -> bool {
    all_context_env_keys().any(|reserved| key.eq_ignore_ascii_case(&reserved))
}

pub fn is_token_env_key(key: &str) -> bool {
    key.eq_ignore_ascii_case(TOKEN_ENV) || key.eq_ignore_ascii_case(&gemini_env_alias(TOKEN_ENV))
}

pub fn remove_inherited_context(command: &mut tokio::process::Command) {
    for key in all_context_env_keys() {
        command.env_remove(key);
    }
    // Environment names are case-sensitive on Unix. Also remove inherited
    // aliases of reserved names rather than trusting only their canonical case.
    for key in std::env::vars_os().map(|(key, _)| key) {
        if key.to_str().is_some_and(is_context_env_key) {
            command.env_remove(key);
        }
    }
}

pub fn add_gemini_environment_aliases(env: &mut ExecutionEnv) -> Result<(), ExecutorError> {
    if ScopedWorkflowMcp::from_execution_env(env)?.is_some() {
        for key in FORWARDED_ENV_KEYS {
            let value = env
                .get(key)
                .cloned()
                .ok_or_else(|| launch_error("Workflow MCP execution context is incomplete"))?;
            env.insert(gemini_env_alias(key), value);
        }
    }
    Ok(())
}

pub fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

#[derive(Debug, Clone)]
pub struct ScopedWorkflowMcp {
    pub executable: PathBuf,
    pub server_name: String,
}

impl ScopedWorkflowMcp {
    pub fn from_execution_env(env: &ExecutionEnv) -> Result<Option<Self>, ExecutorError> {
        if !env.vars.keys().any(|key| is_context_env_key(key)) {
            return Ok(None);
        }
        let field = |name| {
            env.get(name)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| launch_error("Workflow MCP execution context is incomplete"))
        };
        for name in [SESSION_ID_ENV, AGENT_RUN_ID_ENV, TURN_ID_ENV] {
            Uuid::parse_str(field(name)?)
                .map_err(|_| launch_error("Workflow MCP execution identity is invalid"))?;
        }
        if !valid_token(field(TOKEN_ENV)?) {
            return Err(launch_error("Workflow MCP scoped credential is invalid"));
        }
        let url = reqwest::Url::parse(field(BACKEND_URL_ENV)?)
            .map_err(|_| launch_error("Workflow MCP backend URL is invalid"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(launch_error("Workflow MCP backend URL is invalid"));
        }
        let executable = PathBuf::from(field(EXECUTABLE_ENV)?);
        if !executable.is_absolute() || !executable.is_file() {
            return Err(launch_error(
                "The same-version bundled workflow MCP executable is unavailable",
            ));
        }
        let run_id = Uuid::parse_str(field(AGENT_RUN_ID_ENV)?)
            .map_err(|_| launch_error("Invalid main AgentRun"))?;
        // Keep the entire execution UUID without making provider-minted
        // mcp__<server>__<tool> names exceed the common 64-character boundary.
        Ok(Some(Self {
            executable,
            server_name: format!("{SERVER_NAME_PREFIX}{}", run_id.simple()),
        }))
    }

    pub fn args(&self) -> [&str; 2] {
        ["--mode", "workflow"]
    }

    /// Native JSON expansion, never literal secret material.
    pub fn environment_placeholders(&self) -> HashMap<String, String> {
        FORWARDED_ENV_KEYS
            .into_iter()
            .map(|key| (key.to_owned(), format!("${{{key}}}")))
            .collect()
    }
}

pub fn valid_token(token: &str) -> bool {
    (32..=512).contains(&token.len()) && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn bundled_executable_next_to(server: &Path) -> Result<PathBuf, std::io::Error> {
    let filename = if cfg!(windows) {
        "vibe-kanban-mcp.exe"
    } else {
        "vibe-kanban-mcp"
    };
    let path = server
        .parent()
        .ok_or_else(|| std::io::Error::other("Server executable has no parent directory"))?
        .join(filename);
    if !path.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "The same-version bundled workflow MCP executable was not found next to the server",
        ));
    }
    Ok(path)
}

/// Check the actual sibling, not a global PATH executable or an npm download.
pub async fn verify_bundled_executable(path: &Path) -> Result<(), std::io::Error> {
    let mut command = tokio::process::Command::new(path);
    command.arg("--version").kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW for background preflight.
    remove_inherited_context(&mut command);
    command.env_remove(BACKEND_URL_ENV);
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), command.output())
        .await
        .map_err(|_| std::io::Error::other("Bundled workflow MCP version check timed out"))??;
    if !version_output_matches(output.status.success(), &output.stdout) {
        return Err(std::io::Error::other(
            "Bundled workflow MCP is not the same version as this server; reinstall or rebuild the application",
        ));
    }
    Ok(())
}

fn version_output_matches(success: bool, stdout: &[u8]) -> bool {
    success
        && std::str::from_utf8(stdout).is_ok_and(|value| {
            value.trim() == format!("vibe-kanban-mcp {}", env!("CARGO_PKG_VERSION"))
        })
}

pub fn local_backend_url(mut address: SocketAddr) -> String {
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    format!("http://{address}")
}

/// DEFAULT/absent variants are aliases, but model/Agent/reasoning/permissions
/// remain the captured values. A main Session may not silently switch them.
pub fn matches_snapshot(expected: &ExecutorConfig, requested: &ExecutorConfig) -> bool {
    let mut expected = expected.clone();
    let mut requested = requested.clone();
    if expected
        .variant
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("DEFAULT"))
    {
        expected.variant = None;
    }
    if requested
        .variant
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("DEFAULT"))
    {
        requested.variant = None;
    }
    expected == requested
}

fn launch_error(message: &str) -> ExecutorError {
    ExecutorError::Io(std::io::Error::other(message))
}

#[cfg(test)]
pub(crate) fn test_env(executable: &Path) -> ExecutionEnv {
    let mut env = ExecutionEnv::new(crate::env::RepoContext::default(), false, String::new());
    env.insert(EXECUTABLE_ENV, executable.to_string_lossy());
    env.insert(BACKEND_URL_ENV, "http://127.0.0.1:3001");
    env.insert(SESSION_ID_ENV, Uuid::new_v4().to_string());
    env.insert(AGENT_RUN_ID_ENV, Uuid::new_v4().to_string());
    env.insert(TURN_ID_ENV, Uuid::new_v4().to_string());
    env.insert(TOKEN_ENV, "a".repeat(64));
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_context_is_explicit_and_fail_closed() {
        let mut env = ExecutionEnv::new(crate::env::RepoContext::default(), false, String::new());
        assert!(
            ScopedWorkflowMcp::from_execution_env(&env)
                .unwrap()
                .is_none()
        );
        env.insert(TOKEN_ENV, "a".repeat(64));
        assert!(ScopedWorkflowMcp::from_execution_env(&env).is_err());
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut env = test_env(file.path());
        let config = ScopedWorkflowMcp::from_execution_env(&env)
            .unwrap()
            .unwrap();
        assert!(
            !serde_json::to_string(&config.environment_placeholders())
                .unwrap()
                .contains(&"a".repeat(64))
        );
        env.insert(
            BACKEND_URL_ENV,
            "https://username:password@localhost/?token=x",
        );
        assert!(ScopedWorkflowMcp::from_execution_env(&env).is_err());
    }

    #[test]
    fn default_profile_alias_does_not_allow_model_changes() {
        let config = ExecutorConfig::new(crate::executors::BaseCodingAgent::Codex);
        let mut requested = config.clone();
        requested.variant = Some("DEFAULT".into());
        assert!(matches_snapshot(&config, &requested));
        requested.model_id = Some("other-model".into());
        assert!(!matches_snapshot(&config, &requested));
        assert_eq!(
            local_backend_url("0.0.0.0:3001".parse().unwrap()),
            "http://127.0.0.1:3001"
        );
        assert_eq!(
            local_backend_url("[::]:3001".parse().unwrap()),
            "http://[::1]:3001"
        );
    }

    #[test]
    fn scoped_names_and_same_version_are_checked_explicitly() {
        assert!(is_context_env_key("mcp_workflow_token"));
        assert!(is_token_env_key("Mcp_Workflow_Token"));
        assert!(is_context_env_key(
            &gemini_env_alias(BACKEND_URL_ENV).to_lowercase()
        ));
        assert!(is_token_env_key(
            &gemini_env_alias(TOKEN_ENV).to_lowercase()
        ));
        assert!(!is_context_env_key("USER_TOKEN"));
        assert!(version_output_matches(
            true,
            format!("vibe-kanban-mcp {}\r\n", env!("CARGO_PKG_VERSION")).as_bytes()
        ));
        assert!(!version_output_matches(
            true,
            b"vibe-kanban-mcp different-version"
        ));
        assert!(!version_output_matches(
            false,
            format!("vibe-kanban-mcp {}", env!("CARGO_PKG_VERSION")).as_bytes()
        ));
        assert_eq!(
            token_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn workflow_mcp_alias_only_environment_is_not_an_ordinary_session() {
        let mut env = ExecutionEnv::new(crate::env::RepoContext::default(), false, String::new());
        env.insert(gemini_env_alias(TOKEN_ENV), "a".repeat(64));
        assert!(ScopedWorkflowMcp::from_execution_env(&env).is_err());
    }

    #[test]
    fn workflow_mcp_native_names_fit_provider_limits_without_truncating_execution_identity() {
        let executable = tempfile::NamedTempFile::new().unwrap();
        let env = test_env(executable.path());
        let config = ScopedWorkflowMcp::from_execution_env(&env)
            .unwrap()
            .unwrap();
        let run_id = Uuid::parse_str(env.get(AGENT_RUN_ID_ENV).unwrap()).unwrap();
        assert_eq!(config.server_name, format!("vk_{}", run_id.simple()));
        for tool in WORKFLOW_TOOL_NAMES {
            assert!(format!("mcp__{}__{tool}", config.server_name).len() <= 64);
        }
    }
}
