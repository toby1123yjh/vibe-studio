use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use convert_case::{Case, Casing};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use workspace_utils::command_ext::GroupSpawnNoWindowExt;

use super::{ClaudeCode, ClaudePlugin};
use crate::{
    command::{CommandBuildError, CommandBuilder},
    env::{ExecutionEnv, RepoContext},
    executor_discovery::CodexSkillDescription,
    executors::{
        ExecutorError, SlashCommandDescription, SlashCommandSource, SlashCommandSupportLevel,
    },
    model_selector::AgentInfo,
};

const SLASH_COMMANDS_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy)]
struct ClaudeKnownSlashCommand {
    name: &'static str,
    description: &'static str,
    support_level: SlashCommandSupportLevel,
}

const CLAUDE_KNOWN_SLASH_COMMANDS: &[ClaudeKnownSlashCommand] = &[
    ClaudeKnownSlashCommand {
        name: "compact",
        description: "Clear conversation history but keep a summary in context. Optional: /compact [instructions for summarization]",
        support_level: SlashCommandSupportLevel::Product,
    },
    ClaudeKnownSlashCommand {
        name: "review",
        description: "Review a pull request",
        support_level: SlashCommandSupportLevel::Product,
    },
    ClaudeKnownSlashCommand {
        name: "security-review",
        description: "Complete a security review of the pending changes on the current branch",
        support_level: SlashCommandSupportLevel::Product,
    },
    ClaudeKnownSlashCommand {
        name: "init",
        description: "Initialize a new CLAUDE.md file with codebase documentation",
        support_level: SlashCommandSupportLevel::Product,
    },
    ClaudeKnownSlashCommand {
        name: "context",
        description: "Visualize current context usage",
        support_level: SlashCommandSupportLevel::Product,
    },
    ClaudeKnownSlashCommand {
        name: "usage",
        description: "Show Claude usage and billing information",
        support_level: SlashCommandSupportLevel::Product,
    },
    ClaudeKnownSlashCommand {
        name: "insights",
        description: "Show Claude Code usage insights",
        support_level: SlashCommandSupportLevel::Product,
    },
    ClaudeKnownSlashCommand {
        name: "goal",
        description: "View or manage Claude's current task goal",
        support_level: SlashCommandSupportLevel::Product,
    },
    ClaudeKnownSlashCommand {
        name: "clear",
        description: "Clear Claude's current conversation context",
        support_level: SlashCommandSupportLevel::Native,
    },
    ClaudeKnownSlashCommand {
        name: "config",
        description: "Open or manage Claude Code configuration",
        support_level: SlashCommandSupportLevel::Native,
    },
    ClaudeKnownSlashCommand {
        name: "reload-skills",
        description: "Reload Claude Code skills",
        support_level: SlashCommandSupportLevel::Native,
    },
    ClaudeKnownSlashCommand {
        name: "team-onboarding",
        description: "Run Claude Code team onboarding",
        support_level: SlashCommandSupportLevel::Native,
    },
    ClaudeKnownSlashCommand {
        name: "heapdump",
        description: "Create a Claude Code diagnostic heap dump",
        support_level: SlashCommandSupportLevel::Diagnostic,
    },
];

