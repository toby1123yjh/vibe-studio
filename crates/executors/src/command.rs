use std::{collections::HashMap, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;
use workspace_utils::shell::resolve_executable_path;

use crate::executors::ExecutorError;

/// V1 direct runtime command names.  These resolve from PATH and are never
/// rewritten to an npm `latest` selector.
pub const GEMINI_DEFAULT_BASE_COMMAND: &str = "gemini";
pub const CODEX_DEFAULT_BASE_COMMAND: &str = "codex";
pub const CLAUDE_DEFAULT_BASE_COMMAND: &str = "claude";
pub const OH_MY_PI_DEFAULT_BASE_COMMAND: &str = "omp";
pub const OPENCODE_DEFAULT_BASE_COMMAND: &str = "opencode";
pub const DEEPSEEK_HARNESS_DEFAULT_BASE_COMMAND: &str = "dsh";

/// Detect the same executable that launch will use, without running the CLI or
/// interpreting configuration/authentication files as installation evidence.
pub fn is_command_installed(default: &str, overrides: &CmdOverrides) -> bool {
    let base = overrides
        .base_command_override
        .as_deref()
        .unwrap_or(default);
    CommandBuilder::new(base)
        .build_initial()
        .ok()
        .is_some_and(|parts| {
            workspace_utils::shell::resolve_executable_path_blocking(&parts.program).is_some()
        })
}

#[derive(Debug, Error)]
pub enum CommandBuildError {
    #[error("base command cannot be parsed: {0}")]
    InvalidBase(String),
    #[error("base command is empty after parsing")]
    EmptyCommand,
    #[error("failed to quote command: {0}")]
    QuoteError(#[from] shlex::QuoteError),
}

#[derive(Debug, Clone)]
pub struct CommandParts {
    program: String,
    args: Vec<String>,
}

impl CommandParts {
    pub fn new(program: String, args: Vec<String>) -> Self {
        Self { program, args }
    }

    pub fn extend_args(mut self, args: impl IntoIterator<Item = String>) -> Self {
        self.args.extend(args);
        self
    }

    pub async fn into_resolved(self) -> Result<(PathBuf, Vec<String>), ExecutorError> {
        let CommandParts { program, args } = self;
        let executable = resolve_executable_path(&program)
            .await
            .ok_or(ExecutorError::ExecutableNotFound { program })?;
        Ok((executable, args))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, JsonSchema, Default)]
pub struct CmdOverrides {
    #[schemars(
        title = "Base Command Override",
        description = "Override the base command with a custom command"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_command_override: Option<String>,
    #[schemars(
        title = "Additional Parameters",
        description = "Additional parameters to append to the base command"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_params: Option<Vec<String>>,
    #[schemars(
        title = "Environment Variables",
        description = "Environment variables to set when running the executor"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, JsonSchema)]
pub struct CommandBuilder {
    /// Base executable command (e.g., "claude", "codex", or "omp")
    pub base: String,
    /// Optional parameters to append to the base command
    pub params: Option<Vec<String>>,
}

impl CommandBuilder {
    pub fn new<S: Into<String>>(base: S) -> Self {
        Self {
            base: base.into(),
            params: None,
        }
    }

    pub fn params<I>(mut self, params: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        self.params = Some(params.into_iter().map(|p| p.into()).collect());
        self
    }

    pub fn override_base<S: Into<String>>(mut self, base: S) -> Self {
        self.base = base.into();
        self
    }

    pub fn extend_params<I>(mut self, more: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        let extra: Vec<String> = more.into_iter().map(|p| p.into()).collect();
        match &mut self.params {
            Some(p) => p.extend(extra),
            None => self.params = Some(extra),
        }
        self
    }

    pub fn build_initial(&self) -> Result<CommandParts, CommandBuildError> {
        self.build(&[])
    }

    pub fn build_follow_up(
        &self,
        additional_args: &[String],
    ) -> Result<CommandParts, CommandBuildError> {
        self.build(additional_args)
    }

    fn build(&self, additional_args: &[String]) -> Result<CommandParts, CommandBuildError> {
        let mut parts = vec![];
        let base_parts = split_command_line(&self.base)?;
        parts.extend(base_parts);
        if let Some(ref params) = self.params {
            parts.extend(params.clone());
        }
        parts.extend(additional_args.iter().cloned());

        if parts.is_empty() {
            return Err(CommandBuildError::EmptyCommand);
        }

        let program = parts.remove(0);
        Ok(CommandParts::new(program, parts))
    }
}

fn split_command_line(input: &str) -> Result<Vec<String>, CommandBuildError> {
    #[cfg(windows)]
    {
        let parts = split_windows_command_line(input)?;
        if parts.is_empty() {
            Err(CommandBuildError::EmptyCommand)
        } else {
            Ok(parts)
        }
    }

    #[cfg(not(windows))]
    {
        shlex::split(input).ok_or_else(|| CommandBuildError::InvalidBase(input.to_string()))
    }
}

#[cfg(windows)]
fn split_windows_command_line(input: &str) -> Result<Vec<String>, CommandBuildError> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut token_started = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                token_started = true;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                token_started = true;
            }
            '\\' => {
                if matches!(chars.peek(), Some(&'"') | Some(&'\'')) {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(ch);
                }
                token_started = true;
            }
            ch if ch.is_whitespace() && !in_single_quote && !in_double_quote => {
                if token_started {
                    parts.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            _ => {
                current.push(ch);
                token_started = true;
            }
        }
    }

    if in_single_quote || in_double_quote {
        return Err(CommandBuildError::InvalidBase(input.to_string()));
    }

    if token_started {
        parts.push(current);
    }

    Ok(parts)
}

