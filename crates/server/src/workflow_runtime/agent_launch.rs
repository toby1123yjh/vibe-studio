//! Replays the original durable launch envelope, including its native resume
//! reference and timestamps. Transport uncertainty never allocates a new run.
use super::*;

pub(super) async fn run(
    deployment: &DeploymentImpl,
    request: AgentNodeRequest,
) -> Result<AgentNodeExecution, ApiError> {
    let pool = &deployment.db().pool;
    let session_id = request
        .session_id
        .ok_or_else(|| ApiError::Conflict("Workflow execution has no persisted Session".into()))?;
    let accepting: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workflow_runs r WHERE r.id=? AND r.error_text IS NULL AND r.status IN ('running','awaiting_human','awaiting_arena') AND NOT EXISTS(SELECT 1 FROM workflow_stop_intents s WHERE s.run_id=r.id))",
    )
        .bind(request.run_id).fetch_one(pool).await?;
    if !accepting {
        return Err(ApiError::Conflict(
            "Workflow no longer accepts Agent launches".into(),
        ));
    }
    let idempotency_key = format!(
        "workflow:{}:node:{}:iteration:{}:create",
        request.run_id, request.node_id, request.iteration,
    );
    let stored = stored_command(pool, &request, &idempotency_key).await?;
    let mut envelope = if let Some(stored) = stored {
        validate_replay(&stored, &request, session_id)?;
        stored
    } else {
        build_envelope(deployment, &request, session_id, idempotency_key).await?
    };
    let orchestration =
        OrchestrationService::new(pool.clone(), Arc::new(deployment.agent_run_port().clone()));
    // Once enqueued, the outbox owns delivery. A lost response after Create may
    // already have launched the original process; it is not a failed Node.
    if let Err(error) = orchestration.enqueue_command(envelope.clone()).await {
        // Enqueue can persist the outbox and then lose its event reply, or a
        // concurrent builder can accept the same identity first. Recover the
        // actual accepted bytes instead of guessing that Create never existed.
        let accepted = stored_command(pool, &request, &envelope.idempotency_key).await?;
        let Some(accepted) = accepted else {
            return Err(orchestration_api_error(error));
        };
        validate_replay(&accepted, &request, session_id)?;
        envelope = accepted;
        tracing::warn!(run_id=%request.run_id, node_id=%request.node_id, %error,
            "Workflow launch enqueue reply was uncertain; using the original durable envelope");
    }
    if let Err(error) = orchestration.deliver_next().await {
        tracing::warn!(run_id=%request.run_id, node_id=%request.node_id,
            agent_run_id=%envelope.agent_run_id, %error,
            "Workflow launch is durable; retaining its identity for outbox reconciliation");
    }
    Ok(AgentNodeExecution::Started {
        session_id,
        orchestration_node_execution_id: request.orchestration_node_execution_id,
        agent_run_id: envelope.agent_run_id,
        output_text: None,
    })
}

async fn stored_command(
    pool: &SqlitePool,
    request: &AgentNodeRequest,
    key: &str,
) -> Result<Option<AgentRunPortCommandEnvelope>, ApiError> {
    Ok(sqlx::query_scalar::<_,sqlx::types::Json<AgentRunPortCommandEnvelope>>(
        "SELECT command_envelope FROM orchestration_outbox WHERE orchestration_run_id=? AND node_execution_id=? AND idempotency_key=?",
    )
        .bind(request.orchestration_run_id).bind(request.orchestration_node_execution_id)
        .bind(key).fetch_optional(pool).await?.map(|value| value.0))
}

