//! Cancellation through the active ACP peer, acknowledged by successful
//! native stdin delivery. No fabricated session id or duplicate stdin owner.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::executors::{ExecutorControl, ExecutorError, provider_adapter::DirectControl};

type CancelAcknowledgement = oneshot::Sender<Vec<u8>>;
pub(super) const APPROVAL_META_KEY: &str = "_vibe_kanban_approval_id";

#[derive(Debug)]
pub(super) struct NativePermissionDecision {
    pub outcome: agent_client_protocol::RequestPermissionOutcome,
    pub approved: bool,
    pub reason: Option<String>,
}

#[derive(Debug)]
struct PendingPermission {
    allow: Option<agent_client_protocol::PermissionOptionId>,
    reject: Option<agent_client_protocol::PermissionOptionId>,
    options: HashMap<String, (agent_client_protocol::PermissionOptionId, bool)>,
    decision: oneshot::Sender<NativePermissionDecision>,
}

#[derive(Debug)]
pub(super) struct AcpControl {
    cancel: CancellationToken,
    active_session: Mutex<Option<String>>,
    pending: Mutex<Vec<CancelAcknowledgement>>,
    pending_permissions: Mutex<HashMap<String, PendingPermission>>,
    permission_acks: Mutex<HashMap<String, CancelAcknowledgement>>,
    closed: watch::Sender<Option<bool>>,
}

impl AcpControl {
    pub fn new(cancel: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            cancel,
            active_session: Mutex::new(None),
            pending: Mutex::new(Vec::new()),
            pending_permissions: Mutex::new(HashMap::new()),
            permission_acks: Mutex::new(HashMap::new()),
            closed: watch::channel(None).0,
        })
    }

    pub fn set_session(&self, session_id: String) {
        if let Ok(mut session) = self.active_session.lock() {
            *session = Some(session_id);
        }
    }

    pub fn mark_closed(&self, acknowledged: bool) {
        self.closed.send_replace(Some(acknowledged));
    }

    pub fn register_permission(
        &self,
        options: &[agent_client_protocol::PermissionOption],
    ) -> Result<(String, oneshot::Receiver<NativePermissionDecision>), ExecutorError> {
        use agent_client_protocol::PermissionOptionKind;
        let allow = options
            .iter()
            .find(|option| option.kind == PermissionOptionKind::AllowOnce)
            .or_else(|| {
                options
                    .iter()
                    .find(|option| option.kind == PermissionOptionKind::AllowAlways)
            })
            .map(|option| option.option_id.clone());
        let reject = options
            .iter()
            .find(|option| option.kind == PermissionOptionKind::RejectOnce)
            .or_else(|| {
                options
                    .iter()
                    .find(|option| option.kind == PermissionOptionKind::RejectAlways)
            })
            .map(|option| option.option_id.clone());
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending_permissions
            .lock()
            .map_err(|_| unavailable())?
            .insert(
                id.clone(),
                PendingPermission {
                    allow,
                    reject,
                    options: options
                        .iter()
                        .map(|option| {
                            (
                                option.option_id.0.to_string(),
                                (
                                    option.option_id.clone(),
                                    matches!(
                                        option.kind,
                                        PermissionOptionKind::AllowOnce
                                            | PermissionOptionKind::AllowAlways
                                    ),
                                ),
                            )
                        })
                        .collect(),
                    decision: tx,
                },
            );
        Ok((id, rx))
    }

    pub fn abandon_permission(&self, id: &str) {
        if let Ok(mut pending) = self.pending_permissions.lock() {
            pending.remove(id);
        }
        if let Ok(mut acknowledgements) = self.permission_acks.lock() {
            acknowledgements.remove(id);
        }
    }

    async fn deliver_permission(
        &self,
        request_id: String,
        selected: Option<agent_client_protocol::PermissionOptionId>,
        approved: bool,
        reason: Option<String>,
    ) -> Result<Vec<u8>, ExecutorError> {
        use agent_client_protocol::{RequestPermissionOutcome, SelectedPermissionOutcome};
        let (tx, rx) = oneshot::channel();
        {
            let permission = self
                .pending_permissions
                .lock()
                .map_err(|_| unavailable())?
                .remove(&request_id)
                .ok_or_else(|| {
                    ExecutorError::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "ACP approval is no longer pending",
                    ))
                })?;
            let outcome = selected
                .map(|id| RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)))
                .unwrap_or(RequestPermissionOutcome::Cancelled);
            self.permission_acks
                .lock()
                .map_err(|_| unavailable())?
                .insert(request_id.clone(), tx);
            if permission
                .decision
                .send(NativePermissionDecision {
                    outcome,
                    approved,
                    reason,
                })
                .is_err()
            {
                self.permission_acks
                    .lock()
                    .map_err(|_| unavailable())?
                    .remove(&request_id);
                return Err(ExecutorError::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "ACP permission request closed",
                )));
            }
        }
        let result = await_delivery(rx).await;
        self.abandon_permission(&request_id);
        result
    }

    /// Called only after the writer has flushed this exact frame to the child.
    pub fn acknowledge_written(&self, bytes: &[u8]) {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
            return;
        };
        if let Some(id) = value["result"]["_meta"][APPROVAL_META_KEY].as_str() {
            if let Ok(mut acknowledgements) = self.permission_acks.lock()
                && let Some(acknowledgement) = acknowledgements.remove(id)
            {
                let _ = acknowledgement.send(bytes.to_vec());
            }
            return;
        }
        if value["method"] != "session/cancel" {
            return;
        }
        let session = self
            .active_session
            .lock()
            .ok()
            .and_then(|session| session.clone());
        if value["params"]["sessionId"].as_str() != session.as_deref() {
            return;
        }
        if let Ok(mut pending) = self.pending.lock() {
            for acknowledgement in pending.drain(..) {
                let _ = acknowledgement.send(bytes.to_vec());
            }
        }
    }
}

