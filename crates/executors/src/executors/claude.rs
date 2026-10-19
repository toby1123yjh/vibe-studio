// SDK submodules
pub mod client;
pub mod command_adapter;
pub mod protocol;
pub mod slash_commands;
pub mod types;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;
use workspace_utils::{
    approvals::{ApprovalStatus, QuestionStatus},
    command_ext::GroupSpawnNoWindowExt,
    diff::create_unified_diff,
    log_msg::LogMsg,
    msg_store::MsgStore,
    path::make_path_relative,
};

use self::{
    client::{AUTO_APPROVE_CALLBACK_ID, ClaudeAgentClient, STOP_GIT_CHECK_CALLBACK_ID},
    protocol::{ProtocolPeer, StartupFailureSignal},
    types::{ControlRequestType, ControlResponseType, PermissionMode},
};
use crate::{
    approvals::ExecutorApprovalService,
    command::{CmdOverrides, CommandBuildError, CommandBuilder, CommandParts},
    env::ExecutionEnv,
    executors::{
        AppendPrompt, AvailabilityInfo, BaseCodingAgent, ExecutorError, SpawnedChild,
        StandardCodingAgentExecutor, codex::client::LogWriter, utils::reorder_slash_commands,
    },
    logs::{
        ActionType, AnsweredQuestion, AskUserQuestionItem, AskUserQuestionOption, FileChange,
        NormalizedEntry, NormalizedEntryError, NormalizedEntryType, TodoItem, ToolStatus,
        plain_text_processor::PlainTextLogProcessor,
        utils::{
            EntryIndexProvider,
            patch::{self, ConversationPatch},
            shell_command_parsing::CommandCategory,
        },
    },
    model_selector::{ModelCatalogSource, NativeModelDefaults, PermissionPolicy},
    profile::{ExecutorConfig, ProviderRunOptions},
    stdout_dup::create_stdout_pipe_writer,
};

const SUPPRESSED_STDERR_PATTERNS: &[&str] = &["[WARN] Fast mode requires the native binary"];

fn base_command(claude_code_router: bool) -> String {
    if claude_code_router {
        "ccr code".to_string()
    } else {
        "claude".to_string()
    }
}

fn normalize_claude_stderr_logs(
    msg_store: Arc<MsgStore>,
    entry_index_provider: EntryIndexProvider,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut stderr = msg_store.stderr_chunked_stream();

        let mut processor = PlainTextLogProcessor::builder()
            .normalized_entry_producer(|content: String| NormalizedEntry {
                timestamp: None,
                entry_type: NormalizedEntryType::ErrorMessage {
                    error_type: NormalizedEntryError::Other,
                },
                content: strip_ansi_escapes::strip_str(&content),
                metadata: None,
            })
            .time_gap(Duration::from_secs(2))
            .index_provider(entry_index_provider)
            .transform_lines(Box::new(|lines: &mut Vec<String>| {
                lines.retain(|line| {
                    !SUPPRESSED_STDERR_PATTERNS
                        .iter()
                        .any(|pattern| line.contains(pattern))
                });
            }))
            .build();

        while let Some(Ok(chunk)) = stderr.next().await {
            for patch in processor.process(chunk) {
                msg_store.push_patch(patch);
            }
        }
    })
}

use derivative::Derivative;
use strum_macros::{AsRefStr, EnumString};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, JsonSchema, AsRefStr, EnumString)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ClaudeEffort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

#[derive(Derivative, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[derivative(Debug, PartialEq)]
pub struct ClaudeCode {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_code_router: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approvals: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ClaudeEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ultracode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dangerously_skip_permissions: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_api_key: Option<bool>,
    #[serde(flatten)]
    pub cmd: CmdOverrides,

    #[serde(skip)]
    #[ts(skip)]
    #[derivative(Debug = "ignore", PartialEq = "ignore")]
    approvals_service: Option<Arc<dyn ExecutorApprovalService>>,
}

impl ClaudeCode {
    /// V1 direct-runtime metadata and decoder entry point.
    pub const DIRECT_PROVIDER: super::provider_adapter::DirectProvider =
        super::provider_adapter::DirectProvider::ClaudeCode;

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
        if let Some(agent) = &executor_config.agent_id {
            self.agent = Some(agent.clone());
        }
        if let Some(reasoning_id) = &executor_config.reasoning_id {
            self.effort = reasoning_id
                .as_deref()
                .map(|value| {
                    value.parse().map_err(|_| {
                        ExecutorError::Io(std::io::Error::other(format!(
                            "Unsupported Claude effort: {value}"
                        )))
                    })
                })
                .transpose()?;
        }
        if let Some(ProviderRunOptions::ClaudeCode {
            fast_mode,
            permission_mode,
            ultracode,
        }) = &executor_config.provider_options
        {
            if let Some(mode) = permission_mode {
                let native = parse_permission_mode(mode)?;
                let compatible = match &executor_config.permission_policy {
                    Some(PermissionPolicy::Plan) => native == PermissionMode::Plan,
                    Some(PermissionPolicy::Supervised) => native == PermissionMode::Default,
                    Some(PermissionPolicy::Auto) => native == PermissionMode::BypassPermissions,
                    None => true,
                };
                if !compatible {
                    return Err(ExecutorError::Io(std::io::Error::other(
                        "Conflicting Claude permission policy and native permission mode",
                    )));
                }
                self.native_permission_mode = Some(mode.clone());
                self.plan = Some(native == PermissionMode::Plan);
                self.approvals = Some(native == PermissionMode::Default);
            }
            if fast_mode.is_some() {
                self.fast_mode = *fast_mode;
            }
            if ultracode.is_some() {
                self.ultracode = *ultracode;
            }
        }
        if let Some(permission_policy) = executor_config.permission_policy.clone() {
            match permission_policy {
                PermissionPolicy::Plan => {
                    self.plan = Some(true);
                    self.approvals = Some(false);
                }
                PermissionPolicy::Supervised => {
                    self.plan = Some(false);
                    self.approvals = Some(true);
                }
                PermissionPolicy::Auto => {
                    self.plan = Some(false);
                    self.approvals = Some(false);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn use_direct_approvals(&mut self, approvals: Arc<dyn ExecutorApprovalService>) {
        self.approvals_service = Some(approvals);
    }

    pub(crate) async fn launch_direct(
        &self,
        intent: super::provider_adapter::DirectIntent,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        reset_to_message_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let supports_effort = if self.effort.is_some() {
            self.supports_effort_flag_at(Some(current_dir)).await?
        } else {
            false
        };
        let command_parts = command_adapter::ClaudeCodeCommandAdapter::new(self).build_for_intent(
            supports_effort,
            intent,
            session_id,
            reset_to_message_id,
        )?;
        self.spawn_internal(current_dir, prompt, command_parts, env)
            .await
    }

    fn build_command_builder_with_effort_support(
        &self,
        supports_effort: bool,
    ) -> Result<CommandBuilder, CommandBuildError> {
        command_adapter::ClaudeCodeCommandAdapter::new(self).build_runtime(supports_effort)
    }

    #[cfg(test)]
    async fn build_command_builder(&self) -> Result<CommandBuilder, ExecutorError> {
        self.build_command_builder_at(None).await
    }

    async fn build_command_builder_at(
        &self,
        cwd: Option<&Path>,
    ) -> Result<CommandBuilder, ExecutorError> {
        let supports_effort = if self.effort.is_some() {
            self.supports_effort_flag_at(cwd).await?
        } else {
            false
        };

        Ok(self.build_command_builder_with_effort_support(supports_effort)?)
    }

    async fn supports_effort_flag_at(&self, cwd: Option<&Path>) -> Result<bool, ExecutorError> {
        let builder = command_adapter::ClaudeCodeCommandAdapter::new(self).build_effort_probe()?;

        let command_parts = builder.build_initial()?;
        let (program_path, args) = command_parts.into_resolved().await?;

        let mut command = Command::new(program_path);
        command
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NPM_CONFIG_LOGLEVEL", "error")
            .args(&args);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }

        ExecutionEnv::new(crate::env::RepoContext::default(), false, String::new())
            .with_profile(&self.cmd)
            .apply_to_command(&mut command);

        if self.disable_api_key.unwrap_or(false) {
            command.env_remove("ANTHROPIC_API_KEY");
        }

        let output = tokio::time::timeout(Duration::from_secs(5), command.output())
            .await
            .map_err(|_| {
                ExecutorError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Timed out checking Claude Code --effort support",
                ))
            })?
            .map_err(ExecutorError::Io)?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            return Err(ExecutorError::Io(std::io::Error::other(format!(
                "Claude capability probe failed ({})",
                output.status
            ))));
        }

        Ok(stdout.contains("--effort") || stderr.contains("--effort"))
    }

    pub fn permission_mode(&self) -> PermissionMode {
        if let Some(mode) = &self.native_permission_mode {
            if let Ok(mode) = parse_permission_mode(mode) {
                return mode;
            }
        }
        if self.plan.unwrap_or(false) {
            PermissionMode::Plan
        } else if self.approvals.unwrap_or(false) {
            PermissionMode::Default
        } else {
            PermissionMode::BypassPermissions
        }
    }

    pub fn get_hooks(&self, commit_reminder: bool) -> Option<serde_json::Value> {
        let mut hooks = serde_json::Map::new();

        if commit_reminder {
            hooks.insert(
                "Stop".to_string(),
                serde_json::json!([{
                    "hookCallbackIds": [STOP_GIT_CHECK_CALLBACK_ID]
                }]),
            );
        }

        // Add PreToolUse hooks based on plan/approvals settings
        if self.plan.unwrap_or(false) {
            hooks.insert(
                "PreToolUse".to_string(),
                serde_json::json!([
                    {
                        "matcher": "^(ExitPlanMode|AskUserQuestion)$",
                        "hookCallbackIds": ["tool_approval"],
                    },
                    {
                        "matcher": "^(?!(ExitPlanMode|AskUserQuestion)$).*",
                        "hookCallbackIds": [AUTO_APPROVE_CALLBACK_ID],
                    }
                ]),
            );
        } else if self.approvals.unwrap_or(false) {
            hooks.insert(
                "PreToolUse".to_string(),
                serde_json::json!([
                    {
                        "matcher": "^(?!(Glob|Grep|NotebookRead|Read|Task|TodoWrite)$).*",
                        "hookCallbackIds": ["tool_approval"],
                    }
                ]),
            );
        } else {
            hooks.insert(
                "PreToolUse".to_string(),
                serde_json::json!([
                    {
                        "matcher": "^AskUserQuestion$",
                        "hookCallbackIds": ["tool_approval"],
                    }
                ]),
            );
        }

        Some(serde_json::Value::Object(hooks))
    }

    async fn compute_cmd_key(&self, cwd: &Path) -> Result<String, ExecutorError> {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(serde_json::to_vec(self).unwrap_or_default());
        hash.update(cwd.to_string_lossy().as_bytes());
        hash.update(serde_json::to_vec(&self.native_model_defaults(cwd)).unwrap_or_default());
        for name in [
            "PATH",
            "CLAUDE_CONFIG_DIR",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_MODEL",
            "CLAUDE_CODE_EFFORT_LEVEL",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
        ] {
            hash.update(name.as_bytes());
            hash.update(std::env::var(name).unwrap_or_default().as_bytes());
        }
        let (executable, _) = command_adapter::ClaudeCodeCommandAdapter::new(self)
            .build_effort_probe()?
            .build_initial()?
            .into_resolved()
            .await?;
        hash.update(executable.to_string_lossy().as_bytes());
        if let Ok(meta) = std::fs::metadata(executable) {
            hash.update(format!("{}:{:?}", meta.len(), meta.modified().ok()).as_bytes());
        }
        let configured_root = self
            .cmd
            .env
            .as_ref()
            .and_then(|vars| vars.get("CLAUDE_CONFIG_DIR"))
            .cloned()
            .or_else(|| std::env::var("CLAUDE_CONFIG_DIR").ok())
            .map(PathBuf::from);
        let root = configured_root.or_else(|| dirs::home_dir().map(|home| home.join(".claude")));
        let mut paths = vec![
            cwd.join(".claude/settings.json"),
            cwd.join(".claude/settings.local.json"),
        ];
        if let Some(root) = root {
            paths.push(root.join("settings.json"));
            paths.push(root.join(".credentials.json"));
        }
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".claude.json"));
        }
        for path in paths {
            hash.update(path.to_string_lossy().as_bytes());
            if let Ok(bytes) = tokio::fs::read(path).await {
                hash.update(bytes);
            }
        }
        Ok(format!("{:x}", hash.finalize()))
    }

    fn native_model_defaults(&self, cwd: &Path) -> Option<NativeModelDefaults> {
        if self.claude_code_router == Some(true) && self.cmd.base_command_override.is_none() {
            return None;
        }
        crate::agent_settings::native_model_defaults_for(
            Self::DIRECT_PROVIDER,
            Some(cwd),
            &self.cmd,
        )
    }

    pub async fn invalidate_discovery_cache(
        &self,
        workdir: Option<&Path>,
        repo_path: Option<&Path>,
    ) -> Result<(), ExecutorError> {
        use crate::executor_discovery::ExecutorConfigCacheKey;
        let cwd = workdir
            .or(repo_path)
            .map(Path::to_path_buf)
            .unwrap_or(std::env::current_dir()?);
        let authority = self.compute_cmd_key(&cwd).await?;
        let cache = crate::executors::utils::executor_options_cache();
        // Provisional/global rows use this same cwd-inclusive authority hash;
        // invalidate only aliases of the selected authority, never all rows.
        for path in [workdir, repo_path, None] {
            let path = path.map(Path::to_path_buf);
            cache.invalidate(&ExecutorConfigCacheKey::new(
                path.as_ref(),
                authority.clone(),
                BaseCodingAgent::ClaudeCode,
            ));
        }
        Ok(())
    }
}

fn claude_model_options(_supports_effort: bool) -> Vec<crate::model_selector::ModelInfo> {
    use crate::model_selector::ModelInfo;

    [
        ("default", "Native default"),
        ("opus", "Opus"),
        ("sonnet", "Sonnet"),
        ("haiku", "Haiku"),
    ]
    .into_iter()
    .map(|(id, name)| ModelInfo {
        id: id.to_string(),
        name: name.to_string(),
        provider_id: None,
        reasoning_options: vec![],
        ..Default::default()
    })
    .collect()
}

fn apply_claude_effort_support(
    options: &mut crate::executor_discovery::ExecutorDiscoveredOptions,
    supports_effort: bool,
) {
    if !supports_effort {
        for model in &mut options.model_selector.models {
            model.reasoning_options.clear();
        }
    }
}

fn parse_permission_mode(mode: &str) -> Result<PermissionMode, ExecutorError> {
    serde_json::from_value(serde_json::Value::String(mode.into())).map_err(|_| {
        ExecutorError::Io(std::io::Error::other(format!(
            "Unsupported Claude permission mode: {mode}"
        )))
    })
}

/// Only exact native catalog entries define model capabilities. Alias names
/// alone cannot establish effort support (or an account's fast/auto access).
pub(crate) fn models_from_initialize(
    value: &serde_json::Value,
) -> Vec<crate::model_selector::ModelInfo> {
    use crate::model_selector::{CapabilitySupport, ModelCapabilities, ModelInfo, ReasoningOption};
    let support = |key: &str, model: &serde_json::Value| match model[key].as_bool() {
        Some(true) => CapabilitySupport::Supported,
        Some(false) => CapabilitySupport::Unsupported,
        None => CapabilitySupport::Unknown,
    };
    value["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model["value"].as_str()?;
            let reasoning = support("supportsEffort", model);
            let efforts = model["supportedEffortLevels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            Some(ModelInfo {
                id: id.into(),
                name: model["displayName"].as_str().unwrap_or(id).into(),
                reasoning_options: if reasoning == CapabilitySupport::Unsupported {
                    vec![]
                } else {
                    ReasoningOption::from_names(efforts)
                },
                capabilities: Some(ModelCapabilities {
                    reasoning,
                    fast_mode: support("supportsFastMode", model),
                    auto_mode: support("supportsAutoMode", model),
                    resolved_model: model["resolvedModel"].as_str().map(str::to_owned),
                    is_default: id == "default",
                    ..Default::default()
                }),
                ..Default::default()
            })
        })
        .collect()
}

