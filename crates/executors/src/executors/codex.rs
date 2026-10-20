pub mod client;
pub mod jsonrpc;
pub mod normalize_logs;
pub mod review;
pub mod slash_commands;
use std::{
    borrow::Cow,
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};

/// Returns the Codex home directory.
///
/// Checks the `CODEX_HOME` environment variable first, then falls back to `~/.codex`.
/// This allows users to configure a custom location for Codex configuration and state.
pub fn codex_home() -> Option<PathBuf> {
    configured_codex_home().or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
}

/// Resolves the Codex home while retaining an injected home-directory
/// fallback for services and tests that do not run against the process home.
pub fn codex_home_for(home_dir: &Path) -> PathBuf {
    configured_codex_home().unwrap_or_else(|| home_dir.join(".codex"))
}

fn configured_codex_home() -> Option<PathBuf> {
    env::var("CODEX_HOME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
}

pub(crate) fn resume_params_from(
    thread_id: String,
    params: ThreadStartParams,
) -> ThreadResumeParams {
    ThreadResumeParams {
        thread_id,
        model: params.model,
        model_provider: params.model_provider,
        service_tier: params.service_tier,
        cwd: params.cwd,
        runtime_workspace_roots: params.runtime_workspace_roots,
        approval_policy: params.approval_policy,
        approvals_reviewer: params.approvals_reviewer,
        sandbox: params.sandbox,
        permissions: params.permissions,
        config: params.config,
        base_instructions: params.base_instructions,
        developer_instructions: params.developer_instructions,
        personality: params.personality,
        ..Default::default()
    }
}

const SKILLS_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const MODELS_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

use async_trait::async_trait;
use codex_app_server_protocol::{
    AskForApproval as V2AskForApproval, ReviewTarget, SandboxMode as V2SandboxMode, SkillScope,
    SkillsListResponse, ThreadResumeParams, ThreadStartParams, UserInput,
};
use codex_protocol::openai_models::ReasoningEffort as ProtocolReasoningEffort;
use derivative::Derivative;
use futures::StreamExt;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use strum_macros::AsRefStr;
use tokio::process::Command;
use ts_rs::TS;
use workspace_utils::{command_ext::GroupSpawnNoWindowExt, msg_store::MsgStore};

use self::{
    client::{AppServerClient, LogWriter},
    jsonrpc::{ExitSignalSender, JsonRpcPeer},
    normalize_logs::{Error, normalize_logs},
};

pub mod command_adapter;
use crate::{
    actions::SelectedSkill,
    approvals::ExecutorApprovalService,
    command::{CmdOverrides, CommandBuildError, CommandBuilder, CommandParts},
    env::{ExecutionEnv, RepoContext},
    executor_discovery::{CodexSkillDescription, CodexSkillLoadError, ExecutorDiscoveredOptions},
    executors::{
        AppendPrompt, AvailabilityInfo, BaseCodingAgent, ExecutorControl, ExecutorError,
        ExecutorExitResult, SpawnedChild, StandardCodingAgentExecutor,
    },
    logs::utils::patch,
    model_selector::{
        ModelCatalogSource, ModelInfo, ModelSelectorConfig, NativeModelDefaults, PermissionPolicy,
        ReasoningOption,
    },
    profile::{ExecutorConfig, ProviderRunOptions},
    stdout_dup::create_stdout_pipe_writer,
    workflow_mcp::{WorkflowMcpReadiness, WorkflowPromptGate},
};

#[derive(Debug)]
struct DeferredCodexControl {
    client: Arc<OnceLock<Arc<AppServerClient>>>,
}

#[async_trait]
impl ExecutorControl for DeferredCodexControl {
    async fn send(
        &self,
        control: crate::executors::provider_adapter::DirectControl,
    ) -> Result<Vec<u8>, ExecutorError> {
        for _ in 0..100 {
            if let Some(client) = self.client.get() {
                return client.send_direct_control(control).await;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Err(ExecutorError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Codex app-server control peer was not initialized",
        )))
    }
}

/// Sandbox policy modes for Codex
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, JsonSchema, AsRefStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum SandboxMode {
    Auto,
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

/// Determines when the user is consulted to approve Codex actions.
///
/// - `UnlessTrusted`: Read-only commands are auto-approved. Everything else will
///   ask the user to approve.
/// - `OnRequest`: The model decides when to ask the user for approval.
/// - `Never`: Commands never ask for approval. Commands that fail in the
///   restricted sandbox are not retried.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, JsonSchema, AsRefStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum AskForApproval {
    UnlessTrusted,
    OnRequest,
    Never,
}

/// Reasoning effort for the underlying model.
#[derive(Debug, Clone, PartialEq, Eq, TS)]
#[ts(type = "string")]
pub struct ReasoningEffort(String);

impl ReasoningEffort {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn to_protocol(&self) -> ProtocolReasoningEffort {
        self.as_str()
            .parse()
            .expect("local Codex reasoning effort is always non-empty")
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ReasoningEffort {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            Err("reasoning effort must not be empty".to_string())
        } else {
            Ok(Self(value.to_string()))
        }
    }
}

impl Serialize for ReasoningEffort {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ReasoningEffort {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ReasoningEffort {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("ReasoningEffort")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "A non-empty reasoning effort value advertised by Codex model discovery or supplied by the user.",
            "minLength": 1,
        })
    }
}

/// Model reasoning summary style
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, JsonSchema, AsRefStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ReasoningSummary {
    Auto,
    Concise,
    Detailed,
    None,
}

/// Format for model reasoning summaries
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, JsonSchema, AsRefStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ReasoningSummaryFormat {
    None,
    Experimental,
}

enum CodexSessionAction {
    Chat {
        prompt: String,
        selected_skills: Vec<SelectedSkill>,
    },
    Review {
        target: ReviewTarget,
    },
}

#[derive(Derivative, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[derivative(Debug, PartialEq)]
pub struct Codex {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_for_approval: Option<AskForApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oss: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_reasoning_effort: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_mode_reasoning_effort: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_reasoning_summary: Option<ReasoningSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_reasoning_summary_format: Option<ReasoningSummaryFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_apply_patch_tool: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
    #[serde(default)]
    pub plan: bool,
    #[serde(flatten)]
    pub cmd: CmdOverrides,

    #[serde(skip)]
    #[ts(skip)]
    #[derivative(Debug = "ignore", PartialEq = "ignore")]
    approvals: Option<Arc<dyn ExecutorApprovalService>>,
}

