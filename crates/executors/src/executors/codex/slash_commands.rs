use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use codex_app_server_protocol::{
    ConfigEdit, JSONRPCNotification, MergeStrategy, RateLimitResetCreditStatus,
    RateLimitResetCreditsSummary, ReviewTarget, SkillScope, SkillsListResponse, ThreadGoal,
    ThreadGoalSetParams, ThreadGoalStatus, ThreadSettingsUpdateParams,
};
use codex_protocol::{
    config_types::{Personality, ServiceTier},
    protocol::{AgentMessageEvent, ErrorEvent, EventMsg},
};
use serde_json::json;

use super::{
    Codex,
    client::{AppServerClient, LogWriter},
    codex_home, resolve_model, resume_params_from,
};
use crate::{
    actions::SelectedSkill,
    env::ExecutionEnv,
    executors::{
        ExecutorError, ExecutorExitResult, SlashCommandDescription, SlashCommandSource,
        SlashCommandSupportLevel, SpawnedChild,
        utils::{SlashCommandCall, parse_slash_command},
    },
    stdout_dup::spawn_local_output_process,
    workflow_mcp::{ScopedWorkflowMcp, WorkflowMcpReadiness},
};

const CODEX_INIT_PROMPT: &str = include_str!("init_prompt.md");
const DEFAULT_PROJECT_DOC_FILENAME: &str = "AGENTS.md";

#[derive(Debug, Clone)]
pub enum CodexSlashCommand {
    Init,
    Compact { instructions: Option<String> },
    Review { instructions: Option<String> },
    Status,
    Mcp,
    Skills,
    Fast { enable: Option<bool>, status: bool },
    Goal(CodexGoalCommand),
    Personality(CodexPersonalityCommand),
}

#[derive(Debug, Clone)]
pub enum CodexGoalCommand {
    Show,
    Set { objective: String },
    Clear,
    Pause,
    Resume,
}

#[derive(Debug, Clone)]
pub enum CodexPersonalityCommand {
    Show,
    Set(Personality),
    Invalid { value: String },
}

impl CodexSlashCommand {
    pub fn parse(prompt: &str) -> Option<Self> {
        let cmd: SlashCommandCall<'_> = parse_slash_command(prompt)?;
        match cmd.name.as_str() {
            "init" => Some(Self::Init),
            "compact" => Some(Self::Compact {
                instructions: if cmd.arguments.is_empty() {
                    None
                } else {
                    Some(cmd.arguments.to_string())
                },
            }),
            "review" => Some(Self::Review {
                instructions: if cmd.arguments.is_empty() {
                    None
                } else {
                    Some(cmd.arguments.to_string())
                },
            }),
            "status" => Some(Self::Status),
            "mcp" => Some(Self::Mcp),
            "skills" => Some(Self::Skills),
            "fast" => Some(Self::Fast {
                status: matches!(cmd.arguments.trim(), "status"),
                enable: match cmd.arguments.trim() {
                    "on" | "true" | "1" | "yes" | "enable" => Some(true),
                    "off" | "false" | "0" | "no" | "disable" => Some(false),
                    _ => None,
                },
            }),
            "goal" => Some(Self::Goal(parse_goal_command(cmd.arguments))),
            "personality" => Some(Self::Personality(parse_personality_command(cmd.arguments))),
            _ => None,
        }
    }
}

fn validate_workflow_slash_command(
    command: &CodexSlashCommand,
    workflow_scoped: bool,
) -> Result<(), ExecutorError> {
    if workflow_scoped && matches!(command, CodexSlashCommand::Fast { status: false, .. }) {
        return Err(ExecutorError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "The workflow main session's runtime configuration is fixed; /fast changes are not allowed. Use /fast status to read the current setting.",
        )));
    }
    Ok(())
}

fn workflow_diagnostic_session_id(session_id: Option<&str>, workflow_scoped: bool) -> Option<&str> {
    // A workflow diagnostic must load MCP into the same native continuation,
    // not register a replacement thread. Ordinary diagnostics need no thread.
    if workflow_scoped { session_id } else { None }
}

fn parse_goal_command(arguments: &str) -> CodexGoalCommand {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return CodexGoalCommand::Show;
    }

    match trimmed.to_ascii_lowercase().as_str() {
        "clear" => CodexGoalCommand::Clear,
        "pause" => CodexGoalCommand::Pause,
        "resume" => CodexGoalCommand::Resume,
        _ => CodexGoalCommand::Set {
            objective: trimmed.to_string(),
        },
    }
}

fn parse_personality_command(arguments: &str) -> CodexPersonalityCommand {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return CodexPersonalityCommand::Show;
    }

    match trimmed.to_ascii_lowercase().as_str() {
        "friendly" => CodexPersonalityCommand::Set(Personality::Friendly),
        "pragmatic" => CodexPersonalityCommand::Set(Personality::Pragmatic),
        "none" => CodexPersonalityCommand::Set(Personality::None),
        _ => CodexPersonalityCommand::Invalid {
            value: trimmed.to_string(),
        },
    }
}

#[derive(Clone, Copy)]
struct CodexSlashCommandCatalogEntry {
    name: &'static str,
    description: &'static str,
    supported: bool,
}

