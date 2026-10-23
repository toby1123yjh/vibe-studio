use anyhow::Error;
use executors::{executors::BaseCodingAgent, profile::ExecutorProfileId};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
pub use v7::{
    EditorConfig, EditorType, GitHubConfig, NotificationConfig, ShowcaseState, SoundFile,
    ThemeMode, UiLanguage,
};

use crate::services::config::versions::v7;

fn default_git_branch_prefix() -> String {
    "vk".to_string()
}

fn default_pr_auto_description_enabled() -> bool {
    true
}

fn default_commit_reminder_enabled() -> bool {
    true
}

fn default_relay_enabled() -> bool {
    true
}

fn default_hidden_agents() -> Vec<BaseCodingAgent> {
    vec![
        #[cfg(feature = "qa-mode")]
        BaseCodingAgent::QaMock,
    ]
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, TS, PartialEq, Eq)]
pub enum SendMessageShortcut {
    ModifierEnter,
    #[default]
    Enter,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct Config {
    pub config_version: String,
    pub theme: ThemeMode,
    pub executor_profile: ExecutorProfileId,
    pub disclaimer_acknowledged: bool,
    pub onboarding_acknowledged: bool,
    #[serde(default)]
    pub remote_onboarding_acknowledged: bool,
    pub notifications: NotificationConfig,
    pub editor: EditorConfig,
    pub github: GitHubConfig,
    pub analytics_enabled: bool,
    pub workspace_dir: Option<String>,
    pub last_app_version: Option<String>,
    pub show_release_notes: bool,
    #[serde(default)]
    pub language: UiLanguage,
    #[serde(default = "default_git_branch_prefix")]
    pub git_branch_prefix: String,
    #[serde(default)]
    pub showcases: ShowcaseState,
    #[serde(default = "default_pr_auto_description_enabled")]
    pub pr_auto_description_enabled: bool,
    #[serde(default)]
    pub pr_auto_description_prompt: Option<String>,
    #[serde(default = "default_commit_reminder_enabled")]
    pub commit_reminder_enabled: bool,
    #[serde(default)]
    pub commit_reminder_prompt: Option<String>,
    #[serde(default)]
    pub send_message_shortcut: SendMessageShortcut,
    #[serde(default = "default_relay_enabled")]
    pub relay_enabled: bool,
    #[serde(default)]
    pub host_nickname: Option<String>,
    #[serde(default = "default_hidden_agents")]
    pub hidden_agents: Vec<BaseCodingAgent>,
}

impl Config {
    fn from_v7_config(old_config: v7::Config) -> Self {
        // Convert Option<bool> to bool: None or Some(true) become true, Some(false) stays false
        let analytics_enabled = old_config.analytics_enabled.unwrap_or(true);

        Self {
            config_version: "v8".to_string(),
            theme: old_config.theme,
            executor_profile: old_config.executor_profile,
            disclaimer_acknowledged: old_config.disclaimer_acknowledged,
            onboarding_acknowledged: old_config.onboarding_acknowledged,
            remote_onboarding_acknowledged: false,
            notifications: old_config.notifications,
            editor: old_config.editor,
            github: old_config.github,
            analytics_enabled,
            workspace_dir: old_config.workspace_dir,
            last_app_version: old_config.last_app_version,
            show_release_notes: old_config.show_release_notes,
            language: old_config.language,
            git_branch_prefix: old_config.git_branch_prefix,
            showcases: old_config.showcases,
            pr_auto_description_enabled: true,
            pr_auto_description_prompt: None,
            commit_reminder_enabled: true,
            commit_reminder_prompt: None,
            send_message_shortcut: SendMessageShortcut::default(),
            relay_enabled: true,
            host_nickname: None,
            hidden_agents: default_hidden_agents(),
        }
    }

    pub fn from_previous_version(raw_config: &str) -> Result<Self, Error> {
        let old_config = v7::Config::from(raw_config.to_string());
        Ok(Self::from_v7_config(old_config))
    }
}

impl From<String> for Config {
    fn from(raw_config: String) -> Self {
        if let Ok(config) = serde_json::from_str::<Config>(&raw_config)
            && config.config_version == "v8"
        {
            return config;
        }

        match Self::from_previous_version(&raw_config) {
            Ok(config) => {
                tracing::info!("Config upgraded to v8");
                config
            }
            Err(e) => {
                tracing::warn!("Config migration failed: {}, using default", e);
                Self::default()
            }
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: "v8".to_string(),
            theme: ThemeMode::System,
            executor_profile: ExecutorProfileId::new(BaseCodingAgent::ClaudeCode),
            disclaimer_acknowledged: false,
            onboarding_acknowledged: false,
            remote_onboarding_acknowledged: false,
            notifications: NotificationConfig::default(),
            editor: EditorConfig::default(),
            github: GitHubConfig::default(),
            analytics_enabled: true,
            workspace_dir: None,
            last_app_version: None,
            show_release_notes: false,
            language: UiLanguage::default(),
            git_branch_prefix: default_git_branch_prefix(),
            showcases: ShowcaseState::default(),
            pr_auto_description_enabled: true,
            pr_auto_description_prompt: None,
            commit_reminder_enabled: true,
            commit_reminder_prompt: None,
            send_message_shortcut: SendMessageShortcut::default(),
            relay_enabled: true,
            host_nickname: None,
            hidden_agents: default_hidden_agents(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_hides_non_primary_agents() {
        let hidden_agents = Config::default().hidden_agents;

        assert!(!hidden_agents.contains(&BaseCodingAgent::ClaudeCode));
        assert!(!hidden_agents.contains(&BaseCodingAgent::Codex));
        assert!(!hidden_agents.contains(&BaseCodingAgent::Gemini));
        assert!(!hidden_agents.contains(&BaseCodingAgent::OhMyPi));
        #[cfg(feature = "qa-mode")]
        assert_eq!(hidden_agents, vec![BaseCodingAgent::QaMock]);
        #[cfg(not(feature = "qa-mode"))]
        assert!(hidden_agents.is_empty());
    }

    #[test]
    fn missing_hidden_agents_uses_default_hidden_agents() {
        let mut raw_config = serde_json::to_value(Config::default()).unwrap();
        raw_config.as_object_mut().unwrap().remove("hidden_agents");

        let config: Config = serde_json::from_value(raw_config).unwrap();

        assert_eq!(config.hidden_agents, default_hidden_agents());
    }
}