#[async_trait]
impl StandardCodingAgentExecutor for Codex {
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
        self.spawn_slash_command(current_dir, prompt, None, vec![], env)
            .await
    }

    async fn spawn_with_selected_skills(
        &self,
        current_dir: &Path,
        prompt: &str,
        selected_skills: &[SelectedSkill],
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.spawn_slash_command(current_dir, prompt, None, selected_skills.to_vec(), env)
            .await
    }

    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        _reset_to_message_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.spawn_slash_command(current_dir, prompt, Some(session_id), vec![], env)
            .await
    }

    async fn spawn_follow_up_with_selected_skills(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        _reset_to_message_id: Option<&str>,
        selected_skills: &[SelectedSkill],
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.spawn_slash_command(
            current_dir,
            prompt,
            Some(session_id),
            selected_skills.to_vec(),
            env,
        )
        .await
    }

    fn normalize_logs(
        &self,
        msg_store: Arc<MsgStore>,
        worktree_path: &Path,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        normalize_logs(msg_store, worktree_path)
    }

    fn default_mcp_config_path(&self) -> Option<PathBuf> {
        codex_home().map(|home| home.join("config.toml"))
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        if let Some(timestamp) = codex_home()
            .and_then(|home| std::fs::metadata(home.join("auth.json")).ok())
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

        let installation_indicator_found = codex_home()
            .map(|home| home.join("version.json").exists())
            .unwrap_or(false);

        if mcp_config_found
            || installation_indicator_found
            || crate::command::is_command_installed(
                crate::command::CODEX_DEFAULT_BASE_COMMAND,
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
        let permission_policy = if self.plan {
            PermissionPolicy::Plan
        } else if matches!(self.ask_for_approval, None | Some(AskForApproval::Never)) {
            PermissionPolicy::Auto
        } else {
            PermissionPolicy::Supervised
        };

        ExecutorConfig {
            executor: BaseCodingAgent::Codex,
            variant: None,
            model_id: self.model.clone().map(Some),
            agent_id: None,
            reasoning_id: self
                .model_reasoning_effort
                .as_ref()
                .map(|e| Some(e.as_str().to_string())),
            permission_policy: Some(permission_policy),
            provider_options: Some(ProviderRunOptions::Codex {
                service_tier: self.service_tier.clone(),
                plan_mode_reasoning_effort: self
                    .plan_mode_reasoning_effort
                    .as_ref()
                    .map(ToString::to_string),
            }),
        }
    }

    async fn discover_options(
        &self,
        workdir: Option<&std::path::Path>,
        repo_path: Option<&std::path::Path>,
    ) -> Result<futures::stream::BoxStream<'static, json_patch::Patch>, ExecutorError> {
        let skills_cwd = workdir.or(repo_path).map(Path::to_path_buf);
        let mut options = ExecutorDiscoveredOptions {
            model_selector: ModelSelectorConfig {
                models: fallback_models(),
                native_defaults: self
                    .profile
                    .is_none()
                    .then(|| {
                        crate::agent_settings::native_model_defaults_for(
                            Self::DIRECT_PROVIDER,
                            workdir.or(repo_path),
                            &self.cmd,
                        )
                    })
                    .flatten(),
                catalog_source: Some(ModelCatalogSource::Builtin),
                permissions: vec![
                    PermissionPolicy::Auto,
                    PermissionPolicy::Supervised,
                    PermissionPolicy::Plan,
                ],
                ..Default::default()
            },
            slash_commands: slash_commands::supported_slash_commands(),
            ..Default::default()
        };
        options.loading_models = true;
        options.loading_slash_commands = true;
        options.loading_skills = skills_cwd.is_some();
        let initial_patch = patch::executor_discovered_options(options.clone());

        let this = self.clone();
        let models_cwd = skills_cwd.clone();
        let discovery_stream = async_stream::stream! {
            let slash_commands = slash_commands::supported_slash_commands();
            yield patch::update_slash_commands(slash_commands);
            yield patch::slash_commands_loaded();

            // The catalog and inherited config are separate projections from
            // the same app-server authority. A catalog recommendation must not
            // replace the user's configured model or effort.
            let model_dir = models_cwd
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            apply_model_discovery(&mut options, this.discover_models(model_dir).await);
            // Keep the following skill/command patches independent from the
            // model result, including when one of the native queries failed.
            options.loading_slash_commands = false;
            yield patch::executor_discovered_options(options.clone());

            let Some(skills_cwd) = skills_cwd else {
                return;
            };
            let result = tokio::time::timeout(
                SKILLS_DISCOVERY_TIMEOUT,
                this.discover_skills(skills_cwd),
            )
            .await;

            match result {
                Ok(Ok((skills, errors))) => {
                    let mut slash_commands = slash_commands::supported_slash_commands();
                    slash_commands.extend(slash_commands::skill_slash_commands(&skills));
                    yield patch::update_slash_commands(slash_commands);
                    yield patch::update_skills(skills);
                    yield patch::update_skill_errors(errors);
                }
                Ok(Err(err)) => {
                    tracing::warn!("Failed to discover Codex skills: {err}");
                    yield patch::update_skill_errors(vec![CodexSkillLoadError {
                        path: PathBuf::new(),
                        message: format!("Failed to discover Codex skills: {err}"),
                    }]);
                }
                Err(_) => {
                    yield patch::update_skill_errors(vec![CodexSkillLoadError {
                        path: PathBuf::new(),
                        message: "Timed out discovering Codex skills".to_string(),
                    }]);
                }
            }
            yield patch::skills_loaded();
        };

        Ok(Box::pin(
            futures::stream::once(async move { initial_patch }).chain(discovery_stream),
        ))
    }

    async fn spawn_review(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let review_target = ReviewTarget::Custom {
            instructions: prompt.to_string(),
        };
        self.spawn_review_target(current_dir, review_target, session_id, env)
            .await
    }
}

impl Codex {
    /// V1 direct-runtime metadata and decoder entry point.
    pub const DIRECT_PROVIDER: super::provider_adapter::DirectProvider =
        super::provider_adapter::DirectProvider::Codex;

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
        if let Some(model_id) = &executor_config.model_id {
            self.model = model_id.clone();
        }
        if let Some(reasoning_id) = &executor_config.reasoning_id {
            self.model_reasoning_effort = reasoning_id
                .as_ref()
                .map(|value| value.parse())
                .transpose()
                .map_err(ExecutorError::InvalidConfiguration)?;
        }
        if let Some(permission_policy) = &executor_config.permission_policy {
            match permission_policy {
                crate::model_selector::PermissionPolicy::Auto => {
                    self.ask_for_approval = Some(AskForApproval::Never);
                    self.plan = false;
                }
                crate::model_selector::PermissionPolicy::Supervised => {
                    if matches!(self.ask_for_approval, None | Some(AskForApproval::Never)) {
                        self.ask_for_approval = Some(AskForApproval::OnRequest);
                    }
                    self.plan = false;
                }
                crate::model_selector::PermissionPolicy::Plan => {
                    self.plan = true;
                }
            }
        }
        if let Some(ProviderRunOptions::Codex {
            service_tier,
            plan_mode_reasoning_effort,
        }) = &executor_config.provider_options
        {
            if let Some(tier) = service_tier {
                if !matches!(tier.as_str(), "default" | "priority" | "flex") {
                    return Err(ExecutorError::InvalidConfiguration(format!(
                        "Unsupported Codex service tier: {tier}"
                    )));
                }
                self.service_tier = Some(tier.clone());
            }
            if let Some(effort) = plan_mode_reasoning_effort {
                self.plan_mode_reasoning_effort = Some(
                    effort
                        .parse()
                        .map_err(ExecutorError::InvalidConfiguration)?,
                );
            }
        }
        Ok(())
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
        selected_skills: &[SelectedSkill],
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        match intent {
            super::provider_adapter::DirectIntent::Initial => {
                self.spawn_slash_command(current_dir, prompt, None, selected_skills.to_vec(), env)
                    .await
            }
            super::provider_adapter::DirectIntent::FollowUp
            | super::provider_adapter::DirectIntent::Resume => {
                self.spawn_slash_command(
                    current_dir,
                    prompt,
                    session_id,
                    selected_skills.to_vec(),
                    env,
                )
                .await
            }
            super::provider_adapter::DirectIntent::Review => {
                self.spawn_review_target(
                    current_dir,
                    ReviewTarget::Custom {
                        instructions: prompt.to_string(),
                    },
                    session_id,
                    env,
                )
                .await
            }
        }
    }

    async fn spawn_review_target(
        &self,
        current_dir: &Path,
        review_target: ReviewTarget,
        session_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let command_parts = self.build_command_builder()?.build_initial()?;
        let action = CodexSessionAction::Review {
            target: review_target,
        };
        self.spawn_inner(current_dir, command_parts, action, session_id, env)
            .await
    }
}

