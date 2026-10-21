use std::time::Duration;

use axum::{
    Router,
    extract::{Path, Query, State, ws::Message},
    response::{IntoResponse, Json as ResponseJson},
    routing::{get, post},
};
use deployment::Deployment;
use executors::runtime::{
    AgentEventEnvelope, AgentRunPortCommand, AgentRunPortCommandEnvelope, AgentRunPortError,
    ConversationInteractionReply, ORCHESTRATION_COMMAND_SCHEMA_VERSION, RunAttemptMode, RunState,
};
use serde::{Deserialize, Serialize};
use services::services::agent_runtime::{
    AgentEventCursor, AgentRunCommandError, AgentRunCommandService, AgentRunHistoryPage,
    AgentRunStats, AgentRunSummary, AgentRuntimeReadError, AgentRuntimeReadService,
};
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{
    DeploymentImpl,
    error::ApiError,
    middleware::signed_ws::{MaybeSignedWebSocket, SignedWsUpgrade},
};

// Repair only (e.g. an out-of-process writer); normal delivery is post-commit.
const STREAM_REPAIR_INTERVAL: Duration = Duration::from_secs(30);
const STREAM_PAGE_SIZE: u32 = 500;

#[derive(Debug, Clone, Deserialize, TS)]
pub struct AgentRunControlIdentity {
    pub command_id: Uuid,
    pub idempotency_key: String,
    pub correlation_id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct CancelAgentRunRequest {
    #[serde(flatten)]
    #[ts(flatten)]
    pub identity: AgentRunControlIdentity,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct SubmitAgentRunInputRequest {
    #[serde(flatten)]
    #[ts(flatten)]
    pub identity: AgentRunControlIdentity,
    pub input_id: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct ResolveAgentRunApprovalRequest {
    #[serde(flatten)]
    #[ts(flatten)]
    pub identity: AgentRunControlIdentity,
    pub approval_id: String,
    pub approved: bool,
    #[serde(default)]
    #[ts(optional)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct ReplyAgentRunInteractionRequest {
    #[serde(flatten)]
    #[ts(flatten)]
    pub identity: AgentRunControlIdentity,
    pub run_attempt_id: Uuid,
    pub reply_id: Uuid,
    pub reply: ConversationInteractionReply,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct RetryAgentRunRequest {
    #[serde(flatten)]
    #[ts(flatten)]
    pub identity: AgentRunControlIdentity,
    pub mode: RunAttemptMode,
    pub run_attempt_id: Uuid,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct AgentEventQuery {
    #[serde(default)]
    pub after_attempt_number: Option<u32>,
    #[serde(default)]
    pub after_sequence: Option<u64>,
    #[serde(default)]
    pub limit: Option<u32>,
}

impl AgentEventQuery {
    fn cursor(self) -> Result<Option<AgentEventCursor>, ApiError> {
        match (self.after_attempt_number, self.after_sequence) {
            (Some(run_attempt_number), Some(sequence)) => Ok(Some(AgentEventCursor {
                run_attempt_number,
                sequence,
            })),
            (None, None) => Ok(None),
            _ => Err(ApiError::BadRequest(
                "after_attempt_number and after_sequence must be supplied together".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AgentRunStreamMessage {
    Event {
        event: AgentEventEnvelope,
        replay: bool,
    },
    Ready {
        state: RunState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<AgentEventCursor>,
    },
    State {
        state: RunState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<AgentEventCursor>,
    },
    Error {
        message: String,
    },
}

async fn list_agent_runs_for_session(
    State(deployment): State<DeploymentImpl>,
    Path(session_id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<Vec<AgentRunSummary>>>, ApiError> {
    let reader = AgentRuntimeReadService::new(&deployment.db().pool);
    let runs = reader
        .list_for_session(session_id)
        .await
        .map_err(read_api_error)?;
    Ok(ResponseJson(ApiResponse::success(runs)))
}

async fn get_agent_run(
    State(deployment): State<DeploymentImpl>,
    Path(agent_run_id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<RunState>>, ApiError> {
    let reader = AgentRuntimeReadService::new(&deployment.db().pool);
    let state = reader.state(agent_run_id).await.map_err(read_api_error)?;
    Ok(ResponseJson(ApiResponse::success(state)))
}

async fn get_agent_run_events(
    State(deployment): State<DeploymentImpl>,
    Path(agent_run_id): Path<Uuid>,
    Query(query): Query<AgentEventQuery>,
) -> Result<ResponseJson<ApiResponse<AgentRunHistoryPage>>, ApiError> {
    let reader = AgentRuntimeReadService::new(&deployment.db().pool);
    let page = reader
        .history_page(agent_run_id, query.cursor()?, query.limit.unwrap_or(500))
        .await
        .map_err(read_api_error)?;
    Ok(ResponseJson(ApiResponse::success(page)))
}

async fn get_agent_run_stats(
    State(deployment): State<DeploymentImpl>,
    Path(agent_run_id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<AgentRunStats>>, ApiError> {
    let reader = AgentRuntimeReadService::new(&deployment.db().pool);
    let stats = reader.stats(agent_run_id).await.map_err(read_api_error)?;
    Ok(ResponseJson(ApiResponse::success(stats)))
}

async fn cancel_agent_run(
    State(deployment): State<DeploymentImpl>,
    Path(agent_run_id): Path<Uuid>,
    axum::Json(request): axum::Json<CancelAgentRunRequest>,
) -> Result<ResponseJson<ApiResponse<RunState>>, ApiError> {
    dispatch_control(
        &deployment,
        agent_run_id,
        request.identity,
        AgentRunPortCommand::Cancel {
            reason: request.reason,
        },
    )
    .await
}

async fn submit_agent_run_input(
    State(deployment): State<DeploymentImpl>,
    Path(agent_run_id): Path<Uuid>,
    axum::Json(request): axum::Json<SubmitAgentRunInputRequest>,
) -> Result<ResponseJson<ApiResponse<RunState>>, ApiError> {
    dispatch_control(
        &deployment,
        agent_run_id,
        request.identity,
        AgentRunPortCommand::SubmitInput {
            input_id: request.input_id,
            content: request.content,
        },
    )
    .await
}

async fn resolve_agent_run_approval(
    State(deployment): State<DeploymentImpl>,
    Path(agent_run_id): Path<Uuid>,
    axum::Json(request): axum::Json<ResolveAgentRunApprovalRequest>,
) -> Result<ResponseJson<ApiResponse<RunState>>, ApiError> {
    dispatch_control(
        &deployment,
        agent_run_id,
        request.identity,
        AgentRunPortCommand::ResolveApproval {
            approval_id: request.approval_id,
            approved: request.approved,
            reason: request.reason,
        },
    )
    .await
}

async fn retry_agent_run(
    State(deployment): State<DeploymentImpl>,
    Path(agent_run_id): Path<Uuid>,
    axum::Json(request): axum::Json<RetryAgentRunRequest>,
) -> Result<ResponseJson<ApiResponse<RunState>>, ApiError> {
    dispatch_control(
        &deployment,
        agent_run_id,
        request.identity,
        AgentRunPortCommand::Retry {
            mode: request.mode,
            run_attempt_id: request.run_attempt_id,
        },
    )
    .await
}

async fn reply_agent_run_interaction(
    State(deployment): State<DeploymentImpl>,
    Path(agent_run_id): Path<Uuid>,
    axum::Json(request): axum::Json<ReplyAgentRunInteractionRequest>,
) -> Result<ResponseJson<ApiResponse<RunState>>, ApiError> {
    dispatch_control(
        &deployment,
        agent_run_id,
        request.identity,
        AgentRunPortCommand::ReplyInteraction {
            run_attempt_id: request.run_attempt_id,
            reply_id: request.reply_id,
            reply: request.reply,
        },
    )
    .await
}

async fn dispatch_control(
    deployment: &DeploymentImpl,
    agent_run_id: Uuid,
    identity: AgentRunControlIdentity,
    command: AgentRunPortCommand,
) -> Result<ResponseJson<ApiResponse<RunState>>, ApiError> {
    let envelope = AgentRunPortCommandEnvelope {
        schema_version: ORCHESTRATION_COMMAND_SCHEMA_VERSION,
        command_id: identity.command_id,
        idempotency_key: identity.idempotency_key,
        agent_run_id,
        orchestration_run_id: None,
        orchestration_node_execution_id: None,
        correlation_id: identity.correlation_id,
        created_at: identity.created_at,
        command,
    };
    AgentRunCommandService::new(&deployment.db().pool, deployment.agent_run_port())
        .dispatch(envelope)
        .await
        .map_err(command_api_error)?;
    let state = AgentRuntimeReadService::new(&deployment.db().pool)
        .state(agent_run_id)
        .await
        .map_err(read_api_error)?;
    Ok(ResponseJson(ApiResponse::success(state)))
}

async fn stream_agent_run_events_ws(
    ws: SignedWsUpgrade,
    State(deployment): State<DeploymentImpl>,
    Path(agent_run_id): Path<Uuid>,
    Query(query): Query<AgentEventQuery>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(error) =
            handle_agent_run_events_ws(socket, deployment, agent_run_id, query).await
        {
            tracing::warn!(%agent_run_id, %error, "canonical AgentRun WebSocket closed");
        }
    })
}

async fn handle_agent_run_events_ws(
    mut socket: MaybeSignedWebSocket,
    deployment: DeploymentImpl,
    agent_run_id: Uuid,
    query: AgentEventQuery,
) -> anyhow::Result<()> {
    let reader = AgentRuntimeReadService::new(&deployment.db().pool);
    let mut cursor = match query.cursor() {
        Ok(cursor) => cursor,
        Err(error) => {
            send_stream_message(
                &mut socket,
                &AgentRunStreamMessage::Error {
                    message: error.to_string(),
                },
            )
            .await?;
            socket.close().await?;
            return Ok(());
        }
    };

    let mut changes = db::models::agent_runtime::subscribe_agent_event_changes();
    let mut last_state = loop {
        let page = match reader
            .history_page(agent_run_id, cursor, STREAM_PAGE_SIZE)
            .await
        {
            Ok(page) => page,
            Err(error) => {
                send_stream_message(
                    &mut socket,
                    &AgentRunStreamMessage::Error {
                        message: error.to_string(),
                    },
                )
                .await?;
                socket.close().await?;
                return Ok(());
            }
        };
        for event in page.events {
            cursor = Some(AgentEventCursor {
                run_attempt_number: event.run_attempt_number,
                sequence: event.sequence,
            });
            send_stream_message(
                &mut socket,
                &AgentRunStreamMessage::Event {
                    event,
                    replay: true,
                },
            )
            .await?;
        }
        if !page.has_more {
            send_stream_message(
                &mut socket,
                &AgentRunStreamMessage::Ready {
                    state: page.state.clone(),
                    cursor,
                },
            )
            .await?;
            break page.state;
        }
    };

    let mut interval = tokio::time::interval(STREAM_REPAIR_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = async {
                loop {
                    tokio::select! {
                        _ = interval.tick() => break,
                        change = changes.recv() => match change {
                            Ok(id) if id != agent_run_id => continue,
                            _ => break,
                        },
                    }
                }
            } => {
              loop {
                let page = reader
                    .history_page(agent_run_id, cursor, STREAM_PAGE_SIZE)
                    .await?;
                let had_events = !page.events.is_empty();
                for event in page.events {
                    cursor = Some(AgentEventCursor {
                        run_attempt_number: event.run_attempt_number,
                        sequence: event.sequence,
                    });
                    send_stream_message(
                        &mut socket,
                        &AgentRunStreamMessage::Event {
                            event,
                            replay: false,
                        },
                    )
                    .await?;
                }
                // State and events may have been read on opposite sides of a
                // commit. Even an empty wakeup must repair a stale State.
                if should_send_stream_state(had_events, page.has_more, page.state != last_state) {
                    last_state = page.state.clone();
                    send_stream_message(
                        &mut socket,
                        &AgentRunStreamMessage::State {
                            state: page.state,
                            cursor,
                        },
                    )
                    .await?;
                }
                if !page.has_more { break; }
              }
            }
            inbound = socket.recv() => {
                match inbound {
                    Ok(Some(Message::Close(_))) | Ok(None) | Err(_) => break,
                    Ok(Some(_)) => {}
                }
            }
        }
    }
    let _ = socket.close().await;
    Ok(())
}

fn should_send_stream_state(had_events: bool, has_more: bool, state_changed: bool) -> bool {
    !has_more && (had_events || state_changed)
}

async fn send_stream_message(
    socket: &mut MaybeSignedWebSocket,
    message: &AgentRunStreamMessage,
) -> anyhow::Result<()> {
    tokio::time::timeout(
        Duration::from_secs(10),
        socket.send(Message::Text(serde_json::to_string(message)?.into())),
    )
    .await??;
    Ok(())
}

fn read_api_error(error: AgentRuntimeReadError) -> ApiError {
    match error {
        AgentRuntimeReadError::Database(error) => ApiError::Database(error),
        AgentRuntimeReadError::NotFound(agent_run_id) => {
            ApiError::BadRequest(format!("AgentRun {agent_run_id} was not found"))
        }
        other => ApiError::BadRequest(other.to_string()),
    }
}

fn command_api_error(error: AgentRunCommandError) -> ApiError {
    match error {
        AgentRunCommandError::Persistence(
            db::models::agent_runtime::AgentRuntimePersistenceError::Database(error),
        ) => ApiError::Database(error),
        AgentRunCommandError::Port(AgentRunPortError::NotFound(agent_run_id)) => {
            ApiError::BadRequest(format!("AgentRun {agent_run_id} was not found"))
        }
        AgentRunCommandError::Port(AgentRunPortError::Unavailable(message)) => {
            ApiError::BadGateway(message)
        }
        AgentRunCommandError::Port(AgentRunPortError::Rejected(message)) => {
            ApiError::BadRequest(message)
        }
        AgentRunCommandError::DeliveryInProgress(command_id) => ApiError::Conflict(format!(
            "AgentRun command {command_id} is already being delivered"
        )),
        other => ApiError::BadRequest(other.to_string()),
    }
}

pub(super) fn router(_: &DeploymentImpl) -> Router<DeploymentImpl> {
    Router::new()
        .route(
            "/agent-runs/session/{session_id}",
            get(list_agent_runs_for_session),
        )
        .route("/agent-runs/{agent_run_id}", get(get_agent_run))
        .route("/agent-runs/{agent_run_id}/cancel", post(cancel_agent_run))
        .route(
            "/agent-runs/{agent_run_id}/input",
            post(submit_agent_run_input),
        )
        .route(
            "/agent-runs/{agent_run_id}/approval",
            post(resolve_agent_run_approval),
        )
        .route(
            "/agent-runs/{agent_run_id}/interaction",
            post(reply_agent_run_interaction),
        )
        .route("/agent-runs/{agent_run_id}/retry", post(retry_agent_run))
        .route(
            "/agent-runs/{agent_run_id}/events",
            get(get_agent_run_events),
        )
        .route(
            "/agent-runs/{agent_run_id}/events/ws",
            get(stream_agent_run_events_ws),
        )
        .route("/agent-runs/{agent_run_id}/stats", get(get_agent_run_stats))
}

#[cfg(test)]
mod tests {
    use super::AgentEventQuery;

    #[test]
    fn empty_page_repairs_racing_terminal_state_but_never_precedes_remaining_text() {
        assert!(super::should_send_stream_state(false, false, true));
        assert!(!super::should_send_stream_state(true, true, true));
        assert!(!super::should_send_stream_state(false, false, false));
        assert!(super::should_send_stream_state(true, false, false));
    }

    #[test]
    fn cursor_requires_both_parts() {
        assert!(
            AgentEventQuery {
                after_attempt_number: Some(1),
                after_sequence: None,
                limit: None,
            }
            .cursor()
            .is_err()
        );
        assert!(
            AgentEventQuery {
                after_attempt_number: Some(2),
                after_sequence: Some(1),
                limit: None,
            }
            .cursor()
            .unwrap()
            .is_some()
        );
    }
}