fn unavailable() -> ExecutorError {
    ExecutorError::Io(std::io::Error::other("ACP control state unavailable"))
}

async fn await_delivery(rx: oneshot::Receiver<Vec<u8>>) -> Result<Vec<u8>, ExecutorError> {
    tokio::time::timeout(std::time::Duration::from_secs(5), rx)
        .await
        .map_err(|_| {
            ExecutorError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "ACP control delivery was not confirmed",
            ))
        })?
        .map_err(|_| {
            ExecutorError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "ACP control channel closed",
            ))
        })
}

#[async_trait]
impl ExecutorControl for AcpControl {
    async fn send(&self, control: DirectControl) -> Result<Vec<u8>, ExecutorError> {
        if let DirectControl::ReplyInteraction { reply } = &control {
            let (selected, approved) = {
                let pending = self.pending_permissions.lock().map_err(|_| unavailable())?;
                let permission = pending.get(&reply.request_id).ok_or_else(|| {
                    ExecutorError::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "ACP permission is no longer pending",
                    ))
                })?;
                reply
                    .option_id
                    .as_deref()
                    .and_then(|id| permission.options.get(id))
                    .cloned()
                    .ok_or_else(|| {
                        ExecutorError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "ACP permission option was not advertised",
                        ))
                    })?
            };
            return self
                .deliver_permission(
                    reply.request_id.clone(),
                    Some(selected),
                    approved,
                    reply.text.clone(),
                )
                .await;
        }
        if let DirectControl::Approve {
            request_id,
            approved,
            reason,
        } = control
        {
            let selected = {
                let pending = self.pending_permissions.lock().map_err(|_| unavailable())?;
                let permission = pending.get(&request_id).ok_or_else(|| {
                    ExecutorError::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "ACP approval is no longer pending",
                    ))
                })?;
                if approved && permission.allow.is_none() {
                    return Err(ExecutorError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "The provider did not advertise an allow option",
                    )));
                }
                if approved {
                    permission.allow.clone()
                } else {
                    permission.reject.clone()
                }
            };
            return self
                .deliver_permission(request_id, selected, approved, reason)
                .await;
        }
        if !matches!(control, DirectControl::Cancel) {
            return Err(ExecutorError::Io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "ACP controls require the provider's active permission service; steering is unsupported",
            )));
        }
        let active = self
            .active_session
            .lock()
            .ok()
            .is_some_and(|session| session.is_some());
        if !active {
            return Err(ExecutorError::Io(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "ACP has not established a native session",
            )));
        }
        let (tx, rx) = oneshot::channel();
        let mut closed = self.closed.subscribe();
        self.pending
            .lock()
            .map_err(|_| ExecutorError::Io(std::io::Error::other("ACP control state unavailable")))?
            .push(tx);
        self.cancel.cancel();
        let delivered = await_delivery(rx).await?;
        // The host kills the process group after this method returns. Wait
        // for the native close and normalized log flush first, with a bounded
        // grace period; the host still proves process termination afterwards.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(6), async {
            loop {
                if closed.borrow_and_update().is_some() {
                    break;
                }
                if closed.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
        Ok(delivered)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::runtime::ConversationInteractionReply;

    #[tokio::test]
    async fn exact_advertised_option_waits_for_native_writer_flush_and_consumes_once() {
        let control = AcpControl::new(CancellationToken::new());
        let options: Vec<agent_client_protocol::PermissionOption> = serde_json::from_value(json!([
            {"optionId":"allow-only-this-time", "name":"Allow once", "kind":"allow_once"},
            {"optionId":"always-in-this-scope", "name":"Allow always", "kind":"allow_always"},
            {"optionId":"no", "name":"Reject", "kind":"reject_once"}
        ]))
        .unwrap();
        let (id, decision) = control.register_permission(&options).unwrap();
        let reply = |option: &str| DirectControl::ReplyInteraction {
            reply: ConversationInteractionReply {
                request_id: id.clone(),
                option_id: Some(option.into()),
                answers: vec![],
                text: None,
            },
        };
        assert!(control.send(reply("invented-id")).await.is_err());
        let sender = control.clone();
        let command = reply("always-in-this-scope");
        let delivery = tokio::spawn(async move { sender.send(command).await });
        let selected = decision.await.unwrap();
        assert!(selected.approved);
        assert_eq!(
            serde_json::to_value(selected.outcome).unwrap()["optionId"],
            "always-in-this-scope"
        );
        assert!(
            !delivery.is_finished(),
            "callback resolution is not native delivery"
        );
        let bytes = serde_json::to_vec(&json!({"id":"real-native-id", "result":{
            "outcome":{"outcome":"selected", "optionId":"always-in-this-scope"},
            "_meta":{"_vibe_kanban_approval_id":id},
        }}))
        .unwrap();
        control.acknowledge_written(&bytes);
        assert_eq!(delivery.await.unwrap().unwrap(), bytes);
        assert!(control.send(reply("allow-only-this-time")).await.is_err());
    }
}