fn validate_replay(
    envelope: &AgentRunPortCommandEnvelope,
    request: &AgentNodeRequest,
    session_id: Uuid,
) -> Result<(), ApiError> {
    let valid = match &envelope.command {
        AgentRunPortCommand::Create {
            request: accepted,
            attempt,
        } => {
            attempt.validate_for_run(accepted).is_ok()
                && envelope.agent_run_id == accepted.agent_run_id
                && accepted.session_id == session_id
                && attempt.session_id == session_id
                && accepted.workspace.workspace_id == request.workspace_id
                && accepted.input.content == request.prompt
                && envelope.orchestration_run_id == Some(request.orchestration_run_id)
                && envelope.orchestration_node_execution_id
                    == Some(request.orchestration_node_execution_id)
        }
        _ => false,
    };
    if !valid {
        return Err(ApiError::Conflict(
            "Durable workflow launch identity or input changed".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use executors::runtime::{PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION, ProviderSessionReference};

    use super::*;

    fn resumed_launch() -> (AgentNodeRequest, AgentRunPortCommandEnvelope) {
        let request = AgentNodeRequest {
            run_id: Uuid::new_v4(),
            orchestration_run_id: Uuid::new_v4(),
            orchestration_node_execution_id: Uuid::new_v4(),
            iteration: 2,
            node_id: "writer".into(),
            session_id: Some(Uuid::new_v4()),
            workspace_id: Uuid::new_v4(),
            prompt: "Read the accepted work files".into(),
            selected_skills: None,
            executor_config: None,
        };
        let created_at = "2026-10-12T01:00:00Z"
            .parse::<chrono::DateTime<Utc>>()
            .unwrap();
        let identity = |purpose| {
            stable_workflow_identity(
                request.orchestration_run_id,
                request.orchestration_node_execution_id,
                request.iteration,
                purpose,
            )
        };
        let executor_config = ExecutorConfig::new(BaseCodingAgent::Codex);
        let profile = executor_config.profile_id().cache_key();
        let workspace = WorkspaceReference {
            workspace_id: request.workspace_id,
            mode: WorkspaceMode::SharedWorkspace,
            path: "C:/project".into(),
        };
        let key = "accepted-launch".to_string();
        let accepted = AgentRunRequestEnvelope {
            schema_version: AGENT_REQUEST_SCHEMA_VERSION,
            payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
            request_id: identity("agent-request"),
            idempotency_key: key.clone(),
            session_id: request.session_id.unwrap(),
            agent_run_id: identity("agent-run"),
            turn_id: identity("turn"),
            correlation_id: request.orchestration_run_id,
            intent: AgentRunIntent::FollowUp,
            runtime_profile_id: profile.clone(),
            provider_id: "codex".into(),
            workspace: workspace.clone(),
            input: CanonicalMessage {
                message_id: identity("input"),
                role: AgentRuntimeMessageRole::User,
                content: request.prompt.clone(),
            },
            created_at,
        };
        let attempt = RunAttemptRequest {
            schema_version: AGENT_REQUEST_SCHEMA_VERSION,
            payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
            request_id: accepted.request_id,
            idempotency_key: format!("{key}:attempt:1"),
            session_id: accepted.session_id,
            agent_run_id: accepted.agent_run_id,
            turn_id: accepted.turn_id,
            run_attempt_id: identity("attempt-1"),
            attempt_number: 1,
            correlation_id: accepted.correlation_id,
            mode: RunAttemptMode::Resume,
            transport: DirectProvider::Codex.transport(),
            runtime_profile_id: profile.clone(),
            provider_id: "codex".into(),
            workspace,
            capability_snapshot: direct_provider_capability_snapshot(
                DirectProvider::Codex,
                profile.clone(),
            ),
            executor_config,
            selected_skills: None,
            reset_to_message_id: None,
            provider_session: Some(ProviderSessionReference {
                schema_version: PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION,
                provider_id: "codex".into(),
                runtime_profile_id: profile,
                provider_session_id: "original-native-session".into(),
                observed_at: created_at - chrono::Duration::days(1),
                metadata: None,
            }),
            created_at,
        };
        let envelope = AgentRunPortCommandEnvelope {
            schema_version: ORCHESTRATION_COMMAND_SCHEMA_VERSION,
            command_id: identity("create-command"),
            idempotency_key: key,
            agent_run_id: accepted.agent_run_id,
            orchestration_run_id: Some(request.orchestration_run_id),
            orchestration_node_execution_id: Some(request.orchestration_node_execution_id),
            correlation_id: accepted.correlation_id,
            created_at,
            command: AgentRunPortCommand::Create {
                request: accepted,
                attempt,
            },
        };
        (request, envelope)
    }

    #[tokio::test]
    async fn stored_launch_keeps_the_original_native_resume_and_timestamps() {
        let (request, envelope) = resumed_launch();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE orchestration_outbox(orchestration_run_id BLOB,node_execution_id BLOB,idempotency_key TEXT,command_envelope TEXT)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO orchestration_outbox VALUES (?,?,?,?)")
            .bind(request.orchestration_run_id)
            .bind(request.orchestration_node_execution_id)
            .bind(&envelope.idempotency_key)
            .bind(sqlx::types::Json(&envelope))
            .execute(&pool)
            .await
            .unwrap();
        let replay = stored_command(&pool, &request, &envelope.idempotency_key)
            .await
            .unwrap()
            .unwrap();
        validate_replay(&replay, &request, request.session_id.unwrap()).unwrap();
        assert_eq!(replay, envelope);
        let AgentRunPortCommand::Create { attempt, .. } = replay.command else {
            panic!("expected Create")
        };
        assert_eq!(attempt.mode, RunAttemptMode::Resume);
        assert_eq!(
            attempt.provider_session.unwrap().provider_session_id,
            "original-native-session"
        );
    }

    #[test]
    fn replay_rejects_changed_input_or_session_and_incoherent_attempt() {
        let (mut request, mut envelope) = resumed_launch();
        let session = request.session_id.unwrap();
        request.prompt.push_str(" changed");
        assert!(validate_replay(&envelope, &request, session).is_err());
        request
            .prompt
            .truncate(request.prompt.len() - " changed".len());
        assert!(validate_replay(&envelope, &request, Uuid::new_v4()).is_err());
        if let AgentRunPortCommand::Create { attempt, .. } = &mut envelope.command {
            attempt.turn_id = Uuid::new_v4();
        }
        assert!(validate_replay(&envelope, &request, session).is_err());
    }
}

async fn build_envelope(
    deployment: &DeploymentImpl,
    request: &AgentNodeRequest,
    session_id: Uuid,
    key: String,
) -> Result<AgentRunPortCommandEnvelope, ApiError> {
    let pool = &deployment.db().pool;
    let workspace = Workspace::find_by_id(pool, request.workspace_id)
        .await?
        .ok_or(ApiError::Workspace(WorkspaceError::WorkspaceNotFound))?;
    let session = Session::find_by_id(pool, session_id)
        .await?
        .ok_or_else(|| {
            ApiError::Conflict("Workflow execution's original Session is unavailable".into())
        })?;
    if session.workspace_id != workspace.id {
        return Err(ApiError::Conflict(
            "Workflow Session belongs to another workspace".into(),
        ));
    }
    let executor_config = executor_config_from_node(request.executor_config.clone()).await?;
    let provider = direct_provider_for_agent(executor_config.executor)?;
    let runtime_profile_id = executor_config.profile_id().cache_key();
    let provider_session = crate::routes::sessions::agent_run::latest_provider_session(
        pool,
        session_id,
        provider,
        &runtime_profile_id,
    )
    .await?;
    let resumes = provider_session.is_some();
    let root = deployment
        .container()
        .ensure_container_exists(&workspace)
        .await?;
    let workspace_ref = WorkspaceReference {
        workspace_id: workspace.id,
        mode: match workspace.workspace_kind {
            WorkspaceKind::DirectFolder => WorkspaceMode::SharedWorkspace,
            WorkspaceKind::Worktree => WorkspaceMode::IsolatedWorktree,
        },
        path: workflow_direct_folder_agent_path(&root, session.agent_working_dir.as_deref()),
    };
    // This timestamp is part of the persisted product execution, not the clock
    // of a recovery attempt, so competing replays produce the same identity.
    let created_at: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT started_at FROM node_executions WHERE run_id=? AND node_id=? AND iteration=? AND started_at IS NOT NULL",
    )
        .bind(request.run_id).bind(&request.node_id).bind(request.iteration)
        .fetch_optional(pool).await?
        .ok_or_else(||ApiError::Conflict("Workflow launch has no accepted start identity".into()))?;
    let identity = |purpose| {
        stable_workflow_identity(
            request.orchestration_run_id,
            request.orchestration_node_execution_id,
            request.iteration,
            purpose,
        )
    };
    let request_id = identity("agent-request");
    let agent_run_id = identity("agent-run");
    let turn_id = identity("turn");
    let correlation_id = request.orchestration_run_id;
    let run_request = AgentRunRequestEnvelope {
        schema_version: AGENT_REQUEST_SCHEMA_VERSION,
        payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
        request_id,
        idempotency_key: key.clone(),
        session_id,
        agent_run_id,
        turn_id,
        correlation_id,
        intent: if resumes {
            AgentRunIntent::FollowUp
        } else {
            AgentRunIntent::Initial
        },
        runtime_profile_id: runtime_profile_id.clone(),
        provider_id: provider.id().into(),
        workspace: workspace_ref.clone(),
        input: CanonicalMessage {
            message_id: identity("input"),
            role: AgentRuntimeMessageRole::User,
            content: request.prompt.clone(),
        },
        created_at,
    };
    let attempt = RunAttemptRequest {
        schema_version: AGENT_REQUEST_SCHEMA_VERSION,
        payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
        request_id,
        idempotency_key: format!("{key}:attempt:1"),
        session_id,
        agent_run_id,
        turn_id,
        run_attempt_id: identity("attempt-1"),
        attempt_number: 1,
        correlation_id,
        mode: if resumes {
            RunAttemptMode::Resume
        } else {
            RunAttemptMode::Launch
        },
        transport: provider.transport(),
        runtime_profile_id: runtime_profile_id.clone(),
        provider_id: provider.id().into(),
        workspace: workspace_ref,
        capability_snapshot: direct_provider_capability_snapshot(provider, runtime_profile_id),
        executor_config,
        selected_skills: request.selected_skills.clone(),
        reset_to_message_id: None,
        provider_session,
        created_at,
    };
    Ok(AgentRunPortCommandEnvelope {
        schema_version: ORCHESTRATION_COMMAND_SCHEMA_VERSION,
        command_id: identity("create-command"),
        idempotency_key: key,
        agent_run_id,
        orchestration_run_id: Some(request.orchestration_run_id),
        orchestration_node_execution_id: Some(request.orchestration_node_execution_id),
        correlation_id,
        created_at,
        command: AgentRunPortCommand::Create {
            request: run_request,
            attempt,
        },
    })
}
