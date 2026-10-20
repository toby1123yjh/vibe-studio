use std::{
    future::Future,
    path::{Path, PathBuf},
    process::Stdio,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use agent_client_protocol as proto;
use agent_client_protocol::Agent as _;
use command_group::AsyncGroupChild;
use futures::StreamExt;
use tokio::{io::AsyncWriteExt, process::Command, sync::mpsc};
use tokio_util::{
    compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt},
    io::ReaderStream,
    sync::CancellationToken,
};
use tracing::error;
use workspace_utils::{
    approvals::ApprovalStatus, command_ext::GroupSpawnNoWindowExt, stream_lines::LinesStreamExt,
};

use super::{
    AcpClient, SessionManager,
    control::AcpControl,
    session_config::{
        AcpDialect, AcpRuntimeConfiguration, LegacySessionConfiguration, NativeRestoreMethod,
        apply_config_options, apply_legacy_configuration, observe_catalog, observe_legacy_catalog,
        observed_configuration,
    },
};
use crate::{
    approvals::ExecutorApprovalService,
    command::{CmdOverrides, CommandParts},
    env::ExecutionEnv,
    executors::{ExecutorControl, ExecutorError, ExecutorExitResult, SpawnedChild, acp::AcpEvent},
    workflow_mcp::WorkflowMcpReadiness,
};

#[derive(Debug, Clone, Copy)]
enum AcpPromptLoopOutcome {
    Completed,
    Cancelled,
    Failed,
}

impl AcpPromptLoopOutcome {
    fn exit_result(self) -> ExecutorExitResult {
        match self {
            Self::Completed => ExecutorExitResult::Success,
            Self::Cancelled | Self::Failed => ExecutorExitResult::Failure,
        }
    }
}

/// `session/load` may acknowledge before an ACP provider's history replay has
/// finished. Because ACP has no replay-complete notification, hold provider
/// events behind the client gate until the attempted notification count stays
/// unchanged for a short quiescence window. This keeps the pre-adoption
/// transcript out of VK's canonical event stream while allowing the first VK
/// prompt and all subsequent output through.
async fn wait_for_native_replay_quiescence(client: &AcpClient, cancel: &CancellationToken) -> bool {
    const SAMPLE_INTERVAL: Duration = Duration::from_millis(10);
    const QUIET_WINDOW: Duration = Duration::from_millis(100);

    let mut last_count = client.suppressed_event_count();
    let mut quiet_since = Instant::now();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return false,
            _ = tokio::time::sleep(SAMPLE_INTERVAL) => {}
        }
        let count = client.suppressed_event_count();
        if count != last_count {
            last_count = count;
            quiet_since = Instant::now();
        } else if quiet_since.elapsed() >= QUIET_WINDOW {
            return true;
        }
    }
}

