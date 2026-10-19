use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Weak},
    time::Duration,
};

use async_trait::async_trait;
use chrono::Utc;
use command_group::AsyncGroupChild;
use db::{
    DBService,
    models::{
        agent_runtime::{
            AgentEventRecord, AgentProviderSessionRecord, AgentRunRecord, NativeAuditStreamRecord,
        },
        session::Session,
        workflow_management::WorkflowMainSessionBinding,
        workspace::Workspace,
        workspace_repo::WorkspaceRepo,
    },
};
use executors::{
    actions::SelectedSkill,
    approvals::{ExecutorApprovalService, NoopExecutorApprovalService},
    env::{ExecutionEnv, RepoContext},
    executors::{
        CancellationToken, ExecutorExitResult, ExecutorExitSignal,
        provider_adapter::{
            DirectControl, DirectIntent, DirectProvider, DirectProviderLaunchRequest,
            encode_control, require_capability,
        },
    },
    profile::ExecutorConfig,
    runtime::{
        AGENT_EVENT_PAYLOAD_VERSION, AGENT_EVENT_SCHEMA_VERSION, AgentCapability,
        AgentEventEnvelope, AgentEventPayload, AgentEventStream, AgentRunIntent, AgentRunPort,
        AgentRunPortCommand, AgentRunPortCommandEnvelope, AgentRunPortError, AgentRunPortSnapshot,
        AgentRunRequestEnvelope, AgentRunStatus, AgentRuntimeError, AgentRuntimeErrorKind,
        AgentRuntimeMessageRole, CanonicalMessage, NativeAuditChannel, NativeAuditFrame,
        NativeAuditIntegrityStatus, NativeAuditMetadata, NativeAuditReference, NativeAuditWriter,
        ProviderSessionReference, RunAttemptMode, RunAttemptRequest,
    },
    workflow_mcp,
};
use futures::{StreamExt, stream};
use rand::{RngCore, rngs::OsRng};
use serde::Serialize;
use sqlx::{Row, types::Json};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{Mutex, RwLock, broadcast, mpsc},
};
use uuid::Uuid;

use crate::{
    agent_process_registry::{
        AgentProcessRegistry, RegisteredAgentProcess, RegisteredProcessPresence,
    },
    process_host::{
        HostBootstrap, HostCommand, HostEventPayload, HostExecutionEnv, HostJournalReplay,
        HostLaunchRequest, HostReady, host_journal_path, read_host_subscription, send_host_command,
        subscribe_host_events,
    },
    transport::{read_json_frame, write_json_frame},
};

type SharedChild = Arc<RwLock<AsyncGroupChild>>;
const TERMINAL_EVENT_CHANNEL_CAPACITY: usize = 256;
const CANCEL_HOST_REPLAY_DEADLINE: Duration = Duration::from_secs(5);
const CANCEL_HOST_REPLAY_INTERVAL: Duration = Duration::from_millis(250);
const CANCELLING_OBSERVER_FAILURE_THRESHOLD: usize = 3;
const PROCESS_HOST_OBSERVER_FAILURE_THRESHOLD: usize = 3;

type ExitSignalFuture = Pin<
    Box<
        dyn Future<Output = Result<ExecutorExitResult, tokio::sync::oneshot::error::RecvError>>
            + Send,
    >,
>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentRunTerminalEvent {
    pub agent_run_id: Uuid,
    pub session_id: Uuid,
    pub status: AgentRunStatus,
}

#[derive(Clone)]
pub struct LocalAgentRunPort {
    db: DBService,
    process_registry: AgentProcessRegistry,
    children: Arc<RwLock<HashMap<Uuid, SharedChild>>>,
    cancellation_tokens: Arc<RwLock<HashMap<Uuid, CancellationToken>>>,
    launching_attempts: Arc<Mutex<HashSet<Uuid>>>,
    audit_writers: Arc<Mutex<HashMap<Uuid, NativeAuditWriter>>>,
    event_senders: Arc<RwLock<HashMap<Uuid, broadcast::Sender<AgentEventEnvelope>>>>,
    terminal_event_sender: broadcast::Sender<AgentRunTerminalEvent>,
    event_write_lock: Arc<Mutex<()>>,
    command_lock: Arc<Mutex<()>>,
    cancellation_reconciliation_locks: Arc<Mutex<HashMap<Uuid, Weak<Mutex<()>>>>>,
    workflow_backend_url: Arc<RwLock<Option<String>>>,
}

#[derive(Debug)]
enum OutputNotice {
    StdoutClosed,
    StderrClosed,
    ProviderTerminal(AgentRunStatus),
    AuditFailure(String),
    ProtocolFailure(String),
}

#[derive(Debug)]
enum AttemptExit {
    Executor(ExecutorExitResult),
    ExecutorChannelClosed,
    ProviderTerminal(AgentRunStatus),
    Process { success: bool, code: Option<i64> },
    WatcherFailed(String),
    OutputFailure(OutputNotice),
}

struct FrozenDirectProviderLaunchSpec {
    provider: DirectProvider,
    executor_config: ExecutorConfig,
    intent: DirectIntent,
    prompt: String,
    provider_session: Option<ProviderSessionReference>,
    reset_to_message_id: Option<String>,
    selected_skills: Vec<SelectedSkill>,
    approvals: Arc<dyn ExecutorApprovalService>,
    current_dir: PathBuf,
    env: ExecutionEnv,
}

#[derive(Serialize)]
struct AuditedDirectProviderLaunchSpec<'a> {
    schema_version: u16,
    provider: DirectProvider,
    executor_config: &'a ExecutorConfig,
    intent: DirectIntent,
    prompt: &'a str,
    provider_session: Option<&'a ProviderSessionReference>,
    reset_to_message_id: Option<&'a str>,
    selected_skills: &'a [SelectedSkill],
    approval_behavior: &'static str,
    current_dir: &'a Path,
    env: AuditedExecutionEnv<'a>,
}

#[derive(Serialize)]
struct AuditedExecutionEnv<'a> {
    vars: HashMap<&'a str, &'a str>,
    workspace_root: &'a Path,
    repo_names: &'a [String],
    commit_reminder: bool,
    commit_reminder_prompt: &'a str,
}

impl FrozenDirectProviderLaunchSpec {
    fn new(
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        provider: DirectProvider,
        env: ExecutionEnv,
    ) -> Self {
        Self {
            provider,
            executor_config: attempt.executor_config.clone(),
            intent: direct_intent(request.intent, attempt.mode),
            prompt: request.input.content.clone(),
            provider_session: attempt.provider_session.clone(),
            reset_to_message_id: attempt.reset_to_message_id.clone(),
            selected_skills: attempt.selected_skills.clone().unwrap_or_default(),
            approvals: Arc::new(NoopExecutorApprovalService),
            current_dir: PathBuf::from(&attempt.workspace.path),
            env,
        }
    }

    fn with_workflow_prompt(mut self, main_prompt: Option<String>) -> Self {
        if let Some(main_prompt) = main_prompt {
            self.prompt = format!(
                "{main_prompt}\n\nWorkflow management instructions:\nUse workflow_context and workflow_get for current verified facts. Submit work only for an explicit user execution/retry/rework request; keep the same request_id on an uncertain reply. The runtime, not this Agent, dispatches Nodes and controls graph transitions. Notifications and reading results do not authorise another execution or human approval. Stop confirmation is separate from business outcome. Never ask the user for the scoped MCP credential or expose it.\n\nUser input:\n{}",
                self.prompt,
            );
        }
        self
    }

    fn launch_request(&self) -> DirectProviderLaunchRequest<'_> {
        DirectProviderLaunchRequest {
            provider: self.provider,
            executor_config: &self.executor_config,
            intent: self.intent,
            prompt: &self.prompt,
            provider_session: self.provider_session.as_ref(),
            reset_to_message_id: self.reset_to_message_id.as_deref(),
            selected_skills: &self.selected_skills,
            approvals: self.approvals.clone(),
            current_dir: &self.current_dir,
            env: &self.env,
        }
    }

    fn audit_payload(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&AuditedDirectProviderLaunchSpec {
            schema_version: 1,
            provider: self.provider,
            executor_config: &self.executor_config,
            intent: self.intent,
            prompt: &self.prompt,
            provider_session: self.provider_session.as_ref(),
            reset_to_message_id: self.reset_to_message_id.as_deref(),
            selected_skills: &self.selected_skills,
            approval_behavior: "noop",
            current_dir: &self.current_dir,
            env: AuditedExecutionEnv {
                vars: self
                    .env
                    .vars
                    .iter()
                    .filter(|(key, _)| !workflow_mcp::is_token_env_key(key))
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect(),
                workspace_root: &self.env.repo_context.workspace_root,
                repo_names: &self.env.repo_context.repo_names,
                commit_reminder: self.env.commit_reminder,
                commit_reminder_prompt: &self.env.commit_reminder_prompt,
            },
        })
    }
}

fn direct_intent(intent: AgentRunIntent, mode: RunAttemptMode) -> DirectIntent {
    match (intent, mode) {
        (_, RunAttemptMode::Resume) => DirectIntent::Resume,
        (AgentRunIntent::Initial, RunAttemptMode::Launch | RunAttemptMode::Restart) => {
            DirectIntent::Initial
        }
        (AgentRunIntent::FollowUp, RunAttemptMode::Launch) => DirectIntent::FollowUp,
        (AgentRunIntent::FollowUp, RunAttemptMode::Restart) => DirectIntent::Initial,
        (AgentRunIntent::Review, RunAttemptMode::Launch | RunAttemptMode::Restart) => {
            DirectIntent::Review
        }
    }
}

impl LocalAgentRunPort {
    pub(crate) fn new(db: DBService) -> Self {
        let (terminal_event_sender, _) = broadcast::channel(TERMINAL_EVENT_CHANNEL_CAPACITY);
        Self {
            db,
            process_registry: AgentProcessRegistry::default(),
            children: Arc::new(RwLock::new(HashMap::new())),
            cancellation_tokens: Arc::new(RwLock::new(HashMap::new())),
            launching_attempts: Arc::new(Mutex::new(HashSet::new())),
            audit_writers: Arc::new(Mutex::new(HashMap::new())),
            event_senders: Arc::new(RwLock::new(HashMap::new())),
            terminal_event_sender,
            event_write_lock: Arc::new(Mutex::new(())),
            command_lock: Arc::new(Mutex::new(())),
            cancellation_reconciliation_locks: Arc::new(Mutex::new(HashMap::new())),
            workflow_backend_url: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn set_workflow_backend_address(&self, address: std::net::SocketAddr) {
        *self.workflow_backend_url.write().await = Some(workflow_mcp::local_backend_url(address));
    }

    /// Read-only preflight. Issuance belongs to the actual launch, after a real
    /// User turn and an unhanded-off process reservation have been persisted.
    pub async fn validate_workflow_mcp_launch(
        &self,
    ) -> Result<(PathBuf, String), AgentRunPortError> {
        let url = self
            .workflow_backend_url
            .read()
            .await
            .clone()
            .ok_or_else(|| {
                AgentRunPortError::Unavailable(
                    "Current workflow backend address is unavailable".into(),
                )
            })?;
        let server = std::env::current_exe()
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
        let executable = workflow_mcp::bundled_executable_next_to(&server)
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
        workflow_mcp::verify_bundled_executable(&executable)
            .await
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
        Ok((executable, url))
    }

    pub(crate) fn subscribe_terminal_events(&self) -> broadcast::Receiver<AgentRunTerminalEvent> {
        self.terminal_event_sender.subscribe()
    }

    /// Reconnect to every persisted durable process host during application startup.
    ///
    /// This is intentionally public so server startup can invoke reconciliation
    /// before accepting AgentRun traffic. Reconciliation only attaches to the
    /// endpoint/token persisted for each RunAttempt; it never replacement-spawns
    /// a provider when the host is unreachable.
    pub async fn reconcile_process_hosts(&self) {
        let run_attempts: Vec<(Uuid, Uuid)> = match sqlx::query_as(
            r#"
            SELECT apr.run_attempt_id, ara.agent_run_id
            FROM agent_process_registry apr
            JOIN agent_run_attempts ara ON ara.id = apr.run_attempt_id
            WHERE apr.registry_status IN ('reserved', 'spawned', 'running', 'unreachable')
              AND apr.host_endpoint IS NOT NULL AND apr.host_token IS NOT NULL
            "#,
        )
        .fetch_all(&self.db.pool)
        .await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!(%error, "failed to load process-host attachments for reconciliation");
                return;
            }
        };
        for (run_attempt_id, agent_run_id) in run_attempts {
            if let Err(error) = self.attach_process_host(run_attempt_id).await {
                if let Err(mark_error) =
                    AgentRunRecord::mark_process_host_unreachable(&self.db.pool, run_attempt_id)
                        .await
                {
                    tracing::error!(run_attempt_id = %run_attempt_id, %mark_error, "failed to preserve unreachable process host");
                }
                tracing::warn!(run_attempt_id = %run_attempt_id, %error, "process host unavailable during startup reconciliation");
                if let Ok((request, attempt)) = self.load_request(agent_run_id).await {
                    if self
                        .query(agent_run_id)
                        .await
                        .is_ok_and(|snapshot| snapshot.state.status == AgentRunStatus::Cancelling)
                    {
                        let _ = self
                            .reconcile_cancel_after_host_failure(
                                &request,
                                &attempt,
                                &error.to_string(),
                                None,
                            )
                            .await;
                    } else {
                        // Attach may fail because the host is still starting or
                        // because the persisted endpoint is temporarily
                        // unreachable. Keep observing the durable attachment so
                        // a later host response can replay events and a dead host
                        // can converge to a terminal AgentRun status.
                        let port = self.clone();
                        tokio::spawn(async move {
                            port.observe_process_host(request, attempt).await;
                        });
                    }
                }
            }
        }
    }

    /// Persist a complete AgentRun identity without launching its provider.
    ///
    /// Product-level launch gates use this when an independent setup script
    /// must finish before the Agent process is allowed to start.
    pub async fn reserve(
        &self,
        request: AgentRunRequestEnvelope,
        attempt: RunAttemptRequest,
    ) -> Result<Uuid, AgentRunPortError> {
        attempt
            .validate_for_run(&request)
            .map_err(|error| AgentRunPortError::Rejected(error.to_string()))?;
        self.validate_workspace(&request).await?;
        AgentRunRecord::persist_identity_before_launch(&self.db.pool, &request, &attempt)
            .await
            .map(|identity| identity.agent_run_id)
            .map_err(|error| AgentRunPortError::Rejected(error.to_string()))
    }

    /// Launch a previously reserved AgentRun exactly once per local runtime.
    /// Repeated calls after launch acceptance return the current snapshot.
    pub async fn launch_reserved(
        &self,
        agent_run_id: Uuid,
    ) -> Result<AgentRunPortSnapshot, AgentRunPortError> {
        let _command_guard = self.command_lock.lock().await;
        let snapshot = self.query(agent_run_id).await?;
        if snapshot.state.status != AgentRunStatus::Pending {
            return Ok(snapshot);
        }

        let (request, attempt) = self.load_request(agent_run_id).await?;
        let workspace = self.validate_workspace(&request).await?;
        self.start_attempt(request, attempt, workspace).await;
        self.query(agent_run_id).await
    }

    /// Terminalize a setup-gated AgentRun that was reserved but never launched.
    pub async fn fail_reserved(
        &self,
        agent_run_id: Uuid,
        message: String,
    ) -> Result<AgentRunPortSnapshot, AgentRunPortError> {
        let _command_guard = self.command_lock.lock().await;
        let snapshot = self.query(agent_run_id).await?;
        if snapshot.state.status.is_terminal() {
            return Ok(snapshot);
        }
        if snapshot.state.status != AgentRunStatus::Pending {
            return Err(AgentRunPortError::Rejected(format!(
                "cannot fail setup gate for active AgentRun {agent_run_id}"
            )));
        }

        let (request, attempt) = self.load_request(agent_run_id).await?;
        self.terminalize_failure(
            &request,
            &attempt,
            AgentRunStatus::Failed,
            AgentRuntimeError::new(AgentRuntimeErrorKind::StartupFailed, message),
        )
        .await;
        self.query(agent_run_id).await
    }

    /// Attach to one persisted process host and replay observations after the
    /// durable cursor. This is useful to startup orchestration and tests that
    /// need deterministic single-attempt reconstruction.
    pub async fn attach_process_host(&self, run_attempt_id: Uuid) -> Result<(), AgentRunPortError> {
        let row: Option<(
            Json<AgentRunRequestEnvelope>,
            Json<RunAttemptRequest>,
            String,
            String,
            Option<String>,
            i64,
        )> = sqlx::query_as(
            r#"
            SELECT ar.request_envelope, ara.request_envelope,
                   apr.host_endpoint, apr.host_token, apr.host_instance_id,
                   apr.last_host_event_sequence
            FROM agent_process_registry apr
            JOIN agent_run_attempts ara ON ara.id = apr.run_attempt_id
            JOIN agent_runs ar ON ar.id = ara.agent_run_id
            WHERE apr.run_attempt_id = ?
              AND apr.host_endpoint IS NOT NULL AND apr.host_token IS NOT NULL
            "#,
        )
        .bind(run_attempt_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(port_database)?;
        let Some((request, attempt, endpoint, token, host_instance_id, cursor)) = row else {
            return Err(AgentRunPortError::NotFound(run_attempt_id));
        };
        let after_sequence = u64::try_from(cursor).unwrap_or_default();
        let response = match send_host_command(
            &endpoint,
            &token,
            HostCommand::Attach { after_sequence },
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                AgentRunRecord::mark_process_host_unreachable(&self.db.pool, run_attempt_id)
                    .await
                    .map_err(|mark_error| AgentRunPortError::Unavailable(mark_error.to_string()))?;
                return Err(AgentRunPortError::Unavailable(format!(
                    "process host unavailable for RunAttempt {run_attempt_id}: {error}"
                )));
            }
        };
        if let Some(error) = response.error {
            AgentRunRecord::mark_process_host_unreachable(&self.db.pool, run_attempt_id)
                .await
                .map_err(|mark_error| AgentRunPortError::Unavailable(mark_error.to_string()))?;
            return Err(AgentRunPortError::Unavailable(error));
        }
        if let Some(expected) = host_instance_id
            && response.host_instance_id.to_string() != expected
        {
            return Err(AgentRunPortError::Unavailable(
                "process host identity did not match persisted reservation".to_string(),
            ));
        }
        self.apply_host_events(&request.0, &attempt.0, response.events)
            .await?;
        let port = self.clone();
        tokio::spawn(async move {
            port.observe_process_host(request.0, attempt.0).await;
        });
        Ok(())
    }

