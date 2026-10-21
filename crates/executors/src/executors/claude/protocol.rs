use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout},
    sync::{Mutex, oneshot},
};
use tokio_util::sync::CancellationToken;

use super::types::{CLIMessage, ControlRequestType, ControlResponseMessage, ControlResponseType};
use crate::{
    approvals::ExecutorApprovalError,
    executors::{
        ExecutorError, ExecutorExitResult,
        claude::{
            client::ClaudeAgentClient,
            types::{Message, PermissionMode, SDKControlRequest, SDKControlRequestType},
        },
    },
};

/// Handles bidirectional control protocol communication
fn encode_interaction_response(
    payload: &serde_json::Value,
    reply: &crate::runtime::ConversationInteractionReply,
) -> Result<ControlResponseMessage, ExecutorError> {
    use crate::executors::provider_adapter::conversation::claude::interaction_request;
    let invalid = |message: &str| {
        ExecutorError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            message.to_owned(),
        ))
    };
    let interaction = interaction_request(payload);
    if reply.request_id != interaction.request_id {
        return Err(invalid(
            "Claude interaction does not match its original request",
        ));
    }
    let request = &payload["request"];
    let mut input = request["input"].clone();
    let response = if interaction.kind == crate::runtime::ConversationInteractionKind::Question {
        let mut answers = serde_json::Map::new();
        for answer in &reply.answers {
            let question = interaction
                .questions
                .iter()
                .find(|question| question.id == answer.question_id)
                .ok_or_else(|| invalid("Unknown Claude question id"))?;
            if answers.contains_key(&question.id)
                || (!question.multiple && answer.values.len() > 1)
                || (question.required && answer.values.is_empty())
            {
                return Err(invalid("Invalid Claude question answer"));
            }
            answers.insert(
                question.prompt.clone(),
                serde_json::Value::String(answer.values.join(", ")),
            );
        }
        if interaction
            .questions
            .iter()
            .any(|question| question.required && !answers.contains_key(&question.prompt))
        {
            return Err(invalid("A required Claude question is unanswered"));
        }
        input
            .as_object_mut()
            .ok_or_else(|| invalid("Claude tool input is not an object"))?
            .insert("answers".into(), serde_json::Value::Object(answers));
        serde_json::json!({"behavior":"allow", "updatedInput":input})
    } else {
        match reply.option_id.as_deref() {
            Some("allow") => serde_json::json!({"behavior":"allow", "updatedInput":input}),
            Some("deny") => {
                serde_json::json!({"behavior":"deny", "message":reply.text.as_deref().unwrap_or("Denied by user"), "interrupt":false})
            }
            _ => return Err(invalid("Claude permission option was not advertised")),
        }
    };
    Ok(ControlResponseMessage::new(ControlResponseType::Success {
        request_id: interaction.request_id,
        response: Some(response),
    }))
}

fn encode_legacy_native_approval(
    payload: &serde_json::Value,
    request_id: String,
    approved: bool,
    reason: Option<String>,
) -> Result<ControlResponseMessage, ExecutorError> {
    use crate::{
        executors::provider_adapter::conversation::claude::interaction_request,
        runtime::{ConversationInteractionKind, ConversationInteractionReply},
    };

    if interaction_request(payload).kind != ConversationInteractionKind::Permission {
        return Err(ExecutorError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Claude question requests require a structured interaction reply with answers",
        )));
    }
    encode_interaction_response(
        payload,
        &ConversationInteractionReply {
            request_id,
            option_id: Some(if approved { "allow" } else { "deny" }.into()),
            answers: Vec::new(),
            text: reason,
        },
    )
}

#[derive(Clone, Debug)]
pub struct ProtocolPeer {
    stdin: Arc<Mutex<ChildStdin>>,
    interrupt_sent: Arc<AtomicBool>,
    startup_failure: Option<StartupFailureSignal>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<serde_json::Value, String>>>>>,
    native_interactions: bool,
    native_pending: Arc<Mutex<HashMap<String, serde_json::Value>>>,
}

/// A failure-only signal retained by the real peer/control lifetime. Successful
/// startup must not drop its sender: the host interprets a closed channel as a
/// crashed executor, not as "initialization finished".
#[derive(Clone, Debug)]
pub(crate) struct StartupFailureSignal(Arc<Mutex<Option<oneshot::Sender<ExecutorExitResult>>>>);

