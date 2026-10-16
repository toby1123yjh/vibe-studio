use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use derivative::Derivative;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use workspace_utils::msg_store::MsgStore;

use super::{
    acp::{provider::AcpProviderLaunch, session_config::AcpDialect},
    provider_adapter::{DirectIntent, DirectProvider},
};
use crate::{
    approvals::ExecutorApprovalService,
    command::{CmdOverrides, OPENCODE_DEFAULT_BASE_COMMAND, is_command_installed},
    env::ExecutionEnv,
    executors::{
        AppendPrompt, AvailabilityInfo, BaseCodingAgent, ExecutorError, SpawnedChild,
        StandardCodingAgentExecutor,
    },
    logs::utils::patch,
    model_selector::PermissionPolicy,
    profile::ExecutorConfig,
};

pub mod command_adapter;

#[derive(Derivative, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[derivative(Debug, PartialEq)]
pub struct Opencode {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yolo: Option<bool>,
    #[serde(flatten)]
    pub cmd: CmdOverrides,
    #[serde(skip)]
    #[ts(skip)]
    #[derivative(Debug = "ignore", PartialEq = "ignore")]
    pub approvals: Option<Arc<dyn ExecutorApprovalService>>,
}

impl Opencode {
    pub const DIRECT_PROVIDER: DirectProvider = DirectProvider::Opencode;

    pub(crate) fn apply_direct_overrides(
        &mut self,
        config: &ExecutorConfig,
    ) -> Result<(), ExecutorError> {
        if config.permission_policy == Some(PermissionPolicy::Plan) {
            return Err(super::acp::session_config::config_error(
                "Choose an advertised OpenCode mode instead of a generic plan permission policy",
            ));
        }
        if let Some(model) = &config.model_id {
            self.model = Some(model.clone());
        }
        if let Some(effort) = &config.reasoning_id {
            self.reasoning_effort = Some(effort.clone());
        }
        if let Some(agent) = &config.agent_id {
            self.agent = Some(agent.clone());
        }
        if let Some(policy) = &config.permission_policy {
            self.yolo = Some(matches!(policy, PermissionPolicy::Auto));
        }
        Ok(())
    }

    pub(crate) fn use_direct_approvals(&mut self, approvals: Arc<dyn ExecutorApprovalService>) {
        self.approvals = Some(approvals);
    }

    pub(crate) async fn launch_direct(
        &self,
        intent: DirectIntent,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let command = command_adapter::OpencodeCommandAdapter::new(self)
            .build()?
            .build_initial()?;
        AcpProviderLaunch {
            dialect: AcpDialect::Opencode,
            namespace: "opencode_sessions",
            model: self.model.as_deref(),
            effort: self.reasoning_effort.as_deref(),
            mode: self.agent.as_deref(),
            append_prompt: &self.append_prompt,
            cmd: &self.cmd,
            approvals: if self.yolo.unwrap_or(false) {
                None
            } else {
                self.approvals.clone()
            },
        }
        .launch(intent, current_dir, prompt, session_id, command, env)
        .await
    }
}

#[async_trait]
impl StandardCodingAgentExecutor for Opencode {
    fn apply_overrides(&mut self, config: &ExecutorConfig) -> Result<(), ExecutorError> {
        self.apply_direct_overrides(config)
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
        self.launch_direct(DirectIntent::Initial, current_dir, prompt, None, env)
            .await
    }
    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        reset: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        if reset.is_some() {
            return Err(ExecutorError::ResetToMessageNotSupported(
                "OpenCode ACP cannot reset to a message".to_string(),
            ));
        }
        self.launch_direct(
            DirectIntent::FollowUp,
            current_dir,
            prompt,
            Some(session_id),
            env,
        )
        .await
    }
    fn normalize_logs(
        &self,
        store: Arc<MsgStore>,
        worktree_path: &Path,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        super::acp::normalize_logs(store, worktree_path)
    }
    fn default_mcp_config_path(&self) -> Option<PathBuf> {
        // ToolManager owns JSONC/explicit config/overlay precedence and safe
        // writes. Do not expose a lower-priority file to the legacy editor.
        None
    }
    fn get_availability_info(&self) -> AvailabilityInfo {
        if is_command_installed(OPENCODE_DEFAULT_BASE_COMMAND, &self.cmd) {
            AvailabilityInfo::InstallationFound
        } else {
            AvailabilityInfo::NotFound
        }
    }
    fn get_preset_options(&self) -> ExecutorConfig {
        ExecutorConfig {
            executor: BaseCodingAgent::Opencode,
            variant: None,
            model_id: self.model.clone(),
            reasoning_id: self.reasoning_effort.clone(),
            agent_id: self.agent.clone(),
            provider_options: None,
            permission_policy: Some(if self.yolo.unwrap_or(false) {
                PermissionPolicy::Auto
            } else {
                PermissionPolicy::Supervised
            }),
        }
    }
    async fn discover_options(
        &self,
        workdir: Option<&Path>,
        project: Option<&Path>,
    ) -> Result<futures::stream::BoxStream<'static, json_patch::Patch>, ExecutorError> {
        let options = super::acp::provider::discover_options(
            Self::DIRECT_PROVIDER,
            AcpDialect::Opencode,
            workdir,
            project,
            &self.cmd,
            self.model.as_deref(),
        );
        Ok(Box::pin(futures::stream::once(async move {
            patch::executor_discovered_options(options)
        })))
    }
}
