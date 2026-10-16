//! OMP v18 RPC: one lossless stdout reader, correlated setup, settled completion.
//! Protocol v1 is deliberate; do not negotiate chunking until the audit decoder
//! can reassemble it. Unknown frames remain in the native audit unchanged.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{Mutex, oneshot, watch},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::OhMyPi;
use crate::{
    executors::{
        ExecutorControl, ExecutorError, ExecutorExitResult, ExecutorExitSignal,
        provider_adapter::{DirectControl, TypedProviderEvent, encode_stdio_rpc},
    },
    runtime::{
        AgentRunStatus, AgentRuntimeError, AgentRuntimeErrorKind, AgentRuntimeMessageRole,
        AgentRuntimeToolStatus,
    },
    workflow_mcp::WorkflowMcpReadiness,
};

type Output = Arc<Mutex<Box<dyn AsyncWrite + Send + Unpin>>>;
type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

#[derive(Clone)]
pub(super) struct RpcPeer {
    stdin: Output,
    pending: Pending,
    closed: Arc<AtomicBool>,
    // Own the ephemeral mount for the complete native process lifetime.
    _workflow_plugin: Arc<Option<tempfile::TempDir>>,
}

impl std::fmt::Debug for RpcPeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OhMyPiRpcPeer").finish_non_exhaustive()
    }
}

fn failure(message: impl Into<String>) -> ExecutorError {
    ExecutorError::Io(std::io::Error::other(message.into()))
}

async fn emit(output: &Output, frame: &Value) -> Result<(), ExecutorError> {
    let mut output = output.lock().await;
    output.write_all(&encode_stdio_rpc(frame)?).await?;
    output.flush().await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Succeeded,
    Failed,
    Cancelled,
}

/// Acknowledgement and yield are not session quiescence. Only the active
/// prompt's native completion plus settled evidence can complete this run.
#[derive(Debug, Default)]
struct Lifecycle {
    prompt_id: Option<String>,
    yielded: Option<Outcome>,
    terminal: Option<Outcome>,
}

impl Lifecycle {
    fn observe(&mut self, frame: &Value) {
        let Some(prompt_id) = self.prompt_id.as_deref() else {
            return;
        };
        let matches_prompt = frame["id"].as_str() == Some(prompt_id);
        match frame["type"].as_str() {
            Some("response") if matches_prompt && frame["command"] == "prompt" => {
                if frame["success"] == false {
                    self.terminal = Some(Outcome::Failed);
                } else if frame["data"]["agentInvoked"] == false {
                    // Local slash commands emit no session_settled. A get_state
                    // request from the peer verifies no background work remains.
                    self.yielded = Some(Outcome::Succeeded);
                }
            }
            Some("prompt_result") if matches_prompt => {
                self.yielded = match frame["status"].as_str() {
                    Some("completed") => Some(Outcome::Succeeded),
                    Some("aborted") => Some(Outcome::Cancelled),
                    Some("error") => Some(Outcome::Failed),
                    _ => None,
                };
                if frame["sessionSettled"] == true {
                    self.terminal = self.yielded;
                }
            }
            Some("session_settled") => self.terminal = self.yielded,
            Some("agent_end") if self.yielded.is_some() && frame["yielded"] == true => {
                // A background continuation can fail after the initial prompt
                // yielded successfully; settlement must retain that failure.
                let stop = frame["messages"]
                    .as_array()
                    .and_then(|messages| {
                        messages
                            .iter()
                            .rev()
                            .find(|message| message["role"] == "assistant")
                    })
                    .and_then(|message| message["stopReason"].as_str());
                match stop {
                    Some("error") => self.yielded = Some(Outcome::Failed),
                    Some("aborted") => self.yielded = Some(Outcome::Cancelled),
                    _ => {}
                }
            }
            Some("response")
                if frame["command"] == "get_state"
                    && frame["success"] == true
                    && frame["data"]["isSettled"] == true =>
            {
                self.terminal = self.yielded
            }
            _ => {}
        }
    }
}