    async fn cleanup_failed_process_host_launch(
        &self,
        run_attempt_id: Uuid,
        child: Option<&mut tokio::process::Child>,
    ) {
        if let Some(child) = child {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        if let Err(error) =
            AgentRunRecord::clear_process_host_reservation(&self.db.pool, run_attempt_id).await
        {
            tracing::warn!(run_attempt_id = %run_attempt_id, %error, "failed to clear process-host reservation after launch failure");
        }
    }

    async fn append_lifecycle(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        status: AgentRunStatus,
    ) -> Result<(), AgentRunPortError> {
        self.append_event(
            request,
            attempt,
            AgentEventPayload::LifecycleChanged { status },
            Vec::new(),
            Utc::now(),
            None,
        )
        .await
    }

    async fn append_event(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        payload: AgentEventPayload,
        native_refs: Vec<NativeAuditReference>,
        timestamp: chrono::DateTime<Utc>,
        event_id: Option<Uuid>,
    ) -> Result<(), AgentRunPortError> {
        let _guard = self.event_write_lock.lock().await;
        let event_id = event_id.unwrap_or_else(Uuid::new_v4);
        // The orchestration identity is owned by the durable link table, not
        // by provider transports. Resolve it at the canonical write boundary
        // so every emitted AgentEvent can be ingested by orchestration after a
        // restart (direct runs simply have no link and remain unscoped).
        let orchestration_identity: Option<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT orchestration_run_id, node_execution_id FROM orchestration_agent_run_links WHERE agent_run_id = ?",
        )
        .bind(request.agent_run_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(port_database)?;
        let (orchestration_run_id, orchestration_node_execution_id) = orchestration_identity
            .map(|(run_id, node_id)| (Some(run_id), Some(node_id)))
            .unwrap_or((None, None));
        let existing: Option<Json<AgentEventEnvelope>> =
            sqlx::query_scalar("SELECT event_envelope FROM agent_events WHERE event_id = ?")
                .bind(event_id)
                .fetch_optional(&self.db.pool)
                .await
                .map_err(port_database)?;
        if let Some(existing) = existing {
            let event = AgentEventEnvelope {
                schema_version: AGENT_EVENT_SCHEMA_VERSION,
                payload_version: AGENT_EVENT_PAYLOAD_VERSION,
                event_id,
                session_id: request.session_id,
                agent_run_id: request.agent_run_id,
                turn_id: request.turn_id,
                run_attempt_id: attempt.run_attempt_id,
                run_attempt_number: attempt.attempt_number,
                sequence: existing.0.sequence,
                correlation_id: request.correlation_id,
                orchestration_run_id,
                orchestration_node_execution_id,
                timestamp,
                native_refs,
                payload,
            };
            return if event == existing.0 {
                Ok(())
            } else {
                Err(AgentRunPortError::Rejected(format!(
                    "canonical event id {event_id} was reused with different content"
                )))
            };
        }
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_events WHERE run_attempt_id = ?",
        )
        .bind(attempt.run_attempt_id)
        .fetch_one(&self.db.pool)
        .await
        .map_err(port_database)?;
        let sequence = u64::try_from(sequence)
            .map_err(|_| AgentRunPortError::Rejected("invalid event sequence".to_string()))?;
        let event = AgentEventEnvelope {
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload_version: AGENT_EVENT_PAYLOAD_VERSION,
            event_id,
            session_id: request.session_id,
            agent_run_id: request.agent_run_id,
            turn_id: request.turn_id,
            run_attempt_id: attempt.run_attempt_id,
            run_attempt_number: attempt.attempt_number,
            sequence,
            correlation_id: request.correlation_id,
            orchestration_run_id,
            orchestration_node_execution_id,
            timestamp,
            native_refs,
            payload,
        };
        AgentEventRecord::append_and_project(&self.db.pool, &event)
            .await
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
        let terminal_event = match &event.payload {
            AgentEventPayload::LifecycleChanged { status } if status.is_terminal() => {
                Some(AgentRunTerminalEvent {
                    agent_run_id: request.agent_run_id,
                    session_id: request.session_id,
                    status: *status,
                })
            }
            _ => None,
        };
        let sender = self.sender(request.agent_run_id).await;
        let _ = sender.send(event);
        if let Some(terminal_event) = terminal_event {
            let _ = self.terminal_event_sender.send(terminal_event);
        }
        Ok(())
    }

    async fn append_recoverable(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        payload: AgentEventPayload,
        native_refs: Vec<NativeAuditReference>,
        timestamp: chrono::DateTime<Utc>,
        event_id: Option<Uuid>,
    ) {
        if let Err(error) = self
            .append_event(request, attempt, payload, native_refs, timestamp, event_id)
            .await
        {
            tracing::error!(
                agent_run_id = %request.agent_run_id,
                run_attempt_id = %attempt.run_attempt_id,
                %error,
                "canonical AgentRun projection failed; preserving Native Audit for replay"
            );
            if let Err(mark_error) =
                AgentRunRecord::mark_projection_degraded(&self.db.pool, request.agent_run_id).await
            {
                tracing::error!(
                    agent_run_id = %request.agent_run_id,
                    error = %mark_error,
                    "failed to mark AgentRun projection degraded"
                );
            }
        }
    }

    async fn append_mapped_event(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        mapped: AgentEventEnvelope,
        native_ref: NativeAuditReference,
    ) -> Result<(), AgentRunPortError> {
        // Provider session observations are the canonical source used by the
        // next follow-up launch. Persist them independently from the event
        // projection so a replay/query can recover the native session even if
        // the live projection is temporarily degraded.
        let observed_provider_session = match &mapped.payload {
            AgentEventPayload::SessionObserved { provider_session } => {
                Some(provider_session.clone())
            }
            _ => None,
        };
        let catalog_observation =
            matches!(&mapped.payload, AgentEventPayload::ProviderExtension { provider_event, .. }
            if provider_event == executors::executors::acp::session_config::ACP_CATALOG_EVENT
                || provider_event == executors::executors::oh_my_pi::catalog::CATALOG_EVENT)
            .then(|| mapped.payload.clone());
        if let AgentEventPayload::LifecycleChanged { status } = &mapped.payload {
            // A failed snapshot read must not drop the audited canonical
            // lifecycle event.  Persist/reduce it and let the append path
            // surface projection degradation; only suppress events when a
            // successful read proves that the run is already terminal (or is
            // cancelling and this is not the confirming cancellation).
            if let Ok(snapshot) = self.query(request.agent_run_id).await {
                if snapshot.state.status.is_terminal()
                    || (snapshot.state.status == AgentRunStatus::Cancelling
                        && *status != AgentRunStatus::Cancelled)
                {
                    return Ok(());
                }
            }
        }
        self.append_event(
            request,
            attempt,
            mapped.payload,
            vec![native_ref],
            mapped.timestamp,
            Some(mapped.event_id),
        )
        .await?;

        if let Some(payload) = catalog_observation {
            executors::executors::acp::session_config::cache_catalog_extension(
                &attempt.provider_id,
                &payload,
            );
            executors::executors::oh_my_pi::catalog::cache_catalog_extension(
                &attempt.provider_id,
                &payload,
            );
        }
        if let Some(provider_session) = observed_provider_session {
            if let Err(error) = AgentProviderSessionRecord::upsert(
                &self.db.pool,
                Uuid::new_v4(),
                request.session_id,
                &provider_session,
            )
            .await
            {
                tracing::error!(
                    session_id = %request.session_id,
                    run_attempt_id = %attempt.run_attempt_id,
                    %error,
                    "failed to persist observed provider session"
                );
                if let Err(mark_error) =
                    AgentRunRecord::mark_projection_degraded(&self.db.pool, request.agent_run_id)
                        .await
                {
                    tracing::error!(
                        agent_run_id = %request.agent_run_id,
                        error = %mark_error,
                        "failed to mark AgentRun degraded after provider-session persistence failure"
                    );
                }
                return Err(AgentRunPortError::Unavailable(error.to_string()));
            }
        }
        Ok(())
    }

    async fn sender(&self, agent_run_id: Uuid) -> broadcast::Sender<AgentEventEnvelope> {
        let mut senders = self.event_senders.write().await;
        senders
            .entry(agent_run_id)
            .or_insert_with(|| broadcast::channel(256).0)
            .clone()
    }