fn validate_requested_capabilities(
    agent: &ClaudeCode,
    initialization: &serde_json::Value,
) -> Result<(), ExecutorError> {
    use crate::model_selector::CapabilitySupport;
    let models = models_from_initialize(initialization);
    let selected = Some(agent.model.as_deref().unwrap_or("default")).and_then(|id| {
        models.iter().find(|m| {
            m.id == id
                || m.capabilities
                    .as_ref()
                    .and_then(|c| c.resolved_model.as_deref())
                    == Some(id)
        })
    });
    if let Some(effort) = &agent.effort {
        let model = selected.ok_or_else(|| ExecutorError::Io(std::io::Error::other("Claude did not advertise capabilities for the requested model; cannot validate explicit effort")))?;
        if !model
            .reasoning_options
            .iter()
            .any(|option| option.id == effort.as_ref())
        {
            return Err(ExecutorError::Io(std::io::Error::other(format!(
                "Claude model {} does not advertise effort {}",
                model.id,
                effort.as_ref()
            ))));
        }
    }
    for (requested, supported, name) in [
        (
            agent.fast_mode == Some(true),
            selected
                .and_then(|m| m.capabilities.as_ref())
                .map(|c| c.fast_mode.clone()),
            "fast mode",
        ),
        (
            agent.native_permission_mode.as_deref() == Some("auto"),
            selected
                .and_then(|m| m.capabilities.as_ref())
                .map(|c| c.auto_mode.clone()),
            "auto mode",
        ),
    ] {
        if requested && supported != Some(CapabilitySupport::Supported) {
            return Err(ExecutorError::Io(std::io::Error::other(format!(
                "Claude has not confirmed {name} support for the selected model"
            ))));
        }
    }
    Ok(())
}

fn default_discovered_options(
    supports_effort: bool,
) -> crate::executor_discovery::ExecutorDiscoveredOptions {
    use crate::{
        executor_discovery::ExecutorDiscoveredOptions, model_selector::ModelSelectorConfig,
    };

    ExecutorDiscoveredOptions {
        model_selector: ModelSelectorConfig {
            providers: vec![],
            models: claude_model_options(supports_effort),
            default_model: Some("default".to_string()),
            catalog_source: Some(ModelCatalogSource::Builtin),
            agents: vec![],
            permissions: vec![
                PermissionPolicy::Auto,
                PermissionPolicy::Supervised,
                PermissionPolicy::Plan,
            ],
            ..Default::default()
        },
        slash_commands: ClaudeCode::hardcoded_slash_commands(),
        skills: vec![],
        skill_errors: vec![],
        loading_models: false,
        loading_agents: false,
        loading_slash_commands: false,
        loading_skills: false,
        error: None,
    }
}

#[async_trait]
impl StandardCodingAgentExecutor for ClaudeCode {
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
        let command_builder = self.build_command_builder_at(Some(current_dir)).await?;
        let command_parts = command_builder.build_initial()?;
        self.spawn_internal(current_dir, prompt, command_parts, env)
            .await
    }

    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        reset_to_message_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let command_builder = self.build_command_builder_at(Some(current_dir)).await?;

        let mut args = vec!["--resume".to_string(), session_id.to_string()];

        // --resume-session-at truncates Claude's conversation history to the specified
        // message and continues from there.
        if let Some(uuid) = reset_to_message_id {
            args.push("--resume-session-at".to_string());
            args.push(uuid.to_string());
        }

        let command_parts = command_builder.build_follow_up(&args)?;
        self.spawn_internal(current_dir, prompt, command_parts, env)
            .await
    }

    fn normalize_logs(
        &self,
        msg_store: Arc<MsgStore>,
        current_dir: &Path,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let entry_index_provider = EntryIndexProvider::start_from(&msg_store);

        // Process stdout logs (Claude's JSON output)
        let h1 = ClaudeLogProcessor::process_logs(
            msg_store.clone(),
            current_dir,
            entry_index_provider.clone(),
            HistoryStrategy::Default,
        );

        // Process stderr logs
        let h2 = normalize_claude_stderr_logs(msg_store, entry_index_provider);

        vec![h1, h2]
    }

    async fn discover_options(
        &self,
        workdir: Option<&Path>,
        repo_path: Option<&Path>,
    ) -> Result<futures::stream::BoxStream<'static, json_patch::Patch>, ExecutorError> {
        use crate::{
            executor_discovery::ExecutorConfigCacheKey, executors::utils::executor_options_cache,
        };

        let cache = executor_options_cache();
        let discovery_cwd = workdir
            .or(repo_path)
            .map(Path::to_path_buf)
            .unwrap_or(std::env::current_dir()?);
        let native_defaults = self.native_model_defaults(&discovery_cwd);
        // Return useful local config even when the executable/probe is missing
        // or broken. Failure must remain visible alongside the builtin hints.
        let (cmd_key, supports_effort) = match async {
            Ok::<_, ExecutorError>((
                self.compute_cmd_key(&discovery_cwd).await?,
                self.supports_effort_flag_at(Some(&discovery_cwd)).await?,
            ))
        }
        .await
        {
            Ok(result) => result,
            Err(error) => {
                let mut options = default_discovered_options(false);
                options.model_selector.native_defaults = native_defaults;
                options.error = Some(format!("Claude Code option discovery failed: {error}"));
                return Ok(Box::pin(futures::stream::once(async move {
                    patch::executor_discovered_options(options)
                })));
            }
        };
        let base_executor = BaseCodingAgent::ClaudeCode;

        let (target_path, initial_options) = if let Some(wd) = workdir {
            let wd_buf = wd.to_path_buf();
            let target_key =
                ExecutorConfigCacheKey::new(Some(&wd_buf), cmd_key.clone(), base_executor);
            if let Some(cached) = cache.get(&target_key) {
                let mut options = cached.as_ref().clone().with_loading(false);
                apply_claude_effort_support(&mut options, supports_effort);
                options.model_selector.native_defaults = native_defaults;
                return Ok(Box::pin(futures::stream::once(async move {
                    patch::executor_discovered_options(options)
                })));
            }
            let provisional = repo_path
                .and_then(|rp| {
                    let rp_buf = rp.to_path_buf();
                    let repo_key =
                        ExecutorConfigCacheKey::new(Some(&rp_buf), cmd_key.clone(), base_executor);
                    cache.get(&repo_key)
                })
                .or_else(|| {
                    let global_key =
                        ExecutorConfigCacheKey::new(None, cmd_key.clone(), base_executor);
                    cache.get(&global_key)
                });
            (
                Some(wd.to_path_buf()),
                provisional
                    .map(|p| {
                        let mut opts = p.as_ref().clone();
                        apply_claude_effort_support(&mut opts, supports_effort);
                        opts.loading_models = false;
                        opts.loading_agents = true;
                        opts.loading_slash_commands = true;
                        opts.loading_skills = true;
                        opts
                    })
                    .unwrap_or_else(|| {
                        let mut opts = default_discovered_options(supports_effort);
                        opts.loading_models = false;
                        opts.loading_agents = true;
                        opts.loading_slash_commands = true;
                        opts.loading_skills = true;
                        opts
                    }),
            )
        } else if let Some(rp) = repo_path {
            let rp_buf = rp.to_path_buf();
            let target_key =
                ExecutorConfigCacheKey::new(Some(&rp_buf), cmd_key.clone(), base_executor);
            if let Some(cached) = cache.get(&target_key) {
                let mut options = cached.as_ref().clone().with_loading(false);
                apply_claude_effort_support(&mut options, supports_effort);
                options.model_selector.native_defaults = native_defaults;
                return Ok(Box::pin(futures::stream::once(async move {
                    patch::executor_discovered_options(options)
                })));
            }
            let global_key = ExecutorConfigCacheKey::new(None, cmd_key.clone(), base_executor);
            let provisional = cache.get(&global_key);
            (
                Some(rp.to_path_buf()),
                provisional
                    .map(|p| {
                        let mut opts = p.as_ref().clone();
                        apply_claude_effort_support(&mut opts, supports_effort);
                        opts.loading_models = false;
                        opts.loading_agents = true;
                        opts.loading_slash_commands = true;
                        opts.loading_skills = true;
                        opts
                    })
                    .unwrap_or_else(|| {
                        let mut opts = default_discovered_options(supports_effort);
                        opts.loading_models = false;
                        opts.loading_agents = true;
                        opts.loading_slash_commands = true;
                        opts.loading_skills = true;
                        opts
                    }),
            )
        } else {
            let global_key = ExecutorConfigCacheKey::new(None, cmd_key.clone(), base_executor);
            if let Some(cached) = cache.get(&global_key) {
                let mut options = cached.as_ref().clone().with_loading(false);
                apply_claude_effort_support(&mut options, supports_effort);
                options.model_selector.native_defaults = native_defaults;
                return Ok(Box::pin(futures::stream::once(async move {
                    patch::executor_discovered_options(options)
                })));
            }
            let mut opts = default_discovered_options(supports_effort);
            opts.loading_models = false;
            opts.loading_agents = true;
            opts.loading_slash_commands = true;
            opts.loading_skills = true;
            (None, opts)
        };

        let mut initial_options = initial_options;
        initial_options.model_selector.native_defaults = native_defaults.clone();
        initial_options.loading_models = true;
        let initial_patch = patch::executor_discovered_options(initial_options);

        let this = self.clone();
        let cmd_key_for_discovery = cmd_key.clone();

        let discovery_stream = async_stream::stream! {
            let discovery_path = discovery_cwd;
            let mut final_options = default_discovered_options(supports_effort);
            final_options.model_selector.native_defaults = native_defaults;

            match this.discover_agents_and_slash_commands_initial(&discovery_path).await {
                Ok((mut agent_options, slash_commands_initial, plugins, skills, models)) => {
                    if !models.is_empty() {
                        final_options.model_selector.models = models;
                        final_options.model_selector.catalog_source = Some(ModelCatalogSource::Native);
                    } else {
                        final_options.error = Some("Claude returned no models; capabilities are unconfirmed".into());
                    }
                    yield patch::executor_discovered_options(final_options.clone());
                    let default_agents = [
                        "Bash",
                        "general-purpose",
                        "statusline-setup",
                        "Explore",
                        "Plan",
                    ];
                    agent_options.retain(|a| !default_agents.contains(&a.id.as_str()));
                    final_options.model_selector.agents = agent_options.clone();
                    yield patch::update_agents(agent_options);
                    yield patch::agents_loaded();

                    // The discovered list already contains builtin + custom
                    // commands reported by the actual binary. Fall back to the
                    // hardcoded list only when discovery yields nothing (e.g. an
                    // unexpected or very old binary).
                    let slash_commands = if slash_commands_initial.is_empty() {
                        Self::hardcoded_slash_commands()
                    } else {
                        slash_commands_initial
                    };
                    let slash_commands = reorder_slash_commands(slash_commands);
                    final_options.slash_commands = slash_commands.clone();
                    yield patch::update_slash_commands(slash_commands);

                    let slash_commands_with_descriptions = Self::fill_slash_command_descriptions(
                        &discovery_path,
                        &plugins,
                        &final_options.slash_commands,
                    ).await;
                    final_options.slash_commands = slash_commands_with_descriptions;
                    yield patch::update_slash_commands(final_options.slash_commands.clone());
                    yield patch::slash_commands_loaded();

                    final_options.skills = skills.clone();
                    yield patch::update_skills(skills);
                    yield patch::skills_loaded();

                    let cache = executor_options_cache();
                    if let Some(path) = &target_path {
                        let target_cache_key = ExecutorConfigCacheKey::new(
                            Some(path),
                            cmd_key_for_discovery.clone(),
                            BaseCodingAgent::ClaudeCode,
                        );
                        cache.put(target_cache_key, final_options.clone());
                    }
                    let global_cache_key = ExecutorConfigCacheKey::new(
                        None,
                        cmd_key_for_discovery,
                        BaseCodingAgent::ClaudeCode,
                    );
                    cache.put(global_cache_key, final_options);
                }
                Err(e) => {
                    tracing::warn!("Failed to discover Claude Code options: {}", e);
                    yield patch::discovery_error(e.to_string());
                }
            }
        };

        Ok(Box::pin(
            futures::stream::once(async move { initial_patch }).chain(discovery_stream),
        ))
    }

    fn get_preset_options(&self) -> ExecutorConfig {
        use crate::model_selector::*;

        let permission_policy = if self.plan.unwrap_or(false) {
            PermissionPolicy::Plan
        } else if self.dangerously_skip_permissions.unwrap_or(false) {
            PermissionPolicy::Auto
        } else if self.approvals.unwrap_or(false) {
            PermissionPolicy::Supervised
        } else {
            PermissionPolicy::Auto
        };

        ExecutorConfig {
            executor: BaseCodingAgent::ClaudeCode,
            variant: None,
            model_id: self.model.clone().map(Some),
            agent_id: None,
            reasoning_id: self.effort.as_ref().map(|e| Some(e.as_ref().to_owned())),
            permission_policy: Some(permission_policy),
            provider_options: Some(ProviderRunOptions::ClaudeCode {
                fast_mode: self.fast_mode,
                permission_mode: self.native_permission_mode.clone(),
                ultracode: self.ultracode,
            }),
        }
    }

    // MCP configuration methods

    fn default_mcp_config_path(&self) -> Option<std::path::PathBuf> {
        dirs::home_dir().map(|home| home.join(".claude.json"))
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        let auth_file_path = dirs::home_dir().map(|home| home.join(".claude.json"));

        if let Some(path) = auth_file_path
            && let Some(timestamp) = std::fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
        {
            return AvailabilityInfo::LoginDetected {
                last_auth_timestamp: timestamp,
            };
        }
        if crate::command::is_command_installed(
            &base_command(self.claude_code_router.unwrap_or(false)),
            &self.cmd,
        ) {
            AvailabilityInfo::InstallationFound
        } else {
            AvailabilityInfo::NotFound
        }
    }
}