// Synced from the Codex TUI slash command catalog pinned by
// codex-app-server-protocol. Only entries that vibe-kanban can execute through
// Codex app-server are advertised to the UI.
const CODEX_SLASH_COMMAND_CATALOG: &[CodexSlashCommandCatalogEntry] = &[
    CodexSlashCommandCatalogEntry {
        name: "compact",
        description: "summarize conversation to prevent hitting the context limit",
        supported: true,
    },
    CodexSlashCommandCatalogEntry {
        name: "review",
        description: "review my current changes and find issues",
        supported: true,
    },
    CodexSlashCommandCatalogEntry {
        name: "init",
        description: "create an AGENTS.md file with instructions for Codex",
        supported: true,
    },
    CodexSlashCommandCatalogEntry {
        name: "status",
        description: "show current session configuration and token usage",
        supported: true,
    },
    CodexSlashCommandCatalogEntry {
        name: "mcp",
        description: "list configured MCP tools; use /mcp verbose for details",
        supported: true,
    },
    CodexSlashCommandCatalogEntry {
        name: "skills",
        description: "use skills to improve how Codex performs specific tasks",
        supported: true,
    },
    CodexSlashCommandCatalogEntry {
        name: "fast",
        description: "toggle fast mode for highest speed inference (2x plan usage)",
        supported: true,
    },
    CodexSlashCommandCatalogEntry {
        name: "model",
        description: "choose what model and reasoning effort to use",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "ide",
        description: "include current selection, open files, and other context from your IDE",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "permissions",
        description: "choose what Codex is allowed to do",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "keymap",
        description: "remap TUI shortcuts",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "vim",
        description: "toggle Vim mode for the composer",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "setup-default-sandbox",
        description: "set up elevated agent sandbox",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "sandbox-add-read-dir",
        description: "let sandbox read a directory: /sandbox-add-read-dir <absolute_path>",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "experimental",
        description: "toggle experimental features",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "approve",
        description: "approve one retry of a recent auto-review denial",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "memories",
        description: "configure memory use and generation",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "hooks",
        description: "view and manage lifecycle hooks",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "rename",
        description: "rename the current thread",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "new",
        description: "start a new chat during a conversation",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "archive",
        description: "archive this session and exit",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "resume",
        description: "resume a saved chat",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "fork",
        description: "fork the current chat",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "app",
        description: "continue this session in Codex Desktop",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "plan",
        description: "switch to Plan mode",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "goal",
        description: "set or view the goal for a long-running task",
        supported: true,
    },
    CodexSlashCommandCatalogEntry {
        name: "agent",
        description: "switch the active agent thread",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "side",
        description: "start a side conversation in an ephemeral fork",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "btw",
        description: "start a side conversation in an ephemeral fork",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "copy",
        description: "copy last response as markdown",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "raw",
        description: "toggle raw scrollback mode for copy-friendly terminal selection",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "diff",
        description: "show git diff (including untracked files)",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "mention",
        description: "mention a file",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "debug-config",
        description: "show config layers and requirement sources for debugging",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "title",
        description: "configure which items appear in the terminal title",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "statusline",
        description: "configure which items appear in the status line",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "theme",
        description: "choose a syntax highlighting theme",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "pets",
        description: "choose or hide the terminal pet",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "apps",
        description: "manage apps",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "plugins",
        description: "browse plugins",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "logout",
        description: "log out of Codex",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "quit",
        description: "exit Codex",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "exit",
        description: "exit Codex",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "feedback",
        description: "send logs to maintainers",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "rollout",
        description: "print the rollout file path",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "ps",
        description: "list background terminals",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "stop",
        description: "stop all background terminals",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "clear",
        description: "clear the terminal and start a new chat",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "personality",
        description: "choose a communication style for Codex",
        supported: true,
    },
    CodexSlashCommandCatalogEntry {
        name: "realtime",
        description: "toggle realtime voice mode (experimental)",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "settings",
        description: "configure realtime microphone/speaker",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "test-approval",
        description: "test approval request",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "subagents",
        description: "switch the active agent thread",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "debug-m-drop",
        description: "DO NOT USE",
        supported: false,
    },
    CodexSlashCommandCatalogEntry {
        name: "debug-m-update",
        description: "DO NOT USE",
        supported: false,
    },
];

pub(super) fn supported_slash_commands() -> Vec<SlashCommandDescription> {
    CODEX_SLASH_COMMAND_CATALOG
        .iter()
        .filter(|command| command.supported)
        .map(|command| SlashCommandDescription {
            name: command.name.to_string(),
            description: Some(command.description.to_string()),
            source: Some(SlashCommandSource::Builtin),
            support_level: Some(SlashCommandSupportLevel::Product),
        })
        .collect()
}

pub(super) fn skill_slash_commands(
    skills: &[crate::executor_discovery::CodexSkillDescription],
) -> Vec<SlashCommandDescription> {
    skills
        .iter()
        .filter(|skill| skill.enabled)
        .map(|skill| SlashCommandDescription {
            name: skill.name.clone(),
            description: skill
                .short_description
                .clone()
                .or_else(|| Some(skill.description.clone())),
            source: Some(SlashCommandSource::Skill),
            support_level: Some(SlashCommandSupportLevel::Skill),
        })
        .collect()
}

impl Codex {
    pub async fn spawn_slash_command(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        selected_skills: Vec<SelectedSkill>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        if let Some(command) = CodexSlashCommand::parse(prompt) {
            let workflow_scoped = ScopedWorkflowMcp::from_execution_env(env)?.is_some();
            return match command {
                CodexSlashCommand::Init => {
                    let init_target = current_dir.join(DEFAULT_PROJECT_DOC_FILENAME);
                    if init_target.exists() {
                        let message = format!(
                            "`{DEFAULT_PROJECT_DOC_FILENAME}` already exists. Skipping `/init` to avoid overwriting it."
                        );
                        self.return_static_reply(current_dir, Ok(message)).await
                    } else {
                        self.spawn_agent_with_prompt(
                            current_dir,
                            CODEX_INIT_PROMPT,
                            session_id,
                            vec![],
                            env,
                        )
                        .await
                    }
                }
                CodexSlashCommand::Compact { .. } => match session_id {
                    Some(_) => {
                        self.handle_app_server_slash_command(current_dir, command, session_id, env)
                            .await
                    }
                    None => {
                        self.return_static_reply(
                            current_dir,
                            Ok("_No active session to compact._".to_string()),
                        )
                        .await
                    }
                },
                CodexSlashCommand::Review { instructions } => {
                    let review_target = match instructions {
                        Some(instructions) => ReviewTarget::Custom { instructions },
                        None => ReviewTarget::UncommittedChanges,
                    };
                    self.spawn_review_target(current_dir, review_target, session_id, env)
                        .await
                }
                CodexSlashCommand::Status => {
                    self.handle_app_server_slash_command(current_dir, command, session_id, env)
                        .await
                }
                CodexSlashCommand::Mcp => {
                    self.handle_app_server_slash_command(
                        current_dir,
                        command,
                        workflow_diagnostic_session_id(session_id, workflow_scoped),
                        env,
                    )
                    .await
                }
                CodexSlashCommand::Skills => {
                    self.handle_app_server_slash_command(
                        current_dir,
                        command,
                        workflow_diagnostic_session_id(session_id, workflow_scoped),
                        env,
                    )
                    .await
                }
                CodexSlashCommand::Fast { .. } => {
                    self.handle_app_server_slash_command(current_dir, command, session_id, env)
                        .await
                }
                CodexSlashCommand::Goal(_) => match session_id {
                    Some(_) => {
                        self.handle_app_server_slash_command(current_dir, command, session_id, env)
                            .await
                    }
                    None => {
                        self.return_static_reply(
                            current_dir,
                            Ok("_No active Codex session to manage a goal._".to_string()),
                        )
                        .await
                    }
                },
                CodexSlashCommand::Personality(_) => match session_id {
                    Some(_) => {
                        self.handle_app_server_slash_command(current_dir, command, session_id, env)
                            .await
                    }
                    None => {
                        self.return_static_reply(
                            current_dir,
                            Ok("_No active Codex session to change personality._".to_string()),
                        )
                        .await
                    }
                },
            };
        }

        self.spawn_agent_with_prompt(current_dir, prompt, session_id, selected_skills, env)
            .await
    }