    async fn load_request(
        &self,
        agent_run_id: Uuid,
    ) -> Result<(AgentRunRequestEnvelope, RunAttemptRequest), AgentRunPortError> {
        let row: Option<(Json<AgentRunRequestEnvelope>, Json<RunAttemptRequest>)> = sqlx::query_as(
            r#"
                SELECT ar.request_envelope, ara.request_envelope
                FROM agent_runs ar
                JOIN agent_run_attempts ara ON ara.agent_run_id = ar.id
                WHERE ar.id = ?
                ORDER BY ara.attempt_number DESC
                LIMIT 1
                "#,
        )
        .bind(agent_run_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(port_database)?;
        row.map(|(request, attempt)| (request.0, attempt.0))
            .ok_or(AgentRunPortError::NotFound(agent_run_id))
    }

    fn provider(&self, provider_id: &str) -> Result<DirectProvider, AgentRunPortError> {
        match provider_id.replace('-', "_").to_ascii_lowercase().as_str() {
            "gemini" => Ok(DirectProvider::Gemini),
            "codex" => Ok(DirectProvider::Codex),
            "claude_code" | "claude" => Ok(DirectProvider::ClaudeCode),
            "oh_my_pi" | "omp" => Ok(DirectProvider::OhMyPi),
            "opencode" => Ok(DirectProvider::Opencode),
            "deepseek_harness" | "dsh" => Ok(DirectProvider::DeepseekHarness),
            _ => Err(AgentRunPortError::Rejected(format!(
                "unsupported provider {provider_id}"
            ))),
        }
    }

    async fn validate_workspace(
        &self,
        request: &AgentRunRequestEnvelope,
    ) -> Result<Workspace, AgentRunPortError> {
        let session = Session::find_by_id(&self.db.pool, request.session_id)
            .await
            .map_err(port_database)?
            .ok_or(AgentRunPortError::NotFound(request.session_id))?;
        if session.workspace_id != request.workspace.workspace_id {
            return Err(AgentRunPortError::Rejected(
                "session and AgentRun workspace do not match".to_string(),
            ));
        }
        let workspace = Workspace::find_by_id(&self.db.pool, request.workspace.workspace_id)
            .await
            .map_err(port_database)?
            .ok_or(AgentRunPortError::NotFound(request.workspace.workspace_id))?;
        let current_dir = Path::new(&request.workspace.path);
        if !current_dir.is_dir() {
            return Err(AgentRunPortError::Rejected(format!(
                "AgentRun workspace does not exist: {}",
                current_dir.display()
            )));
        }
        let persisted_dir = workspace.container_ref.as_deref().ok_or_else(|| {
            AgentRunPortError::Rejected(
                "AgentRun workspace has no persisted local path".to_string(),
            )
        })?;
        let requested_path = tokio::fs::canonicalize(current_dir)
            .await
            .map_err(|error| AgentRunPortError::Rejected(error.to_string()))?;
        let persisted_path = tokio::fs::canonicalize(persisted_dir)
            .await
            .map_err(|error| AgentRunPortError::Rejected(error.to_string()))?;
        if requested_path != persisted_path {
            return Err(AgentRunPortError::Rejected(format!(
                "AgentRun workspace path {} does not match persisted workspace path {}",
                current_dir.display(),
                Path::new(persisted_dir).display()
            )));
        }
        Ok(workspace)
    }

    async fn execution_env(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        workspace: &Workspace,
    ) -> Result<Option<(ExecutionEnv, Option<String>)>, AgentRunPortError> {
        let repos = WorkspaceRepo::find_repos_for_workspace(&self.db.pool, workspace.id)
            .await
            .map_err(port_database)?;
        let repo_names = repos.into_iter().map(|repo| repo.name).collect();
        let mut env = ExecutionEnv::new(
            RepoContext::new(PathBuf::from(&request.workspace.path), repo_names),
            false,
            String::new(),
        );
        env.insert("VK_WORKSPACE_ID", workspace.id.to_string());
        env.insert("VK_WORKSPACE_BRANCH", &workspace.branch);
        env.insert("VK_AGENT_RUN_ID", request.agent_run_id.to_string());
        env.insert("VK_RUN_ATTEMPT_ID", attempt.run_attempt_id.to_string());
        let binding =
            WorkflowMainSessionBinding::find_by_session_id(&self.db.pool, request.session_id)
                .await
                .map_err(port_database)?;
        let Some(binding) = binding else {
            return Ok(Some((env, None)));
        };
        if binding.actual_main_run_attempt_id == Some(attempt.run_attempt_id) {
            return Ok(None);
        }
        let captured: ExecutorConfig = serde_json::from_str(&binding.main_agent_config_json)
            .map_err(|_| {
                AgentRunPortError::Rejected(
                    "Captured workflow main Agent configuration is invalid".into(),
                )
            })?;
        if !workflow_mcp::matches_snapshot(&captured, &attempt.executor_config) {
            return Err(AgentRunPortError::Rejected(
                "Workflow main Agent configuration cannot change after preparation".into(),
            ));
        }
        let (executable, backend_url) = self.validate_workflow_mcp_launch().await?;
        // Settle previous terminal attempts through the actual process registry.
        // A completed reply alone is not proof that its MCP can no longer act.
        let prior: Vec<Uuid> = sqlx::query_scalar(
            "SELECT a.id FROM agent_run_attempts a JOIN agent_runs r ON r.id=a.agent_run_id WHERE r.session_id=? AND a.id<>? AND a.status IN ('succeeded','failed','cancelled','crashed','audit_failed')",
        ).bind(request.session_id).bind(attempt.run_attempt_id).fetch_all(&self.db.pool).await.map_err(port_database)?;
        for attempt_id in prior {
            self.reconcile_terminal_process(attempt_id).await?;
        }
        let Some((token, prompt)) = self.issue_workflow_mcp_token(request, attempt).await? else {
            return Ok(None);
        };
        env.insert(workflow_mcp::EXECUTABLE_ENV, executable.to_string_lossy());
        env.insert(workflow_mcp::BACKEND_URL_ENV, backend_url);
        env.insert(workflow_mcp::SESSION_ID_ENV, request.session_id.to_string());
        env.insert(
            workflow_mcp::AGENT_RUN_ID_ENV,
            request.agent_run_id.to_string(),
        );
        env.insert(workflow_mcp::TURN_ID_ENV, request.turn_id.to_string());
        env.insert(workflow_mcp::TOKEN_ENV, token);
        Ok(Some((env, Some(prompt))))
    }

    /// Raw credentials are only returned to this ephemeral launch. A write
    /// transaction fences deletion, competing turns and uncertain host handoff.
    /// None means this attempt's handoff already belongs to another launcher:
    /// do not rotate, dispatch again, or overwrite that launcher's run state.
    async fn issue_workflow_mcp_token(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
    ) -> Result<Option<(String, String)>, AgentRunPortError> {
        let rejected = || {
            AgentRunPortError::Rejected("Workflow main Agent scope is inconsistent, or an earlier process has not confirmed exit".into())
        };
        if request.session_id != attempt.session_id
            || request.agent_run_id != attempt.agent_run_id
            || request.turn_id != attempt.turn_id
            || request.workspace != attempt.workspace
        {
            return Err(rejected());
        }
        let mut tx = self
            .db
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(port_database)?;
        if request.input.role != AgentRuntimeMessageRole::User {
            let authorized_handover: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM workflow_handover_events e JOIN workflow_runs r ON r.id=e.run_id JOIN workflow_attempts w ON w.id=r.attempt_id JOIN workflow_project_slots p ON p.run_id=r.id JOIN workflow_split_batches b ON b.id=e.batch_id JOIN node_executions n ON n.id=b.node_execution_id WHERE e.session_id=? AND e.agent_run_id=? AND e.run_attempt_id=? AND e.turn_id=? AND e.state='claimed' AND w.main_session_id=e.session_id AND r.status IN ('running','awaiting_human','awaiting_arena') AND b.state='accepted' AND b.dispatch_sha256 IS NULL AND n.status='succeeded' AND NOT EXISTS(SELECT 1 FROM workflow_stop_intents i WHERE i.run_id=r.id))",
            ).bind(request.session_id).bind(request.agent_run_id).bind(attempt.run_attempt_id).bind(request.turn_id)
                .fetch_one(&mut *tx).await.map_err(port_database)?;
            if request.input.role != AgentRuntimeMessageRole::System || !authorized_handover {
                return Err(rejected());
            }
        }
        let row = sqlx::query(
            "SELECT b.*,m.project_id,s.workspace_id,w.project_id AS template_project_id FROM workflow_main_session_bindings b JOIN sessions s ON s.id=b.session_id JOIN session_project_memberships m ON m.session_id=s.id JOIN workflows w ON w.id=b.workflow_id WHERE b.session_id=?",
        ).bind(request.session_id).fetch_optional(&mut *tx).await.map_err(port_database)?.ok_or_else(rejected)?;
        let project_id: Uuid = row.try_get("project_id").map_err(port_database)?;
        let workspace_id: Uuid = row.try_get("workspace_id").map_err(port_database)?;
        let workflow_id: Uuid = row.try_get("workflow_id").map_err(port_database)?;
        if workspace_id != request.workspace.workspace_id
            || row
                .try_get::<Option<Uuid>, _>("template_project_id")
                .map_err(port_database)?
                .is_some_and(|id| id != project_id)
        {
            return Err(rejected());
        }
        let captured: ExecutorConfig = serde_json::from_str(
            row.try_get::<&str, _>("main_agent_config_json")
                .map_err(port_database)?,
        )
        .map_err(|_| rejected())?;
        if !workflow_mcp::matches_snapshot(&captured, &attempt.executor_config) {
            return Err(rejected());
        }
        let instance = sqlx::query(
            "SELECT a.workflow_id,a.workspace_id,t.project_id,src.template_id FROM workflow_attempts a JOIN tasks t ON t.id=a.task_id LEFT JOIN workflow_attempt_sources src ON src.attempt_id=a.id WHERE a.main_session_id=?",
        ).bind(request.session_id).fetch_optional(&mut *tx).await.map_err(port_database)?;
        if let Some(instance) = instance {
            let publication_id = instance
                .try_get::<Option<Uuid>, _>("template_id")
                .map_err(port_database)?
                .unwrap_or(instance.try_get("workflow_id").map_err(port_database)?);
            if publication_id != workflow_id
                || instance
                    .try_get::<Uuid, _>("project_id")
                    .map_err(port_database)?
                    != project_id
                || instance
                    .try_get::<Option<Uuid>, _>("workspace_id")
                    .map_err(port_database)?
                    != Some(workspace_id)
            {
                return Err(rejected());
            }
        }
        let persisted: Option<(Json<CanonicalMessage>, Json<RunAttemptRequest>)> = sqlx::query_as(
            "SELECT t.input_message,a.request_envelope FROM agent_runs r JOIN agent_turns t ON t.agent_run_id=r.id JOIN agent_run_attempts a ON a.agent_run_id=r.id AND a.turn_id=t.id JOIN agent_process_registry p ON p.run_attempt_id=a.id WHERE r.id=? AND r.session_id=? AND r.workspace_id=? AND t.id=? AND a.id=? AND r.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed') AND a.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed') AND p.registry_status='reserved' AND p.pid IS NULL AND p.host_pid IS NULL AND p.host_endpoint IS NULL AND p.host_token IS NULL",
        ).bind(request.agent_run_id).bind(request.session_id).bind(workspace_id).bind(request.turn_id)
            .bind(attempt.run_attempt_id).fetch_optional(&mut *tx).await.map_err(port_database)?;
        let Some((input, persisted_attempt)) = persisted else {
            return Err(rejected());
        };
        if input.0 != request.input || persisted_attempt.0 != *attempt {
            return Err(rejected());
        }
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM agent_run_attempts a JOIN agent_runs r ON r.id=a.agent_run_id LEFT JOIN agent_process_registry p ON p.run_attempt_id=a.id WHERE r.session_id=? AND a.id<>? AND (a.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed') OR p.id IS NULL OR (p.registry_status<>'exited' AND NOT(p.registry_status='reserved' AND p.pid IS NULL AND p.host_pid IS NULL AND p.host_endpoint IS NULL AND p.host_token IS NULL))))",
        ).bind(request.session_id).bind(attempt.run_attempt_id).fetch_one(&mut *tx).await.map_err(port_database)?;
        if blocked {
            return Err(rejected());
        }
        let old_hash: Option<String> = row.try_get("token_hash").map_err(port_database)?;
        let old_run: Option<Uuid> = row
            .try_get("actual_main_agent_run_id")
            .map_err(port_database)?;
        let old_turn: Option<Uuid> = row.try_get("actual_main_turn_id").map_err(port_database)?;
        let old_attempt: Option<Uuid> = row
            .try_get("actual_main_run_attempt_id")
            .map_err(port_database)?;
        // The raw credential is deliberately not recoverable. Once committed,
        // this attempt cannot issue again, even if its host reservation is still
        // empty: another backend may already be handing off the first token.
        if old_attempt == Some(attempt.run_attempt_id) {
            return Ok(None);
        }
        let prompt: String = row.try_get("main_agent_prompt").map_err(port_database)?;
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let changed = sqlx::query(
            "UPDATE workflow_main_session_bindings SET token_hash=?,actual_main_agent_run_id=?,actual_main_turn_id=?,actual_main_run_attempt_id=?,source_message_id=?,updated_at=? WHERE session_id=? AND token_hash IS ? AND actual_main_agent_run_id IS ? AND actual_main_turn_id IS ? AND actual_main_run_attempt_id IS ?",
        ).bind(workflow_mcp::token_hash(&token)).bind(request.agent_run_id).bind(request.turn_id)
            .bind(attempt.run_attempt_id).bind((request.input.role == AgentRuntimeMessageRole::User).then_some(request.input.message_id)).bind(Utc::now()).bind(request.session_id)
            .bind(old_hash).bind(old_run).bind(old_turn).bind(old_attempt).execute(&mut *tx).await.map_err(port_database)?;
        if changed.rows_affected() != 1 {
            return Err(rejected());
        }
        tx.commit().await.map_err(port_database)?;
        Ok(Some((token, prompt)))
    }

    async fn setup_audit(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        provider: DirectProvider,
        launch: &FrozenDirectProviderLaunchSpec,
    ) -> Result<NativeAuditReference, AgentRunPortError> {
        let versions = provider.versions();
        let metadata = NativeAuditMetadata {
            session_id: request.session_id,
            agent_run_id: request.agent_run_id,
            turn_id: request.turn_id,
            run_attempt_id: attempt.run_attempt_id,
            run_attempt_number: attempt.attempt_number,
            provider_id: provider.id().to_string(),
            runtime_profile_id: request.runtime_profile_id.clone(),
            workspace_path: request.workspace.path.clone(),
            runtime_version: versions.runtime.map(str::to_owned),
            protocol_version: versions.protocol.map(str::to_owned),
            adapter_version: versions.adapter.to_string(),
            mapper_version: versions.mapper.to_string(),
            created_at: Utc::now(),
        };
        let mut writer = NativeAuditWriter::create(metadata).map_err(port_audit)?;
        NativeAuditStreamRecord::insert_open(&self.db.pool, writer.manifest())
            .await
            .map_err(port_database)?;
        let canonical_ref =
            match writer.append_canonical_input(&request.input, request.correlation_id) {
                Ok(reference) => reference,
                Err(error) => {
                    let manifest = writer.fail_closed().ok();
                    if let Some(manifest) = manifest {
                        let _ = NativeAuditStreamRecord::finalize(&self.db.pool, &manifest).await;
                    }
                    return Err(port_audit(error));
                }
            };
        let native_input = match launch.audit_payload() {
            Ok(native_input) => native_input,
            Err(error) => {
                let manifest = writer.fail_closed().ok();
                if let Some(manifest) = manifest {
                    let _ = NativeAuditStreamRecord::finalize(&self.db.pool, &manifest).await;
                }
                return Err(AgentRunPortError::Unavailable(error.to_string()));
            }
        };
        if let Err(error) = writer.append_native_input(
            NativeAuditChannel::NativeInput,
            "application/vnd.vibe-kanban.direct-provider-launch+json",
            request.correlation_id,
            &native_input,
        ) {
            let manifest = writer.fail_closed().ok();
            if let Some(manifest) = manifest {
                let _ = NativeAuditStreamRecord::finalize(&self.db.pool, &manifest).await;
            }
            return Err(port_audit(error));
        }
        self.audit_writers
            .lock()
            .await
            .insert(attempt.run_attempt_id, writer);
        Ok(canonical_ref)
    }

    async fn append_audit_bytes(
        &self,
        attempt: &RunAttemptRequest,
        channel: NativeAuditChannel,
        content_type: &str,
        payload: &[u8],
    ) -> Result<(NativeAuditFrame, NativeAuditReference), AgentRunPortError> {
        let mut writers = self.audit_writers.lock().await;
        let writer = writers.get_mut(&attempt.run_attempt_id).ok_or_else(|| {
            AgentRunPortError::Unavailable("Native Audit writer is not attached".to_string())
        })?;
        let sequence = writer.manifest().last_sequence.unwrap_or(0) + 1;
        let frame = NativeAuditFrame::from_bytes(
            sequence,
            Utc::now(),
            executors::runtime::NativeAuditDirection::Output,
            channel,
            content_type,
            attempt.correlation_id,
            payload,
            None,
        );
        let reference = writer.append(frame.clone()).map_err(port_audit)?;
        Ok((frame, reference))
    }

    async fn require_audit_writer(
        &self,
        attempt: &RunAttemptRequest,
    ) -> Result<(), AgentRunPortError> {
        if self
            .audit_writers
            .lock()
            .await
            .contains_key(&attempt.run_attempt_id)
        {
            Ok(())
        } else {
            Err(AgentRunPortError::Unavailable(
                "Native Audit writer is not attached".to_string(),
            ))
        }
    }

    async fn finalize_audit(&self, attempt: &RunAttemptRequest) -> Result<(), AgentRunPortError> {
        let Some(mut writer) = self
            .audit_writers
            .lock()
            .await
            .remove(&attempt.run_attempt_id)
        else {
            return Err(AgentRunPortError::Unavailable(
                "Native Audit writer disappeared before close".to_string(),
            ));
        };
        match writer.close_with_status(NativeAuditIntegrityStatus::Complete) {
            Ok(manifest) => NativeAuditStreamRecord::finalize(&self.db.pool, &manifest)
                .await
                .map_err(port_database),
            Err(close_error) => {
                match writer.fail_closed() {
                    Ok(manifest) => {
                        if let Err(error) =
                            NativeAuditStreamRecord::finalize(&self.db.pool, &manifest).await
                        {
                            tracing::error!(
                                run_attempt_id = %attempt.run_attempt_id,
                                %error,
                                "failed to persist fail-closed Native Audit manifest"
                            );
                        }
                    }
                    Err(error) => tracing::error!(
                        run_attempt_id = %attempt.run_attempt_id,
                        %error,
                        "Native Audit close failed and fail-closed manifest could not be written"
                    ),
                }
                Err(port_audit(close_error))
            }
        }
    }

    async fn fail_audit_writer(&self, attempt: &RunAttemptRequest) {
        let Some(mut writer) = self
            .audit_writers
            .lock()
            .await
            .remove(&attempt.run_attempt_id)
        else {
            return;
        };
        match writer.fail_closed() {
            Ok(manifest) => {
                if let Err(error) =
                    NativeAuditStreamRecord::finalize(&self.db.pool, &manifest).await
                {
                    tracing::error!(
                        run_attempt_id = %attempt.run_attempt_id,
                        %error,
                        "failed to finalize failed Native Audit index"
                    );
                }
            }
            Err(error) => tracing::error!(
                run_attempt_id = %attempt.run_attempt_id,
                %error,
                "Native Audit failed closed without a final manifest"
            ),
        }
    }

    async fn terminalize_failure(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        status: AgentRunStatus,
        error: AgentRuntimeError,
    ) {
        self.append_recoverable(
            request,
            attempt,
            AgentEventPayload::Error { error },
            Vec::new(),
            Utc::now(),
            None,
        )
        .await;
        self.append_recoverable(
            request,
            attempt,
            AgentEventPayload::LifecycleChanged { status },
            Vec::new(),
            Utc::now(),
            None,
        )
        .await;
    }

    async fn launch_process_host(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        provider: DirectProvider,
        launch: &FrozenDirectProviderLaunchSpec,
    ) -> Result<(), AgentRunPortError> {
        // A create/outbox acknowledgement can be lost after the durable host
        // reservation has been written.  Before spawning another host, always
        // try to reuse a still-reserved endpoint.  The reservation is the
        // idempotency fence: even when host_pid has not been persisted yet,
        // the existing host may already be alive and accepting its launch.
        let existing: Option<(String, String, Option<String>, String)> = sqlx::query_as(
            r#"
            SELECT host_endpoint, host_token, host_instance_id, registry_status
            FROM agent_process_registry
            WHERE run_attempt_id = ?
              AND host_endpoint IS NOT NULL AND host_token IS NOT NULL
            "#,
        )
        .bind(attempt.run_attempt_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(port_database)?;
        if let Some((endpoint, token, host_instance_id, registry_status)) = existing
            && registry_status == "reserved"
        {
            let versions = provider.versions();
            let audited_launch_payload = launch
                .audit_payload()
                .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
            let launch_request = HostLaunchRequest {
                run_attempt_id: attempt.run_attempt_id,
                provider,
                executor_config: launch.executor_config.clone(),
                intent: launch.intent,
                prompt: launch.prompt.clone(),
                provider_session: launch.provider_session.clone(),
                reset_to_message_id: launch.reset_to_message_id.clone(),
                selected_skills: launch.selected_skills.clone(),
                current_dir: launch.current_dir.clone(),
                env: HostExecutionEnv::from(&launch.env),
                audit_metadata: NativeAuditMetadata {
                    session_id: request.session_id,
                    agent_run_id: request.agent_run_id,
                    turn_id: request.turn_id,
                    run_attempt_id: attempt.run_attempt_id,
                    run_attempt_number: attempt.attempt_number,
                    provider_id: provider.id().to_string(),
                    runtime_profile_id: request.runtime_profile_id.clone(),
                    workspace_path: request.workspace.path.clone(),
                    runtime_version: versions.runtime.map(str::to_owned),
                    protocol_version: versions.protocol.map(str::to_owned),
                    adapter_version: versions.adapter.to_string(),
                    mapper_version: versions.mapper.to_string(),
                    created_at: Utc::now(),
                },
                canonical_input: request.input.clone(),
                correlation_id: request.correlation_id,
                audited_launch_payload,
            };
            match send_host_command(&endpoint, &token, HostCommand::Launch(launch_request)).await {
                Ok(response) => {
                    if let Some(expected) = host_instance_id
                        && response.host_instance_id.to_string() != expected
                    {
                        return Err(AgentRunPortError::Unavailable(
                            "process host identity did not match persisted reservation".to_string(),
                        ));
                    }
                    if let Some(error) = response.error {
                        return Err(AgentRunPortError::Unavailable(error));
                    }
                    self.apply_host_events(request, attempt, response.events)
                        .await?;
                }
                Err(error) => {
                    // The launch may have reached the host even when its ACK
                    // was lost.  Preserve the reservation and let the durable
                    // observer/reconciliation loop attach later; never spawn a
                    // replacement process from an uncertain acknowledgement.
                    let port = self.clone();
                    let request = request.clone();
                    let run_attempt_id = attempt.run_attempt_id;
                    let attempt = attempt.clone();
                    tokio::spawn(async move {
                        port.observe_process_host(request, attempt).await;
                    });
                    tracing::debug!(
                        run_attempt_id = %run_attempt_id,
                        %error,
                        "existing process-host launch response unavailable; preserving reservation"
                    );
                    return Ok(());
                }
            }
            let port = self.clone();
            let request = request.clone();
            let attempt = attempt.clone();
            tokio::spawn(async move {
                port.observe_process_host(request, attempt).await;
            });
            return Ok(());
        }

        let host_instance_id = Uuid::new_v4();
        let mut token_bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut token_bytes);
        let auth_token = token_bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let endpoint_reservation = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
        let requested_endpoint = endpoint_reservation
            .local_addr()
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?
            .to_string();
        drop(endpoint_reservation);
        AgentRunRecord::reserve_process_host(
            &self.db.pool,
            attempt.run_attempt_id,
            &requested_endpoint,
            &auth_token,
            host_instance_id,
        )
        .await
        .map_err(|error| AgentRunPortError::Rejected(error.to_string()))?;
        let host_executable = match resolve_process_host_executable() {
            Ok(path) => path,
            Err(error) => {
                self.cleanup_failed_process_host_launch(attempt.run_attempt_id, None)
                    .await;
                return Err(error);
            }
        };
        let mut child = match Command::new(host_executable)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                self.cleanup_failed_process_host_launch(attempt.run_attempt_id, None)
                    .await;
                return Err(AgentRunPortError::Unavailable(error.to_string()));
            }
        };
        let host_pid = match child.id() {
            Some(pid) => pid,
            None => {
                self.cleanup_failed_process_host_launch(attempt.run_attempt_id, Some(&mut child))
                    .await;
                return Err(AgentRunPortError::Unavailable(
                    "process host has no OS pid".to_string(),
                ));
            }
        };
        let mut child_stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                self.cleanup_failed_process_host_launch(attempt.run_attempt_id, Some(&mut child))
                    .await;
                return Err(AgentRunPortError::Unavailable(
                    "process host stdin is unavailable".to_string(),
                ));
            }
        };
        if let Err(error) = write_json_frame(
            &mut child_stdin,
            &HostBootstrap {
                protocol_version: 1,
                run_attempt_id: attempt.run_attempt_id,
                host_instance_id,
                auth_token: auth_token.clone(),
                requested_endpoint: requested_endpoint.clone(),
            },
        )
        .await
        {
            self.cleanup_failed_process_host_launch(attempt.run_attempt_id, Some(&mut child))
                .await;
            return Err(AgentRunPortError::Unavailable(error.to_string()));
        }
        drop(child_stdin);
        let mut child_stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                self.cleanup_failed_process_host_launch(attempt.run_attempt_id, Some(&mut child))
                    .await;
                return Err(AgentRunPortError::Unavailable(
                    "process host stdout is unavailable".to_string(),
                ));
            }
        };
        let ready: HostReady = match read_json_frame(&mut child_stdout).await {
            Ok(ready) => ready,
            Err(error) => {
                self.cleanup_failed_process_host_launch(attempt.run_attempt_id, Some(&mut child))
                    .await;
                return Err(AgentRunPortError::Unavailable(error.to_string()));
            }
        };
        if ready.protocol_version != 1
            || ready.host_instance_id != host_instance_id
            || ready.host_pid != host_pid
        {
            self.cleanup_failed_process_host_launch(attempt.run_attempt_id, Some(&mut child))
                .await;
            return Err(AgentRunPortError::Unavailable(
                "process host handshake did not match the reservation".to_string(),
            ));
        }

        if ready.endpoint != requested_endpoint {
            self.cleanup_failed_process_host_launch(attempt.run_attempt_id, Some(&mut child))
                .await;
            return Err(AgentRunPortError::Unavailable(
                "process host bound a different endpoint than reserved".to_string(),
            ));
        }
        if let Err(error) = AgentRunRecord::mark_process_host_attached(
            &self.db.pool,
            attempt.run_attempt_id,
            host_pid,
        )
        .await
        {
            self.cleanup_failed_process_host_launch(attempt.run_attempt_id, Some(&mut child))
                .await;
            return Err(AgentRunPortError::Unavailable(error.to_string()));
        }

        let versions = provider.versions();
        let launch_request = HostLaunchRequest {
            run_attempt_id: attempt.run_attempt_id,
            provider,
            executor_config: launch.executor_config.clone(),
            intent: launch.intent,
            prompt: launch.prompt.clone(),
            provider_session: launch.provider_session.clone(),
            reset_to_message_id: launch.reset_to_message_id.clone(),
            selected_skills: launch.selected_skills.clone(),
            current_dir: launch.current_dir.clone(),
            env: HostExecutionEnv::from(&launch.env),
            audit_metadata: NativeAuditMetadata {
                session_id: request.session_id,
                agent_run_id: request.agent_run_id,
                turn_id: request.turn_id,
                run_attempt_id: attempt.run_attempt_id,
                run_attempt_number: attempt.attempt_number,
                provider_id: provider.id().to_string(),
                runtime_profile_id: request.runtime_profile_id.clone(),
                workspace_path: request.workspace.path.clone(),
                runtime_version: versions.runtime.map(str::to_owned),
                protocol_version: versions.protocol.map(str::to_owned),
                adapter_version: versions.adapter.to_string(),
                mapper_version: versions.mapper.to_string(),
                created_at: Utc::now(),
            },
            canonical_input: request.input.clone(),
            correlation_id: request.correlation_id,
            audited_launch_payload: match launch.audit_payload() {
                Ok(payload) => payload,
                Err(error) => {
                    self.cleanup_failed_process_host_launch(
                        attempt.run_attempt_id,
                        Some(&mut child),
                    )
                    .await;
                    return Err(AgentRunPortError::Unavailable(error.to_string()));
                }
            },
        };
        let response = send_host_command(
            &ready.endpoint,
            &auth_token,
            HostCommand::Launch(launch_request),
        )
        .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                // The command may have reached the durable host even when its
                // response was lost. Preserve the reservation and reconnect
                // through the host event cursor instead of killing a provider
                // that may already be running.
                let run_attempt_id = attempt.run_attempt_id;
                let port = self.clone();
                let request = request.clone();
                let attempt = attempt.clone();
                tokio::spawn(async move {
                    port.observe_process_host(request, attempt).await;
                });
                tokio::spawn(async move {
                    if let Err(wait_error) = child.wait().await {
                        tracing::debug!(%wait_error, "process host reaper lost its child handle");
                    }
                });
                tracing::debug!(run_attempt_id = %run_attempt_id, %error, "process host launch response unavailable; preserving durable host attachment");
                return Ok(());
            }
        };
        if let Some(error) = response.error {
            self.cleanup_failed_process_host_launch(attempt.run_attempt_id, Some(&mut child))
                .await;
            return Err(AgentRunPortError::Unavailable(error));
        }
        self.apply_host_events(request, attempt, response.events)
            .await?;
        let port = self.clone();
        let request = request.clone();
        let attempt = attempt.clone();
        tokio::spawn(async move {
            port.observe_process_host(request, attempt).await;
        });
        tokio::spawn(async move {
            if let Err(error) = child.wait().await {
                tracing::debug!(%error, "process host reaper lost its child handle");
            }
        });
        Ok(())
    }

    async fn observe_process_host(
        &self,
        request: AgentRunRequestEnvelope,
        attempt: RunAttemptRequest,
    ) {
        let mut consecutive_failures = 0usize;
        let mut subscription = None;
        loop {
            let attachment: Option<(
                String,
                String,
                i64,
                String,
                Option<i64>,
                Option<i64>,
                Option<String>,
            )> = sqlx::query_as(
                r#"
                SELECT host_endpoint, host_token, last_host_event_sequence, registry_status,
                       host_pid, pid, host_instance_id
                FROM agent_process_registry
                WHERE run_attempt_id = ? AND host_endpoint IS NOT NULL AND host_token IS NOT NULL
                "#,
            )
            .bind(attempt.run_attempt_id)
            .fetch_optional(&self.db.pool)
            .await
            .ok()
            .flatten();
            let Some((
                endpoint,
                token,
                cursor,
                registry_status,
                host_pid,
                provider_pid,
                host_instance_id,
            )) = attachment
            else {
                return;
            };
            if registry_status == "exited" {
                return;
            }
            let after_sequence = u64::try_from(cursor).unwrap_or_default();
            let response = async {
                if subscription.is_none() {
                    subscription =
                        Some(subscribe_host_events(&endpoint, &token, after_sequence).await?);
                }
                let response = read_host_subscription(
                    subscription.as_mut().expect("subscription established"),
                )
                .await?;
                if host_instance_id
                    .as_ref()
                    .is_some_and(|expected| *expected != response.host_instance_id.to_string())
                {
                    return Err(crate::transport::TransportError::Protocol(
                        "process host identity did not match persisted reservation".into(),
                    ));
                }
                Ok(response)
            }
            .await;
            match response {
                Ok(response) => {
                    consecutive_failures = 0;
                    if let Some(error) = response.error {
                        subscription = None;
                        tracing::warn!(run_attempt_id = %attempt.run_attempt_id, %error, "process host rejected attach");
                    } else if let Err(error) = self
                        .apply_host_events(&request, &attempt, response.events)
                        .await
                    {
                        // A failed projection must replay from the durable DB
                        // cursor, not continue beyond uncommitted observations.
                        subscription = None;
                        tracing::error!(run_attempt_id = %attempt.run_attempt_id, %error, "failed to project process-host observations");
                    }
                    let status: Option<String> = sqlx::query_scalar(
                        "SELECT registry_status FROM agent_process_registry WHERE run_attempt_id = ?",
                    )
                    .bind(attempt.run_attempt_id)
                    .fetch_optional(&self.db.pool)
                    .await
                    .ok()
                    .flatten();
                    if status.as_deref() == Some("exited") {
                        return;
                    }
                    if subscription.is_some() {
                        continue;
                    }
                }
                Err(error) => {
                    subscription = None;
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    if let Err(mark_error) = AgentRunRecord::mark_process_host_unreachable(
                        &self.db.pool,
                        attempt.run_attempt_id,
                    )
                    .await
                    {
                        tracing::error!(run_attempt_id = %attempt.run_attempt_id, %mark_error, "failed to persist unreachable process host");
                    }
                    tracing::debug!(run_attempt_id = %attempt.run_attempt_id, %error, "process host is temporarily unreachable");
                    let host_presence = match host_pid.and_then(|pid| u32::try_from(pid).ok()) {
                        Some(pid) => match self.process_registry.observe_pid(pid).await {
                            Ok(presence) => Some(presence),
                            Err(observation_error) => {
                                tracing::debug!(
                                    run_attempt_id = %attempt.run_attempt_id,
                                    pid,
                                    %observation_error,
                                    "could not observe unavailable process host"
                                );
                                None
                            }
                        },
                        // A host PID is written before its launch command is
                        // sent. A missing PID after repeated attach failures
                        // therefore describes a stale reservation, not a
                        // healthy host that should spin forever.
                        None => Some(RegisteredProcessPresence::Exited),
                    };
                    if should_reconcile_lost_process_host(consecutive_failures, host_presence) {
                        if host_presence == Some(RegisteredProcessPresence::Exited) {
                            if let Err(replay_error) =
                                self.replay_dead_host_journal(&request, &attempt).await
                            {
                                tracing::error!(run_attempt_id = %attempt.run_attempt_id, %replay_error, "failed to replay dead host journal");
                                // Do not terminalize past unprojected durable text.
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                continue;
                            }
                        }
                        self.reconcile_lost_process_host(
                            &request,
                            &attempt,
                            &registry_status,
                            provider_pid,
                            &error.to_string(),
                        )
                        .await;
                        return;
                    }
                    if consecutive_failures >= CANCELLING_OBSERVER_FAILURE_THRESHOLD
                        && self
                            .query(request.agent_run_id)
                            .await
                            .is_ok_and(|snapshot| {
                                snapshot.state.status == AgentRunStatus::Cancelling
                            })
                    {
                        let _ = self
                            .reconcile_cancel_after_host_failure(
                                &request,
                                &attempt,
                                &error.to_string(),
                                None,
                            )
                            .await;
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn replay_dead_host_journal(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
    ) -> Result<(), AgentRunPortError> {
        let instance: Option<String> = sqlx::query_scalar(
            "SELECT host_instance_id FROM agent_process_registry WHERE run_attempt_id = ?",
        )
        .bind(attempt.run_attempt_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(port_database)?
        .flatten();
        let Some(instance) = instance else {
            return Ok(());
        };
        let instance = Uuid::parse_str(&instance)
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
        let path = host_journal_path(attempt.run_attempt_id, instance);
        let mut replay =
            match HostJournalReplay::open(&path, self.load_host_cursor(attempt).await?).await {
                Ok(replay) => replay,
                Err(crate::process_host::ProcessHostError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Ok(());
                }
                Err(error) => return Err(AgentRunPortError::Unavailable(error.to_string())),
            };
        loop {
            let page = replay
                .next_page()
                .await
                .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
            if page.is_empty() {
                return Ok(());
            }
            self.apply_host_events(request, attempt, page).await?;
        }
    }

    async fn reconcile_lost_process_host(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        registry_status: &str,
        provider_pid: Option<i64>,
        host_error: &str,
    ) {
        let reconciliation_lock = self
            .cancellation_reconciliation_lock(attempt.run_attempt_id)
            .await;
        let reconciliation_guard = reconciliation_lock.lock().await;

        let Ok(snapshot) = self.query(request.agent_run_id).await else {
            return;
        };
        if snapshot.state.status.is_terminal() {
            let _ = self
                .reconcile_terminal_process(attempt.run_attempt_id)
                .await;
            return;
        }
        if snapshot.state.status == AgentRunStatus::Cancelling {
            // Cancellation has stronger precedence than an observer crash.
            // Release the shared lock before entering its reconciliation path.
            drop(reconciliation_guard);
            let _ = self
                .reconcile_cancel_after_host_failure(request, attempt, host_error, None)
                .await;
            return;
        }

        let runtime_absent = self
            .observe_attempt_processes_absent(attempt.run_attempt_id)
            .await
            .unwrap_or(false);

        if runtime_absent {
            if let Err(error) = self
                .process_registry
                .remove_runtime(attempt.run_attempt_id)
                .await
            {
                tracing::warn!(run_attempt_id = %attempt.run_attempt_id, %error, "failed to remove exited process projection");
                return;
            }
            if let Err(error) = AgentRunRecord::mark_process_exited(
                &self.db.pool,
                attempt.run_attempt_id,
                None,
                Utc::now(),
            )
            .await
            {
                tracing::warn!(
                    run_attempt_id = %attempt.run_attempt_id,
                    %error,
                    "failed to mark provider process exited after process-host exit"
                );
            }
        } else if provider_pid.is_none()
            && registry_status == "reserved"
            && let Err(error) = AgentRunRecord::clear_process_host_reservation(
                &self.db.pool,
                attempt.run_attempt_id,
            )
            .await
        {
            tracing::warn!(
                run_attempt_id = %attempt.run_attempt_id,
                %error,
                "failed to clear stale process-host reservation"
            );
        }

        let audit_failed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM native_audit_streams WHERE run_attempt_id = ? AND integrity_status = 'audit_failed')",
        )
        .bind(attempt.run_attempt_id)
        .fetch_one(&self.db.pool)
        .await
        .unwrap_or(false);
        let status = if audit_failed {
            AgentRunStatus::AuditFailed
        } else {
            AgentRunStatus::Crashed
        };
        let error_kind = if audit_failed {
            AgentRuntimeErrorKind::Unknown
        } else {
            AgentRuntimeErrorKind::ProcessCrashed
        };
        self.terminalize_failure(
            request,
            attempt,
            status,
            AgentRuntimeError::new(
                error_kind,
                format!("process host exited while observing the agent: {host_error}"),
            )
            .with_provider(Some(request.provider_id.as_str())),
        )
        .await;
    }

    /// Observe only: terminal outcome and process lifetime are independent.
    /// A reused, live or unobservable PID must never authorize deletion or kill.
    pub async fn reconcile_terminal_process(
        &self,
        attempt_id: Uuid,
    ) -> Result<bool, AgentRunPortError> {
        let row: Option<(String, Option<i64>, Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT registry.registry_status, registry.host_pid, registry.pid, registry.host_endpoint FROM agent_process_registry registry JOIN agent_run_attempts attempt ON attempt.id = registry.run_attempt_id WHERE attempt.id = ? AND attempt.status IN ('succeeded','failed','cancelled','crashed','audit_failed')",
        ).bind(attempt_id).fetch_optional(&self.db.pool).await.map_err(port_database)?;
        let Some((status, host_pid, provider_pid, host_endpoint)) = row else {
            return Ok(false);
        };
        if status == "exited" {
            self.process_registry
                .remove_runtime(attempt_id)
                .await
                .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
            return Ok(true);
        }
        // Missing provider PID is only expected before the provider was spawned.
        if provider_pid.is_none() && status != "reserved" {
            return Ok(false);
        }
        // A crash between spawning the host and persisting its PID can leave
        // a live authenticated endpoint. No PID is not confirmed absence.
        if status == "reserved" && host_endpoint.is_some() && host_pid.is_none() {
            return Ok(false);
        }
        for pid in [host_pid, provider_pid].into_iter().flatten() {
            let Ok(pid) = u32::try_from(pid) else {
                return Ok(false);
            };
            if pid == 0
                || !matches!(
                    self.process_registry.observe_pid(pid).await,
                    Ok(RegisteredProcessPresence::Exited)
                )
            {
                return Ok(false);
            }
        }
        if status == "reserved" {
            AgentRunRecord::clear_process_host_reservation(&self.db.pool, attempt_id)
                .await
                .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
        } else {
            AgentRunRecord::mark_process_exited(&self.db.pool, attempt_id, None, Utc::now())
                .await
                .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
        }
        self.process_registry
            .remove_runtime(attempt_id)
            .await
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
        Ok(true)
    }

    async fn observe_attempt_processes_absent(
        &self,
        attempt_id: Uuid,
    ) -> Result<bool, AgentRunPortError> {
        let row: Option<(String, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT registry_status, host_pid, pid FROM agent_process_registry WHERE run_attempt_id = ?",
        ).bind(attempt_id).fetch_optional(&self.db.pool).await.map_err(port_database)?;
        let Some((status, host_pid, provider_pid)) = row else {
            return Ok(false);
        };
        if status == "exited" {
            return Ok(true);
        }
        if provider_pid.is_none() {
            return Ok(false);
        }
        for pid in [host_pid, provider_pid].into_iter().flatten() {
            let Ok(pid) = u32::try_from(pid) else {
                return Ok(false);
            };
            if pid == 0
                || !matches!(
                    self.process_registry.observe_pid(pid).await,
                    Ok(RegisteredProcessPresence::Exited)
                )
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Terminal processes cannot go through the lifecycle Cancel transition.
    /// Only an authenticated host may stop them; never kill a recorded PID.
    pub async fn stop_terminal_process_for_deletion(
        &self,
        agent_run_id: Uuid,
        attempt_id: Uuid,
    ) -> Result<(), AgentRunPortError> {
        if self.reconcile_terminal_process(attempt_id).await? {
            return Ok(());
        }
        let (request, attempt) = self.load_request(agent_run_id).await?;
        if attempt.run_attempt_id != attempt_id {
            return Err(AgentRunPortError::Rejected(
                "Cannot safely stop an older process attachment".to_string(),
            ));
        }
        let (endpoint, token) = self.load_host_attachment(&attempt).await?.ok_or_else(|| {
            AgentRunPortError::Unavailable(
                "Process exit cannot be verified; session was retained".to_string(),
            )
        })?;
        let attached =
            send_host_command(&endpoint, &token, HostCommand::Attach { after_sequence: 0 })
                .await
                .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
        let terminal_sequence = attached.events.iter().find_map(|event| {
            matches!(&event.payload, HostEventPayload::Terminal { .. }).then_some(event.sequence)
        });
        if let Some(sequence) = terminal_sequence {
            send_host_command(
                &endpoint,
                &token,
                HostCommand::AckTerminal {
                    through_sequence: sequence,
                },
            )
            .await
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
        } else {
            let bytes = encode_control(
                self.provider(&request.provider_id)?,
                &attempt.capability_snapshot,
                DirectControl::Cancel,
            )
            .map_err(|error| AgentRunPortError::Rejected(error.to_string()))?;
            let response = send_host_command(
                &endpoint,
                &token,
                HostCommand::Control {
                    bytes,
                    control: Some(DirectControl::Cancel),
                    cancel: true,
                    after_sequence: self.load_host_cursor(&attempt).await?,
                },
            )
            .await
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
            if let Some(error) = response.error {
                return Err(AgentRunPortError::Unavailable(error));
            }
            if let Some(sequence) = response.events.iter().find_map(|event| {
                matches!(&event.payload, HostEventPayload::Terminal { .. })
                    .then_some(event.sequence)
            }) {
                send_host_command(
                    &endpoint,
                    &token,
                    HostCommand::AckTerminal {
                        through_sequence: sequence,
                    },
                )
                .await
                .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
            }
        }
        // Do not replay terminal lifecycle events over the already final result.
        for _ in 0..20 {
            if self.reconcile_terminal_process(attempt_id).await? {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        Err(AgentRunPortError::Unavailable(
            "Process has not exited; session was retained. Retry deletion.".to_string(),
        ))
    }

    async fn apply_host_events(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        events: Vec<crate::process_host::HostEvent>,
    ) -> Result<(), AgentRunPortError> {
        for event in events {
            let terminal = matches!(&event.payload, HostEventPayload::Terminal { .. });
            match event.payload {
                HostEventPayload::Started {
                    provider_pid,
                    process_group_id,
                    executable,
                    canonical_input_ref,
                    audit_manifest,
                } => {
                    self.ensure_audit_stream(&audit_manifest).await?;
                    AgentRunRecord::mark_process_started(
                        &self.db.pool,
                        attempt.run_attempt_id,
                        provider_pid,
                        process_group_id,
                        Some(&executable),
                        event.timestamp,
                    )
                    .await
                    .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
                    let registered = RegisteredAgentProcess::new(
                        attempt.run_attempt_id,
                        Some(request.session_id),
                        Some(request.workspace.workspace_id),
                        Some(request.provider_id.clone()),
                        provider_pid,
                        process_group_id,
                        Some(executable),
                    );
                    self.process_registry
                        .register(registered)
                        .await
                        .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
                    self.append_event(
                        request,
                        attempt,
                        AgentEventPayload::Message {
                            message: request.input.clone(),
                            final_output: false,
                        },
                        vec![canonical_input_ref],
                        request.created_at,
                        Some(request.input.message_id),
                    )
                    .await?;
                    self.append_event(
                        request,
                        attempt,
                        AgentEventPayload::LifecycleChanged {
                            status: AgentRunStatus::Running,
                        },
                        Vec::new(),
                        event.timestamp,
                        Some(event.event_id),
                    )
                    .await?;
                }
                HostEventPayload::Mapped {
                    event: mapped,
                    native_ref,
                } => {
                    self.append_mapped_event(request, attempt, mapped, native_ref)
                        .await?;
                }
                HostEventPayload::Terminal {
                    mut status,
                    error,
                    error_event_id,
                    exit_code,
                    audit_manifest,
                } => {
                    self.ensure_audit_stream(&audit_manifest).await?;
                    NativeAuditStreamRecord::finalize(&self.db.pool, &audit_manifest)
                        .await
                        .map_err(port_database)?;
                    let current_status = self
                        .query(request.agent_run_id)
                        .await
                        .ok()
                        .map(|snapshot| snapshot.state.status);
                    if current_status == Some(AgentRunStatus::Cancelling) {
                        let process_registry_status: Option<String> = sqlx::query_scalar(
                            "SELECT registry_status FROM agent_process_registry WHERE run_attempt_id = ?",
                        )
                        .bind(attempt.run_attempt_id)
                        .fetch_optional(&self.db.pool)
                        .await
                        .map_err(port_database)?;
                        let process_was_confirmed_exited =
                            process_registry_status.as_deref() == Some("exited");
                        status = prefer_cancellation_terminal(status, process_was_confirmed_exited);
                    }
                    // A late provider failure can race with an accepted user
                    // cancellation. Preserve it in Native Audit, but do not
                    // project it as the cancelled run's canonical last_error.
                    if status != AgentRunStatus::Cancelled
                        && let Some(error) = error
                    {
                        self.append_event(
                            request,
                            attempt,
                            AgentEventPayload::Error { error },
                            Vec::new(),
                            event.timestamp,
                            error_event_id,
                        )
                        .await?;
                    }
                    // A transient snapshot/read failure must not discard the
                    // host's durable terminal fact.  Append it and let the
                    // canonical projection path report degradation; when a
                    // successful read proves the run is already terminal,
                    // the reducer's terminal guard makes a duplicate safe.
                    let should_append_terminal = self
                        .query(request.agent_run_id)
                        .await
                        .map(|current| !current.state.status.is_terminal())
                        .unwrap_or(true);
                    if should_append_terminal {
                        self.append_event(
                            request,
                            attempt,
                            AgentEventPayload::LifecycleChanged { status },
                            Vec::new(),
                            event.timestamp,
                            Some(event.event_id),
                        )
                        .await?;
                    }
                    // A provider can fail before a child is spawned. In that
                    // case the host emits a terminal event without Started,
                    // and the registry must remain `reserved` to satisfy its
                    // CHECK constraints. Once a PID exists, transition it to
                    // exited exactly once and remove the local projection.
                    let pid: Option<i64> = sqlx::query_scalar(
                        "SELECT pid FROM agent_process_registry WHERE run_attempt_id = ?",
                    )
                    .bind(attempt.run_attempt_id)
                    .fetch_optional(&self.db.pool)
                    .await
                    .map_err(port_database)?;
                    if pid.is_some() {
                        AgentRunRecord::mark_process_exited(
                            &self.db.pool,
                            attempt.run_attempt_id,
                            exit_code,
                            event.timestamp,
                        )
                        .await
                        .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
                        if let Err(error) = self
                            .process_registry
                            .remove_runtime(attempt.run_attempt_id)
                            .await
                        {
                            tracing::warn!(run_attempt_id = %attempt.run_attempt_id, %error, "failed to remove exited process registry entry");
                        }
                    }
                }
            }
            AgentRunRecord::advance_process_host_cursor(
                &self.db.pool,
                attempt.run_attempt_id,
                event.sequence,
            )
            .await
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
            if terminal {
                if let Some((endpoint, token)) = self.load_host_attachment(attempt).await? {
                    let _ = send_host_command(
                        &endpoint,
                        &token,
                        HostCommand::AckTerminal {
                            through_sequence: event.sequence,
                        },
                    )
                    .await;
                }
            }
        }
        Ok(())
    }

    async fn ensure_audit_stream(
        &self,
        manifest: &executors::runtime::NativeAuditManifest,
    ) -> Result<(), AgentRunPortError> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM native_audit_streams WHERE run_attempt_id = ?)",
        )
        .bind(manifest.run_attempt_id)
        .fetch_one(&self.db.pool)
        .await
        .map_err(port_database)?;
        if !exists {
            NativeAuditStreamRecord::insert_open(&self.db.pool, manifest)
                .await
                .map_err(port_database)?;
        }
        Ok(())
    }

    async fn load_host_attachment(
        &self,
        attempt: &RunAttemptRequest,
    ) -> Result<Option<(String, String)>, AgentRunPortError> {
        sqlx::query_as(
            "SELECT host_endpoint, host_token FROM agent_process_registry WHERE run_attempt_id = ? AND host_endpoint IS NOT NULL AND host_token IS NOT NULL",
        )
        .bind(attempt.run_attempt_id)
        .fetch_optional(&self.db.pool)
        .await
            .map_err(port_database)
    }

    async fn load_host_cursor(
        &self,
        attempt: &RunAttemptRequest,
    ) -> Result<u64, AgentRunPortError> {
        let cursor: Option<i64> = sqlx::query_scalar(
            "SELECT last_host_event_sequence FROM agent_process_registry WHERE run_attempt_id = ?",
        )
        .bind(attempt.run_attempt_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(port_database)?;
        Ok(cursor
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or_default())
    }

    async fn start_attempt(
        &self,
        request: AgentRunRequestEnvelope,
        attempt: RunAttemptRequest,
        workspace: Workspace,
    ) {
        {
            let mut launching = self.launching_attempts.lock().await;
            if !launching.insert(attempt.run_attempt_id) {
                return;
            }
        }
        let run_attempt_id = attempt.run_attempt_id;
        self.start_attempt_reserved(request, attempt, workspace)
            .await;
        self.launching_attempts.lock().await.remove(&run_attempt_id);
    }

    async fn start_attempt_reserved(
        &self,
        request: AgentRunRequestEnvelope,
        attempt: RunAttemptRequest,
        workspace: Workspace,
    ) {
        if self
            .children
            .read()
            .await
            .contains_key(&attempt.run_attempt_id)
        {
            return;
        }
        let registry: Option<(String, Option<i64>, Option<String>, Option<i64>)> = match sqlx::query_as(
            "SELECT registry_status, pid, host_endpoint, host_pid FROM agent_process_registry WHERE run_attempt_id = ?",
        )
        .bind(attempt.run_attempt_id)
        .fetch_optional(&self.db.pool)
        .await {
            Ok(registry) => registry,
            Err(error) => {
                self.terminalize_failure(&request, &attempt, AgentRunStatus::Failed,
                    AgentRuntimeError::new(AgentRuntimeErrorKind::StartupFailed, error.to_string())).await;
                return;
            }
        };
        if let Some((status, pid, endpoint, host_pid)) = registry {
            if matches!(status.as_str(), "spawned" | "running" | "unreachable") && pid.is_some() {
                return;
            }
            if status == "exited" {
                return;
            }
            if endpoint.is_some() || host_pid.is_some() {
                match WorkflowMainSessionBinding::find_by_session_id(
                    &self.db.pool,
                    request.session_id,
                )
                .await
                {
                    Ok(Some(_)) => {
                        // The raw credential cannot be recovered from its
                        // hash. Observe a handed-off host without reissuing a
                        // token or resending a different launch environment.
                        let port = self.clone();
                        tokio::spawn(async move {
                            port.observe_process_host(request, attempt).await;
                        });
                        return;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(run_attempt_id=%attempt.run_attempt_id, %error, "cannot verify reserved workflow host; preserving launch reservation");
                        return;
                    }
                }
            }
        }

        let provider = match self.provider(&request.provider_id) {
            Ok(provider) => provider,
            Err(error) => {
                self.terminalize_failure(
                    &request,
                    &attempt,
                    AgentRunStatus::Failed,
                    AgentRuntimeError::new(AgentRuntimeErrorKind::StartupFailed, error.to_string()),
                )
                .await;
                return;
            }
        };
        let (env, workflow_prompt) = match self.execution_env(&request, &attempt, &workspace).await
        {
            Ok(Some(env)) => env,
            Ok(None) => {
                tracing::warn!(
                    run_attempt_id = %attempt.run_attempt_id,
                    "workflow MCP credential already issued; preserving existing handoff without re-launching"
                );
                return;
            }
            Err(error) => {
                self.terminalize_failure(
                    &request,
                    &attempt,
                    AgentRunStatus::Failed,
                    AgentRuntimeError::new(AgentRuntimeErrorKind::StartupFailed, error.to_string())
                        .with_provider(Some(provider.id())),
                )
                .await;
                return;
            }
        };
        let launch = FrozenDirectProviderLaunchSpec::new(&request, &attempt, provider, env)
            .with_workflow_prompt(workflow_prompt);
        self.append_recoverable(
            &request,
            &attempt,
            AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Starting,
            },
            Vec::new(),
            Utc::now(),
            None,
        )
        .await;

        if let Err(error) = self
            .launch_process_host(&request, &attempt, provider, &launch)
            .await
        {
            self.terminalize_failure(
                &request,
                &attempt,
                if matches!(error, AgentRunPortError::Unavailable(_)) {
                    AgentRunStatus::AuditFailed
                } else {
                    AgentRunStatus::Failed
                },
                AgentRuntimeError::new(AgentRuntimeErrorKind::StartupFailed, error.to_string())
                    .with_provider(Some(provider.id())),
            )
            .await;
        }
        /*
        let mut spawned = match launch_direct_provider(launch.launch_request()).await {
            Ok(spawned) => spawned,
            Err(error) => {
                let runtime_error =
                    AgentRuntimeError::from_executor_error(&error, Some(provider.id()));
                if let Err(audit_error) = self.finalize_audit(&attempt).await {
                    self.terminalize_failure(
                        &request,
                        &attempt,
                        AgentRunStatus::AuditFailed,
                        AgentRuntimeError::new(
                            AgentRuntimeErrorKind::Unknown,
                            audit_error.to_string(),
                        )
                        .with_provider(Some(provider.id())),
                    )
                    .await;
                } else {
                    self.terminalize_failure(
                        &request,
                        &attempt,
                        AgentRunStatus::Failed,
                        runtime_error,
                    )
                    .await;
                }
                return;
            }
        };

        let Some(pid) = spawned.child.inner().id() else {
            let _ = utils::process::kill_process_group(&mut spawned.child).await;
            self.fail_audit_writer(&attempt).await;
            self.terminalize_failure(
                &request,
                &attempt,
                AgentRunStatus::Crashed,
                AgentRuntimeError::new(
                    AgentRuntimeErrorKind::ProcessCrashed,
                    "spawned AgentRun process has no OS pid",
                )
                .with_provider(Some(provider.id())),
            )
            .await;
            return;
        };
        let registered = RegisteredAgentProcess::new(
            attempt.run_attempt_id,
            Some(request.session_id),
            Some(request.workspace.workspace_id),
            Some(provider.id().to_string()),
            pid,
            Some(pid),
            Some(provider.versions().executable.to_string()),
        );
        if let Err(error) = self.process_registry.register(registered).await {
            let _ = utils::process::kill_process_group(&mut spawned.child).await;
            self.fail_audit_writer(&attempt).await;
            self.terminalize_failure(
                &request,
                &attempt,
                AgentRunStatus::Failed,
                AgentRuntimeError::new(AgentRuntimeErrorKind::StartupFailed, error.to_string())
                    .with_provider(Some(provider.id())),
            )
            .await;
            return;
        }
        if let Err(error) = AgentRunRecord::mark_process_started(
            &self.db.pool,
            attempt.run_attempt_id,
            pid,
            Some(pid),
            Some(provider.versions().executable),
            Utc::now(),
        )
        .await
        {
            let _ = utils::process::kill_process_group(&mut spawned.child).await;
            let _ = self
                .process_registry
                .remove_runtime(attempt.run_attempt_id)
                .await;
            self.fail_audit_writer(&attempt).await;
            self.terminalize_failure(
                &request,
                &attempt,
                AgentRunStatus::Failed,
                AgentRuntimeError::new(AgentRuntimeErrorKind::StartupFailed, error.to_string())
                    .with_provider(Some(provider.id())),
            )
            .await;
            return;
        }

        let Some(stdout) = spawned.child.inner().stdout.take() else {
            let _ = utils::process::kill_process_group(&mut spawned.child).await;
            self.fail_started_attempt(
                &request,
                &attempt,
                provider,
                "process stdout is unavailable",
            )
            .await;
            return;
        };
        let Some(stderr) = spawned.child.inner().stderr.take() else {
            let _ = utils::process::kill_process_group(&mut spawned.child).await;
            self.fail_started_attempt(
                &request,
                &attempt,
                provider,
                "process stderr is unavailable",
            )
            .await;
            return;
        };
        let child = Arc::new(RwLock::new(spawned.child));
        self.children
            .write()
            .await
            .insert(attempt.run_attempt_id, child.clone());
        if let Some(cancel) = spawned.cancel {
            self.cancellation_tokens
                .write()
                .await
                .insert(attempt.run_attempt_id, cancel);
        }
        self.append_recoverable(
            &request,
            &attempt,
            AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Running,
            },
            Vec::new(),
            Utc::now(),
            None,
        )
        .await;

        let (output_tx, output_rx) = mpsc::unbounded_channel();
        self.spawn_stdout_reader(
            request.clone(),
            attempt.clone(),
            provider,
            stdout,
            output_tx.clone(),
        );
        self.spawn_stderr_reader(attempt.clone(), stderr, output_tx);
        self.spawn_attempt_monitor(
            request,
            attempt,
            provider,
            child,
            spawned.exit_signal,
            output_rx,
        );
        */
    }

    async fn fail_started_attempt(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        provider: DirectProvider,
        message: &str,
    ) {
        let _ = AgentRunRecord::mark_process_exited(
            &self.db.pool,
            attempt.run_attempt_id,
            None,
            Utc::now(),
        )
        .await;
        let _ = self
            .process_registry
            .remove_runtime(attempt.run_attempt_id)
            .await;
        match self.finalize_audit(attempt).await {
            Ok(()) => {
                self.terminalize_failure(
                    request,
                    attempt,
                    AgentRunStatus::Crashed,
                    AgentRuntimeError::new(AgentRuntimeErrorKind::ProcessCrashed, message)
                        .with_provider(Some(provider.id())),
                )
                .await;
            }
            Err(error) => {
                self.terminalize_failure(
                    request,
                    attempt,
                    AgentRunStatus::AuditFailed,
                    AgentRuntimeError::new(AgentRuntimeErrorKind::Unknown, error.to_string())
                        .with_provider(Some(provider.id())),
                )
                .await;
            }
        }
    }

    fn spawn_stdout_reader(
        &self,
        request: AgentRunRequestEnvelope,
        attempt: RunAttemptRequest,
        provider: DirectProvider,
        stdout: tokio::process::ChildStdout,
        notices: mpsc::UnboundedSender<OutputNotice>,
    ) {
        let port = self.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                match reader.read_until(b'\n', &mut bytes).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let (frame, native_ref) = match port
                            .append_audit_bytes(
                                &attempt,
                                NativeAuditChannel::Stdout,
                                "application/json",
                                &bytes,
                            )
                            .await
                        {
                            Ok(value) => value,
                            Err(error) => {
                                let _ = notices.send(OutputNotice::AuditFailure(error.to_string()));
                                break;
                            }
                        };
                        let decoded = match provider.decode_native_frame(&frame) {
                            Ok(decoded) => decoded,
                            Err(error) => {
                                let _ =
                                    notices.send(OutputNotice::ProtocolFailure(error.to_string()));
                                break;
                            }
                        };
                        let mapped = {
                            let writers = port.audit_writers.lock().await;
                            let Some(writer) = writers.get(&attempt.run_attempt_id) else {
                                let _ = notices.send(OutputNotice::AuditFailure(
                                    "Native Audit writer disappeared during decode".to_string(),
                                ));
                                break;
                            };
                            provider.map_provider_event(&decoded, writer.manifest())
                        };
                        let mapped = match mapped {
                            Ok(mapped) => mapped,
                            Err(error) => {
                                let _ =
                                    notices.send(OutputNotice::ProtocolFailure(error.to_string()));
                                break;
                            }
                        };
                        for event in mapped {
                            let terminal_status = match &event.payload {
                                AgentEventPayload::LifecycleChanged { status }
                                    if status.is_terminal() =>
                                {
                                    Some(*status)
                                }
                                _ => None,
                            };
                            let _ = port
                                .append_mapped_event(&request, &attempt, event, native_ref.clone())
                                .await;
                            if let Some(status) = terminal_status {
                                let _ = notices.send(OutputNotice::ProviderTerminal(status));
                                break;
                            }
                        }
                    }
                    Err(error) => {
                        let _ = notices.send(OutputNotice::ProtocolFailure(error.to_string()));
                        break;
                    }
                }
            }
            let _ = notices.send(OutputNotice::StdoutClosed);
        });
    }

    fn spawn_stderr_reader(
        &self,
        attempt: RunAttemptRequest,
        stderr: tokio::process::ChildStderr,
        notices: mpsc::UnboundedSender<OutputNotice>,
    ) {
        let port = self.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            loop {
                let mut bytes = Vec::new();
                match reader.read_until(b'\n', &mut bytes).await {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Err(error) = port
                            .append_audit_bytes(
                                &attempt,
                                NativeAuditChannel::Stderr,
                                "text/plain",
                                &bytes,
                            )
                            .await
                        {
                            let _ = notices.send(OutputNotice::AuditFailure(error.to_string()));
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = notices.send(OutputNotice::ProtocolFailure(error.to_string()));
                        break;
                    }
                }
            }
            let _ = notices.send(OutputNotice::StderrClosed);
        });
    }

    fn spawn_attempt_monitor(
        &self,
        request: AgentRunRequestEnvelope,
        attempt: RunAttemptRequest,
        provider: DirectProvider,
        child: SharedChild,
        exit_signal: Option<ExecutorExitSignal>,
        mut output_rx: mpsc::UnboundedReceiver<OutputNotice>,
    ) {
        let port = self.clone();
        tokio::spawn(async move {
            let mut stdout_closed = false;
            let mut stderr_closed = false;
            let mut exit_signal: ExitSignalFuture = match exit_signal {
                Some(signal) => Box::pin(signal),
                None => Box::pin(std::future::pending()),
            };
            let mut process_exit = Box::pin(wait_for_process_exit(child.clone()));
            let outcome = loop {
                tokio::select! {
                    result = &mut exit_signal => {
                        break match result {
                            Ok(result) => AttemptExit::Executor(result),
                            Err(_) => AttemptExit::ExecutorChannelClosed,
                        };
                    }
                    result = &mut process_exit => {
                        break match result {
                            Ok((success, code)) => AttemptExit::Process { success, code },
                            Err(error) => AttemptExit::WatcherFailed(error.to_string()),
                        };
                    }
                    notice = output_rx.recv(), if !(stdout_closed && stderr_closed) => {
                        match notice {
                            Some(OutputNotice::StdoutClosed) => stdout_closed = true,
                            Some(OutputNotice::StderrClosed) => stderr_closed = true,
                            Some(OutputNotice::ProviderTerminal(status)) => {
                                break AttemptExit::ProviderTerminal(status);
                            }
                            Some(failure @ (OutputNotice::AuditFailure(_) | OutputNotice::ProtocolFailure(_))) => {
                                break AttemptExit::OutputFailure(failure);
                            }
                            None => {
                                stdout_closed = true;
                                stderr_closed = true;
                            }
                        }
                    }
                }
            };

            if !matches!(outcome, AttemptExit::Process { .. }) {
                let mut child = child.write().await;
                let _ = utils::process::kill_process_group(&mut child).await;
            }
            let mut post_exit_failure = None;
            while !(stdout_closed && stderr_closed) {
                match output_rx.recv().await {
                    Some(OutputNotice::StdoutClosed) => stdout_closed = true,
                    Some(OutputNotice::StderrClosed) => stderr_closed = true,
                    Some(OutputNotice::ProviderTerminal(_)) => {}
                    Some(OutputNotice::AuditFailure(error)) => {
                        post_exit_failure = Some(OutputNotice::AuditFailure(error));
                    }
                    Some(OutputNotice::ProtocolFailure(error)) if post_exit_failure.is_none() => {
                        post_exit_failure = Some(OutputNotice::ProtocolFailure(error));
                    }
                    Some(OutputNotice::ProtocolFailure(_)) => {}
                    None => break,
                }
            }

            let exit_code = match &outcome {
                AttemptExit::Executor(ExecutorExitResult::Success) => Some(0),
                AttemptExit::Executor(ExecutorExitResult::Failure) => Some(1),
                AttemptExit::ProviderTerminal(AgentRunStatus::Succeeded) => Some(0),
                AttemptExit::ProviderTerminal(_) => None,
                AttemptExit::Process { code, .. } => *code,
                _ => None,
            };
            let audit_result = port.finalize_audit(&attempt).await;
            let current = port.query(request.agent_run_id).await.ok();
            if let Err(error) = audit_result {
                port.terminalize_failure(
                    &request,
                    &attempt,
                    AgentRunStatus::AuditFailed,
                    AgentRuntimeError::new(AgentRuntimeErrorKind::Unknown, error.to_string())
                        .with_provider(Some(provider.id())),
                )
                .await;
            } else if let Some(OutputNotice::AuditFailure(error)) = post_exit_failure.as_ref() {
                port.terminalize_failure(
                    &request,
                    &attempt,
                    AgentRunStatus::AuditFailed,
                    AgentRuntimeError::new(AgentRuntimeErrorKind::Unknown, error)
                        .with_provider(Some(provider.id())),
                )
                .await;
            } else if current
                .as_ref()
                .is_some_and(|snapshot| snapshot.state.status == AgentRunStatus::Cancelling)
            {
                port.append_recoverable(
                    &request,
                    &attempt,
                    AgentEventPayload::LifecycleChanged {
                        status: AgentRunStatus::Cancelled,
                    },
                    Vec::new(),
                    Utc::now(),
                    None,
                )
                .await;
            } else if current
                .as_ref()
                .is_some_and(|snapshot| snapshot.state.status.is_terminal())
            {
                // A provider terminal fact or explicit cancellation is already
                // canonical. A late OS/adapter observation must not overwrite it.
            } else {
                let failure = post_exit_failure.or_else(|| match &outcome {
                    AttemptExit::OutputFailure(OutputNotice::AuditFailure(error)) => {
                        Some(OutputNotice::AuditFailure(error.clone()))
                    }
                    AttemptExit::OutputFailure(OutputNotice::ProtocolFailure(error)) => {
                        Some(OutputNotice::ProtocolFailure(error.clone()))
                    }
                    _ => None,
                });
                match failure {
                    Some(OutputNotice::AuditFailure(error)) => {
                        port.terminalize_failure(
                            &request,
                            &attempt,
                            AgentRunStatus::AuditFailed,
                            AgentRuntimeError::new(AgentRuntimeErrorKind::Unknown, error)
                                .with_provider(Some(provider.id())),
                        )
                        .await;
                    }
                    Some(OutputNotice::ProtocolFailure(error)) => {
                        port.terminalize_failure(
                            &request,
                            &attempt,
                            AgentRunStatus::Failed,
                            AgentRuntimeError::new(AgentRuntimeErrorKind::OutputParseFailed, error)
                                .with_provider(Some(provider.id())),
                        )
                        .await;
                    }
                    _ => match outcome {
                        AttemptExit::Executor(ExecutorExitResult::Success) => {
                            port.append_recoverable(
                                &request,
                                &attempt,
                                AgentEventPayload::LifecycleChanged {
                                    status: AgentRunStatus::Succeeded,
                                },
                                Vec::new(),
                                Utc::now(),
                                None,
                            )
                            .await;
                        }
                        AttemptExit::Executor(ExecutorExitResult::Failure) => {
                            port.terminalize_failure(
                                &request,
                                &attempt,
                                AgentRunStatus::Failed,
                                AgentRuntimeError::new(
                                    AgentRuntimeErrorKind::Unknown,
                                    "provider executor reported failure",
                                )
                                .with_provider(Some(provider.id())),
                            )
                            .await;
                        }
                        AttemptExit::ExecutorChannelClosed => {
                            port.terminalize_failure(
                                &request,
                                &attempt,
                                AgentRunStatus::Crashed,
                                AgentRuntimeError::new(
                                    AgentRuntimeErrorKind::ProcessCrashed,
                                    "provider exit signal closed without a terminal result",
                                )
                                .with_provider(Some(provider.id())),
                            )
                            .await;
                        }
                        AttemptExit::ProviderTerminal(status) => {
                            port.append_recoverable(
                                &request,
                                &attempt,
                                AgentEventPayload::LifecycleChanged { status },
                                Vec::new(),
                                Utc::now(),
                                None,
                            )
                            .await;
                        }
                        AttemptExit::Process {
                            success: false,
                            code,
                        } => {
                            port.terminalize_failure(
                                &request,
                                &attempt,
                                AgentRunStatus::Failed,
                                AgentRuntimeError::new(
                                    AgentRuntimeErrorKind::ProcessCrashed,
                                    "AgentRun process exited unsuccessfully",
                                )
                                .with_provider(Some(provider.id()))
                                .with_exit_code(code.and_then(|value| i32::try_from(value).ok())),
                            )
                            .await;
                        }
                        AttemptExit::Process { success: true, .. } => {
                            // Exit code zero is not proof that a provider task
                            // completed. Providers without an explicit success
                            // event or executor signal fail closed.
                            port.terminalize_failure(
                                &request,
                                &attempt,
                                AgentRunStatus::Crashed,
                                AgentRuntimeError::new(
                                    AgentRuntimeErrorKind::ProcessCrashed,
                                    "process exited without provider success evidence",
                                )
                                .with_provider(Some(provider.id())),
                            )
                            .await;
                        }
                        AttemptExit::WatcherFailed(error) => {
                            port.terminalize_failure(
                                &request,
                                &attempt,
                                AgentRunStatus::Crashed,
                                AgentRuntimeError::new(
                                    AgentRuntimeErrorKind::ProcessCrashed,
                                    error,
                                )
                                .with_provider(Some(provider.id())),
                            )
                            .await;
                        }
                        AttemptExit::OutputFailure(_) => unreachable!(),
                    },
                }
            }

            if let Err(error) = AgentRunRecord::mark_process_exited(
                &port.db.pool,
                attempt.run_attempt_id,
                exit_code,
                Utc::now(),
            )
            .await
            {
                tracing::error!(
                    run_attempt_id = %attempt.run_attempt_id,
                    %error,
                    "failed to mark AgentRun process exited"
                );
            }
            if let Err(error) = port
                .process_registry
                .remove_runtime(attempt.run_attempt_id)
                .await
            {
                tracing::warn!(
                    run_attempt_id = %attempt.run_attempt_id,
                    %error,
                    "failed to remove local AgentRun process registry entry"
                );
            }
            port.children.write().await.remove(&attempt.run_attempt_id);
            port.cancellation_tokens
                .write()
                .await
                .remove(&attempt.run_attempt_id);
        });
    }

    async fn validate_durable_command(
        &self,
        command: &AgentRunPortCommandEnvelope,
    ) -> Result<(), AgentRunPortError> {
        let stored: Option<Json<AgentRunPortCommandEnvelope>> = sqlx::query_scalar(
            r#"
            SELECT command_envelope FROM agent_run_commands
            WHERE command_id = ? AND idempotency_key = ?
            UNION ALL
            SELECT command_envelope FROM orchestration_outbox
            WHERE command_id = ? AND idempotency_key = ?
            LIMIT 1
            "#,
        )
        .bind(command.command_id)
        .bind(&command.idempotency_key)
        .bind(command.command_id)
        .bind(&command.idempotency_key)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(port_database)?;
        match stored {
            Some(stored) if stored.0 == *command => Ok(()),
            Some(_) => Err(AgentRunPortError::Rejected(
                "durable AgentRun command payload does not match delivery".to_string(),
            )),
            None => Err(AgentRunPortError::Rejected(
                "control command is not present in a durable command store".to_string(),
            )),
        }
    }

    async fn write_attached_control(
        &self,
        attempt: &RunAttemptRequest,
        bytes: &[u8],
        direct_control: DirectControl,
    ) -> Result<(), AgentRunPortError> {
        if let Some((endpoint, token)) = self.load_host_attachment(attempt).await? {
            let after_sequence = self.load_host_cursor(attempt).await?;
            let response = send_host_command(
                &endpoint,
                &token,
                HostCommand::Control {
                    bytes: bytes.to_vec(),
                    control: Some(direct_control),
                    cancel: false,
                    after_sequence,
                },
            )
            .await
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
            let (request, _) = self.load_request(attempt.agent_run_id).await?;
            self.apply_host_events(&request, attempt, response.events)
                .await?;
            return response
                .error
                .map_or(Ok(()), |error| Err(AgentRunPortError::Unavailable(error)));
        }
        Err(AgentRunPortError::Unavailable(
            "AgentRun process host attachment is not persisted".to_string(),
        ))
    }

    async fn cancel_attached_attempt(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
    ) -> Result<(), AgentRunPortError> {
        let provider = self.provider(&request.provider_id)?;
        let control = encode_control(
            provider,
            &attempt.capability_snapshot,
            DirectControl::Cancel,
        )
        .map_err(|error| AgentRunPortError::Rejected(error.to_string()))?;
        if let Some((endpoint, token)) = self.load_host_attachment(attempt).await? {
            let after_sequence = self.load_host_cursor(attempt).await?;
            let response = match send_host_command(
                &endpoint,
                &token,
                HostCommand::Control {
                    bytes: control,
                    control: Some(DirectControl::Cancel),
                    cancel: true,
                    after_sequence,
                },
            )
            .await
            {
                Ok(response) => response,
                Err(error) => {
                    return self
                        .reconcile_cancel_after_host_failure(
                            request,
                            attempt,
                            &error.to_string(),
                            None,
                        )
                        .await;
                }
            };
            let response_error = response.error;
            if let Some(error) = response_error {
                return self
                    .reconcile_cancel_after_host_failure(
                        request,
                        attempt,
                        &error,
                        Some(response.events),
                    )
                    .await;
            }
            self.apply_host_events(request, attempt, response.events)
                .await?;
            if let Some(status) = self.current_terminal_status(request.agent_run_id).await {
                return cancellation_terminal_result(status);
            }
            return self
                .reconcile_cancel_after_host_failure(
                    request,
                    attempt,
                    "process host acknowledged cancellation without a terminal event",
                    None,
                )
                .await;
        }
        // No provider bytes were written without a durable host attachment.
        // Keep the fail-closed writer check, but do not record a synthetic
        // stdin frame that could be mistaken for an actual provider input.
        if let Err(error) = self.require_audit_writer(attempt).await {
            let message = error.to_string();
            self.terminalize_failure(
                request,
                attempt,
                AgentRunStatus::AuditFailed,
                AgentRuntimeError::new(AgentRuntimeErrorKind::Unknown, message.clone())
                    .with_provider(Some(provider.id())),
            )
            .await;
            return Err(AgentRunPortError::Unavailable(message));
        }
        Err(AgentRunPortError::Unavailable(
            "AgentRun process host attachment is not persisted".to_string(),
        ))
    }

    async fn current_terminal_status(&self, agent_run_id: Uuid) -> Option<AgentRunStatus> {
        self.query(agent_run_id)
            .await
            .ok()
            .map(|snapshot| snapshot.state.status)
            .filter(|status| status.is_terminal())
    }

    async fn replay_attached_host_once(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
    ) -> Result<Option<AgentRunStatus>, AgentRunPortError> {
        let Some((endpoint, token)) = self.load_host_attachment(attempt).await? else {
            return Err(AgentRunPortError::Unavailable(
                "AgentRun process host attachment is not persisted".to_string(),
            ));
        };
        let after_sequence = self.load_host_cursor(attempt).await?;
        let response = send_host_command(&endpoint, &token, HostCommand::Attach { after_sequence })
            .await
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
        let response_error = response.error;
        self.apply_host_events(request, attempt, response.events)
            .await?;
        let terminal = self.current_terminal_status(request.agent_run_id).await;
        if terminal.is_none()
            && let Some(error) = response_error
        {
            return Err(AgentRunPortError::Unavailable(error));
        }
        Ok(terminal)
    }

    async fn replay_cancel_terminal_until_deadline(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
    ) -> Option<AgentRunStatus> {
        let deadline = tokio::time::Instant::now() + CANCEL_HOST_REPLAY_DEADLINE;
        loop {
            if let Ok(Some(status)) = self.replay_attached_host_once(request, attempt).await {
                return Some(status);
            }
            if tokio::time::Instant::now() >= deadline {
                return self.current_terminal_status(request.agent_run_id).await;
            }
            tokio::time::sleep(CANCEL_HOST_REPLAY_INTERVAL).await;
        }
    }

    async fn reconcile_cancel_after_host_failure(
        &self,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        host_error: &str,
        initial_events: Option<Vec<crate::process_host::HostEvent>>,
    ) -> Result<(), AgentRunPortError> {
        // A user cancel and a background host observer can discover the same
        // transport failure concurrently. Serialize fallback per attempt so
        // the second caller reuses the first caller's terminal fact instead
        // of treating an already-cleaned registry as a failed cleanup.
        let reconciliation_lock = self
            .cancellation_reconciliation_lock(attempt.run_attempt_id)
            .await;
        let _reconciliation_guard = reconciliation_lock.lock().await;

        if let Some(status) = self.current_terminal_status(request.agent_run_id).await {
            return cancellation_terminal_result(status);
        }
        if let Err(error) =
            AgentRunRecord::mark_process_host_unreachable(&self.db.pool, attempt.run_attempt_id)
                .await
        {
            tracing::warn!(run_attempt_id = %attempt.run_attempt_id, %error, "failed to mark process host unreachable during cancellation fallback");
        }

        // A persisted PID is not process identity: it may have been reused.
        // Only the authenticated host may terminate its owned child. Transport
        // failure falls back to observation, never taskkill/SIGTERM by old PID.
        let runtime_absent = self
            .observe_attempt_processes_absent(attempt.run_attempt_id)
            .await
            .unwrap_or(false);
        if !runtime_absent {
            if let Some(events) = initial_events {
                self.apply_host_events(request, attempt, events).await?;
            }
            if let Some(status) = self.current_terminal_status(request.agent_run_id).await {
                return cancellation_terminal_result(status);
            }
            let message = format!(
                "process host cancellation failed ({host_error}); process exit cannot be verified safely"
            );
            self.terminalize_failure(
                request,
                attempt,
                AgentRunStatus::Crashed,
                AgentRuntimeError::new(AgentRuntimeErrorKind::ProcessCrashed, message.clone())
                    .with_provider(Some(request.provider_id.clone())),
            )
            .await;
            return Err(AgentRunPortError::Unavailable(message));
        }

        self.process_registry
            .remove_runtime(attempt.run_attempt_id)
            .await
            .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
        if let Err(error) = AgentRunRecord::mark_process_exited(
            &self.db.pool,
            attempt.run_attempt_id,
            None,
            Utc::now(),
        )
        .await
        {
            tracing::warn!(run_attempt_id = %attempt.run_attempt_id, %error, "failed to persist cancellation fallback process exit");
        }
        if let Some(events) = initial_events {
            self.apply_host_events(request, attempt, events).await?;
            if let Some(status) = self.current_terminal_status(request.agent_run_id).await {
                return cancellation_terminal_result(status);
            }
        }
        if let Some(status) = self
            .replay_cancel_terminal_until_deadline(request, attempt)
            .await
        {
            return cancellation_terminal_result(status);
        }

        let message = format!(
            "provider process was terminated after process host failure ({host_error}), but Native Audit did not produce a terminal manifest"
        );
        self.terminalize_failure(
            request,
            attempt,
            AgentRunStatus::AuditFailed,
            AgentRuntimeError::new(AgentRuntimeErrorKind::Unknown, message.clone())
                .with_provider(Some(request.provider_id.clone())),
        )
        .await;
        Err(AgentRunPortError::Unavailable(message))
    }

    async fn cancellation_reconciliation_lock(&self, run_attempt_id: Uuid) -> Arc<Mutex<()>> {
        let mut locks = self.cancellation_reconciliation_locks.lock().await;
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&run_attempt_id).and_then(Weak::upgrade) {
            return lock;
        }

        let lock = Arc::new(Mutex::new(()));
        locks.insert(run_attempt_id, Arc::downgrade(&lock));
        lock
    }
}