impl RpcPeer {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start(
        stdin: impl AsyncWrite + Send + Unpin + 'static,
        stdout: impl AsyncRead + Send + Unpin + 'static,
        output: impl AsyncWrite + Send + Unpin + 'static,
        agent: OhMyPi,
        scope: super::catalog::CatalogScope,
        prompt: String,
        session_id: Option<String>,
        readiness: Option<WorkflowMcpReadiness>,
        cancel: CancellationToken,
        workflow_plugin: Option<tempfile::TempDir>,
    ) -> (Self, ExecutorExitSignal) {
        let peer = Self {
            stdin: Arc::new(Mutex::new(Box::new(stdin))),
            pending: Default::default(),
            closed: Default::default(),
            _workflow_plugin: Arc::new(workflow_plugin),
        };
        let output: Output = Arc::new(Mutex::new(Box::new(output)));
        let lifecycle = Arc::new(Mutex::new(Lifecycle::default()));
        let (terminal_tx, mut terminal_rx) = watch::channel(None);
        let (exit_tx, exit_rx) = oneshot::channel();
        let reader_peer = peer.clone();
        let reader_output = output.clone();
        let reader_lifecycle = lifecycle.clone();
        tokio::spawn(async move {
            let result = reader_peer
                .read_loop(
                    stdout,
                    reader_output.clone(),
                    reader_lifecycle,
                    &terminal_tx,
                )
                .await;
            if let Err(error) = result {
                let _ = emit(
                    &reader_output,
                    &json!({"type":"error","message":error.to_string()}),
                )
                .await;
            }
            // Wake setup requests on EOF; no fake success from process exit.
            let mut pending = reader_peer.pending.lock().await;
            reader_peer.closed.store(true, Ordering::Release);
            pending.clear();
            drop(pending);
            if terminal_tx.borrow().is_none() {
                let _ = terminal_tx.send(Some(Outcome::Failed));
            }
        });
        let setup_peer = peer.clone();
        tokio::spawn(async move {
            let setup = async {
                if let Some(readiness) = readiness {
                    readiness.wait(&cancel).await?;
                }
                setup_peer
                    .configure(&agent, session_id.as_deref(), &scope, &output)
                    .await?;
                if cancel.is_cancelled() {
                    return Err(failure("Oh My Pi cancelled before prompt"));
                }
                let request =
                    super::command_adapter::session_request(&prompt, session_id.as_deref());
                lifecycle.lock().await.prompt_id = request["id"].as_str().map(str::to_owned);
                // The acknowledgement deadline does not limit agent execution.
                let response = setup_peer.request(request).await?;
                if response["agentInvoked"] == false {
                    let state = setup_peer.request(json!({"type":"get_state"})).await?;
                    emit_configuration(&output, &state).await?;
                }
                Ok::<_, ExecutorError>(())
            };
            let result = tokio::select! {
                result = setup => result,
                _ = cancel.cancelled() => Err(failure("Oh My Pi cancelled before prompt admission")),
            };
            let outcome = match result {
                Err(error) => {
                    let _ = emit(
                        &output,
                        &json!({"type":"error","message":error.to_string()}),
                    )
                    .await;
                    Outcome::Failed
                }
                Ok(()) => loop {
                    if let Some(outcome) = *terminal_rx.borrow() {
                        break outcome;
                    }
                    if terminal_rx.changed().await.is_err() {
                        break Outcome::Failed;
                    }
                },
            };
            // State is read back after native settlement as well as setup, so
            // native slash/model changes are visible without rewriting requests.
            if let Ok(state) = setup_peer.request(json!({"type":"get_state"})).await {
                let _ = emit_configuration(&output, &state).await;
            }
            let status = match outcome {
                Outcome::Succeeded => "completed",
                Outcome::Failed => "failed",
                Outcome::Cancelled => "cancelled",
            };
            let _ = emit(
                &output,
                &json!({"type":"omp_run_completed","status":status}),
            )
            .await;
            let _ = exit_tx.send(if outcome == Outcome::Failed {
                ExecutorExitResult::Failure
            } else {
                ExecutorExitResult::Success
            });
        });
        (peer, exit_rx)
    }