impl StartupFailureSignal {
    pub(crate) fn channel() -> (Self, oneshot::Receiver<ExecutorExitResult>) {
        let (sender, receiver) = oneshot::channel();
        (Self(Arc::new(Mutex::new(Some(sender)))), receiver)
    }

    async fn fail(&self) {
        if let Some(sender) = self.0.lock().await.take() {
            let _ = sender.send(ExecutorExitResult::Failure);
        }
    }
}

#[async_trait::async_trait]
impl crate::executors::ExecutorControl for ProtocolPeer {
    async fn send(
        &self,
        control: crate::executors::provider_adapter::DirectControl,
    ) -> Result<Vec<u8>, ExecutorError> {
        use crate::executors::provider_adapter::DirectControl;

        match control {
            DirectControl::ReplyInteraction { reply } => {
                let response = {
                    let mut pending = self.native_pending.lock().await;
                    let request = pending.get(&reply.request_id).ok_or_else(|| {
                        ExecutorError::Io(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "Claude request is no longer pending",
                        ))
                    })?;
                    let response = encode_interaction_response(request, &reply)?;
                    pending.remove(&reply.request_id);
                    response
                };
                self.send_json_bytes(&response).await
            }
            DirectControl::Cancel => self.send_interrupt_once().await,
            DirectControl::Steer { text } => self.send_user_message_bytes(text).await,
            DirectControl::Approve {
                request_id,
                approved,
                reason,
            } => {
                if self.native_interactions {
                    let response = {
                        let mut pending = self.native_pending.lock().await;
                        let request = pending.get(&request_id).ok_or_else(|| {
                            ExecutorError::Io(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                "Claude request is no longer pending; use a structured interaction reply for the original request",
                            ))
                        })?;
                        let response = encode_legacy_native_approval(
                            request,
                            request_id.clone(),
                            approved,
                            reason,
                        )?;
                        pending.remove(&request_id);
                        response
                    };
                    return self.send_json_bytes(&response).await;
                }
                let response = if approved {
                    serde_json::json!({
                        "behavior": "allow",
                        "updatedInput": {}
                    })
                } else {
                    serde_json::json!({
                        "behavior": "deny",
                        "message": reason.unwrap_or_else(|| "Denied by user".to_string())
                    })
                };
                self.send_json_bytes(&ControlResponseMessage::new(ControlResponseType::Success {
                    request_id,
                    response: Some(response),
                }))
                .await
            }
            DirectControl::Input { .. } => Err(ExecutorError::Io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Claude Code does not expose a separate host input response",
            ))),
        }
    }
}

impl ProtocolPeer {
    pub(crate) fn spawn(
        stdin: ChildStdin,
        stdout: ChildStdout,
        client: Arc<ClaudeAgentClient>,
        cancel: CancellationToken,
        startup_failure: Option<StartupFailureSignal>,
        native_interactions: bool,
    ) -> Self {
        let peer = Self {
            stdin: Arc::new(Mutex::new(stdin)),
            interrupt_sent: Arc::new(AtomicBool::new(false)),
            startup_failure,
            pending: Arc::new(Mutex::new(HashMap::new())),
            native_interactions,
            native_pending: Arc::new(Mutex::new(HashMap::new())),
        };

        let reader_peer = peer.clone();
        tokio::spawn(async move {
            if let Err(e) = reader_peer.read_loop(stdout, client, cancel).await {
                tracing::error!("Protocol reader loop error: {}", e);
            }
        });

        peer
    }

    pub(crate) async fn fail_startup(&self) {
        if let Some(signal) = &self.startup_failure {
            signal.fail().await;
        }
    }