fn cancellation_terminal_result(status: AgentRunStatus) -> Result<(), AgentRunPortError> {
    if status == AgentRunStatus::Cancelled {
        Ok(())
    } else {
        Err(AgentRunPortError::Unavailable(format!(
            "cancellation converged to terminal status {status:?}"
        )))
    }
}

fn should_reconcile_lost_process_host(
    consecutive_failures: usize,
    host_presence: Option<RegisteredProcessPresence>,
) -> bool {
    consecutive_failures >= PROCESS_HOST_OBSERVER_FAILURE_THRESHOLD
        && host_presence == Some(RegisteredProcessPresence::Exited)
}

fn prefer_cancellation_terminal(
    provider_status: AgentRunStatus,
    process_was_confirmed_exited: bool,
) -> AgentRunStatus {
    match provider_status {
        AgentRunStatus::AuditFailed => AgentRunStatus::AuditFailed,
        AgentRunStatus::Cancelled | AgentRunStatus::Succeeded | AgentRunStatus::Failed => {
            AgentRunStatus::Cancelled
        }
        AgentRunStatus::Crashed if process_was_confirmed_exited => AgentRunStatus::Cancelled,
        status => status,
    }
}

#[async_trait]
impl AgentRunPort for LocalAgentRunPort {
    async fn create(
        &self,
        request: AgentRunRequestEnvelope,
        attempt: RunAttemptRequest,
    ) -> Result<Uuid, AgentRunPortError> {
        let agent_run_id = self.reserve(request, attempt).await?;
        self.launch_reserved(agent_run_id).await?;
        // Identity acceptance is the create acknowledgement. Launch/audit
        // failures are canonical terminal facts observed through query/subscribe.
        Ok(agent_run_id)
    }