    async fn request(&self, mut request: Value) -> Result<Value, ExecutorError> {
        let id = request["id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        request["id"] = json!(id);
        let command = request["type"].as_str().unwrap_or_default().to_owned();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if self.closed.load(Ordering::Acquire) {
                return Err(failure(format!("Oh My Pi closed before {command}")));
            }
            pending.insert(id.clone(), tx);
        }
        let result = async {
            self.write(&encode_stdio_rpc(&request)?).await?;
            let response = tokio::time::timeout(Duration::from_secs(30), rx)
                .await
                .map_err(|_| failure(format!("Oh My Pi {command} acknowledgement timed out")))?
                .map_err(|_| {
                    failure(format!("Oh My Pi closed before {command} acknowledgement"))
                })?;
            validate_response(&response, &command)?;
            Ok(response["data"].clone())
        }
        .await;
        self.pending.lock().await.remove(&id);
        result
    }

    async fn write(&self, bytes: &[u8]) -> Result<(), ExecutorError> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(bytes).await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn configure(
        &self,
        agent: &OhMyPi,
        session_id: Option<&str>,
        scope: &super::catalog::CatalogScope,
        output: &Output,
    ) -> Result<(), ExecutorError> {
        let initial = self.request(json!({"type":"get_state"})).await?;
        if let Some(expected) = session_id {
            if initial["sessionId"].as_str() != Some(expected)
                && initial["sessionFile"].as_str() != Some(expected)
            {
                return Err(failure("Oh My Pi did not resume the requested session"));
            }
        }
        let catalog = self.request(json!({"type":"get_available_models"})).await?;
        let selected_model = if let Some(requested) = agent.model.as_deref() {
            let selected = select_model(&catalog, requested)?;
            self.request(json!({"type":"set_model","provider":selected["provider"],"modelId":selected["id"]})).await?;
            model_id(selected)
        } else {
            None
        };
        // Read the directory after selecting a model; another model's efforts
        // are never inherited. Unknown commands fail explicitly for overrides.
        let levels = self
            .request(json!({"type":"get_available_thinking_levels"}))
            .await?;
        if let Some(level) = agent.thinking_level.as_deref() {
            validate_thinking(&levels, level)?;
            self.request(json!({"type":"set_thinking_level","level":level}))
                .await?;
        }
        for (command, enabled) in [
            ("set_auto_compaction", agent.auto_compaction),
            ("set_auto_retry", agent.auto_retry),
        ] {
            if let Some(enabled) = enabled {
                self.request(json!({"type":command,"enabled":enabled}))
                    .await?;
            }
        }
        let state = self.request(json!({"type":"get_state"})).await?;
        if selected_model.is_some() && model_id(&state["model"]) != selected_model {
            emit_configuration(output, &state).await?;
            return Err(failure("Oh My Pi did not confirm the selected model"));
        }
        if let Some(level) = agent.thinking_level.as_deref() {
            if state["thinkingLevel"].as_str() != Some(level) {
                emit_configuration(output, &state).await?;
                return Err(failure(format!(
                    "Oh My Pi did not confirm requested thinking level {level}"
                )));
            }
        }
        if let Some(enabled) = agent.auto_compaction {
            if state["autoCompactionEnabled"].as_bool() != Some(enabled) {
                return Err(failure("Oh My Pi did not confirm auto-compaction"));
            }
        }
        if let Some(session_id) = state["sessionId"].as_str() {
            emit(
                output,
                &json!({"type":"session_observed","session_id":session_id}),
            )
            .await?;
        }
        emit_configuration(output, &state).await?;
        let projected = project_catalog(&catalog, &levels, &state);
        emit(output, &json!({"type":super::catalog::CATALOG_EVENT,"working_dir":scope.working_dir,"scope_id":scope.scope_id,
            "model_selector":{"models":projected["models"],"default_model":projected["current_model_id"],
            "providers":[],"agents":[],"permissions":["SUPERVISED"]}})).await
    }

