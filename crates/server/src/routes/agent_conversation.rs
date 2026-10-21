use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use axum::{
    Router,
    extract::{Path, Query, State, ws::Message},
    response::{IntoResponse, Json},
    routing::get,
};
use deployment::Deployment;
use serde::Deserialize;
use services::services::agent_conversation::{
    AgentConversationService, ConversationBlock, ConversationBlockAppend, ConversationBlockDetail,
    ConversationChangesPage, ConversationCursor, ConversationPage, ConversationReadError,
    ConversationStreamMessage, subscribe_conversation_changes,
};
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{
    DeploymentImpl,
    error::ApiError,
    middleware::signed_ws::{MaybeSignedWebSocket, SignedWsUpgrade},
};

#[derive(Debug, Default, Deserialize)]
struct PageQuery {
    before: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct ChangeQuery {
    epoch: Option<Uuid>,
    after_revision: Option<String>,
    limit: Option<u32>,
}

impl ChangeQuery {
    fn cursor(&self) -> Result<Option<ConversationCursor>, ConversationReadError> {
        match (&self.epoch, &self.after_revision) {
            (None, None) => Ok(None),
            (Some(epoch), Some(revision)) => Ok(Some(ConversationCursor {
                epoch: *epoch,
                revision: revision.clone(),
            })),
            _ => Err(ConversationReadError::InvalidCursor(
                "epoch and after_revision must be supplied together".into(),
            )),
        }
    }
}

#[derive(Debug, Deserialize)]
struct DetailQuery {
    #[serde(default = "default_body_field")]
    field: String,
    #[serde(default)]
    offset: u64,
    limit: Option<u32>,
}

fn default_body_field() -> String {
    "text".into()
}

async fn conversation_page(
    State(deployment): State<DeploymentImpl>,
    Path(session_id): Path<Uuid>,
    Query(query): Query<PageQuery>,
) -> Result<Json<ApiResponse<ConversationPage>>, ApiError> {
    let page = AgentConversationService::new(&deployment.db().pool)
        .page(
            session_id,
            query.before.as_deref(),
            query.limit.unwrap_or(50),
        )
        .await
        .map_err(read_error)?;
    Ok(Json(ApiResponse::success(page)))
}

async fn conversation_changes(
    State(deployment): State<DeploymentImpl>,
    Path(session_id): Path<Uuid>,
    Query(query): Query<ChangeQuery>,
) -> Result<Json<ApiResponse<ConversationChangesPage>>, ApiError> {
    let cursor = query
        .cursor()
        .map_err(read_error)?
        .ok_or_else(|| ApiError::BadRequest("epoch and after_revision are required".into()))?;
    let page = AgentConversationService::new(&deployment.db().pool)
        .changes(session_id, &cursor, query.limit.unwrap_or(50))
        .await
        .map_err(read_error)?;
    Ok(Json(ApiResponse::success(page)))
}

async fn conversation_detail(
    State(deployment): State<DeploymentImpl>,
    Path((session_id, block_id)): Path<(Uuid, String)>,
    Query(query): Query<DetailQuery>,
) -> Result<Json<ApiResponse<ConversationBlockDetail>>, ApiError> {
    let block = AgentConversationService::new(&deployment.db().pool)
        .block_detail(
            session_id,
            &block_id,
            &query.field,
            query.offset,
            query.limit.unwrap_or(32768),
        )
        .await
        .map_err(read_error)?;
    Ok(Json(ApiResponse::success(block)))
}

async fn conversation_stream(
    ws: SignedWsUpgrade,
    State(deployment): State<DeploymentImpl>,
    Path(session_id): Path<Uuid>,
    Query(query): Query<ChangeQuery>,
) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        if let Err(error) = stream(&mut socket, &deployment, session_id, query).await {
            let _ = send(
                &mut socket,
                &ConversationStreamMessage::Error {
                    message: error.to_string(),
                },
            )
            .await;
            tracing::warn!(%session_id,%error,"conversation WebSocket closed");
        }
        let _ = socket.close().await;
    })
}