    async fn spawn_agent_with_prompt(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        selected_skills: Vec<SelectedSkill>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let command_parts = match session_id {
            Some(_) => self.build_command_builder()?.build_follow_up(&[])?,
            None => self.build_command_builder()?.build_initial()?,
        };
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let action = super::CodexSessionAction::Chat {
            prompt: combined_prompt,
            selected_skills,
        };
        self.spawn_inner(current_dir, command_parts, action, session_id, env)
            .await
    }

    // Handle slash commands that require interaction with the app server
    async fn handle_app_server_slash_command(
        &self,
        current_dir: &Path,
        command: CodexSlashCommand,
        session_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        // Validate before starting a CLI or issuing global config/batchWrite.
        // Native slash commands must not bypass the captured Main Session tier.
        validate_workflow_slash_command(
            &command,
            ScopedWorkflowMcp::from_execution_env(env)?.is_some(),
        )?;
        let command_parts = self.build_command_builder()?.build_initial()?;
        let session_id = session_id.map(|s| s.to_string());
        let (_, session_fast) = resolve_model(self.model.as_deref());
        let thread_start_params = self.build_thread_start_params(current_dir);
        let current_dir_path = current_dir.to_path_buf();

        self.spawn_app_server(
            current_dir,
            command_parts,
            env,
            move |client, exit_signal_tx, readiness, cancel, prompt_gate| async move {
                // Scoped commands can resume/compact/change a native thread or
                // wake a goal. Prepare that execution's MCP just as for chat,
                // rather than ignoring the gate on this alternate entrypoint.
                // Ordinary diagnostic commands retain their no-thread behavior.
                let workflow_thread_id = if readiness.is_some() {
                    let (thread_id, model) = match session_id.as_ref() {
                        Some(session_id) => {
                            let response = client
                                .thread_resume(resume_params_from(
                                    session_id.clone(),
                                    thread_start_params.clone(),
                                ))
                                .await?;
                            (response.thread.id, response.model)
                        }
                        None => {
                            let response = client.thread_start(thread_start_params.clone()).await?;
                            (response.thread.id, response.model)
                        }
                    };
                    client.set_resolved_model(model);
                    client.register_session(&thread_id).await?;
                    Some(thread_id)
                } else {
                    None
                };
                WorkflowMcpReadiness::wait_optional(readiness, &cancel).await?;

                match command {
                    CodexSlashCommand::Init => {
                        return Err(ExecutorError::Io(std::io::Error::other(
                            "Unsupported Codex slash command",
                        )));
                    }
                    CodexSlashCommand::Compact { .. } => {
                        let old_thread_id = session_id.ok_or_else(|| {
                            ExecutorError::Io(std::io::Error::other("No active session to compact"))
                        })?;
                        let thread_id = match workflow_thread_id {
                            Some(thread_id) => thread_id,
                            None => {
                                client
                                    .thread_resume(resume_params_from(
                                        old_thread_id,
                                        thread_start_params,
                                    ))
                                    .await?
                                    .thread
                                    .id
                            }
                        };
                        tracing::debug!("resumed thread for compact, thread_id={thread_id}");
                        client.thread_compact_start(thread_id).await?;
                    }
                    CodexSlashCommand::Review { .. } => {
                        return Err(ExecutorError::Io(std::io::Error::other(
                            "Unsupported Codex slash command",
                        )));
                    }
                    CodexSlashCommand::Status => {
                        let message =
                            fetch_status_message(&client, session_id.as_deref(), session_fast)
                                .await?;
                        log_event_raw(client.log_writer(), message).await?;
                        exit_signal_tx
                            .send_exit_signal(ExecutorExitResult::Success)
                            .await;
                    }
                    CodexSlashCommand::Mcp => {
                        let message = fetch_mcp_status_message(&client).await?;
                        log_event_raw(client.log_writer(), message).await?;
                        exit_signal_tx
                            .send_exit_signal(ExecutorExitResult::Success)
                            .await;
                    }
                    CodexSlashCommand::Skills => {
                        let response = client.skills_list(current_dir_path).await?;
                        let message = format_skills_status(&response);
                        log_event_raw(client.log_writer(), message).await?;
                        exit_signal_tx
                            .send_exit_signal(ExecutorExitResult::Success)
                            .await;
                    }
                    CodexSlashCommand::Fast { enable, status } => {
                        // Read current config to support toggle
                        let current_is_fast = client
                            .config_read(None)
                            .await
                            .ok()
                            .and_then(|r| r.config.service_tier)
                            .map(|t| t == ServiceTier::Fast.request_value())
                            .unwrap_or(false);
                        if status {
                            let message = if current_is_fast || session_fast {
                                "**Fast mode is enabled.**".to_string()
                            } else {
                                "**Fast mode is disabled.**".to_string()
                            };
                            log_event_raw(client.log_writer(), message).await?;
                            exit_signal_tx
                                .send_exit_signal(ExecutorExitResult::Success)
                                .await;
                            return Ok(());
                        }
                        let want_fast = match enable {
                            Some(v) => v,
                            None => !current_is_fast, // toggle
                        };
                        // Persist service_tier to codex config via config/batchWrite
                        let config_value = if want_fast {
                            json!(ServiceTier::Fast.request_value())
                        } else {
                            json!(null)
                        };
                        let _ = client
                            .config_batch_write(vec![ConfigEdit {
                                key_path: "service_tier".to_string(),
                                value: config_value,
                                merge_strategy: MergeStrategy::Replace,
                            }])
                            .await;
                        // Apply the tier to the active session immediately.
                        if let Some(old_thread_id) = session_id {
                            let service_tier = if want_fast {
                                Some(Some(ServiceTier::Fast.request_value().to_string()))
                            } else {
                                Some(None)
                            };
                            let mut resume_params =
                                resume_params_from(old_thread_id, thread_start_params);
                            resume_params.service_tier = service_tier;
                            let _ = client.thread_resume(resume_params).await;
                        }
                        let message = if want_fast {
                            "**Fast mode enabled.** Inference runs at higher speed (2× plan usage)."
                                .to_string()
                        } else {
                            "**Fast mode disabled.**".to_string()
                        };
                        log_event_raw(client.log_writer(), message).await?;
                        exit_signal_tx
                            .send_exit_signal(ExecutorExitResult::Success)
                            .await;
                    }
                    CodexSlashCommand::Goal(goal_command) => {
                        let thread_id = session_id.ok_or_else(|| {
                            ExecutorError::Io(std::io::Error::other(
                                "No active Codex session to manage a goal",
                            ))
                        })?;
                        let message = handle_goal_command(&client, thread_id, goal_command).await?;
                        log_event_raw(client.log_writer(), message).await?;
                        exit_signal_tx
                            .send_exit_signal(ExecutorExitResult::Success)
                            .await;
                    }
                    CodexSlashCommand::Personality(personality_command) => {
                        let thread_id = session_id.ok_or_else(|| {
                            ExecutorError::Io(std::io::Error::other(
                                "No active Codex session to change personality",
                            ))
                        })?;
                        let message =
                            handle_personality_command(&client, thread_id, personality_command)
                                .await?;
                        log_event_raw(client.log_writer(), message).await?;
                        exit_signal_tx
                            .send_exit_signal(ExecutorExitResult::Success)
                            .await;
                    }
                }

                // This path has no initial chat prompt. Keep scoped steering
                // blocked until the requested native command has completed.
                prompt_gate.open();
                Ok(())
            },
        )
        .await
    }

