//! Independent owner for one provider process and its Native Audit stream.
//!
//! The application may disconnect or restart without transferring child
//! handles. It reconnects through the authenticated loopback endpoint and
//! replays observations after its persisted host-event cursor.

use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::{DateTime, Utc};
use executors::{
    actions::SelectedSkill,
    approvals::NoopExecutorApprovalService,
    env::{ExecutionEnv, RepoContext},
    executors::{
        CancellationToken, ExecutorControl, ExecutorExitResult,
        provider_adapter::{
            DirectControl, DirectIntent, DirectProvider, DirectProviderLaunchRequest,
            StatefulProviderMapper, launch_direct_provider,
        },
    },
    profile::ExecutorConfig,
    runtime::{
        AgentEventEnvelope, AgentRunStatus, AgentRuntimeError, AgentRuntimeErrorKind,
        CanonicalMessage, NativeAuditChannel, NativeAuditDirection, NativeAuditFrame,
        NativeAuditIntegrityStatus, NativeAuditManifest, NativeAuditMetadata, NativeAuditReference,
        NativeAuditWriter, ProviderSessionReference,
    },
};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{Mutex, Notify, RwLock, mpsc, oneshot},
};
use uuid::Uuid;

use crate::transport::{TransportError, read_json_frame, write_json_frame};

mod journal;
use journal::HostJournal;
pub(crate) use journal::HostJournalReplay;

pub(crate) fn host_journal_path(attempt_id: Uuid, host_instance_id: Uuid) -> PathBuf {
    utils::assets::asset_dir()
        .join("runtime")
        .join("host-events")
        .join(format!("{attempt_id}-{host_instance_id}.jsonl"))
}

const HOST_STREAM_PAGE_SIZE: usize = 128;
const HOST_STREAM_HEARTBEAT: Duration = Duration::from_secs(10);