    async fn read_loop(
        &self,
        stdout: impl AsyncRead + Unpin,
        output: Output,
        lifecycle: Arc<Mutex<Lifecycle>>,
        terminal: &watch::Sender<Option<Outcome>>,
    ) -> Result<(), ExecutorError> {
        let mut reader = BufReader::new(stdout);
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            if reader.read_until(b'\n', &mut bytes).await? == 0 {
                return Ok(());
            }
            // Preserve exactly the native bytes, including unknown frames and
            // response errors, before observing them for protocol control.
            {
                let mut output = output.lock().await;
                output.write_all(&bytes).await?;
                output.flush().await?;
            }
            let Ok(frame) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            let mut state = lifecycle.lock().await;
            state.observe(&frame);
            if let Some(outcome) = state.terminal {
                let _ = terminal.send(Some(outcome));
            }
            drop(state);
            if frame["type"] == "response" {
                if let Some(id) = frame["id"].as_str() {
                    if let Some(pending) = self.pending.lock().await.remove(id) {
                        let _ = pending.send(frame);
                    }
                }
            }
        }
    }
}

#[async_trait]
impl ExecutorControl for RpcPeer {
    async fn send(&self, control: DirectControl) -> Result<Vec<u8>, ExecutorError> {
        let bytes = super::command_adapter::encode_control(control)?;
        self.write(&bytes).await?;
        Ok(bytes)
    }
}

fn validate_response(response: &Value, command: &str) -> Result<(), ExecutorError> {
    if response["command"].as_str() != Some(command) || response["success"] != true {
        return Err(failure(format!(
            "Oh My Pi {command} failed: {}",
            response["error"]
                .as_str()
                .unwrap_or("invalid acknowledgement")
        )));
    }
    Ok(())
}

fn select_model<'a>(catalog: &'a Value, requested: &str) -> Result<&'a Value, ExecutorError> {
    let matches: Vec<_> = catalog["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|model| {
            let id = model["id"].as_str().unwrap_or_default();
            let provider = model["provider"].as_str().unwrap_or_default();
            !id.is_empty()
                && !provider.is_empty()
                && (requested == id || requested == format!("{provider}/{id}"))
        })
        .collect();
    match matches.as_slice() {
        [model] => Ok(model),
        _ => Err(failure(format!(
            "Oh My Pi model {requested} is unavailable or ambiguous; select its full provider/model ID"
        ))),
    }
}

fn validate_thinking(levels: &Value, requested: &str) -> Result<(), ExecutorError> {
    if levels["levels"]
        .as_array()
        .is_some_and(|levels| levels.iter().any(|level| level.as_str() == Some(requested)))
    {
        return Ok(());
    }
    Err(failure(format!(
        "Oh My Pi thinking level {requested} is not advertised for the selected model"
    )))
}

fn model_id(model: &Value) -> Option<String> {
    Some(format!(
        "{}/{}",
        model["provider"].as_str()?,
        model["id"].as_str()?
    ))
}

async fn emit_configuration(output: &Output, state: &Value) -> Result<(), ExecutorError> {
    let mut frame = json!({"type":"runtime_configuration","source":"get_state"});
    if let Some(model) = model_id(&state["model"]) {
        frame["model_id"] = json!(model);
    }
    if let Some(level) = state["thinkingLevel"].as_str() {
        frame["reasoning_id"] = json!(level);
    }
    emit(output, &frame).await
}

fn project_catalog(catalog: &Value, levels: &Value, state: &Value) -> Value {
    let current = model_id(&state["model"]);
    let models: Vec<_> = catalog["models"].as_array().into_iter().flatten().filter_map(|model| {
        let id = model_id(model)?;
        let options: Vec<_> = if current.as_deref() == Some(&id) {
            levels["levels"].as_array().into_iter().flatten().filter_map(Value::as_str)
                .map(|level| json!({"id":level,"label":level,"is_default":false})).collect()
        } else { Vec::new() };
            let reasoning = if current.as_deref() == Some(&id) {
                if options.iter().any(|option| option["id"] != "off") { "supported" } else { "unsupported" }
            } else { "unknown" };
            let inputs: Vec<_> = model["input"].as_array().into_iter().flatten().filter_map(Value::as_str)
                .filter(|input| matches!(*input, "text" | "image")).collect();
            Some(json!({"id":id,"name":model["name"].as_str().unwrap_or(&id),"provider_id":model["provider"],"reasoning_options":options,
                "capabilities":{"reasoning":reasoning,"input_modalities":inputs}}))
    }).collect();
    json!({"type":"omp_model_catalog","models":models,"current_model_id":current})
}