    pub async fn return_static_reply(
        &self,
        current_dir: &Path,
        message: Result<String, String>,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.spawn_static_reply_helper(
            current_dir,
            vec![match message {
                Ok(message) => EventMsg::AgentMessage(AgentMessageEvent {
                    message,
                    phase: None,
                    memory_citation: None,
                }),
                Err(message) => EventMsg::Error(ErrorEvent {
                    message,
                    codex_error_info: None,
                }),
            }],
        )
        .await
    }

    // Helper to spawn a process whose sole purpose is to channel back a static reply
    pub async fn spawn_static_reply_helper(
        &self,
        _current_dir: &Path,
        events: Vec<EventMsg>,
    ) -> Result<SpawnedChild, ExecutorError> {
        let (mut spawned, writer) = spawn_local_output_process()?;
        let log_writer = LogWriter::new(writer);
        let (exit_signal_tx, exit_signal_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let mut exit_result = ExecutorExitResult::Success;
            for event in events {
                if let Err(err) = log_event_notification(&log_writer, event).await {
                    tracing::error!("Failed to emit slash command output: {err}");
                    exit_result = ExecutorExitResult::Failure;
                    break;
                }
            }
            let _ = exit_signal_tx.send(exit_result);
        });

        spawned.exit_signal = Some(exit_signal_rx);
        Ok(spawned)
    }
}

pub async fn log_event_notification(
    log_writer: &LogWriter,
    event: EventMsg,
) -> Result<(), ExecutorError> {
    let event = match event {
        EventMsg::SessionConfigured(mut configured) => {
            configured.initial_messages = None;
            EventMsg::SessionConfigured(configured)
        }
        other => other,
    };
    let notification = JSONRPCNotification {
        method: "codex/event".to_string(),
        params: Some(json!({ "msg": event })),
    };
    let raw = serde_json::to_string(&notification)
        .map_err(|err| ExecutorError::Io(std::io::Error::other(err.to_string())))?;
    log_writer.log_raw(&raw).await
}

pub async fn log_event_raw(log_writer: &LogWriter, message: String) -> Result<(), ExecutorError> {
    log_event_notification(
        log_writer,
        EventMsg::AgentMessage(AgentMessageEvent {
            message,
            phase: None,
            memory_citation: None,
        }),
    )
    .await
}

async fn handle_goal_command(
    client: &AppServerClient,
    thread_id: String,
    command: CodexGoalCommand,
) -> Result<String, ExecutorError> {
    match command {
        CodexGoalCommand::Show => {
            let response = client.thread_goal_get(thread_id).await?;
            Ok(match response.goal {
                Some(goal) => format_goal_status_message(&goal),
                None => "_No active goal for this Codex session._\n\nUsage: `/goal <objective>`"
                    .to_string(),
            })
        }
        CodexGoalCommand::Set { objective } => {
            let response = client
                .thread_goal_set(ThreadGoalSetParams {
                    thread_id,
                    objective: Some(objective),
                    status: Some(ThreadGoalStatus::Active),
                    token_budget: None,
                })
                .await?;
            Ok(format!(
                "**Goal set.**\n\n{}",
                format_goal_details(&response.goal)
            ))
        }
        CodexGoalCommand::Clear => {
            let response = client.thread_goal_clear(thread_id).await?;
            if response.cleared {
                Ok("**Goal cleared.**".to_string())
            } else {
                Ok("_No active goal to clear._".to_string())
            }
        }
        CodexGoalCommand::Pause => {
            let response = client
                .thread_goal_set(ThreadGoalSetParams {
                    thread_id,
                    objective: None,
                    status: Some(ThreadGoalStatus::Paused),
                    token_budget: None,
                })
                .await?;
            Ok(format!(
                "**Goal paused.**\n\n{}",
                format_goal_details(&response.goal)
            ))
        }
        CodexGoalCommand::Resume => {
            let response = client
                .thread_goal_set(ThreadGoalSetParams {
                    thread_id,
                    objective: None,
                    status: Some(ThreadGoalStatus::Active),
                    token_budget: None,
                })
                .await?;
            Ok(format!(
                "**Goal resumed.**\n\n{}",
                format_goal_details(&response.goal)
            ))
        }
    }
}

async fn handle_personality_command(
    client: &AppServerClient,
    thread_id: String,
    command: CodexPersonalityCommand,
) -> Result<String, ExecutorError> {
    match command {
        CodexPersonalityCommand::Show => Ok(personality_usage_message()),
        CodexPersonalityCommand::Invalid { value } => Ok(format!(
            "`{value}` is not a supported Codex personality.\n\n{}",
            personality_usage_message()
        )),
        CodexPersonalityCommand::Set(personality) => {
            client
                .thread_settings_update(ThreadSettingsUpdateParams {
                    thread_id,
                    personality: Some(personality),
                    ..Default::default()
                })
                .await?;
            Ok(format!(
                "**Personality set to `{}`.**",
                personality_label(personality)
            ))
        }
    }
}

fn format_goal_status_message(goal: &ThreadGoal) -> String {
    format!("# Current Goal\n\n{}", format_goal_details(goal))
}

