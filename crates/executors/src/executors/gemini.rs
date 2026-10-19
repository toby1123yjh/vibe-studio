use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use derivative::Derivative;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use workspace_utils::msg_store::MsgStore;

pub use super::acp::AcpAgentHarness;
pub mod command_adapter;
use crate::{
    approvals::ExecutorApprovalService,
    command::{CmdOverrides, CommandBuildError, CommandBuilder},
    env::ExecutionEnv,
    executors::{
        AppendPrompt, AvailabilityInfo, BaseCodingAgent, ExecutorError, SpawnedChild,
        StandardCodingAgentExecutor,
    },
    logs::utils::patch,
    model_selector::PermissionPolicy,
    profile::ExecutorConfig,
};

const SUPPRESSED_STDERR_PATTERNS: &[&str] = &[
    "was started but never ended. Skipping metrics.",
    "YOLO mode is enabled. All tool calls will be automatically approved.",
];
#[derive(Derivative, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[derivative(Debug, PartialEq)]
pub struct Gemini {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yolo: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(flatten)]
    pub cmd: CmdOverrides,
    #[serde(skip)]
    #[ts(skip)]
    #[derivative(Debug = "ignore", PartialEq = "ignore")]
    pub approvals: Option<Arc<dyn ExecutorApprovalService>>,
}

impl Gemini {
    /// V1 direct-runtime metadata and decoder entry point.  Legacy ACP log
    /// normalization remains available to old callers until the runtime port
    /// is connected by the orchestration layer.
    pub const DIRECT_PROVIDER: super::provider_adapter::DirectProvider =
        super::provider_adapter::DirectProvider::Gemini;

    pub fn direct_versions() -> super::provider_adapter::DirectAdapterVersions {
        Self::DIRECT_PROVIDER.versions()
    }

    pub fn direct_capabilities(
        runtime_profile_id: impl Into<String>,
    ) -> crate::runtime::CapabilitySnapshot {
        Self::DIRECT_PROVIDER.capabilities(runtime_profile_id)
    }

    pub fn decode_native_frame(
        frame: &crate::runtime::NativeAuditFrame,
    ) -> Result<super::provider_adapter::DecodedProviderEvent, crate::runtime::NativeAuditError>
    {
        Self::DIRECT_PROVIDER.decode_native_frame(frame)
    }

    pub fn direct_mapper() -> super::provider_adapter::DirectProviderMapper {
        Self::DIRECT_PROVIDER.mapper()
    }

    pub(crate) fn apply_direct_overrides(
        &mut self,
        executor_config: &ExecutorConfig,
    ) -> Result<(), ExecutorError> {
        if executor_config
            .reasoning_id
            .as_ref()
            .is_some_and(Option::is_some)
        {
            return Err(super::acp::session_config::config_error(
                "Gemini thinking budgets are native model settings, not per-attempt reasoning options",
            ));
        }
        let mode = executor_config.agent_id.as_ref().or(self.mode.as_ref());
        if let (Some(mode), Some(policy)) = (mode, executor_config.permission_policy.as_ref()) {
            let compatible = match policy {
                PermissionPolicy::Auto => mode == "yolo",
                PermissionPolicy::Supervised => mode != "yolo" && mode != "plan",
                PermissionPolicy::Plan => mode == "plan",
            };
            if !compatible {
                return Err(super::acp::session_config::config_error(
                    "Gemini native mode conflicts with the selected permission policy",
                ));
            }
        }
        if let Some(model_id) = &executor_config.model_id {
            self.model = model_id.clone();
        }
        if let Some(mode) = &executor_config.agent_id {
            self.mode = Some(mode.clone());
            self.yolo = Some(mode == "yolo");
        }
        if let Some(permission_policy) = executor_config.permission_policy.clone() {
            self.yolo = Some(permission_policy == PermissionPolicy::Auto);
            if executor_config.agent_id.is_none() && self.mode.is_none() {
                self.mode = Some(
                    match permission_policy {
                        PermissionPolicy::Auto => "yolo",
                        PermissionPolicy::Supervised => "default",
                        PermissionPolicy::Plan => "plan",
                    }
                    .to_string(),
                );
            }
        }
        Ok(())
    }