impl ClaudeCode {
    async fn spawn_internal(
        &self,
        current_dir: &Path,
        prompt: &str,
        command_parts: CommandParts,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        if let Some(mode) = &self.native_permission_mode {
            parse_permission_mode(mode)?;
            if self.dangerously_skip_permissions == Some(true) && mode != "bypassPermissions" {
                return Err(ExecutorError::InvalidConfiguration(
                    "Claude native permission mode conflicts with dangerously_skip_permissions"
                        .into(),
                ));
            }
        }
        let workflow_mcp = crate::workflow_mcp::ScopedWorkflowMcp::from_execution_env(env)?;
        let command_parts =
            command_adapter::append_workflow_mcp(command_parts, workflow_mcp.as_ref());
        let mut launch_env = env.clone().with_profile(&self.cmd);
        let readiness = crate::workflow_mcp::WorkflowMcpReadiness::start(&mut launch_env).await?;
        let workflow_scoped = readiness.is_some();
        let startup_deadline = readiness
            .as_ref()
            .map(crate::workflow_mcp::WorkflowMcpReadiness::startup_deadline);
        let prompt_gate = crate::workflow_mcp::WorkflowPromptGate::new(workflow_scoped);
        let (program_path, args) = command_parts.into_resolved().await?;
        let combined_prompt = self.append_prompt.combine_prompt(prompt);

        let mut command = Command::new(program_path);
        command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(current_dir)
            .env("NPM_CONFIG_LOGLEVEL", "error")
            .args(&args);

        launch_env.apply_to_command(&mut command);

        // Remove ANTHROPIC_API_KEY if disable_api_key is enabled
        if self.disable_api_key.unwrap_or(false) {
            command.env_remove("ANTHROPIC_API_KEY");
            tracing::info!("ANTHROPIC_API_KEY removed from environment");
        }

        let mut child = command.group_spawn_no_window()?;
        let child_stdout = child.inner().stdout.take().ok_or_else(|| {
            ExecutorError::Io(std::io::Error::other("Claude Code missing stdout"))
        })?;
        let child_stdin =
            child.inner().stdin.take().ok_or_else(|| {
                ExecutorError::Io(std::io::Error::other("Claude Code missing stdin"))
            })?;

        let new_stdout = create_stdout_pipe_writer(&mut child)?;
        let permission_mode = self.permission_mode();
        let hooks = self.get_hooks(env.commit_reminder);

        // Create cancellation token for graceful shutdown
        let cancel = CancellationToken::new();

        let log_writer = LogWriter::new(new_stdout);
        let client = ClaudeAgentClient::new(
            log_writer.clone(),
            self.approvals_service.clone(),
            env.repo_context.clone(),
            env.commit_reminder_prompt.clone(),
            cancel.clone(),
        );
        let (signal, receiver) = StartupFailureSignal::channel();
        let (startup_failure, exit_signal) = (Some(signal), Some(receiver));
        let protocol_peer = ProtocolPeer::spawn(
            child_stdin,
            child_stdout,
            client.clone(),
            cancel.clone(),
            startup_failure,
        );
        let control = prompt_gate.wrap(Arc::new(protocol_peer.clone()));
        let cancel_for_startup = cancel.clone();

        // Spawn task to initialize the SDK client and send the first message.
        let prompt_clone = combined_prompt.clone();
        let requested = self.clone();
        tokio::spawn(async move {
            let startup_result = crate::workflow_mcp::WorkflowMcpReadiness::bound_startup(
                startup_deadline,
                &cancel_for_startup,
                async {
                    let initialization = protocol_peer.initialize(hooks).await?;
                    validate_requested_capabilities(&requested, &initialization)?;
                    let mut settings = serde_json::Map::new();
                    if let Some(fast) = requested.fast_mode {
                        settings.insert("fastMode".into(), fast.into());
                    }
                    if let Some(ultracode) = requested.ultracode {
                        settings.insert("ultracode".into(), ultracode.into());
                    }
                    if !settings.is_empty() {
                        protocol_peer.apply_flag_settings(settings.into()).await?;
                    }
                    crate::workflow_mcp::WorkflowMcpReadiness::wait_optional(
                        readiness,
                        &cancel_for_startup,
                    )
                    .await?;
                    protocol_peer.set_permission_mode(permission_mode).await?;
                    protocol_peer.send_user_message(prompt_clone).await?;
                    prompt_gate.open();
                    Ok::<(), ExecutorError>(())
                },
            )
            .await
            .and_then(|result| result);

            if let Err(error) = startup_result {
                protocol_peer.fail_startup().await;
                cancel_for_startup.cancel();
                tracing::error!("Claude startup failed: {error}");
                let _ = log_writer.log_raw(&format!("Error: {error}")).await;
            }
        });

        Ok(SpawnedChild {
            child,
            exit_signal,
            cancel: Some(cancel),
            control: Some(control),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryStrategy {
    // Claude-code format
    Default,
}

/// Default context window for models (used until we get actual value from result)
const DEFAULT_CLAUDE_CONTEXT_WINDOW: u32 = 200_000;

/// Handles log processing and interpretation for Claude executor
pub struct ClaudeLogProcessor {
    model_name: Option<String>,
    // Map tool_use_id -> structured info for follow-up ToolResult replacement
    tool_map: HashMap<String, ClaudeToolCallInfo>,
    streaming_messages: HashMap<String, StreamingMessageState>,
    streaming_message_id: Option<String>,
    last_assistant_message: Option<String>,
    // Main model name (excluding subagents). Only used internally for context window tracking.
    main_model_name: Option<String>,
    main_model_context_window: u32,
    context_tokens_used: u32,
    retry_status_fingerprint: Option<String>,
}

impl ClaudeLogProcessor {
    #[cfg(test)]
    fn new() -> Self {
        Self::new_with_strategy(HistoryStrategy::Default)
    }

    fn new_with_strategy(_strategy: HistoryStrategy) -> Self {
        Self {
            model_name: None,
            main_model_name: None,
            tool_map: HashMap::new(),
            streaming_messages: HashMap::new(),
            streaming_message_id: None,
            last_assistant_message: None,
            main_model_context_window: DEFAULT_CLAUDE_CONTEXT_WINDOW,
            context_tokens_used: 0,
            retry_status_fingerprint: None,
        }
    }

    /// Process raw logs and convert them to normalized entries with patches
    pub fn process_logs(
        msg_store: Arc<MsgStore>,
        current_dir: &Path,
        entry_index_provider: EntryIndexProvider,
        strategy: HistoryStrategy,
    ) -> tokio::task::JoinHandle<()> {
        let current_dir_clone = current_dir.to_owned();
        tokio::spawn(async move {
            let mut stream = msg_store.history_plus_stream();
            let mut buffer = String::new();
            let worktree_path = current_dir_clone.to_string_lossy().to_string();
            let mut session_id_extracted = false;
            let mut processor = Self::new_with_strategy(strategy);
            // Track pending assistant UUID - only committed when we see a Result message
            let mut pending_assistant_uuid: Option<String> = None;

            while let Some(Ok(msg)) = stream.next().await {
                let chunk = match msg {
                    LogMsg::Stdout(x) => x,
                    LogMsg::JsonPatch(_)
                    | LogMsg::SessionId(_)
                    | LogMsg::MessageId(_)
                    | LogMsg::Stderr(_)
                    | LogMsg::Ready => continue,
                    LogMsg::Finished => break,
                };

                buffer.push_str(&chunk);

                // Process complete JSON lines
                for line in buffer
                    .split_inclusive('\n')
                    .filter(|l| l.ends_with('\n'))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
                {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    // Filter out claude-code-router service messages
                    if trimmed.starts_with("Service not running, starting service")
                        || trimmed
                            .contains("claude code router service has been successfully stopped")
                    {
                        continue;
                    }

                    match serde_json::from_str::<ClaudeJson>(trimmed) {
                        Ok(claude_json) => {
                            if !session_id_extracted
                                && let Some(session_id) = Self::extract_session_id(&claude_json)
                            {
                                msg_store.push_session_id(session_id);
                                session_id_extracted = true;
                            }

                            // Track message UUIDs for --resume-session-at:
                            // - User messages: always valid, push immediately and clear pending
                            // - Assistant messages: may have incomplete tool calls, store as pending
                            // - Result messages: confirms assistant turn is complete, commit pending
                            match &claude_json {
                                ClaudeJson::User { uuid, .. } => {
                                    pending_assistant_uuid = None;
                                    if let Some(uuid) = uuid {
                                        msg_store.push_message_id(uuid.clone());
                                    }
                                }
                                ClaudeJson::Assistant { uuid, .. } => {
                                    pending_assistant_uuid = uuid.clone();
                                }
                                ClaudeJson::Result { .. } => {
                                    if let Some(uuid) = pending_assistant_uuid.take() {
                                        msg_store.push_message_id(uuid);
                                    }
                                }
                                _ => {}
                            }

                            let patches = processor.normalize_entries(
                                &claude_json,
                                &worktree_path,
                                &entry_index_provider,
                            );
                            for patch in patches {
                                msg_store.push_patch(patch);
                            }
                        }
                        Err(_) => {
                            // Handle non-JSON output as raw system message
                            if !trimmed.is_empty() {
                                let entry = NormalizedEntry {
                                    timestamp: None,
                                    entry_type: NormalizedEntryType::SystemMessage,
                                    content: trimmed.to_string(),
                                    metadata: None,
                                };

                                let patch_id = entry_index_provider.next();
                                let patch =
                                    ConversationPatch::add_normalized_entry(patch_id, entry);
                                msg_store.push_patch(patch);
                            }
                        }
                    }
                }

                // Keep the partial line in the buffer
                buffer = buffer.rsplit('\n').next().unwrap_or("").to_owned();
            }

            // Handle any remaining content in buffer
            if !buffer.trim().is_empty() {
                let entry = NormalizedEntry {
                    timestamp: None,
                    entry_type: NormalizedEntryType::SystemMessage,
                    content: buffer.trim().to_string(),
                    metadata: None,
                };

                let patch_id = entry_index_provider.next();
                let patch = ConversationPatch::add_normalized_entry(patch_id, entry);
                msg_store.push_patch(patch);
            }
        })
    }

    /// Extract session ID from Claude JSON
    fn extract_session_id(claude_json: &ClaudeJson) -> Option<String> {
        match claude_json {
            ClaudeJson::System { .. } => None, // session might not have been initialized yet
            ClaudeJson::Assistant { session_id, .. } => session_id.clone(),
            ClaudeJson::User { session_id, .. } => session_id.clone(),
            ClaudeJson::ToolUse { session_id, .. } => session_id.clone(),
            ClaudeJson::ToolResult { session_id, .. } => session_id.clone(),
            ClaudeJson::Result { session_id, .. } => session_id.clone(),
            ClaudeJson::StreamEvent { .. } => None, // session might not have been initialized yet
            ClaudeJson::ApprovalRequested { .. } => None,
            ClaudeJson::ApprovalResponse { .. } => None,
            ClaudeJson::QuestionResponse { .. } => None,
            ClaudeJson::ControlRequest { .. } => None,
            ClaudeJson::ControlResponse { .. } => None,
            ClaudeJson::ControlCancelRequest { .. } => None,
            ClaudeJson::RateLimitEvent { session_id, .. } => session_id.clone(),
            ClaudeJson::Unknown { .. } => None,
        }
    }

    /// Generate warning entry if API key source is ANTHROPIC_API_KEY
    fn warn_if_unmanaged_key(src: &Option<String>) -> Option<NormalizedEntry> {
        match src.as_deref() {
            Some("ANTHROPIC_API_KEY") => {
                tracing::warn!(
                    "ANTHROPIC_API_KEY env variable detected, your Anthropic subscription is not being used"
                );
                Some(NormalizedEntry {
                    timestamp: None,
                    entry_type: NormalizedEntryType::ErrorMessage { error_type: NormalizedEntryError::Other,
                    },
                    content: "Claude Code + ANTHROPIC_API_KEY detected. Usage will be billed via Anthropic pay-as-you-go instead of your Claude subscription. If this is unintended, please select the `disable_api_key` checkbox in the conding-agent-configurations settings page.".to_string(),
                    metadata: None,
                })
            }
            _ => None,
        }
    }

    /// Normalize Claude tool_result content to either Markdown string or parsed JSON.
    /// - If content is a string that parses as JSON, return Json with parsed value.
    /// - If content is a string (non-JSON), return Markdown with the raw string.
    /// - If content is an array of { text: string }, join texts as Markdown.
    /// - Otherwise return Json with the original value.
    fn normalize_claude_tool_result_value(
        content: &serde_json::Value,
    ) -> (crate::logs::ToolResultValueType, serde_json::Value) {
        if let Some(s) = content.as_str() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                return (crate::logs::ToolResultValueType::Json, parsed);
            }
            return (
                crate::logs::ToolResultValueType::Markdown,
                serde_json::Value::String(s.to_string()),
            );
        }

        if let Ok(items) = serde_json::from_value::<Vec<ClaudeToolResultTextItem>>(content.clone())
            && !items.is_empty()
        {
            let joined = items
                .into_iter()
                .map(|i| i.text)
                .collect::<Vec<_>>()
                .join("\n\n");
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&joined) {
                return (crate::logs::ToolResultValueType::Json, parsed);
            }
            return (
                crate::logs::ToolResultValueType::Markdown,
                serde_json::Value::String(joined),
            );
        }

        (crate::logs::ToolResultValueType::Json, content.clone())
    }

    fn build_tool_use_entry(
        tool_data: &ClaudeToolData,
        worktree_path: &str,
        status: ToolStatus,
    ) -> (NormalizedEntry, String, String) {
        let tool_name = tool_data.get_name().to_string();
        let action_type = Self::extract_action_type(tool_data, worktree_path);
        let content = Self::generate_concise_content(tool_data, &action_type, worktree_path);
        let entry = Self::tool_use_entry(tool_name.clone(), action_type, status, content.clone());
        (entry, tool_name, content)
    }

    fn tool_use_entry(
        tool_name: String,
        action_type: ActionType,
        status: ToolStatus,
        content: String,
    ) -> NormalizedEntry {
        NormalizedEntry {
            timestamp: None,
            entry_type: NormalizedEntryType::ToolUse {
                tool_name,
                action_type,
                status,
            },
            content,
            metadata: None,
        }
    }

    fn replace_tool_entry_status(
        &mut self,
        tool_call_id: &str,
        status: ToolStatus,
        worktree_path: &str,
        patches: &mut Vec<json_patch::Patch>,
    ) {
        if let Some(info) = self.tool_map.get(tool_call_id).cloned() {
            let action_type = Self::extract_action_type(&info.tool_data, worktree_path);
            let entry = Self::tool_use_entry(
                info.tool_name.clone(),
                action_type,
                status,
                info.content.clone(),
            );
            patches.push(ConversationPatch::replace(info.entry_index, entry));
        }
    }

    /// Convert Claude content item to normalized entry
    fn content_item_to_normalized_entry(
        content_item: &ClaudeContentItem,
        role: &str,
        worktree_path: &str,
        last_assistant_message: &mut Option<String>,
    ) -> Option<NormalizedEntry> {
        match content_item {
            ClaudeContentItem::Text { text } => {
                let entry_type = match role {
                    "assistant" => NormalizedEntryType::AssistantMessage,
                    _ => return None,
                };
                *last_assistant_message = Some(text.clone());
                Some(NormalizedEntry {
                    timestamp: None,
                    entry_type,
                    content: text.clone(),
                    metadata: Some(
                        serde_json::to_value(content_item).unwrap_or(serde_json::Value::Null),
                    ),
                })
            }
            ClaudeContentItem::Thinking { thinking } => Some(NormalizedEntry {
                timestamp: None,
                entry_type: NormalizedEntryType::Thinking,
                content: thinking.clone(),
                metadata: Some(
                    serde_json::to_value(content_item).unwrap_or(serde_json::Value::Null),
                ),
            }),
            ClaudeContentItem::ToolUse { tool_data, id: _ } => {
                let (entry, _, _) =
                    Self::build_tool_use_entry(tool_data, worktree_path, ToolStatus::Created);
                Some(entry)
            }
            ClaudeContentItem::ToolResult { .. } => {
                // TODO: Add proper ToolResult support to NormalizedEntry when the type system supports it
                None
            }
        }
    }

    /// Extract action type from structured tool data
    fn extract_action_type(tool_data: &ClaudeToolData, worktree_path: &str) -> ActionType {
        match tool_data {
            ClaudeToolData::Read { file_path } => ActionType::FileRead {
                path: make_path_relative(file_path, worktree_path),
            },
            ClaudeToolData::Edit {
                file_path,
                old_string,
                new_string,
            } => {
                let changes = if old_string.is_some() || new_string.is_some() {
                    vec![FileChange::Edit {
                        unified_diff: create_unified_diff(
                            file_path,
                            &old_string.clone().unwrap_or_default(),
                            &new_string.clone().unwrap_or_default(),
                        ),
                        has_line_numbers: false,
                    }]
                } else {
                    vec![]
                };
                ActionType::FileEdit {
                    path: make_path_relative(file_path, worktree_path),
                    changes,
                }
            }
            ClaudeToolData::MultiEdit { file_path, edits } => {
                let changes: Vec<FileChange> = edits
                    .iter()
                    .filter(|edit| edit.old_string.is_some() || edit.new_string.is_some())
                    .map(|edit| FileChange::Edit {
                        unified_diff: create_unified_diff(
                            file_path,
                            &edit.old_string.clone().unwrap_or_default(),
                            &edit.new_string.clone().unwrap_or_default(),
                        ),
                        has_line_numbers: false,
                    })
                    .collect();
                ActionType::FileEdit {
                    path: make_path_relative(file_path, worktree_path),
                    changes,
                }
            }
            ClaudeToolData::Write { file_path, content } => {
                let diffs = vec![FileChange::Write {
                    content: content.clone(),
                }];
                ActionType::FileEdit {
                    path: make_path_relative(file_path, worktree_path),
                    changes: diffs,
                }
            }
            ClaudeToolData::Bash { command, .. } => ActionType::CommandRun {
                command: command.clone(),
                result: None,
                category: CommandCategory::from_command(command),
            },
            ClaudeToolData::Grep { pattern, .. } => ActionType::Search {
                query: pattern.clone(),
            },
            ClaudeToolData::WebFetch { url, .. } => ActionType::WebFetch { url: url.clone() },
            ClaudeToolData::WebSearch { query, .. } => ActionType::WebFetch { url: query.clone() },
            ClaudeToolData::Task {
                description,
                prompt,
                subagent_type,
            } => {
                let task_description = if let Some(desc) = description {
                    desc.clone()
                } else {
                    prompt.clone().unwrap_or_default()
                };
                ActionType::TaskCreate {
                    description: task_description,
                    subagent_type: subagent_type.clone(),
                    result: None,
                }
            }
            ClaudeToolData::ExitPlanMode { plan } => {
                ActionType::PlanPresentation { plan: plan.clone() }
            }
            ClaudeToolData::NotebookEdit { .. } => ActionType::Tool {
                tool_name: "NotebookEdit".to_string(),
                arguments: Some(serde_json::to_value(tool_data).unwrap_or(serde_json::Value::Null)),
                result: None,
            },
            ClaudeToolData::TodoWrite { todos } => ActionType::TodoManagement {
                todos: todos
                    .iter()
                    .map(|t| TodoItem {
                        content: t.content.clone(),
                        status: t.status.clone(),
                        priority: t.priority.clone(),
                    })
                    .collect(),
                operation: "write".to_string(),
            },
            ClaudeToolData::TodoRead { .. } => ActionType::TodoManagement {
                todos: vec![],
                operation: "read".to_string(),
            },
            ClaudeToolData::Glob { pattern, .. } => ActionType::Search {
                query: pattern.clone(),
            },
            ClaudeToolData::LS { .. } => ActionType::Other {
                description: "List directory".to_string(),
            },
            ClaudeToolData::AskUserQuestion { questions } => ActionType::AskUserQuestion {
                questions: questions
                    .iter()
                    .map(|q| AskUserQuestionItem {
                        question: q.question.clone(),
                        header: q.header.clone(),
                        options: q
                            .options
                            .iter()
                            .map(|o| AskUserQuestionOption {
                                label: o.label.clone(),
                                description: o.description.clone(),
                            })
                            .collect(),
                        multi_select: q.multi_select,
                    })
                    .collect(),
            },
            ClaudeToolData::Unknown { .. } => {
                // Surface MCP tools as generic Tool with args
                let name = tool_data.get_name();
                if name.starts_with("mcp__") {
                    let parts: Vec<&str> = name.split("__").collect();
                    let label = if parts.len() >= 3 {
                        format!("mcp:{}:{}", parts[1], parts[2])
                    } else {
                        name.to_string()
                    };
                    // Extract `input` if present by serializing then deserializing to a tiny struct
                    let args = serde_json::to_value(tool_data)
                        .ok()
                        .and_then(|v| serde_json::from_value::<ClaudeToolWithInput>(v).ok())
                        .map(|w| w.input)
                        .unwrap_or(serde_json::Value::Null);
                    ActionType::Tool {
                        tool_name: label,
                        arguments: Some(args),
                        result: None,
                    }
                } else {
                    ActionType::Other {
                        description: format!("Tool: {}", tool_data.get_name()),
                    }
                }
            }
        }
    }

    /// Convert Claude JSON to normalized patches
    fn normalize_entries(
        &mut self,
        claude_json: &ClaudeJson,
        worktree_path: &str,
        entry_index_provider: &EntryIndexProvider,
    ) -> Vec<json_patch::Patch> {
        let mut patches = Vec::new();
        match claude_json {
            ClaudeJson::System {
                subtype,
                api_key_source,
                model,
                status,
                tool_use_id,
                description,
                task_type,
                prompt,
                summary,
                extra,
                ..
            } => {
                // emit billing warning if required
                if let Some(warning) = Self::warn_if_unmanaged_key(api_key_source) {
                    let idx = entry_index_provider.next();
                    patches.push(ConversationPatch::add_normalized_entry(idx, warning));
                }

                // keep the existing behaviour for the normal system message
                match subtype.as_deref() {
                    Some("init") => {
                        if self.main_model_name.is_none() {
                            // this name matches the model names in the usage report in the result message
                            if let Some(model) = model {
                                self.main_model_name = Some(model.clone());
                                if model.contains("[1m]") {
                                    self.main_model_context_window = 1_000_000;
                                }
                            }
                        }
                        // Skip system init messages because it doesn't contain the actual model that will be used in assistant messages in case of claude-code-router.
                        // We'll send system initialized message with first assistant message that has a model field.
                    }
                    Some("status") => {
                        if let Some(status) = status {
                            if status == "requesting" {
                                patches.push(add_loading_entry(entry_index_provider));
                            } else {
                                patches
                                    .push(add_system_message(status.clone(), entry_index_provider));
                            }
                        }
                    }
                    Some("compact_boundary") => {}
                    Some("api_retry") => {
                        let content =
                            Self::format_api_retry_message(status, description, summary, extra);
                        self.push_deduped_retry_notice(&mut patches, entry_index_provider, content);
                    }
                    Some("hook_started" | "hook_response" | "thinking_tokens") => {}
                    Some("task_started") => {
                        if let Some(tool_use_id) = tool_use_id
                            && !self.tool_map.contains_key(tool_use_id)
                        {
                            let desc = description.clone().unwrap_or_else(|| "Task".to_string());
                            let subagent_type = task_type.clone();
                            let entry = Self::tool_use_entry(
                                "Task".to_string(),
                                ActionType::TaskCreate {
                                    description: desc.clone(),
                                    subagent_type: subagent_type.clone(),
                                    result: None,
                                },
                                ToolStatus::Created,
                                desc.clone(),
                            );
                            let idx = entry_index_provider.next();
                            patches.push(ConversationPatch::add_normalized_entry(idx, entry));
                            self.tool_map.insert(
                                tool_use_id.clone(),
                                ClaudeToolCallInfo {
                                    entry_index: idx,
                                    tool_name: "Task".to_string(),
                                    tool_data: ClaudeToolData::Task {
                                        subagent_type,
                                        description: description.clone(),
                                        prompt: prompt.clone(),
                                    },
                                    content: desc,
                                },
                            );
                        }
                    }
                    Some("task_progress") => {
                        if let (Some(tool_use_id), Some(desc)) = (tool_use_id, description)
                            && let Some(info) = self.tool_map.get(tool_use_id).cloned()
                        {
                            let subagent_type =
                                if let ClaudeToolData::Task { subagent_type, .. } = &info.tool_data
                                {
                                    subagent_type.clone()
                                } else {
                                    None
                                };
                            let entry = Self::tool_use_entry(
                                info.tool_name.clone(),
                                ActionType::TaskCreate {
                                    description: info.content.clone(),
                                    subagent_type,
                                    result: None,
                                },
                                ToolStatus::Created,
                                desc.clone(),
                            );
                            patches.push(ConversationPatch::replace(info.entry_index, entry));
                        }
                    }
                    Some("task_notification") => {
                        if let Some(tool_use_id) = tool_use_id
                            && let Some(info) = self.tool_map.get(tool_use_id).cloned()
                        {
                            let task_status = match status.as_deref() {
                                Some("failed") | Some("error") => ToolStatus::Failed,
                                _ => ToolStatus::Success,
                            };
                            let subagent_type =
                                if let ClaudeToolData::Task { subagent_type, .. } = &info.tool_data
                                {
                                    subagent_type.clone()
                                } else {
                                    None
                                };
                            let desc = summary
                                .clone()
                                .or(description.clone())
                                .unwrap_or_else(|| info.content.clone());
                            let entry = Self::tool_use_entry(
                                info.tool_name.clone(),
                                ActionType::TaskCreate {
                                    description: desc.clone(),
                                    subagent_type,
                                    result: None,
                                },
                                task_status,
                                desc,
                            );
                            patches.push(ConversationPatch::replace(info.entry_index, entry));
                        }
                    }
                    Some(subtype) => {
                        let entry = NormalizedEntry {
                            timestamp: None,
                            entry_type: NormalizedEntryType::SystemMessage,
                            content: format!("System: {subtype}"),
                            metadata: Some(
                                serde_json::to_value(claude_json)
                                    .unwrap_or(serde_json::Value::Null),
                            ),
                        };
                        let idx = entry_index_provider.next();
                        patches.push(ConversationPatch::add_normalized_entry(idx, entry));
                    }
                    None => {
                        let entry = NormalizedEntry {
                            timestamp: None,
                            entry_type: NormalizedEntryType::SystemMessage,
                            content: "System message".to_string(),
                            metadata: Some(
                                serde_json::to_value(claude_json)
                                    .unwrap_or(serde_json::Value::Null),
                            ),
                        };
                        let idx = entry_index_provider.next();
                        patches.push(ConversationPatch::add_normalized_entry(idx, entry));
                    }
                }
            }
            ClaudeJson::Assistant { message, .. } => {
                if let Some(patch) = extract_model_name(self, message) {
                    patches.push(patch);
                }

                let mut streaming_message_state = message
                    .id
                    .as_ref()
                    .and_then(|id| self.streaming_messages.remove(id));

                for (content_index, item) in message.content.items().enumerate() {
                    let entry_index = streaming_message_state
                        .as_mut()
                        .and_then(|state| state.content_entry_index(content_index));

                    match item {
                        ClaudeContentItem::ToolUse { id, tool_data } => {
                            let (entry, tool_name, content_text) = Self::build_tool_use_entry(
                                tool_data,
                                worktree_path,
                                ToolStatus::Created,
                            );
                            let existing_idx = entry_index
                                .or_else(|| self.tool_map.get(id).map(|info| info.entry_index));
                            let is_new = existing_idx.is_none();
                            let id_num =
                                existing_idx.unwrap_or_else(|| entry_index_provider.next());
                            self.tool_map.insert(
                                id.clone(),
                                ClaudeToolCallInfo {
                                    entry_index: id_num,
                                    tool_name: tool_name.clone(),
                                    tool_data: tool_data.clone(),
                                    content: content_text,
                                },
                            );
                            let patch = if is_new {
                                ConversationPatch::add_normalized_entry(id_num, entry)
                            } else {
                                ConversationPatch::replace(id_num, entry)
                            };
                            patches.push(patch);
                        }
                        ClaudeContentItem::Text { .. } | ClaudeContentItem::Thinking { .. } => {
                            if let Some(entry) = Self::content_item_to_normalized_entry(
                                item,
                                &message.role,
                                worktree_path,
                                &mut self.last_assistant_message,
                            ) {
                                let is_new = entry_index.is_none();
                                let idx =
                                    entry_index.unwrap_or_else(|| entry_index_provider.next());
                                let patch = if is_new {
                                    ConversationPatch::add_normalized_entry(idx, entry)
                                } else {
                                    ConversationPatch::replace(idx, entry)
                                };
                                patches.push(patch);
                            }
                        }
                        ClaudeContentItem::ToolResult { .. } => {}
                    }
                }
            }
            ClaudeJson::User {
                message,
                is_synthetic,
                is_replay,
                ..
            } => {
                // Skip replay messages entirely - they're historical context from resumed sessions
                if *is_replay {
                    return patches;
                }

                if *is_synthetic {
                    for item in message.content.items() {
                        if let ClaudeContentItem::Text { text } = item {
                            let entry = NormalizedEntry {
                                timestamp: None,
                                entry_type: NormalizedEntryType::SystemMessage,
                                content: text.clone(),
                                metadata: None,
                            };
                            let id = entry_index_provider.next();
                            patches.push(ConversationPatch::add_normalized_entry(id, entry));
                        }
                    }
                }

                if let Some(mut text) = message.content.as_text().cloned() {
                    if text.starts_with("<local-command-stdout>")
                        && text.ends_with("</local-command-stdout>")
                    {
                        text = text
                            .trim_start_matches("<local-command-stdout>")
                            .trim_end_matches("</local-command-stdout>")
                            .to_string();
                    }
                    patches.push(add_system_message(text.clone(), entry_index_provider));
                }

                for item in message.content.items() {
                    if let ClaudeContentItem::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = item
                        && let Some(info) = self.tool_map.get(tool_use_id).cloned()
                    {
                        let is_command = matches!(info.tool_data, ClaudeToolData::Bash { .. });

                        let _display_tool_name = if is_command {
                            info.tool_name.clone()
                        } else {
                            let raw_name = info.tool_data.get_name().to_string();
                            if raw_name.starts_with("mcp__") {
                                let parts: Vec<&str> = raw_name.split("__").collect();
                                if parts.len() >= 3 {
                                    format!("mcp:{}:{}", parts[1], parts[2])
                                } else {
                                    raw_name
                                }
                            } else {
                                raw_name
                            }
                        };

                        if is_command {
                            let content_str = if let Some(s) = content.as_str() {
                                s.to_string()
                            } else {
                                content.to_string()
                            };

                            let result = if let Ok(result) =
                                serde_json::from_str::<StructuredBashResult>(&content_str)
                            {
                                Some(crate::logs::CommandRunResult {
                                    exit_status: Some(crate::logs::CommandExitStatus::ExitCode {
                                        code: result.exit_code,
                                    }),
                                    output: Some(result.output),
                                })
                            } else {
                                Some(crate::logs::CommandRunResult {
                                    exit_status: (*is_error).map(|is_error| {
                                        crate::logs::CommandExitStatus::Success {
                                            success: !is_error,
                                        }
                                    }),
                                    output: Some(content_str),
                                })
                            };

                            let status = if is_error.unwrap_or(false) {
                                ToolStatus::Failed
                            } else {
                                ToolStatus::Success
                            };

                            let entry = Self::tool_use_entry(
                                info.tool_name.clone(),
                                ActionType::CommandRun {
                                    command: info.content.clone(),
                                    result,
                                    category: CommandCategory::from_command(&info.content),
                                },
                                status,
                                info.content.clone(),
                            );
                            patches.push(ConversationPatch::replace(info.entry_index, entry));
                        } else if matches!(info.tool_data, ClaudeToolData::Task { .. }) {
                            // Handle Task tool results - capture subagent output
                            let (res_type, res_value) =
                                Self::normalize_claude_tool_result_value(content);

                            let status = if is_error.unwrap_or(false) {
                                ToolStatus::Failed
                            } else {
                                ToolStatus::Success
                            };

                            // Extract subagent_type from the original tool_data
                            let subagent_type =
                                if let ClaudeToolData::Task { subagent_type, .. } = &info.tool_data
                                {
                                    subagent_type.clone()
                                } else {
                                    None
                                };

                            let entry = Self::tool_use_entry(
                                info.tool_name.clone(),
                                ActionType::TaskCreate {
                                    description: info.content.clone(),
                                    subagent_type,
                                    result: Some(crate::logs::ToolResult {
                                        r#type: res_type,
                                        value: res_value,
                                    }),
                                },
                                status,
                                info.content.clone(),
                            );
                            patches.push(ConversationPatch::replace(info.entry_index, entry));
                        } else if matches!(
                            info.tool_data,
                            ClaudeToolData::Unknown { .. } | ClaudeToolData::NotebookEdit { .. }
                        ) {
                            let (res_type, res_value) =
                                Self::normalize_claude_tool_result_value(content);

                            let args_to_show = serde_json::to_value(&info.tool_data)
                                .ok()
                                .and_then(|v| serde_json::from_value::<ClaudeToolWithInput>(v).ok())
                                .map(|w| w.input)
                                .unwrap_or(serde_json::Value::Null);

                            let tool_name = info.tool_data.get_name().to_string();
                            let is_mcp = tool_name.starts_with("mcp__");
                            let label = if is_mcp {
                                let parts: Vec<&str> = tool_name.split("__").collect();
                                if parts.len() >= 3 {
                                    format!("mcp:{}:{}", parts[1], parts[2])
                                } else {
                                    tool_name.clone()
                                }
                            } else {
                                tool_name.clone()
                            };

                            let status = if is_error.unwrap_or(false) {
                                ToolStatus::Failed
                            } else {
                                ToolStatus::Success
                            };

                            let entry = Self::tool_use_entry(
                                label.clone(),
                                ActionType::Tool {
                                    tool_name: label,
                                    arguments: Some(args_to_show),
                                    result: Some(crate::logs::ToolResult {
                                        r#type: res_type,
                                        value: res_value,
                                    }),
                                },
                                status,
                                info.content.clone(),
                            );
                            patches.push(ConversationPatch::replace(info.entry_index, entry));
                        }
                        // Note: With control protocol, denials are handled via protocol messages
                        // rather than error content parsing
                    }
                }
            }
            ClaudeJson::ToolUse { tool_data, id, .. } => {
                let (entry, tool_name_value, content_text) =
                    Self::build_tool_use_entry(tool_data, worktree_path, ToolStatus::Created);
                let existing = self.tool_map.get(id);
                let (idx, is_new) = if let Some(info) = existing {
                    (info.entry_index, false)
                } else {
                    (entry_index_provider.next(), true)
                };
                let patch = if is_new {
                    ConversationPatch::add_normalized_entry(idx, entry)
                } else {
                    ConversationPatch::replace(idx, entry)
                };
                patches.push(patch);

                self.tool_map.insert(
                    id.clone(),
                    ClaudeToolCallInfo {
                        entry_index: idx,
                        tool_name: tool_name_value,
                        tool_data: tool_data.clone(),
                        content: content_text,
                    },
                );
            }
            ClaudeJson::ToolResult { .. } => {
                // Add proper ToolResult support to NormalizedEntry when the type system supports it
            }
            ClaudeJson::StreamEvent {
                event,
                parent_tool_use_id,
                ..
            } => match event {
                ClaudeStreamEvent::MessageStart { message } => {
                    if message.role == "assistant" {
                        if let Some(patch) = extract_model_name(self, message) {
                            patches.push(patch);
                        }

                        if let Some(message_id) = message.id.clone() {
                            self.streaming_messages.insert(
                                message_id.clone(),
                                StreamingMessageState::new(message.role.clone()),
                            );
                            self.streaming_message_id = Some(message_id);
                        } else {
                            self.streaming_message_id = None;
                        }
                    } else {
                        self.streaming_message_id = None;
                    }
                }
                ClaudeStreamEvent::ContentBlockStart {
                    index,
                    content_block,
                } => {
                    if let Some(state) = self
                        .streaming_message_id
                        .as_ref()
                        .and_then(|id| self.streaming_messages.get_mut(id))
                    {
                        state.content_block_start(*index, content_block.clone());
                    }
                }
                ClaudeStreamEvent::ContentBlockDelta { index, delta } => {
                    if let Some(state) = self
                        .streaming_message_id
                        .as_ref()
                        .and_then(|id| self.streaming_messages.get_mut(id))
                        && let Some(patch) = state.apply_content_block_delta(
                            *index,
                            delta,
                            worktree_path,
                            entry_index_provider,
                            &mut self.last_assistant_message,
                        )
                    {
                        patches.push(patch);
                    }
                }
                ClaudeStreamEvent::ContentBlockStop { .. } => {}
                ClaudeStreamEvent::MessageDelta { usage, .. } => {
                    // do not report context token usage for subagents
                    if parent_tool_use_id.is_none()
                        && let Some(usage) = usage
                    {
                        let input_tokens = usage.input_tokens.unwrap_or(0)
                            + usage.cache_creation_input_tokens.unwrap_or(0)
                            + usage.cache_read_input_tokens.unwrap_or(0);
                        let output_tokens = usage.output_tokens.unwrap_or(0);
                        let total_tokens = input_tokens + output_tokens;
                        self.context_tokens_used = total_tokens as u32;

                        patches.push(self.add_token_usage_entry(entry_index_provider));
                    }
                }
                ClaudeStreamEvent::MessageStop => {
                    if let Some(message_id) = self.streaming_message_id.take() {
                        let _ = self.streaming_messages.remove(&message_id);
                    }
                }
                ClaudeStreamEvent::Unknown => {}
            },
            ClaudeJson::Result {
                model_usage,
                subtype,
                result,
                ..
            } => {
                // get the real model context window and correct the context usage entry
                if let Some(context_window) = model_usage.as_ref().and_then(|model_usage| {
                    self.main_model_name
                        .as_ref()
                        .and_then(|name| model_usage.get(name))
                        .and_then(|usage| usage.context_window)
                }) {
                    self.main_model_context_window = context_window;
                    patches.push(self.add_token_usage_entry(entry_index_provider));
                }

                if matches!(subtype.as_deref(), Some("success"))
                    && let Some(text) = result.as_ref().and_then(|v| v.as_str())
                    && (self.last_assistant_message.is_none()
                        || matches!(&self.last_assistant_message, Some(message) if !message.contains(text)))
                {
                    let entry = NormalizedEntry {
                        timestamp: None,
                        entry_type: NormalizedEntryType::AssistantMessage,
                        content: text.to_string(),
                        metadata: Some(
                            serde_json::to_value(claude_json).unwrap_or(serde_json::Value::Null),
                        ),
                    };
                    let idx = entry_index_provider.next();
                    patches.push(ConversationPatch::add_normalized_entry(idx, entry));
                }
            }
            ClaudeJson::ApprovalRequested {
                tool_call_id,
                tool_name: _,
                approval_id,
            } => {
                self.replace_tool_entry_status(
                    tool_call_id,
                    ToolStatus::PendingApproval {
                        approval_id: approval_id.clone(),
                    },
                    worktree_path,
                    &mut patches,
                );
            }
            ClaudeJson::ApprovalResponse {
                tool_call_id,
                tool_name,
                approval_status,
            } => {
                if let Some(status) = ToolStatus::from_approval_status(approval_status) {
                    self.replace_tool_entry_status(
                        tool_call_id,
                        status,
                        worktree_path,
                        &mut patches,
                    );
                }

                let entry_opt = match approval_status {
                    ApprovalStatus::Pending | ApprovalStatus::Approved => None,
                    ApprovalStatus::Denied { reason } => Some(NormalizedEntry {
                        timestamp: None,
                        entry_type: NormalizedEntryType::UserFeedback {
                            denied_tool: tool_name.clone(),
                        },
                        content: reason
                            .as_ref()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "User denied this tool use request".to_string()),
                        metadata: None,
                    }),
                    ApprovalStatus::TimedOut => Some(NormalizedEntry {
                        timestamp: None,
                        entry_type: NormalizedEntryType::ErrorMessage {
                            error_type: NormalizedEntryError::Other,
                        },
                        content: format!("Approval timed out for tool {tool_name}"),
                        metadata: None,
                    }),
                };

                if let Some(entry) = entry_opt {
                    let idx = entry_index_provider.next();
                    patches.push(ConversationPatch::add_normalized_entry(idx, entry));
                }
            }
            ClaudeJson::QuestionResponse {
                tool_call_id,
                tool_name: _,
                question_status,
            } => {
                let status = ToolStatus::from_question_status(question_status);
                self.replace_tool_entry_status(tool_call_id, status, worktree_path, &mut patches);
                let entry_opt = match question_status {
                    QuestionStatus::Answered { answers } => {
                        let qa_pairs: Vec<AnsweredQuestion> = answers
                            .iter()
                            .map(|qa| AnsweredQuestion {
                                question: qa.question.clone(),
                                answer: qa.answer.clone(),
                            })
                            .collect();
                        Some(NormalizedEntry {
                            timestamp: None,
                            entry_type: NormalizedEntryType::UserAnsweredQuestions {
                                answers: qa_pairs,
                            },
                            content: format!(
                                "Answered {} question{}",
                                answers.len(),
                                if answers.len() != 1 { "s" } else { "" }
                            ),
                            metadata: None,
                        })
                    }
                    QuestionStatus::TimedOut => None,
                };

                if let Some(entry) = entry_opt {
                    let idx = entry_index_provider.next();
                    patches.push(ConversationPatch::add_normalized_entry(idx, entry));
                }
            }
            ClaudeJson::Unknown { data } => {
                let entry = NormalizedEntry {
                    timestamp: None,
                    entry_type: NormalizedEntryType::SystemMessage,
                    content: format!(
                        "Unrecognized JSON message: {}",
                        serde_json::to_value(data).unwrap_or_default()
                    ),
                    metadata: None,
                };
                let idx = entry_index_provider.next();
                patches.push(ConversationPatch::add_normalized_entry(idx, entry));
            }
            ClaudeJson::RateLimitEvent {
                rate_limit_info, ..
            } => {
                let content = Self::format_rate_limit_event_message(rate_limit_info.as_ref());
                self.push_deduped_retry_notice(&mut patches, entry_index_provider, content);
            }
            ClaudeJson::ControlRequest { .. }
            | ClaudeJson::ControlResponse { .. }
            | ClaudeJson::ControlCancelRequest { .. } => {}
        }
        patches
    }