fn format_goal_details(goal: &ThreadGoal) -> String {
    let mut lines = vec![
        format!("- **Objective**: {}", goal.objective),
        format!("- **Status**: `{}`", goal_status_label(goal.status)),
        format!("- **Tokens used**: `{}`", goal.tokens_used),
        format!(
            "- **Time used**: `{}`",
            format_seconds(goal.time_used_seconds)
        ),
    ];
    if let Some(token_budget) = goal.token_budget {
        lines.push(format!("- **Token budget**: `{token_budget}`"));
    }
    lines.join("\n")
}

fn goal_status_label(status: ThreadGoalStatus) -> &'static str {
    match status {
        ThreadGoalStatus::Active => "active",
        ThreadGoalStatus::Paused => "paused",
        ThreadGoalStatus::Blocked => "blocked",
        ThreadGoalStatus::UsageLimited => "usage_limited",
        ThreadGoalStatus::BudgetLimited => "budget_limited",
        ThreadGoalStatus::Complete => "complete",
    }
}

fn format_seconds(seconds: i64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m {seconds}s");
    }
    let hours = minutes / 60;
    let minutes = minutes % 60;
    format!("{hours}h {minutes}m")
}

fn personality_usage_message() -> String {
    "Usage: `/personality friendly`, `/personality pragmatic`, or `/personality none`.".to_string()
}

fn personality_label(personality: Personality) -> &'static str {
    match personality {
        Personality::Friendly => "friendly",
        Personality::Pragmatic => "pragmatic",
        Personality::None => "none",
    }
}

async fn fetch_status_message(
    client: &AppServerClient,
    thread_id: Option<&str>,
    session_fast: bool,
) -> Result<String, ExecutorError> {
    let mut lines = vec!["# Session Status\n".to_string()];

    let rollout = match thread_id {
        Some(tid) => read_rollout_data(tid).await,
        None => None,
    };

    let config_resp = client.config_read(None).await.ok();

    lines.push("## Configuration".to_string());
    if let Some(ctx) = rollout.as_ref().and_then(|r| r.turn_context.as_ref()) {
        if let Some(model) = &ctx.model {
            lines.push(format!("- **Model**: `{model}`"));
        }
        if let Some(policy) = &ctx.approval_policy {
            lines.push(format!("- **Approvals**: `{policy}`"));
        }
        if let Some(sandbox) = &ctx.sandbox_policy {
            let label = sandbox
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            lines.push(format!("- **Sandbox**: `{label}`"));
        }
        let effort = ctx.effort.as_deref().unwrap_or("default");
        let summary = ctx.summary.as_deref().unwrap_or("auto");
        lines.push(format!(
            "- **Reasoning**: effort: `{effort}` summary: `{summary}`"
        ));
    } else if let Some(ref resp) = config_resp {
        let cfg = &resp.config;
        if let Some(model) = &cfg.model {
            lines.push(format!("- **Model**: `{model}`"));
        }
        if let Some(provider) = &cfg.model_provider {
            lines.push(format!("- **Provider**: `{provider}`"));
        }
        if let Some(policy) = &cfg.approval_policy {
            lines.push(format!("- **Approvals**: `{policy:?}`"));
        }
        if let Some(sandbox) = &cfg.sandbox_mode {
            lines.push(format!("- **Sandbox**: `{sandbox:?}`"));
        }
        if let Some(effort) = &cfg.model_reasoning_effort {
            lines.push(format!("- **Reasoning effort**: `{effort}`"));
        }
        if let Some(summary) = &cfg.model_reasoning_summary {
            lines.push(format!("- **Reasoning summary**: `{summary:?}`"));
        }
    } else {
        lines.push("_Config unavailable_".to_string());
    }

    // Show fast mode
    let global_fast = config_resp
        .as_ref()
        .and_then(|r| r.config.service_tier.as_ref())
        .map(|t| t == ServiceTier::Fast.request_value())
        .unwrap_or(false);
    if global_fast || session_fast {
        lines.push("- **Service Tier**: `fast ⚡`".to_string());
    }

    // Thread info
    if let Some(thread_id) = thread_id {
        lines.push(String::new());
        lines.push("## Thread".to_string());
        match client.thread_read(thread_id.to_string()).await {
            Ok(resp) => {
                let thread = &resp.thread;
                lines.push(format!("- **ID**: `{}`", thread.id));
                if let Some(name) = &thread.name {
                    lines.push(format!("- **Name**: {name}"));
                }
                lines.push(format!("- **CWD**: `{}`", thread.cwd.display()));
                lines.push(format!("- **CLI version**: `{}`", thread.cli_version));
                lines.push(format!(
                    "- **History mode**: `{}`",
                    format!("{:?}", thread.history_mode).to_ascii_lowercase()
                ));
                let source_label = format!("{:?}", thread.source).replace("VsCode", "Vibe Studio");
                lines.push(format!("- **Source**: `{source_label}`"));
            }
            Err(err) => {
                lines.push(format!("_Thread info unavailable: {err}_"));
            }
        }
    }

    // Token usage (best-effort from rollout file)
    if let Some(rollout) = &rollout {
        lines.push(String::new());
        lines.push("## Token Usage".to_string());
        if let Some(info) = &rollout.token_usage {
            let total = &info.total_token_usage;
            let last = &info.last_token_usage;
            lines.push(format!("**Total**: `{}`", total.total_tokens));
            lines.push(format!(
                "  - Input: `{}` | Output: `{}` | Reasoning: `{}` | Cached: `{}`",
                total.input_tokens,
                total.output_tokens,
                total.reasoning_output_tokens,
                total.cached_input_tokens,
            ));
            lines.push(format!("\n**Last Turn**: `{}`", last.total_tokens));
            lines.push(format!(
                "  - Input: `{}` | Output: `{}` | Reasoning: `{}` | Cached: `{}`",
                last.input_tokens,
                last.output_tokens,
                last.reasoning_output_tokens,
                last.cached_input_tokens,
            ));
            if let Some(window) = info.model_context_window {
                lines.push(format!("\n**Context Window**: `{window}`"));
            }
        } else {
            lines.push("_Token usage unavailable_".to_string());
        }
    }

    match client.get_account_rate_limits().await {
        Ok(resp) => {
            let rl = &resp.rate_limits;
            lines.push(String::new());
            lines.push("## Rate Limits".to_string());
            if let Some(plan) = &rl.plan_type {
                lines.push(format!("- **Plan**: `{plan:?}`"));
            }
            if let Some(primary) = &rl.primary {
                lines.push(format!("- **Primary**: `{}%` used", primary.used_percent));
            }
            if let Some(secondary) = &rl.secondary {
                lines.push(format!(
                    "- **Secondary**: `{}%` used",
                    secondary.used_percent
                ));
            }
            if let Some(credits) = &rl.credits {
                let balance = credits.balance.as_deref().unwrap_or(if credits.unlimited {
                    "unlimited"
                } else {
                    "none"
                });
                lines.push(format!("- **Credits**: `{balance}`"));
            }
            if let Some(reset_credits) = &resp.rate_limit_reset_credits {
                lines.extend(format_rate_limit_reset_credits(reset_credits));
            }
        }
        Err(err) => {
            tracing::debug!("rate limits unavailable: {err}");
        }
    }

    Ok(lines.join("\n"))
}