/// Provider-specific names must not fall through to generic lifecycle aliases.
pub(crate) fn classify(payload: &Value) -> Option<TypedProviderEvent> {
    let kind = payload["type"].as_str()?;
    let message_id = || payload["messageId"].as_str().map(str::to_owned);
    match kind {
        "prompt_result" if payload["status"] == "error" => Some(TypedProviderEvent::Error(
            AgentRuntimeError::new(
                AgentRuntimeErrorKind::ProtocolFailed,
                payload["error"]["message"]
                    .as_str()
                    .unwrap_or("Oh My Pi prompt failed"),
            )
            .with_provider(Some("oh_my_pi")),
        )),
        "response" if payload["command"] == "prompt" && payload["success"] == false => {
            Some(TypedProviderEvent::Error(
                AgentRuntimeError::new(
                    AgentRuntimeErrorKind::ProtocolFailed,
                    payload["error"]
                        .as_str()
                        .unwrap_or("Oh My Pi rejected prompt"),
                )
                .with_provider(Some("oh_my_pi")),
            ))
        }
        "omp_run_completed" => Some(TypedProviderEvent::Lifecycle(
            match payload["status"].as_str() {
                Some("completed") => AgentRunStatus::Succeeded,
                Some("cancelled") => AgentRunStatus::Cancelled,
                _ => AgentRunStatus::Failed,
            },
        )),
        "agent_start" => Some(TypedProviderEvent::Lifecycle(AgentRunStatus::Running)),
        "message_update" => {
            let event = &payload["assistantMessageEvent"];
            match event["type"].as_str() {
                Some("text_delta") => Some(TypedProviderEvent::Message {
                    role: AgentRuntimeMessageRole::Assistant,
                    content: event["delta"].as_str().unwrap_or_default().to_owned(),
                    final_output: false,
                    message_id: message_id(),
                }),
                Some("thinking_delta") => Some(TypedProviderEvent::Thinking(
                    event["delta"].as_str().unwrap_or_default().to_owned(),
                )),
                _ => None,
            }
        }
        "message_end" if payload["message"]["role"] == "assistant" => {
            Some(TypedProviderEvent::Message {
                role: AgentRuntimeMessageRole::Assistant,
                content: payload["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|part| part["type"] == "text")
                    .filter_map(|part| part["text"].as_str())
                    .collect::<Vec<_>>()
                    .join(""),
                final_output: true,
                message_id: message_id(),
            })
        }
        "tool_execution_start" | "tool_execution_end" | "tool_execution_update" => {
            Some(TypedProviderEvent::ToolCall {
                id: payload["toolCallId"].as_str().map(str::to_owned),
                name: payload["toolName"]
                    .as_str()
                    .unwrap_or("unknown_tool")
                    .to_owned(),
                status: if kind != "tool_execution_end" {
                    AgentRuntimeToolStatus::Running
                } else if payload["isError"] == true {
                    AgentRuntimeToolStatus::Failed
                } else {
                    AgentRuntimeToolStatus::Succeeded
                },
                arguments: payload.get("args").cloned(),
                result: payload
                    .get("result")
                    .or_else(|| payload.get("partialResult"))
                    .cloned(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rpc_setup_waits_for_acknowledgements_and_forwards_native_frames() {
        let (host, native) = tokio::io::duplex(32768);
        let (host_read, host_write) = tokio::io::split(host);
        let (native_read, mut native_write) = tokio::io::split(native);
        let (output, mut audit) = tokio::io::duplex(32768);
        let fixture = tokio::spawn(async move {
            let mut commands = Vec::new();
            let mut reader = BufReader::new(native_read).lines();
            let mut thinking = "off";
            let mut prompted = false;
            while let Some(line) = reader.next_line().await.unwrap() {
                let request: Value = serde_json::from_str(&line).unwrap();
                let command = request["type"].as_str().unwrap();
                commands.push(command.to_owned());
                let data = match command {
                    "get_state" => {
                        json!({"sessionId":"native-session","model":{"provider":"p","id":"m"},
                        "thinkingLevel":thinking,"autoCompactionEnabled":false,"isSettled":true})
                    }
                    "get_available_models" => {
                        json!({"models":[{"provider":"p","id":"m","name":"Model"}]})
                    }
                    "get_available_thinking_levels" => json!({"levels":["off","high"]}),
                    "set_thinking_level" => {
                        thinking = "high";
                        Value::Null
                    }
                    _ => Value::Null,
                };
                native_write.write_all(&encode_stdio_rpc(&json!({"type":"response","id":"unrelated","command":command,"success":false})).unwrap()).await.unwrap();
                native_write.write_all(&encode_stdio_rpc(&json!({"type":"response","id":request["id"],"command":command,"success":true,"data":data})).unwrap()).await.unwrap();
                if command == "prompt" {
                    assert_eq!(
                        commands,
                        vec![
                            "get_state",
                            "get_available_models",
                            "set_model",
                            "get_available_thinking_levels",
                            "set_thinking_level",
                            "set_auto_compaction",
                            "set_auto_retry",
                            "get_state",
                            "prompt"
                        ]
                    );
                    prompted = true;
                    for frame in [
                        json!({"type":"agent_end","yielded":true}),
                        json!({"type":"prompt_result","id":request["id"],"status":"completed","sessionSettled":false}),
                        json!({"type":"agent_start"}),
                        json!({"type":"session_settled"}),
                    ] {
                        native_write
                            .write_all(&encode_stdio_rpc(&frame).unwrap())
                            .await
                            .unwrap();
                    }
                } else if command == "get_state" && prompted {
                    break;
                }
            }
            commands
        });
        let agent = OhMyPi {
            model: Some("p/m".into()),
            thinking_level: Some("high".into()),
            auto_compaction: Some(false),
            auto_retry: Some(false),
            ..Default::default()
        };
        let (_peer, exit) = RpcPeer::start(
            host_write,
            host_read,
            output,
            agent,
            super::super::catalog::CatalogScope {
                working_dir: std::env::current_dir().unwrap(),
                scope_id: "a".repeat(64),
            },
            "hello".into(),
            None,
            None,
            CancellationToken::new(),
            None,
        );
        let drain = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut bytes = Vec::new();
            audit.read_to_end(&mut bytes).await.unwrap();
            bytes
        });
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(3), exit)
                .await
                .unwrap()
                .unwrap(),
            ExecutorExitResult::Success
        ));
        let commands = fixture.await.unwrap();
        assert_eq!(commands.last().unwrap(), "get_state");
        let output = tokio::time::timeout(Duration::from_secs(3), drain)
            .await
            .unwrap()
            .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(
            output.contains("unrelated"),
            "Unmatched native responses must remain audited"
        );
        assert!(output.contains("runtime_configuration"));
        assert!(output.contains("omp_model_catalog"));
        assert!(output.contains("omp_run_completed"));
    }

    #[test]
    fn native_ack_and_yield_wait_for_background_settlement() {
        let mut state = Lifecycle {
            prompt_id: Some("p1".into()),
            ..Default::default()
        };
        for frame in [
            json!({"id":"p1","type":"response","command":"prompt","success":true}),
            json!({"type":"agent_end","isTerminal":true,"yielded":true}),
            json!({"type":"prompt_result","id":"p1","status":"completed","sessionSettled":false}),
            json!({"type":"agent_start"}),
            json!({"type":"agent_end","yielded":true}),
        ] {
            state.observe(&frame);
            assert_eq!(state.terminal, None);
        }
        state.observe(&json!({"type":"session_settled"}));
        assert_eq!(state.terminal, Some(Outcome::Succeeded));
    }

    #[test]
    fn stale_result_and_startup_settlement_do_not_complete_prompt() {
        let mut state = Lifecycle {
            prompt_id: Some("current".into()),
            ..Default::default()
        };
        state.observe(
            &json!({"type":"prompt_result","id":"old","status":"completed","sessionSettled":true}),
        );
        state.observe(&json!({"type":"session_settled"}));
        assert_eq!(state.terminal, None);
        state.observe(&json!({"type":"prompt_result","id":"current","status":"aborted","sessionSettled":true}));
        assert_eq!(state.terminal, Some(Outcome::Cancelled));
    }

    #[test]
    fn background_retry_does_not_poison_successful_settlement() {
        let mut state = Lifecycle {
            prompt_id: Some("current".into()),
            ..Default::default()
        };
        state.observe(&json!({"type":"prompt_result","id":"current","status":"completed","sessionSettled":false}));
        state.observe(&json!({"type":"agent_end","yielded":false,"messages":[{"role":"assistant","stopReason":"error"}]}));
        assert_eq!(state.terminal, None);
        state.observe(&json!({"type":"agent_end","yielded":true,"messages":[{"role":"assistant","stopReason":"stop"}]}));
        state.observe(&json!({"type":"session_settled"}));
        assert_eq!(state.terminal, Some(Outcome::Succeeded));
    }

    #[test]
    fn exhausted_background_failure_survives_settlement() {
        let mut state = Lifecycle {
            prompt_id: Some("current".into()),
            ..Default::default()
        };
        state.observe(&json!({"type":"prompt_result","id":"current","status":"completed","sessionSettled":false}));
        state.observe(&json!({"type":"agent_end","yielded":true,"messages":[{"role":"assistant","stopReason":"error"}]}));
        state.observe(&json!({"type":"session_settled"}));
        assert_eq!(state.terminal, Some(Outcome::Failed));
    }

    #[tokio::test]
    async fn closed_rpc_peer_rejects_without_waiting_for_timeout() {
        let peer = RpcPeer {
            stdin: Arc::new(Mutex::new(Box::new(tokio::io::sink()))),
            pending: Default::default(),
            closed: Arc::new(AtomicBool::new(true)),
            _workflow_plugin: Arc::new(None),
        };
        let error = tokio::time::timeout(
            Duration::from_millis(100),
            peer.request(json!({"type":"get_state"})),
        )
        .await
        .expect("closed peer must fail immediately")
        .unwrap_err();
        assert!(error.to_string().contains("closed before get_state"));
        assert!(peer.pending.lock().await.is_empty());
    }

    #[test]
    fn local_command_requires_state_quiescence_and_errors_are_failures() {
        let mut state = Lifecycle {
            prompt_id: Some("p".into()),
            ..Default::default()
        };
        state.observe(&json!({"id":"p","type":"response","command":"prompt","success":true,"data":{"agentInvoked":false}}));
        assert_eq!(state.terminal, None);
        state.observe(&json!({"type":"response","command":"get_state","success":true,"data":{"isSettled":true}}));
        assert_eq!(state.terminal, Some(Outcome::Succeeded));
        state.observe(
            &json!({"type":"prompt_result","id":"p","status":"error","sessionSettled":true}),
        );
        assert_eq!(state.terminal, Some(Outcome::Failed));
    }

    #[test]
    fn catalog_preserves_model_scope_and_rejects_missing_effort() {
        let catalog = json!({"models":[{"provider":"a","id":"one","name":"One","apiKey":"hidden"},{"provider":"b","id":"two"}]});
        let levels = json!({"levels":["off","high"]});
        let projected = project_catalog(
            &catalog,
            &levels,
            &json!({"model":{"provider":"a","id":"one"}}),
        );
        assert_eq!(
            projected["models"][0]["reasoning_options"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(
            projected["models"][1]["reasoning_options"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(!projected.to_string().contains("hidden"));
        assert!(validate_thinking(&levels, "xhigh").is_err());
        assert!(validate_thinking(&levels, "high").is_ok());
        assert!(select_model(&catalog, "a/one").is_ok());
        assert!(select_model(&catalog, "missing").is_err());
        assert!(
            validate_response(
                &json!({"command":"set_thinking_level","success":false,"error":"invalid"}),
                "set_thinking_level"
            )
            .is_err()
        );
    }

    #[test]
    fn off_only_native_catalog_does_not_claim_reasoning_support() {
        let projected = project_catalog(
            &json!({"models":[{"provider":"p","id":"plain"},{"provider":"p","id":"other"}]}),
            &json!({"levels":["off"]}),
            &json!({"model":{"provider":"p","id":"plain"}}),
        );
        assert_eq!(
            projected["models"][0]["capabilities"]["reasoning"],
            "unsupported"
        );
        assert_eq!(projected["models"][0]["reasoning_options"][0]["id"], "off");
        assert_eq!(
            projected["models"][1]["capabilities"]["reasoning"],
            "unknown"
        );
    }
}
