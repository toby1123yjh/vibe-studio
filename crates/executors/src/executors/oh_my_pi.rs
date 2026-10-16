//! Oh My Pi (`can1357/oh-my-pi`) direct runtime adapter.
//!
//! V1 intentionally launches the real `omp` executable and speaks its stdio
//! RPC NDJSON protocol.  No Node SDK or in-process package is linked here.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;
use workspace_utils::command_ext::GroupSpawnNoWindowExt;

pub mod catalog;
pub mod command_adapter;
pub(crate) mod rpc;

/// Returns the active Oh My Pi agent directory, including environment,
/// profile, tilde, and migrated XDG layout resolution.
pub fn oh_my_pi_agent_root() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PI_CODING_AGENT_DIR")
        .or_else(|| std::env::var_os("OMP_AGENT_DIR"))
        .map(PathBuf::from)
    {
        let value = path.to_string_lossy();
        if value != "~" && !value.starts_with("~/") && !value.starts_with("~\\") {
            return Some(path);
        }
    }
    dirs::home_dir().map(|home| oh_my_pi_agent_root_for(&home))
}

/// Resolves the active Oh My Pi agent directory using an injected home as the
/// fallback. This keeps all consumers aligned without coupling tests to the
/// process user's real home directory.
pub fn oh_my_pi_agent_root_for(home_dir: &Path) -> PathBuf {
    if let Some(agent_dir) = std::env::var_os("PI_CODING_AGENT_DIR") {
        return expand_tilde_path(PathBuf::from(agent_dir), home_dir);
    }
    if let Some(agent_dir) = std::env::var_os("OMP_AGENT_DIR") {
        return expand_tilde_path(PathBuf::from(agent_dir), home_dir);
    }

    let profile = std::env::var("OMP_PROFILE")
        .ok()
        .or_else(|| std::env::var("PI_PROFILE").ok())
        .and_then(|profile| normalize_profile(&profile));

    if cfg!(unix)
        && let Some(xdg_data) = std::env::var_os("XDG_DATA_HOME")
    {
        let mut root = PathBuf::from(xdg_data).join("omp");
        if let Some(profile) = profile.as_deref() {
            root = root.join("profiles").join(profile);
        }
        if root.exists() {
            return root;
        }
    }

    let config_dir = std::env::var_os("PI_CONFIG_DIR")
        .map(|path| expand_tilde_path(PathBuf::from(path), home_dir))
        .unwrap_or_else(|| PathBuf::from(".omp"));
    let mut root = home_dir.join(config_dir);
    if let Some(profile) = profile {
        root = root.join("profiles").join(profile);
    }
    root.join("agent")
}

fn expand_tilde_path(path: PathBuf, home_dir: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if value == "~" {
        return home_dir.to_path_buf();
    }
    if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return home_dir.join(rest);
    }
    path
}

fn normalize_profile(value: &str) -> Option<String> {
    let profile = value.trim();
    let first_is_alphanumeric = profile
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric);
    if profile.is_empty()
        || profile == "default"
        || profile == "."
        || profile == ".."
        || !first_is_alphanumeric
        || profile.ends_with('.')
        || !profile
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        || is_windows_reserved_profile_name(profile)
    {
        return None;
    }
    Some(profile.to_string())
}

fn is_windows_reserved_profile_name(profile: &str) -> bool {
    let stem = profile
        .split_once('.')
        .map_or(profile, |(stem, _)| stem)
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM0"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT0"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

use crate::{
    command::{CmdOverrides, CommandBuildError, CommandBuilder},
    env::ExecutionEnv,
    executors::{
        AppendPrompt, AvailabilityInfo, BaseCodingAgent, ExecutorError, SpawnedChild,
        StandardCodingAgentExecutor,
        provider_adapter::{DirectIntent, DirectProvider},
    },
    profile::{ExecutorConfig, ProviderRunOptions},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, JsonSchema, Default)]
