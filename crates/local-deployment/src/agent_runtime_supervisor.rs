use std::process::ExitStatus;

use db::models::execution_process::ExecutionProcessStatus;
use executors::{
    approvals::ExecutorApprovalError,
    executors::{ExecutorError, ExecutorExitResult},
    runtime::{
        AgentRunLifecycle, AgentRunStatus, AgentRuntimeError, AgentRuntimeErrorKind,
        AgentRuntimeLaunchPhase,
    },
};
use uuid::Uuid;

const EXECUTOR_SIGNAL_SUCCESS_EXIT_CODE: i64 = 0;
const EXECUTOR_SIGNAL_FAILURE_EXIT_CODE: i64 = 1;

/// Platform identity is kept separate from process handles and observation.
/// It remains stable when a supervisor is restarted or a RunAttempt is
/// re-attached by another service instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentRuntimeIdentity {
    pub session_id: Uuid,
    pub agent_run_id: Uuid,
    pub turn_id: Uuid,
    pub run_attempt_id: Uuid,
    pub correlation_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRuntimeCoreState {
    pub identity: AgentRuntimeIdentity,
    pub lifecycle: AgentRunLifecycle,
    pub status: AgentRunStatus,
    pub terminal_decision: Option<TerminalDecision>,
}

impl AgentRuntimeCoreState {
    pub(crate) fn new(identity: AgentRuntimeIdentity) -> Self {
        Self {
            identity,
            lifecycle: AgentRunLifecycle::Starting,
            status: AgentRunStatus::Starting,
            terminal_decision: None,
        }
    }