async fn stream(
    socket: &mut MaybeSignedWebSocket,
    deployment: &DeploymentImpl,
    session_id: Uuid,
    query: ChangeQuery,
) -> anyhow::Result<()> {
    // Subscribe before the first snapshot/read. Notifications are hints; every
    // catch-up reads committed rows and a lagged receiver never drops facts.
    let mut canonical_changes = db::models::agent_runtime::subscribe_agent_event_changes();
    let mut projection_changes = subscribe_conversation_changes();
    let reader = AgentConversationService::new(&deployment.db().pool);
    let mut known_runs = HashSet::new();
    let mut last_metadata = String::new();
    let mut cursor = match query.cursor()? {
        Some(cursor) => cursor,
        None => {
            let page = reader.page(session_id, None, 50).await?;
            known_runs.extend(page.runs.iter().map(|run| run.agent_run_id));
            last_metadata = serde_json::to_string(&(&page.runs, &page.pending_interactions))?;
            let cursor = page.cursor.clone();
            send(socket, &ConversationStreamMessage::Snapshot(page)).await?;
            cursor
        }
    };
    catch_up(
        socket,
        &reader,
        session_id,
        &mut cursor,
        &mut known_runs,
        &mut last_metadata,
        &[],
    )
    .await?;
    send(socket, &ConversationStreamMessage::Ready(cursor.clone())).await?;
    let mut repair = tokio::time::interval(Duration::from_secs(30));
    repair.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    repair.tick().await;
    loop {
        let mut appends = None;
        tokio::select! {
            notification=canonical_changes.recv()=>{
                if let Ok(run_id)=notification {
                    if !known_runs.contains(&run_id) {
                        let owner:Option<Uuid>=sqlx::query_scalar("SELECT session_id FROM agent_runs WHERE id=?")
                            .bind(run_id).fetch_optional(&deployment.db().pool).await?;
                        if owner!=Some(session_id) {continue;}
                        known_runs.insert(run_id);
                    }
                    // Only ordinary append traffic starts a coalescing window.
                    // A terminal/interaction arriving inside that window waits
                    // at most its remaining 50ms; a directly observed one does
                    // not add another delay.
                    let ordinary:bool=sqlx::query_scalar(
                        "SELECT COALESCE(json_extract(event_envelope,'$.payload.type')='conversation'
                         AND json_extract(event_envelope,'$.payload.data.update.operation.op')='append',0)
                         FROM agent_events WHERE agent_run_id=? ORDER BY run_attempt_number DESC,sequence DESC LIMIT 1",
                    ).bind(run_id).fetch_optional(&deployment.db().pool).await?.unwrap_or(false);
                    if ordinary {tokio::time::sleep(Duration::from_millis(50)).await;}
                    // One committed catch-up covers every wakeup already in the
                    // bounded queue, including different/new runs in a session.
                    while canonical_changes.try_recv().is_ok() {}
                }
            }
            notification=projection_changes.recv()=>{
                match notification {
                    Ok(notification) if notification.session_id!=session_id=>continue,
                    Ok(mut notification) if notification.epoch==cursor.epoch && !notification.appends.is_empty()=>{
                        let acknowledged=cursor.revision.parse::<u64>().unwrap_or(0);
                        notification.appends.retain(|append|append.revision.parse::<u64>().is_ok_and(|revision|revision>acknowledged));
                        if !notification.appends.is_empty() {appends=Some(notification);}
                    }
                    _=>{}
                }
            }
            _=repair.tick()=>{}
            inbound=socket.recv()=>{
                match inbound {
                    Ok(Some(Message::Close(_)))|Ok(None)|Err(_)=>break,
                    _=>continue,
                }
            }
        }
        reader.project_session(session_id).await?;
        let mut candidates = appends
            .map(|notification| notification.appends)
            .unwrap_or_default();
        while let Ok(notification) = projection_changes.try_recv() {
            if notification.session_id == session_id && notification.epoch == cursor.epoch {
                candidates.extend(notification.appends);
            }
        }
        // The bus is an optional bounded optimization, never a delivery log.
        // Oversized batches recover through the durable body references.
        if candidates
            .iter()
            .map(|append| append.text.len())
            .sum::<usize>()
            > 65536
        {
            candidates.clear();
        }
        catch_up(
            socket,
            &reader,
            session_id,
            &mut cursor,
            &mut known_runs,
            &mut last_metadata,
            &candidates,
        )
        .await?;
    }
    Ok(())
}