fn format_rate_limit_reset_credits(summary: &RateLimitResetCreditsSummary) -> Vec<String> {
    let mut lines = vec![format!(
        "- **Rate-limit reset credits available**: `{}`",
        summary.available_count
    )];
    let Some(credits) = &summary.credits else {
        return lines;
    };

    for credit in credits {
        let title = credit.title.as_deref().unwrap_or("Rate-limit reset credit");
        let description = credit.description.as_deref().unwrap_or("Not provided");
        let expires_at = credit
            .expires_at
            .map(format_unix_timestamp)
            .unwrap_or_else(|| "never".to_string());
        let status = match credit.status {
            RateLimitResetCreditStatus::Available => "available",
            RateLimitResetCreditStatus::Redeeming => "redeeming",
            RateLimitResetCreditStatus::Redeemed => "redeemed",
            RateLimitResetCreditStatus::Unknown => "unknown",
        };
        lines.push(format!("  - **{title}**"));
        lines.push(format!("    - Description: {description}"));
        lines.push(format!("    - Status: `{status}`"));
        lines.push(format!("    - Expires: `{expires_at}`"));
    }

    lines
}

fn format_unix_timestamp(timestamp: i64) -> String {
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| timestamp.to_string())
}

#[derive(serde::Deserialize)]
struct RolloutEntry {
    #[serde(rename = "type")]
    entry_type: String,
    #[serde(default)]
    payload: serde_json::Value,
}

#[derive(serde::Deserialize)]
struct TokenCountPayload {
    info: Option<RolloutTokenUsageInfo>,
}

#[derive(serde::Deserialize)]
struct RolloutTokenUsageInfo {
    total_token_usage: RolloutTokenUsage,
    last_token_usage: RolloutTokenUsage,
    model_context_window: Option<u64>,
}

#[derive(serde::Deserialize)]
struct RolloutTokenUsage {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
}

#[derive(serde::Deserialize)]
struct TurnContextPayload {
    model: Option<String>,
    approval_policy: Option<String>,
    sandbox_policy: Option<serde_json::Value>,
    effort: Option<String>,
    summary: Option<String>,
}

struct RolloutData {
    turn_context: Option<TurnContextPayload>,
    token_usage: Option<RolloutTokenUsageInfo>,
}

async fn read_rollout_data(session_id: &str) -> Option<RolloutData> {
    let sessions_dir = codex_home()?.join("sessions");
    let rollout_path = find_rollout_file(&sessions_dir, session_id).await?;

    let file = tokio::fs::File::open(&rollout_path).await.ok()?;
    let reader = tokio::io::BufReader::new(file);

    let mut last_turn_context: Option<TurnContextPayload> = None;
    let mut last_token_usage: Option<RolloutTokenUsageInfo> = None;

    use tokio::io::AsyncBufReadExt;
    let mut lines = reader.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let entry: RolloutEntry = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        match entry.entry_type.as_str() {
            "turn_context" => {
                if let Ok(ctx) = serde_json::from_value::<TurnContextPayload>(entry.payload) {
                    last_turn_context = Some(ctx);
                }
            }
            "event_msg" => {
                if let Ok(tc) = serde_json::from_value::<TokenCountPayload>(entry.payload)
                    && tc.info.is_some()
                {
                    last_token_usage = tc.info;
                }
            }
            _ => {}
        }
    }

    Some(RolloutData {
        turn_context: last_turn_context,
        token_usage: last_token_usage,
    })
}

async fn find_rollout_file(dir: &Path, session_id: &str) -> Option<PathBuf> {
    let mut entries = tokio::fs::read_dir(dir).await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = Box::pin(find_rollout_file(&path, session_id)).await {
                return Some(found);
            }
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && name.starts_with("rollout-")
            && name.contains(session_id)
            && name.ends_with(".jsonl")
        {
            return Some(path);
        }
    }
    None
}

async fn fetch_mcp_status_message(client: &AppServerClient) -> Result<String, ExecutorError> {
    let mut cursor = None;
    let mut servers = Vec::new();
    loop {
        let response = client.list_mcp_server_status(cursor).await?;
        servers.extend(response.data);
        cursor = response.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(format_mcp_status(&servers))
}

fn format_mcp_status(servers: &[codex_app_server_protocol::McpServerStatus]) -> String {
    if servers.is_empty() {
        return "_No MCP servers configured._".to_string();
    }
    let mut lines = vec![format!("# MCP Servers ({})\n", servers.len())];
    for server in servers {
        let auth = format_mcp_auth_status(&server.auth_status);
        lines.push(format!("## {}", server.name));
        lines.push(format!("- **Auth**: `{auth}`"));

        let mut tools: Vec<String> = server.tools.keys().cloned().collect();
        tools.sort();
        if tools.is_empty() {
            lines.push("- **Tools**: _none_".to_string());
        } else {
            lines.push(format!("- **Tools**: `{}`", tools.join("`, `")));
        }

        if !server.resources.is_empty() {
            let mut names: Vec<String> = server
                .resources
                .iter()
                .map(|res| res.name.clone())
                .collect();
            names.sort();
            lines.push(format!("- **Resources**: `{}`", names.join("`, `")));
        }

        if !server.resource_templates.is_empty() {
            let mut names: Vec<String> = server
                .resource_templates
                .iter()
                .map(|template| template.name.clone())
                .collect();
            names.sort();
            lines.push(format!(
                "- **Resource Templates**: `{}`",
                names.join("`, `")
            ));
        }

        lines.push(String::new()); // Empty line between servers
    }
    lines.join("\n")
}

fn format_mcp_auth_status(status: &codex_app_server_protocol::McpAuthStatus) -> &'static str {
    match status {
        codex_app_server_protocol::McpAuthStatus::Unsupported => "unsupported",
        codex_app_server_protocol::McpAuthStatus::NotLoggedIn => "not logged in",
        codex_app_server_protocol::McpAuthStatus::BearerToken => "bearer token",
        codex_app_server_protocol::McpAuthStatus::OAuth => "oauth",
    }
}