    pub(crate) fn is_terminal(&self) -> bool {
        self.terminal_decision.is_some() || self.status.is_terminal()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalDecision {
    Succeeded,
    Failed,
    Cancelled,
    Crashed,
    AuditFailed,
}

impl TerminalDecision {
    pub(crate) fn status(self) -> AgentRunStatus {
        match self {
            Self::Succeeded => AgentRunStatus::Succeeded,
            Self::Failed => AgentRunStatus::Failed,
            Self::Cancelled => AgentRunStatus::Cancelled,
            Self::Crashed => AgentRunStatus::Crashed,
            Self::AuditFailed => AgentRunStatus::AuditFailed,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AgentRuntimeSupervisorOutcome {
    pub lifecycle: AgentRunLifecycle,
    pub terminal_decision: Option<TerminalDecision>,
    pub process_status: Option<ExecutionProcessStatus>,
    pub exit_code: Option<i64>,
    pub runtime_error: Option<AgentRuntimeError>,
}

pub(crate) fn classify_starting() -> AgentRuntimeSupervisorOutcome {
    in_flight(AgentRunLifecycle::Starting)
}

pub(crate) fn classify_running() -> AgentRuntimeSupervisorOutcome {
    in_flight(AgentRunLifecycle::Running)
}

pub(crate) fn classify_cancellation_requested() -> AgentRuntimeSupervisorOutcome {
    in_flight(AgentRunLifecycle::Cancelling)
}

pub(crate) fn classify_launch_timeout() -> AgentRuntimeSupervisorOutcome {
    terminal(
        AgentRunLifecycle::Failed,
        ExecutionProcessStatus::Failed,
        None,
        Some(
            runtime_error(
                AgentRuntimeErrorKind::StartupFailed,
                "agent startup timed out before the provider accepted the run",
                None,
            )
            .with_launch_phase(Some(AgentRuntimeLaunchPhase::Warmup)),
        ),
    )
}

pub(crate) fn classify_launch_executor_error(
    error: &ExecutorError,
    provider: Option<&str>,
) -> AgentRuntimeSupervisorOutcome {
    terminal(
        AgentRunLifecycle::Failed,
        ExecutionProcessStatus::Failed,
        None,
        Some(
            AgentRuntimeError::from_executor_error(error, provider)
                .with_launch_phase(Some(launch_phase_for_executor_error(error))),
        ),
    )
}

pub(crate) fn classify_executor_exit_result(
    result: ExecutorExitResult,
) -> AgentRuntimeSupervisorOutcome {
    match result {
        ExecutorExitResult::Success => terminal(
            AgentRunLifecycle::Completed,
            ExecutionProcessStatus::Completed,
            Some(EXECUTOR_SIGNAL_SUCCESS_EXIT_CODE),
            None,
        ),
        ExecutorExitResult::Failure => {
            let exit_code = Some(EXECUTOR_SIGNAL_FAILURE_EXIT_CODE);
            terminal(
                AgentRunLifecycle::Failed,
                ExecutionProcessStatus::Failed,
                exit_code,
                Some(runtime_error(
                    AgentRuntimeErrorKind::Unknown,
                    "executor reported failure",
                    exit_code,
                )),
            )
        }
    }
}

pub(crate) fn classify_executor_exit_channel_closed() -> AgentRuntimeSupervisorOutcome {
    terminal(
        AgentRunLifecycle::Crashed,
        ExecutionProcessStatus::Failed,
        None,
        Some(runtime_error(
            AgentRuntimeErrorKind::ProcessCrashed,
            "executor exit signal channel closed before reporting an outcome",
            None,
        )),
    )
}

pub(crate) fn classify_process_exit(exit_status: ExitStatus) -> AgentRuntimeSupervisorOutcome {
    let exit_code = Some(i64::from(exit_status.code().unwrap_or(-1)));

    if exit_status.success() {
        terminal(
            AgentRunLifecycle::Completed,
            ExecutionProcessStatus::Completed,
            exit_code,
            None,
        )
    } else {
        terminal(
            AgentRunLifecycle::Failed,
            ExecutionProcessStatus::Failed,
            exit_code,
            Some(runtime_error(
                AgentRuntimeErrorKind::Unknown,
                "agent process exited unsuccessfully",
                exit_code,
            )),
        )
    }
}

pub(crate) fn classify_watcher_error(error: impl Into<String>) -> AgentRuntimeSupervisorOutcome {
    let message = error.into();
    terminal(
        AgentRunLifecycle::Crashed,
        ExecutionProcessStatus::Failed,
        None,
        Some(runtime_error(
            AgentRuntimeErrorKind::ProcessCrashed,
            format!("agent process watcher failed: {message}"),
            None,
        )),
    )
}

fn in_flight(lifecycle: AgentRunLifecycle) -> AgentRuntimeSupervisorOutcome {
    AgentRuntimeSupervisorOutcome {
        lifecycle,
        terminal_decision: None,
        process_status: None,
        exit_code: None,
        runtime_error: None,
    }
}

fn terminal(
    lifecycle: AgentRunLifecycle,
    process_status: ExecutionProcessStatus,
    exit_code: Option<i64>,
    runtime_error: Option<AgentRuntimeError>,
) -> AgentRuntimeSupervisorOutcome {
    AgentRuntimeSupervisorOutcome {
        lifecycle,
        terminal_decision: Some(terminal_decision_for_lifecycle(lifecycle)),
        process_status: Some(process_status),
        exit_code,
        runtime_error,
    }
}

fn terminal_decision_for_lifecycle(lifecycle: AgentRunLifecycle) -> TerminalDecision {
    match lifecycle {
        AgentRunLifecycle::Completed => TerminalDecision::Succeeded,
        AgentRunLifecycle::Cancelling => TerminalDecision::Cancelled,
        AgentRunLifecycle::Crashed => TerminalDecision::Crashed,
        AgentRunLifecycle::Failed => TerminalDecision::Failed,
        AgentRunLifecycle::Starting
        | AgentRunLifecycle::Running
        | AgentRunLifecycle::WaitingApproval
        | AgentRunLifecycle::WaitingInput => TerminalDecision::Failed,
    }
}

fn runtime_error(
    kind: AgentRuntimeErrorKind,
    message: impl Into<String>,
    exit_code: Option<i64>,
) -> AgentRuntimeError {
    let exit_code = exit_code.and_then(|code| i32::try_from(code).ok());
    AgentRuntimeError::new(kind, message).with_exit_code(exit_code)
}

fn launch_phase_for_executor_error(error: &ExecutorError) -> AgentRuntimeLaunchPhase {
    match error {
        ExecutorError::CommandBuild(_) | ExecutorError::InvalidConfiguration(_) => {
            AgentRuntimeLaunchPhase::CommandBuild
        }
        ExecutorError::ExecutorApprovalError(approval_error) => {
            launch_phase_for_approval_error(approval_error)
        }
        ExecutorError::Json(_) => AgentRuntimeLaunchPhase::ProtocolConnect,
        ExecutorError::AuthRequired(_) => AgentRuntimeLaunchPhase::Warmup,
        ExecutorError::FollowUpNotSupported(_) | ExecutorError::ResetToMessageNotSupported(_) => {
            AgentRuntimeLaunchPhase::SessionResume
        }
        ExecutorError::ExecutableNotFound { .. }
        | ExecutorError::UnknownExecutorType(_)
        | ExecutorError::SetupHelperNotSupported
        | ExecutorError::SpawnError(_)
        | ExecutorError::TomlSerialize(_)
        | ExecutorError::TomlDeserialize(_)
        | ExecutorError::Io(_) => AgentRuntimeLaunchPhase::ProcessSpawn,
    }
}

fn launch_phase_for_approval_error(error: &ExecutorApprovalError) -> AgentRuntimeLaunchPhase {
    match error {
        ExecutorApprovalError::Cancelled => AgentRuntimeLaunchPhase::Warmup,
        ExecutorApprovalError::SessionNotRegistered
        | ExecutorApprovalError::RequestFailed(_)
        | ExecutorApprovalError::ServiceUnavailable => AgentRuntimeLaunchPhase::ProtocolConnect,
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use executors::command::CommandBuildError;
    use futures::io::Error as FuturesIoError;

    use super::*;
    use crate::process_supervisor::{
        ProcessObservation, ReconciliationAction, reconciliation_action,
    };

    fn success_status() -> ExitStatus {
        exit_status(0)
    }

    fn failure_status() -> ExitStatus {
        exit_status(2)
    }

    fn exit_status(code: i32) -> ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;

            ExitStatusExt::from_raw(code << 8)
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;

            ExitStatusExt::from_raw(code as u32)
        }
    }

    fn assert_launch_error(
        outcome: AgentRuntimeSupervisorOutcome,
        kind: AgentRuntimeErrorKind,
        launch_phase: AgentRuntimeLaunchPhase,
    ) {
        assert_eq!(outcome.lifecycle, AgentRunLifecycle::Failed);
        assert_eq!(outcome.process_status, Some(ExecutionProcessStatus::Failed));
        assert_eq!(outcome.exit_code, None);

        let runtime_error = outcome.runtime_error.expect("runtime error");
        assert_eq!(runtime_error.kind, kind);
        assert_eq!(runtime_error.launch_phase, Some(launch_phase));
    }

    #[test]
    fn classifies_executor_success_as_completed() {
        let outcome = classify_executor_exit_result(ExecutorExitResult::Success);

        assert_eq!(outcome.lifecycle, AgentRunLifecycle::Completed);
        assert_eq!(
            outcome.process_status,
            Some(ExecutionProcessStatus::Completed)
        );
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(outcome.runtime_error, None);
    }

    #[test]
    fn classifies_executor_failure_as_failed() {
        let outcome = classify_executor_exit_result(ExecutorExitResult::Failure);

        assert_eq!(outcome.lifecycle, AgentRunLifecycle::Failed);
        assert_eq!(outcome.process_status, Some(ExecutionProcessStatus::Failed));
        assert_eq!(outcome.exit_code, Some(1));
        assert_eq!(
            outcome.runtime_error.as_ref().map(|error| error.kind),
            Some(AgentRuntimeErrorKind::Unknown)
        );
    }

    #[test]
    fn classifies_closed_executor_channel_as_crashed() {
        let outcome = classify_executor_exit_channel_closed();

        assert_eq!(outcome.lifecycle, AgentRunLifecycle::Crashed);
        assert_eq!(outcome.process_status, Some(ExecutionProcessStatus::Failed));
        assert_eq!(outcome.exit_code, None);
        let runtime_error = outcome.runtime_error.expect("runtime error");
        assert_eq!(runtime_error.kind, AgentRuntimeErrorKind::ProcessCrashed);
        assert_eq!(runtime_error.exit_code, None);
        assert_eq!(runtime_error.launch_phase, None);
    }

    #[test]
    fn classifies_os_success_as_completed() {
        let outcome = classify_process_exit(success_status());

        assert_eq!(outcome.lifecycle, AgentRunLifecycle::Completed);
        assert_eq!(
            outcome.process_status,
            Some(ExecutionProcessStatus::Completed)
        );
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(outcome.runtime_error, None);
    }

    #[test]
    fn classifies_os_non_zero_as_failed() {
        let outcome = classify_process_exit(failure_status());

        assert_eq!(outcome.lifecycle, AgentRunLifecycle::Failed);
        assert_eq!(outcome.process_status, Some(ExecutionProcessStatus::Failed));
        assert_eq!(outcome.exit_code, Some(2));
        assert_eq!(
            outcome.runtime_error.as_ref().map(|error| error.kind),
            Some(AgentRuntimeErrorKind::Unknown)
        );
    }

    #[test]
    fn classifies_watcher_error_as_crashed() {
        let outcome = classify_watcher_error("child handle missing");

        assert_eq!(outcome.lifecycle, AgentRunLifecycle::Crashed);
        assert_eq!(outcome.process_status, Some(ExecutionProcessStatus::Failed));
        assert_eq!(outcome.exit_code, None);
        assert_eq!(
            outcome.runtime_error.as_ref().map(|error| error.kind),
            Some(AgentRuntimeErrorKind::ProcessCrashed)
        );
    }

    #[test]
    fn classifies_cancellation_request_as_in_flight_cancelling() {
        let outcome = classify_cancellation_requested();

        assert_eq!(outcome.lifecycle, AgentRunLifecycle::Cancelling);
        assert_eq!(outcome.process_status, None);
        assert_eq!(outcome.exit_code, None);
        assert_eq!(outcome.runtime_error, None);
    }

    #[test]
    fn classifies_launch_timeout_as_warmup_failure() {
        assert_launch_error(
            classify_launch_timeout(),
            AgentRuntimeErrorKind::StartupFailed,
            AgentRuntimeLaunchPhase::Warmup,
        );
    }

    #[test]
    fn classifies_command_build_failure_phase() {
        assert_launch_error(
            classify_launch_executor_error(
                &ExecutorError::CommandBuild(CommandBuildError::EmptyCommand),
                Some("codex"),
            ),
            AgentRuntimeErrorKind::CommandBuildFailed,
            AgentRuntimeLaunchPhase::CommandBuild,
        );
    }

    #[test]
    fn rejects_invalid_configuration_before_process_spawn() {
        assert_launch_error(
            classify_launch_executor_error(
                &ExecutorError::InvalidConfiguration("unsupported effort".into()),
                Some("codex"),
            ),
            AgentRuntimeErrorKind::StartupFailed,
            AgentRuntimeLaunchPhase::CommandBuild,
        );
    }

    #[test]
    fn classifies_executable_missing_as_process_spawn_phase() {
        assert_launch_error(
            classify_launch_executor_error(
                &ExecutorError::ExecutableNotFound {
                    program: "codex".to_string(),
                },
                Some("codex"),
            ),
            AgentRuntimeErrorKind::ExecutableNotFound,
            AgentRuntimeLaunchPhase::ProcessSpawn,
        );
    }

    #[test]
    fn classifies_auth_required_as_warmup_phase() {
        assert_launch_error(
            classify_launch_executor_error(
                &ExecutorError::AuthRequired("login required".to_string()),
                Some("codex"),
            ),
            AgentRuntimeErrorKind::AuthRequired,
            AgentRuntimeLaunchPhase::Warmup,
        );
    }

    #[test]
    fn classifies_spawn_error_as_process_spawn_phase() {
        assert_launch_error(
            classify_launch_executor_error(
                &ExecutorError::SpawnError(FuturesIoError::new(
                    io::ErrorKind::NotFound,
                    "spawn failed",
                )),
                Some("codex"),
            ),
            AgentRuntimeErrorKind::StartupFailed,
            AgentRuntimeLaunchPhase::ProcessSpawn,
        );
    }

    #[test]
    fn classifies_protocol_like_approval_error_phase() {
        assert_launch_error(
            classify_launch_executor_error(
                &ExecutorError::ExecutorApprovalError(ExecutorApprovalError::ServiceUnavailable),
                Some("codex"),
            ),
            AgentRuntimeErrorKind::ProtocolFailed,
            AgentRuntimeLaunchPhase::ProtocolConnect,
        );
    }

    #[test]
    fn classifies_follow_up_unsupported_as_session_resume_phase() {
        assert_launch_error(
            classify_launch_executor_error(
                &ExecutorError::FollowUpNotSupported("codex".to_string()),
                Some("codex"),
            ),
            AgentRuntimeErrorKind::StartupFailed,
            AgentRuntimeLaunchPhase::SessionResume,
        );

        assert_launch_error(
            classify_launch_executor_error(
                &ExecutorError::ResetToMessageNotSupported("omp".to_string()),
                Some("omp"),
            ),
            AgentRuntimeErrorKind::StartupFailed,
            AgentRuntimeLaunchPhase::SessionResume,
        );
    }

    #[test]
    fn restart_reconciliation_attaches_live_handle_without_terminal_decision() {
        assert_eq!(
            reconciliation_action(ProcessObservation::Alive, true),
            ReconciliationAction::Attach
        );
        assert_eq!(
            reconciliation_action(ProcessObservation::Alive, false),
            ReconciliationAction::ObserveReadOnly
        );
        assert_eq!(
            reconciliation_action(ProcessObservation::TemporarilyUnreachable, false),
            ReconciliationAction::PreserveForRetry
        );
    }

    #[test]
    fn runtime_core_terminal_decision_is_independent_of_process_status() {
        let identity = AgentRuntimeIdentity {
            session_id: Uuid::new_v4(),
            agent_run_id: Uuid::new_v4(),
            turn_id: Uuid::new_v4(),
            run_attempt_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
        };
        let mut state = AgentRuntimeCoreState::new(identity);
        assert!(!state.is_terminal());
        state.terminal_decision = Some(TerminalDecision::Cancelled);
        assert!(state.is_terminal());
        assert_eq!(
            TerminalDecision::Cancelled.status(),
            executors::runtime::AgentRunStatus::Cancelled
        );
    }
}