impl Codex {
    pub const DEFAULT_BASE_COMMAND: &'static str = "codex";
    const DISABLE_NATIVE_MEMORY_ARGS: [&'static str; 6] = [
        "-c",
        "features.memories=false",
        "-c",
        "memories.generate_memories=false",
        "-c",
        "memories.use_memories=false",
    ];

    /// Resolve the user's locally installed Codex executable from PATH.
    ///
    /// The app-server protocol crate is a compile-time type dependency; it
    /// does not authorize replacing the user's runtime with a pinned npm
    /// binary. Explicit command overrides remain supported by `CmdOverrides`.
    pub fn base_command() -> String {
        Self::DEFAULT_BASE_COMMAND.to_string()
    }

    fn launch_context(&self, program_path: &Path, args: &[String], current_dir: &Path) -> String {
        let codex_config = codex_home()
            .map(|home| home.join("config.toml").display().to_string())
            .unwrap_or_else(|| "<unresolved>".to_string());

        let mut lines = vec![
            format!(
                "base command: {}",
                self.cmd
                    .base_command_override
                    .clone()
                    .unwrap_or_else(Self::base_command)
            ),
            format!(
                "base_command_override: {}",
                if self.cmd.base_command_override.is_some() {
                    "set"
                } else {
                    "not set"
                }
            ),
            format!("resolved executable: {}", program_path.display()),
            format!("args: {args:?}"),
            format!("cwd: {}", current_dir.display()),
            format!("codex config: {codex_config}"),
        ];

        if let Some(model) = &self.model {
            lines.push(format!("model: {model}"));
        }
        if let Some(model_provider) = &self.model_provider {
            lines.push(format!("model_provider: {model_provider}"));
        }
        if let Some(profile) = &self.profile {
            lines.push(format!("profile: {profile}"));
        }
        if let Some(additional_params) = &self.cmd.additional_params {
            lines.push(format!("additional_params: {additional_params:?}"));
        }

        lines.join("\n")
    }

    fn build_command_builder(&self) -> Result<CommandBuilder, CommandBuildError> {
        command_adapter::CodexCommandAdapter::new(self).build()
    }

    async fn discover_skills(
        &self,
        cwd: PathBuf,
    ) -> Result<(Vec<CodexSkillDescription>, Vec<CodexSkillLoadError>), ExecutorError> {
        let request_cwd = cwd.clone();
        let response = self
            .with_discovery_app_server(&cwd, move |client| async move {
                client.skills_list(request_cwd).await
            })
            .await?;
        Ok(skills_list_response_to_discovery(response))
    }

    async fn discover_models(&self, cwd: PathBuf) -> Result<CodexModelDiscovery, ExecutorError> {
        let request_cwd = cwd.to_string_lossy().into_owned();
        let profile = self.profile.clone();
        self.with_discovery_app_server(&cwd, move |client| async move {
            let (models, native_defaults) = tokio::join!(
                tokio::time::timeout(MODELS_DISCOVERY_TIMEOUT, client.model_list()),
                tokio::time::timeout(
                    MODELS_DISCOVERY_TIMEOUT,
                    client.model_configuration_read(request_cwd, profile.as_deref()),
                ),
            );
            Ok(CodexModelDiscovery {
                models: models
                    .map_err(|_| {
                        ExecutorError::InvalidConfiguration("Codex model/list timed out".into())
                    })
                    .and_then(|result| result)
                    .map(model_list_response_to_model_infos),
                native_defaults: native_defaults
                    .map_err(|_| {
                        ExecutorError::InvalidConfiguration("Codex config/read timed out".into())
                    })
                    .and_then(|result| result),
            })
        })
        .await
    }