pub struct OhMyPi {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compaction: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_retry: Option<bool>,
    #[serde(flatten)]
    pub cmd: CmdOverrides,
}

impl OhMyPi {
    pub const DEFAULT_BASE_COMMAND: &'static str = "omp";
    pub const RUNTIME_MODE: &'static str = "rpc";

    pub fn build_command_builder(
        &self,
        intent: DirectIntent,
    ) -> Result<CommandBuilder, CommandBuildError> {
        command_adapter::OhMyPiCommandAdapter::new(self).build(intent, None)
    }

    pub fn provider(&self) -> DirectProvider {
        DirectProvider::OhMyPi
    }

    pub fn capabilities(
        &self,
        runtime_profile_id: impl Into<String>,
    ) -> crate::runtime::CapabilitySnapshot {
        self.provider().capabilities(runtime_profile_id)
    }

    pub fn direct_mapper(&self) -> crate::executors::provider_adapter::DirectProviderMapper {
        self.provider().mapper()
    }

    pub(crate) fn apply_direct_overrides(
        &mut self,
        executor_config: &ExecutorConfig,
    ) -> Result<(), ExecutorError> {
        executor_config
            .validate_provider_options()
            .map_err(ExecutorError::InvalidConfiguration)?;
        if executor_config.agent_id.is_some() {
            return Err(ExecutorError::InvalidConfiguration(
                "Oh My Pi does not expose an agent selector".into(),
            ));
        }
        // Supervised is the existing native-policy marker, not a host-side
        // policy setter. OMP itself retains authority over its permissions.
        if executor_config
            .permission_policy
            .as_ref()
            .is_some_and(|policy| *policy != crate::model_selector::PermissionPolicy::Supervised)
        {
            return Err(ExecutorError::InvalidConfiguration(
                "Oh My Pi does not expose automatic or plan permission overrides; use its native policy".into(),
            ));
        }
        if let Some(model_id) = &executor_config.model_id {
            self.model = Some(model_id.clone());
        }
        if let Some(reasoning_id) = &executor_config.reasoning_id {
            self.thinking_level = Some(reasoning_id.clone());
        }
        if let Some(ProviderRunOptions::OhMyPi {
            auto_compaction,
            auto_retry,
        }) = &executor_config.provider_options
        {
            if auto_compaction.is_some() {
                self.auto_compaction = *auto_compaction;
            }
            if auto_retry.is_some() {
                self.auto_retry = *auto_retry;
            }
        }
        Ok(())
    }

    pub(crate) async fn launch_direct(
        &self,
        intent: DirectIntent,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        match intent {
            DirectIntent::Initial => self.spawn(current_dir, prompt, env).await,
            DirectIntent::FollowUp | DirectIntent::Resume => {
                let session_id = session_id.ok_or_else(|| {
                    ExecutorError::FollowUpNotSupported(
                        "Oh My Pi direct follow-up requires a provider session".to_string(),
                    )
                })?;
                self.spawn_follow_up(current_dir, prompt, session_id, env)
                    .await
            }
            DirectIntent::Review => {
                self.spawn_review(current_dir, prompt, session_id, env)
                    .await
            }
        }
    }

    /// Launch `omp --mode rpc` and hand its audited protocol peer to the
    /// supervisor. The peer confirms execution-local settings before prompting;
    /// the native process owns tool execution and MCP.
    pub async fn spawn(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.spawn_rpc(current_dir, prompt, None, env).await
    }