    async fn read_loop(
        &self,
        stdout: ChildStdout,
        client: Arc<ClaudeAgentClient>,
        cancel: CancellationToken,
    ) -> Result<(), ExecutorError> {
        let mut reader = BufReader::new(stdout);
        let mut buffer = String::new();
        let mut cancellation_observed = false;

        loop {
            buffer.clear();
            tokio::select! {
                biased;
                _ = cancel.cancelled(), if !cancellation_observed => {
                    cancellation_observed = true;
                    tracing::info!("Cancellation received in read_loop, sending interrupt to Claude");
                    if let Err(e) = self.send_interrupt_once().await {
                        tracing::warn!("Failed to send interrupt to Claude: {e}");
                    }
                    // Continue the loop to read Claude's response (it should send a result)
                }
                line_result = reader.read_line(&mut buffer) => {
                    match line_result {
                        Ok(0) => break, // EOF
                        Ok(_) => {
                            let line = buffer.trim();
                            if line.is_empty() {
                                continue;
                            }
                            let observed = serde_json::from_str::<serde_json::Value>(line).ok();
                            let native_request = self.native_interactions && observed.as_ref().is_some_and(|value| {
                                value["type"] == "control_request" && value["request"]["subtype"] == "can_use_tool"
                            });
                            if native_request && let Some(value) = &observed
                                && let Some(id) = value["request_id"].as_str()
                            {
                                self.native_pending.lock().await.insert(id.to_owned(), value.clone());
                            }
                            if let Some(value) = &observed
                                && value["type"] == "control_cancel_request"
                                && let Some(id) = value["request_id"].as_str()
                            {
                                self.native_pending.lock().await.remove(id);
                            }
                            client.log_message(line).await?;
                            if native_request {
                                // Keep reading: other tool requests and native
                                // controls may arrive while this one waits.
                                continue;
                            }
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                                if let Some(observation) = runtime_configuration_observation(&value) {
                                    client.log_message(&observation.to_string()).await?;
                                }
                            }

                            // Parse and handle control messages
                            match serde_json::from_str::<CLIMessage>(line) {
                                Ok(CLIMessage::ControlResponse { response }) => {
                                    let (id, result) = match response {
                                        ControlResponseType::Success { request_id, response } =>
                                            (request_id, Ok(response.unwrap_or(serde_json::Value::Null))),
                                        ControlResponseType::Error { request_id, error } =>
                                            (request_id, Err(error.unwrap_or_else(|| "Claude rejected control request".into()))),
                                    };
                                    if let Some(sender) = self.pending.lock().await.remove(&id) {
                                        let _ = sender.send(result);
                                    }
                                }
                                Ok(CLIMessage::ControlRequest {
                                    request_id,
                                    request,
                                }) => {
                                    self.handle_control_request(&client, request_id, request)
                                        .await;
                                }
                                Ok(CLIMessage::Result(_)) => {
                                    break;
                                }
                                _ => {}
                            }
                        }
                        Err(e) => {
                            tracing::error!("Error reading stdout: {}", e);
                            break;
                        }
                    }
                }
            }
        }
        // Wake setup requests immediately on EOF; do not send a prompt after a
        // failed initialize/resume or leave it waiting for a timeout.
        self.pending.lock().await.clear();
        self.native_pending.lock().await.clear();
        Ok(())
    }

    async fn handle_control_request(
        &self,
        client: &Arc<ClaudeAgentClient>,
        request_id: String,
        request: ControlRequestType,
    ) {
        match request {
            ControlRequestType::CanUseTool {
                tool_name,
                input,
                permission_suggestions,
                blocked_paths: _,
                tool_use_id,
            } => {
                match client
                    .on_can_use_tool(tool_name, input, permission_suggestions, tool_use_id)
                    .await
                {
                    Ok(result) => {
                        if let Err(e) = self
                            .send_hook_response(request_id, serde_json::to_value(result).unwrap())
                            .await
                        {
                            tracing::error!("Failed to send permission result: {e}");
                        }
                    }
                    Err(ExecutorError::ExecutorApprovalError(ExecutorApprovalError::Cancelled)) => {
                    }
                    Err(e) => {
                        tracing::error!("Error in on_can_use_tool: {e}");
                        if let Err(e2) = self.send_error(request_id, e.to_string()).await {
                            tracing::error!("Failed to send error response: {e2}");
                        }
                    }
                }
            }
            ControlRequestType::HookCallback {
                callback_id,
                input,
                tool_use_id,
            } => {
                match client
                    .on_hook_callback(callback_id, input, tool_use_id)
                    .await
                {
                    Ok(hook_output) => {
                        if let Err(e) = self.send_hook_response(request_id, hook_output).await {
                            tracing::error!("Failed to send hook callback result: {e}");
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error in on_hook_callback: {e}");
                        if let Err(e2) = self.send_error(request_id, e.to_string()).await {
                            tracing::error!("Failed to send error response: {e2}");
                        }
                    }
                }
            }
        }
    }

    pub async fn send_hook_response(
        &self,
        request_id: String,
        hook_output: serde_json::Value,
    ) -> Result<(), ExecutorError> {
        self.send_json(&ControlResponseMessage::new(ControlResponseType::Success {
            request_id,
            response: Some(hook_output),
        }))
        .await
    }

    /// Send error response to CLI
    async fn send_error(&self, request_id: String, error: String) -> Result<(), ExecutorError> {
        self.send_json(&ControlResponseMessage::new(ControlResponseType::Error {
            request_id,
            error: Some(error),
        }))
        .await
    }

    async fn send_json<T: serde::Serialize>(&self, message: &T) -> Result<(), ExecutorError> {
        self.send_json_bytes(message).await.map(|_| ())
    }

    async fn send_json_bytes<T: serde::Serialize>(
        &self,
        message: &T,
    ) -> Result<Vec<u8>, ExecutorError> {
        let json = serde_json::to_string(message)?;
        let mut bytes = json.into_bytes();
        bytes.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&bytes).await?;
        stdin.flush().await?;
        Ok(bytes)
    }

    async fn send_interrupt_once(&self) -> Result<Vec<u8>, ExecutorError> {
        if self
            .interrupt_sent
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(Vec::new());
        }

        let result = self
            .send_json_bytes(&SDKControlRequest::new(SDKControlRequestType::Interrupt {}))
            .await;
        if result.is_err() {
            self.interrupt_sent.store(false, Ordering::Release);
        }
        result
    }

    pub async fn send_user_message(&self, content: String) -> Result<(), ExecutorError> {
        self.send_user_message_bytes(content).await.map(|_| ())
    }

    async fn send_user_message_bytes(&self, content: String) -> Result<Vec<u8>, ExecutorError> {
        let message = Message::new_user(content);
        self.send_json_bytes(&message).await
    }

    async fn request(
        &self,
        request: SDKControlRequestType,
    ) -> Result<serde_json::Value, ExecutorError> {
        let request = SDKControlRequest::new(request);
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .await
            .insert(request.request_id.clone(), sender);
        let result = async {
            self.send_json(&request).await?;
            tokio::time::timeout(Duration::from_secs(60), receiver)
                .await
                .map_err(|_| {
                    ExecutorError::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Claude control request timed out",
                    ))
                })?
                .map_err(|_| {
                    ExecutorError::Io(std::io::Error::other(
                        "Claude exited before confirming configuration",
                    ))
                })?
                .map_err(|error| ExecutorError::Io(std::io::Error::other(error)))
        }
        .await;
        self.pending.lock().await.remove(&request.request_id);
        result
    }

    pub async fn initialize(
        &self,
        hooks: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, ExecutorError> {
        self.request(SDKControlRequestType::Initialize { hooks })
            .await
    }

    pub async fn apply_flag_settings(
        &self,
        settings: serde_json::Value,
    ) -> Result<(), ExecutorError> {
        self.request(SDKControlRequestType::ApplyFlagSettings { settings })
            .await
            .map(|_| ())
    }
    pub async fn interrupt(&self) -> Result<(), ExecutorError> {
        self.send_interrupt_once().await.map(|_| ())
    }

    pub async fn set_permission_mode(&self, mode: PermissionMode) -> Result<(), ExecutorError> {
        self.request(SDKControlRequestType::SetPermissionMode { mode })
            .await
            .map(|_| ())
    }
}