    fn push_deduped_retry_notice(
        &mut self,
        patches: &mut Vec<json_patch::Patch>,
        entry_index_provider: &EntryIndexProvider,
        content: String,
    ) {
        if self.retry_status_fingerprint.as_deref() == Some(content.as_str()) {
            return;
        }

        self.retry_status_fingerprint = Some(content.clone());
        patches.push(add_system_message(content, entry_index_provider));
    }

    fn format_api_retry_message(
        status: &Option<String>,
        description: &Option<String>,
        summary: &Option<String>,
        extra: &HashMap<String, serde_json::Value>,
    ) -> String {
        let message = first_extra_field(
            extra,
            &["message", "error", "reason", "detail", "details", "cause"],
        )
        .or_else(|| non_empty_clone(description))
        .or_else(|| non_empty_clone(summary))
        .or_else(|| non_empty_clone(status));

        let mut details = Vec::new();
        if let Some(attempt) = first_extra_field(
            extra,
            &[
                "attempt",
                "attemptNumber",
                "attempt_number",
                "retryAttempt",
                "retry_attempt",
                "retryCount",
                "retry_count",
            ],
        ) {
            details.push(format!("attempt {attempt}"));
        }

        if let Some((key, value)) = first_extra_field_with_key(
            extra,
            &[
                "next",
                "delay",
                "delayMs",
                "delay_ms",
                "retryAfter",
                "retry_after",
                "retryAfterMs",
                "retry_after_ms",
                "nextRetry",
                "next_retry",
                "nextRetryMs",
                "next_retry_ms",
                "sleepMs",
                "sleep_ms",
            ],
        ) {
            details.push(format!("next in {}", format_retry_delay(&key, &value)));
        }

        format_retry_notice("Claude Code retrying API request", message, details)
    }