    pub async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.spawn_rpc(current_dir, prompt, Some(session_id), env)
            .await
    }

    pub async fn spawn_review(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.spawn_rpc(current_dir, prompt, session_id, env).await
    }

    async fn spawn_rpc(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let workflow_mcp = crate::workflow_mcp::ScopedWorkflowMcp::from_execution_env(env)?;
        let mut launch_env = env.clone().with_profile(&self.cmd);
        let readiness = crate::workflow_mcp::WorkflowMcpReadiness::start(&mut launch_env).await?;
        let workflow_plugin = workflow_mcp
            .as_ref()
            .map(command_adapter::create_workflow_plugin)
            .transpose()?;
        let builder = command_adapter::OhMyPiCommandAdapter::new(self).build(
            if session_id.is_some() {
                DirectIntent::FollowUp
            } else {
                DirectIntent::Initial
            },
            session_id,
        )?;
        let builder = command_adapter::append_workflow_plugin(
            builder,
            workflow_plugin.as_ref().map(|plugin| plugin.path()),
        );
        let (program, args) = builder.build_initial()?.into_resolved().await?;
        let mut command = Command::new(program);
        command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(current_dir)
            .args(args);
        launch_env.apply_to_command(&mut command);

        let mut child = command.group_spawn_no_window()?;
        let cancel = CancellationToken::new();
        let stdin = child.inner().stdin.take().ok_or_else(|| {
            ExecutorError::Io(std::io::Error::other("Oh My Pi missing stdin control pipe"))
        })?;
        let stdout = child.inner().stdout.take().ok_or_else(|| {
            ExecutorError::Io(std::io::Error::other("Oh My Pi missing stdout pipe"))
        })?;
        let output = crate::stdout_dup::create_stdout_pipe_writer(&mut child)?;
        let (peer, exit_signal) = rpc::RpcPeer::start(
            stdin,
            stdout,
            output,
            self.clone(),
            catalog::scope(current_dir, &self.cmd),
            self.append_prompt.combine_prompt(prompt),
            session_id.map(str::to_owned),
            readiness,
            cancel.clone(),
            workflow_plugin,
        );

        // The supervisor owns cancellation and process-tree cleanup.  Keeping
        // a token on the returned child makes the control boundary explicit.
        Ok(SpawnedChild {
            child,
            exit_signal: Some(exit_signal),
            cancel: Some(cancel),
            control: Some(Arc::new(peer)),
        })
    }

    pub fn default_mcp_config_path(&self) -> Option<std::path::PathBuf> {
        oh_my_pi_agent_root().map(|root| root.join("mcp.json"))
    }

    pub fn get_availability_info(&self) -> AvailabilityInfo {
        if self
            .default_mcp_config_path()
            .is_some_and(|path| path.exists())
            || crate::command::is_command_installed(
                crate::command::OH_MY_PI_DEFAULT_BASE_COMMAND,
                &self.cmd,
            )
        {
            AvailabilityInfo::InstallationFound
        } else {
            AvailabilityInfo::NotFound
        }
    }

    pub fn get_preset_options(&self) -> ExecutorConfig {
        ExecutorConfig {
            executor: BaseCodingAgent::OhMyPi,
            variant: None,
            model_id: self.model.clone(),
            agent_id: None,
            reasoning_id: self.thinking_level.clone(),
            permission_policy: Some(crate::model_selector::PermissionPolicy::Supervised),
            provider_options: (self.auto_compaction.is_some() || self.auto_retry.is_some())
                .then_some(ProviderRunOptions::OhMyPi {
                    auto_compaction: self.auto_compaction,
                    auto_retry: self.auto_retry,
                }),
        }
    }
}