const HOST_ATTACH_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const HOST_CONTROL_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const HOST_CANCEL_COMMAND_TIMEOUT: Duration = Duration::from_secs(25);
const HOST_LAUNCH_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const HOST_CANCEL_RESULT_TIMEOUT: Duration = Duration::from_secs(20);
const PROCESS_OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(3);
const HOST_TERMINAL_ACK_GRACE_PERIOD: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HostBootstrap {
    pub protocol_version: u16,
    pub run_attempt_id: Uuid,
    pub host_instance_id: Uuid,
    pub auth_token: String,
    pub requested_endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HostReady {
    pub protocol_version: u16,
    pub endpoint: String,
    pub host_pid: u32,
    pub host_instance_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HostLaunchRequest {
    pub run_attempt_id: Uuid,
    pub provider: DirectProvider,
    pub executor_config: ExecutorConfig,
    pub intent: DirectIntent,
    pub prompt: String,
    pub provider_session: Option<ProviderSessionReference>,
    pub reset_to_message_id: Option<String>,
    pub selected_skills: Vec<SelectedSkill>,
    pub current_dir: PathBuf,
    pub env: HostExecutionEnv,
    pub audit_metadata: NativeAuditMetadata,
    pub canonical_input: CanonicalMessage,
    pub correlation_id: Uuid,
    pub audited_launch_payload: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct HostExecutionEnv {
    pub vars: HashMap<String, String>,
    pub workspace_root: PathBuf,
    pub repo_names: Vec<String>,
    pub commit_reminder: bool,
    pub commit_reminder_prompt: String,
}

impl std::fmt::Debug for HostExecutionEnv {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let vars: HashMap<_, _> = self
            .vars
            .iter()
            .map(|(key, value)| {
                (
                    key,
                    if executors::workflow_mcp::is_token_env_key(key) {
                        "<redacted>"
                    } else {
                        value.as_str()
                    },
                )
            })
            .collect();
        formatter
            .debug_struct("HostExecutionEnv")
            .field("vars", &vars)
            .field("workspace_root", &self.workspace_root)
            .field("repo_names", &self.repo_names)
            .field("commit_reminder", &self.commit_reminder)
            .field("commit_reminder_prompt", &self.commit_reminder_prompt)
            .finish()
    }
}

impl From<&ExecutionEnv> for HostExecutionEnv {
    fn from(env: &ExecutionEnv) -> Self {
        Self {
            vars: env.vars.clone(),
            workspace_root: env.repo_context.workspace_root.clone(),
            repo_names: env.repo_context.repo_names.clone(),
            commit_reminder: env.commit_reminder,
            commit_reminder_prompt: env.commit_reminder_prompt.clone(),
        }
    }
}

impl HostExecutionEnv {
    fn into_execution_env(self) -> ExecutionEnv {
        let mut env = ExecutionEnv::new(
            RepoContext::new(self.workspace_root, self.repo_names),
            self.commit_reminder,
            self.commit_reminder_prompt,
        );
        env.vars = self.vars;
        env
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub(crate) enum HostCommand {
    Launch(HostLaunchRequest),
    Attach {
        after_sequence: u64,
    },
    Subscribe {
        after_sequence: u64,
    },
    Control {
        bytes: Vec<u8>,
        control: Option<DirectControl>,
        cancel: bool,
        after_sequence: u64,
    },
    AckTerminal {
        through_sequence: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AuthenticatedHostCommand {
    pub auth_token: String,
    pub command: HostCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HostResponse {
    pub host_instance_id: Uuid,
    pub events: Vec<HostEvent>,
    pub terminal: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HostEvent {
    pub sequence: u64,
    pub event_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub payload: HostEventPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub(crate) enum HostEventPayload {
    Started {
        provider_pid: u32,
        process_group_id: Option<u32>,
        executable: String,
        canonical_input_ref: NativeAuditReference,
        audit_manifest: NativeAuditManifest,
    },
    Mapped {
        event: AgentEventEnvelope,
        native_ref: NativeAuditReference,
    },
    Terminal {
        status: AgentRunStatus,
        error: Option<AgentRuntimeError>,
        error_event_id: Option<Uuid>,
        exit_code: Option<i64>,
        audit_manifest: NativeAuditManifest,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessHostError {
    #[error("host protocol error: {0}")]
    Protocol(String),
    #[error("host transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("host I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("provider launch failed: {0}")]
    Launch(String),
    #[error("Native Audit failed: {0}")]
    Audit(String),
}

#[derive(Default)]
struct HostRuntimeState {
    launched: bool,
    process_commands: Option<mpsc::Sender<ProcessCommand>>,
}

struct SharedHost {
    run_attempt_id: Uuid,
    host_instance_id: Uuid,
    auth_token: String,
    runtime: RwLock<HostRuntimeState>,
    journal: Mutex<HostJournal>,
    changes: tokio::sync::watch::Sender<u64>,
    launch_lock: Mutex<()>,
    terminal: AtomicBool,
    shutdown: Notify,
}

enum ProcessCommand {
    Control {
        bytes: Vec<u8>,
        control: Option<DirectControl>,
        cancel: bool,
        result: oneshot::Sender<Result<(), String>>,
    },
}

#[derive(Debug)]
enum OutputNotice {
    StdoutClosed,
    StderrClosed,
    Mapped(AgentEventEnvelope, NativeAuditReference),
    ProviderTerminal(AgentRunStatus),
    AuditFailure(String),
    ProtocolFailure(String),
}

enum ExitCause {
    Process { success: bool, code: Option<i64> },
    Executor(ExecutorExitResult),
    ExecutorChannelClosed,
    ProviderTerminal(AgentRunStatus),
    Cancelled,
    AuditFailure(String),
    ProtocolFailure(String),
    WaitFailure(String),
}

impl SharedHost {
    async fn response_after(&self, after_sequence: u64) -> Result<HostResponse, ProcessHostError> {
        self.response_page(after_sequence, usize::MAX).await
    }

    async fn response_page(
        &self,
        after_sequence: u64,
        limit: usize,
    ) -> Result<HostResponse, ProcessHostError> {
        let mut journal = self.journal.lock().await;
        let events = journal.read_after(after_sequence, limit).await?;
        let through = events.last().map_or(after_sequence, |event| event.sequence);
        Ok(HostResponse {
            host_instance_id: self.host_instance_id,
            events,
            terminal: self.terminal.load(Ordering::Acquire) && through >= journal.last_sequence(),
            error: None,
        })
    }

    async fn append_event(&self, payload: HostEventPayload) -> Result<HostEvent, ProcessHostError> {
        let mut journal = self.journal.lock().await;
        let event = HostEvent {
            sequence: journal.last_sequence() + 1,
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            payload,
        };
        journal.append(&event).await?;
        self.changes.send_replace(event.sequence);
        Ok(event)
    }

    async fn launch(self: &Arc<Self>, launch: HostLaunchRequest) -> Result<(), ProcessHostError> {
        let _guard = self.launch_lock.lock().await;
        {
            let runtime = self.runtime.read().await;
            if runtime.launched {
                return Ok(());
            }
        }
        if launch.run_attempt_id != self.run_attempt_id {
            return Err(ProcessHostError::Protocol(
                "launch RunAttempt does not match host reservation".to_string(),
            ));
        }

        let mut writer = NativeAuditWriter::create(launch.audit_metadata.clone())
            .map_err(|error| ProcessHostError::Audit(error.to_string()))?;
        let canonical_input_ref = writer
            .append_canonical_input(&launch.canonical_input, launch.correlation_id)
            .map_err(|error| ProcessHostError::Audit(error.to_string()))?;
        writer
            .append_native_input(
                NativeAuditChannel::NativeInput,
                "application/vnd.vibe-kanban.direct-provider-launch+json",
                launch.correlation_id,
                &launch.audited_launch_payload,
            )
            .map_err(|error| ProcessHostError::Audit(error.to_string()))?;

        let env = launch.env.clone().into_execution_env();
        let mut spawned = match launch_direct_provider(DirectProviderLaunchRequest {
            provider: launch.provider,
            executor_config: &launch.executor_config,
            intent: launch.intent,
            prompt: &launch.prompt,
            provider_session: launch.provider_session.as_ref(),
            reset_to_message_id: launch.reset_to_message_id.as_deref(),
            selected_skills: &launch.selected_skills,
            approvals: Arc::new(NoopExecutorApprovalService),
            current_dir: &launch.current_dir,
            env: &env,
        })
        .await
        {
            Ok(spawned) => spawned,
            Err(error) => {
                let runtime_error =
                    AgentRuntimeError::from_executor_error(&error, Some(launch.provider.id()));
                let manifest = close_writer_failed(&mut writer)?;
                self.append_terminal(AgentRunStatus::Failed, Some(runtime_error), None, manifest)
                    .await;
                self.runtime.write().await.launched = true;
                return Ok(());
            }
        };

        let Some(provider_pid) = spawned.child.inner().id() else {
            let _ = utils::process::kill_process_group(&mut spawned.child).await;
            let manifest = close_writer_failed(&mut writer)?;
            self.append_terminal(
                AgentRunStatus::Crashed,
                Some(
                    AgentRuntimeError::new(
                        AgentRuntimeErrorKind::ProcessCrashed,
                        "spawned provider has no OS pid",
                    )
                    .with_provider(Some(launch.provider.id())),
                ),
                None,
                manifest,
            )
            .await;
            self.runtime.write().await.launched = true;
            return Ok(());
        };
        let stdout = match spawned.child.inner().stdout.take() {
            Some(stdout) => stdout,
            None => {
                let _ = utils::process::kill_process_group(&mut spawned.child).await;
                let manifest = close_writer_failed(&mut writer)?;
                self.append_terminal(
                    AgentRunStatus::Crashed,
                    Some(
                        AgentRuntimeError::new(
                            AgentRuntimeErrorKind::ProcessCrashed,
                            "provider process stdout is unavailable",
                        )
                        .with_provider(Some(launch.provider.id())),
                    ),
                    None,
                    manifest,
                )
                .await;
                self.runtime.write().await.launched = true;
                return Ok(());
            }
        };
        let stderr = match spawned.child.inner().stderr.take() {
            Some(stderr) => stderr,
            None => {
                let _ = utils::process::kill_process_group(&mut spawned.child).await;
                let manifest = close_writer_failed(&mut writer)?;
                self.append_terminal(
                    AgentRunStatus::Crashed,
                    Some(
                        AgentRuntimeError::new(
                            AgentRuntimeErrorKind::ProcessCrashed,
                            "provider process stderr is unavailable",
                        )
                        .with_provider(Some(launch.provider.id())),
                    ),
                    None,
                    manifest,
                )
                .await;
                self.runtime.write().await.launched = true;
                return Ok(());
            }
        };
        let control = spawned.control;
        let stdin = spawned.child.inner().stdin.take();
        let writer = Arc::new(Mutex::new(writer));

        if let Err(error) = self
            .append_event(HostEventPayload::Started {
                provider_pid,
                process_group_id: Some(provider_pid),
                executable: launch.provider.versions().executable.to_string(),
                canonical_input_ref,
                audit_manifest: writer.lock().await.manifest().clone(),
            })
            .await
        {
            let _ = utils::process::kill_process_group(&mut spawned.child).await;
            let _ = writer.lock().await.fail_closed();
            return Err(error);
        }

        let (notices_tx, notices_rx) = mpsc::unbounded_channel();
        spawn_stdout_reader(
            launch.provider,
            launch.correlation_id,
            stdout,
            writer.clone(),
            notices_tx.clone(),
        );
        spawn_stderr_reader(launch.correlation_id, stderr, writer.clone(), notices_tx);
        let (commands_tx, commands_rx) = mpsc::channel(16);
        {
            let mut runtime = self.runtime.write().await;
            runtime.launched = true;
            runtime.process_commands = Some(commands_tx);
        }
        let host = self.clone();
        tokio::spawn(async move {
            monitor_process(
                host,
                launch.provider,
                launch.correlation_id,
                spawned.child,
                stdin,
                spawned.cancel,
                spawned.exit_signal,
                control,
                writer,
                notices_rx,
                commands_rx,
            )
            .await;
        });
        Ok(())
    }

    async fn append_terminal(
        self: &Arc<Self>,
        status: AgentRunStatus,
        error: Option<AgentRuntimeError>,
        exit_code: Option<i64>,
        audit_manifest: NativeAuditManifest,
    ) {
        let error_event_id = error.as_ref().map(|_| Uuid::new_v4());
        if let Err(error) = self
            .append_event(HostEventPayload::Terminal {
                status,
                error,
                error_event_id,
                exit_code,
                audit_manifest,
            })
            .await
        {
            tracing::error!(%error, "failed to persist process-host terminal observation");
            self.shutdown.notify_one();
            return;
        }
        self.terminal.store(true, Ordering::Release);
        self.changes.send_modify(|_| {});
        self.runtime.write().await.process_commands = None;
        let host = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(HOST_TERMINAL_ACK_GRACE_PERIOD).await;
            host.shutdown.notify_one();
        });
    }
}

pub async fn run_from_stdin() -> Result<(), ProcessHostError> {
    let mut stdin = tokio::io::stdin();
    let bootstrap: HostBootstrap = read_json_frame(&mut stdin).await?;
    if bootstrap.protocol_version != 1 || bootstrap.auth_token.len() < 32 {
        return Err(ProcessHostError::Protocol(
            "invalid process-host bootstrap".to_string(),
        ));
    }
    let listener = TcpListener::bind(&bootstrap.requested_endpoint).await?;
    let ready = HostReady {
        protocol_version: 1,
        endpoint: listener.local_addr()?.to_string(),
        host_pid: std::process::id(),
        host_instance_id: bootstrap.host_instance_id,
    };
    let mut stdout = tokio::io::stdout();
    write_json_frame(&mut stdout, &ready).await?;
    drop(stdout);

    let journal_directory = utils::assets::asset_dir()
        .join("runtime")
        .join("host-events");
    tokio::fs::create_dir_all(&journal_directory).await?;
    let journal = HostJournal::create(&host_journal_path(
        bootstrap.run_attempt_id,
        bootstrap.host_instance_id,
    ))
    .await?;
    let host = Arc::new(SharedHost {
        run_attempt_id: bootstrap.run_attempt_id,
        host_instance_id: bootstrap.host_instance_id,
        auth_token: bootstrap.auth_token,
        runtime: RwLock::new(HostRuntimeState::default()),
        journal: Mutex::new(journal),
        changes: tokio::sync::watch::channel(0).0,
        launch_lock: Mutex::new(()),
        terminal: AtomicBool::new(false),
        shutdown: Notify::new(),
    });
    serve(listener, host).await
}

async fn serve(listener: TcpListener, host: Arc<SharedHost>) -> Result<(), ProcessHostError> {
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let host = host.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_connection(stream, host).await {
                        tracing::warn!(%error, "process-host client connection failed");
                    }
                });
            }
            () = host.shutdown.notified() => return Ok(()),
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    host: Arc<SharedHost>,
) -> Result<(), ProcessHostError> {
    let request: AuthenticatedHostCommand = read_json_frame(&mut stream).await?;
    if request.auth_token != host.auth_token {
        return Err(ProcessHostError::Protocol(
            "process-host authentication failed".to_string(),
        ));
    }
    let mut shutdown = false;
    let response = match request.command {
        HostCommand::Launch(launch) => match host.launch(launch).await {
            Ok(()) => host.response_after(0).await?,
            Err(error) => HostResponse {
                host_instance_id: host.host_instance_id,
                events: Vec::new(),
                terminal: host.terminal.load(Ordering::Acquire),
                error: Some(error.to_string()),
            },
        },
        HostCommand::Attach { after_sequence } => host.response_after(after_sequence).await?,
        HostCommand::Subscribe { after_sequence } => {
            return stream_observations(&mut stream, &host, after_sequence).await;
        }
        HostCommand::Control {
            bytes,
            control,
            cancel,
            after_sequence,
        } => {
            let sender = host.runtime.read().await.process_commands.clone();
            let error = match sender {
                Some(sender) => {
                    let (result_tx, result_rx) = oneshot::channel();
                    if sender
                        .send(ProcessCommand::Control {
                            bytes,
                            control,
                            cancel,
                            result: result_tx,
                        })
                        .await
                        .is_err()
                    {
                        Some("provider process command channel is closed".to_string())
                    } else {
                        let result_timeout = if cancel {
                            HOST_CANCEL_RESULT_TIMEOUT
                        } else {
                            HOST_CONTROL_COMMAND_TIMEOUT
                        };
                        match tokio::time::timeout(result_timeout, result_rx).await {
                            Ok(Ok(Ok(()))) => None,
                            Ok(Ok(Err(error))) => Some(error),
                            Ok(Err(_)) => {
                                Some("provider process command result was lost".to_string())
                            }
                            Err(_) => Some(
                                "provider process command did not converge before its deadline"
                                    .to_string(),
                            ),
                        }
                    }
                }
                None => Some("provider process is not running".to_string()),
            };
            let mut response = host.response_after(after_sequence).await?;
            response.error = error;
            response
        }
        HostCommand::AckTerminal { through_sequence } => {
            let response = host.response_after(through_sequence).await?;
            let terminal_sequence = if host.terminal.load(Ordering::Acquire) {
                Some(host.journal.lock().await.last_sequence())
            } else {
                None
            };
            shutdown = terminal_sequence.is_some_and(|sequence| through_sequence >= sequence);
            response
        }
    };
    write_json_frame(&mut stream, &response).await?;
    if shutdown {
        host.shutdown.notify_one();
    }
    Ok(())
}

async fn stream_observations(
    stream: &mut TcpStream,
    host: &SharedHost,
    mut cursor: u64,
) -> Result<(), ProcessHostError> {
    // Register before reading: an append during replay leaves a pending wakeup.
    let mut changes = host.changes.subscribe();
    loop {
        let response = host.response_page(cursor, HOST_STREAM_PAGE_SIZE).await?;
        let had_events = !response.events.is_empty();
        cursor = response
            .events
            .last()
            .map_or(cursor, |event| event.sequence);
        let terminal = response.terminal;
        tokio::time::timeout(HOST_STREAM_HEARTBEAT, write_json_frame(stream, &response))
            .await
            .map_err(|_| ProcessHostError::Protocol("slow process-host subscriber".into()))??;
        if terminal {
            return Ok(());
        }
        if had_events {
            continue;
        }
        // Heartbeats detect dead peers without placing a deadline on execution.
        let _ = tokio::time::timeout(HOST_STREAM_HEARTBEAT, changes.changed()).await;
    }
}

fn spawn_stdout_reader(
    provider: DirectProvider,
    correlation_id: Uuid,
    stdout: tokio::process::ChildStdout,
    writer: Arc<Mutex<NativeAuditWriter>>,
    notices: mpsc::UnboundedSender<OutputNotice>,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut mapper = StatefulProviderMapper::new(provider);
        loop {
            let mut bytes = Vec::new();
            match reader.read_until(b'\n', &mut bytes).await {
                Ok(0) => break,
                Ok(_) => {
                    let (frame, native_ref, manifest) = {
                        let mut writer = writer.lock().await;
                        let frame = NativeAuditFrame::from_bytes(
                            writer.manifest().last_sequence.unwrap_or(0) + 1,
                            Utc::now(),
                            NativeAuditDirection::Output,
                            NativeAuditChannel::Stdout,
                            "application/json",
                            correlation_id,
                            &bytes,
                            None,
                        );
                        match writer.append(frame.clone()) {
                            Ok(reference) => (frame, reference, writer.manifest().clone()),
                            Err(error) => {
                                let _ = notices.send(OutputNotice::AuditFailure(error.to_string()));
                                break;
                            }
                        }
                    };
                    let mapped = match mapper.map_frame(&frame, &manifest) {
                        Ok(mapped) => mapped,
                        Err(error) => {
                            let _ = notices.send(OutputNotice::ProtocolFailure(error.to_string()));
                            break;
                        }
                    };
                    let mut frame_terminal = None;
                    for event in mapped {
                        let terminal =
                            match &event.payload {
                                executors::runtime::AgentEventPayload::LifecycleChanged {
                                    status,
                                } if status.is_terminal() => Some(*status),
                                // Provider-native API failures are mapped to
                                // an Error event before the child necessarily
                                // exits (Claude, for example, can report a
                                // 503 and then exit successfully). Treat the
                                // error as terminal so the run cannot be
                                // projected as success or remain running.
                                executors::runtime::AgentEventPayload::Error { .. } => {
                                    Some(AgentRunStatus::Failed)
                                }
                                _ => None,
                            };
                        let _ = notices.send(OutputNotice::Mapped(event, native_ref.clone()));
                        if let Some(status) = terminal {
                            frame_terminal = Some(status);
                        }
                    }
                    // A native frame may contain final content and several
                    // finishes alongside its terminal fact. Deliver all of
                    // them before asking the monitor to complete the attempt.
                    if let Some(status) = frame_terminal {
                        let _ = notices.send(OutputNotice::ProviderTerminal(status));
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
    correlation_id: Uuid,
    stderr: tokio::process::ChildStderr,
    writer: Arc<Mutex<NativeAuditWriter>>,
    notices: mpsc::UnboundedSender<OutputNotice>,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        loop {
            let mut bytes = Vec::new();
            match reader.read_until(b'\n', &mut bytes).await {
                Ok(0) => break,
                Ok(_) => {
                    let mut writer = writer.lock().await;
                    if let Err(error) = writer.append_native_output(
                        NativeAuditChannel::Stderr,
                        "text/plain",
                        correlation_id,
                        &bytes,
                    ) {
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

#[allow(clippy::too_many_arguments)]
async fn monitor_process(
    host: Arc<SharedHost>,
    provider: DirectProvider,
    correlation_id: Uuid,
    mut child: command_group::AsyncGroupChild,
    mut stdin: Option<tokio::process::ChildStdin>,
    cancellation: Option<CancellationToken>,
    exit_signal: Option<executors::executors::ExecutorExitSignal>,
    control: Option<Arc<dyn ExecutorControl>>,
    writer: Arc<Mutex<NativeAuditWriter>>,
    mut notices: mpsc::UnboundedReceiver<OutputNotice>,
    mut commands: mpsc::Receiver<ProcessCommand>,
) {
    let mut stdout_closed = false;
    let mut stderr_closed = false;
    let mut cancel_result = None;
    let mut cancellation_requested = false;
    let mut exit_signal: Pin<
        Box<
            dyn Future<Output = Result<ExecutorExitResult, tokio::sync::oneshot::error::RecvError>>
                + Send,
        >,
    > = match exit_signal {
        Some(signal) => Box::pin(signal),
        None => Box::pin(std::future::pending::<
            Result<ExecutorExitResult, tokio::sync::oneshot::error::RecvError>,
        >()),
    };
    let mut cause = loop {
        tokio::select! {
            status = child.wait() => {
                break match status {
                    Ok(status) => ExitCause::Process {
                        success: status.success(),
                        code: status.code().map(i64::from),
                    },
                    Err(error) => ExitCause::WaitFailure(error.to_string()),
                };
            }
            result = &mut exit_signal => {
                break match result {
                    Ok(result) => ExitCause::Executor(result),
                    Err(_) => ExitCause::ExecutorChannelClosed,
                };
            }
            notice = notices.recv(), if !(stdout_closed && stderr_closed) => {
                match notice {
                    Some(OutputNotice::StdoutClosed) => stdout_closed = true,
                    Some(OutputNotice::StderrClosed) => stderr_closed = true,
                    Some(OutputNotice::Mapped(event, native_ref)) => {
                        if let Err(error) = host.append_event(HostEventPayload::Mapped { event, native_ref }).await {
                            break ExitCause::AuditFailure(error.to_string());
                        }
                    }
                    Some(OutputNotice::ProviderTerminal(status)) => break ExitCause::ProviderTerminal(status),
                    Some(OutputNotice::AuditFailure(error)) => break ExitCause::AuditFailure(error),
                    Some(OutputNotice::ProtocolFailure(error)) => break ExitCause::ProtocolFailure(error),
                    None => {
                        stdout_closed = true;
                        stderr_closed = true;
                    }
                }
            }
            command = commands.recv() => {
                let Some(ProcessCommand::Control { bytes, control: direct_control, cancel, result }) = command else {
                    continue;
                };
                let interaction_reply = matches!(&direct_control, Some(DirectControl::ReplyInteraction { .. }));
                let sent_bytes = if let Some(control_peer) = control.as_ref() {
                    match direct_control {
                        Some(direct_control) => control_peer
                            .send(direct_control)
                            .await
                            .map_err(|error| error.to_string()),
                        None if !cancel => {
                            Err("provider control payload is missing".to_string())
                        }
                        None => Ok(Vec::new()),
                    }
                } else if !bytes.is_empty() {
                    if let Some(stdin) = stdin.as_mut() {
                        let write_result = stdin.write_all(&bytes).await;
                        let write_result = match write_result {
                            Ok(()) => stdin.flush().await,
                            Err(error) => Err(error),
                        };
                        write_result
                            .map(|_| bytes)
                            .map_err(|error| error.to_string())
                    } else if !cancel {
                        Err("provider stdin is owned by its protocol adapter".to_string())
                    } else {
                        Ok(Vec::new())
                    }
                } else if !cancel {
                    Err("provider has no native control peer or input frame".to_string())
                } else {
                    Ok(Vec::new())
                };
                let sent_bytes = match sent_bytes {
                    Ok(sent_bytes) => sent_bytes,
                    Err(message) => {
                        if !cancel {
                            let _ = result.send(Err(message.clone()));
                            // Stale or unsupported native decisions are command
                            // errors. They must not kill an otherwise live run.
                            if interaction_reply {
                                continue;
                            }
                            break ExitCause::ProtocolFailure(message);
                        }
                        // Cancellation is still authoritative when the
                        // provider-native interrupt is unavailable (for
                        // example before Codex has established its thread).
                        // Continue to token/process cleanup instead of
                        // turning a recoverable control race into a protocol
                        // failure.
                        tracing::warn!(%message, "provider-native cancellation was unavailable; using process cleanup");
                        Vec::new()
                    }
                };
                if !sent_bytes.is_empty() {
                    let audit_result = {
                        let mut writer = writer.lock().await;
                        writer.append_native_input(
                            NativeAuditChannel::Stdin,
                            "application/x-ndjson",
                            correlation_id,
                            &sent_bytes,
                        )
                    };
                    if let Err(error) = audit_result {
                        let message = error.to_string();
                        let _ = result.send(Err(message.clone()));
                        break ExitCause::AuditFailure(message);
                    }
                }
                if cancel {
                    cancellation_requested = true;
                    if let Some(cancellation) = cancellation.as_ref() {
                        cancellation.cancel();
                    }
                    cancel_result = Some(result);
                    let kill_result = utils::process::kill_process_group(&mut child).await;
                    match kill_result {
                        Ok(()) => break ExitCause::Cancelled,
                        Err(error) => break ExitCause::WaitFailure(error.to_string()),
                    }
                }
                let _ = result.send(Ok(()));
            }
        }
    };

    if !cancellation_requested
        && !matches!(cause, ExitCause::Process { .. } | ExitCause::Cancelled)
        && let Err(error) = utils::process::kill_process_group(&mut child).await
    {
        cause = ExitCause::WaitFailure(error.to_string());
    }

    // Finished adapters can retain a replacement stdout writer through their
    // control peer (Codex's AppServerClient, for example). Stop their background
    // tasks and release the peer before awaiting EOF. This is resource cleanup,
    // not a new user cancellation or a replacement for the observed outcome.
    if !matches!(cause, ExitCause::Process { .. })
        && let Some(cancellation) = cancellation.as_ref()
    {
        cancellation.cancel();
    }
    drop(control);

    let mut drained_provider_terminal = None;
    let drain_result = tokio::time::timeout(PROCESS_OUTPUT_DRAIN_TIMEOUT, async {
        while !(stdout_closed && stderr_closed) {
            match notices.recv().await {
                Some(OutputNotice::StdoutClosed) => stdout_closed = true,
                Some(OutputNotice::StderrClosed) => stderr_closed = true,
                Some(OutputNotice::Mapped(event, native_ref)) => {
                    if let Err(error) = host
                        .append_event(HostEventPayload::Mapped { event, native_ref })
                        .await
                    {
                        return Some(ExitCause::AuditFailure(error.to_string()));
                    }
                }
                Some(OutputNotice::AuditFailure(error)) => {
                    return Some(ExitCause::AuditFailure(error));
                }
                Some(OutputNotice::ProtocolFailure(error)) => {
                    return Some(ExitCause::ProtocolFailure(error));
                }
                Some(OutputNotice::ProviderTerminal(status)) => {
                    drained_provider_terminal = Some(status);
                }
                None => break,
            }
        }
        None
    })
    .await;
    match drain_result {
        Ok(Some(post_exit_cause)) => cause = post_exit_cause,
        Ok(None) => {
            if let Some(status) = drained_provider_terminal {
                cause = prefer_drained_provider_terminal(cause, status, cancellation_requested);
            }
        }
        Err(_) => {
            cause = ExitCause::AuditFailure(
                "provider output streams did not close before the audit drain deadline".to_string(),
            );
        }
    }

    let (status, error, exit_code) = terminal_from_exit(cause, provider);
    let cancel_error = error.as_ref().map(|error| error.message.clone());
    let manifest = {
        let mut writer = writer.lock().await;
        let close_result = if status == AgentRunStatus::AuditFailed {
            writer.fail_closed()
        } else {
            writer.close_with_status(NativeAuditIntegrityStatus::Complete)
        };
        match close_result {
            Ok(manifest) => manifest,
            Err(error) => match writer.fail_closed() {
                Ok(manifest) => {
                    host.append_terminal(
                        AgentRunStatus::AuditFailed,
                        Some(
                            AgentRuntimeError::new(
                                AgentRuntimeErrorKind::Unknown,
                                error.to_string(),
                            )
                            .with_provider(Some(provider.id())),
                        ),
                        exit_code,
                        manifest,
                    )
                    .await;
                    if let Some(result) = cancel_result {
                        let _ = result.send(Err(error.to_string()));
                    }
                    return;
                }
                Err(final_error) => {
                    tracing::error!(%final_error, "process host could not finalize Native Audit");
                    if let Some(result) = cancel_result {
                        let _ = result.send(Err(final_error.to_string()));
                    }
                    return;
                }
            },
        }
    };
    host.append_terminal(status, error, exit_code, manifest)
        .await;
    if let Some(result) = cancel_result {
        let acknowledgement = if status == AgentRunStatus::Cancelled {
            Ok(())
        } else {
            Err(cancel_error
                .unwrap_or_else(|| format!("cancellation converged to terminal status {status:?}")))
        };
        let _ = result.send(acknowledgement);
    }
}

fn prefer_drained_provider_terminal(
    cause: ExitCause,
    provider_status: AgentRunStatus,
    cancellation_requested: bool,
) -> ExitCause {
    if cancellation_requested
        || matches!(
            cause,
            ExitCause::Cancelled
                | ExitCause::AuditFailure(_)
                | ExitCause::ProtocolFailure(_)
                | ExitCause::WaitFailure(_)
        )
        || (provider_status == AgentRunStatus::Succeeded
            && matches!(
                cause,
                ExitCause::Executor(ExecutorExitResult::Failure)
                    | ExitCause::ExecutorChannelClosed
                    | ExitCause::Process { success: false, .. }
            ))
    {
        cause
    } else {
        ExitCause::ProviderTerminal(provider_status)
    }
}

fn terminal_from_exit(
    cause: ExitCause,
    provider: DirectProvider,
) -> (AgentRunStatus, Option<AgentRuntimeError>, Option<i64>) {
    let runtime_error = |kind, message: String| {
        Some(AgentRuntimeError::new(kind, message).with_provider(Some(provider.id())))
    };
    match cause {
        ExitCause::Executor(ExecutorExitResult::Success) => {
            (AgentRunStatus::Succeeded, None, Some(0))
        }
        ExitCause::Executor(ExecutorExitResult::Failure) => (
            AgentRunStatus::Failed,
            runtime_error(
                AgentRuntimeErrorKind::Unknown,
                "provider executor reported failure".to_string(),
            ),
            Some(1),
        ),
        ExitCause::ExecutorChannelClosed => (
            AgentRunStatus::Crashed,
            runtime_error(
                AgentRuntimeErrorKind::ProcessCrashed,
                "provider exit channel closed".to_string(),
            ),
            None,
        ),
        ExitCause::ProviderTerminal(status) => (
            status,
            None,
            (status == AgentRunStatus::Succeeded).then_some(0),
        ),
        ExitCause::Cancelled => (AgentRunStatus::Cancelled, None, None),
        ExitCause::AuditFailure(message) => (
            AgentRunStatus::AuditFailed,
            runtime_error(AgentRuntimeErrorKind::Unknown, message),
            None,
        ),
        ExitCause::ProtocolFailure(message) => (
            AgentRunStatus::Failed,
            runtime_error(AgentRuntimeErrorKind::OutputParseFailed, message),
            None,
        ),
        ExitCause::WaitFailure(message) => (
            AgentRunStatus::Crashed,
            runtime_error(AgentRuntimeErrorKind::ProcessCrashed, message),
            None,
        ),
        ExitCause::Process {
            success: false,
            code,
        } => (
            AgentRunStatus::Failed,
            runtime_error(
                AgentRuntimeErrorKind::ProcessCrashed,
                "provider process exited unsuccessfully".to_string(),
            ),
            code,
        ),
        ExitCause::Process {
            success: true,
            code,
        } => (
            AgentRunStatus::Crashed,
            runtime_error(
                AgentRuntimeErrorKind::ProcessCrashed,
                "provider process exited without provider success evidence".to_string(),
            ),
            code,
        ),
    }
}

fn close_writer_failed(
    writer: &mut NativeAuditWriter,
) -> Result<NativeAuditManifest, ProcessHostError> {
    writer
        .fail_closed()
        .map_err(|error| ProcessHostError::Audit(error.to_string()))
}

pub(crate) async fn send_host_command(
    endpoint: &str,
    auth_token: &str,
    command: HostCommand,
) -> Result<HostResponse, TransportError> {
    let timeout = match &command {
        HostCommand::Launch(_) => HOST_LAUNCH_COMMAND_TIMEOUT,
        HostCommand::Control { cancel: true, .. } => HOST_CANCEL_COMMAND_TIMEOUT,
        HostCommand::Control { cancel: false, .. } => HOST_CONTROL_COMMAND_TIMEOUT,
        HostCommand::Attach { .. }
        | HostCommand::Subscribe { .. }
        | HostCommand::AckTerminal { .. } => HOST_ATTACH_COMMAND_TIMEOUT,
    };
    send_host_command_with_timeout(endpoint, auth_token, command, timeout).await
}

pub(crate) async fn subscribe_host_events(
    endpoint: &str,
    auth_token: &str,
    after_sequence: u64,
) -> Result<TcpStream, TransportError> {
    tokio::time::timeout(HOST_ATTACH_COMMAND_TIMEOUT, async {
        let mut stream = TcpStream::connect(endpoint)
            .await
            .map_err(|error| TransportError::TemporarilyUnavailable(error.to_string()))?;
        write_json_frame(
            &mut stream,
            &AuthenticatedHostCommand {
                auth_token: auth_token.to_string(),
                command: HostCommand::Subscribe { after_sequence },
            },
        )
        .await?;
        Ok(stream)
    })
    .await
    .map_err(|_| TransportError::TemporarilyUnavailable("host subscription timed out".into()))?
}

pub(crate) async fn read_host_subscription(
    stream: &mut TcpStream,
) -> Result<HostResponse, TransportError> {
    tokio::time::timeout(HOST_STREAM_HEARTBEAT * 3, read_json_frame(stream))
        .await
        .map_err(|_| {
            TransportError::TemporarilyUnavailable("host subscription heartbeat timed out".into())
        })?
}

async fn send_host_command_with_timeout(
    endpoint: &str,
    auth_token: &str,
    command: HostCommand,
    timeout: Duration,
) -> Result<HostResponse, TransportError> {
    tokio::time::timeout(timeout, async {
        let mut stream = TcpStream::connect(endpoint)
            .await
            .map_err(|error| TransportError::TemporarilyUnavailable(error.to_string()))?;
        write_json_frame(
            &mut stream,
            &AuthenticatedHostCommand {
                auth_token: auth_token.to_string(),
                command,
            },
        )
        .await?;
        read_json_frame(&mut stream).await
    })
    .await
    .map_err(|_| {
        TransportError::TemporarilyUnavailable(format!(
            "process-host command timed out after {} ms",
            timeout.as_millis()
        ))
    })?
}

#[cfg(test)]
mod tests {
    use utils::command_ext::GroupSpawnNoWindowExt;

    use super::*;

    struct RetainedOutputControl {
        _writer: Arc<Mutex<Box<dyn tokio::io::AsyncWrite + Send + Unpin>>>,
    }

    impl std::fmt::Debug for RetainedOutputControl {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("RetainedOutputControl")
                .finish_non_exhaustive()
        }
    }

    #[async_trait::async_trait]
    impl ExecutorControl for RetainedOutputControl {
        async fn send(
            &self,
            _control: DirectControl,
        ) -> Result<Vec<u8>, executors::executors::ExecutorError> {
            Ok(Vec::new())
        }
    }

    fn test_payload() -> HostEventPayload {
        HostEventPayload::Terminal {
            status: AgentRunStatus::Succeeded,
            error: None,
            error_event_id: None,
            exit_code: Some(0),
            audit_manifest: NativeAuditManifest {
                audit_schema_version: 1,
                session_id: Uuid::nil(),
                agent_run_id: Uuid::nil(),
                turn_id: Uuid::nil(),
                run_attempt_id: Uuid::nil(),
                run_attempt_number: 1,
                provider_id: "fixture".into(),
                runtime_profile_id: "fixture".into(),
                workspace_path: String::new(),
                runtime_version: None,
                protocol_version: None,
                adapter_version: "1".into(),
                mapper_version: "1".into(),
                frame_count: 0,
                first_sequence: None,
                last_sequence: None,
                final_checksum: None,
                integrity_status: NativeAuditIntegrityStatus::Complete,
                created_at: Utc::now(),
                closed_at: Some(Utc::now()),
                manifest_relative_path: "fixture/manifest.json".into(),
                frames_relative_path: "fixture/frames.jsonl".into(),
                raw_content_trusted: true,
            },
        }
    }

    async fn test_host(path: &std::path::Path) -> Arc<SharedHost> {
        Arc::new(SharedHost {
            run_attempt_id: Uuid::new_v4(),
            host_instance_id: Uuid::new_v4(),
            auth_token: "fixture-token".into(),
            runtime: RwLock::new(HostRuntimeState::default()),
            journal: Mutex::new(HostJournal::create(path).await.unwrap()),
            changes: tokio::sync::watch::channel(0).0,
            launch_lock: Mutex::new(()),
            terminal: AtomicBool::new(false),
            shutdown: Notify::new(),
        })
    }

    fn test_audit_writer(
        path: &std::path::Path,
        host: &SharedHost,
    ) -> Arc<Mutex<NativeAuditWriter>> {
        Arc::new(Mutex::new(
            NativeAuditWriter::create_in(
                path,
                NativeAuditMetadata {
                    session_id: Uuid::new_v4(),
                    agent_run_id: Uuid::new_v4(),
                    turn_id: Uuid::new_v4(),
                    run_attempt_id: host.run_attempt_id,
                    run_attempt_number: 1,
                    provider_id: "codex".into(),
                    runtime_profile_id: "fixture".into(),
                    workspace_path: path.to_string_lossy().into_owned(),
                    runtime_version: None,
                    protocol_version: None,
                    adapter_version: "1".into(),
                    mapper_version: "1".into(),
                    created_at: Utc::now(),
                },
            )
            .unwrap(),
        ))
    }

    #[tokio::test]
    async fn missing_interaction_peer_rejects_without_auditing_or_terminating_the_run() {
        let directory = tempfile::tempdir().unwrap();
        let host = test_host(&directory.path().join("events.jsonl")).await;
        let (mut spawned, output) = executors::stdout_dup::spawn_local_output_process().unwrap();
        let writer = test_audit_writer(directory.path(), &host);
        let correlation_id = Uuid::new_v4();
        let (notices_tx, notices_rx) = mpsc::unbounded_channel();
        spawn_stdout_reader(
            DirectProvider::Codex,
            correlation_id,
            spawned.child.inner().stdout.take().unwrap(),
            writer.clone(),
            notices_tx.clone(),
        );
        spawn_stderr_reader(
            correlation_id,
            spawned.child.inner().stderr.take().unwrap(),
            writer.clone(),
            notices_tx,
        );
        let (commands_tx, commands_rx) = mpsc::channel(1);
        let monitor = tokio::spawn(monitor_process(
            host.clone(),
            DirectProvider::Codex,
            correlation_id,
            spawned.child,
            None,
            None,
            None,
            None,
            writer.clone(),
            notices_rx,
            commands_rx,
        ));
        let (result, response) = oneshot::channel();
        commands_tx
            .send(ProcessCommand::Control {
                bytes: Vec::new(),
                control: Some(DirectControl::ReplyInteraction {
                    reply: executors::runtime::ConversationInteractionReply {
                        request_id: "native-request".into(),
                        option_id: Some("accept".into()),
                        answers: Vec::new(),
                        text: None,
                    },
                }),
                cancel: false,
                result,
            })
            .await
            .unwrap();
        let error = tokio::time::timeout(Duration::from_secs(3), response)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.contains("no native control peer"), "{error}");
        assert!(!host.response_after(0).await.unwrap().terminal);
        assert_eq!(writer.lock().await.manifest().frame_count, 0);
        drop(output);
        let (result, response) = oneshot::channel();
        commands_tx
            .send(ProcessCommand::Control {
                bytes: Vec::new(),
                control: None,
                cancel: true,
                result,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), monitor)
            .await
            .unwrap()
            .unwrap();
        response.await.unwrap().unwrap();
        assert_eq!(writer.lock().await.manifest().frame_count, 0);
        assert!(
            host.response_after(0)
                .await
                .unwrap()
                .events
                .iter()
                .any(|event| {
                    matches!(
                        &event.payload,
                        HostEventPayload::Terminal {
                            status: AgentRunStatus::Cancelled,
                            ..
                        }
                    )
                })
        );
    }

    #[tokio::test]
    async fn output_drain_releases_control_and_stops_adapter_tasks() {
        for cancelled in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let host = test_host(&directory.path().join("events.jsonl")).await;
            let (mut spawned, output) =
                executors::stdout_dup::spawn_local_output_process().unwrap();
            let writer = test_audit_writer(directory.path(), &host);
            let cancellation = CancellationToken::new();
            let output: Arc<Mutex<Box<dyn tokio::io::AsyncWrite + Send + Unpin>>> =
                Arc::new(Mutex::new(Box::new(output)));
            let control = Arc::new(RetainedOutputControl {
                _writer: output.clone(),
            });
            let adapter_cancellation = cancellation.clone();
            let adapter = tokio::spawn(async move {
                adapter_cancellation.cancelled().await;
                drop(output);
            });
            let (notices_tx, notices_rx) = mpsc::unbounded_channel();
            let correlation_id = Uuid::new_v4();
            spawn_stdout_reader(
                DirectProvider::Codex,
                correlation_id,
                spawned.child.inner().stdout.take().unwrap(),
                writer.clone(),
                notices_tx.clone(),
            );
            spawn_stderr_reader(
                correlation_id,
                spawned.child.inner().stderr.take().unwrap(),
                writer.clone(),
                notices_tx,
            );
            let (commands_tx, commands_rx) = mpsc::channel(1);
            let (exit_tx, exit_rx) = oneshot::channel();
            let cancel_reply = if cancelled {
                let (result, reply) = oneshot::channel();
                commands_tx
                    .send(ProcessCommand::Control {
                        bytes: Vec::new(),
                        control: None,
                        cancel: true,
                        result,
                    })
                    .await
                    .unwrap();
                Some(reply)
            } else {
                exit_tx.send(ExecutorExitResult::Success).unwrap();
                None
            };
            tokio::time::timeout(
                Duration::from_secs(10),
                monitor_process(
                    host.clone(),
                    DirectProvider::Codex,
                    correlation_id,
                    spawned.child,
                    None,
                    Some(cancellation.clone()),
                    if cancelled { None } else { Some(exit_rx) },
                    Some(control),
                    writer,
                    notices_rx,
                    commands_rx,
                ),
            )
            .await
            .unwrap();
            assert!(cancellation.is_cancelled());
            tokio::time::timeout(Duration::from_secs(1), adapter)
                .await
                .unwrap()
                .unwrap();
            if let Some(reply) = cancel_reply {
                reply.await.unwrap().unwrap();
            }
            let observations = host.response_after(0).await.unwrap();
            assert!(observations.terminal);
            let expected_status = if cancelled {
                AgentRunStatus::Cancelled
            } else {
                AgentRunStatus::Succeeded
            };
            assert!(observations.events.iter().any(|event| matches!(
                &event.payload,
                HostEventPayload::Terminal { status, error: None, audit_manifest, .. }
                    if *status == expected_status
                        && audit_manifest.integrity_status == NativeAuditIntegrityStatus::Complete
            )));
        }
    }

    #[tokio::test]
    async fn natural_process_exit_drains_adapter_tail_without_cancelling_it() {
        let directory = tempfile::tempdir().unwrap();
        let host = test_host(&directory.path().join("events.jsonl")).await;
        #[cfg(windows)]
        let mut command = {
            let mut command = tokio::process::Command::new("cmd.exe");
            command.args(["/d", "/c", "exit 0"]);
            command
        };
        #[cfg(unix)]
        let mut command = {
            let mut command = tokio::process::Command::new("/bin/sh");
            command.args(["-c", "exit 0"]);
            command
        };
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.group_spawn_no_window().unwrap();
        let output = executors::stdout_dup::create_stdout_pipe_writer(&mut child).unwrap();
        // Make the OS exit observable before the adapter flushes its last frame.
        assert!(child.wait().await.unwrap().success());
        let writer = test_audit_writer(directory.path(), &host);
        let frames_path = writer.lock().await.paths().1.to_path_buf();
        let cancellation = CancellationToken::new();
        let output: Arc<Mutex<Box<dyn tokio::io::AsyncWrite + Send + Unpin>>> =
            Arc::new(Mutex::new(Box::new(output)));
        let control = Arc::new(RetainedOutputControl {
            _writer: output.clone(),
        });
        let tail = b"{\"type\":\"result\",\"text\":\"tail after OS exit\"}\n";
        let adapter_cancellation = cancellation.clone();
        let adapter = tokio::spawn(async move {
            tokio::select! {
                _ = adapter_cancellation.cancelled() => return false,
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
            let mut output = output.lock().await;
            output.write_all(tail).await.unwrap();
            output.flush().await.unwrap();
            true
        });
        let (notices_tx, notices_rx) = mpsc::unbounded_channel();
        let correlation_id = Uuid::new_v4();
        spawn_stdout_reader(
            DirectProvider::Codex,
            correlation_id,
            child.inner().stdout.take().unwrap(),
            writer.clone(),
            notices_tx.clone(),
        );
        spawn_stderr_reader(
            correlation_id,
            child.inner().stderr.take().unwrap(),
            writer.clone(),
            notices_tx,
        );
        let (_commands_tx, commands_rx) = mpsc::channel(1);
        tokio::time::timeout(
            Duration::from_secs(10),
            monitor_process(
                host.clone(),
                DirectProvider::Codex,
                correlation_id,
                child,
                None,
                Some(cancellation.clone()),
                None,
                Some(control),
                writer,
                notices_rx,
                commands_rx,
            ),
        )
        .await
        .unwrap();
        assert!(!cancellation.is_cancelled());
        assert!(
            adapter.await.unwrap(),
            "adapter was cancelled before flushing"
        );
        let observations = host.response_after(0).await.unwrap();
        assert!(observations.terminal);
        assert!(observations.events.iter().any(|event| matches!(
            &event.payload,
            HostEventPayload::Terminal {
                status: AgentRunStatus::Succeeded, error: None, audit_manifest, ..
            } if audit_manifest.integrity_status == NativeAuditIntegrityStatus::Complete
                && audit_manifest.frame_count == 1
        )));
        let content = tokio::fs::read_to_string(frames_path).await.unwrap();
        let frame: NativeAuditFrame = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(frame.payload_bytes().unwrap(), tail);
    }

    #[tokio::test]
    async fn subscription_replays_then_wakes_without_polling_and_reconnects_by_cursor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let host = test_host(&path).await;
        for _ in 0..3 {
            host.append_event(test_payload()).await.unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(serve(listener, host.clone()));
        let mut stream = subscribe_host_events(&endpoint, "fixture-token", 1)
            .await
            .unwrap();
        let replay = read_host_subscription(&mut stream).await.unwrap();
        assert_eq!(
            replay
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert!(
            read_host_subscription(&mut stream)
                .await
                .unwrap()
                .events
                .is_empty()
        );
        let started = tokio::time::Instant::now();
        host.append_event(test_payload()).await.unwrap();
        let live = tokio::time::timeout(
            Duration::from_millis(500),
            read_host_subscription(&mut stream),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(live.events[0].sequence, 4);
        assert!(started.elapsed() < Duration::from_millis(500));
        drop(stream);
        host.append_event(test_payload()).await.unwrap();
        host.terminal.store(true, Ordering::Release);
        host.changes.send_modify(|_| {});
        let mut reconnected = subscribe_host_events(&endpoint, "fixture-token", 4)
            .await
            .unwrap();
        let final_page = read_host_subscription(&mut reconnected).await.unwrap();
        assert_eq!(final_page.events.len(), 1);
        assert_eq!(final_page.events[0].sequence, 5);
        assert!(final_page.terminal);
        // Journal is already readable before the downstream terminal ack.
        assert_eq!(
            tokio::fs::read_to_string(&path)
                .await
                .unwrap()
                .lines()
                .count(),
            5
        );
        server.abort();
    }

    #[tokio::test]
    async fn bounded_pages_do_not_report_terminal_before_last_event() {
        let directory = tempfile::tempdir().unwrap();
        let host = test_host(&directory.path().join("events.jsonl")).await;
        for _ in 0..5 {
            host.append_event(test_payload()).await.unwrap();
        }
        host.terminal.store(true, Ordering::Release);
        let first = host.response_page(0, 2).await.unwrap();
        assert_eq!(first.events.len(), 2);
        assert!(!first.terminal);
        let last = host.response_page(4, 2).await.unwrap();
        assert_eq!(last.events[0].sequence, 5);
        assert!(last.terminal);
    }

    #[tokio::test]
    async fn dead_host_journal_replays_after_cursor_and_ignores_uncommitted_tail() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let host = test_host(&path).await;
        for _ in 0..3 {
            host.append_event(test_payload()).await.unwrap();
        }
        drop(host);
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .unwrap();
        file.write_all(b"{\"sequence\":4").await.unwrap();
        file.flush().await.unwrap();
        drop(file);
        let mut replay = HostJournalReplay::open(&path, 1).await.unwrap();
        let events = replay.next_page().await.unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert!(replay.next_page().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn idle_subscription_does_not_block_authenticated_control_and_rejects_wrong_token() {
        let directory = tempfile::tempdir().unwrap();
        let host = test_host(&directory.path().join("events.jsonl")).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(serve(listener, host));
        let mut subscriber = subscribe_host_events(&endpoint, "fixture-token", 0)
            .await
            .unwrap();
        assert!(
            read_host_subscription(&mut subscriber)
                .await
                .unwrap()
                .events
                .is_empty()
        );
        let control = tokio::time::timeout(
            Duration::from_millis(500),
            send_host_command(
                &endpoint,
                "fixture-token",
                HostCommand::Control {
                    bytes: vec![],
                    control: None,
                    cancel: true,
                    after_sequence: 0,
                },
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            control.error.as_deref(),
            Some("provider process is not running")
        );
        let mut rejected = subscribe_host_events(&endpoint, "wrong-token", 0)
            .await
            .unwrap();
        assert!(read_host_subscription(&mut rejected).await.is_err());
        server.abort();
    }

    #[tokio::test]
    async fn host_command_response_read_is_bounded() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let endpoint = listener.local_addr().expect("local address").to_string();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("accept");
            std::future::pending::<()>().await;
        });

        let error = send_host_command_with_timeout(
            &endpoint,
            "test-auth-token",
            HostCommand::Attach { after_sequence: 0 },
            Duration::from_millis(25),
        )
        .await
        .expect_err("host that never responds must time out");

        assert!(matches!(error, TransportError::TemporarilyUnavailable(_)));
        assert!(error.to_string().contains("timed out"));
        server.abort();
    }

    #[test]
    fn cancellation_has_a_larger_protocol_budget_than_attach() {
        assert!(HOST_CANCEL_COMMAND_TIMEOUT > HOST_ATTACH_COMMAND_TIMEOUT);
        assert!(HOST_CANCEL_COMMAND_TIMEOUT > HOST_CANCEL_RESULT_TIMEOUT);
        assert!(HOST_CANCEL_RESULT_TIMEOUT > PROCESS_OUTPUT_DRAIN_TIMEOUT);
        assert!(HOST_CANCEL_COMMAND_TIMEOUT > PROCESS_OUTPUT_DRAIN_TIMEOUT);
        assert!(HOST_TERMINAL_ACK_GRACE_PERIOD > HOST_CANCEL_COMMAND_TIMEOUT);
    }

    #[test]
    fn drained_provider_failure_overrides_racing_successful_exit() {
        let cause = prefer_drained_provider_terminal(
            ExitCause::Executor(ExecutorExitResult::Success),
            AgentRunStatus::Failed,
            false,
        );

        assert!(matches!(
            cause,
            ExitCause::ProviderTerminal(AgentRunStatus::Failed)
        ));
    }

    #[test]
    fn cancellation_keeps_precedence_over_drained_provider_terminal() {
        let cause =
            prefer_drained_provider_terminal(ExitCause::Cancelled, AgentRunStatus::Failed, true);

        assert!(matches!(cause, ExitCause::Cancelled));
    }
}