pub fn apply_overrides(
    builder: CommandBuilder,
    overrides: &CmdOverrides,
) -> Result<CommandBuilder, CommandBuildError> {
    let builder = if let Some(ref base) = overrides.base_command_override {
        builder.override_base(base.clone())
    } else {
        builder
    };
    if let Some(ref extra) = overrides.additional_params {
        Ok(builder.extend_params(extra.clone()))
    } else {
        Ok(builder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_detection_preserves_quoted_explicit_override() {
        let executable = std::env::current_exe().unwrap();
        let overrides = super::CmdOverrides {
            base_command_override: Some(format!("\"{}\" --unused", executable.display())),
            ..Default::default()
        };
        assert!(super::is_command_installed("not-installed", &overrides));
        let empty = super::CmdOverrides {
            base_command_override: Some(String::new()),
            ..Default::default()
        };
        assert!(!super::is_command_installed("codex", &empty));
    }
    #[test]
    fn parses_builtin_local_executor_commands() {
        let cases: [(&str, &str, Vec<&str>); 4] = [
            ("claude", "claude", vec![]),
            ("codex", "codex", vec![]),
            ("gemini", "gemini", vec![]),
            ("omp", "omp", vec![]),
        ];

        for (command, expected_program, expected_args) in cases {
            let parts = CommandBuilder::new(command)
                .build_initial()
                .expect("command should parse");

            assert_eq!(parts.program, expected_program);
            assert_eq!(
                parts.args,
                expected_args
                    .iter()
                    .map(|arg| (*arg).to_string())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn parses_explicit_scoped_npm_override_command() {
        let parts = CommandBuilder::new("npx -y --package @example/agent@1.2.3 agent")
            .build_initial()
            .expect("command should parse");

        assert_eq!(parts.program, "npx");
        assert_eq!(
            parts.args,
            vec!["-y", "--package", "@example/agent@1.2.3", "agent"]
        );
    }

    #[test]
    fn parses_quoted_windows_paths() {
        let parts = CommandBuilder::new(r#""C:\Program Files\OpenAI Codex\codex.exe" app-server"#)
            .build_initial()
            .expect("command should parse");

        assert_eq!(parts.program, r#"C:\Program Files\OpenAI Codex\codex.exe"#);
        assert_eq!(parts.args, vec!["app-server"]);
    }

    #[test]
    fn base_command_override_preserves_custom_base_and_params() {
        let builder = CommandBuilder::new("gemini").extend_params(["--acp"]);
        let overrides = CmdOverrides {
            base_command_override: Some(
                r#""C:\Program Files\Google Gemini\gemini.cmd""#.to_string(),
            ),
            ..Default::default()
        };

        let parts = apply_overrides(builder, &overrides)
            .expect("overrides should apply")
            .build_initial()
            .expect("command should parse");

        assert_eq!(
            parts.program,
            r#"C:\Program Files\Google Gemini\gemini.cmd"#
        );
        assert_eq!(parts.args, vec!["--acp"]);
    }

    #[test]
    fn base_command_override_preserves_explicit_npm_command_and_params() {
        let builder = CommandBuilder::new("gemini").extend_params(["--acp"]);
        let overrides = CmdOverrides {
            base_command_override: Some("npx -y --package @google/gemini-cli gemini".to_string()),
            ..Default::default()
        };

        let parts = apply_overrides(builder, &overrides)
            .expect("overrides should apply")
            .build_initial()
            .expect("command should parse");

        assert_eq!(parts.program, "npx");
        assert_eq!(
            parts.args,
            vec!["-y", "--package", "@google/gemini-cli", "gemini", "--acp"]
        );
    }

    #[test]
    fn additional_params_preserve_argument_boundaries() {
        let builder = CommandBuilder::new("claude");
        let overrides = CmdOverrides {
            additional_params: Some(vec![
                "--model".to_string(),
                "claude sonnet".to_string(),
                "--flag=value with spaces".to_string(),
            ]),
            ..Default::default()
        };

        let parts = apply_overrides(builder, &overrides)
            .expect("overrides should apply")
            .build_initial()
            .expect("command should parse");

        assert_eq!(
            parts.args,
            vec!["--model", "claude sonnet", "--flag=value with spaces",]
        );
    }
}