fn runtime_configuration_observation(value: &serde_json::Value) -> Option<serde_json::Value> {
    // Exclude subagent traffic and multi-model usage totals: neither identifies
    // the main conversation's actual model.
    if value
        .get("parent_tool_use_id")
        .is_some_and(|id| !id.is_null())
    {
        return None;
    }
    let (model, source) = match value["type"].as_str()? {
        "system" if value["subtype"] == "init" => (value["model"].as_str()?, "claude.system/init"),
        "assistant" => (value["message"]["model"].as_str()?, "claude.assistant"),
        _ => return None,
    };
    Some(serde_json::json!({"type":"runtime_configuration", "model_id":model, "source":source}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_responses_preserve_tool_input_and_question_answers() {
        use crate::runtime::{ConversationInteractionReply, ConversationQuestionAnswer};
        let payload = serde_json::json!({"type":"control_request","request_id":"native-1", "request":{
            "subtype":"can_use_tool", "tool_name":"Bash", "input":{"command":"pwd", "timeout":123}, "tool_use_id":"tool-1"
        }});
        let reply = ConversationInteractionReply {
            request_id: "native-1".into(),
            option_id: Some("allow".into()),
            answers: vec![],
            text: None,
        };
        let response =
            serde_json::to_value(encode_interaction_response(&payload, &reply).unwrap()).unwrap();
        assert_eq!(response["response"]["request_id"], "native-1");
        assert_eq!(
            response["response"]["response"]["updatedInput"],
            payload["request"]["input"]
        );
        let legacy = serde_json::to_value(
            encode_legacy_native_approval(&payload, "native-1".into(), true, None).unwrap(),
        )
        .unwrap();
        assert_eq!(legacy, response);
        assert!(
            encode_legacy_native_approval(&payload, "other-request".into(), true, None).is_err()
        );
        let question = serde_json::json!({"type":"control_request","request_id":"q-1", "request":{
            "subtype":"can_use_tool", "tool_name":"AskUserQuestion", "input":{"questions":[{"question":"Which?", "options":[{"label":"A"}], "multiSelect":true}]}
        }});
        let reply = ConversationInteractionReply {
            request_id: "q-1".into(),
            option_id: None,
            answers: vec![ConversationQuestionAnswer {
                question_id: "Which?".into(),
                values: vec!["A".into(), "custom choice".into()],
            }],
            text: None,
        };
        let response =
            serde_json::to_value(encode_interaction_response(&question, &reply).unwrap()).unwrap();
        assert_eq!(
            response["response"]["response"]["updatedInput"]["answers"]["Which?"],
            "A, custom choice"
        );
        assert_eq!(
            response["response"]["response"]["updatedInput"]["questions"],
            question["request"]["input"]["questions"]
        );
        assert!(encode_legacy_native_approval(&question, "q-1".into(), true, None).is_err());
        let mut invalid = reply;
        invalid.request_id = "other-request".into();
        assert!(encode_interaction_response(&question, &invalid).is_err());
    }

    #[test]
    fn observations_only_report_main_model_and_never_guess_effort() {
        let observed = runtime_configuration_observation(&serde_json::json!({"type":"system", "subtype":"init", "model":"resolved-model", "effort":"high"})).unwrap();
        assert_eq!(observed["model_id"], "resolved-model");
        assert!(observed.get("reasoning_id").is_none());
        assert!(runtime_configuration_observation(&serde_json::json!({"type":"assistant", "parent_tool_use_id":"task-1", "message":{"model":"subagent-model"}})).is_none());
        assert!(
            runtime_configuration_observation(
                &serde_json::json!({"type":"result", "modelUsage":{"model-a":{},"model-b":{}}})
            )
            .is_none()
        );
    }

    #[test]
    fn native_control_fixtures_preserve_request_id_and_flag_settings() {
        let request = SDKControlRequest::new(SDKControlRequestType::ApplyFlagSettings {
            settings: serde_json::json!({"fastMode":false,"ultracode":true}),
        });
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["request"]["subtype"], "apply_flag_settings");
        assert_eq!(value["request"]["settings"]["ultracode"], true);
        assert!(value["request"]["settings"].get("effortLevel").is_none());
        let response: CLIMessage = serde_json::from_value(serde_json::json!({"type":"control_response","response":{"subtype":"error","request_id":request.request_id,"error":"Unsupported setting"}})).unwrap();
        assert!(matches!(
            response,
            CLIMessage::ControlResponse {
                response: ControlResponseType::Error { .. }
            }
        ));
    }

    #[tokio::test]
    async fn workflow_claude_successful_startup_keeps_exit_signal_pending_for_peer_lifetime() {
        let (peer_signal, mut receiver) = StartupFailureSignal::channel();
        let startup_signal = peer_signal.clone();
        tokio::spawn(async move {
            drop(startup_signal);
        })
        .await
        .unwrap();
        assert!(matches!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        drop(peer_signal);
        assert!(matches!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
    }

    #[tokio::test]
    async fn workflow_claude_startup_failure_is_signalled_exactly_once() {
        let (signal, receiver) = StartupFailureSignal::channel();
        signal.fail().await;
        signal.clone().fail().await;
        assert!(matches!(
            receiver.await.unwrap(),
            ExecutorExitResult::Failure
        ));
    }
}