fn format_skills_status(response: &SkillsListResponse) -> String {
    let has_skills = response.data.iter().any(|entry| !entry.skills.is_empty());
    let has_errors = response.data.iter().any(|entry| !entry.errors.is_empty());
    if !has_skills && !has_errors {
        return "_No Codex skills available for this workspace._".to_string();
    }

    let mut lines = vec!["# Codex Skills".to_string()];

    for entry in &response.data {
        lines.push(String::new());
        lines.push(format!("## {}", entry.cwd.display()));

        if entry.skills.is_empty() {
            lines.push("_No skills found._".to_string());
        } else {
            let mut skills = entry.skills.iter().collect::<Vec<_>>();
            skills.sort_by(|a, b| a.name.cmp(&b.name));

            for skill in skills {
                let status = if skill.enabled { "enabled" } else { "disabled" };
                lines.push(format!(
                    "- `{}` - {}",
                    format_skill_invocation(&skill.name),
                    skill.description
                ));
                lines.push(format!("  - Status: `{status}`"));
                lines.push(format!("  - Scope: `{}`", format_skill_scope(skill.scope)));
                lines.push(format!("  - Path: `{}`", skill.path.display()));
            }
        }

        if !entry.errors.is_empty() {
            lines.push(String::new());
            lines.push("### Load Errors".to_string());
            for error in &entry.errors {
                lines.push(format!("- `{}`: {}", error.path.display(), error.message));
            }
        }
    }

    lines.join("\n")
}

fn format_skill_invocation(name: &str) -> String {
    if name.starts_with('$') {
        name.to_string()
    } else {
        format!("${name}")
    }
}

fn format_skill_scope(scope: SkillScope) -> &'static str {
    match scope {
        SkillScope::User => "user",
        SkillScope::Repo => "repo",
        SkillScope::System => "system",
        SkillScope::Admin => "admin",
    }
}

#[cfg(test)]
mod tests {
    use codex_app_server_protocol::{
        RateLimitResetCredit, RateLimitResetCreditStatus, RateLimitResetCreditsSummary,
        RateLimitResetType, ThreadGoal, ThreadGoalStatus,
    };
    use codex_protocol::config_types::Personality;
    use serde_json::json;

    use super::{
        CodexGoalCommand, CodexPersonalityCommand, CodexSlashCommand, format_goal_details,
        format_rate_limit_reset_credits, format_seconds, format_skills_status,
        skill_slash_commands, supported_slash_commands, validate_workflow_slash_command,
        workflow_diagnostic_session_id,
    };

    #[test]
    fn workflow_codex_diagnostics_retain_bound_native_session_and_leave_ordinary_queries_threadless()
     {
        assert_eq!(
            workflow_diagnostic_session_id(Some("bound-main-native-thread"), true),
            Some("bound-main-native-thread")
        );
        assert_eq!(
            workflow_diagnostic_session_id(Some("ordinary-native-thread"), false),
            None
        );
        assert_eq!(workflow_diagnostic_session_id(None, false), None);
        // A first scoped command has no continuation to invent; the handler's
        // existing scoped-start branch can create the initial native thread.
        assert_eq!(workflow_diagnostic_session_id(None, true), None);
    }

    #[test]
    fn workflow_codex_fast_mutations_are_rejected_before_native_config_writes() {
        for prompt in [
            "/fast",
            "/fast on",
            "/fast off",
            "/fast true",
            "/fast false",
        ] {
            let command = CodexSlashCommand::parse(prompt).unwrap();
            let error = validate_workflow_slash_command(&command, true).unwrap_err();
            assert!(
                error.to_string().contains("runtime configuration is fixed"),
                "{prompt}"
            );
            assert!(error.to_string().contains("/fast status"));
            assert!(
                validate_workflow_slash_command(&command, false).is_ok(),
                "ordinary {prompt}"
            );
        }
    }

    #[test]
    fn workflow_codex_fast_status_and_other_read_only_commands_remain_allowed() {
        for prompt in ["/fast status", "/status", "/mcp", "/skills"] {
            let command = CodexSlashCommand::parse(prompt).unwrap();
            assert!(
                validate_workflow_slash_command(&command, true).is_ok(),
                "{prompt}"
            );
        }
        // Personality is not a captured ExecutorConfig field. Do not broaden
        // this narrowly scoped immutable-tier check into a native-command ban.
        let personality = CodexSlashCommand::Personality(super::CodexPersonalityCommand::Set(
            Personality::Friendly,
        ));
        assert!(validate_workflow_slash_command(&personality, true).is_ok());
    }

    #[test]
    fn formats_rate_limit_reset_credit_details() {
        let lines = format_rate_limit_reset_credits(&RateLimitResetCreditsSummary {
            available_count: 2,
            credits: Some(vec![RateLimitResetCredit {
                id: "credit-1".to_string(),
                reset_type: RateLimitResetType::CodexRateLimits,
                status: RateLimitResetCreditStatus::Available,
                granted_at: 1_700_000_000,
                expires_at: Some(1_893_456_000),
                title: Some("Launch reset".to_string()),
                description: Some("Resets the Codex rate-limit window once.".to_string()),
            }]),
        });

        let message = lines.join("\n");
        assert!(message.contains("`2`"));
        assert!(message.contains("Launch reset"));
        assert!(message.contains("Resets the Codex rate-limit window once."));
        assert!(message.contains("Status: `available`"));
        assert!(message.contains("Expires: `2030-01-01T00:00:00Z`"));
    }

    #[test]
    fn parses_fast_enable_and_disable() {
        assert!(matches!(
            CodexSlashCommand::parse("/fast on"),
            Some(CodexSlashCommand::Fast {
                enable: Some(true),
                status: false,
            })
        ));
        assert!(matches!(
            CodexSlashCommand::parse("/fast off"),
            Some(CodexSlashCommand::Fast {
                enable: Some(false),
                status: false,
            })
        ));
    }

    #[test]
    fn parses_fast_status() {
        assert!(matches!(
            CodexSlashCommand::parse("/fast status"),
            Some(CodexSlashCommand::Fast {
                enable: None,
                status: true,
            })
        ));
    }

