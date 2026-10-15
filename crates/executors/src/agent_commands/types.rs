use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use ts_rs::TS;

use super::{claude, codex, gemini, oh_my_pi, opencode};
use crate::executors::provider_adapter::DirectProvider;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandProvider {
    Codex,
    ClaudeCode,
    Gemini,
    OhMyPi,
    Opencode,
    DeepseekHarness,
}

impl AgentCommandProvider {
    pub const ALL: [Self; 6] = [
        Self::Codex,
        Self::ClaudeCode,
        Self::Gemini,
        Self::OhMyPi,
        Self::Opencode,
        Self::DeepseekHarness,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude_code",
            Self::Gemini => "gemini",
            Self::OhMyPi => "oh_my_pi",
            Self::Opencode => "opencode",
            Self::DeepseekHarness => "deepseek_harness",
        }
    }

    pub(super) const fn executable(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude",
            Self::Gemini => "gemini",
            Self::OhMyPi => "omp",
            Self::Opencode => "opencode",
            Self::DeepseekHarness => "dsh",
        }
    }
}

impl From<DirectProvider> for AgentCommandProvider {
    fn from(value: DirectProvider) -> Self {
        match value {
            DirectProvider::Codex => Self::Codex,
            DirectProvider::ClaudeCode => Self::ClaudeCode,
            DirectProvider::Gemini => Self::Gemini,
            DirectProvider::OhMyPi => Self::OhMyPi,
            DirectProvider::Opencode => Self::Opencode,
            DirectProvider::DeepseekHarness => Self::DeepseekHarness,
        }
    }
}