    fn format_rate_limit_event_message(rate_limit_info: Option<&serde_json::Value>) -> String {
        let message = rate_limit_info
            .and_then(value_to_display_string)
            .or_else(|| rate_limit_info.and_then(compact_json_display));

        format_retry_notice(
            "Claude Code rate limit notice",
            message,
            Vec::<String>::new(),
        )
    }
    /// Generate concise, readable content for tool usage using structured data
    fn generate_concise_content(
        tool_data: &ClaudeToolData,
        action_type: &ActionType,
        worktree_path: &str,
    ) -> String {
        match action_type {
            ActionType::FileRead { path } => path.to_string(),
            ActionType::FileEdit { path, .. } => path.to_string(),
            ActionType::CommandRun { command, .. } => command.to_string(),
            ActionType::Search { query } => query.to_string(),
            ActionType::WebFetch { url } => url.to_string(),
            ActionType::TaskCreate { description, .. } => {
                if description.is_empty() {
                    "Task".to_string()
                } else {
                    format!("Task: `{description}`")
                }
            }
            ActionType::Tool { .. } => match tool_data {
                ClaudeToolData::NotebookEdit { notebook_path, .. } => {
                    format!("`{}`", make_path_relative(notebook_path, worktree_path))
                }
                ClaudeToolData::Unknown { .. } => {
                    let name = tool_data.get_name();
                    if name.starts_with("mcp__") {
                        let parts: Vec<&str> = name.split("__").collect();
                        if parts.len() >= 3 {
                            return format!("mcp:{}:{}", parts[1], parts[2]);
                        }
                    }
                    name.to_string()
                }
                _ => tool_data.get_name().to_string(),
            },
            ActionType::PlanPresentation { plan } => plan.clone(),
            ActionType::TodoManagement { .. } => "TODO list updated".to_string(),
            ActionType::AskUserQuestion { questions } => {
                if questions.len() == 1 {
                    questions[0].question.clone()
                } else {
                    format!("{} questions", questions.len())
                }
            }
            ActionType::Other { description: _ } => match tool_data {
                ClaudeToolData::LS { path } => {
                    let relative_path = make_path_relative(path, worktree_path);
                    if relative_path.is_empty() {
                        "List directory".to_string()
                    } else {
                        format!("List directory: {relative_path}")
                    }
                }
                ClaudeToolData::Glob { pattern, path, .. } => {
                    if let Some(search_path) = path {
                        format!(
                            "Find files: `{}` in {}",
                            pattern,
                            make_path_relative(search_path, worktree_path)
                        )
                    } else {
                        format!("Find files: `{pattern}`")
                    }
                }
                _ => tool_data.get_name().to_string(),
            },
        }
    }

    fn add_token_usage_entry(
        &mut self,
        entry_index_provider: &EntryIndexProvider,
    ) -> json_patch::Patch {
        let entry = NormalizedEntry {
            timestamp: None,
            entry_type: NormalizedEntryType::TokenUsageInfo(crate::logs::TokenUsageInfo {
                total_tokens: self.context_tokens_used,
                model_context_window: self.main_model_context_window,
            }),
            content: format!(
                "Tokens used: {} / Context window: {}",
                self.context_tokens_used, self.main_model_context_window
            ),
            metadata: None,
        };
        let idx = entry_index_provider.next();
        ConversationPatch::add_normalized_entry(idx, entry)
    }
}