async fn catch_up(
    socket: &mut MaybeSignedWebSocket,
    reader: &AgentConversationService<'_>,
    session_id: Uuid,
    cursor: &mut ConversationCursor,
    known_runs: &mut HashSet<Uuid>,
    last_metadata: &mut String,
    candidates: &[ConversationBlockAppend],
) -> anyhow::Result<()> {
    loop {
        let mut page = reader.changes(session_id, cursor, 50).await?;
        if page.reset_required {
            let baseline = reader.page(session_id, None, 50).await?;
            *cursor = baseline.cursor.clone();
            known_runs.extend(baseline.runs.iter().map(|run| run.agent_run_id));
            *last_metadata =
                serde_json::to_string(&(&baseline.runs, &baseline.pending_interactions))?;
            send(socket, &ConversationStreamMessage::Snapshot(baseline)).await?;
            // Subscribe was established before reset; reread after baseline.
            continue;
        }
        let (appends, optimized) = eligible_appends(&page.blocks, candidates, cursor);
        if !appends.is_empty() {
            send(
                socket,
                &ConversationStreamMessage::Append {
                    session_id,
                    epoch: cursor.epoch,
                    appends,
                },
            )
            .await?;
            page.blocks
                .retain(|block| !optimized.contains(&block.block_id));
        }
        let cursor_changed = *cursor != page.cursor;
        *cursor = page.cursor.clone();
        known_runs.extend(page.runs.iter().map(|run| run.agent_run_id));
        let has_more = page.has_more;
        let metadata = serde_json::to_string(&(&page.runs, &page.pending_interactions))?;
        if cursor_changed
            || !page.blocks.is_empty()
            || !page.tombstones.is_empty()
            || metadata != *last_metadata
        {
            *last_metadata = metadata;
            send(socket, &ConversationStreamMessage::Changes(page)).await?;
        }
        if !has_more {
            break;
        }
    }
    Ok(())
}

fn eligible_appends(
    blocks: &[ConversationBlock],
    candidates: &[ConversationBlockAppend],
    cursor: &ConversationCursor,
) -> (Vec<ConversationBlockAppend>, HashSet<String>) {
    let acknowledged = cursor.revision.parse::<u64>().unwrap_or(0);
    let mut chains: HashMap<&str, Vec<&ConversationBlockAppend>> = HashMap::new();
    for append in candidates {
        if append
            .revision
            .parse::<u64>()
            .is_ok_and(|revision| revision > acknowledged)
        {
            chains.entry(&append.block_id).or_default().push(append);
        }
    }
    let mut optimized = HashSet::new();
    for block in blocks {
        if let Some(chain) = chains.get(block.block_id.as_str()) {
            let base = chain[0].base_revision.parse::<u64>().unwrap_or(u64::MAX);
            if base > 0
                && base <= acknowledged
                && chain
                    .last()
                    .is_some_and(|append| append.revision == block.revision)
                && chain
                    .windows(2)
                    .all(|pair| pair[0].revision == pair[1].base_revision)
            {
                optimized.insert(block.block_id.clone());
            }
        }
    }
    let appends = candidates
        .iter()
        .filter(|append| {
            optimized.contains(&append.block_id)
                && append
                    .revision
                    .parse::<u64>()
                    .is_ok_and(|revision| revision > acknowledged)
        })
        .cloned()
        .collect();
    (appends, optimized)
}