    fn configure_harness(&self, harness: AcpAgentHarness, cwd: &Path) -> AcpAgentHarness {
        let mut harness =
            harness.with_catalog_identity(super::acp::session_config::scoped_catalog_identity(
                super::acp::session_config::AcpDialect::Gemini,
                cwd,
                &self.cmd,
            ));
        if let Some(model) = &self.model {
            harness = harness.with_model(model);
        }
        if let Some(mode) = &self.mode {
            harness = harness.with_mode(mode);
        } else if let Some(yolo) = self.yolo {
            harness = harness.with_mode(if yolo { "yolo" } else { "default" });
        }
        harness
    }

    pub(crate) fn use_direct_approvals(&mut self, approvals: Arc<dyn ExecutorApprovalService>) {
        self.approvals = Some(approvals);
    }

    pub(crate) async fn launch_direct(
        &self,
        intent: super::provider_adapter::DirectIntent,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        match intent {
            super::provider_adapter::DirectIntent::Initial => {
                self.spawn_initial_direct(current_dir, prompt, env).await
            }
            super::provider_adapter::DirectIntent::FollowUp
            | super::provider_adapter::DirectIntent::Review => {
                if let Some(session_id) = session_id {
                    self.spawn_follow_up_direct(current_dir, prompt, session_id, env)
                        .await
                } else {
                    self.spawn_initial_direct(current_dir, prompt, env).await
                }
            }
            super::provider_adapter::DirectIntent::Resume => {
                let session_id = session_id.ok_or_else(|| {
                    ExecutorError::FollowUpNotSupported(
                        "Gemini native resume requires a provider session".to_string(),
                    )
                })?;
                self.spawn_native_resume_direct(current_dir, prompt, session_id, env)
                    .await
            }
        }
    }

    async fn spawn_initial_direct(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let (env, readiness) = command_adapter::prepare_workflow_launch(env, &self.cmd).await?;
        let harness = self
            .configure_harness(AcpAgentHarness::new(), current_dir)
            .with_mcp_servers(command_adapter::workflow_mcp_servers(&env)?)
            .with_workflow_readiness(readiness);
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let gemini_command = self.build_command_builder()?.build_initial()?;
        let approvals = if self.yolo.unwrap_or(false) {
            None
        } else {
            self.approvals.clone()
        };
        harness
            .spawn_with_command(
                current_dir,
                combined_prompt,
                gemini_command,
                &env,
                &self.cmd,
                approvals,
            )
            .await
    }

    async fn spawn_follow_up_direct(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let (env, readiness) = command_adapter::prepare_workflow_launch(env, &self.cmd).await?;
        let harness = self
            .configure_harness(AcpAgentHarness::new(), current_dir)
            .with_mcp_servers(command_adapter::workflow_mcp_servers(&env)?)
            .with_workflow_readiness(readiness);
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let gemini_command = self.build_command_builder()?.build_follow_up(&[])?;
        let approvals = if self.yolo.unwrap_or(false) {
            None
        } else {
            self.approvals.clone()
        };
        harness
            .spawn_native_resume_with_command(
                current_dir,
                combined_prompt,
                session_id,
                gemini_command,
                &env,
                &self.cmd,
                approvals,
            )
            .await
    }

    async fn spawn_native_resume_direct(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let (env, readiness) = command_adapter::prepare_workflow_launch(env, &self.cmd).await?;
        let harness = self
            .configure_harness(AcpAgentHarness::new(), current_dir)
            .with_mcp_servers(command_adapter::workflow_mcp_servers(&env)?)
            .with_workflow_readiness(readiness);
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let gemini_command = self.build_command_builder()?.build_initial()?;
        let approvals = if self.yolo.unwrap_or(false) {
            None
        } else {
            self.approvals.clone()
        };
        harness
            .spawn_native_resume_with_command(
                current_dir,
                combined_prompt,
                session_id,
                gemini_command,
                &env,
                &self.cmd,
                approvals,
            )
            .await
    }

    fn build_command_builder(&self) -> Result<CommandBuilder, CommandBuildError> {
        command_adapter::GeminiCommandAdapter::new(self).build()
    }
}

#[async_trait]
impl StandardCodingAgentExecutor for Gemini {
    fn apply_overrides(&mut self, executor_config: &ExecutorConfig) -> Result<(), ExecutorError> {
        self.apply_direct_overrides(executor_config)
    }