fn add_system_message(
    content: String,
    entry_index_provider: &EntryIndexProvider,
) -> json_patch::Patch {
    let entry = NormalizedEntry {
        timestamp: None,
        entry_type: NormalizedEntryType::SystemMessage,
        content,
        metadata: None,
    };
    let id = entry_index_provider.next();
    ConversationPatch::add_normalized_entry(id, entry)
}

fn non_empty_clone(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn first_extra_field(extra: &HashMap<String, serde_json::Value>, keys: &[&str]) -> Option<String> {
    first_extra_field_with_key(extra, keys).map(|(_, value)| value)
}

fn first_extra_field_with_key(
    extra: &HashMap<String, serde_json::Value>,
    keys: &[&str],
) -> Option<(String, String)> {
    keys.iter().find_map(|key| {
        extra
            .get(*key)
            .and_then(value_to_display_string)
            .map(|value| ((*key).to_string(), value))
    })
}

fn value_to_display_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Object(object) => [
            "message",
            "error",
            "reason",
            "description",
            "detail",
            "code",
            "type",
        ]
        .iter()
        .find_map(|key| object.get(*key).and_then(value_to_display_string)),
        _ => None,
    }
}

fn compact_json_display(value: &serde_json::Value) -> Option<String> {
    let mut text = serde_json::to_string(value).ok()?;
    const MAX_LEN: usize = 180;
    if text.len() > MAX_LEN {
        text.truncate(MAX_LEN);
        text.push_str("...");
    }
    Some(text)
}

fn format_retry_delay(key: &str, value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed
            .chars()
            .any(|ch| ch.is_ascii_alphabetic() || ch == ':')
    {
        return trimmed.to_string();
    }

    let key = key.to_ascii_lowercase();
    if key.contains("ms") || key == "next" {
        format!("{trimmed}ms")
    } else if key.contains("after") || key.contains("second") || key.ends_with("_s") {
        format!("{trimmed}s")
    } else {
        trimmed.to_string()
    }
}

fn format_retry_notice(base: &str, message: Option<String>, details: Vec<String>) -> String {
    let mut content = base.to_string();
    if let Some(message) = message {
        content.push_str(": ");
        content.push_str(&message);
    }
    if !details.is_empty() {
        content.push_str(" (");
        content.push_str(&details.join(", "));
        content.push(')');
    }
    content
}

fn add_loading_entry(entry_index_provider: &EntryIndexProvider) -> json_patch::Patch {
    let entry = NormalizedEntry {
        timestamp: None,
        entry_type: NormalizedEntryType::Loading,
        content: String::new(),
        metadata: None,
    };
    let id = entry_index_provider.next();
    ConversationPatch::add_normalized_entry(id, entry)
}

fn extract_model_name(
    processor: &mut ClaudeLogProcessor,
    message: &ClaudeMessage,
) -> Option<json_patch::Patch> {
    if processor.model_name.is_none()
        && let Some(model) = message.model.as_ref()
    {
        processor.model_name = Some(model.clone());
    }

    None
}

struct StreamingMessageState {
    role: String,
    contents: HashMap<usize, StreamingContentState>,
}

impl StreamingMessageState {
    fn new(role: String) -> Self {
        Self {
            role,
            contents: HashMap::new(),
        }
    }

    fn content_block_start(&mut self, index: usize, content_block: ClaudeContentItem) {
        if let Some(state) = StreamingContentState::from_content_block(content_block) {
            self.contents.insert(index, state);
        }
    }

    fn apply_content_block_delta(
        &mut self,
        index: usize,
        delta: &ClaudeContentBlockDelta,
        worktree_path: &str,
        entry_index_provider: &EntryIndexProvider,
        last_assistant_message: &mut Option<String>,
    ) -> Option<json_patch::Patch> {
        if let std::collections::hash_map::Entry::Vacant(e) = self.contents.entry(index) {
            let new_state = StreamingContentState::from_delta(delta)?;
            e.insert(new_state);
        }

        let entry_state = self.contents.get_mut(&index)?;
        entry_state.apply_content_delta(delta);

        let content_item = entry_state.to_content_item();
        let entry = ClaudeLogProcessor::content_item_to_normalized_entry(
            &content_item,
            &self.role,
            worktree_path,
            last_assistant_message,
        )?;

        if let Some(existing_index) = entry_state.entry_index {
            Some(ConversationPatch::replace(existing_index, entry))
        } else {
            let entry_index = entry_index_provider.next();
            entry_state.entry_index = Some(entry_index);
            Some(ConversationPatch::add_normalized_entry(entry_index, entry))
        }
    }

    fn content_entry_index(&self, content_index: usize) -> Option<usize> {
        self.contents
            .get(&content_index)
            .and_then(|s| s.entry_index)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamingContentKind {
    Text,
    Thinking,
}

struct StreamingContentState {
    kind: StreamingContentKind,
    buffer: String,
    entry_index: Option<usize>,
}

impl StreamingContentState {
    fn from_content_block(content_block: ClaudeContentItem) -> Option<Self> {
        match content_block {
            ClaudeContentItem::Text { text } => Some(Self {
                kind: StreamingContentKind::Text,
                buffer: text,
                entry_index: None,
            }),
            ClaudeContentItem::Thinking { thinking } => Some(Self {
                kind: StreamingContentKind::Thinking,
                buffer: thinking,
                entry_index: None,
            }),
            _ => None,
        }
    }

    fn from_delta(delta: &ClaudeContentBlockDelta) -> Option<Self> {
        match delta {
            ClaudeContentBlockDelta::TextDelta { .. } => Some(Self {
                kind: StreamingContentKind::Text,
                buffer: String::new(),
                entry_index: None,
            }),
            ClaudeContentBlockDelta::ThinkingDelta { .. } => Some(Self {
                kind: StreamingContentKind::Thinking,
                buffer: String::new(),
                entry_index: None,
            }),
            _ => None,
        }
    }

    fn apply_content_delta(&mut self, delta: &ClaudeContentBlockDelta) {
        match (self.kind, delta) {
            (StreamingContentKind::Text, ClaudeContentBlockDelta::TextDelta { text }) => {
                self.buffer.push_str(text);
            }
            (
                StreamingContentKind::Thinking,
                ClaudeContentBlockDelta::ThinkingDelta { thinking },
            ) => {
                self.buffer.push_str(thinking);
            }
            // Signature deltas are sent at the end of thinking blocks for verification;
            // they don't contain display content so we ignore them.
            (StreamingContentKind::Thinking, ClaudeContentBlockDelta::SignatureDelta { .. }) => {}
            _ => {
                tracing::warn!(
                    "Mismatched content types: delta {:?}, kind {:?}",
                    delta,
                    self.kind
                );
            }
        }
    }

    fn to_content_item(&self) -> ClaudeContentItem {
        match self.kind {
            StreamingContentKind::Text => ClaudeContentItem::Text {
                text: self.buffer.clone(),
            },
            StreamingContentKind::Thinking => ClaudeContentItem::Thinking {
                thinking: self.buffer.clone(),
            },
        }
    }
}

// Data structures for parsing Claude's JSON output format
fn deserialize_string_list_from_values<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(vec![]);
    };

    let values = match value {
        serde_json::Value::Array(values) => values,
        serde_json::Value::String(value) => return Ok(vec![value]),
        _ => return Ok(vec![]),
    };