impl From<AgentCommandProvider> for DirectProvider {
    fn from(value: AgentCommandProvider) -> Self {
        match value {
            AgentCommandProvider::Codex => Self::Codex,
            AgentCommandProvider::ClaudeCode => Self::ClaudeCode,
            AgentCommandProvider::Gemini => Self::Gemini,
            AgentCommandProvider::OhMyPi => Self::OhMyPi,
            AgentCommandProvider::Opencode => Self::Opencode,
            AgentCommandProvider::DeepseekHarness => Self::DeepseekHarness,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandScope {
    User,
    Project,
}

impl AgentCommandScope {
    pub(super) const fn id(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandState {
    Enabled,
    Disabled,
    Error,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandFormat {
    CodexLegacyMarkdown,
    ClaudeMarkdown,
    GeminiToml,
    OhMyPiPromptMarkdown,
    OhMyPiExecutableModule,
    OpencodeMarkdown,
    OpencodeInline,
}

impl AgentCommandFormat {
    pub(super) const fn extension(self) -> &'static str {
        match self {
            Self::CodexLegacyMarkdown
            | Self::ClaudeMarkdown
            | Self::OhMyPiPromptMarkdown
            | Self::OpencodeMarkdown => "md",
            Self::GeminiToml => "toml",
            Self::OhMyPiExecutableModule => "ts",
            Self::OpencodeInline => "jsonc",
        }
    }

    pub(super) const fn is_managed_prompt(self) -> bool {
        !matches!(self, Self::OhMyPiExecutableModule | Self::OpencodeInline)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AgentCommandCapabilities {
    pub editable: bool,
    pub removable: bool,
    pub toggleable: bool,
}

impl AgentCommandCapabilities {
    pub(super) const fn managed() -> Self {
        Self {
            editable: true,
            removable: true,
            toggleable: true,
        }
    }

    pub(super) const fn read_only() -> Self {
        Self {
            editable: false,
            removable: false,
            toggleable: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AgentCommandProviderCapabilities {
    pub discoverable: bool,
    pub creatable: bool,
    pub supported_scopes: Vec<AgentCommandScope>,
    pub writable_formats: Vec<AgentCommandFormat>,
}

pub(super) fn provider_capabilities(
    provider: AgentCommandProvider,
) -> AgentCommandProviderCapabilities {
    match provider {
        AgentCommandProvider::Codex => codex::capabilities(),
        AgentCommandProvider::ClaudeCode => claude::capabilities(),
        AgentCommandProvider::Gemini => gemini::capabilities(),
        AgentCommandProvider::OhMyPi => oh_my_pi::capabilities(),
        AgentCommandProvider::Opencode => opencode::capabilities(),
        AgentCommandProvider::DeepseekHarness => AgentCommandProviderCapabilities {
            discoverable: false,
            creatable: false,
            supported_scopes: Vec::new(),
            writable_formats: Vec::new(),
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentCommandDefinition {
    CodexLegacy {
        description: Option<String>,
        argument_hint: Option<String>,
        body: String,
    },
    ClaudeCode {
        description: Option<String>,
        body: String,
    },
    Gemini {
        description: Option<String>,
        prompt: String,
    },
    OhMyPiPrompt {
        description: Option<String>,
        body: String,
    },
    OhMyPiExecutable {
        entrypoint_configured: bool,
    },
    Opencode {
        description: Option<String>,
        body: String,
    },
    Invalid {
        content_configured: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AgentCommandDefinitionView {
    Opencode {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        body: String,
    },
    CodexLegacy {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        argument_hint: Option<String>,
        body: String,
    },
    ClaudeCode {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        body: String,
    },
    Gemini {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        prompt: String,
    },
    OhMyPiPrompt {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        body: String,
    },
    OhMyPiExecutable {
        entrypoint_configured: bool,
    },
    Invalid {
        content_configured: bool,
    },
}

impl From<AgentCommandDefinition> for AgentCommandDefinitionView {
    fn from(value: AgentCommandDefinition) -> Self {
        match value {
            AgentCommandDefinition::Opencode { description, body } => {
                Self::Opencode { description, body }
            }
            AgentCommandDefinition::CodexLegacy {
                description,
                argument_hint,
                body,
            } => Self::CodexLegacy {
                description,
                argument_hint,
                body,
            },
            AgentCommandDefinition::ClaudeCode { description, body } => {
                Self::ClaudeCode { description, body }
            }
            AgentCommandDefinition::Gemini {
                description,
                prompt,
            } => Self::Gemini {
                description,
                prompt,
            },
            AgentCommandDefinition::OhMyPiPrompt { description, body } => {
                Self::OhMyPiPrompt { description, body }
            }
            AgentCommandDefinition::OhMyPiExecutable {
                entrypoint_configured,
            } => Self::OhMyPiExecutable {
                entrypoint_configured,
            },
            AgentCommandDefinition::Invalid { content_configured } => {
                Self::Invalid { content_configured }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    pub provider: AgentCommandProvider,
    pub scope: AgentCommandScope,
    pub name: String,
    pub state: AgentCommandState,
    pub format: AgentCommandFormat,
    pub capabilities: AgentCommandCapabilities,
    pub revision: String,
    pub(super) definition: AgentCommandDefinition,
    pub(super) native_path: PathBuf,
    pub(super) relative_path: PathBuf,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct AgentCommandView {
    pub installation_id: String,
    pub provider: AgentCommandProvider,
    pub scope: AgentCommandScope,
    pub name: String,
    pub state: AgentCommandState,
    pub format: AgentCommandFormat,
    pub capabilities: AgentCommandCapabilities,
    pub revision: String,
    pub definition: AgentCommandDefinitionView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<AgentCommand> for AgentCommandView {
    fn from(item: AgentCommand) -> Self {
        Self {
            installation_id: installation_id(&item),
            provider: item.provider,
            scope: item.scope,
            name: item.name,
            state: item.state,
            format: item.format,
            capabilities: item.capabilities,
            revision: item.revision,
            definition: item.definition.into(),
            error: item.error,
        }
    }
}

pub(super) fn installation_id(item: &AgentCommand) -> String {
    let mut hasher = Sha256::new();
    hasher.update(item.provider.id().as_bytes());
    hasher.update([0]);
    hasher.update(item.scope.id().as_bytes());
    hasher.update([0]);
    hasher.update(item.name.as_bytes());
    hasher.update([0]);
    hasher.update(item.native_path.to_string_lossy().as_bytes());
    format!("{:x}", hasher.finalize())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct AgentCommandProviderError {
    pub provider: AgentCommandProvider,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct AgentCommandProviderInventoryView {
    pub provider: AgentCommandProvider,
    pub installed: bool,
    pub capabilities: AgentCommandProviderCapabilities,
    pub items: Vec<AgentCommandView>,
    #[serde(default)]
    pub limitations: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct AgentCommandInventoryView {
    pub providers: Vec<AgentCommandProviderInventoryView>,
    #[serde(default)]
    pub errors: Vec<AgentCommandProviderError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AgentCommandLocator {
    pub provider: AgentCommandProvider,
    pub scope: AgentCommandScope,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum CommandTextWrite {
    Preserve,
    Replace { value: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum OptionalCommandTextWrite {
    Preserve,
    Replace { value: String },
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AgentCommandWriteDefinition {
    Opencode {
        description: OptionalCommandTextWrite,
        body: CommandTextWrite,
    },
    CodexLegacy {
        description: OptionalCommandTextWrite,
        argument_hint: OptionalCommandTextWrite,
        body: CommandTextWrite,
    },
    ClaudeCode {
        description: OptionalCommandTextWrite,
        body: CommandTextWrite,
    },
    Gemini {
        description: OptionalCommandTextWrite,
        prompt: CommandTextWrite,
    },
    OhMyPiPrompt {
        description: OptionalCommandTextWrite,
        body: CommandTextWrite,
    },
}

impl AgentCommandWriteDefinition {
    pub(super) const fn provider(&self) -> AgentCommandProvider {
        match self {
            Self::Opencode { .. } => AgentCommandProvider::Opencode,
            Self::CodexLegacy { .. } => AgentCommandProvider::Codex,
            Self::ClaudeCode { .. } => AgentCommandProvider::ClaudeCode,
            Self::Gemini { .. } => AgentCommandProvider::Gemini,
            Self::OhMyPiPrompt { .. } => AgentCommandProvider::OhMyPi,
        }
    }

    pub(super) fn resolve(
        self,
        current: Option<&AgentCommandDefinition>,
    ) -> Result<AgentCommandDefinition, AgentCommandError> {
        match self {
            Self::Opencode { description, body } => {
                let current = current.and_then(|value| match value {
                    AgentCommandDefinition::Opencode { description, body } => {
                        Some((description, body))
                    }
                    _ => None,
                });
                Ok(AgentCommandDefinition::Opencode {
                    description: resolve_optional_text(
                        description,
                        current.map(|(description, _)| description.as_deref()),
                    )?,
                    body: resolve_text(body, current.map(|(_, body)| body.as_str()))?,
                })
            }
            Self::CodexLegacy {
                description,
                argument_hint,
                body,
            } => {
                let current = current.and_then(|value| match value {
                    AgentCommandDefinition::CodexLegacy {
                        description,
                        argument_hint,
                        body,
                    } => Some((description, argument_hint, body)),
                    _ => None,
                });
                Ok(AgentCommandDefinition::CodexLegacy {
                    description: resolve_optional_text(
                        description,
                        current.map(|(description, _, _)| description.as_deref()),
                    )?,
                    argument_hint: resolve_optional_text(
                        argument_hint,
                        current.map(|(_, argument_hint, _)| argument_hint.as_deref()),
                    )?,
                    body: resolve_text(body, current.map(|(_, _, body)| body.as_str()))?,
                })
            }
            Self::ClaudeCode { description, body } => {
                let current = current.and_then(|value| match value {
                    AgentCommandDefinition::ClaudeCode { description, body } => {
                        Some((description, body))
                    }
                    _ => None,
                });
                Ok(AgentCommandDefinition::ClaudeCode {
                    description: resolve_optional_text(
                        description,
                        current.map(|(description, _)| description.as_deref()),
                    )?,
                    body: resolve_text(body, current.map(|(_, body)| body.as_str()))?,
                })
            }
            Self::Gemini {
                description,
                prompt,
            } => {
                let current = current.and_then(|value| match value {
                    AgentCommandDefinition::Gemini {
                        description,
                        prompt,
                    } => Some((description, prompt)),
                    _ => None,
                });
                Ok(AgentCommandDefinition::Gemini {
                    description: resolve_optional_text(
                        description,
                        current.map(|(description, _)| description.as_deref()),
                    )?,
                    prompt: resolve_text(prompt, current.map(|(_, prompt)| prompt.as_str()))?,
                })
            }
            Self::OhMyPiPrompt { description, body } => {
                let current = current.and_then(|value| match value {
                    AgentCommandDefinition::OhMyPiPrompt { description, body } => {
                        Some((description, body))
                    }
                    _ => None,
                });
                Ok(AgentCommandDefinition::OhMyPiPrompt {
                    description: resolve_optional_text(
                        description,
                        current.map(|(description, _)| description.as_deref()),
                    )?,
                    body: resolve_text(body, current.map(|(_, body)| body.as_str()))?,
                })
            }
        }
    }
}

fn resolve_text(
    operation: CommandTextWrite,
    current: Option<&str>,
) -> Result<String, AgentCommandError> {
    match operation {
        CommandTextWrite::Preserve => current.map(str::to_owned).ok_or_else(|| {
            AgentCommandError::InvalidRequest(
                "new commands must explicitly replace their prompt body".into(),
            )
        }),
        CommandTextWrite::Replace { value } => Ok(value),
    }
}

fn resolve_optional_text(
    operation: OptionalCommandTextWrite,
    current: Option<Option<&str>>,
) -> Result<Option<String>, AgentCommandError> {
    match operation {
        OptionalCommandTextWrite::Preserve => current
            .map(|value| value.map(str::to_owned))
            .ok_or_else(|| {
                AgentCommandError::InvalidRequest(
                    "new commands must explicitly replace or clear their description".into(),
                )
            }),
        OptionalCommandTextWrite::Replace { value } => Ok(Some(value)),
        OptionalCommandTextWrite::Clear => Ok(None),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CreateAgentCommandRequest {
    pub target: AgentCommandLocator,
    pub definition: AgentCommandWriteDefinition,
    #[serde(default)]
    pub replace: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct UpdateAgentCommandRequest {
    pub target: AgentCommandLocator,
    pub expected_revision: String,
    pub definition: AgentCommandWriteDefinition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RemoveAgentCommandRequest {
    pub target: AgentCommandLocator,
    pub expected_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ToggleAgentCommandRequest {
    pub target: AgentCommandLocator,
    pub expected_revision: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandErrorCode {
    InvalidRequest,
    InvalidConfiguration,
    NotFound,
    Collision,
    StaleRevision,
    Unsupported,
    UnsafePath,
    Io,
    VerificationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AgentCommandOperationError {
    pub code: AgentCommandErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentCommandProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Error)]
pub enum AgentCommandError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("invalid command configuration: {0}")]
    InvalidConfiguration(String),
    #[error("command not found: {0}")]
    NotFound(String),
    #[error("command already exists: {0}")]
    Collision(String),
    #[error("command changed outside Vibe Studio; refresh and retry")]
    StaleRevision,
    #[error("unsupported command operation: {0}")]
    Unsupported(String),
    #[error("unsafe path: {0}")]
    UnsafePath(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("write verification failed: {0}")]
    VerificationFailed(String),
}

impl AgentCommandError {
    pub fn operation_error(
        &self,
        provider: Option<AgentCommandProvider>,
        name: Option<String>,
    ) -> AgentCommandOperationError {
        let code = match self {
            Self::InvalidRequest(_) => AgentCommandErrorCode::InvalidRequest,
            Self::InvalidConfiguration(_) => AgentCommandErrorCode::InvalidConfiguration,
            Self::NotFound(_) => AgentCommandErrorCode::NotFound,
            Self::Collision(_) => AgentCommandErrorCode::Collision,
            Self::StaleRevision => AgentCommandErrorCode::StaleRevision,
            Self::Unsupported(_) => AgentCommandErrorCode::Unsupported,
            Self::UnsafePath(_) => AgentCommandErrorCode::UnsafePath,
            Self::Io(_) => AgentCommandErrorCode::Io,
            Self::VerificationFailed(_) => AgentCommandErrorCode::VerificationFailed,
        };
        AgentCommandOperationError {
            code,
            message: self.to_string(),
            provider,
            name,
        }
    }
}