#[async_trait]
impl StandardCodingAgentExecutor for OhMyPi {
    async fn discover_options(
        &self,
        workdir: Option<&Path>,
        repo_path: Option<&Path>,
    ) -> Result<futures::stream::BoxStream<'static, json_patch::Patch>, ExecutorError> {
        use crate::{
            executor_discovery::ExecutorDiscoveredOptions,
            model_selector::{ModelInfo, ModelSelectorConfig, PermissionPolicy},
        };
        let cwd = workdir
            .or(repo_path)
            .map(Path::to_path_buf)
            .unwrap_or(std::env::current_dir()?);
        let observed = catalog::observed_catalog(&cwd, &self.cmd);
        let error = observed.is_none().then(||
            "Oh My Pi capabilities are unconfirmed; the next agent run will report its native model catalog".to_string());
        let model_selector = observed.unwrap_or_else(|| ModelSelectorConfig {
            models: self
                .model
                .iter()
                .map(|model| ModelInfo {
                    id: model.clone(),
                    name: model.clone(),
                    ..Default::default()
                })
                .collect(),
            default_model: self.model.clone(),
            permissions: vec![PermissionPolicy::Supervised],
            ..Default::default()
        });
        let options = ExecutorDiscoveredOptions {
            model_selector,
            error,
            ..Default::default()
        };
        Ok(Box::pin(futures::stream::once(async move {
            crate::logs::utils::patch::executor_discovered_options(options)
        })))
    }

    fn apply_overrides(&mut self, executor_config: &ExecutorConfig) -> Result<(), ExecutorError> {
        self.apply_direct_overrides(executor_config)
    }

    async fn spawn(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        OhMyPi::spawn(self, current_dir, prompt, env).await
    }

    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        reset_to_message_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        if reset_to_message_id.is_some() {
            return Err(ExecutorError::ResetToMessageNotSupported(
                "Oh My Pi stdio RPC does not expose message-level reset".to_string(),
            ));
        }
        OhMyPi::spawn_follow_up(self, current_dir, prompt, session_id, env).await
    }

    async fn spawn_review(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        OhMyPi::spawn_review(self, current_dir, prompt, session_id, env).await
    }

    fn default_mcp_config_path(&self) -> Option<std::path::PathBuf> {
        OhMyPi::default_mcp_config_path(self)
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        OhMyPi::get_availability_info(self)
    }

    fn get_preset_options(&self) -> ExecutorConfig {
        OhMyPi::get_preset_options(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_are_safe_path_segments() {
        assert_eq!(normalize_profile("default"), None);
        assert_eq!(
            normalize_profile(" work-profile_1 "),
            Some("work-profile_1".to_string())
        );
        for invalid in [
            "../escape",
            ".",
            "profile.",
            "CON",
            "LPT1.json",
            "with space",
        ] {
            assert_eq!(normalize_profile(invalid), None);
        }
    }

    #[test]
    fn registered_preset_uses_oh_my_pi_identity() {
        let preset = OhMyPi::default().get_preset_options();

        assert_eq!(preset.executor, BaseCodingAgent::OhMyPi);
        assert_eq!(preset.variant, None);
    }

    #[test]
    fn unsupported_omp_overrides_fail_instead_of_disappearing() {
        use crate::model_selector::PermissionPolicy;
        let mut agent = OhMyPi::default();
        let mut config = agent.get_preset_options();
        assert!(agent.apply_direct_overrides(&config).is_ok());
        for policy in [PermissionPolicy::Auto, PermissionPolicy::Plan] {
            config.permission_policy = Some(policy);
            assert!(matches!(
                agent.apply_direct_overrides(&config),
                Err(ExecutorError::InvalidConfiguration(_))
            ));
        }
        config.permission_policy = None;
        config.agent_id = Some("unavailable-agent".into());
        assert!(matches!(
            agent.apply_direct_overrides(&config),
            Err(ExecutorError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn rpc_command_uses_the_real_omp_executable() {
        let command = OhMyPi::default()
            .build_command_builder(DirectIntent::Initial)
            .expect("Oh My Pi command should build");

        assert_eq!(command.base, "omp");
        assert_eq!(command.params, Some(vec!["--mode".into(), "rpc".into()]));
    }

    #[tokio::test]
    async fn reset_request_is_rejected_instead_of_silently_downgrading() {
        let env = ExecutionEnv::new(Default::default(), false, String::new());
        let result = <OhMyPi as StandardCodingAgentExecutor>::spawn_follow_up(
            &OhMyPi::default(),
            Path::new("."),
            "prompt",
            "session",
            Some("message"),
            &env,
        )
        .await;

        assert!(matches!(
            result,
            Err(ExecutorError::ResetToMessageNotSupported(_))
        ));
    }
}
