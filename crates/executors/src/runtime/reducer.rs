use super::{
    AgentEventEnvelope, AgentEventPayload, ContractVersionError, ProjectionStatus,
    RUN_STATE_REDUCER_VERSION, RUN_STATE_SCHEMA_VERSION, RunState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReducerApply {
    Applied,
    AppliedDegraded,
    Duplicate,
    IgnoredTerminalRegression,
}

#[derive(Debug, thiserror::Error)]
pub enum ReducerError {
    #[error(transparent)]
    EventVersion(#[from] ContractVersionError),
    #[error("event identity does not belong to this run state")]
    IdentityMismatch,
    #[error("run attempt number and identity conflict with the projection cursor")]
    RunAttemptConflict,
    #[error("run attempt number and event sequence must both be greater than zero")]
    InvalidStreamPosition,
    #[error("event sequence {received} conflicts with last applied sequence {last_applied}")]
    SequenceConflict { received: u64, last_applied: u64 },
    #[error("state schema version {0} is not supported")]
    UnsupportedStateSchema(u16),
    #[error("reducer version {0} is not supported")]
    UnsupportedReducerVersion(u16),
}

pub fn reduce_agent_event(
    state: &mut RunState,
    event: &AgentEventEnvelope,
) -> Result<ReducerApply, ReducerError> {
    validate_state(state)?;
    if let Err(error) = event.validate_for_projection() {
        state.projection_status = ProjectionStatus::ProjectionDegraded;
        return Err(error.into());
    }
    if state.session_id != event.session_id
        || state.agent_run_id != event.agent_run_id
        || state.turn_id != event.turn_id
    {
        return Err(ReducerError::IdentityMismatch);
    }
    if event.run_attempt_number == 0 || event.sequence == 0 {
        state.projection_status = ProjectionStatus::ProjectionDegraded;
        return Err(ReducerError::InvalidStreamPosition);
    }

    let is_new_attempt = match state.last_run_attempt_id {
        None => true,
        Some(last_attempt_id) => {
            if event.run_attempt_number < state.last_run_attempt_number {
                state.projection_status = ProjectionStatus::ProjectionDegraded;
                return Err(ReducerError::RunAttemptConflict);
            }
            if event.run_attempt_number == state.last_run_attempt_number {
                if event.run_attempt_id != last_attempt_id {
                    state.projection_status = ProjectionStatus::ProjectionDegraded;
                    return Err(ReducerError::RunAttemptConflict);
                }
                false
            } else {
                true
            }
        }
    };

    if !is_new_attempt && event.sequence <= state.last_event_sequence {
        return if event.sequence == state.last_event_sequence
            && state.last_event_id == Some(event.event_id)
        {
            Ok(ReducerApply::Duplicate)
        } else {
            state.projection_status = ProjectionStatus::ProjectionDegraded;
            Err(ReducerError::SequenceConflict {
                received: event.sequence,
                last_applied: state.last_event_sequence,
            })
        };
    }

    let had_gap = if is_new_attempt {
        event.sequence != 1
            || (state.last_run_attempt_number != 0
                && event.run_attempt_number != state.last_run_attempt_number + 1)
    } else {
        event.sequence != state.last_event_sequence + 1
    };
    if had_gap {
        state.projection_status = ProjectionStatus::ProjectionDegraded;
    }

    if is_new_attempt {
        state.terminal_output = None;
        state.last_error = None;
        state.observed_configuration = None;
    }

    let apply = match &event.payload {
        AgentEventPayload::LifecycleChanged { status } => {
            if !is_new_attempt && state.status.is_terminal() {
                ReducerApply::IgnoredTerminalRegression
            } else {
                state.status = *status;
                ReducerApply::Applied
            }
        }
        AgentEventPayload::SessionObserved { provider_session } => {
            state.provider_session = Some(provider_session.clone());
            ReducerApply::Applied
        }
        AgentEventPayload::Message {
            message,
            final_output,
        } => {
            if *final_output {
                state.terminal_output = Some(message.clone());
            }
            ReducerApply::Applied
        }
        AgentEventPayload::Error { error } => {
            state.last_error = Some(error.clone());
            ReducerApply::Applied
        }
        AgentEventPayload::ProjectionDegraded { .. } => {
            state.projection_status = ProjectionStatus::ProjectionDegraded;
            ReducerApply::AppliedDegraded
        }
        AgentEventPayload::Unknown { .. } => {
            state.unknown_event_count += 1;
            state.projection_status = ProjectionStatus::ProjectionDegraded;
            ReducerApply::AppliedDegraded
        }
        AgentEventPayload::ProviderExtension {
            provider_event,
            payload,
            ..
        } if provider_event == "runtime_configuration" => {
            // Only the provider boundary emits this allowlisted observation.
            // Missing values remain unconfirmed; never backfill from a request.
            let model_id = payload
                .get("model_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            let reasoning_id = payload
                .get("reasoning_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            if (model_id.is_some() || reasoning_id.is_some())
                && let Some(source) = payload.get("source").and_then(serde_json::Value::as_str)
            {
                state.observed_configuration = Some(super::ObservedRuntimeConfiguration {
                    model_id,
                    reasoning_id,
                    source: source.to_owned(),
                    event_id: event.event_id,
                    run_attempt_id: event.run_attempt_id,
                });
            }
            ReducerApply::Applied
        }
        AgentEventPayload::Thinking { .. }
        | AgentEventPayload::FileChanges { .. }
        | AgentEventPayload::ToolCall { .. }
        | AgentEventPayload::ApprovalRequested { .. }
        | AgentEventPayload::ApprovalResolved { .. }
        | AgentEventPayload::InputRequested { .. }
        | AgentEventPayload::InputResolved { .. }
        | AgentEventPayload::TokenUsage { .. }
        | AgentEventPayload::ProviderExtension { .. } => ReducerApply::Applied,
    };

    state.last_run_attempt_id = Some(event.run_attempt_id);
    state.last_run_attempt_number = event.run_attempt_number;
    state.last_event_sequence = event.sequence;
    state.last_event_id = Some(event.event_id);
    state.updated_at = event.timestamp;

    if had_gap || state.projection_status == ProjectionStatus::ProjectionDegraded {
        Ok(match apply {
            ReducerApply::Duplicate | ReducerApply::IgnoredTerminalRegression => apply,
            _ => ReducerApply::AppliedDegraded,
        })
    } else {
        Ok(apply)
    }
}

fn validate_state(state: &RunState) -> Result<(), ReducerError> {
    if state.state_schema_version != RUN_STATE_SCHEMA_VERSION {
        return Err(ReducerError::UnsupportedStateSchema(
            state.state_schema_version,
        ));
    }
    if state.reducer_version != RUN_STATE_REDUCER_VERSION {
        return Err(ReducerError::UnsupportedReducerVersion(
            state.reducer_version,
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::runtime::{
        AGENT_EVENT_PAYLOAD_VERSION, AGENT_EVENT_SCHEMA_VERSION, AGENT_REQUEST_PAYLOAD_VERSION,
        AGENT_REQUEST_SCHEMA_VERSION, AgentRunIntent, AgentRunRequestEnvelope, AgentRunStatus,
        CanonicalMessage, WorkspaceMode, WorkspaceReference,
    };

    fn request() -> AgentRunRequestEnvelope {
        AgentRunRequestEnvelope {
            schema_version: AGENT_REQUEST_SCHEMA_VERSION,
            payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
            request_id: Uuid::new_v4(),
            idempotency_key: "run-command-1".to_string(),
            session_id: Uuid::new_v4(),
            agent_run_id: Uuid::new_v4(),
            turn_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            intent: AgentRunIntent::Initial,
            runtime_profile_id: "codex:default".to_string(),
            provider_id: "codex".to_string(),
            workspace: WorkspaceReference {
                workspace_id: Uuid::new_v4(),
                mode: WorkspaceMode::SharedWorkspace,
                path: "C:/workspace".to_string(),
            },
            input: CanonicalMessage {
                message_id: Uuid::new_v4(),
                role: crate::runtime::AgentRuntimeMessageRole::User,
                content: "Implement the task".to_string(),
            },
            created_at: Utc::now(),
        }
    }

    fn event(
        request: &AgentRunRequestEnvelope,
        run_attempt_id: Uuid,
        run_attempt_number: u32,
        sequence: u64,
        payload: AgentEventPayload,
    ) -> AgentEventEnvelope {
        AgentEventEnvelope {
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload_version: AGENT_EVENT_PAYLOAD_VERSION,
            event_id: Uuid::new_v4(),
            session_id: request.session_id,
            agent_run_id: request.agent_run_id,
            turn_id: request.turn_id,
            run_attempt_id,
            run_attempt_number,
            sequence,
            correlation_id: request.correlation_id,
            orchestration_run_id: None,
            orchestration_node_execution_id: None,
            timestamp: Utc::now(),
            native_refs: Vec::new(),
            payload,
        }
    }

    fn configuration_event(
        request: &AgentRunRequestEnvelope,
        attempt_id: Uuid,
        sequence: u64,
        payload: serde_json::Value,
    ) -> AgentEventEnvelope {
        event(
            request,
            attempt_id,
            1,
            sequence,
            AgentEventPayload::ProviderExtension {
                provider_namespace: "codex".into(),
                provider_event: "runtime_configuration".into(),
                payload,
            },
        )
    }

    #[test]
    fn actual_configuration_replays_from_persisted_event_and_keeps_native_empty_reasoning() {
        let request = request();
        let attempt_id = Uuid::new_v4();
        let observed = configuration_event(
            &request,
            attempt_id,
            1,
            serde_json::json!({
                "model_id":"resolved/model", "reasoning_id":"", "source":"acp_config_options",
                "ignored_native_secret":"not-in-projection"
            }),
        );
        // The projection must work after serializing the canonical event, not
        // only while a provider process keeps an in-memory catalog alive.
        let restored: AgentEventEnvelope =
            serde_json::from_str(&serde_json::to_string(&observed).unwrap()).unwrap();
        let mut state = RunState::pending(&request);
        assert_eq!(
            reduce_agent_event(&mut state, &restored).unwrap(),
            ReducerApply::Applied
        );
        let configuration = state.observed_configuration.as_ref().unwrap();
        assert_eq!(configuration.model_id.as_deref(), Some("resolved/model"));
        assert_eq!(configuration.reasoning_id.as_deref(), Some(""));
        assert_eq!(configuration.event_id, observed.event_id);
        assert_eq!(configuration.run_attempt_id, attempt_id);
        assert!(
            !serde_json::to_string(&state)
                .unwrap()
                .contains("not-in-projection")
        );
        assert_eq!(
            reduce_agent_event(&mut state, &restored).unwrap(),
            ReducerApply::Duplicate
        );
    }

    #[test]
    fn changed_model_does_not_inherit_previous_effort_and_new_attempt_resets_actual_configuration()
    {
        let request = request();
        let attempt_id = Uuid::new_v4();
        let mut state = RunState::pending(&request);
        reduce_agent_event(
            &mut state,
            &configuration_event(
                &request,
                attempt_id,
                1,
                serde_json::json!({
                    "model_id":"first/model", "reasoning_id":"high", "source":"native"
                }),
            ),
        )
        .unwrap();
        reduce_agent_event(
            &mut state,
            &configuration_event(
                &request,
                attempt_id,
                2,
                serde_json::json!({
                    "model_id":"second/model", "source":"native"
                }),
            ),
        )
        .unwrap();
        let actual = state.observed_configuration.as_ref().unwrap();
        assert_eq!(actual.model_id.as_deref(), Some("second/model"));
        assert!(actual.reasoning_id.is_none());
        reduce_agent_event(
            &mut state,
            &event(
                &request,
                Uuid::new_v4(),
                2,
                1,
                AgentEventPayload::LifecycleChanged {
                    status: AgentRunStatus::Running,
                },
            ),
        )
        .unwrap();
        assert!(state.observed_configuration.is_none());
    }

    #[test]
    fn absent_or_malformed_native_observation_does_not_claim_requested_settings() {
        let request = request();
        let attempt_id = Uuid::new_v4();
        let mut state = RunState::pending(&request);
        for (sequence, payload) in [
            serde_json::json!({"source":"empty_setter_reply"}),
            serde_json::json!({"model_id":"requested-but-no-source"}),
            serde_json::json!({"model_id":123,"reasoning_id":false,"source":"invalid"}),
        ]
        .into_iter()
        .enumerate()
        {
            reduce_agent_event(
                &mut state,
                &configuration_event(&request, attempt_id, sequence as u64 + 1, payload),
            )
            .unwrap();
            assert!(state.observed_configuration.is_none());
        }
    }

    #[test]
    fn reducer_is_idempotent_for_same_event_identity() {
        let request = request();
        let mut state = RunState::pending(&request);
        let run_attempt_id = Uuid::new_v4();
        let event = event(
            &request,
            run_attempt_id,
            1,
            1,
            AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Running,
            },
        );

        assert_eq!(
            reduce_agent_event(&mut state, &event).unwrap(),
            ReducerApply::Applied
        );
        assert_eq!(
            reduce_agent_event(&mut state, &event).unwrap(),
            ReducerApply::Duplicate
        );
        assert_eq!(state.status, AgentRunStatus::Running);
    }

    #[test]
    fn sequence_gap_marks_projection_degraded() {
        let request = request();
        let mut state = RunState::pending(&request);
        let run_attempt_id = Uuid::new_v4();
        let event = event(
            &request,
            run_attempt_id,
            1,
            2,
            AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Running,
            },
        );

        assert_eq!(
            reduce_agent_event(&mut state, &event).unwrap(),
            ReducerApply::AppliedDegraded
        );
        assert_eq!(
            state.projection_status,
            ProjectionStatus::ProjectionDegraded
        );
        assert_eq!(state.last_event_sequence, 2);
    }

    #[test]
    fn unknown_event_marks_projection_degraded_without_losing_sequence() {
        let request = request();
        let mut state = RunState::pending(&request);
        let run_attempt_id = Uuid::new_v4();
        let event = event(
            &request,
            run_attempt_id,
            1,
            1,
            AgentEventPayload::Unknown {
                event_type: "future_event".to_string(),
                payload: serde_json::json!({ "value": 1 }),
            },
        );

        assert_eq!(
            reduce_agent_event(&mut state, &event).unwrap(),
            ReducerApply::AppliedDegraded
        );
        assert_eq!(state.unknown_event_count, 1);
        assert_eq!(state.last_event_sequence, 1);
    }

    #[test]
    fn late_nonterminal_event_does_not_reopen_terminal_run() {
        let request = request();
        let mut state = RunState::pending(&request);
        let run_attempt_id = Uuid::new_v4();
        let completed = event(
            &request,
            run_attempt_id,
            1,
            1,
            AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Succeeded,
            },
        );
        reduce_agent_event(&mut state, &completed).unwrap();

        let late_running = event(
            &request,
            run_attempt_id,
            1,
            2,
            AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Running,
            },
        );

        assert_eq!(
            reduce_agent_event(&mut state, &late_running).unwrap(),
            ReducerApply::IgnoredTerminalRegression
        );
        assert_eq!(state.status, AgentRunStatus::Succeeded);
        assert_eq!(state.last_event_sequence, 2);
    }

    #[test]
    fn next_run_attempt_restarts_its_sequence_without_conflict() {
        let request = request();
        let mut state = RunState::pending(&request);
        let first_attempt_id = Uuid::new_v4();
        let failed = event(
            &request,
            first_attempt_id,
            1,
            1,
            AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Failed,
            },
        );
        reduce_agent_event(&mut state, &failed).unwrap();

        let second_attempt_id = Uuid::new_v4();
        let restarted = event(
            &request,
            second_attempt_id,
            2,
            1,
            AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Starting,
            },
        );

        assert_eq!(
            reduce_agent_event(&mut state, &restarted).unwrap(),
            ReducerApply::Applied
        );
        assert_eq!(state.status, AgentRunStatus::Starting);
        assert_eq!(state.last_run_attempt_id, Some(second_attempt_id));
        assert_eq!(state.last_run_attempt_number, 2);
        assert_eq!(state.last_event_sequence, 1);
    }

    #[test]
    fn reducer_rejects_unvalidated_event_versions() {
        let request = request();
        let mut state = RunState::pending(&request);
        let mut future_event = event(
            &request,
            Uuid::new_v4(),
            1,
            1,
            AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Running,
            },
        );
        future_event.schema_version = AGENT_EVENT_SCHEMA_VERSION + 1;

        assert!(matches!(
            reduce_agent_event(&mut state, &future_event),
            Err(ReducerError::EventVersion(
                ContractVersionError::UnsupportedFutureVersion {
                    contract: "agent_event.schema",
                    ..
                }
            ))
        ));
        assert_eq!(
            state.projection_status,
            ProjectionStatus::ProjectionDegraded
        );
    }
}
