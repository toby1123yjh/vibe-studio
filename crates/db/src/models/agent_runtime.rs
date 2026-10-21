use chrono::{DateTime, Utc};
use executors::{
    actions::ExecutorAction,
    profile::runtime_profile_ids_match,
    runtime::{
        AgentEventEnvelope, AgentRunPortCommand, AgentRunPortCommandEnvelope,
        AgentRunRequestEnvelope, AgentRunStatus, ContractVersionError,
        OrchestrationCommandValidationError, ProjectionStatus, ProviderSessionReference,
        ReducerApply, ReducerError, RunAttemptRequest, RunState, reduce_agent_event,
    },
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool, types::Json};
use thiserror::Error;
use uuid::Uuid;

// Wakeups are hints only: consumers subscribe before reading their durable
// cursor. A bounded process-wide bus is shared by every pool/DB clone; lagging
// consumers recover from SQLite, never from an unbounded in-memory queue.
fn event_changes() -> &'static tokio::sync::broadcast::Sender<Uuid> {
    static CHANGES: std::sync::OnceLock<tokio::sync::broadcast::Sender<Uuid>> =
        std::sync::OnceLock::new();
    CHANGES.get_or_init(|| tokio::sync::broadcast::channel(256).0)
}

pub fn subscribe_agent_event_changes() -> tokio::sync::broadcast::Receiver<Uuid> {
    event_changes().subscribe()
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentProviderSessionRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    pub schema_version: i64,
    pub provider_id: String,
    pub runtime_profile_id: String,
    pub provider_session_id: String,
    pub session_reference: Json<ProviderSessionReference>,
    pub observed_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentRunRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    pub workspace_id: Uuid,
    pub request_id: Uuid,
    pub idempotency_key: String,
    pub correlation_id: Uuid,
    pub schema_version: i64,
    pub payload_version: i64,
    pub runtime_profile_id: String,
    pub provider_id: String,
    pub workspace_mode: String,
    pub workspace_path: String,
    pub status: AgentRunStatus,
    pub projection_status: ProjectionStatus,
    pub request_envelope: Json<AgentRunRequestEnvelope>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentRunWorkspaceSummaryRecord {
    pub agent_run_id: Uuid,
    pub workspace_id: Uuid,
    pub session_id: Uuid,
    pub status: AgentRunStatus,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentTurnRecord {
    pub id: Uuid,
    pub agent_run_id: Uuid,
    pub request_id: Uuid,
    pub turn_number: i64,
    pub intent: String,
    pub input_message: Json<executors::runtime::CanonicalMessage>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentRunAttemptRecord {
    pub id: Uuid,
    pub agent_run_id: Uuid,
    pub turn_id: Uuid,
    pub request_id: Uuid,
    pub idempotency_key: String,
    pub attempt_number: i64,
    pub mode: String,
    pub transport: String,
    pub schema_version: i64,
    pub payload_version: i64,
    pub capability_snapshot: Json<executors::runtime::CapabilitySnapshot>,
    pub request_envelope: Json<RunAttemptRequest>,
    pub status: AgentRunStatus,
    pub projection_status: ProjectionStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentProcessRegistryRecord {
    pub id: Uuid,
    pub run_attempt_id: Uuid,
    pub registry_status: String,
    pub host_endpoint: Option<String>,
    pub host_token: Option<String>,
    pub host_instance_id: Option<String>,
    pub host_pid: Option<i64>,
    pub last_host_event_sequence: i64,
    pub supervisor_instance_id: Option<String>,
    pub pid: Option<i64>,
    pub process_group_id: Option<i64>,
    pub process_started_at: Option<DateTime<Utc>>,
    pub executable: Option<String>,
    pub command_fingerprint: Option<String>,
    pub exit_code: Option<i64>,
    pub observed_exited_at: Option<DateTime<Utc>>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentEventRecord {
    pub event_id: Uuid,
    pub session_id: Uuid,
    pub agent_run_id: Uuid,
    pub turn_id: Uuid,
    pub run_attempt_id: Uuid,
    pub run_attempt_number: i64,
    pub sequence: i64,
    pub correlation_id: Uuid,
    pub schema_version: i64,
    pub payload_version: i64,
    pub event_envelope: Json<AgentEventEnvelope>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentRunStateRecord {
    pub agent_run_id: Uuid,
    pub state_schema_version: i64,
    pub reducer_version: i64,
    pub last_run_attempt_id: Option<Uuid>,
    pub last_run_attempt_number: i64,
    pub last_event_sequence: i64,
    pub last_event_id: Option<Uuid>,
    pub status: AgentRunStatus,
    pub projection_status: ProjectionStatus,
    pub state_json: Json<RunState>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentRunCommandRecord {
    pub command_id: Uuid,
    pub agent_run_id: Uuid,
    pub idempotency_key: String,
    pub command_schema_version: i64,
    pub command_envelope: Json<AgentRunPortCommandEnvelope>,
    pub delivery_status: String,
    pub delivery_attempts: i64,
    pub delivered_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "agent_run_launch_gate_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum AgentRunLaunchGateStatus {
    WaitingSetupStart,
    WaitingSetup,
    Satisfied,
    Failed,
}

#[derive(Debug, Clone, FromRow)]
pub struct AgentRunLaunchGateRecord {
    pub agent_run_id: Uuid,
    pub setup_execution_process_id: Option<Uuid>,
    pub gate_status: AgentRunLaunchGateStatus,
    pub setup_action: Json<ExecutorAction>,
    pub failure_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct NativeAuditStreamRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    pub agent_run_id: Uuid,
    pub run_attempt_id: Uuid,
    pub audit_schema_version: i64,
    pub runtime_version: Option<String>,
    pub protocol_version: Option<String>,
    pub adapter_version: String,
    pub mapper_version: String,
    pub manifest_relative_path: String,
    pub frames_relative_path: String,
    pub first_sequence: Option<i64>,
    pub last_sequence: Option<i64>,
    pub final_checksum: Option<String>,
    pub integrity_status: String,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl AgentRunRecord {
    pub async fn find_latest_for_workspaces(
        pool: &SqlitePool,
        archived: bool,
    ) -> Result<Vec<AgentRunWorkspaceSummaryRecord>, sqlx::Error> {
        sqlx::query_as::<_, AgentRunWorkspaceSummaryRecord>(
            r#"
            WITH ranked_runs AS (
                SELECT ar.id AS agent_run_id,
                       ar.workspace_id,
                       ar.session_id,
                       state.status,
                       state.updated_at,
                       ROW_NUMBER() OVER (
                           PARTITION BY ar.workspace_id
                           ORDER BY ar.created_at DESC, ar.id DESC
                       ) AS row_number
                FROM agent_runs ar
                JOIN agent_run_state state ON state.agent_run_id = ar.id
                JOIN workspaces workspace ON workspace.id = ar.workspace_id
                WHERE workspace.archived = ?
            )
            SELECT agent_run_id, workspace_id, session_id, status, updated_at
            FROM ranked_runs
            WHERE row_number = 1
            "#,
        )
        .bind(archived)
        .fetch_all(pool)
        .await
    }

    pub async fn find_workspaces_awaiting_approval(
        pool: &SqlitePool,
        archived: bool,
    ) -> Result<Vec<Uuid>, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            SELECT DISTINCT ar.workspace_id
            FROM agent_runs ar
            JOIN agent_run_state state ON state.agent_run_id = ar.id
            JOIN workspaces workspace ON workspace.id = ar.workspace_id
            WHERE workspace.archived = ?
              AND state.status = 'awaiting_approval'
            "#,
        )
        .bind(archived)
        .fetch_all(pool)
        .await
    }

    pub async fn find_workspaces_with_unseen(
        pool: &SqlitePool,
        archived: bool,
    ) -> Result<Vec<Uuid>, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            SELECT DISTINCT ar.workspace_id
            FROM agent_runs ar
            JOIN agent_run_state state ON state.agent_run_id = ar.id
            JOIN agent_run_attempts attempt ON attempt.id = state.last_run_attempt_id
            JOIN workspaces workspace ON workspace.id = ar.workspace_id
            LEFT JOIN agent_run_seen seen ON seen.agent_run_id = ar.id
            WHERE workspace.archived = ?
              AND state.status IN (
                  'succeeded', 'failed', 'cancelled', 'crashed', 'audit_failed'
              )
              AND attempt.finished_at IS NOT NULL
              -- SQLite stores some timestamps using its native
              -- `datetime('now', 'subsec')` format (with a space) while
              -- sqlx-bound DateTime values use RFC3339 (with `T`). A raw
              -- lexical comparison therefore makes an acknowledged run
              -- appear unseen forever. Compare parsed Julian timestamps so
              -- both representations have the same ordering semantics.
              AND (
                  seen.agent_run_id IS NULL
                  OR julianday(seen.seen_at) < julianday(attempt.finished_at)
              )
            "#,
        )
        .bind(archived)
        .fetch_all(pool)
        .await
    }

    pub async fn mark_seen_by_workspace_id(
        pool: &SqlitePool,
        workspace_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO agent_run_seen (agent_run_id, seen_at)
            SELECT id, datetime('now', 'subsec')
            FROM agent_runs
            WHERE workspace_id = ?
            ON CONFLICT(agent_run_id) DO UPDATE SET seen_at = excluded.seen_at
            "#,
        )
        .bind(workspace_id)
        .execute(pool)
        .await?;
        Ok(())
    }
}

impl AgentRunLaunchGateRecord {
    pub async fn create(
        pool: &SqlitePool,
        agent_run_id: Uuid,
        setup_action: &ExecutorAction,
    ) -> Result<Self, AgentRuntimePersistenceError> {
        let setup_action_json = serde_json::to_string(setup_action)?;
        sqlx::query(
            r#"
            INSERT INTO agent_run_launch_gates (agent_run_id, setup_action)
            VALUES (?, ?)
            ON CONFLICT(agent_run_id) DO NOTHING
            "#,
        )
        .bind(agent_run_id)
        .bind(&setup_action_json)
        .execute(pool)
        .await?;

        let record = Self::find(pool, agent_run_id).await?.ok_or(
            AgentRuntimePersistenceError::MissingLaunchGate(agent_run_id),
        )?;
        if serde_json::to_string(&record.setup_action.0)? != setup_action_json {
            return Err(AgentRuntimePersistenceError::IdempotencyConflict {
                entity: "agent run launch gate",
                key: agent_run_id.to_string(),
            });
        }
        Ok(record)
    }

    pub async fn find(pool: &SqlitePool, agent_run_id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM agent_run_launch_gates WHERE agent_run_id = ?")
            .bind(agent_run_id)
            .fetch_optional(pool)
            .await
    }

    pub async fn find_by_setup_process(
        pool: &SqlitePool,
        setup_execution_process_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM agent_run_launch_gates WHERE setup_execution_process_id = ?")
            .bind(setup_execution_process_id)
            .fetch_optional(pool)
            .await
    }

    pub async fn find_for_session(
        pool: &SqlitePool,
        session_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as(
            r#"
            SELECT gate.*
            FROM agent_run_launch_gates gate
            JOIN agent_runs run ON run.id = gate.agent_run_id
            WHERE run.session_id = ? AND run.status = 'pending'
            ORDER BY gate.created_at DESC, gate.agent_run_id DESC
            LIMIT 1
            "#,
        )
        .bind(session_id)
        .fetch_optional(pool)
        .await
    }

    pub async fn find_unattached_setup_process(
        &self,
        pool: &SqlitePool,
    ) -> Result<Option<Uuid>, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            SELECT process.id
            FROM execution_processes process
            JOIN agent_runs run ON run.session_id = process.session_id
            WHERE run.id = ?
              AND process.run_reason = 'setupscript'
              AND process.executor_action = ?
              AND julianday(process.created_at) >= julianday(?)
            ORDER BY process.created_at, process.id
            LIMIT 1
            "#,
        )
        .bind(self.agent_run_id)
        .bind(
            serde_json::to_string(&self.setup_action.0)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
        )
        .bind(self.created_at)
        .fetch_optional(pool)
        .await
    }

    pub async fn find_reconcilable(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as(
            r#"
            SELECT gate.*
            FROM agent_run_launch_gates gate
            JOIN agent_runs run ON run.id = gate.agent_run_id
            WHERE run.status = 'pending'
            ORDER BY gate.created_at, gate.agent_run_id
            "#,
        )
        .fetch_all(pool)
        .await
    }

    pub async fn attach_setup_process(
        pool: &SqlitePool,
        agent_run_id: Uuid,
        setup_execution_process_id: Uuid,
    ) -> Result<(), AgentRuntimePersistenceError> {
        let updated = sqlx::query(
            r#"
            UPDATE agent_run_launch_gates
            SET setup_execution_process_id = ?, gate_status = 'waiting_setup',
                updated_at = datetime('now', 'subsec')
            WHERE agent_run_id = ? AND gate_status = 'waiting_setup_start'
            "#,
        )
        .bind(setup_execution_process_id)
        .bind(agent_run_id)
        .execute(pool)
        .await?;
        if updated.rows_affected() == 1 {
            return Ok(());
        }

        let record = Self::find(pool, agent_run_id).await?.ok_or(
            AgentRuntimePersistenceError::MissingLaunchGate(agent_run_id),
        )?;
        if record.setup_execution_process_id == Some(setup_execution_process_id)
            && matches!(
                record.gate_status,
                AgentRunLaunchGateStatus::WaitingSetup | AgentRunLaunchGateStatus::Satisfied
            )
        {
            Ok(())
        } else {
            Err(AgentRuntimePersistenceError::IdentityConflict {
                entity: "agent run launch gate process",
                key: agent_run_id.to_string(),
            })
        }
    }

    pub async fn mark_satisfied(
        pool: &SqlitePool,
        agent_run_id: Uuid,
    ) -> Result<(), AgentRuntimePersistenceError> {
        let updated = sqlx::query(
            r#"
            UPDATE agent_run_launch_gates
            SET gate_status = 'satisfied', failure_message = NULL,
                updated_at = datetime('now', 'subsec')
            WHERE agent_run_id = ? AND gate_status = 'waiting_setup'
            "#,
        )
        .bind(agent_run_id)
        .execute(pool)
        .await?;
        if updated.rows_affected() == 1 {
            return Ok(());
        }

        let record = Self::find(pool, agent_run_id).await?.ok_or(
            AgentRuntimePersistenceError::MissingLaunchGate(agent_run_id),
        )?;
        if record.gate_status == AgentRunLaunchGateStatus::Satisfied {
            Ok(())
        } else {
            Err(AgentRuntimePersistenceError::IdentityConflict {
                entity: "agent run launch gate transition",
                key: agent_run_id.to_string(),
            })
        }
    }

    pub async fn mark_failed(
        pool: &SqlitePool,
        agent_run_id: Uuid,
        message: &str,
    ) -> Result<(), AgentRuntimePersistenceError> {
        let updated = sqlx::query(
            r#"
            UPDATE agent_run_launch_gates
            SET gate_status = 'failed', failure_message = COALESCE(failure_message, ?),
                updated_at = datetime('now', 'subsec')
            WHERE agent_run_id = ?
              AND gate_status IN ('waiting_setup_start', 'waiting_setup')
            "#,
        )
        .bind(message)
        .bind(agent_run_id)
        .execute(pool)
        .await?;
        if updated.rows_affected() == 1 {
            return Ok(());
        }

        let record = Self::find(pool, agent_run_id).await?.ok_or(
            AgentRuntimePersistenceError::MissingLaunchGate(agent_run_id),
        )?;
        if record.gate_status == AgentRunLaunchGateStatus::Failed {
            Ok(())
        } else {
            Err(AgentRuntimePersistenceError::IdentityConflict {
                entity: "agent run launch gate transition",
                key: agent_run_id.to_string(),
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedAgentRunIdentity {
    pub agent_run_id: Uuid,
    pub turn_id: Uuid,
    pub run_attempt_id: Uuid,
    pub process_registry_id: Uuid,
}

#[derive(Debug, Error)]
pub enum AgentRuntimePersistenceError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    InvalidAttempt(#[from] executors::runtime::RunAttemptRequestError),
    #[error(transparent)]
    InvalidCommand(#[from] OrchestrationCommandValidationError),
    #[error(transparent)]
    InvalidVersion(#[from] ContractVersionError),
    #[error("agent run and first run attempt must use attempt_number=1")]
    InvalidFirstAttempt,
    #[error("idempotency key {key:?} was reused with different {entity} identity or payload")]
    IdempotencyConflict { entity: &'static str, key: String },
    #[error("{entity} identity {key:?} conflicts with persisted runtime state")]
    IdentityConflict { entity: &'static str, key: String },
    #[error("runtime contract field {0} did not serialize as a string enum")]
    InvalidEnumEncoding(&'static str),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error(transparent)]
    Reducer(#[from] ReducerError),
    #[error("agent run state row was not found for run {0}")]
    MissingState(Uuid),
    #[error("agent run launch gate was not found for run {0}")]
    MissingLaunchGate(Uuid),
    #[error("create commands must use AgentRunPort::create")]
    InvalidDirectCommand,
}

impl AgentRunCommandRecord {
    pub async fn enqueue(
        pool: &SqlitePool,
        command: &AgentRunPortCommandEnvelope,
    ) -> Result<(Self, bool), AgentRuntimePersistenceError> {
        command.validate_current()?;
        if matches!(command.command, AgentRunPortCommand::Create { .. }) {
            return Err(AgentRuntimePersistenceError::InvalidDirectCommand);
        }
        let command_json = serde_json::to_string(command)?;
        let inserted = sqlx::query(
            r#"
            INSERT INTO agent_run_commands (
                command_id, agent_run_id, idempotency_key,
                command_schema_version, command_envelope
            ) VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(idempotency_key) DO NOTHING
            "#,
        )
        .bind(command.command_id)
        .bind(command.agent_run_id)
        .bind(&command.idempotency_key)
        .bind(i64::from(command.schema_version))
        .bind(command_json)
        .execute(pool)
        .await?;
        let record =
            sqlx::query_as::<_, Self>("SELECT * FROM agent_run_commands WHERE idempotency_key = ?")
                .bind(&command.idempotency_key)
                .fetch_one(pool)
                .await?;
        if record.command_envelope.0 != *command {
            return Err(AgentRuntimePersistenceError::IdempotencyConflict {
                entity: "AgentRun command",
                key: command.idempotency_key.clone(),
            });
        }
        Ok((record, inserted.rows_affected() == 1))
    }

    pub async fn claim(
        pool: &SqlitePool,
        command_id: Uuid,
    ) -> Result<Option<Self>, AgentRuntimePersistenceError> {
        let mut transaction = pool.begin().await?;
        let claimed = sqlx::query(
            r#"
            UPDATE agent_run_commands
            SET delivery_status = 'delivering',
                delivery_attempts = delivery_attempts + 1,
                updated_at = ?
            WHERE command_id = ? AND delivery_status IN ('pending', 'failed')
            "#,
        )
        .bind(Utc::now())
        .bind(command_id)
        .execute(&mut *transaction)
        .await?;
        let record = if claimed.rows_affected() == 1 {
            Some(
                sqlx::query_as::<_, Self>("SELECT * FROM agent_run_commands WHERE command_id = ?")
                    .bind(command_id)
                    .fetch_one(&mut *transaction)
                    .await?,
            )
        } else {
            None
        };
        transaction.commit().await?;
        Ok(record)
    }

    pub async fn reconcile_inflight(
        pool: &SqlitePool,
    ) -> Result<u64, AgentRuntimePersistenceError> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        // An interrupted native answer may already have reached the CLI.
        // Keep its uncertainty durable instead of resetting it to pending.
        sqlx::query(
            "UPDATE agent_conversation_interaction_replies
             SET delivery_status='delivery_unknown',last_error='native reply delivery interrupted',updated_at=?
             WHERE delivery_status='delivering'",
        ).bind(Utc::now()).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE agent_run_commands SET delivery_status='failed',
             last_error='native interaction delivery is unconfirmed after restart',updated_at=?
             WHERE delivery_status='delivering' AND json_extract(command_envelope,'$.command.type')='reply_interaction'",
        ).bind(Utc::now()).execute(&mut *tx).await?;
        let result = sqlx::query(
            "UPDATE agent_run_commands SET delivery_status = 'pending', updated_at = ? WHERE delivery_status = 'delivering'",
        )
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn pending(pool: &SqlitePool) -> Result<Vec<Self>, AgentRuntimePersistenceError> {
        Ok(sqlx::query_as::<_, Self>(
            "SELECT * FROM agent_run_commands WHERE delivery_status = 'pending'
             OR (delivery_status = 'failed' AND COALESCE(json_extract(command_envelope,'$.command.type'),'') <> 'reply_interaction')
             ORDER BY created_at, command_id",
        )
        .fetch_all(pool)
        .await?)
    }

    pub async fn mark_delivered(
        pool: &SqlitePool,
        command_id: Uuid,
    ) -> Result<bool, AgentRuntimePersistenceError> {
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE agent_run_commands SET delivery_status = 'delivered', delivered_at = ?, last_error = NULL, updated_at = ? WHERE command_id = ? AND delivery_status = 'delivering'",
        )
        .bind(now)
        .bind(now)
        .bind(command_id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn mark_failed(
        pool: &SqlitePool,
        command_id: Uuid,
        error: &str,
    ) -> Result<bool, AgentRuntimePersistenceError> {
        let result = sqlx::query(
            "UPDATE agent_run_commands SET delivery_status = 'failed', last_error = ?, updated_at = ? WHERE command_id = ? AND delivery_status = 'delivering'",
        )
        .bind(error)
        .bind(Utc::now())
        .bind(command_id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}

impl AgentRunRecord {
    pub async fn reserve_process_host(
        pool: &SqlitePool,
        run_attempt_id: Uuid,
        endpoint: &str,
        token: &str,
        host_instance_id: Uuid,
    ) -> Result<(), AgentRuntimePersistenceError> {
        let result = sqlx::query(
            r#"
            UPDATE agent_process_registry
            SET host_endpoint = ?, host_token = ?, host_instance_id = ?, updated_at = ?
            WHERE run_attempt_id = ? AND registry_status = 'reserved'
              AND host_endpoint IS NULL AND host_token IS NULL
            "#,
        )
        .bind(endpoint)
        .bind(token)
        .bind(host_instance_id.to_string())
        .bind(Utc::now())
        .bind(run_attempt_id)
        .execute(pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(AgentRuntimePersistenceError::IdentityConflict {
                entity: "agent process host reservation",
                key: run_attempt_id.to_string(),
            });
        }
        Ok(())
    }

    pub async fn mark_process_host_attached(
        pool: &SqlitePool,
        run_attempt_id: Uuid,
        host_pid: u32,
    ) -> Result<(), AgentRuntimePersistenceError> {
        sqlx::query(
            r#"
            UPDATE agent_process_registry
            SET host_pid = ?, updated_at = ?
            WHERE run_attempt_id = ? AND registry_status != 'exited'
            "#,
        )
        .bind(i64::from(host_pid))
        .bind(Utc::now())
        .bind(run_attempt_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Release a process-host reservation when host startup fails before the
    /// provider process is durably running. Clearing the endpoint/token keeps
    /// startup reconciliation from attempting to attach to a dead reservation.
    pub async fn clear_process_host_reservation(
        pool: &SqlitePool,
        run_attempt_id: Uuid,
    ) -> Result<(), AgentRuntimePersistenceError> {
        sqlx::query(
            r#"
            UPDATE agent_process_registry
            SET host_endpoint = NULL,
                host_token = NULL,
                host_instance_id = NULL,
                host_pid = NULL,
                updated_at = ?
            WHERE run_attempt_id = ? AND registry_status = 'reserved'
            "#,
        )
        .bind(Utc::now())
        .bind(run_attempt_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn advance_process_host_cursor(
        pool: &SqlitePool,
        run_attempt_id: Uuid,
        sequence: u64,
    ) -> Result<(), AgentRuntimePersistenceError> {
        let sequence = i64::try_from(sequence).map_err(|error| {
            AgentRuntimePersistenceError::Serialization(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error,
            )))
        })?;
        sqlx::query(
            r#"
            UPDATE agent_process_registry
            SET last_host_event_sequence = MAX(last_host_event_sequence, ?), updated_at = ?
            WHERE run_attempt_id = ?
            "#,
        )
        .bind(sequence)
        .bind(Utc::now())
        .bind(run_attempt_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn mark_process_host_unreachable(
        pool: &SqlitePool,
        run_attempt_id: Uuid,
    ) -> Result<(), AgentRuntimePersistenceError> {
        sqlx::query(
            r#"
            UPDATE agent_process_registry
            SET registry_status = 'unreachable', updated_at = ?
            WHERE run_attempt_id = ? AND registry_status IN ('spawned', 'running', 'unreachable')
            "#,
        )
        .bind(Utc::now())
        .bind(run_attempt_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn persist_identity_before_launch(
        pool: &SqlitePool,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
    ) -> Result<PersistedAgentRunIdentity, AgentRuntimePersistenceError> {
        attempt.validate_for_run(request)?;
        if attempt.attempt_number != 1 {
            return Err(AgentRuntimePersistenceError::InvalidFirstAttempt);
        }

        let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = sqlx::query_as::<_, ExistingAgentRunIdentity>(
            r#"
            SELECT ar.id AS agent_run_id,
                   at.id AS turn_id,
                   ara.id AS run_attempt_id,
                   apr.id AS process_registry_id,
                   ar.request_envelope,
                   ara.request_envelope AS attempt_envelope
            FROM agent_runs ar
            JOIN agent_turns at ON at.agent_run_id = ar.id
            JOIN agent_run_attempts ara
              ON ara.agent_run_id = ar.id AND ara.attempt_number = 1
            JOIN agent_process_registry apr ON apr.run_attempt_id = ara.id
            WHERE ar.idempotency_key = ?
            "#,
        )
        .bind(&request.idempotency_key)
        .fetch_optional(&mut *transaction)
        .await?
        {
            if existing.request_envelope.0 != *request || existing.attempt_envelope.0 != *attempt {
                return Err(AgentRuntimePersistenceError::IdempotencyConflict {
                    entity: "agent run",
                    key: request.idempotency_key.clone(),
                });
            }
            transaction.commit().await?;
            return Ok(existing.into());
        }

        let workspace_mode = enum_string("workspace_mode", request.workspace.mode)?;
        let intent = enum_string("agent_run_intent", request.intent)?;
        let attempt_mode = enum_string("run_attempt_mode", attempt.mode)?;
        let transport = enum_string("agent_transport_kind", attempt.transport)?;
        let request_json = serde_json::to_string(request)?;
        let input_json = serde_json::to_string(&request.input)?;
        let attempt_json = serde_json::to_string(attempt)?;
        let capabilities_json = serde_json::to_string(&attempt.capability_snapshot)?;
        let state = RunState::pending(request);
        let state_json = serde_json::to_string(&state)?;
        let process_registry_id = Uuid::new_v4();

        sqlx::query(
            r#"
            INSERT INTO agent_runs (
                id, session_id, workspace_id, request_id, idempotency_key,
                correlation_id, schema_version, payload_version,
                runtime_profile_id, provider_id, workspace_mode, workspace_path,
                status, projection_status, request_envelope
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(request.agent_run_id)
        .bind(request.session_id)
        .bind(request.workspace.workspace_id)
        .bind(request.request_id)
        .bind(&request.idempotency_key)
        .bind(request.correlation_id)
        .bind(i64::from(request.schema_version))
        .bind(i64::from(request.payload_version))
        .bind(&request.runtime_profile_id)
        .bind(&request.provider_id)
        .bind(workspace_mode)
        .bind(&request.workspace.path)
        .bind(AgentRunStatus::Pending)
        .bind(ProjectionStatus::Current)
        .bind(request_json)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO agent_turns (
                id, agent_run_id, request_id, turn_number, intent, input_message
            ) VALUES (?, ?, ?, 1, ?, ?)
            "#,
        )
        .bind(request.turn_id)
        .bind(request.agent_run_id)
        .bind(request.request_id)
        .bind(intent)
        .bind(input_json)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO agent_run_attempts (
                id, agent_run_id, turn_id, request_id, idempotency_key,
                attempt_number, mode, transport, schema_version, payload_version,
                capability_snapshot, request_envelope, status, projection_status
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(attempt.run_attempt_id)
        .bind(request.agent_run_id)
        .bind(request.turn_id)
        .bind(attempt.request_id)
        .bind(&attempt.idempotency_key)
        .bind(i64::from(attempt.attempt_number))
        .bind(attempt_mode)
        .bind(transport)
        .bind(i64::from(attempt.schema_version))
        .bind(i64::from(attempt.payload_version))
        .bind(capabilities_json)
        .bind(attempt_json)
        .bind(AgentRunStatus::Pending)
        .bind(ProjectionStatus::Current)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO agent_process_registry (id, run_attempt_id, registry_status, updated_at)
            VALUES (?, ?, 'reserved', ?)
            "#,
        )
        .bind(process_registry_id)
        .bind(attempt.run_attempt_id)
        .bind(Utc::now())
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO agent_run_state (
                agent_run_id, state_schema_version, reducer_version,
                last_run_attempt_id, last_run_attempt_number, last_event_sequence,
                status, projection_status, state_json
            ) VALUES (?, ?, ?, NULL, 0, 0, ?, ?, ?)
            "#,
        )
        .bind(request.agent_run_id)
        .bind(i64::from(state.state_schema_version))
        .bind(i64::from(state.reducer_version))
        .bind(state.status)
        .bind(state.projection_status)
        .bind(state_json)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(PersistedAgentRunIdentity {
            agent_run_id: request.agent_run_id,
            turn_id: request.turn_id,
            run_attempt_id: attempt.run_attempt_id,
            process_registry_id,
        })
    }

    /// Persist a subsequent platform retry before starting another process.
    /// The retry intentionally reuses the AgentRun/Turn identities while
    /// allocating a new RunAttempt and process-registry reservation.
    pub async fn persist_retry_attempt_before_launch(
        pool: &SqlitePool,
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
    ) -> Result<PersistedAgentRunIdentity, AgentRuntimePersistenceError> {
        attempt.validate_for_run(request)?;
        if attempt.attempt_number <= 1 {
            return Err(AgentRuntimePersistenceError::InvalidFirstAttempt);
        }

        let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = sqlx::query_as::<_, ExistingAgentRunIdentity>(
            r#"
            SELECT ar.id AS agent_run_id,
                   at.id AS turn_id,
                   ara.id AS run_attempt_id,
                   apr.id AS process_registry_id,
                   ar.request_envelope,
                   ara.request_envelope AS attempt_envelope
            FROM agent_runs ar
            JOIN agent_turns at ON at.agent_run_id = ar.id
            JOIN agent_run_attempts ara ON ara.agent_run_id = ar.id
            JOIN agent_process_registry apr ON apr.run_attempt_id = ara.id
            WHERE ara.idempotency_key = ?
            "#,
        )
        .bind(&attempt.idempotency_key)
        .fetch_optional(&mut *transaction)
        .await?
        {
            if existing.request_envelope.0 != *request || existing.attempt_envelope.0 != *attempt {
                return Err(AgentRuntimePersistenceError::IdempotencyConflict {
                    entity: "run attempt",
                    key: attempt.idempotency_key.clone(),
                });
            }
            transaction.commit().await?;
            return Ok(existing.into());
        }

        let existing_request: Json<AgentRunRequestEnvelope> =
            sqlx::query_scalar("SELECT request_envelope FROM agent_runs WHERE id = ?")
                .bind(request.agent_run_id)
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or(AgentRuntimePersistenceError::MissingState(
                    request.agent_run_id,
                ))?;
        if existing_request.0 != *request {
            return Err(AgentRuntimePersistenceError::IdempotencyConflict {
                entity: "agent run",
                key: request.idempotency_key.clone(),
            });
        }

        let attempt_mode = enum_string("run_attempt_mode", attempt.mode)?;
        let transport = enum_string("agent_transport_kind", attempt.transport)?;
        let attempt_json = serde_json::to_string(attempt)?;
        let capabilities_json = serde_json::to_string(&attempt.capability_snapshot)?;
        let process_registry_id = Uuid::new_v4();

        sqlx::query(
            r#"
            INSERT INTO agent_run_attempts (
                id, agent_run_id, turn_id, request_id, idempotency_key,
                attempt_number, mode, transport, schema_version, payload_version,
                capability_snapshot, request_envelope, status, projection_status
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(attempt.run_attempt_id)
        .bind(request.agent_run_id)
        .bind(request.turn_id)
        .bind(attempt.request_id)
        .bind(&attempt.idempotency_key)
        .bind(i64::from(attempt.attempt_number))
        .bind(attempt_mode)
        .bind(transport)
        .bind(i64::from(attempt.schema_version))
        .bind(i64::from(attempt.payload_version))
        .bind(capabilities_json)
        .bind(attempt_json)
        .bind(AgentRunStatus::Pending)
        .bind(ProjectionStatus::Current)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO agent_process_registry (id, run_attempt_id, registry_status, updated_at) VALUES (?, ?, 'reserved', ?)",
        )
        .bind(process_registry_id)
        .bind(attempt.run_attempt_id)
        .bind(Utc::now())
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(PersistedAgentRunIdentity {
            agent_run_id: request.agent_run_id,
            turn_id: request.turn_id,
            run_attempt_id: attempt.run_attempt_id,
            process_registry_id,
        })
    }

    pub async fn mark_process_started(
        pool: &SqlitePool,
        run_attempt_id: Uuid,
        pid: u32,
        process_group_id: Option<u32>,
        executable: Option<&str>,
        started_at: DateTime<Utc>,
    ) -> Result<(), AgentRuntimePersistenceError> {
        sqlx::query(
            r#"
            UPDATE agent_process_registry
            SET registry_status = 'running', pid = ?, process_group_id = ?,
                process_started_at = ?, executable = ?, updated_at = ?
            WHERE run_attempt_id = ?
              AND registry_status IN ('reserved', 'spawned', 'running', 'unreachable')
            "#,
        )
        .bind(i64::from(pid))
        .bind(process_group_id.map(i64::from))
        .bind(started_at)
        .bind(executable)
        .bind(Utc::now())
        .bind(run_attempt_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn mark_process_exited(
        pool: &SqlitePool,
        run_attempt_id: Uuid,
        exit_code: Option<i64>,
        exited_at: DateTime<Utc>,
    ) -> Result<(), AgentRuntimePersistenceError> {
        sqlx::query(
            r#"
            UPDATE agent_process_registry
            SET registry_status = 'exited', exit_code = ?, observed_exited_at = ?, updated_at = ?
            WHERE run_attempt_id = ?
              AND registry_status IN ('spawned', 'running', 'unreachable')
            "#,
        )
        .bind(exit_code)
        .bind(exited_at)
        .bind(Utc::now())
        .bind(run_attempt_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn mark_projection_degraded(
        pool: &SqlitePool,
        agent_run_id: Uuid,
    ) -> Result<(), AgentRuntimePersistenceError> {
        let state_json: Json<RunState> =
            sqlx::query_scalar("SELECT state_json FROM agent_run_state WHERE agent_run_id = ?")
                .bind(agent_run_id)
                .fetch_optional(pool)
                .await?
                .ok_or(AgentRuntimePersistenceError::MissingState(agent_run_id))?;
        let mut state = state_json.0;
        state.projection_status = ProjectionStatus::ProjectionDegraded;
        let state_json = serde_json::to_string(&state)?;
        sqlx::query(
            "UPDATE agent_run_state SET projection_status = 'projection_degraded', state_json = ?, updated_at = ? WHERE agent_run_id = ?",
        )
        .bind(state_json)
        .bind(Utc::now())
        .bind(agent_run_id)
        .execute(pool)
        .await?;
        sqlx::query(
            "UPDATE agent_runs SET projection_status = 'projection_degraded', updated_at = ? WHERE id = ?",
        )
        .bind(Utc::now())
        .bind(agent_run_id)
        .execute(pool)
        .await?;
        Ok(())
    }
}

impl NativeAuditStreamRecord {
    pub async fn insert_open(
        pool: &SqlitePool,
        manifest: &executors::runtime::NativeAuditManifest,
    ) -> Result<(), sqlx::Error> {
        let index = manifest.index();
        sqlx::query(
            r#"
            INSERT INTO native_audit_streams (
                id, session_id, agent_run_id, run_attempt_id, audit_schema_version,
                runtime_version, protocol_version, adapter_version, mapper_version,
                manifest_relative_path, frames_relative_path, first_sequence, last_sequence,
                final_checksum, integrity_status, created_at, closed_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(run_attempt_id) DO NOTHING
            "#,
        )
        .bind(index.id.unwrap_or(manifest.run_attempt_id))
        .bind(index.session_id)
        .bind(index.agent_run_id)
        .bind(index.run_attempt_id)
        .bind(i64::from(index.audit_schema_version))
        .bind(index.runtime_version)
        .bind(index.protocol_version)
        .bind(index.adapter_version)
        .bind(index.mapper_version)
        .bind(index.manifest_relative_path)
        .bind(index.frames_relative_path)
        .bind(index.first_sequence.map(|value| value as i64))
        .bind(index.last_sequence.map(|value| value as i64))
        .bind(index.final_checksum)
        .bind(index.integrity_status)
        .bind(index.created_at)
        .bind(index.closed_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn finalize(
        pool: &SqlitePool,
        manifest: &executors::runtime::NativeAuditManifest,
    ) -> Result<(), sqlx::Error> {
        let index = manifest.index();
        sqlx::query(
            r#"
            UPDATE native_audit_streams
            SET first_sequence = ?, last_sequence = ?, final_checksum = ?,
                integrity_status = ?, closed_at = ?
            WHERE run_attempt_id = ?
            "#,
        )
        .bind(index.first_sequence.map(|value| value as i64))
        .bind(index.last_sequence.map(|value| value as i64))
        .bind(index.final_checksum)
        .bind(index.integrity_status)
        .bind(index.closed_at)
        .bind(index.run_attempt_id)
        .execute(pool)
        .await?;
        Ok(())
    }
}

impl AgentProviderSessionRecord {
    pub async fn upsert(
        pool: &SqlitePool,
        id: Uuid,
        session_id: Uuid,
        reference: &ProviderSessionReference,
    ) -> Result<(), AgentRuntimePersistenceError> {
        reference.validate_current()?;
        let existing = sqlx::query_as::<_, AgentProviderSessionRecord>(
            r#"
            SELECT id, session_id, schema_version, provider_id, runtime_profile_id,
                   provider_session_id, session_reference, observed_at, created_at, updated_at
            FROM agent_provider_sessions
            WHERE session_id = ?
            LIMIT 1
            "#,
        )
        .bind(session_id)
        .fetch_optional(pool)
        .await?;

        if let Some(existing) = &existing {
            if existing.provider_id != reference.provider_id
                || existing.provider_session_id != reference.provider_session_id
                || !runtime_profile_ids_match(
                    &existing.runtime_profile_id,
                    &reference.runtime_profile_id,
                )
            {
                return Err(AgentRuntimePersistenceError::IdentityConflict {
                    entity: "provider session",
                    key: format!(
                        "{}:{}:{}",
                        reference.provider_id,
                        reference.runtime_profile_id,
                        reference.provider_session_id
                    ),
                });
            }
        }

        let mut stored_reference = reference.clone();
        if let Some(existing) = existing {
            // Retain the original unique key when DEFAULT was spelled differently.
            stored_reference.runtime_profile_id = existing.runtime_profile_id;
            stored_reference.metadata = merge_provider_session_metadata(
                existing.session_reference.0.metadata,
                stored_reference.metadata,
            );
        }
        let reference_json = serde_json::to_string(&stored_reference)?;
        sqlx::query(
            r#"
            INSERT INTO agent_provider_sessions (
                id, session_id, schema_version, provider_id, runtime_profile_id,
                provider_session_id, session_reference, observed_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(session_id, provider_id, runtime_profile_id) DO UPDATE SET
                provider_session_id = excluded.provider_session_id,
                session_reference = excluded.session_reference,
                observed_at = excluded.observed_at,
                updated_at = datetime('now', 'subsec')
            "#,
        )
        .bind(id)
        .bind(session_id)
        .bind(i64::from(reference.schema_version))
        .bind(&reference.provider_id)
        .bind(&stored_reference.runtime_profile_id)
        .bind(&stored_reference.provider_session_id)
        .bind(reference_json)
        .bind(stored_reference.observed_at)
        .execute(pool)
        .await?;
        Ok(())
    }
}

fn merge_provider_session_metadata(
    existing: Option<serde_json::Value>,
    incoming: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    match (existing, incoming) {
        (None, incoming) => incoming,
        (existing, None) => existing,
        (Some(existing), Some(incoming)) => {
            let Some(mut existing_object) = existing.as_object().cloned() else {
                return Some(existing);
            };
            let Some(incoming_object) = incoming.as_object() else {
                return Some(existing);
            };
            let durable_source = existing_object.get("source").cloned();

            for (key, value) in incoming_object {
                existing_object.insert(key.clone(), value.clone());
            }

            // Native adoption metadata is the durable provenance. Provider
            // frames may add observation details, but must not erase the
            // identity/profile/scope fields that make the binding immutable.
            if durable_source
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .is_some_and(|source| source == "native_adopted")
            {
                for key in [
                    "source",
                    "profile_fingerprint",
                    "profile_context",
                    "scope_path",
                    "native_source_scope_path",
                ] {
                    if let Some(value) = existing.get(key) {
                        existing_object.insert(key.to_string(), value.clone());
                    }
                }
            }

            Some(serde_json::Value::Object(existing_object))
        }
    }
}

impl AgentEventRecord {
    pub async fn append(
        pool: &SqlitePool,
        event: &AgentEventEnvelope,
    ) -> Result<bool, AgentRuntimePersistenceError> {
        event.validate_for_projection().map_err(|error| {
            AgentRuntimePersistenceError::Serialization(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error,
            )))
        })?;
        let event_json = serde_json::to_string(event)?;
        let sequence = i64::try_from(event.sequence).map_err(|error| {
            AgentRuntimePersistenceError::Serialization(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error,
            )))
        })?;

        let result = sqlx::query(
            r#"
            INSERT INTO agent_events (
                event_id, session_id, agent_run_id, turn_id, run_attempt_id,
                run_attempt_number, sequence, correlation_id, schema_version,
                payload_version, event_envelope
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(event_id) DO NOTHING
            "#,
        )
        .bind(event.event_id)
        .bind(event.session_id)
        .bind(event.agent_run_id)
        .bind(event.turn_id)
        .bind(event.run_attempt_id)
        .bind(i64::from(event.run_attempt_number))
        .bind(sequence)
        .bind(event.correlation_id)
        .bind(i64::from(event.schema_version))
        .bind(i64::from(event.payload_version))
        .bind(event_json)
        .execute(pool)
        .await?;

        if result.rows_affected() == 1 {
            let _ = event_changes().send(event.agent_run_id);
            return Ok(true);
        }

        let existing: Json<AgentEventEnvelope> =
            sqlx::query_scalar("SELECT event_envelope FROM agent_events WHERE event_id = ?")
                .bind(event.event_id)
                .fetch_one(pool)
                .await?;
        if existing.0 == *event {
            Ok(false)
        } else {
            Err(AgentRuntimePersistenceError::IdempotencyConflict {
                entity: "agent event",
                key: event.event_id.to_string(),
            })
        }
    }

    /// Append a canonical event and advance RunState in one transaction.
    /// Native audit remains the replay source when this projection fails.
    pub async fn append_and_project(
        pool: &SqlitePool,
        event: &AgentEventEnvelope,
    ) -> Result<ReducerApply, AgentRuntimePersistenceError> {
        event.validate_for_projection()?;
        let sequence = i64::try_from(event.sequence).map_err(|error| {
            AgentRuntimePersistenceError::Serialization(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error,
            )))
        })?;
        let event_json = serde_json::to_string(event)?;
        let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;

        if let Some(existing) = sqlx::query_as::<_, ExistingAgentEvent>(
            "SELECT event_envelope FROM agent_events WHERE event_id = ?",
        )
        .bind(event.event_id)
        .fetch_optional(&mut *transaction)
        .await?
        {
            if existing.event_envelope.0 == *event {
                transaction.commit().await?;
                return Ok(ReducerApply::Duplicate);
            }
            return Err(AgentRuntimePersistenceError::IdempotencyConflict {
                entity: "agent event",
                key: event.event_id.to_string(),
            });
        }
        if let Some(existing) = sqlx::query_as::<_, ExistingAgentEvent>(
            "SELECT event_envelope FROM agent_events WHERE run_attempt_id = ? AND sequence = ?",
        )
        .bind(event.run_attempt_id)
        .bind(sequence)
        .fetch_optional(&mut *transaction)
        .await?
        {
            if existing.event_envelope.0 == *event {
                transaction.commit().await?;
                return Ok(ReducerApply::Duplicate);
            }
            return Err(AgentRuntimePersistenceError::IdempotencyConflict {
                entity: "agent event sequence",
                key: format!("{}:{sequence}", event.run_attempt_id),
            });
        }

        let state_json: Json<RunState> =
            sqlx::query_scalar("SELECT state_json FROM agent_run_state WHERE agent_run_id = ?")
                .bind(event.agent_run_id)
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or(AgentRuntimePersistenceError::MissingState(
                    event.agent_run_id,
                ))?;
        let mut state = state_json.0;
        let applied = reduce_agent_event(&mut state, event)?;
        sqlx::query(
            r#"
            INSERT INTO agent_events (
                event_id, session_id, agent_run_id, turn_id, run_attempt_id,
                run_attempt_number, sequence, correlation_id, schema_version,
                payload_version, event_envelope
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event.event_id)
        .bind(event.session_id)
        .bind(event.agent_run_id)
        .bind(event.turn_id)
        .bind(event.run_attempt_id)
        .bind(i64::from(event.run_attempt_number))
        .bind(sequence)
        .bind(event.correlation_id)
        .bind(i64::from(event.schema_version))
        .bind(i64::from(event.payload_version))
        .bind(event_json)
        .execute(&mut *transaction)
        .await?;

        let projected_json = serde_json::to_string(&state)?;
        sqlx::query(
            r#"
            UPDATE agent_run_state
            SET last_run_attempt_id = ?, last_run_attempt_number = ?,
                last_event_sequence = ?, last_event_id = ?, status = ?,
                projection_status = ?, state_json = ?, updated_at = ?
            WHERE agent_run_id = ?
            "#,
        )
        .bind(state.last_run_attempt_id)
        .bind(i64::from(state.last_run_attempt_number))
        .bind(sequence)
        .bind(state.last_event_id)
        .bind(state.status)
        .bind(state.projection_status)
        .bind(projected_json)
        .bind(state.updated_at)
        .bind(event.agent_run_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE agent_runs SET status = ?, projection_status = ?, updated_at = ? WHERE id = ?",
        )
        .bind(state.status)
        .bind(state.projection_status)
        .bind(state.updated_at)
        .bind(event.agent_run_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            UPDATE agent_run_attempts
            SET status = ?, projection_status = ?,
                finished_at = CASE
                    WHEN ? AND finished_at IS NULL THEN ?
                    ELSE finished_at
                END,
                updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(state.status)
        .bind(state.projection_status)
        .bind(state.status.is_terminal())
        .bind(state.updated_at)
        .bind(state.updated_at)
        .bind(event.run_attempt_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        let _ = event_changes().send(event.agent_run_id);
        Ok(applied)
    }
}

#[derive(Debug, FromRow)]
struct ExistingAgentRunIdentity {
    agent_run_id: Uuid,
    turn_id: Uuid,
    run_attempt_id: Uuid,
    process_registry_id: Uuid,
    request_envelope: Json<AgentRunRequestEnvelope>,
    attempt_envelope: Json<RunAttemptRequest>,
}

#[derive(Debug, FromRow)]
struct ExistingAgentEvent {
    event_envelope: Json<AgentEventEnvelope>,
}

impl From<ExistingAgentRunIdentity> for PersistedAgentRunIdentity {
    fn from(value: ExistingAgentRunIdentity) -> Self {
        Self {
            agent_run_id: value.agent_run_id,
            turn_id: value.turn_id,
            run_attempt_id: value.run_attempt_id,
            process_registry_id: value.process_registry_id,
        }
    }
}

fn enum_string<T: Serialize>(
    field: &'static str,
    value: T,
) -> Result<String, AgentRuntimePersistenceError> {
    match serde_json::to_value(value)? {
        serde_json::Value::String(value) => Ok(value),
        _ => Err(AgentRuntimePersistenceError::InvalidEnumEncoding(field)),
    }
}

#[cfg(test)]
mod tests {
    use executors::{
        actions::{
            ExecutorActionType,
            script::{ScriptContext, ScriptRequest, ScriptRequestLanguage},
        },
        runtime::{
            AGENT_EVENT_PAYLOAD_VERSION, AGENT_EVENT_SCHEMA_VERSION, AGENT_REQUEST_PAYLOAD_VERSION,
            AGENT_REQUEST_SCHEMA_VERSION, AgentCapability, AgentEventPayload, AgentRunIntent,
            AgentRuntimeMessageRole, AgentTransportKind, CanonicalMessage, CapabilitySnapshot,
            CapabilitySnapshotEntry, CapabilitySource, CapabilityState, RunAttemptMode,
            WorkspaceMode, WorkspaceReference,
        },
    };
    use sqlx::sqlite::SqlitePoolOptions;

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
            .expect("enable foreign keys");
        sqlx::raw_sql(
            r#"
            CREATE TABLE workspaces (
                id BLOB PRIMARY KEY,
                archived INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE sessions (
                id BLOB PRIMARY KEY,
                workspace_id BLOB NOT NULL,
                FOREIGN KEY (workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE
            );
            CREATE TABLE execution_processes (
                id BLOB PRIMARY KEY,
                session_id BLOB NOT NULL,
                run_reason TEXT NOT NULL,
                executor_action TEXT NOT NULL,
                status TEXT NOT NULL,
                exit_code INTEGER,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );
            "#,
        )
        .execute(&pool)
        .await
        .expect("create anchor schema");
        sqlx::raw_sql(include_str!(
            "../../migrations/20260811000000_agent_runtime_v1.sql"
        ))
        .execute(&pool)
        .await
        .expect("create runtime schema");
        pool
    }

    fn requests(
        session_id: Uuid,
        workspace_id: Uuid,
        idempotency_key: &str,
    ) -> (AgentRunRequestEnvelope, RunAttemptRequest) {
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
            idempotency_key: idempotency_key.to_string(),
            session_id,
            agent_run_id: Uuid::new_v4(),
            turn_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            intent: AgentRunIntent::Initial,
            runtime_profile_id: "CODEX:default".to_string(),
            provider_id: "codex".to_string(),
            workspace: workspace.clone(),
            input: CanonicalMessage {
                message_id: Uuid::new_v4(),
                role: AgentRuntimeMessageRole::User,
                content: "Implement WS1".to_string(),
            },
            created_at: now,
        };
        let attempt = RunAttemptRequest {
            schema_version: AGENT_REQUEST_SCHEMA_VERSION,
            payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
            request_id: Uuid::new_v4(),
            idempotency_key: format!("{idempotency_key}:attempt:1"),
            session_id,
            agent_run_id: request.agent_run_id,
            turn_id: request.turn_id,
            run_attempt_id: Uuid::new_v4(),
            attempt_number: 1,
            correlation_id: request.correlation_id,
            mode: RunAttemptMode::Launch,
            transport: AgentTransportKind::AppServerJsonrpc,
            runtime_profile_id: request.runtime_profile_id.clone(),
            provider_id: request.provider_id.clone(),
            workspace,
            capability_snapshot: CapabilitySnapshot {
                schema_version: 1,
                runtime_profile_id: request.runtime_profile_id.clone(),
                provider_id: request.provider_id.clone(),
                runtime_version: Some("1.0.0".to_string()),
                protocol_version: Some("app-server-v2".to_string()),
                adapter_version: "1".to_string(),
                resolved_at: now,
                capabilities: vec![CapabilitySnapshotEntry {
                    capability: AgentCapability::SessionResume,
                    state: CapabilityState::Native,
                    source: CapabilitySource::VersionProbe,
                    emulation_policy: None,
                    evidence: None,
                }],
            },
            executor_config: executors::profile::ExecutorConfig {
                executor: executors::executors::BaseCodingAgent::Codex,
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

        (request, attempt)
    }

    async fn insert_anchors(pool: &SqlitePool) -> (Uuid, Uuid) {
        let workspace_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
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
        (session_id, workspace_id)
    }

    fn setup_action(script: &str) -> ExecutorAction {
        ExecutorAction::new(
            ExecutorActionType::ScriptRequest(ScriptRequest {
                script: script.to_string(),
                language: ScriptRequestLanguage::Bash,
                context: ScriptContext::SetupScript,
                working_dir: Some("repo".to_string()),
            }),
            None,
        )
    }

    fn event(
        request: &AgentRunRequestEnvelope,
        attempt: &RunAttemptRequest,
        sequence: u64,
    ) -> AgentEventEnvelope {
        AgentEventEnvelope {
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload_version: AGENT_EVENT_PAYLOAD_VERSION,
            event_id: Uuid::new_v4(),
            session_id: request.session_id,
            agent_run_id: request.agent_run_id,
            turn_id: request.turn_id,
            run_attempt_id: attempt.run_attempt_id,
            run_attempt_number: attempt.attempt_number,
            sequence,
            correlation_id: request.correlation_id,
            orchestration_run_id: None,
            orchestration_node_execution_id: None,
            timestamp: Utc::now(),
            native_refs: Vec::new(),
            payload: AgentEventPayload::LifecycleChanged {
                status: AgentRunStatus::Running,
            },
        }
    }

    #[tokio::test]
    async fn provider_session_default_alias_keeps_one_binding_and_original_key() {
        let pool = setup_pool().await;
        let (session_id, _) = insert_anchors(&pool).await;
        let mut reference = ProviderSessionReference {
            schema_version: executors::runtime::PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION,
            provider_id: "codex".to_string(),
            runtime_profile_id: "CODEX:DEFAULT".to_string(),
            provider_session_id: "native-session".to_string(),
            observed_at: Utc::now(),
            metadata: None,
        };
        let id = Uuid::new_v4();
        AgentProviderSessionRecord::upsert(&pool, id, session_id, &reference)
            .await
            .unwrap();
        reference.runtime_profile_id = "CODEX".to_string();
        AgentProviderSessionRecord::upsert(&pool, Uuid::new_v4(), session_id, &reference)
            .await
            .unwrap();
        let rows: Vec<(Uuid, String, Json<ProviderSessionReference>)> = sqlx::query_as("SELECT id, runtime_profile_id, session_reference FROM agent_provider_sessions WHERE session_id = ?")
            .bind(session_id).fetch_all(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, id);
        assert_eq!(rows[0].1, "CODEX:DEFAULT");
        assert_eq!(rows[0].2.0.runtime_profile_id, "CODEX:DEFAULT");
        reference.runtime_profile_id = "CODEX:PLAN".to_string();
        assert!(
            AgentProviderSessionRecord::upsert(&pool, Uuid::new_v4(), session_id, &reference)
                .await
                .is_err()
        );
        reference.runtime_profile_id = "CODEX".to_string();
        reference.provider_session_id = "other-session".to_string();
        assert!(
            AgentProviderSessionRecord::upsert(&pool, Uuid::new_v4(), session_id, &reference)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn persists_all_launch_identity_atomically_and_idempotently() {
        let pool = setup_pool().await;
        let (session_id, workspace_id) = insert_anchors(&pool).await;
        let (request, attempt) = requests(session_id, workspace_id, "dispatch-1");

        let first = AgentRunRecord::persist_identity_before_launch(&pool, &request, &attempt)
            .await
            .expect("persist identity");

        let repeated = AgentRunRecord::persist_identity_before_launch(&pool, &request, &attempt)
            .await
            .expect("repeat identity persistence");

        assert_eq!(first, repeated);
        for table in [
            "agent_runs",
            "agent_turns",
            "agent_run_attempts",
            "agent_process_registry",
            "agent_run_state",
        ] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(count, 1, "{table} should contain one logical identity");
        }

        let registry_status: String =
            sqlx::query_scalar("SELECT registry_status FROM agent_process_registry WHERE id = ?")
                .bind(first.process_registry_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(registry_status, "reserved");
    }

    #[test]
    fn native_adoption_metadata_keeps_immutable_identity_fields_on_observation() {
        let merged = merge_provider_session_metadata(
            Some(serde_json::json!({
                "source": "native_adopted",
                "profile_fingerprint": "sha256:adopted",
                "profile_context": {"model_id": "gpt-5"},
                "scope_path": "C:/vk-worktree",
            })),
            Some(serde_json::json!({
                "source": "provider_observed",
                "profile_fingerprint": "sha256:overwritten",
                "scope_path": "C:/native-source",
                "native_session_title": "Observed title",
            })),
        )
        .expect("merged metadata");

        assert_eq!(merged["source"], "native_adopted");
        assert_eq!(merged["profile_fingerprint"], "sha256:adopted");
        assert_eq!(merged["scope_path"], "C:/vk-worktree");
        assert_eq!(merged["native_session_title"], "Observed title");
    }

    #[tokio::test]
    async fn launch_gate_is_idempotent_and_recovers_its_setup_process() {
        let pool = setup_pool().await;
        let (session_id, workspace_id) = insert_anchors(&pool).await;
        let (request, attempt) = requests(session_id, workspace_id, "setup-gate");
        AgentRunRecord::persist_identity_before_launch(&pool, &request, &attempt)
            .await
            .unwrap();

        let action = setup_action("pnpm install");
        let gate = AgentRunLaunchGateRecord::create(&pool, request.agent_run_id, &action)
            .await
            .unwrap();
        let repeated = AgentRunLaunchGateRecord::create(&pool, request.agent_run_id, &action)
            .await
            .unwrap();
        assert_eq!(repeated.agent_run_id, gate.agent_run_id);
        assert_eq!(
            gate.gate_status,
            AgentRunLaunchGateStatus::WaitingSetupStart
        );

        let conflict = AgentRunLaunchGateRecord::create(
            &pool,
            request.agent_run_id,
            &setup_action("cargo fetch"),
        )
        .await;
        assert!(matches!(
            conflict,
            Err(AgentRuntimePersistenceError::IdempotencyConflict { .. })
        ));

        let setup_process_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO execution_processes (
                id, session_id, run_reason, executor_action, status, exit_code
            ) VALUES (?, ?, 'setupscript', ?, 'running', NULL)
            "#,
        )
        .bind(setup_process_id)
        .bind(session_id)
        .bind(serde_json::to_string(&action).unwrap())
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            gate.find_unattached_setup_process(&pool).await.unwrap(),
            Some(setup_process_id)
        );
        AgentRunLaunchGateRecord::attach_setup_process(
            &pool,
            request.agent_run_id,
            setup_process_id,
        )
        .await
        .unwrap();
        AgentRunLaunchGateRecord::attach_setup_process(
            &pool,
            request.agent_run_id,
            setup_process_id,
        )
        .await
        .unwrap();
        assert_eq!(
            AgentRunLaunchGateRecord::find_by_setup_process(&pool, setup_process_id)
                .await
                .unwrap()
                .unwrap()
                .agent_run_id,
            request.agent_run_id
        );
        assert_eq!(
            AgentRunLaunchGateRecord::find_for_session(&pool, session_id)
                .await
                .unwrap()
                .unwrap()
                .agent_run_id,
            request.agent_run_id
        );

        AgentRunLaunchGateRecord::mark_satisfied(&pool, request.agent_run_id)
            .await
            .unwrap();
        AgentRunLaunchGateRecord::mark_satisfied(&pool, request.agent_run_id)
            .await
            .unwrap();
        let reconciled = AgentRunLaunchGateRecord::find_reconcilable(&pool)
            .await
            .unwrap();
        assert_eq!(reconciled.len(), 1);
        assert_eq!(
            reconciled[0].gate_status,
            AgentRunLaunchGateStatus::Satisfied
        );
    }

    #[tokio::test]
    async fn launch_gate_failure_preserves_the_first_terminal_reason() {
        let pool = setup_pool().await;
        let (session_id, workspace_id) = insert_anchors(&pool).await;
        let (request, attempt) = requests(session_id, workspace_id, "failed-setup-gate");
        AgentRunRecord::persist_identity_before_launch(&pool, &request, &attempt)
            .await
            .unwrap();
        AgentRunLaunchGateRecord::create(
            &pool,
            request.agent_run_id,
            &setup_action("pnpm install"),
        )
        .await
        .unwrap();

        AgentRunLaunchGateRecord::mark_failed(&pool, request.agent_run_id, "first failure")
            .await
            .unwrap();
        AgentRunLaunchGateRecord::mark_failed(&pool, request.agent_run_id, "late failure")
            .await
            .unwrap();

        let gate = AgentRunLaunchGateRecord::find(&pool, request.agent_run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(gate.gate_status, AgentRunLaunchGateStatus::Failed);
        assert_eq!(gate.failure_message.as_deref(), Some("first failure"));
    }

    #[tokio::test]
    async fn idempotency_key_rejects_identity_or_payload_drift() {
        let pool = setup_pool().await;
        let (session_id, workspace_id) = insert_anchors(&pool).await;
        let (request, attempt) = requests(session_id, workspace_id, "dispatch-conflict");
        AgentRunRecord::persist_identity_before_launch(&pool, &request, &attempt)
            .await
            .unwrap();

        let (conflicting_request, conflicting_attempt) =
            requests(session_id, workspace_id, "dispatch-conflict");
        assert!(matches!(
            AgentRunRecord::persist_identity_before_launch(
                &pool,
                &conflicting_request,
                &conflicting_attempt,
            )
            .await,
            Err(AgentRuntimePersistenceError::IdempotencyConflict {
                entity: "agent run",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn event_append_rejects_same_identity_with_different_payload() {
        let pool = setup_pool().await;
        let (session_id, workspace_id) = insert_anchors(&pool).await;
        let (request, attempt) = requests(session_id, workspace_id, "event-conflict");
        AgentRunRecord::persist_identity_before_launch(&pool, &request, &attempt)
            .await
            .unwrap();

        let event = event(&request, &attempt, 1);
        assert!(AgentEventRecord::append(&pool, &event).await.unwrap());
        assert!(!AgentEventRecord::append(&pool, &event).await.unwrap());

        let mut conflicting = event.clone();
        conflicting.payload = AgentEventPayload::LifecycleChanged {
            status: AgentRunStatus::Failed,
        };
        assert!(matches!(
            AgentEventRecord::append(&pool, &conflicting).await,
            Err(AgentRuntimePersistenceError::IdempotencyConflict {
                entity: "agent event",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn canonical_event_append_projects_run_state_atomically() {
        let pool = setup_pool().await;
        let (session_id, workspace_id) = insert_anchors(&pool).await;
        let (request, attempt) = requests(session_id, workspace_id, "event-project");
        AgentRunRecord::persist_identity_before_launch(&pool, &request, &attempt)
            .await
            .unwrap();
        let event = event(&request, &attempt, 1);
        let mut changes = subscribe_agent_event_changes();
        assert_eq!(
            AgentEventRecord::append_and_project(&pool, &event)
                .await
                .unwrap(),
            ReducerApply::Applied
        );
        // Notification is emitted only after commit; a fresh reader observes
        // the exact event/state even when it wakes immediately.
        while changes.recv().await.unwrap() != request.agent_run_id {}
        let persisted: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM agent_events WHERE event_id = ?")
                .bind(event.event_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(persisted, 1);
        assert_eq!(
            AgentEventRecord::append_and_project(&pool, &event)
                .await
                .unwrap(),
            ReducerApply::Duplicate
        );
        let status: AgentRunStatus =
            sqlx::query_scalar("SELECT status FROM agent_runs WHERE id = ?")
                .bind(request.agent_run_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, AgentRunStatus::Running);
        sqlx::raw_sql("CREATE TRIGGER reject_test_event BEFORE INSERT ON agent_events BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;")
            .execute(&pool).await.unwrap();
        let mut rejected = event.clone();
        rejected.event_id = Uuid::new_v4();
        rejected.sequence = 2;
        assert!(
            AgentEventRecord::append_and_project(&pool, &rejected)
                .await
                .is_err()
        );
        while let Ok(changed) = changes.try_recv() {
            assert_ne!(
                changed, request.agent_run_id,
                "duplicate/rollback must not publish a change"
            );
        }
    }

    #[tokio::test]
    async fn unseen_workspace_tracks_unacknowledged_terminal_transition_only() {
        let pool = setup_pool().await;
        let (session_id, workspace_id) = insert_anchors(&pool).await;
        let (request, attempt) = requests(session_id, workspace_id, "unseen-terminal");
        AgentRunRecord::persist_identity_before_launch(&pool, &request, &attempt)
            .await
            .unwrap();

        assert!(
            AgentRunRecord::find_workspaces_with_unseen(&pool, false)
                .await
                .unwrap()
                .is_empty(),
            "pending runs must not be presented as unseen completed work"
        );

        let running = event(&request, &attempt, 1);
        AgentEventRecord::append_and_project(&pool, &running)
            .await
            .unwrap();
        assert!(
            AgentRunRecord::find_workspaces_with_unseen(&pool, false)
                .await
                .unwrap()
                .is_empty(),
            "active runs must not be presented as unseen completed work"
        );

        let mut succeeded = event(&request, &attempt, 2);
        succeeded.payload = AgentEventPayload::LifecycleChanged {
            status: AgentRunStatus::Succeeded,
        };
        AgentEventRecord::append_and_project(&pool, &succeeded)
            .await
            .unwrap();
        assert_eq!(
            AgentRunRecord::find_workspaces_with_unseen(&pool, false)
                .await
                .unwrap(),
            vec![workspace_id]
        );

        let finished_at: DateTime<Utc> =
            sqlx::query_scalar("SELECT finished_at FROM agent_run_attempts WHERE id = ?")
                .bind(attempt.run_attempt_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        AgentRunRecord::mark_seen_by_workspace_id(&pool, workspace_id)
            .await
            .unwrap();
        assert!(
            AgentRunRecord::find_workspaces_with_unseen(&pool, false)
                .await
                .unwrap()
                .is_empty()
        );

        let mut late_running = event(&request, &attempt, 3);
        late_running.payload = AgentEventPayload::LifecycleChanged {
            status: AgentRunStatus::Running,
        };
        assert_eq!(
            AgentEventRecord::append_and_project(&pool, &late_running)
                .await
                .unwrap(),
            ReducerApply::IgnoredTerminalRegression
        );
        let finished_after_late_event: DateTime<Utc> =
            sqlx::query_scalar("SELECT finished_at FROM agent_run_attempts WHERE id = ?")
                .bind(attempt.run_attempt_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(finished_after_late_event, finished_at);
        assert!(
            AgentRunRecord::find_workspaces_with_unseen(&pool, false)
                .await
                .unwrap()
                .is_empty(),
            "late events must not reopen an acknowledged terminal run"
        );
    }

    #[tokio::test]
    async fn event_foreign_keys_reject_cross_run_attempt_identity() {
        let pool = setup_pool().await;
        let (session_id, workspace_id) = insert_anchors(&pool).await;
        let (request_one, attempt_one) = requests(session_id, workspace_id, "event-run-one");
        let (request_two, attempt_two) = requests(session_id, workspace_id, "event-run-two");
        AgentRunRecord::persist_identity_before_launch(&pool, &request_one, &attempt_one)
            .await
            .unwrap();
        AgentRunRecord::persist_identity_before_launch(&pool, &request_two, &attempt_two)
            .await
            .unwrap();

        let mut mismatched = event(&request_one, &attempt_one, 1);
        mismatched.run_attempt_id = attempt_two.run_attempt_id;
        assert!(matches!(
            AgentEventRecord::append(&pool, &mismatched).await,
            Err(AgentRuntimePersistenceError::Database(_))
        ));
    }

    #[tokio::test]
    async fn failed_launch_identity_transaction_leaves_no_partial_rows() {
        let pool = setup_pool().await;
        let (session_id, workspace_id) = insert_anchors(&pool).await;
        let (seed_request, seed_attempt) = requests(session_id, workspace_id, "rollback-seed");
        AgentRunRecord::persist_identity_before_launch(&pool, &seed_request, &seed_attempt)
            .await
            .unwrap();
        let (request, mut attempt) = requests(session_id, workspace_id, "rollback-after-run");
        attempt.idempotency_key = seed_attempt.idempotency_key;

        assert!(matches!(
            AgentRunRecord::persist_identity_before_launch(&pool, &request, &attempt).await,
            Err(AgentRuntimePersistenceError::Database(_))
        ));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn new_statuses_are_first_class_schema_values() {
        let pool = setup_pool().await;
        let (session_id, workspace_id) = insert_anchors(&pool).await;
        let (request, attempt) = requests(session_id, workspace_id, "dispatch-2");
        let identity = AgentRunRecord::persist_identity_before_launch(&pool, &request, &attempt)
            .await
            .unwrap();

        sqlx::query(
            "UPDATE agent_runs SET status = 'awaiting_input', projection_status = 'projection_degraded' WHERE id = ?",
        )
        .bind(identity.agent_run_id)
        .execute(&pool)
        .await
        .expect("persist awaiting/degraded states");
        sqlx::query("UPDATE agent_runs SET status = 'cancelled' WHERE id = ?")
            .bind(identity.agent_run_id)
            .execute(&pool)
            .await
            .expect("persist explicit cancellation");
    }

    #[tokio::test]
    async fn native_audit_finalization_persists_terminal_manifest_and_replays() {
        use executors::runtime::{NativeAuditIntegrityStatus, NativeAuditManifest};

        let pool = setup_pool().await;
        let (session_id, workspace_id) = insert_anchors(&pool).await;
        let (request, attempt) = requests(session_id, workspace_id, "audit-finalization");
        AgentRunRecord::persist_identity_before_launch(&pool, &request, &attempt)
            .await
            .expect("persist identity");
        let mut manifest = NativeAuditManifest {
            audit_schema_version: 1,
            session_id,
            agent_run_id: request.agent_run_id,
            turn_id: request.turn_id,
            run_attempt_id: attempt.run_attempt_id,
            run_attempt_number: 1,
            provider_id: request.provider_id.clone(),
            runtime_profile_id: request.runtime_profile_id.clone(),
            workspace_path: request.workspace.path.clone(),
            runtime_version: None,
            protocol_version: None,
            adapter_version: "adapter-1".to_string(),
            mapper_version: "mapper-1".to_string(),
            frame_count: 0,
            first_sequence: None,
            last_sequence: None,
            final_checksum: None,
            integrity_status: NativeAuditIntegrityStatus::Open,
            created_at: Utc::now(),
            closed_at: None,
            manifest_relative_path: format!("audit/{}/manifest.json", attempt.run_attempt_id),
            frames_relative_path: format!("audit/{}/frames.jsonl", attempt.run_attempt_id),
            raw_content_trusted: true,
        };
        NativeAuditStreamRecord::insert_open(&pool, &manifest)
            .await
            .expect("persist open stream");
        manifest.frame_count = 7;
        manifest.first_sequence = Some(1);
        manifest.last_sequence = Some(7);
        manifest.final_checksum = Some("a".repeat(64));
        manifest.integrity_status = NativeAuditIntegrityStatus::Complete;
        manifest.closed_at = Some(Utc::now());
        for _ in 0..2 {
            NativeAuditStreamRecord::finalize(&pool, &manifest)
                .await
                .expect("finalize and replay against the actual runtime migration");
        }
        let record = sqlx::query_as::<_, NativeAuditStreamRecord>(
            "SELECT * FROM native_audit_streams WHERE run_attempt_id = ?",
        )
        .bind(attempt.run_attempt_id)
        .fetch_one(&pool)
        .await
        .expect("read closed stream");
        assert_eq!(record.session_id, session_id);
        assert_eq!(record.agent_run_id, request.agent_run_id);
        assert_eq!(record.first_sequence, Some(1));
        assert_eq!(record.last_sequence, Some(7));
        assert_eq!(record.final_checksum, manifest.final_checksum);
        assert_eq!(record.integrity_status, "complete");
        assert_eq!(record.created_at, manifest.created_at);
        assert_eq!(record.closed_at, manifest.closed_at);
    }

    #[tokio::test]
    async fn native_audit_terminal_integrity_statuses_are_first_class_schema_values() {
        let pool = setup_pool().await;
        let (session_id, workspace_id) = insert_anchors(&pool).await;
        let (request, attempt) = requests(session_id, workspace_id, "audit-statuses");
        AgentRunRecord::persist_identity_before_launch(&pool, &request, &attempt)
            .await
            .expect("persist identity");

        sqlx::query(
            r#"
            INSERT INTO native_audit_streams (
                id, session_id, agent_run_id, run_attempt_id,
                audit_schema_version, adapter_version, mapper_version,
                manifest_relative_path, frames_relative_path, integrity_status
            ) VALUES (?, ?, ?, ?, 1, 'adapter-1', 'mapper-1', ?, ?, 'audit_failed')
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(session_id)
        .bind(request.agent_run_id)
        .bind(attempt.run_attempt_id)
        .bind(format!(
            "runtime/native-audit/{}/manifest.json",
            attempt.run_attempt_id
        ))
        .bind(format!(
            "runtime/native-audit/{}/frames.jsonl",
            attempt.run_attempt_id
        ))
        .execute(&pool)
        .await
        .expect("persist audit_failed status");

        sqlx::query(
            "UPDATE native_audit_streams SET integrity_status = 'recovered' WHERE run_attempt_id = ?",
        )
        .bind(attempt.run_attempt_id)
        .execute(&pool)
        .await
        .expect("persist recovered status");

        let status: String = sqlx::query_scalar(
            "SELECT integrity_status FROM native_audit_streams WHERE run_attempt_id = ?",
        )
        .bind(attempt.run_attempt_id)
        .fetch_one(&pool)
        .await
        .expect("read recovered status");
        assert_eq!(status, "recovered");
    }

    #[tokio::test]
    async fn runtime_schema_has_no_legacy_execution_foreign_keys() {
        let pool = setup_pool().await;
        let tables = [
            "agent_runs",
            "agent_turns",
            "agent_run_attempts",
            "agent_process_registry",
            "agent_events",
            "orchestration_runs",
            "orchestration_node_executions",
            "orchestration_agent_run_links",
            "orchestration_outbox",
            "orchestration_inbox",
        ];

        for table in tables {
            let foreign_tables: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT \"table\" FROM pragma_foreign_key_list('{table}')"
            ))
            .fetch_all(&pool)
            .await
            .unwrap();
            assert!(
                foreign_tables.iter().all(|target| !matches!(
                    target.as_str(),
                    "execution_processes"
                        | "coding_agent_turns"
                        | "workflow_runs"
                        | "node_executions"
                )),
                "{table} references a legacy runtime table: {foreign_tables:?}"
            );
        }
    }
}
