use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::launch_phase::AgentRuntimeLaunchPhase;
use crate::{approvals::ExecutorApprovalError, executors::ExecutorError};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum AgentRuntimeErrorKind {
    ExecutableNotFound,
    CommandBuildFailed,
    AuthRequired,
    StartupFailed,
    ProtocolFailed,
    ProcessCrashed,
    OutputParseFailed,
    ApprovalCancelled,
    ApprovalTimedOut,
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct AgentRuntimeError {
    pub kind: AgentRuntimeErrorKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_phase: Option<AgentRuntimeLaunchPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl AgentRuntimeError {
    pub fn new(kind: AgentRuntimeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            provider: None,
            launch_phase: None,
            exit_code: None,
        }
    }

    pub fn with_provider(mut self, provider: Option<impl Into<String>>) -> Self {
        self.provider = provider.map(Into::into);
        self
    }

    pub fn with_launch_phase(mut self, launch_phase: Option<AgentRuntimeLaunchPhase>) -> Self {
        self.launch_phase = launch_phase;
        self
    }

    pub fn with_exit_code(mut self, exit_code: Option<i32>) -> Self {
        self.exit_code = exit_code;
        self
    }

    pub fn from_executor_error(error: &ExecutorError, provider: Option<&str>) -> Self {
        Self::new(classify_executor_error(error), error.to_string())
            .with_provider(provider.map(str::to_string))
    }
}

pub fn classify_executor_error(error: &ExecutorError) -> AgentRuntimeErrorKind {
    match error {
        ExecutorError::ExecutableNotFound { .. } => AgentRuntimeErrorKind::ExecutableNotFound,
        ExecutorError::CommandBuild(_) => AgentRuntimeErrorKind::CommandBuildFailed,
        ExecutorError::AuthRequired(_) => AgentRuntimeErrorKind::AuthRequired,
        ExecutorError::Json(_) => AgentRuntimeErrorKind::OutputParseFailed,
        ExecutorError::ExecutorApprovalError(approval_error) => {
            classify_approval_error(approval_error)
        }
        ExecutorError::UnknownExecutorType(_)
        | ExecutorError::InvalidConfiguration(_)
        | ExecutorError::SetupHelperNotSupported
        | ExecutorError::FollowUpNotSupported(_)
        | ExecutorError::ResetToMessageNotSupported(_)
        | ExecutorError::SpawnError(_)
        | ExecutorError::TomlSerialize(_)
        | ExecutorError::TomlDeserialize(_) => AgentRuntimeErrorKind::StartupFailed,
        ExecutorError::Io(_) => AgentRuntimeErrorKind::Unknown,
    }
}

fn classify_approval_error(error: &ExecutorApprovalError) -> AgentRuntimeErrorKind {
    match error {
        ExecutorApprovalError::Cancelled => AgentRuntimeErrorKind::ApprovalCancelled,
        ExecutorApprovalError::SessionNotRegistered
        | ExecutorApprovalError::RequestFailed(_)
        | ExecutorApprovalError::ServiceUnavailable => AgentRuntimeErrorKind::ProtocolFailed,
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use futures_io::Error as FuturesIoError;

    use super::*;
    use crate::command::CommandBuildError;

    fn mapped_kind(error: ExecutorError) -> AgentRuntimeErrorKind {
        AgentRuntimeError::from_executor_error(&error, None).kind
    }

    #[test]
    fn maps_executable_not_found() {
        assert_eq!(
            mapped_kind(ExecutorError::ExecutableNotFound {
                program: "codex".to_string()
            }),
            AgentRuntimeErrorKind::ExecutableNotFound
        );
    }

    #[test]
    fn maps_command_build_failure() {
        assert_eq!(
            mapped_kind(ExecutorError::CommandBuild(CommandBuildError::EmptyCommand)),
            AgentRuntimeErrorKind::CommandBuildFailed
        );
    }

    #[test]
    fn maps_auth_required() {
        assert_eq!(
            mapped_kind(ExecutorError::AuthRequired("login first".to_string())),
            AgentRuntimeErrorKind::AuthRequired
        );
    }

    #[test]
    fn maps_json_parse_failure() {
        let json_error =
            serde_json::from_str::<serde_json::Value>("{").expect_err("invalid json should fail");

        assert_eq!(
            mapped_kind(ExecutorError::Json(json_error)),
            AgentRuntimeErrorKind::OutputParseFailed
        );
    }

    #[test]
    fn maps_approval_cancelled() {
        assert_eq!(
            mapped_kind(ExecutorError::ExecutorApprovalError(
                ExecutorApprovalError::Cancelled
            )),
            AgentRuntimeErrorKind::ApprovalCancelled
        );
    }

    #[test]
    fn maps_approval_service_failures_to_protocol_failed() {
        let cases = [
            ExecutorApprovalError::SessionNotRegistered,
            ExecutorApprovalError::RequestFailed("request failed".to_string()),
            ExecutorApprovalError::ServiceUnavailable,
        ];

        for error in cases {
            assert_eq!(
                mapped_kind(ExecutorError::ExecutorApprovalError(error)),
                AgentRuntimeErrorKind::ProtocolFailed
            );
        }
    }

    #[test]
    fn maps_startup_failures() {
        let cases = [
            ExecutorError::UnknownExecutorType("unknown".to_string()),
            ExecutorError::InvalidConfiguration("unsupported model effort".to_string()),
            ExecutorError::SetupHelperNotSupported,
            ExecutorError::FollowUpNotSupported("provider".to_string()),
            ExecutorError::ResetToMessageNotSupported("provider".to_string()),
            ExecutorError::SpawnError(FuturesIoError::new(io::ErrorKind::NotFound, "spawn failed")),
        ];

        for error in cases {
            assert_eq!(mapped_kind(error), AgentRuntimeErrorKind::StartupFailed);
        }
    }

    #[test]
    fn maps_io_conservatively_to_unknown() {
        assert_eq!(
            mapped_kind(ExecutorError::Io(io::Error::other("ambiguous io"))),
            AgentRuntimeErrorKind::Unknown
        );
    }

    #[test]
    fn preserves_provider_context() {
        let runtime_error = AgentRuntimeError::from_executor_error(
            &ExecutorError::AuthRequired("login first".to_string()),
            Some("codex"),
        );

        assert_eq!(runtime_error.provider.as_deref(), Some("codex"));
        assert_eq!(runtime_error.launch_phase, None);
        assert_eq!(runtime_error.exit_code, None);
    }

    #[test]
    fn preserves_launch_phase_context() {
        let runtime_error = AgentRuntimeError::new(
            AgentRuntimeErrorKind::StartupFailed,
            "agent startup timed out",
        )
        .with_launch_phase(Some(AgentRuntimeLaunchPhase::Warmup));

        assert_eq!(
            runtime_error.launch_phase,
            Some(AgentRuntimeLaunchPhase::Warmup)
        );
    }
}
