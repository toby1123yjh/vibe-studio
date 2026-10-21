use chrono::{DateTime, Duration, Utc};
use executors::runtime::{
    AgentRunPortCommand, AgentRunPortCommandEnvelope, AgentRunStatus, AgentRuntimeMessageRole,
    ContractVersionError, OrchestrationCommandValidationError, OrchestrationEventEnvelope,
    OrchestrationNodeStatus, OrchestrationPlanSnapshot, OrchestrationReducerApply,
    OrchestrationReducerError, OrchestrationRunStatus, OrchestrationState, ProjectionStatus,
    UpstreamHandoff, UpstreamSourceReference, reduce_orchestration_event,
};
use serde::Serialize;
use sqlx::{FromRow, SqliteConnection, SqlitePool, types::Json};
use thiserror::Error;
use uuid::Uuid;

const DEFAULT_OUTBOX_REDELIVERY_AFTER: Duration = Duration::seconds(30);

#[derive(Debug, Clone, FromRow)]
pub struct OrchestrationRunRecord {
    pub id: Uuid,
    pub request_id: Uuid,
    pub idempotency_key: String,
    pub correlation_id: Uuid,
    pub product_kind: String,
    pub source_definition_id: Uuid,
    pub source_definition_version: String,
    pub plan_schema_version: i64,
    pub plan_snapshot: Json<OrchestrationPlanSnapshot>,
    pub status: OrchestrationRunStatus,
    pub projection_status: ProjectionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct OrchestrationNodeExecutionRecord {
    pub id: Uuid,
    pub orchestration_run_id: Uuid,
    pub node_key: String,
    pub iteration: i64,
    pub stable_order: i64,
    pub status: executors::runtime::OrchestrationNodeStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct OrchestrationAgentRunLinkRecord {
    pub id: Uuid,
    pub orchestration_run_id: Uuid,
    pub node_execution_id: Uuid,
    pub agent_run_id: Uuid,
    pub dispatch_idempotency_key: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct OrchestrationEventRecord {
    pub event_id: Uuid,
    pub orchestration_run_id: Uuid,
    pub sequence: i64,
    pub correlation_id: Uuid,
    pub schema_version: i64,
    pub payload_version: i64,
    pub event_envelope: Json<OrchestrationEventEnvelope>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct OrchestrationStateRecord {
    pub orchestration_run_id: Uuid,
    pub state_schema_version: i64,
    pub reducer_version: i64,
    pub last_event_sequence: i64,
    pub last_event_id: Option<Uuid>,
    pub status: OrchestrationRunStatus,
    pub projection_status: ProjectionStatus,
    pub state_json: Json<OrchestrationState>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct OrchestrationOutboxRecord {
    pub id: Uuid,
    pub orchestration_run_id: Uuid,
    pub node_execution_id: Option<Uuid>,
    pub command_id: Uuid,
    pub idempotency_key: String,
    pub command_schema_version: i64,
    pub command_envelope: Json<AgentRunPortCommandEnvelope>,
    pub delivery_status: String,
    pub delivery_attempts: i64,
    pub available_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct OrchestrationInboxRecord {
    pub id: Uuid,
    pub orchestration_run_id: Uuid,
    pub source_event_id: Uuid,
    pub source_agent_run_id: Uuid,
    pub source_sequence: i64,
    pub event_envelope: Json<executors::runtime::AgentEventEnvelope>,
    pub consumption_status: String,
    pub received_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, FromRow)]
pub struct OrchestrationConsumptionRecord {
    pub id: Uuid,
    pub orchestration_run_id: Uuid,
    pub join_node_execution_id: Uuid,
    pub source_node_execution_id: Uuid,
    pub source_agent_run_id: Uuid,
    pub source_event_id: Uuid,
    pub target_node_execution_id: Option<Uuid>,
    pub consumed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialEachQueueItem {
    pub consumption_id: Uuid,
    pub source_event_id: Uuid,
    pub source_sequence: u64,
    pub target_node_execution_id: Uuid,
    pub target_status: OrchestrationNodeStatus,
}

#[derive(Debug, Clone, FromRow)]
pub struct OrchestrationLeaseRecord {
    pub resource_kind: String,
    pub resource_id: Uuid,
    pub owner_id: String,
    pub fencing_token: i64,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Error)]
pub enum OrchestrationPersistenceError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error(transparent)]
    InvalidVersion(#[from] ContractVersionError),
    #[error(transparent)]
    InvalidCommand(#[from] OrchestrationCommandValidationError),
    #[error("orchestration field {0} did not serialize as a string enum")]
    InvalidEnumEncoding(&'static str),
    #[error("node execution identity was not found after idempotent persistence")]
    MissingNodeIdentity,
    #[error("idempotency identity {key:?} was reused with different {entity} identity or payload")]
    IdempotencyConflict { entity: &'static str, key: String },
    #[error(transparent)]
    Reducer(#[from] OrchestrationReducerError),
    #[error("orchestration state row was not found for run {0}")]
    MissingState(Uuid),
    #[error("orchestration inbox event {0} was not linked to the requested run")]
    InboxRunMismatch(Uuid),
    #[error("orchestration inbox event {event_id} came from unmanaged AgentRun {agent_run_id}")]
    UnmanagedInboxSource { event_id: Uuid, agent_run_id: Uuid },
    #[error("orchestration inbox source event {0} was not persisted in the canonical event log")]
    MissingInboxSourceEvent(Uuid),
    #[error("orchestration inbox event {0} did not identify its linked node execution")]
    InboxNodeMismatch(Uuid),
    #[error("orchestration inbox event {0} has an invalid sequence")]
    InvalidInboxSequence(Uuid),
    #[error("orchestration source node {0} is not a successful consumable handoff")]
    NonConsumableHandoff(Uuid),
    #[error("orchestration follow-up effect does not belong to the consumed source run")]
    EffectRunMismatch,
    #[error("orchestration command AgentRun is not linked to its target node execution")]
    CommandAgentRunMismatch,
}

impl OrchestrationRunRecord {
    pub async fn persist_before_dispatch(
        pool: &SqlitePool,
        id: Uuid,
        request_id: Uuid,
        idempotency_key: &str,
        correlation_id: Uuid,
        plan: &OrchestrationPlanSnapshot,
    ) -> Result<Uuid, OrchestrationPersistenceError> {
        plan.validate_current()?;
        // Acquire the writer before the idempotency read. A deferred WAL
        // snapshot cannot be upgraded while another connection is writing.
        let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = sqlx::query_as::<_, ExistingOrchestrationRun>(
            r#"
            SELECT id, request_id, correlation_id, plan_snapshot
            FROM orchestration_runs
            WHERE idempotency_key = ?
            "#,
        )
        .bind(idempotency_key)
        .fetch_optional(&mut *transaction)
        .await?
        {
            if existing.id != id
                || existing.request_id != request_id
                || existing.correlation_id != correlation_id
                || existing.plan_snapshot.0 != *plan
            {
                return Err(OrchestrationPersistenceError::IdempotencyConflict {
                    entity: "orchestration run",
                    key: idempotency_key.to_string(),
                });
            }
            transaction.commit().await?;
            return Ok(existing.id);
        }

        let product_kind = enum_string("product_kind", plan.product_kind)?;
        let plan_json = serde_json::to_string(plan)?;
        let state = OrchestrationState::pending(id, plan.created_at);
        let state_json = serde_json::to_string(&state)?;

        sqlx::query(
            r#"
            INSERT INTO orchestration_runs (
                id, request_id, idempotency_key, correlation_id, product_kind,
                source_definition_id, source_definition_version,
                plan_schema_version, plan_snapshot, status, projection_status
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(id)
        .bind(request_id)
        .bind(idempotency_key)
        .bind(correlation_id)
        .bind(product_kind)
        .bind(plan.source_definition_id)
        .bind(&plan.source_definition_version)
        .bind(i64::from(plan.schema_version))
        .bind(plan_json)
        .bind(OrchestrationRunStatus::Pending)
        .bind(ProjectionStatus::Current)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO orchestration_state (
                orchestration_run_id, state_schema_version, reducer_version,
                last_event_sequence, status, projection_status, state_json
            ) VALUES (?, ?, ?, 0, ?, ?, ?)
            "#,
        )
        .bind(id)
        .bind(i64::from(state.state_schema_version))
        .bind(i64::from(state.reducer_version))
        .bind(state.status)
        .bind(state.projection_status)
        .bind(state_json)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(id)
    }
}

impl OrchestrationEventRecord {
    /// Append one orchestration fact and update its reducer-owned projection
    /// atomically. A duplicate event is acknowledged without changing state.
    pub async fn append_and_project(
        pool: &SqlitePool,
        event: &OrchestrationEventEnvelope,
    ) -> Result<OrchestrationReducerApply, OrchestrationPersistenceError> {
        let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
        let applied = append_and_project_in_transaction(&mut *transaction, event).await?;
        transaction.commit().await?;
        Ok(applied)
    }
}

async fn append_and_project_in_transaction(
    connection: &mut SqliteConnection,
    event: &OrchestrationEventEnvelope,
) -> Result<OrchestrationReducerApply, OrchestrationPersistenceError> {
    event.validate_for_projection()?;
    let sequence = i64::try_from(event.sequence).map_err(|error| {
        OrchestrationPersistenceError::Serialization(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error,
        )))
    })?;
    let event_json = serde_json::to_string(event)?;

    if let Some(existing) = sqlx::query_as::<_, ExistingOrchestrationEvent>(
        "SELECT event_envelope FROM orchestration_events WHERE event_id = ?",
    )
    .bind(event.event_id)
    .fetch_optional(&mut *connection)
    .await?
    {
        if existing.event_envelope.0 == *event {
            return Ok(OrchestrationReducerApply::Duplicate);
        }
        return Err(OrchestrationPersistenceError::IdempotencyConflict {
            entity: "orchestration event",
            key: event.event_id.to_string(),
        });
    }

    if let Some(existing) = sqlx::query_as::<_, ExistingOrchestrationEvent>(
        "SELECT event_envelope FROM orchestration_events WHERE orchestration_run_id = ? AND sequence = ?",
    )
    .bind(event.orchestration_run_id)
    .bind(sequence)
    .fetch_optional(&mut *connection)
    .await?
    {
        if existing.event_envelope.0 == *event {
            return Ok(OrchestrationReducerApply::Duplicate);
        }
        return Err(OrchestrationPersistenceError::IdempotencyConflict {
            entity: "orchestration event sequence",
            key: format!("{}:{sequence}", event.orchestration_run_id),
        });
    }

    let state_json: Json<OrchestrationState> = sqlx::query_scalar(
        "SELECT state_json FROM orchestration_state WHERE orchestration_run_id = ?",
    )
    .bind(event.orchestration_run_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(OrchestrationPersistenceError::MissingState(
        event.orchestration_run_id,
    ))?;
    let mut state = state_json.0;
    let applied = reduce_orchestration_event(&mut state, event)?;
    sqlx::query(
        r#"
        INSERT INTO orchestration_events (
            event_id, orchestration_run_id, sequence, correlation_id,
            schema_version, payload_version, event_envelope
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(event.event_id)
    .bind(event.orchestration_run_id)
    .bind(sequence)
    .bind(event.correlation_id)
    .bind(i64::from(event.schema_version))
    .bind(i64::from(event.payload_version))
    .bind(event_json)
    .execute(&mut *connection)
    .await?;

    let projected_json = serde_json::to_string(&state)?;
    sqlx::query(
        r#"
        UPDATE orchestration_state
        SET last_event_sequence = ?, last_event_id = ?, status = ?,
            projection_status = ?, state_json = ?, updated_at = ?
        WHERE orchestration_run_id = ?
        "#,
    )
    .bind(sequence)
    .bind(state.last_event_id)
    .bind(state.status)
    .bind(state.projection_status)
    .bind(projected_json)
    .bind(state.updated_at)
    .bind(event.orchestration_run_id)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "UPDATE orchestration_runs SET status = ?, projection_status = ?, updated_at = ? WHERE id = ?",
    )
    .bind(state.status)
    .bind(state.projection_status)
    .bind(state.updated_at)
    .bind(event.orchestration_run_id)
    .execute(&mut *connection)
    .await?;
    Ok(applied)
}

impl OrchestrationNodeExecutionRecord {
    pub async fn persist_identity_before_dispatch(
        pool: &SqlitePool,
        id: Uuid,
        orchestration_run_id: Uuid,
        node_key: &str,
        iteration: u32,
        stable_order: u32,
    ) -> Result<Uuid, OrchestrationPersistenceError> {
        sqlx::query(
            r#"
            INSERT INTO orchestration_node_executions (
                id, orchestration_run_id, node_key, iteration, stable_order, status
            ) VALUES (?, ?, ?, ?, ?, 'pending')
            ON CONFLICT(orchestration_run_id, node_key, iteration) DO NOTHING
            "#,
        )
        .bind(id)
        .bind(orchestration_run_id)
        .bind(node_key)
        .bind(i64::from(iteration))
        .bind(i64::from(stable_order))
        .execute(pool)
        .await?;

        let existing = sqlx::query_as::<_, ExistingNodeExecution>(
            r#"
            SELECT id, stable_order FROM orchestration_node_executions
            WHERE orchestration_run_id = ? AND node_key = ? AND iteration = ?
            "#,
        )
        .bind(orchestration_run_id)
        .bind(node_key)
        .bind(i64::from(iteration))
        .fetch_optional(pool)
        .await?
        .ok_or(OrchestrationPersistenceError::MissingNodeIdentity)?;
        if existing.id != id || existing.stable_order != i64::from(stable_order) {
            return Err(OrchestrationPersistenceError::IdempotencyConflict {
                entity: "orchestration node execution",
                key: format!("{orchestration_run_id}:{node_key}:{iteration}"),
            });
        }
        Ok(existing.id)
    }
}

impl OrchestrationAgentRunLinkRecord {
    pub async fn persist(
        pool: &SqlitePool,
        id: Uuid,
        orchestration_run_id: Uuid,
        node_execution_id: Uuid,
        agent_run_id: Uuid,
        dispatch_idempotency_key: &str,
    ) -> Result<Uuid, OrchestrationPersistenceError> {
        sqlx::query(
            r#"
            INSERT INTO orchestration_agent_run_links (
                id, orchestration_run_id, node_execution_id, agent_run_id,
                dispatch_idempotency_key
            ) VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(dispatch_idempotency_key) DO NOTHING
            "#,
        )
        .bind(id)
        .bind(orchestration_run_id)
        .bind(node_execution_id)
        .bind(agent_run_id)
        .bind(dispatch_idempotency_key)
        .execute(pool)
        .await?;
        let existing = sqlx::query_as::<_, ExistingAgentRunLink>(
            r#"
            SELECT id, orchestration_run_id, node_execution_id, agent_run_id
            FROM orchestration_agent_run_links
            WHERE dispatch_idempotency_key = ?
            "#,
        )
        .bind(dispatch_idempotency_key)
        .fetch_one(pool)
        .await?;
        if existing.orchestration_run_id != orchestration_run_id
            || existing.node_execution_id != node_execution_id
            || existing.agent_run_id != agent_run_id
            || existing.id != id
        {
            return Err(OrchestrationPersistenceError::IdempotencyConflict {
                entity: "orchestration agent run link",
                key: dispatch_idempotency_key.to_string(),
            });
        }
        Ok(existing.id)
    }
}

impl OrchestrationOutboxRecord {
    /// Internal cancellation producers can race or retry with a new timestamp
    /// and diagnostic reason. Retain the first exact command for the same
    /// operation and target; all other command types keep strict idempotency.
    pub async fn enqueue_cancellation(
        pool: &SqlitePool,
        id: Uuid,
        command: &AgentRunPortCommandEnvelope,
    ) -> Result<Uuid, OrchestrationPersistenceError> {
        Ok(Self::enqueue_cancellation_with_outcome(pool, id, command)
            .await?
            .0)
    }

    pub async fn enqueue_cancellation_with_outcome(
        pool: &SqlitePool,
        id: Uuid,
        command: &AgentRunPortCommandEnvelope,
    ) -> Result<(Uuid, bool), OrchestrationPersistenceError> {
        command.validate_current()?;
        if !matches!(&command.command, AgentRunPortCommand::Cancel { .. }) {
            return Err(OrchestrationPersistenceError::IdempotencyConflict {
                entity: "orchestration cancellation",
                key: command.idempotency_key.clone(),
            });
        }
        let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing = sqlx::query_as::<_, ExistingOutboxCommand>(
            "SELECT id, command_envelope FROM orchestration_outbox WHERE idempotency_key = ?",
        )
        .bind(&command.idempotency_key)
        .fetch_optional(&mut *transaction)
        .await?;
        let outcome = if let Some(existing) = existing {
            let original = &existing.command_envelope.0;
            if original.schema_version != command.schema_version
                || original.command_id != command.command_id
                || original.agent_run_id != command.agent_run_id
                || original.orchestration_run_id != command.orchestration_run_id
                || original.orchestration_node_execution_id
                    != command.orchestration_node_execution_id
                || !matches!(&original.command, AgentRunPortCommand::Cancel { .. })
            {
                return Err(OrchestrationPersistenceError::IdempotencyConflict {
                    entity: "orchestration cancellation",
                    key: command.idempotency_key.clone(),
                });
            }
            (existing.id, false)
        } else {
            enqueue_outbox_in_transaction(&mut transaction, id, command).await?
        };
        transaction.commit().await?;
        Ok(outcome)
    }

    pub async fn enqueue(
        pool: &SqlitePool,
        id: Uuid,
        command: &AgentRunPortCommandEnvelope,
    ) -> Result<Uuid, OrchestrationPersistenceError> {
        Ok(Self::enqueue_with_outcome(pool, id, command).await?.0)
    }

    pub async fn enqueue_with_outcome(
        pool: &SqlitePool,
        id: Uuid,
        command: &AgentRunPortCommandEnvelope,
    ) -> Result<(Uuid, bool), OrchestrationPersistenceError> {
        let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
        let outcome = enqueue_outbox_in_transaction(&mut *transaction, id, command).await?;
        transaction.commit().await?;
        Ok(outcome)
    }

    pub async fn claim_next(
        pool: &SqlitePool,
        now: DateTime<Utc>,
    ) -> Result<Option<Self>, OrchestrationPersistenceError> {
        Self::claim_next_with_redelivery(pool, now, DEFAULT_OUTBOX_REDELIVERY_AFTER).await
    }

    /// Claim a command, reclaiming deliveries left in-flight by a crashed
    /// dispatcher. The delivery lease is deliberately separate from the
    /// AgentRun lifecycle: reclaiming an outbox row only retries an
    /// idempotent command and never implies that an Agent process ended.
    pub async fn claim_next_with_redelivery(
        pool: &SqlitePool,
        now: DateTime<Utc>,
        redelivery_after: Duration,
    ) -> Result<Option<Self>, OrchestrationPersistenceError> {
        let mut transaction = pool.begin().await?;
        let stale_before = now - redelivery_after;
        sqlx::query(
            "UPDATE orchestration_outbox SET delivery_status='failed',
             last_error='native interaction delivery is unconfirmed',updated_at=?
             WHERE delivery_status='delivering' AND updated_at<=?
             AND json_extract(command_envelope,'$.command.type')='reply_interaction'",
        )
        .bind(now)
        .bind(stale_before)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE orchestration_outbox SET delivery_status = 'pending', updated_at = ? WHERE delivery_status = 'delivering' AND updated_at <= ?",
        )
        .bind(now)
        .bind(stale_before)
        .execute(&mut *transaction)
        .await?;
        let candidate = sqlx::query_as::<_, ExistingOutboxIdentity>(
            r#"
            SELECT id
            FROM orchestration_outbox
            WHERE delivery_status IN ('pending', 'failed')
              AND NOT (delivery_status='failed' AND json_extract(command_envelope,'$.command.type')='reply_interaction')
              AND available_at <= ?
            ORDER BY available_at ASC, created_at ASC
            LIMIT 1
            "#,
        )
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(candidate) = candidate else {
            transaction.commit().await?;
            return Ok(None);
        };
        let updated = sqlx::query(
            r#"
            UPDATE orchestration_outbox
            SET delivery_status = 'delivering',
                delivery_attempts = delivery_attempts + 1,
                updated_at = ?
            WHERE id = ? AND delivery_status IN ('pending', 'failed')
            "#,
        )
        .bind(now)
        .bind(candidate.id)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() == 0 {
            transaction.commit().await?;
            return Ok(None);
        }
        let record = sqlx::query_as::<_, Self>("SELECT * FROM orchestration_outbox WHERE id = ?")
            .bind(candidate.id)
            .fetch_one(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(Some(record))
    }

    /// Make all in-flight rows eligible for delivery after a process restart.
    pub async fn reconcile_inflight(
        pool: &SqlitePool,
        now: DateTime<Utc>,
        redelivery_after: Duration,
    ) -> Result<u64, OrchestrationPersistenceError> {
        let stale_before = now - redelivery_after;
        let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "UPDATE orchestration_outbox SET delivery_status='failed',
             last_error='native interaction delivery is unconfirmed after restart',updated_at=?
             WHERE delivery_status='delivering' AND updated_at<=?
             AND json_extract(command_envelope,'$.command.type')='reply_interaction'",
        )
        .bind(now)
        .bind(stale_before)
        .execute(&mut *transaction)
        .await?;
        let result = sqlx::query(
            "UPDATE orchestration_outbox SET delivery_status = 'pending', updated_at = ? WHERE delivery_status = 'delivering' AND updated_at <= ?",
        )
        .bind(now)
        .bind(stale_before)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn pending_count(pool: &SqlitePool) -> Result<u64, OrchestrationPersistenceError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM orchestration_outbox WHERE delivery_status IN ('pending', 'failed', 'delivering')",
        )
        .fetch_one(pool)
        .await?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    pub async fn mark_delivered(
        pool: &SqlitePool,
        id: Uuid,
        delivered_at: DateTime<Utc>,
    ) -> Result<bool, OrchestrationPersistenceError> {
        let result = sqlx::query(
            r#"
            UPDATE orchestration_outbox
            SET delivery_status = 'delivered', delivered_at = ?, updated_at = ?
            WHERE id = ? AND delivery_status = 'delivering'
            "#,
        )
        .bind(delivered_at)
        .bind(delivered_at)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn mark_failed(
        pool: &SqlitePool,
        id: Uuid,
        error: &str,
        available_at: DateTime<Utc>,
    ) -> Result<bool, OrchestrationPersistenceError> {
        let result = sqlx::query(
            r#"
            UPDATE orchestration_outbox
            SET delivery_status = 'failed', last_error = ?, available_at = ?, updated_at = ?
            WHERE id = ? AND delivery_status = 'delivering'
            "#,
        )
        .bind(error)
        .bind(available_at)
        .bind(Utc::now())
        .bind(id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}

async fn enqueue_outbox_in_transaction(
    connection: &mut SqliteConnection,
    id: Uuid,
    command: &AgentRunPortCommandEnvelope,
) -> Result<(Uuid, bool), OrchestrationPersistenceError> {
    command.validate_current()?;
    let orchestration_run_id = command
        .orchestration_run_id
        .ok_or(OrchestrationPersistenceError::MissingNodeIdentity)?;
    let orchestration_node_execution_id = command
        .orchestration_node_execution_id
        .ok_or(OrchestrationPersistenceError::MissingNodeIdentity)?;
    let target_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM orchestration_node_executions WHERE id = ? AND orchestration_run_id = ?)",
    )
    .bind(orchestration_node_execution_id)
    .bind(orchestration_run_id)
    .fetch_one(&mut *connection)
    .await?;
    if !target_exists {
        return Err(OrchestrationPersistenceError::MissingNodeIdentity);
    }
    let linked_agent_run_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT agent_run_id FROM orchestration_agent_run_links WHERE orchestration_run_id = ? AND node_execution_id = ?",
    )
    .bind(orchestration_run_id)
    .bind(orchestration_node_execution_id)
    .fetch_optional(&mut *connection)
    .await?;
    let is_create = matches!(&command.command, AgentRunPortCommand::Create { .. });
    if linked_agent_run_id.is_some_and(|linked| linked != command.agent_run_id)
        || (!is_create && linked_agent_run_id != Some(command.agent_run_id))
    {
        return Err(OrchestrationPersistenceError::CommandAgentRunMismatch);
    }
    let command_json = serde_json::to_string(command)?;
    let inserted = sqlx::query(
        r#"
            INSERT INTO orchestration_outbox (
                id, orchestration_run_id, node_execution_id, command_id,
                idempotency_key, command_schema_version, command_envelope
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(idempotency_key) DO NOTHING
            "#,
    )
    .bind(id)
    .bind(orchestration_run_id)
    .bind(orchestration_node_execution_id)
    .bind(command.command_id)
    .bind(&command.idempotency_key)
    .bind(i64::from(command.schema_version))
    .bind(command_json)
    .execute(&mut *connection)
    .await?;

    let existing = sqlx::query_as::<_, ExistingOutboxCommand>(
        r#"
            SELECT id, command_envelope
            FROM orchestration_outbox
            WHERE idempotency_key = ?
            "#,
    )
    .bind(&command.idempotency_key)
    .fetch_one(&mut *connection)
    .await?;
    if existing.command_envelope.0 != *command {
        return Err(OrchestrationPersistenceError::IdempotencyConflict {
            entity: "orchestration outbox command",
            key: command.idempotency_key.clone(),
        });
    }
    Ok((existing.id, inserted.rows_affected() == 1))
}

impl OrchestrationInboxRecord {
    /// Persist a canonical AgentEvent exactly once. The source event FK and
    /// orchestration-agent link FK make ingestion fail closed for events from
    /// another run or an unmanaged AgentRun.
    pub async fn ingest(
        pool: &SqlitePool,
        id: Uuid,
        orchestration_run_id: Uuid,
        event: &executors::runtime::AgentEventEnvelope,
    ) -> Result<Uuid, OrchestrationPersistenceError> {
        event
            .validate_for_projection()
            .map_err(OrchestrationPersistenceError::InvalidVersion)?;
        if event.orchestration_run_id != Some(orchestration_run_id) {
            return Err(OrchestrationPersistenceError::InboxRunMismatch(
                event.event_id,
            ));
        }
        let linked_node_execution_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT node_execution_id FROM orchestration_agent_run_links WHERE orchestration_run_id = ? AND agent_run_id = ?",
        )
        .bind(orchestration_run_id)
        .bind(event.agent_run_id)
        .fetch_optional(pool)
        .await?;
        let Some(linked_node_execution_id) = linked_node_execution_id else {
            return Err(OrchestrationPersistenceError::UnmanagedInboxSource {
                event_id: event.event_id,
                agent_run_id: event.agent_run_id,
            });
        };
        if event.orchestration_node_execution_id != Some(linked_node_execution_id) {
            return Err(OrchestrationPersistenceError::InboxNodeMismatch(
                event.event_id,
            ));
        }
        let source_event: Option<Json<executors::runtime::AgentEventEnvelope>> =
            sqlx::query_scalar(
                "SELECT event_envelope FROM agent_events WHERE event_id = ? AND agent_run_id = ?",
            )
            .bind(event.event_id)
            .bind(event.agent_run_id)
            .fetch_optional(pool)
            .await?;
        let Some(source_event) = source_event else {
            return Err(OrchestrationPersistenceError::MissingInboxSourceEvent(
                event.event_id,
            ));
        };
        if source_event.0 != *event {
            return Err(OrchestrationPersistenceError::IdempotencyConflict {
                entity: "orchestration inbox source event",
                key: event.event_id.to_string(),
            });
        }
        let source_sequence = i64::try_from(event.sequence)
            .map_err(|_| OrchestrationPersistenceError::InvalidInboxSequence(event.event_id))?;
        if source_sequence <= 0 {
            return Err(OrchestrationPersistenceError::InvalidInboxSequence(
                event.event_id,
            ));
        }
        let event_json = serde_json::to_string(event)?;
        sqlx::query(
            r#"
            INSERT INTO orchestration_inbox (
                id, orchestration_run_id, source_event_id, source_agent_run_id,
                source_sequence, event_envelope
            ) VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(source_event_id) DO NOTHING
            "#,
        )
        .bind(id)
        .bind(orchestration_run_id)
        .bind(event.event_id)
        .bind(event.agent_run_id)
        .bind(i64::try_from(event.sequence).map_err(|_| {
            OrchestrationPersistenceError::IdempotencyConflict {
                entity: "orchestration inbox event sequence",
                key: event.event_id.to_string(),
            }
        })?)
        .bind(event_json)
        .execute(pool)
        .await?;
        let existing = sqlx::query_as::<_, ExistingInboxIdentity>(
            "SELECT id, orchestration_run_id, source_agent_run_id, event_envelope FROM orchestration_inbox WHERE source_event_id = ?",
        )
        .bind(event.event_id)
        .fetch_one(pool)
        .await?;
        if existing.orchestration_run_id != orchestration_run_id
            || existing.source_agent_run_id != event.agent_run_id
            || existing.event_envelope.0 != *event
        {
            return Err(OrchestrationPersistenceError::IdempotencyConflict {
                entity: "orchestration inbox event",
                key: event.event_id.to_string(),
            });
        }
        Ok(existing.id)
    }

    /// Claim one pending inbox event. Startup reconciliation resets claims
    /// owned by a previous service process; live consumers never steal a
    /// processing row using the event's immutable receipt timestamp.
    pub async fn claim_next(
        pool: &SqlitePool,
        orchestration_run_id: Uuid,
    ) -> Result<Option<Self>, OrchestrationPersistenceError> {
        let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
        let candidate = sqlx::query_as::<_, ExistingInboxIdentity>(
            "SELECT id, orchestration_run_id, source_agent_run_id, event_envelope FROM orchestration_inbox WHERE orchestration_run_id = ? AND consumption_status = 'pending' ORDER BY received_at ASC, id ASC LIMIT 1",
        )
        .bind(orchestration_run_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(candidate) = candidate else {
            transaction.commit().await?;
            return Ok(None);
        };
        let claimed = sqlx::query(
            "UPDATE orchestration_inbox SET consumption_status = 'processing' WHERE id = ? AND consumption_status = 'pending'",
        )
        .bind(candidate.id)
        .execute(&mut *transaction)
        .await?;
        if claimed.rows_affected() == 0 {
            transaction.commit().await?;
            return Ok(None);
        }
        let record = sqlx::query_as::<_, Self>("SELECT * FROM orchestration_inbox WHERE id = ?")
            .bind(candidate.id)
            .fetch_one(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(Some(record))
    }

    /// Reset claims that belonged to the previous service process. This is an
    /// explicit startup action; normal claimers do not steal live work based
    /// on the event's original receipt timestamp.
    pub async fn reconcile_processing(
        pool: &SqlitePool,
        orchestration_run_id: Option<Uuid>,
    ) -> Result<u64, OrchestrationPersistenceError> {
        let result = if let Some(run_id) = orchestration_run_id {
            sqlx::query(
                "UPDATE orchestration_inbox SET consumption_status = 'pending' WHERE orchestration_run_id = ? AND consumption_status = 'processing'",
            )
            .bind(run_id)
            .execute(pool)
            .await?
        } else {
            sqlx::query(
                "UPDATE orchestration_inbox SET consumption_status = 'pending' WHERE consumption_status = 'processing'",
            )
            .execute(pool)
            .await?
        };
        Ok(result.rows_affected())
    }

    pub async fn mark_consumed(
        pool: &SqlitePool,
        inbox_id: Uuid,
        consumed_at: DateTime<Utc>,
    ) -> Result<bool, OrchestrationPersistenceError> {
        let result = sqlx::query(
            "UPDATE orchestration_inbox SET consumption_status = 'consumed', consumed_at = ? WHERE id = ? AND consumption_status IN ('pending', 'processing')",
        )
        .bind(consumed_at)
        .bind(inbox_id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Return a claimed event to the pending queue when its join cannot yet
    /// make a durable decision. Claims are deliberately short-lived and are
    /// only used to serialize consumers; waiting for another upstream event
    /// must not turn the source fact into a terminally consumed row.
    pub async fn release_claim(
        pool: &SqlitePool,
        inbox_id: Uuid,
    ) -> Result<bool, OrchestrationPersistenceError> {
        let result = sqlx::query(
            "UPDATE orchestration_inbox SET consumption_status = 'pending' WHERE id = ? AND consumption_status = 'processing'",
        )
        .bind(inbox_id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Atomically record a source event's consumption and mark its inbox row
    /// consumed. Repeating the same source/join pair is an acknowledgement,
    /// while a different source event remains independently consumable by an
    /// `all` join target.
    pub async fn consume(
        pool: &SqlitePool,
        inbox_id: Uuid,
        join_node_execution_id: Uuid,
        source_node_execution_id: Uuid,
        target_node_execution_id: Option<Uuid>,
        consumed_at: DateTime<Utc>,
    ) -> Result<bool, OrchestrationPersistenceError> {
        let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
        let inserted = record_consumption_in_transaction(
            &mut transaction,
            inbox_id,
            join_node_execution_id,
            source_node_execution_id,
            target_node_execution_id,
            consumed_at,
        )
        .await?;
        transaction.commit().await?;
        Ok(inserted)
    }

    /// Close the durable consumption loop in one SQLite transaction: consume
    /// the canonical inbox fact, append/project the orchestration decision,
    /// and optionally persist the follow-up AgentRun command.
    pub async fn consume_with_effects(
        pool: &SqlitePool,
        inbox_id: Uuid,
        join_node_execution_id: Uuid,
        source_node_execution_id: Uuid,
        target_node_execution_id: Option<Uuid>,
        event: &OrchestrationEventEnvelope,
        follow_up: Option<(Uuid, &AgentRunPortCommandEnvelope)>,
        consumed_at: DateTime<Utc>,
    ) -> Result<bool, OrchestrationPersistenceError> {
        let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
        let inbox_run_id: Uuid =
            sqlx::query_scalar("SELECT orchestration_run_id FROM orchestration_inbox WHERE id = ?")
                .bind(inbox_id)
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or(OrchestrationPersistenceError::MissingNodeIdentity)?;
        let inbox_status: String =
            sqlx::query_scalar("SELECT consumption_status FROM orchestration_inbox WHERE id = ?")
                .bind(inbox_id)
                .fetch_one(&mut *transaction)
                .await?;
        if event.orchestration_run_id != inbox_run_id {
            return Err(OrchestrationPersistenceError::EffectRunMismatch);
        }
        if let Some((_, command)) = follow_up {
            if command.orchestration_run_id != Some(inbox_run_id)
                || command.orchestration_node_execution_id != target_node_execution_id
            {
                return Err(OrchestrationPersistenceError::EffectRunMismatch);
            }
        }

        let inserted = record_consumption_in_transaction(
            &mut transaction,
            inbox_id,
            join_node_execution_id,
            source_node_execution_id,
            target_node_execution_id,
            consumed_at,
        )
        .await?;

        // The source/join identity is the durable idempotency boundary. A
        // redelivered inbox row must not append a second event with the same
        // stable join id but a new timestamp, nor enqueue the follow-up
        // command again. The first transaction either committed all effects
        // or none of them, so an existing consumption is a complete no-op.
        if !inserted {
            if inbox_status == "consumed" {
                transaction.commit().await?;
                return Ok(false);
            }
            transaction.commit().await?;
            return Ok(false);
        }
        append_and_project_in_transaction(&mut *transaction, event).await?;
        if let Some((outbox_id, command)) = follow_up {
            enqueue_outbox_in_transaction(&mut *transaction, outbox_id, command).await?;
            // A serial queue item is persisted as a pending target before its
            // Create command is known to be dispatchable. Keep that command
            // durable but invisible to the dispatcher until the queue head is
            // promoted. `promote_next_serial` releases it atomically with the
            // target status transition.
            let serial_each: bool = {
                let (plan_snapshot, join_node_key): (Json<OrchestrationPlanSnapshot>, String) =
                    sqlx::query_as(
                        r#"
                    SELECT runs.plan_snapshot, join_node.node_key
                    FROM orchestration_runs runs
                    JOIN orchestration_node_executions join_node
                      ON join_node.id = ?
                     AND join_node.orchestration_run_id = runs.id
                    WHERE runs.id = ?
                    "#,
                    )
                    .bind(join_node_execution_id)
                    .bind(inbox_run_id)
                    .fetch_one(&mut *transaction)
                    .await?;
                plan_snapshot.0.nodes.iter().any(|node| {
                    node.node_key == join_node_key
                        && node.join == executors::runtime::OrchestrationJoinPolicy::Each
                        && node.each_downstream_execution
                            == executors::runtime::EachDownstreamExecution::Serial
                })
            };
            let target_status: Option<OrchestrationNodeStatus> = if let Some(target_id) =
                target_node_execution_id
            {
                sqlx::query_scalar("SELECT status FROM orchestration_node_executions WHERE id = ?")
                    .bind(target_id)
                    .fetch_optional(&mut *transaction)
                    .await?
            } else {
                None
            };
            if serial_each && target_status == Some(OrchestrationNodeStatus::Pending) {
                sqlx::query(
                    "UPDATE orchestration_outbox SET available_at = '9999-12-31T23:59:59Z' WHERE id = ? AND delivery_status = 'pending'",
                )
                .bind(outbox_id)
                .execute(&mut *transaction)
                .await?;
            }
        }
        transaction.commit().await?;
        Ok(inserted)
    }

    /// Return the highest source sequence consumed for an AgentRun. Runtime
    /// consumers intentionally ingest selected canonical facts (rather than
    /// every thinking/tool event), so a contiguous-prefix cursor would never
    /// advance past the first filtered event. The max sequence is durable and
    /// remains safe to rebuild after a restart.
    pub async fn source_cursor(
        pool: &SqlitePool,
        orchestration_run_id: Uuid,
        source_agent_run_id: Uuid,
    ) -> Result<u64, OrchestrationPersistenceError> {
        let sequence: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(source_sequence) FROM orchestration_inbox WHERE orchestration_run_id = ? AND source_agent_run_id = ? AND consumption_status = 'consumed'",
        )
        .bind(orchestration_run_id)
        .bind(source_agent_run_id)
        .fetch_one(pool)
        .await?;
        Ok(sequence
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(0))
    }
}

impl OrchestrationConsumptionRecord {
    pub async fn source_cursor(
        pool: &SqlitePool,
        orchestration_run_id: Uuid,
        source_agent_run_id: Uuid,
    ) -> Result<u64, OrchestrationPersistenceError> {
        OrchestrationInboxRecord::source_cursor(pool, orchestration_run_id, source_agent_run_id)
            .await
    }

    /// Build the deliberately minimal handoff projection from canonical
    /// storage. No transcript, stderr, diff, workspace metadata, or provider
    /// payload is consulted.
    pub async fn upstream_handoff(
        pool: &SqlitePool,
        join_node_execution_id: Uuid,
        source_node_execution_id: Uuid,
    ) -> Result<UpstreamHandoff, OrchestrationPersistenceError> {
        let row = sqlx::query_as::<_, HandoffProjectionRow>(
            r#"
            SELECT consumption.orchestration_run_id,
                   consumption.source_node_execution_id,
                   consumption.source_agent_run_id,
                   source_node.status AS source_status,
                   turns.input_message,
                   run_state.state_json
            FROM orchestration_consumption consumption
            LEFT JOIN orchestration_node_executions source_node
              ON source_node.id = consumption.source_node_execution_id
             AND source_node.orchestration_run_id = consumption.orchestration_run_id
            JOIN agent_turns turns
              ON turns.agent_run_id = consumption.source_agent_run_id
            JOIN agent_run_state run_state
              ON run_state.agent_run_id = consumption.source_agent_run_id
            WHERE consumption.join_node_execution_id = ?
              AND consumption.source_node_execution_id = ?
            ORDER BY turns.turn_number ASC, turns.created_at ASC, turns.id ASC
            LIMIT 1
            "#,
        )
        .bind(join_node_execution_id)
        .bind(source_node_execution_id)
        .fetch_optional(pool)
        .await?
        .ok_or(OrchestrationPersistenceError::MissingNodeIdentity)?;
        if row.source_status != OrchestrationNodeStatus::Succeeded
            || row.state_json.0.status != AgentRunStatus::Succeeded
        {
            return Err(OrchestrationPersistenceError::NonConsumableHandoff(
                source_node_execution_id,
            ));
        }
        let terminal_output = row.state_json.0.terminal_output.clone().filter(|message| {
            // A handoff is an Agent output, never a user/system message that
            // happened to be marked final by a provider adapter.
            message.role == AgentRuntimeMessageRole::Assistant
        });
        Ok(UpstreamHandoff {
            source_ref: UpstreamSourceReference {
                orchestration_run_id: row.orchestration_run_id,
                node_execution_id: row.source_node_execution_id,
                agent_run_id: row.source_agent_run_id,
            },
            initiating_input: row.input_message.0,
            terminal_output,
        })
    }

    /// Promote exactly one queued `each serial` target. Queue order follows
    /// the frozen source node order, then source sequence and event identity;
    /// it therefore survives process restart and duplicate delivery.
    pub async fn promote_next_serial(
        pool: &SqlitePool,
        join_node_execution_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<SerialEachQueueItem>, OrchestrationPersistenceError> {
        let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
        let run_status: OrchestrationRunStatus = sqlx::query_scalar(
            "SELECT status FROM orchestration_runs WHERE id = (SELECT orchestration_run_id FROM orchestration_node_executions WHERE id = ?)",
        )
        .bind(join_node_execution_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(OrchestrationPersistenceError::MissingNodeIdentity)?;
        if run_status == OrchestrationRunStatus::Cancelling || run_status.is_terminal() {
            transaction.commit().await?;
            return Ok(None);
        }
        let active: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM orchestration_consumption consumption
                JOIN orchestration_node_executions target
                  ON target.id = consumption.target_node_execution_id
                 AND target.orchestration_run_id = consumption.orchestration_run_id
                WHERE consumption.join_node_execution_id = ?
                  AND target.status IN (
                      'ready', 'running', 'awaiting_input',
                      'awaiting_approval', 'cancelling'
                  )
            )
            "#,
        )
        .bind(join_node_execution_id)
        .fetch_one(&mut *transaction)
        .await?;
        if active {
            transaction.commit().await?;
            return Ok(None);
        }

        let next = sqlx::query_as::<_, SerialEachQueueRow>(
            r#"
            SELECT consumption.id AS consumption_id,
                   consumption.source_event_id,
                   inbox.source_sequence,
                   consumption.target_node_execution_id,
                   target.status AS target_status
            FROM orchestration_consumption consumption
            JOIN orchestration_inbox inbox
              ON inbox.source_event_id = consumption.source_event_id
             AND inbox.orchestration_run_id = consumption.orchestration_run_id
            JOIN orchestration_node_executions target
              ON target.id = consumption.target_node_execution_id
             AND target.orchestration_run_id = consumption.orchestration_run_id
            LEFT JOIN orchestration_node_executions source_node
              ON source_node.id = consumption.source_node_execution_id
             AND source_node.orchestration_run_id = consumption.orchestration_run_id
            WHERE consumption.join_node_execution_id = ?
              AND target.status = 'pending'
            ORDER BY COALESCE(source_node.stable_order, 9223372036854775807) ASC,
                     COALESCE(source_node.iteration, 9223372036854775807) ASC,
                     inbox.source_sequence ASC,
                     consumption.source_event_id ASC
            LIMIT 1
            "#,
        )
        .bind(join_node_execution_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(next) = next else {
            transaction.commit().await?;
            return Ok(None);
        };
        let target_node_execution_id = next
            .target_node_execution_id
            .ok_or(OrchestrationPersistenceError::MissingNodeIdentity)?;
        let promoted = sqlx::query(
            "UPDATE orchestration_node_executions SET status = 'ready', updated_at = ? WHERE id = ? AND status = 'pending'",
        )
        .bind(now)
        .bind(target_node_execution_id)
        .execute(&mut *transaction)
        .await?;
        if promoted.rows_affected() == 0 {
            transaction.commit().await?;
            return Ok(None);
        }
        sqlx::query(
            "UPDATE orchestration_outbox SET available_at = ? WHERE node_execution_id = ? AND delivery_status IN ('pending', 'failed')",
        )
        .bind(now)
        .bind(target_node_execution_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(Some(SerialEachQueueItem {
            consumption_id: next.consumption_id,
            source_event_id: next.source_event_id,
            source_sequence: u64::try_from(next.source_sequence).map_err(|_| {
                OrchestrationPersistenceError::InvalidInboxSequence(next.source_event_id)
            })?,
            target_node_execution_id,
            target_status: next.target_status,
        }))
    }
}

async fn record_consumption_in_transaction(
    connection: &mut SqliteConnection,
    inbox_id: Uuid,
    join_node_execution_id: Uuid,
    source_node_execution_id: Uuid,
    target_node_execution_id: Option<Uuid>,
    consumed_at: DateTime<Utc>,
) -> Result<bool, OrchestrationPersistenceError> {
    let inbox = sqlx::query_as::<_, ExistingInboxForConsumption>(
        "SELECT orchestration_run_id, source_event_id, source_agent_run_id, event_envelope FROM orchestration_inbox WHERE id = ?",
    )
    .bind(inbox_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(OrchestrationPersistenceError::MissingNodeIdentity)?;
    if inbox.event_envelope.0.orchestration_run_id != Some(inbox.orchestration_run_id)
        || inbox.event_envelope.0.orchestration_node_execution_id != Some(source_node_execution_id)
    {
        return Err(OrchestrationPersistenceError::InboxNodeMismatch(
            inbox.source_event_id,
        ));
    }
    let inserted = sqlx::query(
        r#"
        INSERT INTO orchestration_consumption (
            id, orchestration_run_id, join_node_execution_id,
            source_node_execution_id, source_agent_run_id, source_event_id,
            target_node_execution_id, consumed_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(join_node_execution_id, source_node_execution_id) DO NOTHING
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(inbox.orchestration_run_id)
    .bind(join_node_execution_id)
    .bind(source_node_execution_id)
    .bind(inbox.source_agent_run_id)
    .bind(inbox.source_event_id)
    .bind(target_node_execution_id)
    .bind(consumed_at)
    .execute(&mut *connection)
    .await?;
    let existing = sqlx::query_as::<_, ExistingConsumption>(
        "SELECT orchestration_run_id, source_agent_run_id, source_event_id, target_node_execution_id FROM orchestration_consumption WHERE join_node_execution_id = ? AND source_node_execution_id = ?",
    )
    .bind(join_node_execution_id)
    .bind(source_node_execution_id)
    .fetch_one(&mut *connection)
    .await?;
    if existing.orchestration_run_id != inbox.orchestration_run_id
        || existing.source_agent_run_id != inbox.source_agent_run_id
        || existing.source_event_id != inbox.source_event_id
        || existing.target_node_execution_id != target_node_execution_id
    {
        return Err(OrchestrationPersistenceError::IdempotencyConflict {
            entity: "orchestration source consumption",
            key: format!("{join_node_execution_id}:{source_node_execution_id}"),
        });
    }
    sqlx::query(
        "UPDATE orchestration_inbox SET consumption_status = 'consumed', consumed_at = ? WHERE id = ? AND consumption_status IN ('pending', 'processing')",
    )
    .bind(consumed_at)
    .bind(inbox_id)
    .execute(&mut *connection)
    .await?;
    Ok(inserted.rows_affected() == 1)
}

impl OrchestrationLeaseRecord {
    /// Release dispatcher ownership left by the previous local service
    /// process. Leases protect orchestration workers only; they are not an
    /// AgentRun lifetime and recovering them must never cancel, fail, or
    /// replace an Agent process.
    pub async fn reconcile_startup(
        pool: &SqlitePool,
    ) -> Result<u64, OrchestrationPersistenceError> {
        let result = sqlx::query("DELETE FROM orchestration_leases")
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn acquire(
        pool: &SqlitePool,
        resource_kind: &str,
        resource_id: Uuid,
        owner_id: &str,
        now: DateTime<Utc>,
        ttl: Duration,
    ) -> Result<bool, OrchestrationPersistenceError> {
        let expires_at = now + ttl;
        let result = sqlx::query(
            r#"
            INSERT INTO orchestration_leases (
                resource_kind, resource_id, owner_id, fencing_token,
                acquired_at, expires_at, updated_at
            ) VALUES (?, ?, ?, 1, ?, ?, ?)
            ON CONFLICT(resource_kind, resource_id) DO UPDATE SET
                owner_id = excluded.owner_id,
                fencing_token = orchestration_leases.fencing_token + 1,
                expires_at = excluded.expires_at,
                updated_at = excluded.updated_at
            WHERE orchestration_leases.expires_at <= excluded.updated_at
               OR orchestration_leases.owner_id = excluded.owner_id
            "#,
        )
        .bind(resource_kind)
        .bind(resource_id)
        .bind(owner_id)
        .bind(now)
        .bind(expires_at)
        .bind(now)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn release(
        pool: &SqlitePool,
        resource_kind: &str,
        resource_id: Uuid,
        owner_id: &str,
    ) -> Result<bool, OrchestrationPersistenceError> {
        let result = sqlx::query(
            "DELETE FROM orchestration_leases WHERE resource_kind = ? AND resource_id = ? AND owner_id = ?",
        )
        .bind(resource_kind)
        .bind(resource_id)
        .bind(owner_id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}

#[derive(Debug, FromRow)]
struct ExistingOrchestrationRun {
    id: Uuid,
    request_id: Uuid,
    correlation_id: Uuid,
    plan_snapshot: Json<OrchestrationPlanSnapshot>,
}

#[derive(Debug, FromRow)]
struct ExistingNodeExecution {
    id: Uuid,
    stable_order: i64,
}

#[derive(Debug, FromRow)]
struct ExistingOutboxCommand {
    id: Uuid,
    command_envelope: Json<AgentRunPortCommandEnvelope>,
}

#[derive(Debug, FromRow)]
struct ExistingOutboxIdentity {
    id: Uuid,
}

#[derive(Debug, FromRow)]
struct ExistingAgentRunLink {
    id: Uuid,
    orchestration_run_id: Uuid,
    node_execution_id: Uuid,
    agent_run_id: Uuid,
}

#[derive(Debug, FromRow)]
struct ExistingOrchestrationEvent {
    event_envelope: Json<OrchestrationEventEnvelope>,
}

#[derive(Debug, FromRow)]
struct ExistingInboxIdentity {
    id: Uuid,
    orchestration_run_id: Uuid,
    source_agent_run_id: Uuid,
    event_envelope: Json<executors::runtime::AgentEventEnvelope>,
}

#[derive(Debug, FromRow)]
struct ExistingInboxForConsumption {
    orchestration_run_id: Uuid,
    source_event_id: Uuid,
    source_agent_run_id: Uuid,
    event_envelope: Json<executors::runtime::AgentEventEnvelope>,
}

#[derive(Debug, FromRow)]
struct ExistingConsumption {
    orchestration_run_id: Uuid,
    source_agent_run_id: Uuid,
    source_event_id: Uuid,
    target_node_execution_id: Option<Uuid>,
}

#[derive(Debug, FromRow)]
struct HandoffProjectionRow {
    orchestration_run_id: Uuid,
    source_node_execution_id: Uuid,
    source_agent_run_id: Uuid,
    source_status: OrchestrationNodeStatus,
    input_message: Json<executors::runtime::CanonicalMessage>,
    state_json: Json<executors::runtime::RunState>,
}

#[derive(Debug, FromRow)]
struct SerialEachQueueRow {
    consumption_id: Uuid,
    source_event_id: Uuid,
    source_sequence: i64,
    target_node_execution_id: Option<Uuid>,
    target_status: OrchestrationNodeStatus,
}

fn enum_string<T: Serialize>(
    field: &'static str,
    value: T,
) -> Result<String, OrchestrationPersistenceError> {
    match serde_json::to_value(value)? {
        serde_json::Value::String(value) => Ok(value),
        _ => Err(OrchestrationPersistenceError::InvalidEnumEncoding(field)),
    }
}

#[cfg(test)]
mod tests {
    use executors::runtime::{
        AgentRunPortCommand, EachDownstreamExecution, ORCHESTRATION_COMMAND_SCHEMA_VERSION,
        ORCHESTRATION_PLAN_SCHEMA_VERSION, OrchestrationEventPayload, OrchestrationFailurePolicy,
        OrchestrationJoinPolicy, OrchestrationPlanNode, OrchestrationProductKind,
        OrchestrationReducerApply, OrchestrationRetryPolicy, RemainingUpstreamsPolicy,
        WorkspaceMode,
    };
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

    use super::*;

    async fn setup_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        initialize_test_schema(&pool).await;
        pool
    }

    async fn initialize_test_schema(pool: &SqlitePool) {
        sqlx::raw_sql(
            r#"
            CREATE TABLE workspaces (id BLOB PRIMARY KEY);
            CREATE TABLE sessions (
                id BLOB PRIMARY KEY,
                workspace_id BLOB NOT NULL,
                FOREIGN KEY (workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE
            );
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/20260811000000_agent_runtime_v1.sql"
        ))
        .execute(pool)
        .await
        .expect("create runtime schema");
    }

    async fn setup_file_pool() -> (std::path::PathBuf, SqlitePool) {
        let path =
            std::env::temp_dir().join(format!("vk-orchestration-writer-{}.sqlite", Uuid::new_v4()));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(std::time::Duration::from_secs(5)),
            )
            .await
            .unwrap();
        initialize_test_schema(&pool).await;
        (path, pool)
    }

    #[tokio::test]
    async fn orchestration_start_waits_for_an_independent_sqlite_writer() {
        let (path, pool) = setup_file_pool().await;
        let run_id = Uuid::new_v4();
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let plan = plan();
        {
            let mut writer = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
            sqlx::query("INSERT INTO workspaces (id) VALUES (?)")
                .bind(Uuid::new_v4())
                .execute(&mut *writer)
                .await
                .unwrap();
            let start = OrchestrationRunRecord::persist_before_dispatch(
                &pool,
                run_id,
                request_id,
                "contended-start",
                correlation_id,
                &plan,
            );
            tokio::pin!(start);
            // A deferred reader-to-writer upgrade fails immediately in WAL mode.
            // The launch must wait at BEGIN IMMEDIATE before reading its snapshot.
            tokio::select! {
                result = &mut start => panic!("Launch did not wait for the writer: {result:?}"),
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
            }
            writer.commit().await.unwrap();
            assert_eq!(start.await.unwrap(), run_id);
        }
        let replay = OrchestrationRunRecord::persist_before_dispatch(
            &pool,
            run_id,
            request_id,
            "contended-start",
            correlation_id,
            &plan,
        )
        .await
        .unwrap();
        assert_eq!(replay, run_id);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM orchestration_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        pool.close().await;
        drop(pool);
        // SQLite workers can finish releasing their OS handles after pool
        // close on Windows. Cleanup is not the writer-contention assertion.
        if let Err(error) = std::fs::remove_file(&path) {
            eprintln!(
                "Retained temporary test database {}: {error}",
                path.display()
            );
        }
    }

    fn plan() -> OrchestrationPlanSnapshot {
        OrchestrationPlanSnapshot {
            schema_version: ORCHESTRATION_PLAN_SCHEMA_VERSION,
            plan_id: Uuid::new_v4(),
            source_definition_id: Uuid::new_v4(),
            source_definition_version: "1".to_string(),
            product_kind: OrchestrationProductKind::Workflow,
            workspace_mode: WorkspaceMode::SharedWorkspace,
            nodes: vec![OrchestrationPlanNode {
                node_key: "implement".to_string(),
                stable_order: 0,
                dependencies: Vec::new(),
                join: OrchestrationJoinPolicy::All,
                failure_policy: OrchestrationFailurePolicy::FailFast,
                remaining_upstreams: RemainingUpstreamsPolicy::Continue,
                each_downstream_execution: EachDownstreamExecution::Parallel,
                retry: OrchestrationRetryPolicy::default(),
                runtime_profile_id: Some("codex:default".to_string()),
                provider_id: Some("codex".to_string()),
                provider_config: None,
            }],
            created_at: Utc::now(),
        }
    }

    async fn link_test_agent_run(
        pool: &SqlitePool,
        orchestration_run_id: Uuid,
        node_execution_id: Uuid,
    ) -> Uuid {
        let workspace_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let agent_run_id = Uuid::new_v4();
        sqlx::query("INSERT INTO workspaces (id) VALUES (?)")
            .bind(workspace_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sessions (id, workspace_id) VALUES (?, ?)")
            .bind(session_id)
            .bind(workspace_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            INSERT INTO agent_runs (
                id, session_id, workspace_id, request_id, idempotency_key,
                correlation_id, schema_version, payload_version,
                runtime_profile_id, provider_id, workspace_mode,
                workspace_path, request_envelope
            ) VALUES (?, ?, ?, ?, ?, ?, 1, 1, 'codex:default', 'codex',
                      'shared_workspace', '.', '{}')
            "#,
        )
        .bind(agent_run_id)
        .bind(session_id)
        .bind(workspace_id)
        .bind(Uuid::new_v4())
        .bind(format!("test-agent-run-{agent_run_id}"))
        .bind(Uuid::new_v4())
        .execute(pool)
        .await
        .unwrap();
        OrchestrationAgentRunLinkRecord::persist(
            pool,
            Uuid::new_v4(),
            orchestration_run_id,
            node_execution_id,
            agent_run_id,
            &format!("test-link-{agent_run_id}"),
        )
        .await
        .unwrap();
        agent_run_id
    }

    #[tokio::test]
    async fn run_and_node_identity_are_idempotent_before_dispatch() {
        let pool = setup_pool().await;
        let first_proposed_run_id = Uuid::new_v4();
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let plan = plan();
        let first = OrchestrationRunRecord::persist_before_dispatch(
            &pool,
            first_proposed_run_id,
            request_id,
            "workflow-dispatch-1",
            correlation_id,
            &plan,
        )
        .await
        .unwrap();
        let repeated = OrchestrationRunRecord::persist_before_dispatch(
            &pool,
            first_proposed_run_id,
            request_id,
            "workflow-dispatch-1",
            correlation_id,
            &plan,
        )
        .await
        .unwrap();
        assert_eq!(first, repeated);

        let proposed_node_id = Uuid::new_v4();
        let first_node = OrchestrationNodeExecutionRecord::persist_identity_before_dispatch(
            &pool,
            proposed_node_id,
            first,
            "implement",
            0,
            0,
        )
        .await
        .unwrap();
        let repeated_node = OrchestrationNodeExecutionRecord::persist_identity_before_dispatch(
            &pool,
            proposed_node_id,
            first,
            "implement",
            0,
            0,
        )
        .await
        .unwrap();
        assert_eq!(first_node, repeated_node);
        assert!(matches!(
            OrchestrationNodeExecutionRecord::persist_identity_before_dispatch(
                &pool,
                Uuid::new_v4(),
                first,
                "implement",
                0,
                0,
            )
            .await,
            Err(OrchestrationPersistenceError::IdempotencyConflict {
                entity: "orchestration node execution",
                ..
            })
        ));

        let run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM orchestration_runs")
            .fetch_one(&pool)
            .await
            .unwrap();
        let state_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM orchestration_state")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(run_count, 1);
        assert_eq!(state_count, 1);
    }

    #[tokio::test]
    async fn orchestration_idempotency_rejects_identity_drift() {
        let pool = setup_pool().await;
        let plan = plan();
        let idempotency_key = "workflow-dispatch-conflict";
        OrchestrationRunRecord::persist_before_dispatch(
            &pool,
            Uuid::new_v4(),
            Uuid::new_v4(),
            idempotency_key,
            Uuid::new_v4(),
            &plan,
        )
        .await
        .unwrap();

        assert!(matches!(
            OrchestrationRunRecord::persist_before_dispatch(
                &pool,
                Uuid::new_v4(),
                Uuid::new_v4(),
                idempotency_key,
                Uuid::new_v4(),
                &plan,
            )
            .await,
            Err(OrchestrationPersistenceError::IdempotencyConflict {
                entity: "orchestration run",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn outbox_strict_idempotency_and_internal_cancellation_replay() {
        let (path, pool) = setup_file_pool().await;
        let plan = plan();
        let run_id = Uuid::new_v4();
        OrchestrationRunRecord::persist_before_dispatch(
            &pool,
            run_id,
            Uuid::new_v4(),
            "outbox-run",
            Uuid::new_v4(),
            &plan,
        )
        .await
        .unwrap();
        let node_id = Uuid::new_v4();
        OrchestrationNodeExecutionRecord::persist_identity_before_dispatch(
            &pool,
            node_id,
            run_id,
            "implement",
            0,
            0,
        )
        .await
        .unwrap();
        let agent_run_id = link_test_agent_run(&pool, run_id, node_id).await;

        let command = AgentRunPortCommandEnvelope {
            schema_version: ORCHESTRATION_COMMAND_SCHEMA_VERSION,
            command_id: Uuid::new_v4(),
            idempotency_key: "cancel-command".to_string(),
            agent_run_id,
            orchestration_run_id: Some(run_id),
            orchestration_node_execution_id: Some(node_id),
            correlation_id: Uuid::new_v4(),
            created_at: Utc::now(),
            command: AgentRunPortCommand::Cancel {
                reason: "user requested".to_string(),
            },
        };
        let outbox_id = Uuid::new_v4();
        let first = OrchestrationOutboxRecord::enqueue(&pool, outbox_id, &command)
            .await
            .unwrap();
        let repeated = OrchestrationOutboxRecord::enqueue(&pool, Uuid::new_v4(), &command)
            .await
            .unwrap();
        assert_eq!(first, repeated);

        let mut conflicting = command.clone();
        conflicting.command = AgentRunPortCommand::Cancel {
            reason: "different reason".to_string(),
        };
        assert!(matches!(
            OrchestrationOutboxRecord::enqueue(&pool, Uuid::new_v4(), &conflicting).await,
            Err(OrchestrationPersistenceError::IdempotencyConflict {
                entity: "orchestration outbox command",
                ..
            })
        ));

        // Background reconciliation and an explicit stop can race. Their
        // original operation/target are identical, but their diagnostics and
        // clocks differ. Keep exactly one complete accepted Cancel envelope.
        let mut first_cancel = command.clone();
        first_cancel.idempotency_key = "concurrent-internal-cancel".to_string();
        first_cancel.command_id = Uuid::new_v4();
        let mut second_cancel = first_cancel.clone();
        second_cancel.correlation_id = Uuid::new_v4();
        second_cancel.created_at += Duration::seconds(1);
        second_cancel.command = AgentRunPortCommand::Cancel {
            reason: "fail_fast sibling cancellation".to_string(),
        };
        let (first, second) = tokio::join!(
            OrchestrationOutboxRecord::enqueue_cancellation(&pool, Uuid::new_v4(), &first_cancel),
            OrchestrationOutboxRecord::enqueue_cancellation(&pool, Uuid::new_v4(), &second_cancel),
        );
        assert_eq!(first.unwrap(), second.unwrap());
        let rows = sqlx::query_as::<_, ExistingOutboxCommand>(
            "SELECT id, command_envelope FROM orchestration_outbox WHERE idempotency_key = ?",
        )
        .bind(&first_cancel.idempotency_key)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        let original = &rows[0].command_envelope.0;
        assert!(original == &first_cancel || original == &second_cancel);
        first_cancel.agent_run_id = Uuid::new_v4();
        assert!(matches!(
            OrchestrationOutboxRecord::enqueue_cancellation(&pool, Uuid::new_v4(), &first_cancel)
                .await,
            Err(OrchestrationPersistenceError::IdempotencyConflict {
                entity: "orchestration cancellation",
                ..
            })
        ));
        pool.close().await;
        drop(pool);
        if let Err(error) = std::fs::remove_file(&path) {
            eprintln!(
                "Retained temporary test database {}: {error}",
                path.display()
            );
        }
    }

    #[tokio::test]
    async fn all_join_schema_allows_multiple_sources_for_one_target() {
        let pool = setup_pool().await;
        let unique_indexes: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_index_list('orchestration_consumption') WHERE \"unique\" = 1",
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        for index in unique_indexes {
            let columns: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT name FROM pragma_index_info('{index}') ORDER BY seqno"
            ))
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_ne!(
                columns,
                vec![
                    "join_node_execution_id".to_string(),
                    "target_node_execution_id".to_string()
                ],
                "all-join sources must be allowed to share one downstream target"
            );
        }
    }

    #[tokio::test]
    async fn inbox_claim_can_be_released_without_consuming_the_source_fact() {
        let pool = setup_pool().await;
        let run_id = Uuid::new_v4();
        OrchestrationRunRecord::persist_before_dispatch(
            &pool,
            run_id,
            Uuid::new_v4(),
            "release-claim-run",
            Uuid::new_v4(),
            &plan(),
        )
        .await
        .unwrap();

        // Ingestion normally enforces the AgentRun/link/event foreign keys.
        // This fixture only exercises the claim state transition, so keep the
        // row minimal and disable FK checks while inserting it.
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&pool)
            .await
            .unwrap();
        let inbox_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO orchestration_inbox (id, orchestration_run_id, source_event_id, source_agent_run_id, source_sequence, event_envelope, consumption_status) VALUES (?, ?, ?, ?, 1, '{}', 'processing')",
        )
        .bind(inbox_id)
        .bind(run_id)
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();

        assert!(
            OrchestrationInboxRecord::release_claim(&pool, inbox_id)
                .await
                .unwrap()
        );
        let status: String =
            sqlx::query_scalar("SELECT consumption_status FROM orchestration_inbox WHERE id = ?")
                .bind(inbox_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "pending");
        assert!(
            !OrchestrationInboxRecord::release_claim(&pool, inbox_id)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn serial_each_queue_promotes_one_item_and_resumes_after_terminal() {
        let pool = setup_pool().await;
        let run_id = Uuid::new_v4();
        let plan = plan();
        OrchestrationRunRecord::persist_before_dispatch(
            &pool,
            run_id,
            Uuid::new_v4(),
            "serial-queue-run",
            Uuid::new_v4(),
            &plan,
        )
        .await
        .unwrap();
        let join_id = OrchestrationNodeExecutionRecord::persist_identity_before_dispatch(
            &pool,
            Uuid::new_v4(),
            run_id,
            "implement",
            0,
            0,
        )
        .await
        .unwrap();
        let first_target = OrchestrationNodeExecutionRecord::persist_identity_before_dispatch(
            &pool,
            Uuid::new_v4(),
            run_id,
            "implement",
            1,
            0,
        )
        .await
        .unwrap();
        let second_target = OrchestrationNodeExecutionRecord::persist_identity_before_dispatch(
            &pool,
            Uuid::new_v4(),
            run_id,
            "implement",
            2,
            0,
        )
        .await
        .unwrap();

        // Queue promotion only reads durable consumption/inbox facts. Keep
        // this fixture focused by disabling FK checks while inserting those
        // source facts; production ingestion validates the same references.
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&pool)
            .await
            .unwrap();
        for (inbox_id, event_id, source_sequence, target_id) in [
            (Uuid::new_v4(), Uuid::new_v4(), 10_i64, first_target),
            (Uuid::new_v4(), Uuid::new_v4(), 20_i64, second_target),
        ] {
            let source_node = Uuid::new_v4();
            let source_agent_run = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO orchestration_inbox (id, orchestration_run_id, source_event_id, source_agent_run_id, source_sequence, event_envelope) VALUES (?, ?, ?, ?, ?, '{}')",
            )
            .bind(inbox_id)
            .bind(run_id)
            .bind(event_id)
            .bind(source_agent_run)
            .bind(source_sequence)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO orchestration_consumption (id, orchestration_run_id, join_node_execution_id, source_node_execution_id, source_agent_run_id, source_event_id, target_node_execution_id) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(Uuid::new_v4())
            .bind(run_id)
            .bind(join_id)
            .bind(source_node)
            .bind(source_agent_run)
            .bind(event_id)
            .bind(target_id)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();

        let promoted =
            OrchestrationConsumptionRecord::promote_next_serial(&pool, join_id, Utc::now())
                .await
                .unwrap()
                .expect("first queue item should be promoted");
        assert_eq!(promoted.target_node_execution_id, first_target);
        assert!(
            OrchestrationConsumptionRecord::promote_next_serial(&pool, join_id, Utc::now())
                .await
                .unwrap()
                .is_none(),
            "a serial queue must not promote a second active item"
        );
        sqlx::query("UPDATE orchestration_node_executions SET status = 'succeeded' WHERE id = ?")
            .bind(first_target)
            .execute(&pool)
            .await
            .unwrap();
        let resumed =
            OrchestrationConsumptionRecord::promote_next_serial(&pool, join_id, Utc::now())
                .await
                .unwrap()
                .expect("second queue item should resume after terminal state");
        assert_eq!(resumed.target_node_execution_id, second_target);
    }

    #[tokio::test]
    async fn append_event_projects_state_and_replays_idempotently() {
        let pool = setup_pool().await;
        let plan = plan();
        let run_id = Uuid::new_v4();
        OrchestrationRunRecord::persist_before_dispatch(
            &pool,
            run_id,
            Uuid::new_v4(),
            "event-project-run",
            Uuid::new_v4(),
            &plan,
        )
        .await
        .unwrap();
        let event = OrchestrationEventEnvelope {
            schema_version: 1,
            payload_version: 1,
            event_id: Uuid::new_v4(),
            orchestration_run_id: run_id,
            sequence: 1,
            correlation_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            payload: OrchestrationEventPayload::LifecycleChanged {
                status: OrchestrationRunStatus::Running,
            },
        };
        assert_eq!(
            OrchestrationEventRecord::append_and_project(&pool, &event)
                .await
                .unwrap(),
            OrchestrationReducerApply::Applied
        );
        assert_eq!(
            OrchestrationEventRecord::append_and_project(&pool, &event)
                .await
                .unwrap(),
            OrchestrationReducerApply::Duplicate
        );
        let status: OrchestrationRunStatus =
            sqlx::query_scalar("SELECT status FROM orchestration_runs WHERE id = ?")
                .bind(run_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, OrchestrationRunStatus::Running);
    }

    #[tokio::test]
    async fn outbox_claim_and_lease_are_restart_safe() {
        let pool = setup_pool().await;
        let plan = plan();
        let run_id = Uuid::new_v4();
        OrchestrationRunRecord::persist_before_dispatch(
            &pool,
            run_id,
            Uuid::new_v4(),
            "outbox-claim-run",
            Uuid::new_v4(),
            &plan,
        )
        .await
        .unwrap();
        let node_id = OrchestrationNodeExecutionRecord::persist_identity_before_dispatch(
            &pool,
            Uuid::new_v4(),
            run_id,
            "implement",
            0,
            0,
        )
        .await
        .unwrap();
        let agent_run_id = link_test_agent_run(&pool, run_id, node_id).await;
        let command = AgentRunPortCommandEnvelope {
            schema_version: 1,
            command_id: Uuid::new_v4(),
            idempotency_key: "claim-command".to_string(),
            agent_run_id,
            orchestration_run_id: Some(run_id),
            orchestration_node_execution_id: Some(node_id),
            correlation_id: Uuid::new_v4(),
            created_at: Utc::now(),
            command: AgentRunPortCommand::Cancel {
                reason: "test".to_string(),
            },
        };
        let outbox_id = OrchestrationOutboxRecord::enqueue(&pool, Uuid::new_v4(), &command)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE orchestration_outbox SET delivery_status = 'delivering', updated_at = '2000-01-01T00:00:00Z' WHERE id = ?",
        )
        .bind(outbox_id)
        .execute(&pool)
        .await
        .unwrap();
        let claimed = OrchestrationOutboxRecord::claim_next(&pool, Utc::now())
            .await
            .unwrap()
            .expect("pending outbox command");
        assert_eq!(claimed.id, outbox_id);
        assert!(
            OrchestrationOutboxRecord::mark_delivered(&pool, outbox_id, Utc::now())
                .await
                .unwrap()
        );
        assert!(
            OrchestrationLeaseRecord::acquire(
                &pool,
                "dispatcher",
                run_id,
                "owner-a",
                Utc::now(),
                Duration::seconds(60),
            )
            .await
            .unwrap()
        );
        assert!(
            !OrchestrationLeaseRecord::acquire(
                &pool,
                "dispatcher",
                run_id,
                "owner-b",
                Utc::now(),
                Duration::seconds(60),
            )
            .await
            .unwrap()
        );
    }
}