async fn send(
    socket: &mut MaybeSignedWebSocket,
    message: &ConversationStreamMessage,
) -> anyhow::Result<()> {
    tokio::time::timeout(
        Duration::from_secs(10),
        socket.send(Message::Text(serde_json::to_string(message)?.into())),
    )
    .await??;
    Ok(())
}

fn read_error(error: ConversationReadError) -> ApiError {
    match error {
        ConversationReadError::Database(error) => ApiError::Database(error),
        ConversationReadError::SessionNotFound(_) => {
            ApiError::Session(db::models::session::SessionError::NotFound)
        }
        ConversationReadError::IdentityConflict(block) => {
            ApiError::Conflict(format!("conversation block identity changed: {block}"))
        }
        other => ApiError::BadRequest(other.to_string()),
    }
}

pub(super) fn router(_: &DeploymentImpl) -> Router<DeploymentImpl> {
    Router::new()
        .route(
            "/sessions/{session_id}/conversation",
            get(conversation_page),
        )
        .route(
            "/sessions/{session_id}/conversation/changes",
            get(conversation_changes),
        )
        .route(
            "/sessions/{session_id}/conversation/blocks/{block_id}",
            get(conversation_detail),
        )
        .route(
            "/sessions/{session_id}/conversation/stream/ws",
            get(conversation_stream),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_cursor_requires_both_parts() {
        assert!(ChangeQuery::default().cursor().unwrap().is_none());
        assert!(
            ChangeQuery {
                epoch: Some(Uuid::nil()),
                ..Default::default()
            }
            .cursor()
            .is_err()
        );
        assert!(
            ChangeQuery {
                after_revision: Some("10".into()),
                ..Default::default()
            }
            .cursor()
            .is_err()
        );
    }

    fn block(id: &str, revision: &str) -> ConversationBlock {
        ConversationBlock {
            block_id: id.into(),
            agent_run_id: Uuid::nil(),
            run_attempt_id: Uuid::nil(),
            kind: executors::runtime::ConversationBlockKind::Text,
            role: None,
            native_id: None,
            parent_block_id: None,
            position: "opaque".into(),
            revision: revision.into(),
            fields: Default::default(),
            bodies: Vec::new(),
            finished: false,
            finish_reason: None,
            source_event_id: Uuid::nil(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn append(id: &str, base: &str, revision: &str) -> ConversationBlockAppend {
        ConversationBlockAppend {
            block_id: id.into(),
            base_revision: base.into(),
            revision: revision.into(),
            field: "text".into(),
            text: "delta".into(),
        }
    }

    #[test]
    fn complete_append_chain_optimizes_only_changes_after_the_acknowledged_cursor() {
        let cursor = ConversationCursor {
            epoch: Uuid::nil(),
            revision: "10".into(),
        };
        let candidates = vec![
            append("a", "0", "3"),
            append("a", "3", "12"),
            append("b", "2", "13"),
            append("a", "12", "17"),
        ];
        let (appends, optimized) =
            eligible_appends(&[block("a", "17"), block("b", "13")], &candidates, &cursor);
        assert_eq!(
            appends
                .iter()
                .map(|append| append.revision.as_str())
                .collect::<Vec<_>>(),
            vec!["12", "13", "17"]
        );
        assert_eq!(optimized, HashSet::from(["a".into(), "b".into()]));
    }

    #[test]
    fn missing_baselines_patch_gaps_and_finish_updates_require_a_full_baseline() {
        let cursor = ConversationCursor {
            epoch: Uuid::nil(),
            revision: "10".into(),
        };
        for (baseline, candidates) in [
            (block("a", "12"), vec![append("a", "0", "12")]),
            (block("a", "12"), vec![append("a", "11", "12")]),
            (
                block("a", "17"),
                vec![append("a", "3", "12"), append("a", "14", "17")],
            ),
            (
                block("a", "18"),
                vec![append("a", "3", "12"), append("a", "12", "17")],
            ),
        ] {
            let (appends, optimized) = eligible_appends(&[baseline], &candidates, &cursor);
            assert!(appends.is_empty());
            assert!(optimized.is_empty());
        }
    }
}