    Ok(values
        .into_iter()
        .filter_map(|value| match value {
            serde_json::Value::String(value) => Some(value),
            serde_json::Value::Object(object) => object
                .get("name")
                .or_else(|| object.get("command"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            _ => None,
        })
        .collect())
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClaudeJson {
    System {
        subtype: Option<String>,
        session_id: Option<String>,
        cwd: Option<String>,
        tools: Option<Vec<serde_json::Value>>,
        model: Option<String>,
        #[serde(default, rename = "apiKeySource")]
        api_key_source: Option<String>,
        status: Option<String>,
        #[serde(default)]
        slash_commands: Vec<String>,
        #[serde(default)]
        plugins: Vec<ClaudePlugin>,
        #[serde(default)]
        agents: Vec<String>,
        #[serde(default, deserialize_with = "deserialize_string_list_from_values")]
        skills: Vec<String>,
        #[serde(default)]
        task_id: Option<String>,
        #[serde(default)]
        tool_use_id: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        task_type: Option<String>,
        #[serde(default)]
        prompt: Option<String>,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        last_tool_name: Option<String>,
        #[serde(default)]
        #[serde(flatten)]
        extra: HashMap<String, serde_json::Value>,
    },
    Assistant {
        message: ClaudeMessage,
        session_id: Option<String>,
        #[serde(default)]
        uuid: Option<String>,
    },
    User {
        message: ClaudeMessage,
        session_id: Option<String>,
        #[serde(default)]
        uuid: Option<String>,
        #[serde(default, rename = "isSynthetic")]
        is_synthetic: bool,
        #[serde(default, rename = "isReplay")]
        is_replay: bool,
    },
    ToolUse {
        id: String,
        tool_name: String,
        #[serde(flatten)]
        tool_data: ClaudeToolData,
        session_id: Option<String>,
    },
    ToolResult {
        result: serde_json::Value,
        is_error: Option<bool>,
        session_id: Option<String>,
    },
    StreamEvent {
        event: ClaudeStreamEvent,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        parent_tool_use_id: Option<String>,
        #[serde(default)]
        uuid: Option<String>,
    },
    Result {
        #[serde(default)]
        subtype: Option<String>,
        #[serde(default, alias = "isError")]
        is_error: Option<bool>,
        #[serde(default, alias = "durationMs")]
        duration_ms: Option<u64>,
        #[serde(default)]
        result: Option<serde_json::Value>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default, alias = "numTurns")]
        num_turns: Option<u32>,
        #[serde(default, alias = "sessionId")]
        session_id: Option<String>,
        #[serde(default, alias = "modelUsage")]
        model_usage: Option<HashMap<String, ClaudeModelUsage>>,
        #[serde(default)]
        usage: Option<ClaudeUsage>,
    },
    ApprovalRequested {
        tool_call_id: String,
        tool_name: String,
        approval_id: String,
    },
    ApprovalResponse {
        tool_call_id: String,
        tool_name: String,
        approval_status: ApprovalStatus,
    },
    QuestionResponse {
        tool_call_id: String,
        tool_name: String,
        question_status: QuestionStatus,
    },
    ControlRequest {
        request_id: String,
        request: ControlRequestType,
    },
    ControlResponse {
        response: ControlResponseType,
    },
    ControlCancelRequest {
        request_id: String,
    },
    RateLimitEvent {
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        rate_limit_info: Option<serde_json::Value>,
    },
    // Catch-all for unknown message types
    #[serde(untagged)]
    Unknown {
        #[serde(flatten)]
        data: HashMap<String, serde_json::Value>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ClaudePlugin {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct ClaudeMessage {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub message_type: Option<String>,
    pub role: String,
    pub model: Option<String>,
    pub content: ClaudeMessageContent,
    pub stop_reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum ClaudeMessageContent {
    Array(Vec<ClaudeContentItem>),
    Text(String),
}

impl ClaudeMessageContent {
    fn items(&self) -> impl Iterator<Item = &ClaudeContentItem> {
        match self {
            ClaudeMessageContent::Array(items) => items.iter(),
            ClaudeMessageContent::Text(_) => [].iter(),
        }
    }

    fn as_text(&self) -> Option<&String> {
        match self {
            ClaudeMessageContent::Text(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum ClaudeContentItem {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        #[serde(flatten)]
        tool_data: ClaudeToolData,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: serde_json::Value,
        is_error: Option<bool>,
    },
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum ClaudeStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: ClaudeMessage },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: ClaudeContentItem,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta {
        index: usize,
        delta: ClaudeContentBlockDelta,
    },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta {
        #[serde(default)]
        delta: Option<ClaudeMessageDelta>,
        #[serde(default)]
        usage: Option<ClaudeUsage>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum ClaudeContentBlockDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta {
        #[serde(default)]
        signature: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Default)]
pub struct ClaudeMessageDelta {
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Default)]
pub struct ClaudeUsage {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default, rename = "cache_creation_input_tokens")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default, rename = "cache_read_input_tokens")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    pub service_tier: Option<String>,
}

/// Per-model usage statistics from result message
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeModelUsage {
    #[serde(default)]
    pub context_window: Option<u32>,
}

/// Structured tool data for Claude tools based on real samples
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "name", content = "input")]
pub enum ClaudeToolData {
    #[serde(rename = "TodoWrite", alias = "todo_write")]
    TodoWrite {
        todos: Vec<ClaudeTodoItem>,
    },
    #[serde(rename = "Task", alias = "task", alias = "Agent")]
    Task {
        subagent_type: Option<String>,
        description: Option<String>,
        prompt: Option<String>,
    },
    #[serde(rename = "Glob", alias = "glob")]
    Glob {
        #[serde(alias = "filePattern")]
        pattern: String,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        limit: Option<u32>,
    },
    #[serde(rename = "LS", alias = "list_directory", alias = "ls")]
    LS {
        path: String,
    },
    #[serde(rename = "Read", alias = "read")]
    Read {
        #[serde(alias = "path")]
        file_path: String,
    },
    #[serde(rename = "Bash", alias = "bash")]
    Bash {
        #[serde(alias = "cmd", alias = "command_line")]
        command: String,
        #[serde(default)]
        description: Option<String>,
    },
    #[serde(rename = "Grep", alias = "grep")]
    Grep {
        pattern: String,
        #[serde(default)]
        output_mode: Option<String>,
        #[serde(default)]
        path: Option<String>,
    },
    ExitPlanMode {
        plan: String,
    },
    #[serde(rename = "Edit", alias = "edit_file")]
    Edit {
        #[serde(alias = "path")]
        file_path: String,
        #[serde(alias = "old_str")]
        old_string: Option<String>,
        #[serde(alias = "new_str")]
        new_string: Option<String>,
    },
    #[serde(rename = "MultiEdit", alias = "multi_edit")]
    MultiEdit {
        #[serde(alias = "path")]
        file_path: String,
        edits: Vec<ClaudeEditItem>,
    },
    #[serde(rename = "Write", alias = "create_file", alias = "write_file")]
    Write {
        #[serde(alias = "path")]
        file_path: String,
        content: String,
    },
    #[serde(rename = "NotebookEdit", alias = "notebook_edit")]
    NotebookEdit {
        notebook_path: String,
        new_source: String,
        edit_mode: String,
        #[serde(default)]
        cell_id: Option<String>,
    },
    #[serde(rename = "WebFetch", alias = "read_web_page")]
    WebFetch {
        url: String,
        #[serde(default)]
        prompt: Option<String>,
    },
    #[serde(rename = "WebSearch", alias = "web_search")]
    WebSearch {
        query: String,
        #[serde(default)]
        num_results: Option<u32>,
    },
    #[serde(rename = "TodoRead", alias = "todo_read")]
    TodoRead {},
    AskUserQuestion {
        questions: Vec<AskUserQuestionInputItem>,
    },
    #[serde(untagged)]
    Unknown {
        #[serde(flatten)]
        data: std::collections::HashMap<String, serde_json::Value>,
    },
}

// Helper structs for parsing tool_result content and generic tool input
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
struct ClaudeToolResultTextItem {
    text: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
struct ClaudeToolWithInput {
    #[serde(default)]
    input: serde_json::Value,
}

// Claude-compatible structured Bash tool_result content format
// Example content (often delivered as a JSON string):
//   {"output":"...","exitCode":0}
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
struct StructuredBashResult {
    #[serde(default)]
    output: String,
    #[serde(rename = "exitCode")]
    exit_code: i32,
}

#[derive(Debug, Clone)]
struct ClaudeToolCallInfo {
    entry_index: usize,
    tool_name: String,
    tool_data: ClaudeToolData,
    content: String,
}

/// A single question from AskUserQuestion tool input.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct AskUserQuestionInputItem {
    pub question: String,
    pub header: String,
    pub options: Vec<AskUserQuestionInputOption>,
    #[serde(rename = "multiSelect")]
    pub multi_select: bool,
}

/// An option for an AskUserQuestion question.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct AskUserQuestionInputOption {
    pub label: String,
    pub description: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct ClaudeTodoItem {
    #[serde(default)]
    pub id: Option<String>,
    pub content: String,
    pub status: String,
    #[serde(default)]
    pub priority: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct ClaudeEditItem {
    pub old_string: Option<String>,
    pub new_string: Option<String>,
}

impl ClaudeToolData {
    pub fn get_name(&self) -> &str {
        match self {
            ClaudeToolData::TodoWrite { .. } => "TodoWrite",
            ClaudeToolData::Task { .. } => "Task",
            ClaudeToolData::Glob { .. } => "Glob",
            ClaudeToolData::LS { .. } => "LS",
            ClaudeToolData::Read { .. } => "Read",
            ClaudeToolData::Bash { .. } => "Bash",
            ClaudeToolData::Grep { .. } => "Grep",
            ClaudeToolData::ExitPlanMode { .. } => "ExitPlanMode",
            ClaudeToolData::Edit { .. } => "Edit",
            ClaudeToolData::MultiEdit { .. } => "MultiEdit",
            ClaudeToolData::Write { .. } => "Write",
            ClaudeToolData::NotebookEdit { .. } => "NotebookEdit",
            ClaudeToolData::WebFetch { .. } => "WebFetch",
            ClaudeToolData::WebSearch { .. } => "WebSearch",
            ClaudeToolData::TodoRead { .. } => "TodoRead",
            ClaudeToolData::AskUserQuestion { .. } => "AskUserQuestion",
            ClaudeToolData::Unknown { data } => data
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::utils::{EntryIndexProvider, patch::extract_normalized_entry_from_patch};

    static DISCOVERY_FIXTURE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn patches_to_entries(patches: &[json_patch::Patch]) -> Vec<NormalizedEntry> {
        patches
            .iter()
            .filter_map(|patch| extract_normalized_entry_from_patch(patch).map(|(_, entry)| entry))
            .collect()
    }

    fn normalize_helper(
        processor: &mut ClaudeLogProcessor,
        json: &ClaudeJson,
        worktree: &str,
    ) -> Vec<NormalizedEntry> {
        let provider = EntryIndexProvider::test_new();
        let patches = processor.normalize_entries(json, worktree, &provider);
        patches_to_entries(&patches)
    }

    fn normalize(json: &ClaudeJson, worktree: &str) -> Vec<NormalizedEntry> {
        let mut processor = ClaudeLogProcessor::new();
        normalize_helper(&mut processor, json, worktree)
    }

    fn test_executor(claude_code_router: Option<bool>) -> ClaudeCode {
        ClaudeCode {
            claude_code_router,
            plan: None,
            approvals: None,
            model: None,
            effort: None,
            fast_mode: None,
            ultracode: None,
            native_permission_mode: None,
            agent: None,
            append_prompt: AppendPrompt::default(),
            dangerously_skip_permissions: None,
            cmd: crate::command::CmdOverrides::default(),
            approvals_service: None,
            disable_api_key: None,
        }
    }

    #[tokio::test]
    async fn default_claude_uses_local_claude_command() {
        let executor = test_executor(None);

        let builder = executor
            .build_command_builder()
            .await
            .expect("command should build");

        assert_eq!(builder.base, "claude");
        assert_eq!(builder.params.as_ref().unwrap()[0], "-p");
    }

    #[tokio::test]
    async fn claude_code_router_uses_local_ccr_command() {
        let executor = test_executor(Some(true));

        let builder = executor
            .build_command_builder()
            .await
            .expect("command should build");

        assert_eq!(builder.base, "ccr code");
        assert_eq!(builder.params.as_ref().unwrap()[0], "-p");
    }

    #[test]
    fn command_builder_rejects_explicit_effort_when_cli_does_not_support_it() {
        let mut executor = test_executor(None);
        executor.effort = Some(ClaudeEffort::Max);

        assert!(
            executor
                .build_command_builder_with_effort_support(false)
                .is_err()
        );
    }

    #[test]
    fn command_builder_includes_effort_when_cli_supports_it() {
        let mut executor = test_executor(None);
        executor.effort = Some(ClaudeEffort::Max);

        let builder = executor
            .build_command_builder_with_effort_support(true)
            .expect("command should build");
        let params = builder.params.as_ref().expect("params should be present");
        let effort_index = params
            .iter()
            .position(|param| param == "--effort")
            .expect("effort flag should be present");

        assert_eq!(
            params.get(effort_index + 1).map(String::as_str),
            Some("max")
        );
    }

    #[test]
    fn default_options_hide_effort_when_cli_does_not_support_it() {
        let options = default_discovered_options(false);

        for model in &options.model_selector.models {
            assert!(
                model.reasoning_options.is_empty(),
                "{} should not expose effort options",
                model.id
            );
        }
    }

    #[test]
    fn native_defaults_read_configured_model_and_per_model_effort_without_execution_overrides() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("claude-config");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{
            "model":"custom-model-alias",
            "effortLevel":"high",
            "modelSettings":{"canonical-future-model":{"effortLevel":"xhigh"}},
            "env":{"ANTHROPIC_API_KEY":"secret-not-for-picker"}
        }"#,
        )
        .unwrap();
        let mut executor = test_executor(None);
        executor.cmd.env = Some(HashMap::from([
            (
                "CLAUDE_CONFIG_DIR".into(),
                root.to_string_lossy().into_owned(),
            ),
            (
                if cfg!(windows) { "USERPROFILE" } else { "HOME" }.into(),
                directory.path().to_string_lossy().into_owned(),
            ),
            ("ANTHROPIC_MODEL".into(), "custom-model-alias".into()),
            ("CLAUDE_CODE_EFFORT_LEVEL".into(), String::new()),
        ]));
        let defaults = executor.native_model_defaults(directory.path()).unwrap();
        assert_eq!(defaults.model_id.as_deref(), Some("custom-model-alias"));
        assert_eq!(defaults.reasoning_id.as_deref(), Some("high"));
        assert_eq!(
            defaults
                .model_reasoning
                .get("canonical-future-model")
                .map(String::as_str),
            Some("xhigh")
        );
        assert!(
            !serde_json::to_string(&defaults)
                .unwrap()
                .contains("secret-not-for-picker")
        );
        assert_eq!(executor.get_preset_options().model_id, None);
        assert_eq!(executor.get_preset_options().reasoning_id, None);
        // Opaque wrappers cannot establish that host files are their authority.
        executor.cmd.base_command_override = Some("custom-claude-wrapper".into());
        assert!(executor.native_model_defaults(directory.path()).is_none());
    }

    #[tokio::test]
    async fn discovery_cache_tracks_native_config_changes_and_preserves_catalog_source() {
        let _guard = DISCOVERY_FIXTURE_LOCK.lock().await;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("native-config");
        std::fs::create_dir_all(&root).unwrap();
        let settings = root.join("settings.json");
        std::fs::write(&settings, r#"{"model":"local-alias-a"}"#).unwrap();
        let trace = directory.path().join("initialize.jsonl");
        let script = directory.path().join("claude-discovery.cjs");
        std::fs::write(&script, r#"
const fs = require('node:fs');
if (process.argv.includes('--help')) { process.stdout.write('--effort'); process.exit(0); }
const readline = require('node:readline');
readline.createInterface({input:process.stdin}).on('line', line => {
  const request = JSON.parse(line);
  fs.appendFileSync(process.env.VK_FIXTURE_TRACE, JSON.stringify({request,args:process.argv.slice(2)}) + '\n');
  if (request.type !== 'control_request' || request.request.subtype !== 'initialize') process.exit(22);
  process.stdout.write(JSON.stringify({type:'control_response',response:{subtype:'success',request_id:request.request_id,response:{models:[{value:'native-alias',resolvedModel:'canonical-native-model',supportsEffort:true,supportedEffortLevels:['high','max']}]}}}) + '\n');
});
"#).unwrap();
        let mut executor = test_executor(None);
        executor.cmd.base_command_override = Some(format!("node \"{}\"", script.display()));
        executor.cmd.env = Some(HashMap::from([
            (
                "CLAUDE_CONFIG_DIR".into(),
                root.to_string_lossy().into_owned(),
            ),
            (
                "VK_FIXTURE_TRACE".into(),
                trace.to_string_lossy().into_owned(),
            ),
        ]));
        for expected_probes in [1, 1, 2] {
            if expected_probes == 2 {
                std::fs::write(&settings, r#"{"model":"local-alias-b","modelSettings":{"canonical-native-model":{"effortLevel":"xhigh"}}}"#).unwrap();
            }
            let mut stream = executor
                .discover_options(Some(directory.path()), None)
                .await
                .unwrap();
            let mut state = serde_json::json!({"options":null});
            while let Some(patch) = stream.next().await {
                json_patch::patch(&mut state, &patch).unwrap();
            }
            let options: crate::executor_discovery::ExecutorDiscoveredOptions =
                serde_json::from_value(state["options"].clone()).unwrap();
            assert_eq!(
                options.model_selector.catalog_source,
                Some(ModelCatalogSource::Native)
            );
            assert_eq!(options.model_selector.models[0].id, "native-alias");
            assert_eq!(options.model_selector.models[0].reasoning_options.len(), 2);
            assert!(options.error.is_none());
            // The fixture uses an opaque command override, so its catalog is
            // native but unrelated host defaults must not be projected.
            assert!(options.model_selector.native_defaults.is_none());
            let probes: Vec<serde_json::Value> = std::fs::read_to_string(&trace)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(probes.len(), expected_probes);
            assert!(probes.iter().all(|probe| {
                probe["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|arg| arg == "--no-session-persistence")
            }));
            assert!(
                probes
                    .iter()
                    .all(|probe| probe["request"]["request"]["subtype"] == "initialize")
            );
        }
    }

    #[tokio::test]
    async fn missing_claude_discovery_returns_builtin_source_with_visible_failure() {
        // Resolving a missing CLI may refresh process PATH. Keep it from
        // changing the cache authority during the neighboring fixture test.
        let _guard = DISCOVERY_FIXTURE_LOCK.lock().await;
        let mut executor = test_executor(None);
        executor.cmd.base_command_override =
            Some("missing-vibe-studio-discovery-fixture-executable".into());
        let mut stream = executor.discover_options(None, None).await.unwrap();
        let mut state = serde_json::json!({"options":null});
        while let Some(patch) = stream.next().await {
            json_patch::patch(&mut state, &patch).unwrap();
        }
        let options: crate::executor_discovery::ExecutorDiscoveredOptions =
            serde_json::from_value(state["options"].clone()).unwrap();
        assert_eq!(
            options.model_selector.catalog_source,
            Some(ModelCatalogSource::Builtin)
        );
        assert!(options.error.is_some());
        assert!(!options.loading_models);
        assert!(
            options
                .model_selector
                .models
                .iter()
                .all(|model| model.reasoning_options.is_empty())
        );
    }

    #[test]
    fn native_catalog_overrides_family_name_assumptions() {
        let init = serde_json::json!({"models":[
            {"value":"haiku", "resolvedModel":"claude-sonnet-5", "supportsEffort":true, "supportedEffortLevels":["low","max"], "supportsFastMode":false},
            {"value":"opus", "supportsEffort":false},
            {"value":"custom-model"}
        ]});
        let models = models_from_initialize(&init);
        assert_eq!(models[0].reasoning_options.len(), 2);
        assert_eq!(
            models[0]
                .capabilities
                .as_ref()
                .unwrap()
                .resolved_model
                .as_deref(),
            Some("claude-sonnet-5")
        );
        assert!(models[1].reasoning_options.is_empty());
        assert_eq!(
            models[2].capabilities.as_ref().unwrap().reasoning,
            crate::model_selector::CapabilitySupport::Unknown
        );
        let mut agent = test_executor(None);
        agent.model = Some("haiku".into());
        agent.effort = Some(ClaudeEffort::Max);
        assert!(validate_requested_capabilities(&agent, &init).is_ok());
        agent.model = Some("opus".into());
        assert!(validate_requested_capabilities(&agent, &init).is_err());
        agent.model = Some("custom-model".into());
        assert!(validate_requested_capabilities(&agent, &init).is_err());
    }

    #[test]
    fn invalid_effort_and_conflicting_native_permissions_are_rejected() {
        let mut agent = test_executor(None);
        let invalid: ExecutorConfig = serde_json::from_value(
            serde_json::json!({"executor":"CLAUDE_CODE","reasoning_id":"ultracode"}),
        )
        .unwrap();
        assert!(agent.apply_direct_overrides(&invalid).is_err());
        let conflict: ExecutorConfig = serde_json::from_value(serde_json::json!({"executor":"CLAUDE_CODE","permission_policy":"AUTO","provider_options":{"provider":"claude_code","permission_mode":"auto"}})).unwrap();
        assert!(agent.apply_direct_overrides(&conflict).is_err());
    }

    #[test]
    fn help_flag_does_not_invent_model_scoped_effort() {
        let options = default_discovered_options(true);
        let opus = options
            .model_selector
            .models
            .iter()
            .find(|model| model.id == "opus")
            .expect("opus model should exist");
        let haiku = options
            .model_selector
            .models
            .iter()
            .find(|model| model.id == "haiku")
            .expect("haiku model should exist");

        assert!(opus.reasoning_options.is_empty());
        assert!(haiku.reasoning_options.is_empty());
    }

    #[test]
    fn test_claude_json_parsing() {
        let system_json =
            r#"{"type":"system","subtype":"init","session_id":"abc123","model":"claude-sonnet-4"}"#;
        let parsed: ClaudeJson = serde_json::from_str(system_json).unwrap();

        // System messages no longer extract session_id
        assert_eq!(ClaudeLogProcessor::extract_session_id(&parsed), None);

        let entries = normalize(&parsed, "");
        assert_eq!(entries.len(), 0);

        let assistant_json = r#"
        {"type":"assistant","message":{"type":"message","role":"assistant","model":"claude-sonnet-4-20250514","content":[{"type":"text","text":"Hi! I'm Claude Code."}]}}"#;
        let parsed: ClaudeJson = serde_json::from_str(assistant_json).unwrap();
        let entries = normalize(&parsed, "");

        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].entry_type,
            NormalizedEntryType::AssistantMessage
        ));
        assert_eq!(entries[0].content, "Hi! I'm Claude Code.");
    }

    #[test]
    fn test_assistant_message_parsing() {
        let assistant_json = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello world"}]},"session_id":"abc123"}"#;
        let parsed: ClaudeJson = serde_json::from_str(assistant_json).unwrap();

        let entries = normalize(&parsed, "");
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].entry_type,
            NormalizedEntryType::AssistantMessage
        ));
        assert_eq!(entries[0].content, "Hello world");
    }

    #[test]
    fn test_result_message_emits_final_text_if_not_seen() {
        let result_json = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":6059,"result":"Final result"}"#;
        let parsed: ClaudeJson = serde_json::from_str(result_json).unwrap();

        let entries = normalize(&parsed, "");
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].entry_type,
            NormalizedEntryType::AssistantMessage
        ));
        assert_eq!(entries[0].content, "Final result");
    }

    #[test]
    fn test_thinking_content() {
        let thinking_json = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Let me think about this..."}]}}"#;
        let parsed: ClaudeJson = serde_json::from_str(thinking_json).unwrap();

        let entries = normalize(&parsed, "");
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].entry_type,
            NormalizedEntryType::Thinking
        ));
        assert_eq!(entries[0].content, "Let me think about this...");
    }

    #[test]
    fn test_claude_hook_system_events_are_hidden() {
        let hook_started_json = r#"{"type":"system","subtype":"hook_started"}"#;
        let parsed: ClaudeJson = serde_json::from_str(hook_started_json).unwrap();
        let entries = normalize(&parsed, "");
        assert!(entries.is_empty());

        let hook_response_json = r#"{"type":"system","subtype":"hook_response"}"#;
        let parsed: ClaudeJson = serde_json::from_str(hook_response_json).unwrap();
        let entries = normalize(&parsed, "");
        assert!(entries.is_empty());

        let thinking_tokens_json = r#"{"type":"system","subtype":"thinking_tokens"}"#;
        let parsed: ClaudeJson = serde_json::from_str(thinking_tokens_json).unwrap();
        let entries = normalize(&parsed, "");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_claude_api_retry_system_event_is_visible() {
        let api_retry_json = r#"{"type":"system","subtype":"api_retry","message":"rate limited","attempt":2,"delayMs":5000}"#;
        let parsed: ClaudeJson = serde_json::from_str(api_retry_json).unwrap();
        let entries = normalize(&parsed, "");
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].entry_type,
            NormalizedEntryType::SystemMessage
        ));
        assert_eq!(
            entries[0].content,
            "Claude Code retrying API request: rate limited (attempt 2, next in 5000ms)"
        );
    }

    #[test]
    fn test_claude_rate_limit_event_is_visible() {
        let rate_limit_json =
            r#"{"type":"rate_limit_event","rate_limit_info":{"message":"limit reached"}}"#;
        let parsed: ClaudeJson = serde_json::from_str(rate_limit_json).unwrap();
        let entries = normalize(&parsed, "");
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].entry_type,
            NormalizedEntryType::SystemMessage
        ));
        assert_eq!(
            entries[0].content,
            "Claude Code rate limit notice: limit reached"
        );
    }

    #[test]
    fn test_claude_requesting_status_renders_as_loading() {
        let status_json = r#"{"type":"system","subtype":"status","status":"requesting"}"#;
        let parsed: ClaudeJson = serde_json::from_str(status_json).unwrap();

        let entries = normalize(&parsed, "");
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].entry_type,
            NormalizedEntryType::Loading
        ));
    }

    #[test]
    fn test_todo_tool_empty_list() {
        // Test TodoWrite with empty todo list
        let empty_data = ClaudeToolData::TodoWrite { todos: vec![] };

        let action_type =
            ClaudeLogProcessor::extract_action_type(&empty_data, "/tmp/test-worktree");
        let result = ClaudeLogProcessor::generate_concise_content(
            &empty_data,
            &action_type,
            "/tmp/test-worktree",
        );

        assert_eq!(result, "TODO list updated");
    }

    #[test]
    fn test_glob_tool_content_extraction() {
        // Test Glob with pattern and path
        let glob_data = ClaudeToolData::Glob {
            pattern: "**/*.ts".to_string(),
            path: Some("/tmp/test-worktree/src".to_string()),
            limit: None,
        };

        let action_type = ClaudeLogProcessor::extract_action_type(&glob_data, "/tmp/test-worktree");
        let result = ClaudeLogProcessor::generate_concise_content(
            &glob_data,
            &action_type,
            "/tmp/test-worktree",
        );

        assert_eq!(result, "**/*.ts");
    }

    #[test]
    fn test_glob_tool_pattern_only() {
        // Test Glob with pattern only
        let glob_data = ClaudeToolData::Glob {
            pattern: "*.js".to_string(),
            path: None,
            limit: None,
        };

        let action_type = ClaudeLogProcessor::extract_action_type(&glob_data, "/tmp/test-worktree");
        let result = ClaudeLogProcessor::generate_concise_content(
            &glob_data,
            &action_type,
            "/tmp/test-worktree",
        );

        assert_eq!(result, "*.js");
    }

    #[test]
    fn test_ls_tool_content_extraction() {
        // Test LS with path
        let ls_data = ClaudeToolData::LS {
            path: "/tmp/test-worktree/components".to_string(),
        };

        let action_type = ClaudeLogProcessor::extract_action_type(&ls_data, "/tmp/test-worktree");
        let result = ClaudeLogProcessor::generate_concise_content(
            &ls_data,
            &action_type,
            "/tmp/test-worktree",
        );

        assert_eq!(result, "List directory: components");
    }

    #[test]
    fn test_path_relative_conversion() {
        // Test with relative path (should remain unchanged)
        let relative_result = make_path_relative("src/main.rs", "/tmp/test-worktree");
        assert_eq!(relative_result, "src/main.rs");

        // Test with absolute path (should become relative if possible)
        let test_worktree = "/tmp/test-worktree";
        let absolute_path = format!("{test_worktree}/src/main.rs");
        let absolute_result = make_path_relative(&absolute_path, test_worktree);
        assert_eq!(absolute_result, "src/main.rs");
    }

    #[tokio::test]
    async fn test_streaming_patch_generation() {
        use std::sync::Arc;

        use workspace_utils::msg_store::MsgStore;

        let executor = ClaudeCode {
            claude_code_router: Some(false),
            plan: None,
            approvals: None,
            model: None,
            effort: None,
            fast_mode: None,
            ultracode: None,
            native_permission_mode: None,
            agent: None,
            append_prompt: AppendPrompt::default(),
            dangerously_skip_permissions: None,
            cmd: crate::command::CmdOverrides {
                base_command_override: None,
                additional_params: None,
                env: None,
            },
            approvals_service: None,
            disable_api_key: None,
        };
        let msg_store = Arc::new(MsgStore::new());
        let current_dir = std::path::PathBuf::from("/tmp/test-worktree");

        // Push some test messages
        msg_store.push_stdout(
            r#"{"type":"system","subtype":"init","session_id":"test123"}"#.to_string(),
        );
        msg_store.push_stdout(r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello"}]}}"#.to_string());
        msg_store.push_finished();

        // Start normalization (this spawns async task)
        executor.normalize_logs(msg_store.clone(), &current_dir);

        // Give some time for async processing
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Check that the history now contains patch messages
        let history = msg_store.get_history();
        let patch_count = history
            .iter()
            .filter(|msg| matches!(msg, workspace_utils::log_msg::LogMsg::JsonPatch(_)))
            .count();
        assert!(
            patch_count > 0,
            "Expected JsonPatch messages to be generated from streaming processing"
        );
    }

    #[test]
    fn test_session_id_extraction() {
        let system_json = r#"{"type":"system","session_id":"test-session-123"}"#;
        let parsed: ClaudeJson = serde_json::from_str(system_json).unwrap();

        // System messages no longer extract session_id
        assert_eq!(ClaudeLogProcessor::extract_session_id(&parsed), None);

        let tool_use_json = r#"{"id":"t1","type":"tool_use","tool_name":"read","input":{},"session_id":"another-session"}"#;
        let parsed_tool: ClaudeJson = serde_json::from_str(tool_use_json).unwrap();

        assert_eq!(
            ClaudeLogProcessor::extract_session_id(&parsed_tool),
            Some("another-session".to_string())
        );
    }

    #[test]
    fn test_tool_aliases_create_file_and_edit_file() {
        // Alternate tool names deserialize into the canonical file operations.
        let assistant_with_create = r#"{
            "type":"assistant",
            "message":{
                "role":"assistant",
                "content":[
                    {"type":"tool_use","id":"t1","name":"create_file","input":{"path":"/tmp/work/src/new.txt","content":"hello"}}
                ]
            }
        }"#;
        let parsed: ClaudeJson = serde_json::from_str(assistant_with_create).unwrap();
        let entries = normalize(&parsed, "/tmp/work");
        assert_eq!(entries.len(), 1);
        match &entries[0].entry_type {
            NormalizedEntryType::ToolUse { action_type, .. } => match action_type {
                ActionType::FileEdit { path, .. } => assert_eq!(path, "src/new.txt"),
                other => panic!("Expected FileEdit, got {other:?}"),
            },
            other => panic!("Expected ToolUse, got {other:?}"),
        }

        // Alternate edit names deserialize into Edit with path/old_str/new_str aliases.
        let assistant_with_edit = r#"{
            "type":"assistant",
            "message":{
                "role":"assistant",
                "content":[
                    {"type":"tool_use","id":"t2","name":"edit_file","input":{"path":"/tmp/work/README.md","old_str":"foo","new_str":"bar"}}
                ]
            }
        }"#;
        let parsed_edit: ClaudeJson = serde_json::from_str(assistant_with_edit).unwrap();
        let entries = normalize(&parsed_edit, "/tmp/work");
        assert_eq!(entries.len(), 1);
        match &entries[0].entry_type {
            NormalizedEntryType::ToolUse { action_type, .. } => match action_type {
                ActionType::FileEdit { path, .. } => assert_eq!(path, "README.md"),
                other => panic!("Expected FileEdit, got {other:?}"),
            },
            other => panic!("Expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn test_bash_and_task_content() {
        // Bash with alias field cmd
        let bash_json = r#"{
            "type":"assistant",
            "message":{
                "role":"assistant",
                "content":[
                    {"type":"tool_use","id":"t1","name":"bash","input":{"cmd":"echo hello"}}
                ]
            }
        }"#;
        let parsed: ClaudeJson = serde_json::from_str(bash_json).unwrap();
        let entries = normalize(&parsed, "/tmp/work");
        assert_eq!(entries.len(), 1);
        // Content should display the command
        assert_eq!(entries[0].content, "echo hello");

        // Task content should include description/prompt wrapped in backticks
        let task_json = r#"{
            "type":"assistant",
            "message":{
                "role":"assistant",
                "content":[
                    {"type":"tool_use","id":"t2","name":"task","input":{"subagent_type":"Task","prompt":"Add header to README"}}
                ]
            }
        }"#;
        let parsed: ClaudeJson = serde_json::from_str(task_json).unwrap();
        let entries = normalize(&parsed, "/tmp/work");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "Task: `Add header to README`");
    }

    #[test]
    fn test_task_description_or_prompt_backticks() {
        // When description present, use it
        let with_desc = r#"{
            "type":"assistant",
            "message":{
                "role":"assistant",
                "content":[
                    {"type":"tool_use","id":"t3","name":"Task","input":{
                        "subagent_type":"Task",
                        "prompt":"Fallback prompt",
                        "description":"Primary description"
                    }}
                ]
            }
        }"#;
        let parsed: ClaudeJson = serde_json::from_str(with_desc).unwrap();
        let entries = normalize(&parsed, "/tmp/work");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "Task: `Primary description`");

        // When description missing, fall back to prompt
        let no_desc = r#"{
            "type":"assistant",
            "message":{
                "role":"assistant",
                "content":[
                    {"type":"tool_use","id":"t4","name":"Task","input":{
                        "subagent_type":"Task",
                        "prompt":"Only prompt"
                    }}
                ]
            }
        }"#;
        let parsed: ClaudeJson = serde_json::from_str(no_desc).unwrap();
        let entries = normalize(&parsed, "/tmp/work");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "Task: `Only prompt`");
    }

    #[test]
    fn test_tool_result_parsing_ignored() {
        let tool_result_json = r#"{"type":"tool_result","result":"File content here","is_error":false,"session_id":"test123"}"#;
        let parsed: ClaudeJson = serde_json::from_str(tool_result_json).unwrap();

        // Test session ID extraction from ToolResult still works
        assert_eq!(
            ClaudeLogProcessor::extract_session_id(&parsed),
            Some("test123".to_string())
        );

        // ToolResult messages should be ignored (produce no entries) until proper support is added
        let entries = normalize(&parsed, "");
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_content_item_tool_result_ignored() {
        let assistant_with_tool_result = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_result","tool_use_id":"tool_123","content":"Operation completed","is_error":false}]}}"#;
        let parsed: ClaudeJson = serde_json::from_str(assistant_with_tool_result).unwrap();

        // ToolResult content items should be ignored (produce no entries) until proper support is added
        let entries = normalize(&parsed, "");
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_api_key_source_warning() {
        // Test with ANTHROPIC_API_KEY - should generate warning
        let system_with_env_key = r#"{"type":"system","subtype":"init","apiKeySource":"ANTHROPIC_API_KEY","session_id":"test123"}"#;
        let parsed: ClaudeJson = serde_json::from_str(system_with_env_key).unwrap();
        let entries = normalize(&parsed, "");

        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].entry_type,
            NormalizedEntryType::ErrorMessage {
                error_type: NormalizedEntryError::Other,
            },
        ));
        assert_eq!(
            entries[0].content,
            "Claude Code + ANTHROPIC_API_KEY detected. Usage will be billed via Anthropic pay-as-you-go instead of your Claude subscription. If this is unintended, please select the `disable_api_key` checkbox in the conding-agent-configurations settings page."
        );

        // Test with managed API key source - should not generate warning
        let system_with_managed_key = r#"{"type":"system","subtype":"init","apiKeySource":"/login managed key","session_id":"test123"}"#;
        let parsed_managed: ClaudeJson = serde_json::from_str(system_with_managed_key).unwrap();
        let entries_managed = normalize(&parsed_managed, "");

        assert_eq!(entries_managed.len(), 0); // No warning for managed key

        // Test with other apiKeySource values - should not generate warning
        let system_other_key = r#"{"type":"system","subtype":"init","apiKeySource":"OTHER_KEY","session_id":"test123"}"#;
        let parsed_other: ClaudeJson = serde_json::from_str(system_other_key).unwrap();
        let entries_other = normalize(&parsed_other, "");

        assert_eq!(entries_other.len(), 0); // No warning for other keys

        // Test with missing apiKeySource - should not generate warning
        let system_no_key = r#"{"type":"system","subtype":"init","session_id":"test123"}"#;
        let parsed_no_key: ClaudeJson = serde_json::from_str(system_no_key).unwrap();
        let entries_no_key = normalize(&parsed_no_key, "");

        assert_eq!(entries_no_key.len(), 0); // No warning when field is missing
    }

    #[test]
    fn test_mixed_content_with_thinking_ignores_tool_result() {
        let complex_assistant_json = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"I need to read the file first"},{"type":"text","text":"I'll help you with that"},{"type":"tool_result","tool_use_id":"tool_789","content":"Success","is_error":false}]}}"#;
        let parsed: ClaudeJson = serde_json::from_str(complex_assistant_json).unwrap();

        let entries = normalize(&parsed, "");
        // Only thinking and text entries should be processed, tool_result ignored
        assert_eq!(entries.len(), 2);

        // Check thinking entry
        assert!(matches!(
            entries[0].entry_type,
            NormalizedEntryType::Thinking
        ));
        assert_eq!(entries[0].content, "I need to read the file first");

        // Check assistant message
        assert!(matches!(
            entries[1].entry_type,
            NormalizedEntryType::AssistantMessage
        ));
        assert_eq!(entries[1].content, "I'll help you with that");

        // ToolResult entry is ignored - no third entry
    }

    #[test]
    fn test_control_request_with_permission_suggestions() {
        let control_request_json = r#"{"type":"control_request","request_id":"f559d907-b139-475b-addd-79c05591eb99","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"./gradlew :web:testApi","timeout":300000,"description":"Run API tests"},"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"./gradlew :web:testApi:"}],"behavior":"allow","destination":"localSettings"}],"tool_use_id":"toolu_014PR3WXsJfiftSCbjcjEbeM"}}"#;
        let parsed: ClaudeJson = serde_json::from_str(control_request_json).unwrap();
        assert!(matches!(parsed, ClaudeJson::ControlRequest { .. }));
    }
}