impl ClaudeCode {
    fn extract_description(content: &str) -> Option<String> {
        if !content.starts_with("---") {
            return None;
        }

        // Find end of frontmatter
        let end = content[3..].find("---")?;
        let frontmatter = &content[3..3 + end];

        for line in frontmatter.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("description:") {
                return Some(rest.trim().to_string());
            }
        }
        None
    }

    fn make_key(prefix: &Option<String>, name: &str) -> String {
        prefix
            .as_ref()
            .map(|p| format!("{}:{}", p, name))
            .unwrap_or_else(|| name.to_string())
    }

    async fn try_read_description(path: &Path) -> Option<String> {
        match fs::read_to_string(path).await {
            Ok(content) => Self::extract_description(&content).or_else(|| {
                tracing::warn!("Failed to read frontmatter description from {:?}", path);
                None
            }),
            Err(e) => {
                tracing::error!("Failed to read file {:?}: {}", path, e);
                None
            }
        }
    }

    async fn scan_dir(
        dir: &Path,
        prefix: &Option<String>,
        get_entry: fn(&Path) -> Option<(&str, PathBuf)>,
    ) -> HashMap<String, String> {
        let mut result = HashMap::new();
        if let Ok(mut entries) = fs::read_dir(dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Some((name, desc_path)) = get_entry(&entry.path())
                    && let Some(desc) = Self::try_read_description(&desc_path).await
                {
                    result.insert(Self::make_key(prefix, name), desc);
                }
            }
        }
        result
    }

    async fn scan_base_path(base_path: &Path, prefix: Option<String>) -> HashMap<String, String> {
        let mut descriptions = HashMap::new();

        descriptions.extend(
            Self::scan_dir(&base_path.join("commands"), &prefix, |path| {
                path.extension()
                    .is_some_and(|ext| ext == "md")
                    .then(|| {
                        let name = path.file_stem()?.to_str()?;
                        Some((name, path.to_path_buf()))
                    })
                    .flatten()
            })
            .await,
        );

        descriptions.extend(
            Self::scan_dir(&base_path.join("skills"), &prefix, |path| {
                path.is_dir()
                    .then(|| {
                        let name = path.file_name()?.to_str()?;
                        let skill_md = path.join("SKILL.md");
                        skill_md.exists().then_some((name, skill_md))
                    })
                    .flatten()
            })
            .await,
        );

        descriptions
    }

    pub async fn discover_custom_command_descriptions(
        current_dir: &Path,
        plugins: &[ClaudePlugin],
    ) -> HashMap<String, String> {
        let mut descriptions = HashMap::new();

        // Project specific
        descriptions.extend(Self::scan_base_path(&current_dir.join(".claude"), None).await);

        // Global
        if let Some(home) = dirs::home_dir() {
            descriptions.extend(Self::scan_base_path(&home.join(".claude"), None).await);
        }

        // Plugins
        for plugin in plugins {
            descriptions
                .extend(Self::scan_base_path(&plugin.path, Some(plugin.name.clone())).await);
            descriptions.extend(
                Self::scan_base_path(&plugin.path.join(".claude"), Some(plugin.name.clone())).await,
            );
        }

        descriptions
    }

    pub(super) fn hardcoded_slash_commands() -> Vec<SlashCommandDescription> {
        CLAUDE_KNOWN_SLASH_COMMANDS
            .iter()
            .map(|command| SlashCommandDescription {
                name: command.name.to_string(),
                description: Some(command.description.to_string()),
                source: Some(SlashCommandSource::Fallback),
                support_level: Some(SlashCommandSupportLevel::Fallback),
            })
            .collect()
    }

    async fn build_slash_commands_discovery_command_builder(
        &self,
    ) -> Result<CommandBuilder, CommandBuildError> {
        super::command_adapter::ClaudeCodeCommandAdapter::new(self).build_discovery_probe()
    }

    async fn discover_available_command_and_plugins(
        &self,
        current_dir: &Path,
    ) -> Result<
        (
            Vec<String>,
            Vec<ClaudePlugin>,
            Vec<String>,
            Vec<String>,
            Vec<crate::model_selector::ModelInfo>,
        ),
        ExecutorError,
    > {
        let command_builder = self
            .build_slash_commands_discovery_command_builder()
            .await?;
        let command_parts = command_builder.build_initial()?;
        let (program_path, args) = command_parts.into_resolved().await?;

        let mut command = Command::new(program_path);
        command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .current_dir(current_dir)
            .args(&args);

        ExecutionEnv::new(RepoContext::default(), false, String::new())
            .with_profile(&self.cmd)
            .apply_to_command(&mut command);

        if self.disable_api_key.unwrap_or(false) {
            command.env_remove("ANTHROPIC_API_KEY");
        }

        let mut child = command.group_spawn_no_window()?;
        let mut stdin =
            child.inner().stdin.take().ok_or_else(|| {
                ExecutorError::Io(std::io::Error::other("Claude Code missing stdin"))
            })?;
        let request =
            super::types::SDKControlRequest::new(super::types::SDKControlRequestType::Initialize {
                hooks: None,
            });
        let mut bytes = serde_json::to_vec(&request)
            .map_err(|e| ExecutorError::Io(std::io::Error::other(e)))?;
        bytes.push(b'\n');
        stdin.write_all(&bytes).await?;
        stdin.flush().await?;
        let stdout = child.inner().stdout.take().ok_or_else(|| {
            ExecutorError::Io(std::io::Error::other("Claude Code missing stdout"))
        })?;

        let mut lines = BufReader::new(stdout).lines();

        let mut discovered = None;
        let discovery = async {
            while let Some(line) = lines.next_line().await.map_err(ExecutorError::Io)? {
                if let Ok(super::types::CLIMessage::ControlResponse { response }) =
                    serde_json::from_str(&line)
                {
                    match response {
                        super::types::ControlResponseType::Success {
                            request_id,
                            response,
                        } if request_id == request.request_id => {
                            let value = response.unwrap_or_default();
                            let names = |key: &str| {
                                value[key]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(|item| {
                                        item.as_str().or_else(|| item["name"].as_str())
                                    })
                                    .map(str::to_owned)
                                    .collect::<Vec<_>>()
                            };
                            discovered = Some((
                                names("commands"),
                                vec![],
                                names("agents"),
                                names("skills"),
                                super::models_from_initialize(&value),
                            ));
                            break;
                        }
                        super::types::ControlResponseType::Error { request_id, error }
                            if request_id == request.request_id =>
                        {
                            return Err(ExecutorError::Io(std::io::Error::other(
                                error.unwrap_or_else(|| {
                                    "Claude discovery initialization failed".into()
                                }),
                            )));
                        }
                        _ => {}
                    }
                }
            }

            Ok::<(), ExecutorError>(())
        };

        let res = tokio::time::timeout(SLASH_COMMANDS_DISCOVERY_TIMEOUT, discovery).await;
        let _ = child.kill().await;

        let result = match res {
            Ok(Ok(())) => discovered.ok_or_else(|| {
                ExecutorError::Io(std::io::Error::other(
                    "Claude exited before discovery initialization",
                ))
            })?,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(ExecutorError::Io(std::io::Error::other(
                    "Timed out discovering Claude Code slash commands",
                )));
            }
        };

        Ok(result)
    }

    pub async fn discover_agents_and_slash_commands_initial(
        &self,
        current_dir: &Path,
    ) -> Result<
        (
            Vec<AgentInfo>,
            Vec<SlashCommandDescription>,
            Vec<ClaudePlugin>,
            Vec<CodexSkillDescription>,
            Vec<crate::model_selector::ModelInfo>,
        ),
        ExecutorError,
    > {
        let (names, plugins, agents, skills, models) = self
            .discover_available_command_and_plugins(current_dir)
            .await?;

        let agent_options = Self::map_discovered_agents(agents);

        // The init event lists every command the running binary supports
        // (builtin + custom), so it is the source of truth for which commands
        // exist in THIS Claude version — new builtins appear automatically and
        // removed builtins disappear. The hardcoded table only supplies nice
        // descriptions for well-known builtins; custom descriptions are filled
        // later from command frontmatter.
        let builtin_descriptions: HashMap<&str, &str> = CLAUDE_KNOWN_SLASH_COMMANDS
            .iter()
            .map(|command| (command.name, command.description))
            .collect();
        let discovered_skills = Self::map_discovered_skills(current_dir, skills);
        let skill_names: HashSet<String> = discovered_skills
            .iter()
            .map(|skill| skill.name.clone())
            .collect();
        let plugin_names: HashSet<String> =
            plugins.iter().map(|plugin| plugin.name.clone()).collect();

        let mut seen = HashSet::new();
        let mut slash_commands: Vec<SlashCommandDescription> = names
            .into_iter()
            .filter(|name| !name.is_empty() && seen.insert(name.clone()))
            .map(|name| {
                let description = builtin_descriptions
                    .get(name.as_str())
                    .map(|description| (*description).to_string());
                Self::classify_slash_command(name, description, &skill_names, &plugin_names)
            })
            .collect();

        for skill in &discovered_skills {
            if skill.name.is_empty() || !seen.insert(skill.name.clone()) {
                continue;
            }
            slash_commands.push(Self::classify_slash_command(
                skill.name.clone(),
                skill
                    .short_description
                    .clone()
                    .or_else(|| Some(skill.description.clone())),
                &skill_names,
                &plugin_names,
            ));
        }

        Ok((
            agent_options,
            slash_commands,
            plugins,
            discovered_skills,
            models,
        ))
    }

    pub async fn fill_slash_command_descriptions(
        current_dir: &Path,
        plugins: &[ClaudePlugin],
        slash_commands: &[SlashCommandDescription],
    ) -> Vec<SlashCommandDescription> {
        let descriptions = Self::discover_custom_command_descriptions(current_dir, plugins).await;

        slash_commands
            .iter()
            .map(|cmd| SlashCommandDescription {
                name: cmd.name.clone(),
                description: descriptions
                    .get(&cmd.name)
                    .cloned()
                    .or(cmd.description.clone()),
                source: cmd.source,
                support_level: cmd.support_level,
            })
            .collect()
    }

    fn classify_slash_command(
        name: String,
        description: Option<String>,
        skill_names: &HashSet<String>,
        plugin_names: &HashSet<String>,
    ) -> SlashCommandDescription {
        if let Some(command) = CLAUDE_KNOWN_SLASH_COMMANDS
            .iter()
            .find(|command| command.name == name)
        {
            return SlashCommandDescription {
                name,
                description: description.or_else(|| Some(command.description.to_string())),
                source: Some(SlashCommandSource::Builtin),
                support_level: Some(command.support_level),
            };
        }

        if skill_names.contains(&name) {
            return SlashCommandDescription {
                name,
                description,
                source: Some(SlashCommandSource::Skill),
                support_level: Some(SlashCommandSupportLevel::Skill),
            };
        }

        if let Some((prefix, _)) = name.split_once(':')
            && plugin_names.contains(prefix)
        {
            return SlashCommandDescription {
                name,
                description,
                source: Some(SlashCommandSource::Plugin),
                support_level: Some(SlashCommandSupportLevel::Custom),
            };
        }

        SlashCommandDescription {
            name,
            description,
            source: Some(SlashCommandSource::Custom),
            support_level: Some(SlashCommandSupportLevel::Custom),
        }
    }

    fn map_discovered_agents(agents: Vec<String>) -> Vec<AgentInfo> {
        let mut seen = HashSet::new();

        agents
            .into_iter()
            .filter(|name| name != "statusline-setup")
            .filter_map(|name| {
                let option = AgentInfo {
                    id: name.clone(),
                    label: Self::format_agent_label(&name),
                    description: None,
                    is_default: name == "general-purpose",
                };

                if option.id.trim().is_empty() || !seen.insert(option.id.clone()) {
                    return None;
                }
                Some(option)
            })
            .collect()
    }

    fn map_discovered_skills(
        current_dir: &Path,
        skills: Vec<String>,
    ) -> Vec<CodexSkillDescription> {
        let mut seen = HashSet::new();
        let home_claude = dirs::home_dir().map(|home| home.join(".claude"));

        skills
            .into_iter()
            .filter_map(|name| {
                let name = name.trim().to_string();
                if name.is_empty() || !seen.insert(name.clone()) {
                    return None;
                }

                let project_path = current_dir
                    .join(".claude")
                    .join("skills")
                    .join(&name)
                    .join("SKILL.md");
                let global_path = home_claude
                    .as_ref()
                    .map(|claude_dir| claude_dir.join("skills").join(&name).join("SKILL.md"));
                let path = if project_path.exists() {
                    project_path
                } else if let Some(global_path) = global_path
                    && global_path.exists()
                {
                    global_path
                } else {
                    PathBuf::from(".claude")
                        .join("skills")
                        .join(&name)
                        .join("SKILL.md")
                };

                Some(CodexSkillDescription {
                    name,
                    description: "Claude Code skill".to_string(),
                    short_description: None,
                    path,
                    scope: "claude".to_string(),
                    enabled: true,
                })
            })
            .collect()
    }

    fn format_agent_label(raw: &str) -> String {
        let raw = raw.trim();

        if let Some((prefix, suffix)) = raw.split_once(':') {
            format!("{}: {}", prefix.trim(), suffix.to_case(Case::Title))
        } else {
            raw.to_case(Case::Title)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn fallback_commands_use_current_claude_names_and_fallback_metadata() {
        let commands = ClaudeCode::hardcoded_slash_commands();
        let names = commands
            .iter()
            .map(|command| command.name.as_str())
            .collect::<HashSet<_>>();

        assert!(names.contains("usage"));
        assert!(names.contains("insights"));
        assert!(names.contains("goal"));
        assert!(!names.contains("cost"));
        assert!(!names.contains("release-notes"));
        assert!(!names.contains("pr-comments"));

        for command in commands {
            assert_eq!(command.source, Some(SlashCommandSource::Fallback));
            assert_eq!(
                command.support_level,
                Some(SlashCommandSupportLevel::Fallback)
            );
        }
    }

    #[test]
    fn classifies_known_product_commands_as_builtins() {
        let command = ClaudeCode::classify_slash_command(
            "usage".to_string(),
            None,
            &HashSet::new(),
            &HashSet::new(),
        );

        assert_eq!(command.source, Some(SlashCommandSource::Builtin));
        assert_eq!(
            command.support_level,
            Some(SlashCommandSupportLevel::Product)
        );
        assert_eq!(
            command.description.as_deref(),
            Some("Show Claude usage and billing information")
        );
    }

    #[test]
    fn classifies_skill_plugin_and_custom_commands_without_product_support() {
        let skill_names = HashSet::from(["frontend-design".to_string()]);
        let plugin_names = HashSet::from(["superpowers".to_string()]);

        let skill = ClaudeCode::classify_slash_command(
            "frontend-design".to_string(),
            None,
            &skill_names,
            &plugin_names,
        );
        assert_eq!(skill.source, Some(SlashCommandSource::Skill));
        assert_eq!(skill.support_level, Some(SlashCommandSupportLevel::Skill));

        let plugin = ClaudeCode::classify_slash_command(
            "superpowers:brainstorm".to_string(),
            None,
            &skill_names,
            &plugin_names,
        );
        assert_eq!(plugin.source, Some(SlashCommandSource::Plugin));
        assert_eq!(plugin.support_level, Some(SlashCommandSupportLevel::Custom));

        let custom = ClaudeCode::classify_slash_command(
            "ccg:daily-radar".to_string(),
            None,
            &skill_names,
            &plugin_names,
        );
        assert_eq!(custom.source, Some(SlashCommandSource::Custom));
        assert_eq!(custom.support_level, Some(SlashCommandSupportLevel::Custom));
    }

    #[test]
    fn maps_discovered_skills_as_enabled_claude_skills() {
        let skills = ClaudeCode::map_discovered_skills(
            Path::new("."),
            vec![
                "frontend-design".to_string(),
                "frontend-design".to_string(),
                " ".to_string(),
                "code-search".to_string(),
            ],
        );

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "frontend-design");
        assert_eq!(skills[0].description, "Claude Code skill");
        assert_eq!(skills[0].scope, "claude");
        assert!(skills[0].enabled);
        assert_eq!(skills[1].name, "code-search");
    }
}