async fn bound_acp_startup<T, E: std::fmt::Display>(
    deadline: Option<tokio::time::Instant>,
    cancel: &CancellationToken,
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, ExecutorError> {
    WorkflowMcpReadiness::bound_startup(deadline, cancel, future)
        .await?
        .map_err(|error| ExecutorError::Io(std::io::Error::other(error.to_string())))
}

/// Closing is a resource/persistence flush, not deletion of native history.
/// Bound only shutdown acknowledgements; never bound the agent's run time.
async fn finish_native_session(
    connection: &proto::ClientSideConnection,
    session_id: &str,
    supports_close: bool,
    cancelled: bool,
) -> bool {
    const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
    if cancelled {
        let sent = tokio::time::timeout(
            SHUTDOWN_TIMEOUT,
            connection.cancel(proto::CancelNotification::new(session_id.to_owned())),
        )
        .await;
        if !matches!(sent, Ok(Ok(()))) {
            return false;
        }
    }
    if supports_close {
        return matches!(
            tokio::time::timeout(
                SHUTDOWN_TIMEOUT,
                connection.close_session(proto::CloseSessionRequest::new(session_id.to_owned()))
            )
            .await,
            Ok(Ok(_))
        );
    }
    true
}

/// Reusable harness for ACP-based connections such as Gemini.
pub struct AcpAgentHarness {
    session_namespace: String,
    dialect: AcpDialect,
    model: Option<String>,
    reasoning_effort: Option<String>,
    catalog_identity: Option<String>,
    mode: Option<String>,
    mcp_servers: Vec<proto::McpServer>,
    workflow_readiness: Option<WorkflowMcpReadiness>,
}

impl Default for AcpAgentHarness {
    fn default() -> Self {
        // Keep existing behavior for Gemini
        Self::new()
    }
}

impl AcpAgentHarness {
    /// Create a harness with the default Gemini namespace
    pub fn new() -> Self {
        Self {
            session_namespace: "gemini_sessions".to_string(),
            dialect: AcpDialect::Gemini,
            model: None,
            reasoning_effort: None,
            catalog_identity: None,
            mode: None,
            mcp_servers: Vec::new(),
            workflow_readiness: None,
        }
    }

    /// Create a harness with a custom session namespace.
    pub fn with_session_namespace(namespace: impl Into<String>) -> Self {
        Self {
            session_namespace: namespace.into(),
            dialect: AcpDialect::Gemini,
            model: None,
            reasoning_effort: None,
            catalog_identity: None,
            mode: None,
            mcp_servers: Vec::new(),
            workflow_readiness: None,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_dialect(mut self, dialect: AcpDialect) -> Self {
        self.dialect = dialect;
        self
    }

    pub fn with_reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        self.reasoning_effort = Some(effort.into());
        self
    }

    pub fn with_catalog_identity(mut self, identity: String) -> Self {
        self.catalog_identity = Some(identity);
        self
    }

    pub fn with_mode(mut self, mode: impl Into<String>) -> Self {
        self.mode = Some(mode.into());
        self
    }

    pub fn with_mcp_servers(mut self, servers: Vec<proto::McpServer>) -> Self {
        self.mcp_servers = servers;
        self
    }

    pub fn with_workflow_readiness(mut self, readiness: Option<WorkflowMcpReadiness>) -> Self {
        self.workflow_readiness = readiness;
        self
    }

    pub fn apply_overrides(&mut self, executor_config: &crate::profile::ExecutorConfig) {
        if let Some(model_id) = &executor_config.model_id {
            self.model = model_id.clone();
        }

        if let Some(agent_id) = &executor_config.agent_id {
            self.mode = Some(agent_id.clone());
        }
        if let Some(effort) = &executor_config.reasoning_id {
            self.reasoning_effort = effort.clone();
        }
    }

    pub async fn spawn_with_command(
        self,
        current_dir: &Path,
        prompt: String,
        command_parts: CommandParts,
        env: &ExecutionEnv,
        cmd_overrides: &CmdOverrides,
        approvals: Option<std::sync::Arc<dyn ExecutorApprovalService>>,
    ) -> Result<SpawnedChild, ExecutorError> {
        let (program_path, args) = command_parts.into_resolved().await?;
        let mut command = Command::new(program_path);
        command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(current_dir)
            .env("NPM_CONFIG_LOGLEVEL", "error")
            .env("NODE_NO_WARNINGS", "1")
            .args(&args);

        env.clone()
            .with_profile(cmd_overrides)
            .apply_to_command(&mut command);

        let mut child = command.group_spawn_no_window()?;

        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<ExecutorExitResult>();
        let cancel = CancellationToken::new();

        let control = Self::bootstrap_acp_connection(
            &mut child,
            current_dir.to_path_buf(),
            None,
            prompt,
            Some(exit_tx),
            self.session_namespace.clone(),
            self.dialect,
            self.model.clone(),
            self.reasoning_effort.clone(),
            self.catalog_identity.clone(),
            self.mode.clone(),
            approvals,
            cancel.clone(),
            false,
            self.mcp_servers.clone(),
            self.workflow_readiness,
        )
        .await?;

        Ok(SpawnedChild {
            child,
            exit_signal: Some(exit_rx),
            cancel: Some(cancel),
            control,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn spawn_follow_up_with_command(
        self,
        current_dir: &Path,
        prompt: String,
        session_id: &str,
        command_parts: CommandParts,
        env: &ExecutionEnv,
        cmd_overrides: &CmdOverrides,
        approvals: Option<std::sync::Arc<dyn ExecutorApprovalService>>,
    ) -> Result<SpawnedChild, ExecutorError> {
        let (program_path, args) = command_parts.into_resolved().await?;
        let mut command = Command::new(program_path);
        command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(current_dir)
            .env("NPM_CONFIG_LOGLEVEL", "error")
            .env("NODE_NO_WARNINGS", "1")
            .args(&args);

        env.clone()
            .with_profile(cmd_overrides)
            .apply_to_command(&mut command);

        let mut child = command.group_spawn_no_window()?;

        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<ExecutorExitResult>();
        let cancel = CancellationToken::new();

        let control = Self::bootstrap_acp_connection(
            &mut child,
            current_dir.to_path_buf(),
            Some(session_id.to_string()),
            prompt,
            Some(exit_tx),
            self.session_namespace.clone(),
            self.dialect,
            self.model.clone(),
            self.reasoning_effort.clone(),
            self.catalog_identity.clone(),
            self.mode.clone(),
            approvals,
            cancel.clone(),
            true,
            self.mcp_servers.clone(),
            self.workflow_readiness,
        )
        .await?;

        Ok(SpawnedChild {
            child,
            exit_signal: Some(exit_rx),
            cancel: Some(cancel),
            control,
        })
    }

    /// Resume a provider-owned ACP session without forking it or synthesizing
    /// a prompt from VK's local history. The provider's native session id is
    /// kept as the ACP session id for the lifetime of the run.
    #[allow(clippy::too_many_arguments)]
    pub async fn spawn_native_resume_with_command(
        self,
        current_dir: &Path,
        prompt: String,
        session_id: &str,
        command_parts: CommandParts,
        env: &ExecutionEnv,
        cmd_overrides: &CmdOverrides,
        approvals: Option<std::sync::Arc<dyn ExecutorApprovalService>>,
    ) -> Result<SpawnedChild, ExecutorError> {
        let (program_path, args) = command_parts.into_resolved().await?;
        let mut command = Command::new(program_path);
        command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(current_dir)
            .env("NPM_CONFIG_LOGLEVEL", "error")
            .env("NODE_NO_WARNINGS", "1")
            .args(&args);

        env.clone()
            .with_profile(cmd_overrides)
            .apply_to_command(&mut command);

        let mut child = command.group_spawn_no_window()?;
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<ExecutorExitResult>();
        let cancel = CancellationToken::new();

        let control = Self::bootstrap_acp_connection(
            &mut child,
            current_dir.to_path_buf(),
            Some(session_id.to_string()),
            prompt,
            Some(exit_tx),
            self.session_namespace.clone(),
            self.dialect,
            self.model.clone(),
            self.reasoning_effort.clone(),
            self.catalog_identity.clone(),
            self.mode.clone(),
            approvals,
            cancel.clone(),
            true,
            self.mcp_servers.clone(),
            self.workflow_readiness,
        )
        .await?;

        Ok(SpawnedChild {
            child,
            exit_signal: Some(exit_rx),
            cancel: Some(cancel),
            control,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn bootstrap_acp_connection(
        child: &mut AsyncGroupChild,
        cwd: PathBuf,
        existing_session: Option<String>,
        prompt: String,
        exit_signal: Option<tokio::sync::oneshot::Sender<ExecutorExitResult>>,
        session_namespace: String,
        dialect: AcpDialect,
        model: Option<String>,
        reasoning_effort: Option<String>,
        catalog_identity: Option<String>,
        mode: Option<String>,
        approvals: Option<std::sync::Arc<dyn ExecutorApprovalService>>,
        cancel: CancellationToken,
        native_resume: bool,
        mcp_servers: Vec<proto::McpServer>,
        workflow_readiness: Option<WorkflowMcpReadiness>,
    ) -> Result<Option<Arc<dyn ExecutorControl>>, ExecutorError> {
        let native_resume =
            native_resume || (dialect != AcpDialect::Gemini && existing_session.is_some());
        let control = AcpControl::new(cancel.clone());
        let control_for_writer = control.clone();
        let control_for_session = control.clone();
        // Take child's stdio for ACP wiring
        let orig_stdout = child.inner().stdout.take().ok_or_else(|| {
            ExecutorError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Child process has no stdout",
            ))
        })?;
        let orig_stdin = child.inner().stdin.take().ok_or_else(|| {
            ExecutorError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Child process has no stdin",
            ))
        })?;

        // Create a fresh stdout pipe for logs
        let writer = crate::stdout_dup::create_stdout_pipe_writer(child)?;
        let shared_writer = Arc::new(tokio::sync::Mutex::new(writer));
        let (log_tx, mut log_rx) = mpsc::unbounded_channel::<String>();

        // Spawn log -> stdout writer task
        let log_writer = tokio::spawn(async move {
            while let Some(line) = log_rx.recv().await {
                let mut data = line.into_bytes();
                data.push(b'\n');
                let mut w = shared_writer.lock().await;
                let _ = w.write_all(&data).await;
            }
        });

        // ACP client STDIO
        let (mut to_acp_writer, acp_incoming_reader) = tokio::io::duplex(64 * 1024);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        // Process stdout -> ACP
        let stdout_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            let mut stdout_stream = ReaderStream::new(orig_stdout);
            while let Some(res) = stdout_stream.next().await {
                if *stdout_shutdown_rx.borrow() {
                    break;
                }
                match res {
                    Ok(data) => {
                        let _ = to_acp_writer.write_all(&data).await;
                    }
                    Err(_) => break,
                }
            }
        });

        // ACP crate expects futures::AsyncRead + AsyncWrite, use tokio compat to adapt tokio::io::AsyncRead + Write
        let (acp_out_writer, acp_out_reader) = tokio::io::duplex(64 * 1024);
        let outgoing = acp_out_writer.compat_write();
        let incoming = acp_incoming_reader.compat();

        // Process ACP -> stdin
        let stdin_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            let mut child_stdin = orig_stdin;
            let mut lines = ReaderStream::new(acp_out_reader)
                .map(|res| res.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
                .lines();
            while let Some(result) = lines.next().await {
                if *stdin_shutdown_rx.borrow() {
                    break;
                }
                match result {
                    Ok(line) => {
                        // Use \r\n on Windows for compatibility with buggy ACP implementations
                        const LINE_ENDING: &str = if cfg!(windows) { "\r\n" } else { "\n" };
                        let line = line + LINE_ENDING;
                        if let Err(err) = child_stdin.write_all(line.as_bytes()).await {
                            tracing::debug!("Failed to write to child stdin {err}");
                            break;
                        }
                        if child_stdin.flush().await.is_err() {
                            break;
                        }
                        control_for_writer.acknowledge_written(line.as_bytes());
                    }
                    Err(err) => {
                        tracing::debug!("ACP stdin line error {err}");
                        break;
                    }
                }
            }
        });

        let mut exit_signal_tx = exit_signal;
        let startup_deadline = workflow_readiness
            .as_ref()
            .map(WorkflowMcpReadiness::startup_deadline);

        // Run ACP client in a LocalSet
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build runtime");

            rt.block_on(async move {
                let local = tokio::task::LocalSet::new();
                local
                    .run_until(async move {
                        // Create event and raw channels
                        // Typed events available for future use; raw lines forwarded and persisted
                        let (event_tx, mut event_rx) =
                            mpsc::unbounded_channel::<crate::executors::acp::AcpEvent>();

                        // Create session manager
                        let session_manager = if dialect == AcpDialect::Gemini { match SessionManager::new(session_namespace) {
                            Ok(sm) => Some(Arc::new(sm)),
                            Err(e) => {
                                error!("Failed to create session manager: {}", e);
                                if let Some(tx) = exit_signal_tx.take() {
                                    let _ = tx.send(ExecutorExitResult::Failure);
                                }
                                let _ = shutdown_tx.send(true);
                                return;
                            }
                        }} else { None };

                        // Create ACP client with approvals support
                        let mut client =
                            AcpClient::new(event_tx.clone(), approvals.clone(), cancel.clone());
                        if approvals.is_some() {
                            client = client.with_native_control(control_for_session.clone());
                        }
                        let client_feedback_handle = client.clone();

                        if native_resume {
                            client.suppress_events();
                        } else {
                            client.record_user_prompt_event(&prompt);
                        }

                        // Set up connection
                        let (conn, io_fut) =
                            proto::ClientSideConnection::new(client, outgoing, incoming, |fut| {
                                tokio::task::spawn_local(fut);
                            });
                        let conn = Rc::new(conn);

                        // Drive I/O
                        let io_handle = tokio::task::spawn_local(async move {
                            let _ = io_fut.await;
                        });

                        // Initialize
                        let initialization = match bound_acp_startup(
                            startup_deadline,
                            &cancel,
                            conn.initialize(proto::InitializeRequest::new(
                                proto::ProtocolVersion::V1,
                            )),
                        )
                        .await {
                            Ok(response) => response,
                            Err(_) => {
                                let _ = log_tx.send(AcpEvent::Error("Failed to initialize ACP connection".to_string()).to_string());
                                if let Some(tx) = exit_signal_tx.take() {
                                    let _ = tx.send(ExecutorExitResult::Failure);
                                }
                                let _ = shutdown_tx.send(true);
                                io_handle.abort();
                                return;
                            }
                        };
                        let supports_close = initialization.agent_capabilities.session_capabilities.close.is_some();

                        // Handle provider-native loading, VK-owned forking, or
                        // creation. Native loading deliberately never reads or
                        // writes VK's prior transcript.
                        let (acp_session_id, display_session_id, prompt_to_send, config_options, legacy_configuration) = if native_resume
                        {
                            let Some(existing) = existing_session else {
                                error!("Native ACP resume requires a provider session id");
                                if let Some(tx) = exit_signal_tx.take() {
                                    let _ = tx.send(ExecutorExitResult::Failure);
                                }
                                let _ = shutdown_tx.send(true);
                                return;
                            };
                            let native_id = existing.clone();
                            let restoration = async {
                                let method = dialect.restore_method(&initialization.agent_capabilities)?;
                                let options = match method {
                                    NativeRestoreMethod::Resume => conn.resume_session(
                                        proto::ResumeSessionRequest::new(native_id.clone(), cwd.clone()).mcp_servers(mcp_servers.clone()),
                                    ).await.map(|response| (response.config_options, LegacySessionConfiguration {models: response.models, modes: response.modes})),
                                    NativeRestoreMethod::Load => conn.load_session(
                                        proto::LoadSessionRequest::new(native_id.clone(), cwd.clone()).mcp_servers(mcp_servers.clone()),
                                    ).await.map(|response| (response.config_options, LegacySessionConfiguration {models: response.models, modes: response.modes})),
                                }.map_err(|_| ExecutorError::FollowUpNotSupported("The agent could not restore its native session".to_string()))?;
                                Ok::<_, ExecutorError>((options.0.unwrap_or_default(), method, options.1))
                            };
                            match bound_acp_startup(startup_deadline, &cancel, restoration).await {
                                Ok((options, method, legacy)) => {
                                    // ACP session/load is allowed to return
                                    // before the provider finishes streaming
                                    // history. Keep the event gate closed and
                                    // wait for a quiet boundary before the
                                    // first VK-owned prompt is forwarded.
                                    if method == NativeRestoreMethod::Load && !WorkflowMcpReadiness::bound_startup(
                                        startup_deadline,
                                        &cancel,
                                        wait_for_native_replay_quiescence(
                                            &client_feedback_handle,
                                            &cancel,
                                        ),
                                    )
                                    .await
                                    .unwrap_or(false)
                                    {
                                        if let Some(tx) = exit_signal_tx.take() {
                                            let _ = tx.send(ExecutorExitResult::Failure);
                                        }
                                        let _ = shutdown_tx.send(true);
                                        return;
                                    }
                                    (native_id.clone(), native_id, prompt.clone(), options, legacy)
                                }
                                Err(_) => {
                                    error!("Failed to restore native ACP session; no fresh session will be created");
                                    let _ = log_tx.send(
                                        AcpEvent::Error("Failed to restore native ACP session; verify the native session, working directory and provider capabilities".to_string())
                                        .to_string(),
                                    );
                                    if let Some(tx) = exit_signal_tx.take() {
                                        let _ = tx.send(ExecutorExitResult::Failure);
                                    }
                                    let _ = shutdown_tx.send(true);
                                    io_handle.abort();
                                    return;
                                }
                            }
                        } else if let Some(existing) = existing_session {
                            // Fork existing session
                            let Some(session_manager) = session_manager.as_ref() else {
                                let _ = log_tx.send(AcpEvent::Error("Native ACP sessions cannot use transcript-fork continuation".to_string()).to_string());
                                if let Some(tx) = exit_signal_tx.take() { let _ = tx.send(ExecutorExitResult::Failure); }
                                let _ = shutdown_tx.send(true);
                                io_handle.abort();
                                return;
                            };
                            let new_ui_id = uuid::Uuid::new_v4().to_string();
                            let _ = session_manager.fork_session(&existing, &new_ui_id);

                            let history = session_manager.read_session_raw(&new_ui_id).ok();
                            let meta = history.map(|h| serde_json::json!({ "history_jsonl": h }));

                            let mut req = proto::NewSessionRequest::new(cwd.clone())
                                .mcp_servers(mcp_servers.clone());
                            if let Some(m) = meta
                                && let Some(obj) = m.as_object()
                            {
                                req = req.meta(obj.clone());
                            }
                            match bound_acp_startup(
                                startup_deadline,
                                &cancel,
                                conn.new_session(req),
                            )
                            .await
                            {
                                Ok(resp) => {
                                    let resume_prompt = session_manager
                                        .generate_resume_prompt(&new_ui_id, &prompt)
                                        .unwrap_or_else(|_| prompt.clone());
                                    (resp.session_id.0.to_string(), new_ui_id, resume_prompt, resp.config_options.unwrap_or_default(), LegacySessionConfiguration {models: resp.models, modes: resp.modes})
                                }
                                Err(_) => {
                                    error!("Failed to create ACP session");
                                    let _ = log_tx.send(
                                        AcpEvent::Error("Failed to create ACP session".to_string())
                                        .to_string(),
                                    );
                                    if let Some(tx) = exit_signal_tx.take() {
                                        let _ = tx.send(ExecutorExitResult::Failure);
                                    }
                                    let _ = shutdown_tx.send(true);
                                    return;
                                }
                            }
                        } else {
                            // New session
                            match bound_acp_startup(
                                startup_deadline,
                                &cancel,
                                conn.new_session(
                                    proto::NewSessionRequest::new(cwd.clone())
                                        .mcp_servers(mcp_servers.clone()),
                                ),
                            )
                            .await
                            {
                                Ok(resp) => {
                                    let sid = resp.session_id.0.to_string();
                                    (sid.clone(), sid, prompt.clone(), resp.config_options.unwrap_or_default(), LegacySessionConfiguration {models: resp.models, modes: resp.modes})
                                }
                                Err(_) => {
                                    error!("Failed to create ACP session");
                                    let _ = log_tx.send(
                                        AcpEvent::Error("Failed to create ACP session".to_string())
                                        .to_string(),
                                    );
                                    if let Some(tx) = exit_signal_tx.take() {
                                        let _ = tx.send(ExecutorExitResult::Failure);
                                    }
                                    let _ = shutdown_tx.send(true);
                                    return;
                                }
                            }
                        };
                        control_for_session.set_session(acp_session_id.clone());

                        if let Err(error) =
                            WorkflowMcpReadiness::wait_optional(workflow_readiness, &cancel).await
                        {
                            error!("ACP workflow MCP startup failed: {error}");
                            let _ = log_tx.send(AcpEvent::Error(error.to_string()).to_string());
                            let _ = finish_native_session(&conn, &acp_session_id, supports_close, true).await;
                            if let Some(tx) = exit_signal_tx.take() {
                                let _ = tx.send(ExecutorExitResult::Failure);
                            }
                            let _ = shutdown_tx.send(true);
                            io_handle.abort();
                            cancel.cancel();
                            return;
                        }

                        // Emit session ID
                        let _ = log_tx
                            .send(AcpEvent::SessionStart(display_session_id.clone()).to_string());

                        if dialect != AcpDialect::Gemini {
                            let result = bound_acp_startup(startup_deadline, &cancel, apply_config_options(
                                &conn, &acp_session_id, dialect, config_options,
                                model.as_deref(), reasoning_effort.as_deref(), mode.as_deref(),
                            )).await;
                            match result {
                                Ok(options) => {
                                    let _ = log_tx.send(AcpEvent::ConfigurationObserved(observed_configuration(dialect, &options)).to_string());
                                    if let Some(identity) = &catalog_identity
                                        && let Some(observation) = observe_catalog(dialect, cwd.clone(), identity.clone(), &options)
                                    {
                                        let _ = log_tx.send(AcpEvent::CatalogObserved(observation).to_string());
                                    }
                                },
                                Err(_) => {
                                    let _ = log_tx.send(AcpEvent::Error("The agent rejected the requested model, reasoning effort or mode; no prompt was sent".to_string()).to_string());
                                    let closed = finish_native_session(&conn, &acp_session_id, supports_close, cancel.is_cancelled()).await;
                                    let _ = shutdown_tx.send(true);
                                    io_handle.abort();
                                    drop(log_tx);
                                    let _ = log_writer.await;
                                    control_for_session.mark_closed(closed);
                                    if let Some(tx) = exit_signal_tx.take() { let _ = tx.send(ExecutorExitResult::Failure); }
                                    return;
                                }
                            }
                        } else {
                            match bound_acp_startup(startup_deadline, &cancel, apply_legacy_configuration(
                                &conn, &acp_session_id, legacy_configuration, model.as_deref(), mode.as_deref(),
                            )).await {
                                Ok(state) => {
                                    let model_id = state.models.as_ref().map(|models| models.current_model_id.0.to_string()).filter(|model| !model.is_empty());
                                    let _ = log_tx.send(AcpEvent::ConfigurationObserved(AcpRuntimeConfiguration {
                                        model_id, reasoning_id: None, source: "acp_session_state".into(),
                                    }).to_string());
                                    if let Some(identity) = &catalog_identity && let Some(observation) = observe_legacy_catalog(cwd.clone(), identity.clone(), &state) {
                                        let _ = log_tx.send(AcpEvent::CatalogObserved(observation).to_string());
                                    }
                                }
                                Err(_) => {
                                    let _ = log_tx.send(AcpEvent::Error("Gemini could not apply the requested model or mode; no prompt was sent".to_string()).to_string());
                                    let closed = finish_native_session(&conn, &acp_session_id, supports_close, cancel.is_cancelled()).await;
                                    let _ = shutdown_tx.send(true);
                                    io_handle.abort();
                                    drop(log_tx);
                                    let _ = log_writer.await;
                                    control_for_session.mark_closed(closed);
                                    if let Some(tx) = exit_signal_tx.take() { let _ = tx.send(ExecutorExitResult::Failure); }
                                    return;
                                }
                            }
                        }
                        if let Some(identity) = catalog_identity {
                            client_feedback_handle.enable_configuration_observations(dialect, cwd.clone(), identity);
                        }

                        // Option-setting requests share the startup deadline;
                        // never persist or forward the prompt after a timeout.
                        if cancel.is_cancelled() {
                            let _ = finish_native_session(&conn, &acp_session_id, supports_close, true).await;
                            if let Some(tx) = exit_signal_tx.take() {
                                let _ = tx.send(ExecutorExitResult::Failure);
                            }
                            let _ = shutdown_tx.send(true);
                            io_handle.abort();
                            return;
                        }

                        // Start raw event forwarder and persistence
                        let app_tx_clone = log_tx.clone();
                        let sess_id_for_writer = display_session_id.clone();
                        let sm_for_writer = session_manager.clone();
                        let conn_for_cancel = conn.clone();
                        let acp_session_id_for_cancel = acp_session_id.clone();
                        let (event_flush_tx, mut event_flush_rx) = mpsc::unbounded_channel::<tokio::sync::oneshot::Sender<()>>();
                        let event_forwarder = tokio::task::spawn_local(async move {
                            loop {
                                let event = tokio::select! {
                                    biased;
                                    event = event_rx.recv() => event,
                                    Some(acknowledgement) = event_flush_rx.recv() => {
                                        let _ = acknowledgement.send(());
                                        continue;
                                    }
                                };
                                let Some(event) = event else { break; };
                                if let AcpEvent::ApprovalResponse(resp) = &event
                                    && let ApprovalStatus::Denied {
                                        reason: Some(reason),
                                    } = &resp.status
                                    && !reason.trim().is_empty()
                                {
                                    let _ = conn_for_cancel
                                        .cancel(proto::CancelNotification::new(
                                            proto::SessionId::new(
                                                acp_session_id_for_cancel.clone(),
                                            ),
                                        ))
                                        .await;
                                }

                                let line = event.to_string();
                                // Forward to stdout
                                let _ = app_tx_clone.send(line.clone());
                                // Persist to session file
                                if let Some(sm) = &sm_for_writer {
                                    let _ = sm.append_raw_line(&sess_id_for_writer, &line);
                                }
                            }
                        });

                        // Save prompt to session
                        if let Some(sm) = session_manager {
                        let _ = sm.append_raw_line(
                            &display_session_id,
                            &serde_json::to_string(&serde_json::json!({ "user": prompt_to_send }))
                                .unwrap_or_default(),
                        );
                        }

                        // Build prompt request
                        let initial_req = proto::PromptRequest::new(
                            proto::SessionId::new(acp_session_id.clone()),
                            vec![proto::ContentBlock::Text(proto::TextContent::new(
                                prompt_to_send,
                            ))],
                        );

                        let mut current_req = Some(initial_req);
                        let mut prompt_outcome = AcpPromptLoopOutcome::Completed;
                        let mut native_prompt_recorded = false;
                        let mut completion_reason = None;

                        while let Some(req) = current_req.take() {
                            if cancel.is_cancelled() {
                                tracing::debug!("ACP executor cancelled, stopping prompt loop");
                                prompt_outcome = AcpPromptLoopOutcome::Cancelled;
                                break;
                            }

                            // Open the canonical stream only once the replay
                            // gate has settled and the first VK prompt is
                            // about to be sent. This minimizes the race where
                            // a provider emits a delayed history notification
                            // between `session/load` and `prompt`.
                            if native_resume && !native_prompt_recorded {
                                client_feedback_handle
                                    .enable_events_and_record_user_prompt(&prompt);
                                native_prompt_recorded = true;
                            }

                            tracing::trace!("sending ACP prompt request");
                            // Send the prompt and await completion to obtain stop_reason
                            let prompt_result = tokio::select! {
                                _ = cancel.cancelled() => {
                                    tracing::debug!("ACP executor cancelled during prompt");
                                    prompt_outcome = AcpPromptLoopOutcome::Cancelled;
                                    break;
                                }
                                result = conn.prompt(req) => result,
                            };

                            match prompt_result {
                                Ok(resp) => {
                                    if let Some(usage) = resp.usage {
                                        let _ = log_tx.send(AcpEvent::Usage(usage).to_string());
                                    }
                                    // Emit done with stop_reason
                                    let stop_reason = serde_json::to_string(&resp.stop_reason)
                                        .unwrap_or_default();
                                    if resp.stop_reason == proto::StopReason::Cancelled || cancel.is_cancelled() {
                                        prompt_outcome = AcpPromptLoopOutcome::Cancelled;
                                        completion_reason = Some(stop_reason);
                                        break;
                                    }
                                    completion_reason = Some(stop_reason);
                                }
                                Err(_) => {
                                    if cancel.is_cancelled() {
                                        tracing::debug!("ACP prompt stopped after cancellation");
                                        prompt_outcome = AcpPromptLoopOutcome::Cancelled;
                                    } else {
                                        let _ = log_tx
                                            .send(AcpEvent::Error("The ACP agent failed to complete the prompt".to_string()).to_string());
                                        prompt_outcome = AcpPromptLoopOutcome::Failed;
                                    }
                                    break;
                                }
                            }

                            // Flush any pending user feedback after finish
                            let feedback = client_feedback_handle
                                .drain_feedback()
                                .await
                                .join("\n")
                                .trim()
                                .to_string();
                            if !feedback.is_empty() {
                                tracing::trace!("sending ACP follow-up feedback");
                                let session_id = proto::SessionId::new(acp_session_id.clone());
                                let feedback_req = proto::PromptRequest::new(
                                    session_id.clone(),
                                    vec![proto::ContentBlock::Text(proto::TextContent::new(
                                        feedback,
                                    ))],
                                );
                                current_req = Some(feedback_req);
                            }
                        }

                        // Close/drain before reporting completion. DSH's
                        // session/close flushes its durable semantic history.
                        let cancelled = matches!(prompt_outcome, AcpPromptLoopOutcome::Cancelled) || cancel.is_cancelled();
                        let closed = finish_native_session(&conn, &acp_session_id, supports_close, cancelled).await;
                        if !closed {
                            let _ = log_tx.send(AcpEvent::Error("ACP session shutdown was not acknowledged".to_string()).to_string());
                        }
                        if cancelled { prompt_outcome = AcpPromptLoopOutcome::Cancelled; }
                        else if !closed { prompt_outcome = AcpPromptLoopOutcome::Failed; }
                        // Preserve notification-before-completion ordering in
                        // the normalized audit stream, even on a fast peer.
                        let (flush_tx, flush_rx) = tokio::sync::oneshot::channel();
                        if event_flush_tx.send(flush_tx).is_ok() {
                            let _ = tokio::time::timeout(Duration::from_secs(5), flush_rx).await;
                        }
                        {
                            if cancelled {
                                let _ = log_tx.send(AcpEvent::Done(serde_json::to_string("cancelled").unwrap_or_default()).to_string());
                            } else if matches!(prompt_outcome, AcpPromptLoopOutcome::Completed) && let Some(reason) = completion_reason {
                                let _ = log_tx.send(AcpEvent::Done(reason).to_string());
                            }
                        }
                        // Cleanup
                        drop(conn);
                        let _ = shutdown_tx.send(true);
                        io_handle.abort();
                        event_forwarder.abort();
                        let _ = event_forwarder.await;
                        drop(log_tx);
                        // Flush the normalized stdout pipe before the host
                        // receives the completion signal and stops the child.
                        let _ = log_writer.await;
                        control_for_session.mark_closed(closed);
                        if let Some(tx) = exit_signal_tx.take() { let _ = tx.send(prompt_outcome.exit_result()); }
                    })
                    .await;
            });
        });

        Ok(Some(control as Arc<dyn ExecutorControl>))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use tokio::io::AsyncBufReadExt;

    use super::{AcpAgentHarness, AcpDialect, AcpEvent, AcpPromptLoopOutcome, ExecutorExitResult};
    use crate::{
        command::{CmdOverrides, CommandParts},
        env::{ExecutionEnv, RepoContext},
        executors::{SpawnedChild, provider_adapter::DirectControl},
    };

    const SELECTED_MODEL: &str = r#"["provider/name","selected/model"]"#;

    struct FakeRun {
        _directory: tempfile::TempDir,
        trace: PathBuf,
        child: SpawnedChild,
        logs: tokio::task::JoinHandle<String>,
        observed: Arc<Mutex<Vec<super::AcpEvent>>>,
    }

    impl FakeRun {
        fn trace(&self) -> Vec<serde_json::Value> {
            std::fs::read_to_string(&self.trace)
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        }

        async fn finish(mut self) -> (ExecutorExitResult, Vec<serde_json::Value>, String) {
            let result = tokio::time::timeout(
                Duration::from_secs(10),
                self.child.exit_signal.take().unwrap(),
            )
            .await
            .expect("ACP lifecycle did not finish")
            .unwrap();
            let trace = self.trace();
            let _ = self.child.child.kill().await;
            let _ = self.child.child.wait().await;
            let logs = tokio::time::timeout(Duration::from_secs(5), self.logs)
                .await
                .expect("normalized log pipe did not drain")
                .unwrap();
            (result, trace, logs)
        }

        async fn wait_for_prompt(&self) {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                if self
                    .trace()
                    .iter()
                    .any(|event| event["method"] == "session/prompt")
                {
                    return;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "fake ACP never received the prompt"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }

        async fn wait_for_approval(&self) -> String {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let id = self.observed.lock().unwrap().iter().find_map(|event| {
                    if let super::AcpEvent::ApprovalRequested { approval_id, .. } = event {
                        Some(approval_id.clone())
                    } else {
                        None
                    }
                });
                if let Some(id) = id {
                    return id;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "fake ACP permission did not reach the canonical stream"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }

    async fn fake_run(
        scenario: &str,
        dialect: AcpDialect,
        restore: bool,
        model: Option<&str>,
        effort: Option<&str>,
        workflow: bool,
    ) -> FakeRun {
        let directory = tempfile::tempdir().unwrap();
        let trace = directory.path().join("native-methods.jsonl");
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_acp_agent.mjs");
        let mut params = vec![
            fixture.to_string_lossy().into_owned(),
            scenario.to_string(),
            trace.to_string_lossy().into_owned(),
        ];
        if scenario == "gemini-initial-model" {
            params.push(model.expect("initial native model is required").to_owned());
        }
        let parts = CommandParts::new("node".to_string(), params);
        let env = ExecutionEnv::new(RepoContext::default(), false, String::new());
        let overrides = CmdOverrides::default();
        let approvals: Option<Arc<dyn crate::approvals::ExecutorApprovalService>> =
            (scenario == "permission").then(|| {
                Arc::new(crate::approvals::NoopExecutorApprovalService)
                    as Arc<dyn crate::approvals::ExecutorApprovalService>
            });
        let mut harness = AcpAgentHarness::with_session_namespace("fake-acp-unused-native-history")
            .with_dialect(dialect)
            .with_catalog_identity(super::super::session_config::catalog_identity(&overrides));
        if let Some(model) = model {
            harness = harness.with_model(model);
        }
        if let Some(effort) = effort {
            harness = harness.with_reasoning_effort(effort);
        }
        if scenario == "gemini-mode-reject" {
            harness = harness.with_mode("plan");
        }
        if workflow {
            let workflow_env = crate::workflow_mcp::test_env(&fixture);
            harness = harness.with_mcp_servers(
                super::super::provider::workflow_mcp_servers(&workflow_env).unwrap(),
            );
        }
        let mut child = if restore {
            harness
                .spawn_native_resume_with_command(
                    directory.path(),
                    "current prompt".to_string(),
                    "fixture-native-session",
                    parts,
                    &env,
                    &overrides,
                    approvals,
                )
                .await
        } else {
            harness
                .spawn_with_command(
                    directory.path(),
                    "current prompt".to_string(),
                    parts,
                    &env,
                    &overrides,
                    approvals,
                )
                .await
        }
        .expect("fake local ACP should launch");
        let stdout = child.child.inner().stdout.take().unwrap();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_events = observed.clone();
        let logs = tokio::spawn(async move {
            let mut logs = String::new();
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            while let Some(line) = lines.next_line().await.unwrap() {
                if let Ok(event) = serde_json::from_str::<super::AcpEvent>(&line) {
                    observed_events.lock().unwrap().push(event);
                }
                logs.push_str(&line);
                logs.push('\n');
            }
            logs
        });
        FakeRun {
            _directory: directory,
            trace,
            child,
            logs,
            observed,
        }
    }

    #[test]
    fn acp_prompt_loop_only_reports_success_for_completion() {
        assert!(matches!(
            AcpPromptLoopOutcome::Completed.exit_result(),
            ExecutorExitResult::Success
        ));
        assert!(matches!(
            AcpPromptLoopOutcome::Cancelled.exit_result(),
            ExecutorExitResult::Failure
        ));
        assert!(matches!(
            AcpPromptLoopOutcome::Failed.exit_result(),
            ExecutorExitResult::Failure
        ));
    }

    #[tokio::test]
    async fn dsh_native_resume_uses_real_peer_and_preserves_opaque_model_and_empty_effort() {
        let run = fake_run(
            "complete",
            AcpDialect::DeepseekHarness,
            true,
            Some(SELECTED_MODEL),
            Some(""),
            true,
        )
        .await;
        let (result, trace, logs) = run.finish().await;
        assert!(matches!(result, ExecutorExitResult::Success));
        let methods: Vec<_> = trace
            .iter()
            .filter_map(|event| event["method"].as_str())
            .collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "session/resume",
                "session/set_config_option",
                "session/set_config_option",
                "session/prompt",
                "session/close",
                "close_flushed"
            ]
        );
        assert_eq!(trace[1]["forwardedTokenPresent"], true);
        assert_eq!(trace[2]["value"], SELECTED_MODEL);
        assert_eq!(trace[3]["configId"], "reasoning_effort");
        assert_eq!(trace[3]["value"], "");
        assert!(!logs.contains(&"a".repeat(64)));
        assert!(
            !serde_json::to_string(&trace)
                .unwrap()
                .contains(&"a".repeat(64))
        );
        assert!(!logs.contains("native-private-value"));
        let observation = logs
            .lines()
            .find_map(
                |line| match serde_json::from_str::<super::AcpEvent>(line).ok()? {
                    super::AcpEvent::CatalogObserved(observation) => Some(observation),
                    _ => None,
                },
            )
            .expect("the real session must publish a safe catalog observation");
        assert_eq!(
            observation.model_selector.default_model.as_deref(),
            Some(SELECTED_MODEL)
        );
        assert_eq!(
            observation
                .model_selector
                .models
                .iter()
                .find(|model| model.id == SELECTED_MODEL)
                .unwrap()
                .reasoning_options[0]
                .id,
            ""
        );
        assert_eq!(observation.scope_id.len(), 64);
        assert!(logs.find("fresh-agent-output").unwrap() < logs.find("end_turn").unwrap());
    }

    #[tokio::test]
    async fn failed_or_unadvertised_dsh_restore_never_creates_a_fresh_session() {
        for scenario in ["restore-fail", "missing-resume"] {
            let (result, trace, logs) = fake_run(
                scenario,
                AcpDialect::DeepseekHarness,
                true,
                None,
                None,
                false,
            )
            .await
            .finish()
            .await;
            assert!(matches!(result, ExecutorExitResult::Failure));
            assert!(!trace.iter().any(|event| matches!(
                event["method"].as_str(),
                Some("session/new" | "session/load" | "session/prompt")
            )));
            assert!(!logs.contains("native-private-value"));
        }
    }

    #[tokio::test]
    async fn model_and_refreshed_effort_rejection_abort_before_prompt_and_close_the_session() {
        for (scenario, model, effort) in [
            ("complete", "unknown-model", None),
            ("complete", SELECTED_MODEL, Some("low")),
            ("config-reject", SELECTED_MODEL, None),
        ] {
            let (result, trace, logs) = fake_run(
                scenario,
                AcpDialect::DeepseekHarness,
                false,
                Some(model),
                effort,
                false,
            )
            .await
            .finish()
            .await;
            assert!(matches!(result, ExecutorExitResult::Failure));
            assert!(
                !trace
                    .iter()
                    .any(|event| event["method"] == "session/prompt")
            );
            assert!(trace.iter().any(|event| event["method"] == "close_flushed"));
            assert!(!logs.contains("native-private-value"));
            assert!(
                !trace
                    .iter()
                    .any(|event| event["configId"] == "reasoning_effort")
            );
        }
    }

    #[tokio::test]
    async fn opencode_load_fallback_suppresses_native_history_and_then_continues() {
        let (result, trace, logs) = fake_run("load", AcpDialect::Opencode, true, None, None, false)
            .await
            .finish()
            .await;
        assert!(matches!(result, ExecutorExitResult::Success));
        assert!(trace.iter().any(|event| event["method"] == "session/load"));
        assert!(!trace.iter().any(|event| event["method"] == "session/new"));
        assert!(logs.contains("fresh-agent-output"));
        assert!(!logs.contains("old-native-history"));
    }

    #[tokio::test]
    async fn gemini_continuation_loads_native_session_and_does_not_fork_or_replay_history() {
        let (result, trace, logs) = fake_run("load", AcpDialect::Gemini, true, None, None, false)
            .await
            .finish()
            .await;
        assert!(matches!(result, ExecutorExitResult::Success));
        assert!(trace.iter().any(|event| event["method"] == "session/load"));
        assert!(!trace.iter().any(|event| event["method"] == "session/new"));
        assert!(!logs.contains("old-native-history"));
        assert!(logs.contains("ConfigurationObserved"));
        assert!(logs.contains("CatalogObserved"));
        assert!(logs.find("fresh-agent-output").unwrap() < logs.find("end_turn").unwrap());
    }

    #[tokio::test]
    async fn gemini_failed_selection_never_sends_business_prompt() {
        for (scenario, model) in [
            ("gemini-model-reject", Some(SELECTED_MODEL)),
            ("gemini-model-reject", Some("custom/gemini-future")),
            ("gemini-mode-reject", None),
            ("complete", Some("unadvertised-model")),
            ("complete", Some("")),
            ("complete", Some(" \t")),
            ("gemini-missing-model-state", Some(SELECTED_MODEL)),
        ] {
            let (result, trace, logs) =
                fake_run(scenario, AcpDialect::Gemini, false, model, None, false)
                    .await
                    .finish()
                    .await;
            assert!(matches!(result, ExecutorExitResult::Failure));
            assert!(
                !trace
                    .iter()
                    .any(|event| event["method"] == "session/prompt")
            );
            assert!(trace.iter().any(|event| event["method"] == "close_flushed"));
            assert!(!logs.contains("native-private-value"));
        }
    }

    #[tokio::test]
    async fn gemini_unlisted_model_requires_native_acceptance_before_new_and_restored_prompts() {
        for restore in [false, true] {
            let (result, trace, logs) = fake_run(
                "gemini-custom-model",
                AcpDialect::Gemini,
                restore,
                Some("custom/gemini-future"),
                None,
                false,
            )
            .await
            .finish()
            .await;
            assert!(matches!(result, ExecutorExitResult::Success));
            let selected = trace
                .iter()
                .position(|event| event["method"] == "session/set_model")
                .expect("an unconfirmed model requires native acceptance");
            let prompted = trace
                .iter()
                .position(|event| event["method"] == "session/prompt")
                .unwrap();
            assert!(selected < prompted);
            assert_eq!(trace[selected]["modelId"], "custom/gemini-future");
            assert!(trace.iter().any(|event| event["method"]
                == if restore {
                    "session/load"
                } else {
                    "session/new"
                }));
            let observed = logs
                .lines()
                .find_map(|line| match serde_json::from_str::<AcpEvent>(line).ok()? {
                    AcpEvent::ConfigurationObserved(observed) => Some(observed),
                    _ => None,
                })
                .unwrap();
            // An empty native acknowledgement accepts the request but does not
            // prove which concrete model eventually serves it.
            assert!(observed.model_id.is_none());
        }
    }

    #[tokio::test]
    async fn gemini_native_current_model_confirms_initial_flag_without_redundant_setter() {
        for model in [SELECTED_MODEL, "custom/gemini-future"] {
            let (result, trace, logs) = fake_run(
                "gemini-initial-model",
                AcpDialect::Gemini,
                false,
                Some(model),
                None,
                false,
            )
            .await
            .finish()
            .await;
            assert!(matches!(result, ExecutorExitResult::Success));
            assert!(
                !trace
                    .iter()
                    .any(|event| event["method"] == "session/set_model")
            );
            assert!(
                trace
                    .iter()
                    .any(|event| event["method"] == "session/prompt")
            );
            let observed = logs
                .lines()
                .find_map(|line| match serde_json::from_str::<AcpEvent>(line).ok()? {
                    AcpEvent::ConfigurationObserved(observed) => Some(observed),
                    _ => None,
                })
                .unwrap();
            assert_eq!(observed.model_id.as_deref(), Some(model));
        }
    }

    #[tokio::test]
    async fn gemini_empty_model_setter_reply_does_not_fabricate_actual_model() {
        let (result, _, logs) = fake_run(
            "complete",
            AcpDialect::Gemini,
            false,
            Some(SELECTED_MODEL),
            None,
            false,
        )
        .await
        .finish()
        .await;
        assert!(matches!(result, ExecutorExitResult::Success));
        let observed = logs
            .lines()
            .find_map(|line| match serde_json::from_str::<AcpEvent>(line).ok()? {
                AcpEvent::ConfigurationObserved(observed) => Some(observed),
                _ => None,
            })
            .unwrap();
        assert!(observed.model_id.is_none());
    }

    #[tokio::test]
    async fn gemini_cancel_uses_native_session_id_and_waits_for_close() {
        let run = fake_run("cancel", AcpDialect::Gemini, false, None, None, false).await;
        run.wait_for_prompt().await;
        let bytes = run
            .child
            .control
            .as_ref()
            .unwrap()
            .send(DirectControl::Cancel)
            .await
            .unwrap();
        let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(wire["params"]["sessionId"], "fixture-native-session");
        let (result, trace, logs) = run.finish().await;
        assert!(matches!(result, ExecutorExitResult::Failure));
        assert!(trace.iter().any(|event| event["method"] == "close_flushed"));
        assert!(logs.contains("cancelled"));
        assert!(!logs.contains("end_turn"));
    }

    #[tokio::test]
    async fn cancel_uses_actual_native_peer_bytes_and_awaits_close_without_reporting_success() {
        for scenario in ["cancel", "cancel-close-fail"] {
            let run = fake_run(
                scenario,
                AcpDialect::DeepseekHarness,
                false,
                None,
                None,
                false,
            )
            .await;
            run.wait_for_prompt().await;
            let bytes = run
                .child
                .control
                .as_ref()
                .unwrap()
                .send(DirectControl::Cancel)
                .await
                .unwrap();
            let written: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(written["method"], "session/cancel");
            assert_eq!(written["params"]["sessionId"], "fixture-native-session");
            let (result, trace, logs) = run.finish().await;
            assert!(matches!(result, ExecutorExitResult::Failure));
            assert!(trace.iter().any(|event| event["method"] == "close_flushed"));
            assert!(logs.contains("cancelled"));
            assert!(!logs.contains("end_turn"));
            assert!(!logs.contains("native-private-value"));
            if scenario == "cancel-close-fail" {
                assert!(logs.contains("shutdown was not acknowledged"));
            }
        }
    }

    #[tokio::test]
    async fn native_cancelled_stop_reason_is_not_success() {
        let (result, trace, logs) = fake_run(
            "cancelled-native",
            AcpDialect::DeepseekHarness,
            false,
            None,
            None,
            false,
        )
        .await
        .finish()
        .await;
        assert!(matches!(result, ExecutorExitResult::Failure));
        assert!(
            trace
                .iter()
                .any(|event| event["method"] == "session/cancel")
        );
        assert!(trace.iter().any(|event| event["method"] == "close_flushed"));
        assert!(logs.contains("cancelled"));
    }

    #[tokio::test]
    async fn supervised_native_permissions_wait_for_real_decision_even_with_noop_host_service() {
        for (dialect, approved) in [
            (AcpDialect::DeepseekHarness, true),
            (AcpDialect::Opencode, false),
        ] {
            let run = fake_run("permission", dialect, false, None, None, false).await;
            let approval_id = run.wait_for_approval().await;
            assert!(
                !run.trace()
                    .iter()
                    .any(|event| event["method"] == "permission_response")
            );
            let bytes = run
                .child
                .control
                .as_ref()
                .unwrap()
                .send(DirectControl::Approve {
                    request_id: approval_id.clone(),
                    approved,
                    reason: None,
                })
                .await
                .unwrap();
            let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(response["id"], "native-permission-request");
            assert_eq!(
                response["result"]["_meta"][super::super::control::APPROVAL_META_KEY],
                approval_id
            );
            assert_eq!(
                response["result"]["outcome"]["optionId"],
                if approved {
                    "allow-native-once"
                } else {
                    "reject-native-once"
                }
            );
            let (result, trace, _) = run.finish().await;
            assert!(matches!(result, ExecutorExitResult::Success));
            assert!(
                trace
                    .iter()
                    .any(|event| event["method"] == "permission_response")
            );
            assert!(trace.iter().any(|event| event["method"] == "close_flushed"));
        }
    }
}