    async fn with_discovery_app_server<T, F, Fut>(
        &self,
        current_dir: &Path,
        task: F,
    ) -> Result<T, ExecutorError>
    where
        F: FnOnce(Arc<AppServerClient>) -> Fut,
        Fut: std::future::Future<Output = Result<T, ExecutorError>>,
    {
        let command_parts = self.build_command_builder()?.build_initial()?;
        let (program_path, args) = command_parts.into_resolved().await?;
        let launch_context = self.launch_context(&program_path, &args, current_dir);

        let mut process = Command::new(&program_path);
        process
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .current_dir(current_dir)
            .env("NPM_CONFIG_LOGLEVEL", "error")
            .env("NODE_NO_WARNINGS", "1")
            .env("NO_COLOR", "1")
            .env("RUST_LOG", "error")
            .args(&args);

        ExecutionEnv::new(RepoContext::default(), false, String::new())
            .with_profile(&self.cmd)
            .apply_to_command(&mut process);

        let mut child = process.group_spawn_no_window().map_err(|err| {
            ExecutorError::Io(std::io::Error::other(format!(
                "failed to spawn Codex app-server for discovery: {err}\n\nCodex launch context:\n{}",
                launch_context
            )))
        })?;

        let child_stdout = child.inner().stdout.take().ok_or_else(|| {
            ExecutorError::Io(std::io::Error::other("Codex app server missing stdout"))
        })?;
        let child_stdin = child.inner().stdin.take().ok_or_else(|| {
            ExecutorError::Io(std::io::Error::other("Codex app server missing stdin"))
        })?;
        // Native startup/config warnings can fill stderr before the handshake
        // is written. Drain without retaining or exposing potentially private
        // diagnostic bytes, and stop the reader when this request is dropped.
        let stderr_drain = child.inner().stderr.take().map(|mut stderr| {
            tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
            }))
        });

        let cancel = tokio_util::sync::CancellationToken::new();
        let (exit_signal_tx, _exit_signal_rx) = tokio::sync::oneshot::channel();
        let exit_signal_tx = ExitSignalSender::new(exit_signal_tx);
        let client = AppServerClient::new(
            LogWriter::new(tokio::io::sink()),
            None,
            false,
            self.plan,
            None,
            RepoContext::default(),
            false,
            String::new(),
            cancel.clone(),
        );
        let rpc_peer = JsonRpcPeer::spawn(
            child_stdin,
            child_stdout,
            client.clone(),
            exit_signal_tx,
            cancel.clone(),
        );
        client.connect(rpc_peer);

        let result = async {
            tokio::time::timeout(MODELS_DISCOVERY_TIMEOUT, client.initialize())
                .await
                .map_err(|_| {
                    ExecutorError::InvalidConfiguration(
                        "Codex discovery initialization timed out".into(),
                    )
                })??;
            task(client).await
        }
        .await;

        cancel.cancel();
        let _ = child.kill().await;
        if let Some(stderr_drain) = stderr_drain {
            stderr_drain.abort();
            let _ = stderr_drain.await;
        }

        result
    }

    fn build_thread_start_params(&self, cwd: &Path) -> ThreadStartParams {
        let sandbox = match self.sandbox.as_ref() {
            None | Some(SandboxMode::Auto) => Some(V2SandboxMode::WorkspaceWrite), // match the Auto preset in codex
            Some(SandboxMode::ReadOnly) => Some(V2SandboxMode::ReadOnly),
            Some(SandboxMode::WorkspaceWrite) => Some(V2SandboxMode::WorkspaceWrite),
            Some(SandboxMode::DangerFullAccess) => Some(V2SandboxMode::DangerFullAccess),
        };

        let approval_policy = match self.ask_for_approval.as_ref() {
            None if matches!(self.sandbox.as_ref(), None | Some(SandboxMode::Auto)) => {
                // match the Auto preset in codex
                Some(V2AskForApproval::OnRequest)
            }
            None => None,
            Some(AskForApproval::UnlessTrusted) => Some(V2AskForApproval::UnlessTrusted),
            Some(AskForApproval::OnRequest) => Some(V2AskForApproval::OnRequest),
            Some(AskForApproval::Never) => Some(V2AskForApproval::Never),
        };

        let mut config = self.build_config_overrides();
        // V1 top-level params that moved into config overrides in v2
        if let Some(profile) = &self.profile {
            config
                .get_or_insert_with(HashMap::new)
                .insert("profile".to_string(), Value::String(profile.clone()));
        }
        if let Some(include) = self.include_apply_patch_tool {
            config
                .get_or_insert_with(HashMap::new)
                .insert("include_apply_patch_tool".to_string(), Value::Bool(include));
        }
        if let Some(compact) = &self.compact_prompt {
            config
                .get_or_insert_with(HashMap::new)
                .insert("compact_prompt".to_string(), Value::String(compact.clone()));
        }
        let model = self.model.as_deref();
        let service_tier = self.service_tier.clone().map(Some);

        ThreadStartParams {
            model: model.map(|m| m.to_string()),
            cwd: Some(cwd.to_string_lossy().to_string()),
            approval_policy,
            sandbox,
            config,
            base_instructions: self.base_instructions.clone(),
            model_provider: self.model_provider.clone(),
            developer_instructions: self.developer_instructions.clone(),
            service_tier,
            ..Default::default()
        }
    }

    fn build_config_overrides(&self) -> Option<HashMap<String, Value>> {
        let mut overrides = HashMap::new();
        if let Some(effort) = &self.plan_mode_reasoning_effort {
            overrides.insert(
                "plan_mode_reasoning_effort".into(),
                Value::String(effort.to_string()),
            );
        }

        if let Some(effort) = &self.model_reasoning_effort {
            overrides.insert(
                "model_reasoning_effort".to_string(),
                Value::String(effort.as_str().to_string()),
            );
        }

        if let Some(summary) = &self.model_reasoning_summary {
            overrides.insert(
                "model_reasoning_summary".to_string(),
                Value::String(summary.as_ref().to_string()),
            );
        }

        if let Some(format) = &self.model_reasoning_summary_format
            && format != &ReasoningSummaryFormat::None
        {
            overrides.insert(
                "model_reasoning_summary_format".to_string(),
                Value::String(format.as_ref().to_string()),
            );
        }

        if overrides.is_empty() {
            None
        } else {
            Some(overrides)
        }
    }

    async fn spawn_inner(
        &self,
        current_dir: &Path,
        command_parts: CommandParts,
        action: CodexSessionAction,
        resume_session: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        if matches!(self.ask_for_approval, Some(AskForApproval::UnlessTrusted)) {
            return Err(ExecutorError::InvalidConfiguration("Codex no longer supports the untrusted approval policy; select on-request explicitly".into()));
        }
        let params = self.build_thread_start_params(current_dir);
        let resume_session = resume_session.map(|s| s.to_string());

        self.spawn_app_server(
            current_dir,
            command_parts,
            env,
            move |client, _, readiness, cancel, prompt_gate| async move {
                match action {
                    CodexSessionAction::Chat {
                        prompt,
                        selected_skills,
                    } => {
                        Self::launch_codex_agent(
                            params,
                            resume_session,
                            prompt,
                            selected_skills,
                            client,
                            readiness,
                            cancel,
                            prompt_gate,
                        )
                        .await
                    }
                    CodexSessionAction::Review { target } => {
                        review::launch_codex_review(
                            params,
                            resume_session,
                            target,
                            client,
                            readiness,
                            cancel,
                            prompt_gate,
                        )
                        .await
                    }
                }
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn launch_codex_agent(
        thread_start_params: ThreadStartParams,
        resume_session: Option<String>,
        combined_prompt: String,
        selected_skills: Vec<SelectedSkill>,
        client: Arc<AppServerClient>,
        readiness: Option<WorkflowMcpReadiness>,
        cancel: tokio_util::sync::CancellationToken,
        prompt_gate: WorkflowPromptGate,
    ) -> Result<(), ExecutorError> {
        let account = client.get_account().await?;
        if account.requires_openai_auth && account.account.is_none() {
            return Err(ExecutorError::AuthRequired(
                "Codex authentication required".to_string(),
            ));
        }

        let (thread_id, resolved_model) = match resume_session {
            None => {
                let response = client.thread_start(thread_start_params).await?;
                (response.thread.id, response.model)
            }
            Some(session_id) => {
                let response = client
                    .thread_resume(resume_params_from(session_id, thread_start_params))
                    .await?;
                tracing::debug!("resumed thread, thread_id={}", response.thread.id);
                (response.thread.id, response.model)
            }
        };

        client.set_resolved_model(resolved_model);
        client.register_session(&thread_id).await?;
        WorkflowMcpReadiness::wait_optional(readiness, &cancel).await?;
        let collaboration_mode = client.initial_collaboration_mode()?;
        let input = build_chat_input(combined_prompt, selected_skills);
        client
            .turn_start_with_mode(thread_id, input, Some(collaboration_mode))
            .await?;
        prompt_gate.open();

        Ok(())
    }

    /// Common boilerplate for spawning a Codex app server process
    /// Handles process spawning, stdout/stderr piping, exit signal handling, client initialization, and error logging.
    /// Delegates the actual Codex session logic to the provided `task` closure.
    async fn spawn_app_server<F, Fut>(
        &self,
        current_dir: &Path,
        command_parts: CommandParts,
        env: &ExecutionEnv,
        task: F,
    ) -> Result<SpawnedChild, ExecutorError>
    where
        F: FnOnce(
                Arc<AppServerClient>,
                ExitSignalSender,
                Option<WorkflowMcpReadiness>,
                tokio_util::sync::CancellationToken,
                WorkflowPromptGate,
            ) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = Result<(), ExecutorError>> + Send + 'static,
    {
        let workflow_mcp = crate::workflow_mcp::ScopedWorkflowMcp::from_execution_env(env)?;
        let mut launch_env = env.clone().with_profile(&self.cmd);
        let readiness = WorkflowMcpReadiness::start(&mut launch_env).await?;
        let startup_deadline = readiness
            .as_ref()
            .map(WorkflowMcpReadiness::startup_deadline);
        let prompt_gate = WorkflowPromptGate::new(readiness.is_some());
        let command_parts =
            command_adapter::append_workflow_mcp(command_parts, workflow_mcp.as_ref());
        let (program_path, args) = command_parts.into_resolved().await?;
        let launch_context = self.launch_context(&program_path, &args, current_dir);
        tracing::debug!("Launching Codex app-server:\n{}", launch_context);

        let mut process = Command::new(&program_path);
        process
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .current_dir(current_dir)
            .env("NPM_CONFIG_LOGLEVEL", "error")
            .env("NODE_NO_WARNINGS", "1")
            .env("NO_COLOR", "1")
            .env("RUST_LOG", "error")
            .args(&args);

        launch_env.apply_to_command(&mut process);

        let spawn_error_context = launch_context.clone();
        let mut child = process.group_spawn_no_window().map_err(|err| {
            ExecutorError::Io(std::io::Error::other(format!(
                "failed to spawn Codex app-server: {err}\n\nCodex launch context:\n{}",
                spawn_error_context
            )))
        })?;

        let child_stdout = child.inner().stdout.take().ok_or_else(|| {
            ExecutorError::Io(std::io::Error::other("Codex app server missing stdout"))
        })?;
        let child_stdin = child.inner().stdin.take().ok_or_else(|| {
            ExecutorError::Io(std::io::Error::other("Codex app server missing stdin"))
        })?;

        let new_stdout = create_stdout_pipe_writer(&mut child)?;
        let (exit_signal_tx, exit_signal_rx) = tokio::sync::oneshot::channel();
        let cancel = tokio_util::sync::CancellationToken::new();

        let auto_approve = matches!(
            (&self.sandbox, &self.ask_for_approval),
            (Some(SandboxMode::DangerFullAccess), None)
        );
        let plan_mode = self.plan;
        let reasoning_effort = (if self.plan {
            self.plan_mode_reasoning_effort.as_ref()
        } else {
            self.model_reasoning_effort.as_ref()
        })
        .map(ReasoningEffort::to_protocol);
        let approvals = self.approvals.clone();
        let repo_context = env.repo_context.clone();
        let commit_reminder = env.commit_reminder;
        let commit_reminder_prompt = env.commit_reminder_prompt.clone();
        let cancel_for_task = cancel.clone();
        let client_slot = Arc::new(OnceLock::new());
        let control = prompt_gate.wrap(Arc::new(DeferredCodexControl {
            client: client_slot.clone(),
        }));

        tokio::spawn(async move {
            let exit_signal_tx = ExitSignalSender::new(exit_signal_tx);
            let log_writer = LogWriter::new(new_stdout);

            // Initialize the AppServerClient
            let client = AppServerClient::new(
                log_writer.clone(),
                approvals,
                auto_approve,
                plan_mode,
                reasoning_effort,
                repo_context,
                commit_reminder,
                commit_reminder_prompt,
                cancel_for_task.clone(),
            );
            let rpc_peer = JsonRpcPeer::spawn(
                child_stdin,
                child_stdout,
                client.clone(),
                exit_signal_tx.clone(),
                cancel_for_task.clone(),
            );
            client.connect(rpc_peer);
            // Do not expose the control peer until it owns a connected RPC
            // transport.  The process host can send cancellation immediately
            // after launch, before the initialization request has completed.
            let _ = client_slot.set(client.clone());

            let result =
                WorkflowMcpReadiness::bound_startup(startup_deadline, &cancel_for_task, async {
                    client.initialize().await?;
                    task(
                        client,
                        exit_signal_tx.clone(),
                        readiness,
                        cancel_for_task.clone(),
                        prompt_gate,
                    )
                    .await
                })
                .await
                .and_then(|result| result);

            if let Err(err) = result {
                match &err {
                    ExecutorError::Io(io_err)
                        if io_err.kind() == std::io::ErrorKind::BrokenPipe =>
                    {
                        // Broken pipe likely means the parent process exited, so we can ignore it
                        return;
                    }
                    ExecutorError::AuthRequired(message) => {
                        log_writer
                            .log_raw(&Error::auth_required(message.clone()).raw())
                            .await
                            .ok();
                        exit_signal_tx
                            .send_exit_signal(ExecutorExitResult::Failure)
                            .await;
                        return;
                    }
                    _ => {
                        tracing::error!("Codex spawn error: {}", err);
                        let error = format!("{}\n\nCodex launch context:\n{}", err, launch_context);
                        log_writer
                            .log_raw(&Error::launch_error(error).raw())
                            .await
                            .ok();
                    }
                }
                exit_signal_tx
                    .send_exit_signal(ExecutorExitResult::Failure)
                    .await;
            }
        });

        Ok(SpawnedChild {
            child,
            exit_signal: Some(exit_signal_rx),
            cancel: Some(cancel),
            control: Some(control),
        })
    }
}

fn build_chat_input(
    combined_prompt: String,
    selected_skills: Vec<SelectedSkill>,
) -> Vec<UserInput> {
    let mut input = selected_skills
        .into_iter()
        .map(|skill| UserInput::Skill {
            name: skill.name,
            path: skill.path,
        })
        .collect::<Vec<_>>();
    input.push(UserInput::Text {
        text: combined_prompt,
        text_elements: vec![],
    });
    input
}

fn skills_list_response_to_discovery(
    response: SkillsListResponse,
) -> (Vec<CodexSkillDescription>, Vec<CodexSkillLoadError>) {
    let mut skills = Vec::new();
    let mut errors = Vec::new();

    for entry in response.data {
        for skill in entry.skills {
            skills.push(CodexSkillDescription {
                name: skill.name,
                description: skill.description,
                short_description: skill
                    .interface
                    .and_then(|interface| interface.short_description)
                    .or(skill.short_description),
                path: skill.path.to_path_buf(),
                scope: skill_scope_to_string(skill.scope).to_string(),
                enabled: skill.enabled,
            });
        }

        for error in entry.errors {
            errors.push(CodexSkillLoadError {
                path: error.path,
                message: error.message,
            });
        }
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
    errors.sort_by(|a, b| a.path.cmp(&b.path));
    (skills, errors)
}

fn skill_scope_to_string(scope: SkillScope) -> &'static str {
    match scope {
        SkillScope::User => "user",
        SkillScope::Repo => "repo",
        SkillScope::System => "system",
        SkillScope::Admin => "admin",
    }
}

struct CodexModelDiscovery {
    models: Result<Vec<ModelInfo>, ExecutorError>,
    native_defaults: Result<NativeModelDefaults, ExecutorError>,
}

fn apply_model_discovery(
    options: &mut ExecutorDiscoveredOptions,
    result: Result<CodexModelDiscovery, ExecutorError>,
) {
    let mut errors = Vec::new();
    match result {
        Ok(discovery) => {
            match discovery.models {
                Ok(models) if !models.is_empty() => {
                    options.model_selector.models = models;
                    options.model_selector.catalog_source = Some(ModelCatalogSource::Native);
                }
                Ok(_) => {
                    errors.push("Codex returned no models; capabilities are unconfirmed".into())
                }
                Err(error) => errors.push(format!("Codex model discovery failed: {error}")),
            }
            match discovery.native_defaults {
                Ok(defaults) => options.model_selector.native_defaults = Some(defaults),
                Err(error) => errors.push(format!(
                    "Codex native configuration discovery failed: {error}"
                )),
            }
        }
        Err(error) => errors.push(format!("Codex model/config discovery failed: {error}")),
    }
    options.error = (!errors.is_empty()).then(|| errors.join("; "));
    options.loading_models = false;
}

/// Convert a live `model/list` response into the UI model-selector shape,
/// dropping models hidden from the default picker.
fn model_list_response_to_model_infos(response: client::ModelCatalogResponse) -> Vec<ModelInfo> {
    response
        .data
        .into_iter()
        .filter(|model| !model.hidden)
        .map(|model| {
            let reasoning_options = ReasoningOption::from_names_with_default(
                model
                    .supported_reasoning_efforts
                    .iter()
                    // Use the same boundary as execution. The native protocol
                    // preserves future effort IDs as Custom(String).
                    .filter(|option| option.reasoning_effort.parse::<ReasoningEffort>().is_ok())
                    .map(|option| option.reasoning_effort.clone()),
                model.default_reasoning_effort.as_deref(),
            );
            let reasoning = if model.supported_reasoning_efforts.is_empty() {
                crate::model_selector::CapabilitySupport::Unsupported
            } else if reasoning_options.is_empty() {
                crate::model_selector::CapabilitySupport::Unknown
            } else {
                crate::model_selector::CapabilitySupport::Supported
            };
            ModelInfo {
                id: model.id,
                name: model.display_name,
                provider_id: None,
                reasoning_options,
                capabilities: Some(crate::model_selector::ModelCapabilities {
                    reasoning,
                    input_modalities: model.input_modalities,
                    service_tiers: model.service_tiers,
                    default_service_tier: model.default_service_tier,
                    is_default: model.is_default,
                    ..Default::default()
                }),
            }
        })
        .collect()
}

/// Static model list used before/instead of a live `model/list` response
/// (discovery failure, timeout, or empty result).
fn fallback_models() -> Vec<ModelInfo> {
    let xhigh_reasoning_options =
        ReasoningOption::from_names(["none", "minimal", "low", "medium", "high", "xhigh"]);
    let max_reasoning_options =
        ReasoningOption::from_names(["low", "medium", "high", "xhigh", "max"]);
    let ultra_reasoning_options =
        ReasoningOption::from_names(["low", "medium", "high", "xhigh", "max", "ultra"]);

    let mut models = [
        ("gpt-5.5", "GPT-5.5"),
        ("gpt-5.4", "GPT-5.4"),
        ("gpt-5.4-mini", "GPT-5.4 Mini"),
        ("gpt-5.3-codex", "GPT-5.3 Codex"),
        ("gpt-5.3-codex-spark", "GPT-5.3 Codex Spark"),
        ("gpt-5.2", "GPT-5.2"),
    ]
    .into_iter()
    .map(|(id, name)| ModelInfo {
        id: id.to_string(),
        name: name.to_string(),
        provider_id: None,
        reasoning_options: xhigh_reasoning_options.clone(),
        ..Default::default()
    })
    .collect::<Vec<_>>();
    models.splice(
        0..0,
        [
            ModelInfo {
                id: "gpt-5.6-sol".to_string(),
                name: "GPT-5.6-Sol".to_string(),
                provider_id: None,
                reasoning_options: ultra_reasoning_options.clone(),
                ..Default::default()
            },
            ModelInfo {
                id: "gpt-5.6-terra".to_string(),
                name: "GPT-5.6-Terra".to_string(),
                provider_id: None,
                reasoning_options: ultra_reasoning_options,
                ..Default::default()
            },
            ModelInfo {
                id: "gpt-5.6-luna".to_string(),
                name: "GPT-5.6-Luna".to_string(),
                provider_id: None,
                reasoning_options: max_reasoning_options,
                ..Default::default()
            },
        ],
    );
    // Static names are only display hints, not current native capabilities.
    for model in &mut models {
        model.reasoning_options.clear();
    }
    models
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use codex_app_server_protocol::{Model, ModelListResponse, ReasoningEffortOption, UserInput};
    use codex_protocol::openai_models::ReasoningEffort as ProtocolReasoningEffort;
    use serde_json::json;

    use super::{
        AskForApproval, Codex, ReasoningEffort, build_chat_input, fallback_models,
        model_list_response_to_model_infos, resume_params_from,
    };
    use crate::{
        actions::SelectedSkill,
        executors::{BaseCodingAgent, StandardCodingAgentExecutor},
        profile::ExecutorConfig,
    };

    fn test_executor() -> Codex {
        serde_json::from_value(json!({})).expect("empty Codex config should deserialize")
    }

    #[tokio::test]
    async fn model_discovery_reads_native_config_and_catalog_from_one_read_only_process() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("native-discovery.cjs");
        let trace = directory.path().join("requests.jsonl");
        std::fs::write(&script, r#"
const fs = require('node:fs');
const readline = require('node:readline');
// A synchronous write larger than a pipe must be drained before initialize.
fs.writeSync(2, 'private-fixture-stderr'.repeat(16384));
readline.createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  fs.appendFileSync(process.env.VK_FIXTURE_TRACE, JSON.stringify({ method: request.method, params: request.params, pid: process.pid, cwd: process.cwd(), scope: process.env.VK_FIXTURE_SCOPE }) + '\n');
  if (!Object.hasOwn(request, 'id')) return;
  let result;
  if (request.method === 'initialize') result = { userAgent: 'codex/0.115.0', platformFamily: process.platform === 'win32' ? 'windows' : 'unix', platformOs: process.platform };
  else if (request.method === 'config/read') result = { config: { model: 'unlisted-native-model', model_reasoning_effort: 'future-effort', api_key: 'private-fixture-token' } };
  else if (request.method === 'model/list') {
    if (process.env.VK_FIXTURE_FAIL_CATALOG === '1') { process.stdout.write(JSON.stringify({ id: request.id, error: { code: -32000, message: 'catalog unavailable' } }) + '\n'); return; }
    result = { data: [{ id: 'catalog-model', model: 'catalog-model', displayName: 'Catalog Model', defaultReasoningEffort: 'low', supportedReasoningEfforts: [{reasoningEffort:'low'}] }], nextCursor: null };
  } else { process.exit(21); }
  process.stdout.write(JSON.stringify({ id: request.id, result }) + '\n');
});
"#).unwrap();
        for fail_catalog in [false, true] {
            std::fs::write(&trace, "").unwrap();
            let mut executor = test_executor();
            executor.cmd.base_command_override = Some(format!("node \"{}\"", script.display()));
            executor.cmd.env = Some(std::collections::HashMap::from([
                (
                    "VK_FIXTURE_TRACE".into(),
                    trace.to_string_lossy().into_owned(),
                ),
                ("VK_FIXTURE_SCOPE".into(), "selected-profile".into()),
                (
                    "VK_FIXTURE_FAIL_CATALOG".into(),
                    if fail_catalog { "1" } else { "0" }.into(),
                ),
            ]));
            let result = executor
                .discover_models(directory.path().to_path_buf())
                .await;
            let mut options = crate::executor_discovery::ExecutorDiscoveredOptions::default();
            options.model_selector.models = fallback_models();
            options.model_selector.catalog_source =
                Some(crate::model_selector::ModelCatalogSource::Builtin);
            super::apply_model_discovery(&mut options, result);
            let defaults = options.model_selector.native_defaults.as_ref().unwrap();
            assert_eq!(defaults.model_id.as_deref(), Some("unlisted-native-model"));
            assert_eq!(defaults.reasoning_id.as_deref(), Some("future-effort"));
            assert_eq!(options.error.is_some(), fail_catalog);
            assert!(!options.loading_models);
            assert_eq!(
                options.model_selector.catalog_source,
                Some(if fail_catalog {
                    crate::model_selector::ModelCatalogSource::Builtin
                } else {
                    crate::model_selector::ModelCatalogSource::Native
                })
            );
            assert!(
                !serde_json::to_string(&options)
                    .unwrap()
                    .contains("private-fixture-token")
            );
            assert!(
                !serde_json::to_string(&options)
                    .unwrap()
                    .contains("private-fixture-stderr")
            );
            assert_eq!(executor.get_preset_options().model_id, None);
            assert_eq!(executor.get_preset_options().reasoning_id, None);
            let requests: Vec<serde_json::Value> = std::fs::read_to_string(&trace)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(requests.len(), 4);
            assert!(
                requests
                    .iter()
                    .all(|request| request["pid"] == requests[0]["pid"])
            );
            assert!(
                requests
                    .iter()
                    .all(|request| request["scope"] == "selected-profile")
            );
            let config_request = requests
                .iter()
                .find(|request| request["method"] == "config/read")
                .unwrap();
            assert_eq!(
                config_request["params"]["cwd"],
                directory.path().to_string_lossy().as_ref()
            );
            assert_ne!(config_request["params"]["includeLayers"], true);
            assert!(requests.iter().all(|request| {
                !request["method"].as_str().unwrap().starts_with("thread/")
                    && !request["method"].as_str().unwrap().starts_with("turn/")
            }));
        }
    }

    #[test]
    fn model_discovery_failure_keeps_native_config_without_promoting_builtin_defaults() {
        use crate::{
            executor_discovery::ExecutorDiscoveredOptions,
            model_selector::{ModelCatalogSource, NativeModelDefaults},
        };
        let defaults = NativeModelDefaults {
            model_id: Some("configured-model".into()),
            reasoning_id: Some("configured-effort".into()),
            ..Default::default()
        };
        let mut options = ExecutorDiscoveredOptions::default();
        options.model_selector.native_defaults = Some(defaults.clone());
        options.model_selector.models = fallback_models();
        options.model_selector.catalog_source = Some(ModelCatalogSource::Builtin);
        super::apply_model_discovery(
            &mut options,
            Err(crate::executors::ExecutorError::InvalidConfiguration(
                "initialization failed".into(),
            )),
        );
        assert_eq!(options.model_selector.native_defaults, Some(defaults));
        assert_eq!(
            options.model_selector.catalog_source,
            Some(ModelCatalogSource::Builtin)
        );
        assert!(
            options
                .error
                .as_deref()
                .unwrap()
                .contains("initialization failed")
        );
        assert!(
            options
                .model_selector
                .models
                .iter()
                .all(|model| model.reasoning_options.is_empty())
        );
    }

    fn config_with_reasoning(reasoning_id: &str) -> ExecutorConfig {
        ExecutorConfig {
            executor: BaseCodingAgent::Codex,
            variant: None,
            model_id: None,
            agent_id: None,
            reasoning_id: Some(Some(reasoning_id.to_string())),
            permission_policy: None,
            provider_options: None,
        }
    }

    fn expected_memory_policy_args() -> Vec<String> {
        [
            "app-server",
            "-c",
            "features.memories=false",
            "-c",
            "memories.generate_memories=false",
            "-c",
            "memories.use_memories=false",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    #[test]
    fn app_server_uses_the_users_local_codex_command() {
        assert_eq!(Codex::base_command(), Codex::DEFAULT_BASE_COMMAND);
    }

    #[test]
    fn app_server_disables_native_memory_for_discovery_and_runtime() {
        let builder = test_executor()
            .build_command_builder()
            .expect("Codex app-server command should build");

        assert_eq!(builder.params, Some(expected_memory_policy_args()));
    }

    #[test]
    fn additional_params_can_explicitly_override_native_memory_policy() {
        let mut executor = test_executor();
        executor.cmd.additional_params = Some(vec![
            "-c".to_string(),
            "features.memories=true".to_string(),
            "-c".to_string(),
            "memories.generate_memories=true".to_string(),
            "-c".to_string(),
            "memories.use_memories=true".to_string(),
        ]);

        let params = executor
            .build_command_builder()
            .expect("Codex app-server command should build")
            .params
            .expect("Codex app-server command should have parameters");

        let expected_prefix = expected_memory_policy_args();
        assert_eq!(&params[..expected_prefix.len()], expected_prefix.as_slice());
        assert_eq!(
            &params[expected_prefix.len()..],
            [
                "-c",
                "features.memories=true",
                "-c",
                "memories.generate_memories=true",
                "-c",
                "memories.use_memories=true",
            ]
        );
    }

    #[test]
    fn service_tier_is_independent_of_model_identity() {
        let executor: Codex =
            serde_json::from_value(json!({"model":"custom-fast","service_tier":"priority"}))
                .unwrap();
        let params = executor.build_thread_start_params(Path::new("."));
        assert_eq!(params.model.as_deref(), Some("custom-fast"));
        assert_eq!(params.service_tier, Some(Some("priority".into())));
    }

    #[test]
    fn plan_effort_does_not_replace_regular_effort() {
        let executor: Codex = serde_json::from_value(
            json!({"model_reasoning_effort":"high","plan_mode_reasoning_effort":"low"}),
        )
        .unwrap();
        let params = executor.build_config_overrides().unwrap();
        assert_eq!(params["model_reasoning_effort"], "high");
        assert_eq!(params["plan_mode_reasoning_effort"], "low");
    }

    #[test]
    fn apply_overrides_preserves_codex_protocol_reasoning_values() {
        let mut executor = test_executor();
        executor
            .apply_overrides(&config_with_reasoning("minimal"))
            .unwrap();

        let overrides = executor
            .build_config_overrides()
            .expect("reasoning override should produce config");
        assert_eq!(
            overrides.get("model_reasoning_effort"),
            Some(&json!("minimal"))
        );
    }

    #[test]
    fn missing_reasoning_override_omits_model_reasoning_effort() {
        let executor = test_executor();

        let overrides = executor.build_config_overrides();

        assert_eq!(overrides, None);
    }

    #[test]
    fn apply_overrides_preserves_custom_reasoning_values() {
        let mut executor = test_executor();
        executor
            .apply_overrides(&config_with_reasoning("max"))
            .unwrap();

        let overrides = executor
            .build_config_overrides()
            .expect("reasoning override should produce config");
        assert_eq!(overrides.get("model_reasoning_effort"), Some(&json!("max")));
    }

    #[test]
    fn reasoning_effort_maps_to_codex_protocol_values() {
        let xhigh = "xhigh"
            .parse::<ReasoningEffort>()
            .expect("xhigh is a valid reasoning effort");
        assert_eq!(xhigh.to_protocol(), ProtocolReasoningEffort::XHigh);

        let max = "max"
            .parse::<ReasoningEffort>()
            .expect("max is a valid reasoning effort");
        assert_eq!(max.to_protocol(), ProtocolReasoningEffort::Max);

        let ultra = "ultra"
            .parse::<ReasoningEffort>()
            .expect("ultra is a valid reasoning effort");
        assert_eq!(ultra.to_protocol(), ProtocolReasoningEffort::Ultra);

        let custom = "max-plus"
            .parse::<ReasoningEffort>()
            .expect("custom non-empty efforts are valid");
        assert_eq!(
            custom.to_protocol(),
            ProtocolReasoningEffort::Custom("max-plus".to_string())
        );
    }

    #[test]
    fn removed_on_failure_approval_policy_is_rejected() {
        assert!(serde_json::from_str::<AskForApproval>(r#""on-failure""#).is_err());
    }

    #[test]
    fn new_threads_use_supported_default_history_without_model_fallback() {
        let params = test_executor().build_thread_start_params(Path::new("/tmp/test-worktree"));

        assert!(params.history_mode.is_none());
        assert!(!params.allow_provider_model_fallback);
    }

    #[test]
    fn resumed_threads_keep_the_same_id_and_start_overrides() {
        let start = test_executor().build_thread_start_params(Path::new("/tmp/test-worktree"));
        let expected_model = start.model.clone();
        let expected_cwd = start.cwd.clone();
        let expected_service_tier = start.service_tier.clone();

        let resume = resume_params_from("thread-1".to_string(), start);

        assert_eq!(resume.thread_id, "thread-1");
        assert_eq!(resume.model, expected_model);
        assert_eq!(resume.cwd, expected_cwd);
        assert_eq!(resume.service_tier, expected_service_tier);
        assert!(resume.path.is_none());
        assert!(resume.history.is_none());
    }

    #[test]
    fn model_list_uses_codex_default_reasoning_effort() {
        let response = ModelListResponse {
            data: vec![Model {
                id: "gpt-test".to_string(),
                model: "gpt-test".to_string(),
                upgrade: None,
                upgrade_info: None,
                availability_nux: None,
                display_name: "GPT Test".to_string(),
                description: "test model".to_string(),
                hidden: false,
                supported_reasoning_efforts: vec![
                    ReasoningEffortOption {
                        reasoning_effort: ProtocolReasoningEffort::High,
                        description: "High".to_string(),
                    },
                    ReasoningEffortOption {
                        reasoning_effort: ProtocolReasoningEffort::Minimal,
                        description: "Minimal".to_string(),
                    },
                ],
                default_reasoning_effort: ProtocolReasoningEffort::Minimal,
                input_modalities: vec![],
                supports_personality: false,
                additional_speed_tiers: vec![],
                service_tiers: vec![codex_app_server_protocol::ModelServiceTier {
                    id: "priority".into(),
                    name: "Fast".into(),
                    description: "Priority processing".into(),
                }],
                default_service_tier: Some("priority".into()),
                is_default: false,
            }],
            next_cursor: None,
        };

        let models = model_list_response_to_model_infos(
            serde_json::from_value(serde_json::to_value(response).unwrap()).unwrap(),
        );

        assert_eq!(models.len(), 1);
        let capabilities = models[0].capabilities.as_ref().unwrap();
        assert_eq!(capabilities.service_tiers[0].id, "priority");
        assert_eq!(capabilities.service_tiers[0].name, "Fast");
        assert_eq!(
            capabilities.default_service_tier.as_deref(),
            Some("priority")
        );
        let reasoning_options = &models[0].reasoning_options;
        assert_eq!(
            reasoning_options
                .iter()
                .find(|option| option.is_default)
                .map(|option| option.id.as_str()),
            Some("minimal")
        );
    }

    #[test]
    fn model_list_preserves_gpt_5_6_max_and_ultra_reasoning_efforts() {
        let response = ModelListResponse {
            data: vec![Model {
                id: "gpt-5.6-sol".to_string(),
                model: "gpt-5.6-sol".to_string(),
                upgrade: None,
                upgrade_info: None,
                availability_nux: None,
                display_name: "GPT-5.6-Sol".to_string(),
                description: "Latest frontier agentic coding model.".to_string(),
                hidden: false,
                supported_reasoning_efforts: vec![
                    ReasoningEffortOption {
                        reasoning_effort: ProtocolReasoningEffort::Max,
                        description: "Maximum reasoning depth".to_string(),
                    },
                    ReasoningEffortOption {
                        reasoning_effort: ProtocolReasoningEffort::Ultra,
                        description: "Maximum reasoning with delegation".to_string(),
                    },
                ],
                default_reasoning_effort: ProtocolReasoningEffort::Max,
                input_modalities: vec![],
                supports_personality: false,
                additional_speed_tiers: vec![],
                service_tiers: vec![],
                default_service_tier: None,
                is_default: true,
            }],
            next_cursor: None,
        };

        let models = model_list_response_to_model_infos(
            serde_json::from_value(serde_json::to_value(response).unwrap()).unwrap(),
        );
        let ids = models[0]
            .reasoning_options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["max", "ultra"]);
        assert!(models[0].reasoning_options[0].is_default);
    }

    #[test]
    fn model_list_preserves_future_efforts_through_execution_without_inventing_defaults() {
        let response = serde_json::from_value(json!({"data":[{
            "id":"model-new", "model":"model-new", "displayName":"New Model",
            "supportedReasoningEfforts":[{"reasoningEffort":"future-depth"},{"reasoningEffort":"high"},{"reasoningEffort":""}],
            "defaultReasoningEffort":"future-depth", "inputModalities":["text","future-input"]
        }]})).unwrap();
        let models = model_list_response_to_model_infos(response);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].reasoning_options.len(), 2);
        let native_default = models[0]
            .reasoning_options
            .iter()
            .find(|option| option.is_default)
            .unwrap();
        assert_eq!(native_default.id, "future-depth");
        assert!(
            !models[0]
                .reasoning_options
                .iter()
                .find(|option| option.id == "high")
                .unwrap()
                .is_default
        );
        let mut executor = test_executor();
        executor
            .apply_overrides(&config_with_reasoning(&native_default.id))
            .unwrap();
        assert_eq!(
            executor.build_config_overrides().unwrap()["model_reasoning_effort"],
            "future-depth"
        );
        let protocol_effort = executor
            .model_reasoning_effort
            .as_ref()
            .unwrap()
            .to_protocol();
        assert_eq!(
            protocol_effort,
            ProtocolReasoningEffort::Custom("future-depth".into())
        );
        assert_eq!(
            serde_json::to_value(protocol_effort).unwrap(),
            "future-depth"
        );
        assert_eq!(
            models[0].capabilities.as_ref().unwrap().input_modalities,
            ["text", "future-input"]
        );
    }

    #[test]
    fn fallback_models_include_gpt_5_6_catalog() {
        let models = fallback_models();

        assert_eq!(
            models
                .iter()
                .take(3)
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]
        );
        let sol = &models[0];
        assert!(sol.reasoning_options.is_empty());
        assert!(sol.capabilities.is_none());
    }

    #[test]
    fn build_chat_input_places_selected_skills_before_text() {
        let skill_path = PathBuf::from("/tmp/skills/review/SKILL.md");
        let input = build_chat_input(
            "review this change".to_string(),
            vec![SelectedSkill {
                name: "code-review".to_string(),
                path: skill_path.clone(),
            }],
        );

        assert_eq!(input.len(), 2);
        match &input[0] {
            UserInput::Skill { name, path } => {
                assert_eq!(name, "code-review");
                assert_eq!(path, &skill_path);
            }
            other => panic!("expected skill input first, got {other:?}"),
        }
        match &input[1] {
            UserInput::Text {
                text,
                text_elements,
            } => {
                assert_eq!(text, "review this change");
                assert!(text_elements.is_empty());
            }
            other => panic!("expected text input second, got {other:?}"),
        }
    }
}