    async fn query(&self, agent_run_id: Uuid) -> Result<AgentRunPortSnapshot, AgentRunPortError> {
        let state: Option<Json<executors::runtime::RunState>> =
            sqlx::query_scalar("SELECT state_json FROM agent_run_state WHERE agent_run_id = ?")
                .bind(agent_run_id)
                .fetch_optional(&self.db.pool)
                .await
                .map_err(port_database)?;
        state
            .map(|state| AgentRunPortSnapshot {
                agent_run_id,
                state: state.0,
            })
            .ok_or(AgentRunPortError::NotFound(agent_run_id))
    }

    async fn control(&self, command: AgentRunPortCommandEnvelope) -> Result<(), AgentRunPortError> {
        command
            .validate_current()
            .map_err(|error| AgentRunPortError::Rejected(error.to_string()))?;
        self.validate_durable_command(&command).await?;
        // Serialize controls within this supervisor. Durable command identity
        // still protects retries across supervisor instances, while this lock
        // prevents cancel/retry/input races between local tasks.
        let _command_guard = self.command_lock.lock().await;
        let agent_run_id = command.agent_run_id;
        match &command.command {
            AgentRunPortCommand::Cancel { .. } => {
                let (request, attempt) = self.load_request(agent_run_id).await?;
                let state = self.query(agent_run_id).await?;
                if state.state.status.is_terminal() {
                    return Ok(());
                }
                if state.state.status != AgentRunStatus::Cancelling {
                    self.append_lifecycle(&request, &attempt, AgentRunStatus::Cancelling)
                        .await?;
                }
                self.cancel_attached_attempt(&request, &attempt).await
            }
            AgentRunPortCommand::SubmitInput { input_id, content } => {
                let (request, attempt) = self.load_request(agent_run_id).await?;
                if self.query(agent_run_id).await?.state.status.is_terminal() {
                    return Err(AgentRunPortError::Rejected(
                        "cannot submit input to a terminal AgentRun".to_string(),
                    ));
                }
                let provider = self.provider(&request.provider_id)?;
                let bytes = encode_control(
                    provider,
                    &attempt.capability_snapshot,
                    DirectControl::Input {
                        request_id: input_id.clone(),
                        text: content.clone(),
                    },
                )
                .map_err(|error| AgentRunPortError::Rejected(error.to_string()))?;
                self.append_event(
                    &request,
                    &attempt,
                    AgentEventPayload::Message {
                        message: CanonicalMessage {
                            message_id: command.command_id,
                            role: AgentRuntimeMessageRole::User,
                            content: content.clone(),
                        },
                        final_output: false,
                    },
                    Vec::new(),
                    command.created_at,
                    Some(command.command_id),
                )
                .await?;
                self.write_attached_control(
                    &attempt,
                    &bytes,
                    DirectControl::Input {
                        request_id: input_id.clone(),
                        text: content.clone(),
                    },
                )
                .await?;
                self.append_event(
                    &request,
                    &attempt,
                    AgentEventPayload::InputResolved {
                        input_id: input_id.clone(),
                        answered: true,
                    },
                    Vec::new(),
                    Utc::now(),
                    None,
                )
                .await
            }
            AgentRunPortCommand::ResolveApproval {
                approval_id,
                approved,
                reason,
            } => {
                let (request, attempt) = self.load_request(agent_run_id).await?;
                if self.query(agent_run_id).await?.state.status.is_terminal() {
                    return Err(AgentRunPortError::Rejected(
                        "cannot resolve approval for a terminal AgentRun".to_string(),
                    ));
                }
                let provider = self.provider(&request.provider_id)?;
                let bytes = encode_control(
                    provider,
                    &attempt.capability_snapshot,
                    DirectControl::Approve {
                        request_id: approval_id.clone(),
                        approved: *approved,
                        reason: reason.clone(),
                    },
                )
                .map_err(|error| AgentRunPortError::Rejected(error.to_string()))?;
                self.write_attached_control(
                    &attempt,
                    &bytes,
                    DirectControl::Approve {
                        request_id: approval_id.clone(),
                        approved: *approved,
                        reason: reason.clone(),
                    },
                )
                .await?;
                self.append_event(
                    &request,
                    &attempt,
                    AgentEventPayload::ApprovalResolved {
                        approval_id: approval_id.clone(),
                        approved: *approved,
                        reason: reason.clone(),
                    },
                    Vec::new(),
                    Utc::now(),
                    None,
                )
                .await
            }
            AgentRunPortCommand::Retry {
                mode,
                run_attempt_id,
            } => {
                if *mode == RunAttemptMode::Launch {
                    return Err(AgentRunPortError::Rejected(
                        "retry mode must be resume or restart".to_string(),
                    ));
                }
                let (request, previous) = self.load_request(agent_run_id).await?;
                let state = self.query(agent_run_id).await?;
                if !state.state.status.is_terminal() {
                    return Err(AgentRunPortError::Rejected(
                        "cannot retry an active AgentRun".to_string(),
                    ));
                }
                // Retry shares the command lock with launch_reserved. A
                // terminal old Run cannot bypass a newer Session reservation
                // or enter the same native Session before a peer exits.
                let busy: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM agent_runs r LEFT JOIN agent_run_state s ON s.agent_run_id=r.id WHERE r.session_id=? AND r.id<>? AND (s.agent_run_id IS NULL OR s.status NOT IN ('succeeded','failed','cancelled','crashed','audit_failed')))",
                ).bind(request.session_id).bind(agent_run_id).fetch_one(&self.db.pool)
                    .await.map_err(port_database)?;
                if busy {
                    return Err(AgentRunPortError::Rejected(
                        "cannot retry while another Session Run is active or reserved".into(),
                    ));
                }
                let attempts: Vec<Uuid> = sqlx::query_scalar(
                    "SELECT a.id FROM agent_run_attempts a JOIN agent_runs r ON r.id=a.agent_run_id WHERE r.session_id=?",
                ).bind(request.session_id).fetch_all(&self.db.pool).await.map_err(port_database)?;
                for attempt_id in attempts {
                    if !self.reconcile_terminal_process(attempt_id).await? {
                        return Err(AgentRunPortError::Rejected(
                            "cannot retry before all Session processes have confirmed exit".into(),
                        ));
                    }
                }
                let provider = self.provider(&request.provider_id)?;
                let provider_session = if *mode == RunAttemptMode::Resume {
                    require_capability(
                        provider,
                        &previous.capability_snapshot,
                        AgentCapability::SessionResume,
                        false,
                    )
                    .map_err(|error| AgentRunPortError::Rejected(error.to_string()))?;
                    Some(state.state.provider_session.clone().ok_or_else(|| {
                        AgentRunPortError::Rejected(
                            "resume retry requires an observed provider session".to_string(),
                        )
                    })?)
                } else {
                    None
                };
                let attempt = RunAttemptRequest {
                    schema_version: previous.schema_version,
                    payload_version: previous.payload_version,
                    request_id: command.command_id,
                    idempotency_key: command.idempotency_key.clone(),
                    session_id: previous.session_id,
                    agent_run_id: previous.agent_run_id,
                    turn_id: previous.turn_id,
                    run_attempt_id: *run_attempt_id,
                    attempt_number: previous.attempt_number + 1,
                    correlation_id: previous.correlation_id,
                    mode: *mode,
                    transport: previous.transport,
                    runtime_profile_id: previous.runtime_profile_id.clone(),
                    provider_id: previous.provider_id.clone(),
                    workspace: previous.workspace.clone(),
                    capability_snapshot: previous.capability_snapshot.clone(),
                    executor_config: previous.executor_config.clone(),
                    selected_skills: previous.selected_skills.clone(),
                    reset_to_message_id: previous.reset_to_message_id.clone(),
                    provider_session,
                    created_at: command.created_at,
                };
                let workspace = self.validate_workspace(&request).await?;
                AgentRunRecord::persist_retry_attempt_before_launch(
                    &self.db.pool,
                    &request,
                    &attempt,
                )
                .await
                .map_err(|error| AgentRunPortError::Rejected(error.to_string()))?;
                self.start_attempt(request, attempt, workspace).await;
                Ok(())
            }
            AgentRunPortCommand::Create { .. } => Err(AgentRunPortError::Rejected(
                "create commands must use AgentRunPort::create".to_string(),
            )),
        }
    }

    async fn subscribe(&self, agent_run_id: Uuid) -> Result<AgentEventStream, AgentRunPortError> {
        self.query(agent_run_id).await?;
        let receiver = self.sender(agent_run_id).await.subscribe();
        let history: Vec<Json<AgentEventEnvelope>> = sqlx::query_scalar(
            "SELECT event_envelope FROM agent_events WHERE agent_run_id = ? ORDER BY run_attempt_number, sequence",
        )
        .bind(agent_run_id)
        .fetch_all(&self.db.pool)
        .await
        .map_err(port_database)?;
        let history_event_ids: HashSet<Uuid> =
            history.iter().map(|event| event.0.event_id).collect();
        let live = stream::unfold(
            (receiver, history_event_ids),
            |(mut receiver, mut seen)| async move {
                loop {
                    match receiver.recv().await {
                        Ok(event) if seen.insert(event.event_id) => {
                            return Some((event, (receiver, seen)));
                        }
                        Ok(_) => continue,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                }
            },
        );
        Ok(stream::iter(history.into_iter().map(|event| event.0))
            .chain(live)
            .boxed())
    }
}