    fn use_approvals(&mut self, approvals: Arc<dyn ExecutorApprovalService>) {
        self.use_direct_approvals(approvals);
    }

    async fn spawn(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.spawn_initial_direct(current_dir, prompt, env).await
    }

    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        _reset_to_message_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.spawn_follow_up_direct(current_dir, prompt, session_id, env)
            .await
    }

    fn normalize_logs(
        &self,
        msg_store: Arc<MsgStore>,
        worktree_path: &Path,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        super::acp::normalize_logs_with_suppressed_stderr_patterns(
            msg_store,
            worktree_path,
            SUPPRESSED_STDERR_PATTERNS,
        )
    }

    fn default_mcp_config_path(&self) -> Option<std::path::PathBuf> {
        dirs::home_dir().map(|home| home.join(".gemini").join("settings.json"))
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        if let Some(timestamp) = dirs::home_dir()
            .and_then(|home| std::fs::metadata(home.join(".gemini").join("oauth_creds.json")).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
        {
            return AvailabilityInfo::LoginDetected {
                last_auth_timestamp: timestamp,
            };
        }

        let mcp_config_found = self
            .default_mcp_config_path()
            .map(|p| p.exists())
            .unwrap_or(false);

        let installation_indicator_found = dirs::home_dir()
            .map(|home| home.join(".gemini").join("installation_id").exists())
            .unwrap_or(false);

        if mcp_config_found
            || installation_indicator_found
            || crate::command::is_command_installed(
                crate::command::GEMINI_DEFAULT_BASE_COMMAND,
                &self.cmd,
            )
        {
            AvailabilityInfo::InstallationFound
        } else {
            AvailabilityInfo::NotFound
        }
    }

    fn get_preset_options(&self) -> ExecutorConfig {
        use crate::model_selector::*;
        ExecutorConfig {
            executor: BaseCodingAgent::Gemini,
            variant: None,
            model_id: self.model.clone().map(Some),
            agent_id: self.mode.clone(),
            reasoning_id: None,
            provider_options: None,
            permission_policy: self
                .mode
                .is_none()
                .then_some(if self.yolo.unwrap_or(false) {
                    PermissionPolicy::Auto
                } else {
                    PermissionPolicy::Supervised
                }),
        }
    }

    async fn discover_options(
        &self,
        workdir: Option<&std::path::Path>,
        repo_path: Option<&std::path::Path>,
    ) -> Result<futures::stream::BoxStream<'static, json_patch::Patch>, ExecutorError> {
        let options = super::acp::provider::discover_options(
            Self::DIRECT_PROVIDER,
            super::acp::session_config::AcpDialect::Gemini,
            workdir,
            repo_path,
            &self.cmd,
            self.model.as_deref(),
        );
        Ok(Box::pin(futures::stream::once(async move {
            patch::executor_discovered_options(options)
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_mode_and_permissions_reject_conflicts_before_mutation() {
        let mut gemini: Gemini =
            serde_json::from_value(serde_json::json!({"model":"original"})).unwrap();
        let mut config = ExecutorConfig::new(BaseCodingAgent::Gemini);
        config.model_id = Some(Some("different".into()));
        config.agent_id = Some("yolo".into());
        config.permission_policy = Some(PermissionPolicy::Supervised);
        assert!(gemini.apply_direct_overrides(&config).is_err());
        assert_eq!(gemini.model.as_deref(), Some("original"));
        config.permission_policy = Some(PermissionPolicy::Auto);
        gemini.apply_direct_overrides(&config).unwrap();
        assert_eq!(gemini.mode.as_deref(), Some("yolo"));
        assert_eq!(gemini.yolo, Some(true));
        assert!(gemini.get_preset_options().permission_policy.is_none());
    }

    #[test]
    fn explicit_thinking_is_not_silently_ignored() {
        let mut gemini: Gemini = serde_json::from_value(serde_json::json!({})).unwrap();
        let mut config = ExecutorConfig::new(BaseCodingAgent::Gemini);
        config.reasoning_id = Some(Some("high".into()));
        assert!(gemini.apply_direct_overrides(&config).is_err());
    }
}