    #[test]
    fn parses_fast_toggle_without_argument() {
        assert!(matches!(
            CodexSlashCommand::parse("/fast"),
            Some(CodexSlashCommand::Fast {
                enable: None,
                status: false,
            })
        ));
    }

    #[test]
    fn parses_skills_command() {
        assert!(matches!(
            CodexSlashCommand::parse("/skills"),
            Some(CodexSlashCommand::Skills)
        ));
    }

    #[test]
    fn parses_goal_commands() {
        assert!(matches!(
            CodexSlashCommand::parse("/goal"),
            Some(CodexSlashCommand::Goal(CodexGoalCommand::Show))
        ));
        assert!(matches!(
            CodexSlashCommand::parse("/goal clear"),
            Some(CodexSlashCommand::Goal(CodexGoalCommand::Clear))
        ));
        assert!(matches!(
            CodexSlashCommand::parse("/goal pause"),
            Some(CodexSlashCommand::Goal(CodexGoalCommand::Pause))
        ));
        assert!(matches!(
            CodexSlashCommand::parse("/goal resume"),
            Some(CodexSlashCommand::Goal(CodexGoalCommand::Resume))
        ));
        assert!(matches!(
            CodexSlashCommand::parse("/goal ship Codex slash commands"),
            Some(CodexSlashCommand::Goal(CodexGoalCommand::Set { objective }))
                if objective == "ship Codex slash commands"
        ));
    }

    #[test]
    fn parses_personality_commands() {
        assert!(matches!(
            CodexSlashCommand::parse("/personality"),
            Some(CodexSlashCommand::Personality(
                CodexPersonalityCommand::Show
            ))
        ));
        assert!(matches!(
            CodexSlashCommand::parse("/personality friendly"),
            Some(CodexSlashCommand::Personality(
                CodexPersonalityCommand::Set(Personality::Friendly)
            ))
        ));
        assert!(matches!(
            CodexSlashCommand::parse("/personality pragmatic"),
            Some(CodexSlashCommand::Personality(
                CodexPersonalityCommand::Set(Personality::Pragmatic)
            ))
        ));
        assert!(matches!(
            CodexSlashCommand::parse("/personality none"),
            Some(CodexSlashCommand::Personality(
                CodexPersonalityCommand::Set(Personality::None)
            ))
        ));
        assert!(matches!(
            CodexSlashCommand::parse("/personality verbose"),
            Some(CodexSlashCommand::Personality(
                CodexPersonalityCommand::Invalid { value }
            )) if value == "verbose"
        ));
    }

    #[test]
    fn parses_review_with_optional_instructions() {
        assert!(matches!(
            CodexSlashCommand::parse("/review"),
            Some(CodexSlashCommand::Review { instructions: None })
        ));

        assert!(matches!(
            CodexSlashCommand::parse("/review focus on regressions"),
            Some(CodexSlashCommand::Review {
                instructions: Some(instructions)
            }) if instructions == "focus on regressions"
        ));
    }

    #[test]
    fn advertised_slash_commands_are_supported_by_parser() {
        let commands = supported_slash_commands();
        let command_names = commands
            .iter()
            .map(|command| command.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            command_names,
            vec![
                "compact",
                "review",
                "init",
                "status",
                "mcp",
                "skills",
                "fast",
                "goal",
                "personality"
            ]
        );

        assert!(commands.iter().any(|cmd| cmd.name == "skills"));
        assert!(commands.iter().any(|cmd| cmd.name == "review"));
        assert!(!commands.iter().any(|cmd| cmd.name == "model"));
        assert!(!commands.iter().any(|cmd| cmd.name == "plan"));

        for command in commands {
            let prompt = format!("/{}", command.name);
            assert!(
                CodexSlashCommand::parse(&prompt).is_some(),
                "advertised command `{}` must be parsed",
                command.name
            );
        }
    }

    #[test]
    fn skill_slash_commands_only_include_enabled_skills() {
        let commands = skill_slash_commands(&[
            crate::executor_discovery::CodexSkillDescription {
                name: "frontend-design".to_string(),
                description: "Design frontend UX".to_string(),
                short_description: Some("Design UX".to_string()),
                path: "skills/frontend-design/SKILL.md".into(),
                scope: "repo".to_string(),
                enabled: true,
            },
            crate::executor_discovery::CodexSkillDescription {
                name: "disabled-skill".to_string(),
                description: "Disabled".to_string(),
                short_description: None,
                path: "skills/disabled/SKILL.md".into(),
                scope: "repo".to_string(),
                enabled: false,
            },
        ]);

        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, "frontend-design");
        assert_eq!(
            commands[0].source,
            Some(crate::executors::SlashCommandSource::Skill)
        );
        assert_eq!(
            commands[0].support_level,
            Some(crate::executors::SlashCommandSupportLevel::Skill)
        );
        assert_eq!(commands[0].description.as_deref(), Some("Design UX"));
    }

    #[test]
    fn formats_skills_status() {
        let cwd = std::env::current_dir().unwrap();
        let skill_path = cwd
            .join(".codex")
            .join("skills")
            .join("demo")
            .join("SKILL.md");
        let response = serde_json::from_value(json!({
            "data": [{
                "cwd": cwd,
                "skills": [{
                    "name": "demo-skill",
                    "description": "Demo skill description",
                    "shortDescription": "Demo short description",
                    "path": skill_path,
                    "scope": "user",
                    "enabled": true
                }],
                "errors": []
            }]
        }))
        .unwrap();

        let formatted = format_skills_status(&response);

        assert!(formatted.contains("# Codex Skills"));
        assert!(formatted.contains("`$demo-skill`"));
        assert!(formatted.contains("Demo skill description"));
        assert!(formatted.contains("enabled"));
    }

    #[test]
    fn formats_goal_details() {
        let goal = ThreadGoal {
            thread_id: "thread_123".to_string(),
            objective: "finish slash commands".to_string(),
            status: ThreadGoalStatus::Paused,
            token_budget: Some(12000),
            tokens_used: 345,
            time_used_seconds: 3723,
            created_at: 1,
            updated_at: 2,
        };

        let formatted = format_goal_details(&goal);

        assert!(formatted.contains("finish slash commands"));
        assert!(formatted.contains("`paused`"));
        assert!(formatted.contains("`345`"));
        assert!(formatted.contains("`1h 2m`"));
        assert!(formatted.contains("`12000`"));
    }

    #[test]
    fn formats_seconds_for_short_and_long_durations() {
        assert_eq!(format_seconds(12), "12s");
        assert_eq!(format_seconds(125), "2m 5s");
        assert_eq!(format_seconds(3723), "1h 2m");
    }
}