async fn wait_for_process_exit(child: SharedChild) -> std::io::Result<(bool, Option<i64>)> {
    loop {
        let status = child.write().await.try_wait()?;
        if let Some(status) = status {
            return Ok((status.success(), status.code().map(i64::from)));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn resolve_process_host_executable() -> Result<PathBuf, AgentRunPortError> {
    if let Some(path) = std::env::var_os("VIBE_KANBAN_AGENT_PROCESS_HOST") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(AgentRunPortError::Unavailable(format!(
            "configured process host does not exist: {}",
            path.display()
        )));
    }
    if let Some(path) = option_env!("CARGO_BIN_EXE_agent-process-host") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    let current_exe = std::env::current_exe()
        .map_err(|error| AgentRunPortError::Unavailable(error.to_string()))?;
    let file_name = if cfg!(windows) {
        "agent-process-host.exe"
    } else {
        "agent-process-host"
    };
    let sibling = current_exe.with_file_name(file_name);
    if sibling.is_file() {
        return Ok(sibling);
    }
    if current_exe
        .parent()
        .is_some_and(|directory| directory.ends_with("deps"))
        && let Some(target_directory) = current_exe.parent().and_then(Path::parent)
    {
        let target_sibling = target_directory.join(file_name);
        if target_sibling.is_file() {
            return Ok(target_sibling);
        }
    }
    Err(AgentRunPortError::Unavailable(format!(
        "agent process host executable was not found next to {}",
        current_exe.display()
    )))
}

fn port_database(error: sqlx::Error) -> AgentRunPortError {
    AgentRunPortError::Unavailable(error.to_string())
}

fn port_audit(error: executors::runtime::NativeAuditError) -> AgentRunPortError {
    AgentRunPortError::Unavailable(error.to_string())
}

#[cfg(test)]
mod tests {
    use executors::{
        command::CmdOverrides,
        executors::{
            BaseCodingAgent, CodingAgent, StandardCodingAgentExecutor,
            acp::{
                AcpEvent,
                session_config::{AcpCatalogObservation, catalog_identity},
            },
        },
        model_selector::{ModelInfo, ModelSelectorConfig, ReasoningOption},
        runtime::{
            AGENT_REQUEST_PAYLOAD_VERSION, AGENT_REQUEST_SCHEMA_VERSION, AgentRuntimeMessageRole,
            CanonicalMessage, NativeAuditDirection, WorkspaceMode, WorkspaceReference,
        },
    };
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    async fn setup_runtime_db() -> DBService {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .expect("enable foreign keys");
        sqlx::raw_sql(
            r#"
            CREATE TABLE workspaces (id BLOB PRIMARY KEY);
            CREATE TABLE sessions (
                id BLOB PRIMARY KEY,
                workspace_id BLOB NOT NULL,
                FOREIGN KEY (workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE
            );
            CREATE TABLE session_project_memberships (session_id BLOB PRIMARY KEY, project_id BLOB NOT NULL);
            CREATE TABLE workflows (id BLOB PRIMARY KEY, project_id BLOB);
            CREATE TABLE tasks (id BLOB PRIMARY KEY, project_id BLOB NOT NULL);
            CREATE TABLE workflow_attempts (id BLOB PRIMARY KEY,workflow_id BLOB,workspace_id BLOB,main_session_id BLOB,task_id BLOB);
            CREATE TABLE workflow_attempt_sources (attempt_id BLOB PRIMARY KEY,template_id BLOB);
            CREATE TABLE workflow_main_session_bindings (
                session_id BLOB PRIMARY KEY,workflow_id BLOB NOT NULL,prepared_issue_id BLOB,
                main_agent_config_json TEXT NOT NULL,main_agent_prompt TEXT NOT NULL,
                token_hash TEXT,actual_main_agent_run_id BLOB,actual_main_turn_id BLOB,actual_main_run_attempt_id BLOB,source_message_id BLOB,
                created_at TEXT NOT NULL DEFAULT (datetime('now','subsec')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now','subsec'))
            );
            "#,
        )
        .execute(&pool)
        .await
        .expect("create anchor schema");
        sqlx::raw_sql(include_str!(
            "../../db/migrations/20260811000000_agent_runtime_v1.sql"
        ))
        .execute(&pool)
        .await
        .expect("create runtime schema");
        DBService { pool }
    }

    async fn persisted_codex_run(db: &DBService) -> (AgentRunRequestEnvelope, RunAttemptRequest) {
        persisted_provider_run(db, BaseCodingAgent::Codex).await
    }

    async fn persisted_provider_run(
        db: &DBService,
        executor: BaseCodingAgent,
    ) -> (AgentRunRequestEnvelope, RunAttemptRequest) {
        let provider = DirectProvider::from_base_agent(executor).expect("direct provider");
        let runtime_profile_id = format!("{executor}:default");
        let workspace_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        sqlx::query("INSERT INTO workspaces (id) VALUES (?)")
            .bind(workspace_id)
            .execute(&db.pool)
            .await
            .expect("insert workspace");
        sqlx::query("INSERT INTO sessions (id, workspace_id) VALUES (?, ?)")
            .bind(session_id)
            .bind(workspace_id)
            .execute(&db.pool)
            .await
            .expect("insert session");

        let now = Utc::now();
        let workspace = WorkspaceReference {
            workspace_id,
            mode: WorkspaceMode::SharedWorkspace,
            path: "C:/workspace".to_string(),
        };
        let request = AgentRunRequestEnvelope {
            schema_version: AGENT_REQUEST_SCHEMA_VERSION,
            payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
            request_id: Uuid::new_v4(),
            idempotency_key: "cancel-audit-failure".to_string(),
            session_id,
            agent_run_id: Uuid::new_v4(),
            turn_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            intent: AgentRunIntent::Initial,
            runtime_profile_id: runtime_profile_id.clone(),
            provider_id: provider.id().to_string(),
            workspace: workspace.clone(),
            input: CanonicalMessage {
                message_id: Uuid::new_v4(),
                role: AgentRuntimeMessageRole::User,
                content: "cancel me".to_string(),
            },
            created_at: now,
        };
        let attempt = RunAttemptRequest {
            schema_version: AGENT_REQUEST_SCHEMA_VERSION,
            payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
            request_id: Uuid::new_v4(),
            idempotency_key: "cancel-audit-failure:attempt:1".to_string(),
            session_id,
            agent_run_id: request.agent_run_id,
            turn_id: request.turn_id,
            run_attempt_id: Uuid::new_v4(),
            attempt_number: 1,
            correlation_id: request.correlation_id,
            mode: RunAttemptMode::Launch,
            transport: provider.transport(),
            runtime_profile_id: request.runtime_profile_id.clone(),
            provider_id: request.provider_id.clone(),
            workspace,
            capability_snapshot: provider.capabilities(runtime_profile_id),
            executor_config: executors::profile::ExecutorConfig {
                executor,
                variant: Some("default".to_string()),
                model_id: None,
                agent_id: None,
                reasoning_id: None,
                permission_policy: None,
                provider_options: None,
            },
            selected_skills: None,
            reset_to_message_id: None,
            provider_session: None,
            created_at: now,
        };
        AgentRunRecord::persist_identity_before_launch(&db.pool, &request, &attempt)
            .await
            .expect("persist AgentRun identity");
        (request, attempt)
    }

    fn catalog_test_agent(
        executor: BaseCodingAgent,
        cmd: &CmdOverrides,
        model: Option<&str>,
    ) -> CodingAgent {
        serde_json::from_value(serde_json::json!({(executor.to_string()): {
            "base_command_override": cmd.base_command_override,
            "additional_params": cmd.additional_params,
            "env": cmd.env,
            "model": model,
        }}))
        .expect("provider profile")
    }

    async fn picker_catalog(agent: &CodingAgent, cwd: &Path) -> serde_json::Value {
        let mut patches = agent.discover_options(Some(cwd), None).await.unwrap();
        let patch = serde_json::to_value(patches.next().await.expect("discovery patch")).unwrap();
        assert_eq!(patch[0]["path"], "/options");
        patch[0]["value"]["model_selector"].clone()
    }

    #[tokio::test]
    async fn acp_catalog_host_observation_reaches_picker_discovery() {
        for executor in [
            BaseCodingAgent::Opencode,
            BaseCodingAgent::DeepseekHarness,
            BaseCodingAgent::OhMyPi,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let cwd = directory.path();
            let provider = DirectProvider::from_base_agent(executor).unwrap();
            let port = LocalAgentRunPort::new(setup_runtime_db().await);
            let (request, attempt) = persisted_provider_run(&port.db, executor).await;
            let cmd = CmdOverrides {
                // Discovery must not start this command or create an ACP session.
                base_command_override: Some("must-never-run".into()),
                env: Some(HashMap::from([(
                    "CUSTOM_MODEL_ROUTE".into(),
                    "native-route-a".into(),
                )])),
                ..Default::default()
            };
            let agent = catalog_test_agent(executor, &cmd, None);
            assert_eq!(
                picker_catalog(&agent, cwd).await["models"],
                serde_json::json!([])
            );
            assert_eq!(std::fs::read_dir(cwd).unwrap().count(), 0);

            let mut launched_cmd = cmd.clone();
            launched_cmd.env.as_mut().unwrap().extend([
                ("VK_AGENT_RUN_ID".into(), request.agent_run_id.to_string()),
                (
                    "VK_RUN_ATTEMPT_ID".into(),
                    attempt.run_attempt_id.to_string(),
                ),
                ("VK_WORKSPACE_BRANCH".into(), "auto-run-branch".into()),
                (
                    workflow_mcp::TOKEN_ENV.into(),
                    "private-workflow-secret".into(),
                ),
            ]);
            assert_eq!(catalog_identity(&launched_cmd), catalog_identity(&cmd));
            let model = if executor == BaseCodingAgent::DeepseekHarness {
                r#"["provider/name","Model/name"]"#
            } else {
                "provider/name/Model/name"
            };
            // Construct directly: observe_catalog would seed the server-side
            // cache before the serialized independent-host boundary is tested.
            let observation = AcpCatalogObservation {
                working_dir: if executor == BaseCodingAgent::OhMyPi {
                    std::fs::canonicalize(cwd).unwrap()
                } else {
                    cwd.to_path_buf()
                },
                scope_id: if executor == BaseCodingAgent::OhMyPi {
                    let identity = executors::executors::oh_my_pi::catalog::catalog_identity(
                        cwd,
                        &launched_cmd,
                    );
                    assert_eq!(
                        identity,
                        executors::executors::oh_my_pi::catalog::catalog_identity(cwd, &cmd)
                    );
                    identity
                } else {
                    let dialect = if executor == BaseCodingAgent::Opencode {
                        executors::executors::acp::session_config::AcpDialect::Opencode
                    } else {
                        executors::executors::acp::session_config::AcpDialect::DeepseekHarness
                    };
                    executors::executors::acp::session_config::scoped_catalog_identity(
                        dialect,
                        cwd,
                        &launched_cmd,
                    )
                },
                model_selector: ModelSelectorConfig {
                    models: vec![
                        ModelInfo {
                            id: model.into(),
                            name: "Native model".into(),
                            capabilities: None,
                            provider_id: None,
                            reasoning_options: vec![ReasoningOption {
                                id: "".into(),
                                label: "No effort".into(),
                                is_default: true,
                            }],
                        },
                        ModelInfo {
                            id: model.to_lowercase(),
                            name: "Other model".into(),
                            capabilities: None,
                            provider_id: None,
                            reasoning_options: Vec::new(),
                        },
                    ],
                    default_model: Some(model.into()),
                    ..Default::default()
                },
            };
            let versions = provider.versions();
            let mut writer = NativeAuditWriter::create_in(
                cwd,
                NativeAuditMetadata {
                    session_id: request.session_id,
                    agent_run_id: request.agent_run_id,
                    turn_id: request.turn_id,
                    run_attempt_id: attempt.run_attempt_id,
                    run_attempt_number: attempt.attempt_number,
                    provider_id: provider.id().into(),
                    runtime_profile_id: request.runtime_profile_id.clone(),
                    workspace_path: cwd.display().to_string(),
                    runtime_version: versions.runtime.map(str::to_owned),
                    protocol_version: versions.protocol.map(str::to_owned),
                    adapter_version: versions.adapter.into(),
                    mapper_version: versions.mapper.into(),
                    created_at: Utc::now(),
                },
            )
            .unwrap();
            let catalog_frame = if executor == BaseCodingAgent::OhMyPi {
                let mut frame = serde_json::to_value(&observation).unwrap();
                frame["type"] =
                    serde_json::json!(executors::executors::oh_my_pi::catalog::CATALOG_EVENT);
                frame.to_string()
            } else {
                AcpEvent::CatalogObserved(observation.clone()).to_string()
            };
            let frame = NativeAuditFrame::from_bytes(
                1,
                Utc::now(),
                NativeAuditDirection::Output,
                NativeAuditChannel::Stdout,
                "application/json",
                request.correlation_id,
                catalog_frame.as_bytes(),
                None,
            );
            let native_ref = writer.append(frame.clone()).unwrap();
            let manifest = writer.close().unwrap();
            let decoded = provider.decode_native_frame(&frame).unwrap();
            let mut mapped = provider.map_provider_event(&decoded, &manifest).unwrap();
            assert_eq!(mapped.len(), 1);
            let host_event = crate::process_host::HostEvent {
                sequence: 1,
                event_id: Uuid::new_v4(),
                timestamp: frame.timestamp,
                payload: HostEventPayload::Mapped {
                    event: mapped.remove(0),
                    native_ref,
                },
            };
            let wire = serde_json::to_string(&host_event).unwrap();
            assert!(!wire.contains("private-workflow-secret"));
            assert!(!wire.contains(workflow_mcp::TOKEN_ENV));
            let transported = serde_json::from_str(&wire).unwrap();
            port.ensure_audit_stream(&manifest).await.unwrap();
            port.apply_host_events(&request, &attempt, vec![transported])
                .await
                .unwrap();
            assert_eq!(port.load_host_cursor(&attempt).await.unwrap(), 1);

            let catalog = picker_catalog(&agent, cwd).await;
            assert_eq!(
                catalog,
                serde_json::to_value(&observation.model_selector).unwrap()
            );
            assert_eq!(catalog["default_model"], model);
            assert_eq!(catalog["models"][0]["reasoning_options"][0]["id"], "");
            assert_eq!(
                catalog["models"][1]["reasoning_options"],
                serde_json::json!([])
            );
            // Selection changes do not invent a different native authority.
            let selected = catalog_test_agent(executor, &cmd, Some("different-selection"));
            assert_eq!(picker_catalog(&selected, cwd).await, catalog);
            let other_executor = if executor == BaseCodingAgent::Opencode {
                BaseCodingAgent::DeepseekHarness
            } else {
                BaseCodingAgent::Opencode
            };
            let other_provider = catalog_test_agent(other_executor, &cmd, None);
            assert_eq!(
                picker_catalog(&other_provider, cwd).await["models"],
                serde_json::json!([])
            );
            let other_directory = tempfile::tempdir().unwrap();
            assert_eq!(
                picker_catalog(&agent, other_directory.path()).await["models"],
                serde_json::json!([])
            );
            let mut other_cmd = cmd.clone();
            other_cmd.base_command_override = Some("other-must-never-run".into());
            let other_agent = catalog_test_agent(executor, &other_cmd, None);
            assert_eq!(
                picker_catalog(&other_agent, cwd).await["models"],
                serde_json::json!([])
            );
            other_cmd = cmd;
            other_cmd
                .env
                .as_mut()
                .unwrap()
                .insert("CUSTOM_MODEL_ROUTE".into(), "native-route-b".into());
            let other_agent = catalog_test_agent(executor, &other_cmd, None);
            assert_eq!(
                picker_catalog(&other_agent, cwd).await["models"],
                serde_json::json!([])
            );
        }
    }

    fn test_execution_env(workspace_path: &str) -> ExecutionEnv {
        let mut env = ExecutionEnv::new(
            RepoContext::new(
                PathBuf::from(workspace_path),
                vec!["repository".to_string()],
            ),
            false,
            String::new(),
        );
        env.insert("VK_AGENT_RUN_ID", "frozen-run");
        env
    }

    async fn bind_workflow_main(
        port: &LocalAgentRunPort,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
    ) {
        let project_id = Uuid::new_v4();
        let workflow_id = Uuid::new_v4();
        sqlx::query("INSERT INTO session_project_memberships VALUES (?,?)")
            .bind(request.session_id)
            .bind(project_id)
            .execute(&port.db.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflows VALUES (?,?)")
            .bind(workflow_id)
            .bind(project_id)
            .execute(&port.db.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_main_session_bindings (session_id,workflow_id,main_agent_config_json,main_agent_prompt) VALUES (?,?,?,?)")
            .bind(request.session_id).bind(workflow_id).bind(Json(&attempt.executor_config))
            .bind("Manage the prepared workflow").execute(&port.db.pool).await.unwrap();
    }

    async fn next_main_turn(
        port: &LocalAgentRunPort,
        original: &AgentRunRequestEnvelope,
        previous: &RunAttemptRequest,
    ) -> (AgentRunRequestEnvelope, RunAttemptRequest) {
        let mut request = original.clone();
        request.request_id = Uuid::new_v4();
        request.agent_run_id = Uuid::new_v4();
        request.turn_id = Uuid::new_v4();
        request.correlation_id = Uuid::new_v4();
        request.idempotency_key = format!("main-next-{}", request.request_id);
        request.input.message_id = Uuid::new_v4();
        request.input.content = "Read the previous result".into();
        let mut attempt = previous.clone();
        attempt.request_id = request.request_id;
        attempt.agent_run_id = request.agent_run_id;
        attempt.turn_id = request.turn_id;
        attempt.correlation_id = request.correlation_id;
        attempt.run_attempt_id = Uuid::new_v4();
        attempt.idempotency_key = format!("{}:attempt", request.idempotency_key);
        AgentRunRecord::persist_identity_before_launch(&port.db.pool, &request, &attempt)
            .await
            .unwrap();
        (request, attempt)
    }

    #[tokio::test]
    async fn workflow_token_tracks_real_user_identity_without_persisting_secret() {
        let port = LocalAgentRunPort::new(setup_runtime_db().await);
        let (request, attempt) = persisted_codex_run(&port.db).await;
        bind_workflow_main(&port, &request, &attempt).await;
        let (token, prompt) = port
            .issue_workflow_mcp_token(&request, &attempt)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(prompt, "Manage the prepared workflow");
        let restarted = LocalAgentRunPort::new(port.db.clone());
        let binding =
            WorkflowMainSessionBinding::find_by_session_id(&restarted.db.pool, request.session_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(binding.token_hash, Some(workflow_mcp::token_hash(&token)));
        assert_eq!(binding.actual_main_agent_run_id, Some(request.agent_run_id));
        assert_eq!(binding.actual_main_turn_id, Some(request.turn_id));
        assert_eq!(
            binding.actual_main_run_attempt_id,
            Some(attempt.run_attempt_id)
        );
        assert_eq!(binding.source_message_id, Some(request.input.message_id));
        assert!(!format!("{binding:?}").contains(&token));
        // A restart has no in-memory lock/secret but must not replace the token
        // between its durable issuance and native-host reservation.
        assert!(
            restarted
                .issue_workflow_mcp_token(&request, &attempt)
                .await
                .unwrap()
                .is_none()
        );
        let mut wrong_input = request.clone();
        wrong_input.input.message_id = Uuid::new_v4();
        assert!(
            port.issue_workflow_mcp_token(&wrong_input, &attempt)
                .await
                .is_err()
        );
        wrong_input = request.clone();
        wrong_input.input.role = AgentRuntimeMessageRole::System;
        assert!(
            port.issue_workflow_mcp_token(&wrong_input, &attempt)
                .await
                .is_err()
        );
        let mut wrong_config = attempt.clone();
        wrong_config.executor_config.model_id = Some(Some("switched-model".into()));
        assert!(
            port.issue_workflow_mcp_token(&request, &wrong_config)
                .await
                .is_err()
        );
        AgentRunRecord::reserve_process_host(
            &port.db.pool,
            attempt.run_attempt_id,
            "127.0.0.1:12345",
            "host-auth",
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        assert!(
            port.issue_workflow_mcp_token(&request, &attempt)
                .await
                .is_err()
        );
        let after =
            WorkflowMainSessionBinding::find_by_session_id(&port.db.pool, request.session_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(after.token_hash, binding.token_hash);
    }

    #[tokio::test]
    async fn workflow_rotation_requires_previous_process_exit_not_finished_reply() {
        let port = LocalAgentRunPort::new(setup_runtime_db().await);
        let (previous, previous_attempt) = persisted_codex_run(&port.db).await;
        bind_workflow_main(&port, &previous, &previous_attempt).await;
        let (old_token, _) = port
            .issue_workflow_mcp_token(&previous, &previous_attempt)
            .await
            .unwrap()
            .unwrap();
        AgentRunRecord::mark_process_started(
            &port.db.pool,
            previous_attempt.run_attempt_id,
            std::process::id(),
            Some(std::process::id()),
            Some("test-main"),
            Utc::now(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE agent_run_attempts SET status='succeeded' WHERE id=?")
            .bind(previous_attempt.run_attempt_id)
            .execute(&port.db.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE agent_runs SET status='succeeded' WHERE id=?")
            .bind(previous.agent_run_id)
            .execute(&port.db.pool)
            .await
            .unwrap();
        let (request, attempt) = next_main_turn(&port, &previous, &previous_attempt).await;
        assert!(
            port.issue_workflow_mcp_token(&request, &attempt)
                .await
                .is_err()
        );
        let binding =
            WorkflowMainSessionBinding::find_by_session_id(&port.db.pool, request.session_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            binding.token_hash,
            Some(workflow_mcp::token_hash(&old_token))
        );
        AgentRunRecord::mark_process_exited(
            &port.db.pool,
            previous_attempt.run_attempt_id,
            Some(0),
            Utc::now(),
        )
        .await
        .unwrap();
        let (new_token, _) = port
            .issue_workflow_mcp_token(&request, &attempt)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(new_token, old_token);
        let binding =
            WorkflowMainSessionBinding::find_by_session_id(&port.db.pool, request.session_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(binding.actual_main_agent_run_id, Some(request.agent_run_id));
        assert_eq!(
            binding.actual_main_run_attempt_id,
            Some(attempt.run_attempt_id)
        );
        assert_eq!(binding.source_message_id, Some(request.input.message_id));
    }

    #[tokio::test]
    async fn workflow_token_competing_backends_issue_only_once_per_attempt() {
        let port = LocalAgentRunPort::new(setup_runtime_db().await);
        let (request, attempt) = persisted_codex_run(&port.db).await;
        bind_workflow_main(&port, &request, &attempt).await;
        let competing = LocalAgentRunPort::new(port.db.clone());
        let (first, second) = tokio::join!(
            port.issue_workflow_mcp_token(&request, &attempt),
            competing.issue_workflow_mcp_token(&request, &attempt),
        );
        let (token, _) = match (first, second) {
            (Ok(Some(issued)), Ok(None)) | (Ok(None), Ok(Some(issued))) => issued,
            _ => panic!("Exactly one backend must issue the attempt credential"),
        };
        let binding =
            WorkflowMainSessionBinding::find_by_session_id(&port.db.pool, request.session_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(binding.token_hash, Some(workflow_mcp::token_hash(&token)));
        assert_eq!(
            binding.actual_main_run_attempt_id,
            Some(attempt.run_attempt_id)
        );
    }

    #[tokio::test]
    async fn workflow_prompt_and_native_audit_preserve_provenance_without_token() {
        let port = LocalAgentRunPort::new(setup_runtime_db().await);
        let (request, attempt) = persisted_codex_run(&port.db).await;
        let secret = "a".repeat(64);
        let mut env = test_execution_env(&attempt.workspace.path);
        env.insert(workflow_mcp::TOKEN_ENV, &secret);
        let host_env = HostExecutionEnv::from(&env);
        assert!(!format!("{host_env:?}").contains(&secret));
        let launch =
            FrozenDirectProviderLaunchSpec::new(&request, &attempt, DirectProvider::Codex, env)
                .with_workflow_prompt(Some("Captured manager instructions".into()));
        assert!(launch.prompt.contains("Captured manager instructions"));
        assert!(launch.prompt.ends_with(&request.input.content));
        let audit = launch.audit_payload().unwrap();
        assert!(!String::from_utf8_lossy(&audit).contains(&secret));
        let payload: serde_json::Value = serde_json::from_slice(&audit).unwrap();
        assert!(
            payload["env"]["vars"]
                .get(workflow_mcp::TOKEN_ENV)
                .is_none()
        );
        // Only the native launch prompt is augmented. The persisted canonical
        // User input retains the exact real content and message identity.
        let saved: Json<CanonicalMessage> =
            sqlx::query_scalar("SELECT input_message FROM agent_turns WHERE id=?")
                .bind(request.turn_id)
                .fetch_one(&port.db.pool)
                .await
                .unwrap();
        assert_eq!(saved.0, request.input);
    }

    #[tokio::test]
    async fn direct_launch_and_audit_use_frozen_attempt_inputs() {
        let db = setup_runtime_db().await;
        let port = LocalAgentRunPort::new(db);
        let (request, mut attempt) = persisted_codex_run(&port.db).await;
        let selected_skills = vec![api_types::SelectedSkill {
            name: "runtime-contracts".to_string(),
            path: PathBuf::from("C:/skills/runtime-contracts/SKILL.md"),
        }];
        attempt.executor_config.model_id = Some(Some("openai/gpt-5.6-codex".to_string()));
        attempt.executor_config.reasoning_id = Some(Some("high".to_string()));
        attempt.selected_skills = Some(selected_skills.clone());

        let launch = FrozenDirectProviderLaunchSpec::new(
            &request,
            &attempt,
            DirectProvider::Codex,
            test_execution_env(&attempt.workspace.path),
        );
        let direct = launch.launch_request();
        assert_eq!(direct.provider, DirectProvider::Codex);
        assert_eq!(direct.intent, DirectIntent::Initial);
        assert_eq!(direct.executor_config, &attempt.executor_config);
        assert_eq!(direct.prompt, request.input.content);
        assert_eq!(direct.selected_skills, selected_skills);
        assert_eq!(direct.current_dir, Path::new(&attempt.workspace.path));
        assert_eq!(
            direct.env.get("VK_AGENT_RUN_ID").map(String::as_str),
            Some("frozen-run")
        );

        let audited: serde_json::Value = serde_json::from_slice(
            &launch
                .audit_payload()
                .expect("frozen direct launch must serialize for audit"),
        )
        .expect("audit payload must be JSON");
        assert_eq!(audited["provider"], "codex");
        assert_eq!(audited["intent"], "initial");
        assert_eq!(audited["prompt"], request.input.content);
        assert_eq!(
            audited["executor_config"],
            serde_json::to_value(&attempt.executor_config).unwrap()
        );
        assert_eq!(
            audited["selected_skills"],
            serde_json::to_value(&selected_skills).unwrap()
        );
        assert_eq!(audited["approval_behavior"], "noop");
        assert_eq!(audited["current_dir"], attempt.workspace.path);
        assert_eq!(audited["env"]["vars"]["VK_AGENT_RUN_ID"], "frozen-run");
    }

    #[tokio::test]
    async fn direct_follow_up_uses_frozen_session_and_reset_target() {
        let db = setup_runtime_db().await;
        let port = LocalAgentRunPort::new(db);
        let (mut request, mut attempt) = persisted_codex_run(&port.db).await;
        request.intent = AgentRunIntent::FollowUp;
        request.runtime_profile_id = "CLAUDE_CODE:default".to_string();
        request.provider_id = "claude_code".to_string();
        attempt.runtime_profile_id = request.runtime_profile_id.clone();
        attempt.provider_id = request.provider_id.clone();
        attempt.executor_config = executors::profile::ExecutorConfig {
            executor: executors::executors::BaseCodingAgent::ClaudeCode,
            variant: Some("default".to_string()),
            model_id: Some(Some("claude-sonnet-4".to_string())),
            agent_id: None,
            reasoning_id: None,
            permission_policy: None,
            provider_options: None,
        };
        attempt.reset_to_message_id = Some("message-17".to_string());
        attempt.provider_session = Some(executors::runtime::ProviderSessionReference {
            schema_version: executors::runtime::PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION,
            provider_id: request.provider_id.clone(),
            runtime_profile_id: request.runtime_profile_id.clone(),
            provider_session_id: "claude-session".to_string(),
            observed_at: Utc::now(),
            metadata: None,
        });

        let launch = FrozenDirectProviderLaunchSpec::new(
            &request,
            &attempt,
            DirectProvider::ClaudeCode,
            test_execution_env(&attempt.workspace.path),
        );
        let direct = launch.launch_request();
        assert_eq!(direct.intent, DirectIntent::FollowUp);
        assert_eq!(direct.executor_config, &attempt.executor_config);
        assert_eq!(direct.reset_to_message_id, Some("message-17"));
        assert_eq!(
            direct
                .provider_session
                .map(|session| session.provider_session_id.as_str()),
            Some("claude-session")
        );
    }

    #[test]
    fn direct_intent_explicitly_combines_run_intent_and_attempt_mode() {
        let cases = [
            (
                AgentRunIntent::Initial,
                RunAttemptMode::Launch,
                DirectIntent::Initial,
            ),
            (
                AgentRunIntent::Initial,
                RunAttemptMode::Resume,
                DirectIntent::Resume,
            ),
            (
                AgentRunIntent::Initial,
                RunAttemptMode::Restart,
                DirectIntent::Initial,
            ),
            (
                AgentRunIntent::FollowUp,
                RunAttemptMode::Launch,
                DirectIntent::FollowUp,
            ),
            (
                AgentRunIntent::FollowUp,
                RunAttemptMode::Resume,
                DirectIntent::Resume,
            ),
            (
                AgentRunIntent::FollowUp,
                RunAttemptMode::Restart,
                DirectIntent::Initial,
            ),
            (
                AgentRunIntent::Review,
                RunAttemptMode::Launch,
                DirectIntent::Review,
            ),
            (
                AgentRunIntent::Review,
                RunAttemptMode::Resume,
                DirectIntent::Resume,
            ),
            (
                AgentRunIntent::Review,
                RunAttemptMode::Restart,
                DirectIntent::Review,
            ),
        ];

        for (intent, mode, expected) in cases {
            assert_eq!(direct_intent(intent, mode), expected);
        }
    }

    #[tokio::test]
    async fn cancel_audit_failure_has_no_provider_or_process_side_effects() {
        let db = setup_runtime_db().await;
        let port = LocalAgentRunPort::new(db);
        let (request, attempt) = persisted_codex_run(&port.db).await;
        let cancellation = CancellationToken::new();
        port.cancellation_tokens
            .write()
            .await
            .insert(attempt.run_attempt_id, cancellation.clone());

        let error = port
            .cancel_attached_attempt(&request, &attempt)
            .await
            .expect_err("missing Native Audit writer must fail closed");

        assert!(matches!(error, AgentRunPortError::Unavailable(_)));
        assert_eq!(
            port.query(request.agent_run_id).await.unwrap().state.status,
            AgentRunStatus::AuditFailed
        );
        assert!(
            !cancellation.is_cancelled(),
            "audit failure must stop before the provider cancellation token"
        );
    }

    #[tokio::test]
    async fn terminal_registry_reconciliation_is_idempotent_and_preserves_outcome() {
        let db = setup_runtime_db().await;
        let mut port = LocalAgentRunPort::new(db);
        let registry_dir = tempfile::TempDir::new().unwrap();
        port.process_registry =
            AgentProcessRegistry::new(registry_dir.path().join("registry.json"));
        let (request, attempt) = persisted_codex_run(&port.db).await;
        // Keep the missing PID within the signed range accepted by tasklist
        // and Unix kill(0); u32::MAX is invalid input, not proof of absence.
        // This test only observes processes; it never spawns or terminates one.
        let missing_pid = i32::MAX as u32;
        AgentRunRecord::mark_process_started(
            &port.db.pool,
            attempt.run_attempt_id,
            missing_pid,
            None,
            Some("fixture-dead-provider"),
            Utc::now(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE agent_process_registry SET host_pid = ? WHERE run_attempt_id = ?")
            .bind(i64::from(missing_pid))
            .bind(attempt.run_attempt_id)
            .execute(&port.db.pool)
            .await
            .unwrap();
        AgentRunRecord::mark_process_host_unreachable(&port.db.pool, attempt.run_attempt_id)
            .await
            .unwrap();
        port.terminalize_failure(
            &request,
            &attempt,
            AgentRunStatus::Failed,
            AgentRuntimeError::new(AgentRuntimeErrorKind::Unknown, "fixture".to_string()),
        )
        .await;
        let before = port.query(request.agent_run_id).await.unwrap().state;
        assert!(
            port.reconcile_terminal_process(attempt.run_attempt_id)
                .await
                .unwrap()
        );
        assert!(
            port.reconcile_terminal_process(attempt.run_attempt_id)
                .await
                .unwrap()
        );
        let after = port.query(request.agent_run_id).await.unwrap().state;
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap()
        );
    }

    async fn assert_retry_rejected_without_new_attempt(
        port: &LocalAgentRunPort,
        request: &AgentRunRequestEnvelope,
        reason: &str,
    ) {
        let retry_attempt_id = Uuid::new_v4();
        let command_id = Uuid::new_v4();
        let command = AgentRunPortCommandEnvelope {
            schema_version: executors::runtime::ORCHESTRATION_COMMAND_SCHEMA_VERSION,
            command_id,
            idempotency_key: command_id.to_string(),
            agent_run_id: request.agent_run_id,
            orchestration_run_id: None,
            orchestration_node_execution_id: None,
            correlation_id: request.correlation_id,
            created_at: Utc::now(),
            command: AgentRunPortCommand::Retry {
                mode: RunAttemptMode::Restart,
                run_attempt_id: retry_attempt_id,
            },
        };
        db::models::agent_runtime::AgentRunCommandRecord::enqueue(&port.db.pool, &command)
            .await
            .unwrap();
        let error = port.control(command).await.unwrap_err();
        assert!(error.to_string().contains(reason), "{error}");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_run_attempts WHERE id=?")
            .bind(retry_attempt_id)
            .fetch_one(&port.db.pool)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "Rejected retry must not reserve another native attempt"
        );
    }

    #[tokio::test]
    async fn retry_cannot_bypass_another_session_reservation_or_unknown_state() {
        for missing_projection in [false, true] {
            let port = LocalAgentRunPort::new(setup_runtime_db().await);
            let (request, attempt) = persisted_codex_run(&port.db).await;
            port.terminalize_failure(
                &request,
                &attempt,
                AgentRunStatus::Failed,
                AgentRuntimeError::new(AgentRuntimeErrorKind::Unknown, "original failure"),
            )
            .await;
            let mut peer = request.clone();
            peer.agent_run_id = Uuid::new_v4();
            peer.turn_id = Uuid::new_v4();
            peer.request_id = Uuid::new_v4();
            peer.idempotency_key = "accepted-queued:queued".into();
            let mut peer_attempt = attempt.clone();
            peer_attempt.agent_run_id = peer.agent_run_id;
            peer_attempt.turn_id = peer.turn_id;
            peer_attempt.run_attempt_id = Uuid::new_v4();
            peer_attempt.request_id = Uuid::new_v4();
            peer_attempt.idempotency_key = "accepted-queued:queued:attempt:1".into();
            AgentRunRecord::persist_identity_before_launch(&port.db.pool, &peer, &peer_attempt)
                .await
                .unwrap();
            if missing_projection {
                sqlx::query("DELETE FROM agent_run_state WHERE agent_run_id=?")
                    .bind(peer.agent_run_id)
                    .execute(&port.db.pool)
                    .await
                    .unwrap();
            }
            assert_retry_rejected_without_new_attempt(
                &port,
                &request,
                "another Session Run is active or reserved",
            )
            .await;
        }
    }

    #[tokio::test]
    async fn retry_requires_actual_exit_even_after_terminal_outcome() {
        let mut port = LocalAgentRunPort::new(setup_runtime_db().await);
        let registry_dir = tempfile::TempDir::new().unwrap();
        port.process_registry =
            AgentProcessRegistry::new(registry_dir.path().join("registry.json"));
        let (request, attempt) = persisted_codex_run(&port.db).await;
        // Only observe the unrelated test process. Never cancel or kill it.
        AgentRunRecord::mark_process_started(
            &port.db.pool,
            attempt.run_attempt_id,
            std::process::id(),
            None,
            Some("unrelated-test-process"),
            Utc::now(),
        )
        .await
        .unwrap();
        AgentRunRecord::mark_process_host_unreachable(&port.db.pool, attempt.run_attempt_id)
            .await
            .unwrap();
        port.terminalize_failure(
            &request,
            &attempt,
            AgentRunStatus::Failed,
            AgentRuntimeError::new(AgentRuntimeErrorKind::Unknown, "terminal but alive"),
        )
        .await;
        assert_retry_rejected_without_new_attempt(
            &port,
            &request,
            "all Session processes have confirmed exit",
        )
        .await;
    }

    #[tokio::test]
    async fn workflow_reserved_endpoint_without_host_pid_is_not_confirmed_exit() {
        let mut port = LocalAgentRunPort::new(setup_runtime_db().await);
        let registry_dir = tempfile::TempDir::new().unwrap();
        port.process_registry =
            AgentProcessRegistry::new(registry_dir.path().join("registry.json"));
        let (request, attempt) = persisted_codex_run(&port.db).await;
        bind_workflow_main(&port, &request, &attempt).await;
        let (token, _) = port
            .issue_workflow_mcp_token(&request, &attempt)
            .await
            .unwrap()
            .unwrap();
        let host_instance_id = Uuid::new_v4();
        AgentRunRecord::reserve_process_host(
            &port.db.pool,
            attempt.run_attempt_id,
            "127.0.0.1:12345",
            "fixture-host-auth",
            host_instance_id,
        )
        .await
        .unwrap();
        // No real process is started. The durable handoff is uncertain because
        // a host may have started before its PID could be recorded.
        port.terminalize_failure(
            &request,
            &attempt,
            AgentRunStatus::Failed,
            AgentRuntimeError::new(
                AgentRuntimeErrorKind::StartupFailed,
                "fixture interrupted handoff",
            ),
        )
        .await;
        let before = port.query(request.agent_run_id).await.unwrap().state;
        assert!(
            !port
                .reconcile_terminal_process(attempt.run_attempt_id)
                .await
                .unwrap()
        );
        let row: (String, Option<i64>, String, String) = sqlx::query_as(
            "SELECT registry_status,host_pid,host_endpoint,host_token FROM agent_process_registry WHERE run_attempt_id=?",
        ).bind(attempt.run_attempt_id).fetch_one(&port.db.pool).await.unwrap();
        assert_eq!(
            row,
            (
                "reserved".into(),
                None,
                "127.0.0.1:12345".into(),
                "fixture-host-auth".into()
            )
        );
        let binding =
            WorkflowMainSessionBinding::find_by_session_id(&port.db.pool, request.session_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(binding.token_hash, Some(workflow_mcp::token_hash(&token)));
        assert_eq!(
            binding.actual_main_run_attempt_id,
            Some(attempt.run_attempt_id)
        );
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(port.query(request.agent_run_id).await.unwrap().state).unwrap()
        );
    }

    #[tokio::test]
    async fn unreachable_provider_pid_is_observed_not_killed() {
        let db = setup_runtime_db().await;
        let mut port = LocalAgentRunPort::new(db);
        let registry_dir = tempfile::TempDir::new().unwrap();
        port.process_registry =
            AgentProcessRegistry::new(registry_dir.path().join("registry.json"));
        let (request, attempt) = persisted_codex_run(&port.db).await;
        // The test process is deliberately unrelated to this run. Never kill it.
        AgentRunRecord::mark_process_started(
            &port.db.pool,
            attempt.run_attempt_id,
            std::process::id(),
            None,
            Some("unrelated-test-process"),
            Utc::now(),
        )
        .await
        .unwrap();
        assert!(
            !port
                .observe_attempt_processes_absent(attempt.run_attempt_id)
                .await
                .unwrap()
        );
        let error = port
            .reconcile_cancel_after_host_failure(
                &request,
                &attempt,
                "fixture host unavailable",
                None,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cannot be verified safely"));
        assert!(
            !port
                .reconcile_terminal_process(attempt.run_attempt_id)
                .await
                .unwrap()
        );
        let status: String = sqlx::query_scalar(
            "SELECT registry_status FROM agent_process_registry WHERE run_attempt_id = ?",
        )
        .bind(attempt.run_attempt_id)
        .fetch_one(&port.db.pool)
        .await
        .unwrap();
        assert_ne!(status, "exited");
    }

    #[test]
    fn accepted_cancellation_wins_over_late_provider_terminal_when_exit_is_proven() {
        assert_eq!(
            prefer_cancellation_terminal(AgentRunStatus::Succeeded, false),
            AgentRunStatus::Cancelled
        );
        assert_eq!(
            prefer_cancellation_terminal(AgentRunStatus::Failed, false),
            AgentRunStatus::Cancelled
        );
        assert_eq!(
            prefer_cancellation_terminal(AgentRunStatus::Crashed, true),
            AgentRunStatus::Cancelled
        );
        assert_eq!(
            prefer_cancellation_terminal(AgentRunStatus::Crashed, false),
            AgentRunStatus::Crashed
        );
        assert_eq!(
            prefer_cancellation_terminal(AgentRunStatus::AuditFailed, true),
            AgentRunStatus::AuditFailed
        );
    }

    #[test]
    fn lost_process_host_reconciliation_requires_repeated_failures_and_confirmed_exit() {
        assert!(!should_reconcile_lost_process_host(
            PROCESS_HOST_OBSERVER_FAILURE_THRESHOLD - 1,
            Some(RegisteredProcessPresence::Exited),
        ));
        assert!(!should_reconcile_lost_process_host(
            PROCESS_HOST_OBSERVER_FAILURE_THRESHOLD,
            Some(RegisteredProcessPresence::Alive),
        ));
        assert!(should_reconcile_lost_process_host(
            PROCESS_HOST_OBSERVER_FAILURE_THRESHOLD,
            Some(RegisteredProcessPresence::Exited),
        ));
        assert!(!should_reconcile_lost_process_host(
            PROCESS_HOST_OBSERVER_FAILURE_THRESHOLD,
            Some(RegisteredProcessPresence::Unreachable),
        ));
        assert!(!should_reconcile_lost_process_host(
            PROCESS_HOST_OBSERVER_FAILURE_THRESHOLD,
            None,
        ));
    }

    #[tokio::test]
    async fn cancellation_reconciliation_is_serialized_per_attempt() {
        let db = setup_runtime_db().await;
        let port = LocalAgentRunPort::new(db);
        let run_attempt_id = Uuid::new_v4();

        let first = port.cancellation_reconciliation_lock(run_attempt_id).await;
        let same_attempt = port.cancellation_reconciliation_lock(run_attempt_id).await;
        let other_attempt = port.cancellation_reconciliation_lock(Uuid::new_v4()).await;

        assert!(Arc::ptr_eq(&first, &same_attempt));
        let first_guard = first.lock().await;
        assert!(same_attempt.try_lock().is_err());
        assert!(other_attempt.try_lock().is_ok());
        drop(first_guard);

        let old_lock = Arc::downgrade(&first);
        drop(first);
        drop(same_attempt);
        assert!(old_lock.upgrade().is_none());
        assert!(
            port.cancellation_reconciliation_lock(run_attempt_id)
                .await
                .try_lock()
                .is_ok()
        );
    }

    #[tokio::test]
    async fn canonical_event_identity_is_idempotent_before_sequence_allocation() {
        let db = setup_runtime_db().await;
        let port = LocalAgentRunPort::new(db);
        let (request, attempt) = persisted_codex_run(&port.db).await;
        let event_id = Uuid::new_v4();
        let timestamp = Utc::now();
        let payload = AgentEventPayload::Message {
            message: CanonicalMessage {
                message_id: event_id,
                role: AgentRuntimeMessageRole::User,
                content: "answer the pending question".to_string(),
            },
            final_output: false,
        };

        port.append_event(
            &request,
            &attempt,
            payload.clone(),
            Vec::new(),
            timestamp,
            Some(event_id),
        )
        .await
        .expect("append canonical input message");
        port.append_event(
            &request,
            &attempt,
            payload,
            Vec::new(),
            timestamp,
            Some(event_id),
        )
        .await
        .expect("redelivered canonical input message is idempotent");

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_events WHERE agent_run_id = ? AND event_id = ?",
        )
        .bind(request.agent_run_id)
        .bind(event_id)
        .fetch_one(&port.db.pool)
        .await
        .expect("count canonical events");
        assert_eq!(count, 1);
        assert_eq!(
            port.query(request.agent_run_id)
                .await
                .expect("query AgentRun")
                .state
                .last_event_sequence,
            1
        );
    }
}
