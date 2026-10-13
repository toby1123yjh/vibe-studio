use std::sync::Arc;

use chrono::{Duration, Utc};
use db::models::orchestration::{
    OrchestrationAgentRunLinkRecord, OrchestrationConsumptionRecord, OrchestrationEventRecord,
    OrchestrationInboxRecord, OrchestrationLeaseRecord, OrchestrationNodeExecutionRecord,
    OrchestrationOutboxRecord, OrchestrationPersistenceError, OrchestrationRunRecord,
    SerialEachQueueItem,
};
use executors::runtime::{
    AgentEventEnvelope, AgentEventPayload, AgentRunPort, AgentRunPortCommand,
    AgentRunPortCommandEnvelope, AgentRunPortError, AgentRunStatus,
    ORCHESTRATION_EVENT_PAYLOAD_VERSION, ORCHESTRATION_EVENT_SCHEMA_VERSION,
    OrchestrationEventEnvelope, OrchestrationEventPayload, OrchestrationFailurePolicy,
    OrchestrationJoinPolicy, OrchestrationNodeStatus, OrchestrationPlanNode,
    OrchestrationPlanSnapshot, OrchestrationReducerApply, OrchestrationRunStatus, RunState,
    UpstreamHandoff,
};
use futures::Stream;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum OrchestrationServiceError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Persistence(#[from] OrchestrationPersistenceError),
    #[error(transparent)]
    Port(#[from] AgentRunPortError),
    #[error("orchestration run {0} has no event state")]
    MissingState(Uuid),
    #[error("orchestration run {0} no longer accepts new dispatches")]
    DispatchStopped(Uuid),
    #[error("orchestration command is missing its run or node origin")]
    MissingCommandOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinDecision {
    Waiting,
    Ready,
    ReadyAndCancelRemaining,
    Failed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StartupReconciliationReport {
    pub recovered_outbox_deliveries: u64,
    pub recovered_inbox_claims: u64,
    pub recovered_leases: u64,
    pub reconciled_runs: u64,
    pub unreachable_agent_runs: u64,
    pub delivery_failures: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InboxDrainReport {
    pub claimed: u64,
    pub consumed: u64,
    pub join_decisions: u64,
    pub cancel_commands_queued: u64,
}

/// Durable orchestration facade. Product layers compile their graph into a
/// frozen plan and interact with AgentRun only through this service/port.
pub struct OrchestrationService<P> {
    pool: SqlitePool,
    port: Arc<P>,
    owner_id: String,
}

impl<P> OrchestrationService<P>
where
    P: AgentRunPort + 'static,
{
    pub fn new(pool: SqlitePool, port: Arc<P>) -> Self {
        Self {
            pool,
            port,
            owner_id: format!("orchestration-{}", Uuid::new_v4()),
        }
    }

    pub async fn start_run(
        &self,
        run_id: Uuid,
        request_id: Uuid,
        idempotency_key: &str,
        correlation_id: Uuid,
        plan: &OrchestrationPlanSnapshot,
    ) -> Result<Uuid, OrchestrationServiceError> {
        let run_id = OrchestrationRunRecord::persist_before_dispatch(
            &self.pool,
            run_id,
            request_id,
            idempotency_key,
            correlation_id,
            plan,
        )
        .await?;
        for node in &plan.nodes {
            OrchestrationNodeExecutionRecord::persist_identity_before_dispatch(
                &self.pool,
                stable_node_execution_id(run_id, &node.node_key, 0),
                run_id,
                &node.node_key,
                0,
                node.stable_order,
            )
            .await?;
        }
        // Replaying the same start command resolves to the existing durable
        // run and must not append another lifecycle event. A terminal run is
        // never reopened by a late/duplicate start request.
        let current_status: OrchestrationRunStatus =
            sqlx::query_scalar("SELECT status FROM orchestration_runs WHERE id = ?")
                .bind(run_id)
                .fetch_one(&self.pool)
                .await?;
        if current_status == OrchestrationRunStatus::Pending {
            self.append_event(
                run_id,
                correlation_id,
                OrchestrationEventPayload::LifecycleChanged {
                    status: OrchestrationRunStatus::Running,
                },
            )
            .await?;
        }
        Ok(run_id)
    }

    pub async fn enqueue_command(
        &self,
        command: AgentRunPortCommandEnvelope,
    ) -> Result<Uuid, OrchestrationServiceError> {
        let orchestration_run_id = command
            .orchestration_run_id
            .ok_or(OrchestrationServiceError::MissingCommandOrigin)?;
        command
            .orchestration_node_execution_id
            .ok_or(OrchestrationServiceError::MissingCommandOrigin)?;
        if matches!(
            &command.command,
            AgentRunPortCommand::Create { .. } | AgentRunPortCommand::Retry { .. }
        ) {
            let status: OrchestrationRunStatus =
                sqlx::query_scalar("SELECT status FROM orchestration_runs WHERE id = ?")
                    .bind(orchestration_run_id)
                    .fetch_one(&self.pool)
                    .await?;
            if status == OrchestrationRunStatus::Cancelling || status.is_terminal() {
                return Err(OrchestrationServiceError::DispatchStopped(
                    orchestration_run_id,
                ));
            }
        }
        let (outbox_id, inserted) =
            if matches!(&command.command, AgentRunPortCommand::Cancel { .. }) {
                OrchestrationOutboxRecord::enqueue_cancellation_with_outcome(
                    &self.pool,
                    Uuid::new_v4(),
                    &command,
                )
                .await?
            } else {
                OrchestrationOutboxRecord::enqueue_with_outcome(
                    &self.pool,
                    Uuid::new_v4(),
                    &command,
                )
                .await?
            };
        if inserted {
            self.append_event(
                orchestration_run_id,
                command.correlation_id,
                OrchestrationEventPayload::CommandQueued {
                    command_id: command.command_id,
                    idempotency_key: command.idempotency_key.clone(),
                },
            )
            .await?;
        }
        Ok(outbox_id)
    }

    pub async fn deliver_next(&self) -> Result<bool, OrchestrationServiceError> {
        let Some(outbox) = OrchestrationOutboxRecord::claim_next(&self.pool, Utc::now()).await?
        else {
            return Ok(false);
        };
        let command = outbox.command_envelope.0.clone();
        let orchestration_run_id = command
            .orchestration_run_id
            .ok_or(OrchestrationServiceError::MissingCommandOrigin)?;
        let orchestration_node_execution_id = command
            .orchestration_node_execution_id
            .ok_or(OrchestrationServiceError::MissingCommandOrigin)?;
        // Cancellation is a durable decision. Commands queued before that
        // decision may still be present in the outbox, but Create/Retry must
        // not start new AgentRuns after the parent entered cancelling (or a
        // terminal state). Mark the stale command resolved without invoking
        // the port; explicit Cancel commands remain deliverable.
        if matches!(
            &command.command,
            AgentRunPortCommand::Create { .. } | AgentRunPortCommand::Retry { .. }
        ) {
            let status: OrchestrationRunStatus =
                sqlx::query_scalar("SELECT status FROM orchestration_runs WHERE id = ?")
                    .bind(orchestration_run_id)
                    .fetch_one(&self.pool)
                    .await?;
            if status == OrchestrationRunStatus::Cancelling || status.is_terminal() {
                OrchestrationOutboxRecord::mark_delivered(&self.pool, outbox.id, Utc::now())
                    .await?;
                return Ok(true);
            }
        }
        let result = match &command.command {
            AgentRunPortCommand::Create { request, attempt } => {
                match self.port.create(request.clone(), attempt.clone()).await {
                    Err(error) => Err(error),
                    Ok(agent_run_id) if agent_run_id != request.agent_run_id => {
                        Err(AgentRunPortError::Rejected(
                            "AgentRunPort returned a different run identity".to_string(),
                        ))
                    }
                    Ok(agent_run_id) => OrchestrationAgentRunLinkRecord::persist(
                        &self.pool,
                        stable_operation_id(
                            orchestration_run_id,
                            orchestration_node_execution_id,
                            "agent-link",
                        ),
                        orchestration_run_id,
                        orchestration_node_execution_id,
                        agent_run_id,
                        &command.idempotency_key,
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| AgentRunPortError::Rejected(error.to_string())),
                }
            }
            AgentRunPortCommand::Cancel { .. }
            | AgentRunPortCommand::SubmitInput { .. }
            | AgentRunPortCommand::ResolveApproval { .. }
            | AgentRunPortCommand::Retry { .. } => self.port.control(command.clone()).await,
        };
        match result {
            Ok(()) => {
                OrchestrationOutboxRecord::mark_delivered(&self.pool, outbox.id, Utc::now())
                    .await?;
                Ok(true)
            }
            Err(error) => {
                OrchestrationOutboxRecord::mark_failed(
                    &self.pool,
                    outbox.id,
                    &error.to_string(),
                    Utc::now() + Duration::seconds(1),
                )
                .await?;
                Err(error.into())
            }
        }
    }

    pub async fn cancel(
        &self,
        orchestration_run_id: Uuid,
        correlation_id: Uuid,
    ) -> Result<OrchestrationReducerApply, OrchestrationServiceError> {
        let current: OrchestrationRunStatus =
            sqlx::query_scalar("SELECT status FROM orchestration_runs WHERE id = ?")
                .bind(orchestration_run_id)
                .fetch_one(&self.pool)
                .await?;
        if current.is_terminal() {
            return Ok(OrchestrationReducerApply::Duplicate);
        }
        let apply = if current == OrchestrationRunStatus::Cancelling {
            OrchestrationReducerApply::Duplicate
        } else {
            self.append_event(
                orchestration_run_id,
                correlation_id,
                OrchestrationEventPayload::LifecycleChanged {
                    status: OrchestrationRunStatus::Cancelling,
                },
            )
            .await?
        };

        let undispatched: Vec<Uuid> = sqlx::query_scalar(
            r#"
            UPDATE orchestration_node_executions
            SET status = 'cancelled', updated_at = ?
            WHERE orchestration_run_id = ?
              AND status IN ('pending', 'ready')
              AND NOT EXISTS (
                  SELECT 1 FROM orchestration_agent_run_links links
                  WHERE links.node_execution_id = orchestration_node_executions.id
              )
            RETURNING id
            "#,
        )
        .bind(Utc::now())
        .bind(orchestration_run_id)
        .fetch_all(&self.pool)
        .await?;
        for node_execution_id in undispatched {
            self.append_event(
                orchestration_run_id,
                correlation_id,
                OrchestrationEventPayload::NodeStatusChanged {
                    node_execution_id,
                    status: OrchestrationNodeStatus::Cancelled,
                },
            )
            .await?;
        }

        let linked_to_cancel: Vec<Uuid> = sqlx::query_scalar(
            r#"
            UPDATE orchestration_node_executions
            SET status = 'cancelling', updated_at = ?
            WHERE orchestration_run_id = ?
              AND status NOT IN ('succeeded', 'failed', 'cancelled', 'cancelling')
              AND EXISTS (
                  SELECT 1 FROM orchestration_agent_run_links links
                  WHERE links.node_execution_id = orchestration_node_executions.id
              )
            RETURNING id
            "#,
        )
        .bind(Utc::now())
        .bind(orchestration_run_id)
        .fetch_all(&self.pool)
        .await?;
        for node_execution_id in linked_to_cancel {
            self.append_event(
                orchestration_run_id,
                correlation_id,
                OrchestrationEventPayload::NodeStatusChanged {
                    node_execution_id,
                    status: OrchestrationNodeStatus::Cancelling,
                },
            )
            .await?;
        }

        let active_children: Vec<(Uuid, Uuid)> = sqlx::query_as(
            r#"
            SELECT links.node_execution_id, links.agent_run_id
            FROM orchestration_agent_run_links links
            JOIN orchestration_node_executions nodes ON nodes.id = links.node_execution_id
            WHERE links.orchestration_run_id = ?
              AND nodes.status NOT IN ('succeeded', 'failed', 'cancelled')
            ORDER BY nodes.stable_order, nodes.iteration
            "#,
        )
        .bind(orchestration_run_id)
        .fetch_all(&self.pool)
        .await?;
        for (node_execution_id, agent_run_id) in active_children {
            let idempotency_key =
                format!("orchestration:{orchestration_run_id}:node:{node_execution_id}:cancel");
            self.enqueue_command(AgentRunPortCommandEnvelope {
                schema_version: executors::runtime::ORCHESTRATION_COMMAND_SCHEMA_VERSION,
                command_id: stable_operation_id(orchestration_run_id, node_execution_id, "cancel"),
                idempotency_key,
                agent_run_id,
                orchestration_run_id: Some(orchestration_run_id),
                orchestration_node_execution_id: Some(node_execution_id),
                correlation_id,
                created_at: Utc::now(),
                command: AgentRunPortCommand::Cancel {
                    reason: "parent orchestration cancelled".to_string(),
                },
            })
            .await?;
        }
        self.refresh_run_projection(orchestration_run_id, correlation_id)
            .await?;
        Ok(apply)
    }

    pub async fn ingest_agent_event(
        &self,
        orchestration_run_id: Uuid,
        event: &AgentEventEnvelope,
    ) -> Result<Uuid, OrchestrationServiceError> {
        Ok(OrchestrationInboxRecord::ingest(
            &self.pool,
            Uuid::new_v4(),
            orchestration_run_id,
            event,
        )
        .await?)
    }

    pub async fn claim_inbox_event(
        &self,
        orchestration_run_id: Uuid,
    ) -> Result<Option<OrchestrationInboxRecord>, OrchestrationServiceError> {
        Ok(OrchestrationInboxRecord::claim_next(&self.pool, orchestration_run_id).await?)
    }

    /// Drain canonical AgentRun facts into the frozen orchestration plan.
    ///
    /// The inbox stores every canonical event, while a join only consumes a
    /// terminal lifecycle fact. This loop is intentionally separate from the
    /// product Workflow planner: it establishes the durable source/join
    /// consumption identity first, so a replay or restart cannot create a
    /// second downstream execution. Product layers may then use their own
    /// graph/workspace policy to dispatch a ready target through the outbox.
    pub async fn drain_inbox_for_run(
        &self,
        orchestration_run_id: Uuid,
    ) -> Result<InboxDrainReport, OrchestrationServiceError> {
        let run_status: OrchestrationRunStatus =
            sqlx::query_scalar("SELECT status FROM orchestration_runs WHERE id = ?")
                .bind(orchestration_run_id)
                .fetch_one(&self.pool)
                .await?;
        let plan: sqlx::types::Json<OrchestrationPlanSnapshot> =
            sqlx::query_scalar("SELECT plan_snapshot FROM orchestration_runs WHERE id = ?")
                .bind(orchestration_run_id)
                .fetch_one(&self.pool)
                .await?;

        let mut report = InboxDrainReport::default();
        if run_status.is_terminal() {
            // Late provider events remain auditable in the canonical stream,
            // but a terminal parent must never reopen or trigger a child.
            while let Some(inbox) = self.claim_inbox_event(orchestration_run_id).await? {
                report.claimed += 1;
                if OrchestrationInboxRecord::mark_consumed(&self.pool, inbox.id, Utc::now()).await?
                {
                    report.consumed += 1;
                }
            }
            return Ok(report);
        }
        loop {
            let Some(inbox) = self.claim_inbox_event(orchestration_run_id).await? else {
                break;
            };
            report.claimed += 1;

            let event = inbox.event_envelope.0.clone();
            let terminal_status = match event.payload {
                AgentEventPayload::LifecycleChanged { status } if status.is_terminal() => status,
                _ => {
                    // Thinking, message, tool, and non-terminal lifecycle
                    // facts remain in the canonical event stream but are not
                    // join inputs. Acknowledging them prevents one noisy
                    // provider stream from starving later terminal facts.
                    if OrchestrationInboxRecord::mark_consumed(&self.pool, inbox.id, Utc::now())
                        .await?
                    {
                        report.consumed += 1;
                    }
                    continue;
                }
            };

            let Some(source_node_execution_id) = event.orchestration_node_execution_id else {
                OrchestrationInboxRecord::mark_consumed(&self.pool, inbox.id, Utc::now()).await?;
                report.consumed += 1;
                continue;
            };
            let Some((source_node_key, source_status)) = sqlx::query_as::<
                _,
                (String, OrchestrationNodeStatus),
            >(
                "SELECT node_key, status FROM orchestration_node_executions WHERE id = ? AND orchestration_run_id = ?",
            )
            .bind(source_node_execution_id)
            .bind(orchestration_run_id)
            .fetch_optional(&self.pool)
            .await?
            else {
                OrchestrationInboxRecord::mark_consumed(&self.pool, inbox.id, Utc::now()).await?;
                report.consumed += 1;
                continue;
            };

            // Failed/cancelled/crashed terminal facts are acknowledged as
            // soon as they arrive. They remain visible in the node projection
            // for join evaluation, but never create an upstream handoff.
            if terminal_status != AgentRunStatus::Succeeded {
                if OrchestrationInboxRecord::mark_consumed(&self.pool, inbox.id, Utc::now()).await?
                {
                    report.consumed += 1;
                }
                continue;
            }

            // A lifecycle event is appended and streamed after the AgentRun
            // reducer commits, but the orchestration node projection may be
            // reconciled by a separate watcher. Do not manufacture a handoff
            // until both canonical projections confirm success.
            if terminal_status == AgentRunStatus::Succeeded {
                let agent_status: Option<AgentRunStatus> =
                    sqlx::query_scalar("SELECT status FROM agent_run_state WHERE agent_run_id = ?")
                        .bind(event.agent_run_id)
                        .fetch_optional(&self.pool)
                        .await?;
                if source_status != OrchestrationNodeStatus::Succeeded
                    || agent_status != Some(AgentRunStatus::Succeeded)
                {
                    OrchestrationInboxRecord::release_claim(&self.pool, inbox.id).await?;
                    break;
                }
            }

            let targets: Vec<&OrchestrationPlanNode> = plan
                .0
                .nodes
                .iter()
                .filter(|node| node.dependencies.iter().any(|key| key == &source_node_key))
                .collect();
            if targets.is_empty() {
                OrchestrationInboxRecord::mark_consumed(&self.pool, inbox.id, Utc::now()).await?;
                report.consumed += 1;
                continue;
            }

            // Evaluate every downstream join before consuming this inbox row.
            // If one target still waits for another upstream, leave the event
            // pending so the next terminal source event can retry the same
            // deterministic decision.
            let mut decisions = Vec::with_capacity(targets.len());
            for target in targets.iter() {
                let statuses = self
                    .join_dependency_statuses(orchestration_run_id, target)
                    .await?;
                let decision = evaluate_join(
                    target.join,
                    target.failure_policy,
                    target.remaining_upstreams,
                    &statuses,
                );
                if decision == JoinDecision::Waiting {
                    OrchestrationInboxRecord::release_claim(&self.pool, inbox.id).await?;
                    break;
                }
                decisions.push((*target, decision));
            }
            if decisions.len() != targets.len() {
                break;
            }

            for (target, decision) in decisions {
                let (join_node_execution_id, target_node_execution_id) = self
                    .ensure_join_target_execution(
                        orchestration_run_id,
                        target,
                        source_node_execution_id,
                    )
                    .await?;
                match decision {
                    JoinDecision::Ready | JoinDecision::ReadyAndCancelRemaining => {
                        if self
                            .consume_source_event(
                                inbox.id,
                                join_node_execution_id,
                                source_node_execution_id,
                                Some(target_node_execution_id),
                            )
                            .await?
                        {
                            report.join_decisions += 1;
                        }
                        if decision == JoinDecision::ReadyAndCancelRemaining {
                            report.cancel_commands_queued += self
                                .cancel_remaining_join_sources(
                                    orchestration_run_id,
                                    join_node_execution_id,
                                    source_node_execution_id,
                                    event.correlation_id,
                                    target,
                                )
                                .await?;
                        }
                    }
                    // Failed/cancelled upstreams participate in join
                    // readiness, but never become a handoff source.
                    JoinDecision::Failed => {
                        OrchestrationInboxRecord::mark_consumed(&self.pool, inbox.id, Utc::now())
                            .await?;
                    }
                    JoinDecision::Waiting => unreachable!("waiting decisions are handled above"),
                }
            }
            report.consumed += 1;
        }
        Ok(report)
    }

    async fn join_dependency_statuses(
        &self,
        orchestration_run_id: Uuid,
        target: &OrchestrationPlanNode,
    ) -> Result<Vec<OrchestrationNodeStatus>, OrchestrationServiceError> {
        let mut statuses = Vec::with_capacity(target.dependencies.len());
        for dependency in &target.dependencies {
            if target.join == OrchestrationJoinPolicy::Each {
                // `each` is a fan-in over every source execution. Looking at
                // only iteration zero causes a later successful iteration to
                // be treated as failed/waiting when the first iteration
                // failed or is still pending. Preserve stable ordering while
                // evaluating all source iterations independently.
                let dependency_statuses: Vec<OrchestrationNodeStatus> = sqlx::query_scalar(
                    "SELECT status FROM orchestration_node_executions WHERE orchestration_run_id = ? AND node_key = ? ORDER BY iteration",
                )
                .bind(orchestration_run_id)
                .bind(dependency)
                .fetch_all(&self.pool)
                .await?;
                if dependency_statuses.is_empty() {
                    statuses.push(OrchestrationNodeStatus::Pending);
                } else {
                    statuses.extend(dependency_statuses);
                }
            } else {
                let status: Option<OrchestrationNodeStatus> = sqlx::query_scalar(
                    "SELECT status FROM orchestration_node_executions WHERE orchestration_run_id = ? AND node_key = ? AND iteration = 0",
                )
                .bind(orchestration_run_id)
                .bind(dependency)
                .fetch_optional(&self.pool)
                .await?;
                statuses.push(status.unwrap_or(OrchestrationNodeStatus::Pending));
            }
        }
        Ok(statuses)
    }

    async fn ensure_join_target_execution(
        &self,
        orchestration_run_id: Uuid,
        target: &OrchestrationPlanNode,
        source_node_execution_id: Uuid,
    ) -> Result<(Uuid, Uuid), OrchestrationServiceError> {
        let join_node_execution_id =
            stable_node_execution_id(orchestration_run_id, &target.node_key, 0);
        let existing: Option<(Option<Uuid>,)> = sqlx::query_as(
            "SELECT target_node_execution_id FROM orchestration_consumption WHERE orchestration_run_id = ? AND join_node_execution_id = ? AND source_node_execution_id = ?",
        )
        .bind(orchestration_run_id)
        .bind(join_node_execution_id)
        .bind(source_node_execution_id)
        .fetch_optional(&self.pool)
        .await?;
        if let Some((Some(target_id),)) = existing {
            return Ok((join_node_execution_id, target_id));
        }

        let iteration = if target.join == OrchestrationJoinPolicy::Each {
            self.each_target_iteration(orchestration_run_id, target, source_node_execution_id)
                .await?
        } else {
            0
        };
        let iteration_u32 = u32::try_from(iteration).map_err(|_| {
            OrchestrationServiceError::Persistence(
                OrchestrationPersistenceError::MissingNodeIdentity,
            )
        })?;
        let target_id = if target.join == OrchestrationJoinPolicy::Each {
            stable_node_execution_id(orchestration_run_id, &target.node_key, iteration_u32)
        } else {
            join_node_execution_id
        };
        let target_id = OrchestrationNodeExecutionRecord::persist_identity_before_dispatch(
            &self.pool,
            target_id,
            orchestration_run_id,
            &target.node_key,
            iteration_u32,
            target.stable_order,
        )
        .await?;
        Ok((join_node_execution_id, target_id))
    }

    /// Assign each downstream execution from the frozen source identity, not
    /// from the number of rows a racing consumer happened to observe. Several
    /// AgentRun watchers can drain the same orchestration run concurrently;
    /// counting existing consumptions would let both source events select
    /// iteration zero. Stable source order/iteration gives every source a
    /// deterministic target identity and is unchanged by replay.
    async fn each_target_iteration(
        &self,
        orchestration_run_id: Uuid,
        target: &OrchestrationPlanNode,
        source_node_execution_id: Uuid,
    ) -> Result<u32, OrchestrationServiceError> {
        let source: (i64, i64, String) = sqlx::query_as(
            "SELECT stable_order, iteration, node_key FROM orchestration_node_executions WHERE id = ? AND orchestration_run_id = ?",
        )
        .bind(source_node_execution_id)
        .bind(orchestration_run_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(OrchestrationServiceError::Persistence(
            OrchestrationPersistenceError::MissingNodeIdentity,
        ))?;

        let source_rows: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT stable_order, iteration, node_key FROM orchestration_node_executions WHERE orchestration_run_id = ?",
        )
        .bind(orchestration_run_id)
        .fetch_all(&self.pool)
        .await?;
        let rank = source_rows
            .into_iter()
            .filter(|(stable_order, iteration, node_key)| {
                target.dependencies.iter().any(|key| key == node_key)
                    && (*stable_order < source.0
                        || (*stable_order == source.0 && *iteration < source.1))
            })
            .count();
        u32::try_from(rank).map_err(|_| {
            OrchestrationServiceError::Persistence(
                OrchestrationPersistenceError::MissingNodeIdentity,
            )
        })
    }

    async fn cancel_remaining_join_sources(
        &self,
        orchestration_run_id: Uuid,
        join_node_execution_id: Uuid,
        source_node_execution_id: Uuid,
        correlation_id: Uuid,
        target: &OrchestrationPlanNode,
    ) -> Result<u64, OrchestrationServiceError> {
        let mut queued = 0;
        for dependency in &target.dependencies {
            let Some((node_execution_id, agent_run_id, status)) = sqlx::query_as::<
                _,
                (Uuid, Option<Uuid>, OrchestrationNodeStatus),
            >(
                "SELECT nodes.id, links.agent_run_id, nodes.status FROM orchestration_node_executions nodes LEFT JOIN orchestration_agent_run_links links ON links.node_execution_id = nodes.id AND links.orchestration_run_id = nodes.orchestration_run_id WHERE nodes.orchestration_run_id = ? AND nodes.node_key = ? AND nodes.iteration = 0",
            )
            .bind(orchestration_run_id)
            .bind(dependency)
            .fetch_optional(&self.pool)
            .await?
            else {
                continue;
            };
            if node_execution_id == source_node_execution_id {
                continue;
            }
            if agent_run_id.is_none()
                && matches!(
                    status,
                    OrchestrationNodeStatus::Pending | OrchestrationNodeStatus::Ready
                )
            {
                let cancelled = sqlx::query(
                    "UPDATE orchestration_node_executions SET status = 'cancelled', updated_at = ? WHERE id = ? AND status IN ('pending', 'ready')",
                )
                .bind(Utc::now())
                .bind(node_execution_id)
                .execute(&self.pool)
                .await?
                .rows_affected()
                    == 1;
                if cancelled {
                    self.append_event(
                        orchestration_run_id,
                        correlation_id,
                        OrchestrationEventPayload::NodeStatusChanged {
                            node_execution_id,
                            status: OrchestrationNodeStatus::Cancelled,
                        },
                    )
                    .await?;
                }
                continue;
            }
            let Some(agent_run_id) = agent_run_id else {
                continue;
            };
            if !matches!(
                status,
                OrchestrationNodeStatus::Running
                    | OrchestrationNodeStatus::AwaitingInput
                    | OrchestrationNodeStatus::AwaitingApproval
            ) {
                continue;
            }
            let transitioned = sqlx::query(
                "UPDATE orchestration_node_executions SET status = 'cancelling', updated_at = ? WHERE id = ? AND status NOT IN ('succeeded', 'failed', 'cancelled', 'cancelling')",
            )
            .bind(Utc::now())
            .bind(node_execution_id)
            .execute(&self.pool)
            .await?
            .rows_affected()
                == 1;
            if !transitioned {
                // Another consumer may have persisted the cancellation
                // transition (or the provider may have reached a terminal
                // state) between the read and this update. Do not append a
                // duplicate node event or enqueue another command.
                continue;
            }
            self.append_event(
                orchestration_run_id,
                correlation_id,
                OrchestrationEventPayload::NodeStatusChanged {
                    node_execution_id,
                    status: OrchestrationNodeStatus::Cancelling,
                },
            )
            .await?;
            let idempotency_key = format!(
                "orchestration:{orchestration_run_id}:join:{join_node_execution_id}:node:{node_execution_id}:cancel_remaining"
            );
            let before: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM orchestration_outbox WHERE idempotency_key = ?",
            )
            .bind(&idempotency_key)
            .fetch_one(&self.pool)
            .await?;
            if before > 0 {
                continue;
            }
            self.enqueue_command(AgentRunPortCommandEnvelope {
                schema_version: executors::runtime::ORCHESTRATION_COMMAND_SCHEMA_VERSION,
                command_id: stable_join_operation_id(
                    orchestration_run_id,
                    join_node_execution_id,
                    node_execution_id,
                    "cancel_remaining",
                ),
                idempotency_key,
                agent_run_id,
                orchestration_run_id: Some(orchestration_run_id),
                orchestration_node_execution_id: Some(node_execution_id),
                correlation_id,
                created_at: Utc::now(),
                command: AgentRunPortCommand::Cancel {
                    reason: "any join cancel_remaining".to_string(),
                },
            })
            .await?;
            queued += 1;
        }
        Ok(queued)
    }

    pub async fn consume_source_event(
        &self,
        inbox_id: Uuid,
        join_node_execution_id: Uuid,
        source_node_execution_id: Uuid,
        target_node_execution_id: Option<Uuid>,
    ) -> Result<bool, OrchestrationServiceError> {
        self.consume_source_event_with_effects(
            inbox_id,
            join_node_execution_id,
            source_node_execution_id,
            target_node_execution_id,
            None,
        )
        .await
    }

    /// Consume an Agent event and persist the join decision plus a follow-up
    /// command atomically. The command is still delivered at-least-once by the
    /// ordinary outbox dispatcher after this method commits.
    pub async fn consume_source_event_and_enqueue(
        &self,
        inbox_id: Uuid,
        join_node_execution_id: Uuid,
        source_node_execution_id: Uuid,
        target_node_execution_id: Uuid,
        command: AgentRunPortCommandEnvelope,
    ) -> Result<bool, OrchestrationServiceError> {
        self.consume_source_event_with_effects(
            inbox_id,
            join_node_execution_id,
            source_node_execution_id,
            Some(target_node_execution_id),
            Some(command),
        )
        .await
    }

    pub async fn upstream_handoff(
        &self,
        join_node_execution_id: Uuid,
        source_node_execution_id: Uuid,
    ) -> Result<UpstreamHandoff, OrchestrationServiceError> {
        Ok(OrchestrationConsumptionRecord::upstream_handoff(
            &self.pool,
            join_node_execution_id,
            source_node_execution_id,
        )
        .await?)
    }

    /// Resume the queue after a terminal child or service restart. A ready
    /// target is returned to the caller so it can construct/deliver its
    /// idempotent Create command through the normal outbox path.
    pub async fn resume_serial_queue(
        &self,
        join_node_execution_id: Uuid,
    ) -> Result<Option<SerialEachQueueItem>, OrchestrationServiceError> {
        Ok(OrchestrationConsumptionRecord::promote_next_serial(
            &self.pool,
            join_node_execution_id,
            Utc::now(),
        )
        .await?)
    }

    pub async fn resume_serial_queues_for_run(
        &self,
        orchestration_run_id: Uuid,
    ) -> Result<Vec<SerialEachQueueItem>, OrchestrationServiceError> {
        let plan: sqlx::types::Json<executors::runtime::OrchestrationPlanSnapshot> =
            sqlx::query_scalar("SELECT plan_snapshot FROM orchestration_runs WHERE id = ?")
                .bind(orchestration_run_id)
                .fetch_one(&self.pool)
                .await?;
        let mut promoted = Vec::new();
        for node in plan.0.nodes.iter().filter(|node| {
            node.join == OrchestrationJoinPolicy::Each
                && node.each_downstream_execution
                    == executors::runtime::EachDownstreamExecution::Serial
        }) {
            let node_execution_id =
                stable_node_execution_id(orchestration_run_id, &node.node_key, 0);
            if let Some(item) = self.resume_serial_queue(node_execution_id).await? {
                promoted.push(item);
            }
        }
        Ok(promoted)
    }

    async fn consume_source_event_with_effects(
        &self,
        inbox_id: Uuid,
        join_node_execution_id: Uuid,
        source_node_execution_id: Uuid,
        target_node_execution_id: Option<Uuid>,
        command: Option<AgentRunPortCommandEnvelope>,
    ) -> Result<bool, OrchestrationServiceError> {
        let (run_id, correlation_id, sequence, plan, join_node_key): (
            Uuid,
            Uuid,
            i64,
            sqlx::types::Json<OrchestrationPlanSnapshot>,
            String,
        ) = sqlx::query_as(
            r#"
            SELECT inbox.orchestration_run_id,
                   runs.correlation_id,
                   COALESCE(state.last_event_sequence, 0) + 1,
                   runs.plan_snapshot,
                   join_node.node_key
            FROM orchestration_inbox inbox
            JOIN orchestration_runs runs ON runs.id = inbox.orchestration_run_id
            JOIN orchestration_state state ON state.orchestration_run_id = runs.id
            JOIN orchestration_node_executions join_node
              ON join_node.id = ?
             AND join_node.orchestration_run_id = runs.id
            WHERE inbox.id = ?
            "#,
        )
        .bind(join_node_execution_id)
        .bind(inbox_id)
        .fetch_one(&self.pool)
        .await?;
        let join_policy = plan
            .nodes
            .iter()
            .find(|node| node.node_key == join_node_key)
            .map(|node| node.join)
            .ok_or(OrchestrationServiceError::Persistence(
                OrchestrationPersistenceError::MissingNodeIdentity,
            ))?;
        if !OrchestrationLeaseRecord::acquire(
            &self.pool,
            "dispatcher",
            run_id,
            &self.owner_id,
            Utc::now(),
            Duration::seconds(30),
        )
        .await?
        {
            return Err(OrchestrationServiceError::Persistence(
                OrchestrationPersistenceError::IdempotencyConflict {
                    entity: "orchestration lease",
                    key: run_id.to_string(),
                },
            ));
        }
        let event = OrchestrationEventEnvelope {
            schema_version: ORCHESTRATION_EVENT_SCHEMA_VERSION,
            payload_version: ORCHESTRATION_EVENT_PAYLOAD_VERSION,
            event_id: stable_join_event_id(
                run_id,
                join_node_execution_id,
                source_node_execution_id,
            ),
            orchestration_run_id: run_id,
            sequence: u64::try_from(sequence).unwrap_or(u64::MAX),
            correlation_id,
            timestamp: Utc::now(),
            payload: OrchestrationEventPayload::JoinDecided {
                node_execution_id: join_node_execution_id,
                policy: join_policy,
                consumed_source_execution_ids: vec![source_node_execution_id],
            },
        };
        let result = OrchestrationInboxRecord::consume_with_effects(
            &self.pool,
            inbox_id,
            join_node_execution_id,
            source_node_execution_id,
            target_node_execution_id,
            &event,
            command.as_ref().map(|value| (Uuid::new_v4(), value)),
            Utc::now(),
        )
        .await;
        OrchestrationLeaseRecord::release(&self.pool, "dispatcher", run_id, &self.owner_id).await?;
        Ok(result?)
    }

    /// Reconcile durable orchestration facts after service startup. No run is
    /// failed or replaced merely because its watcher or lease disappeared.
    pub async fn reconcile_startup(
        &self,
    ) -> Result<StartupReconciliationReport, OrchestrationServiceError> {
        let mut report = StartupReconciliationReport {
            recovered_outbox_deliveries: OrchestrationOutboxRecord::reconcile_inflight(
                &self.pool,
                Utc::now(),
                Duration::zero(),
            )
            .await?,
            recovered_inbox_claims: OrchestrationInboxRecord::reconcile_processing(
                &self.pool, None,
            )
            .await?,
            recovered_leases: OrchestrationLeaseRecord::reconcile_startup(&self.pool).await?,
            ..StartupReconciliationReport::default()
        };
        let runs: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT id, correlation_id FROM orchestration_runs WHERE status NOT IN ('succeeded', 'failed', 'cancelled') ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await?;
        for (run_id, correlation_id) in runs {
            report.unreachable_agent_runs += self.reconcile_run(run_id, correlation_id).await?;
            // A terminal AgentRun may have emitted its final canonical event
            // just before this service restarted, so it will not appear in
            // the live watcher subscription set. Replay the durable inbox
            // after child projections have been reconciled.
            self.drain_inbox_for_run(run_id).await?;
            report.reconciled_runs += 1;
        }
        let terminal_runs_with_pending_inbox: Vec<Uuid> = sqlx::query_scalar(
            r#"SELECT DISTINCT inbox.orchestration_run_id
               FROM orchestration_inbox inbox
               JOIN orchestration_runs runs ON runs.id = inbox.orchestration_run_id
               WHERE inbox.consumption_status = 'pending'
                 AND runs.status IN ('succeeded', 'failed', 'cancelled')"#,
        )
        .fetch_all(&self.pool)
        .await?;
        for run_id in terminal_runs_with_pending_inbox {
            self.drain_inbox_for_run(run_id).await?;
        }

        let delivery_budget = OrchestrationOutboxRecord::pending_count(&self.pool).await?;
        for _ in 0..delivery_budget {
            match self.deliver_next().await {
                Ok(true) => {}
                Ok(false) => break,
                Err(OrchestrationServiceError::Port(_)) => report.delivery_failures += 1,
                Err(error) => return Err(error),
            }
        }
        Ok(report)
    }

    pub async fn reconcile_run(
        &self,
        orchestration_run_id: Uuid,
        correlation_id: Uuid,
    ) -> Result<u64, OrchestrationServiceError> {
        let run_status: OrchestrationRunStatus =
            sqlx::query_scalar("SELECT status FROM orchestration_runs WHERE id = ?")
                .bind(orchestration_run_id)
                .fetch_one(&self.pool)
                .await?;
        // A crash can occur after the cancelling decision but before the
        // fan-out commands are written. Re-run the idempotent fan-out during
        // startup reconciliation; this never reopens the parent or starts a
        // new child.
        if run_status == OrchestrationRunStatus::Cancelling {
            self.cancel(orchestration_run_id, correlation_id).await?;
        }
        let children: Vec<(Uuid, Uuid, OrchestrationNodeStatus)> = sqlx::query_as(
            r#"
            SELECT links.node_execution_id, links.agent_run_id, nodes.status
            FROM orchestration_agent_run_links links
            JOIN orchestration_node_executions nodes ON nodes.id = links.node_execution_id
            WHERE links.orchestration_run_id = ?
              AND nodes.status NOT IN ('succeeded', 'failed', 'cancelled')
            ORDER BY nodes.stable_order, nodes.iteration
            "#,
        )
        .bind(orchestration_run_id)
        .fetch_all(&self.pool)
        .await?;
        let mut unreachable = 0;
        for (node_execution_id, agent_run_id, current) in children {
            let snapshot = match self.port.query(agent_run_id).await {
                Ok(snapshot) => snapshot,
                Err(AgentRunPortError::Unavailable(_)) | Err(AgentRunPortError::NotFound(_)) => {
                    unreachable += 1;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let observed = node_status_from_agent(snapshot.state.status);
            if current == observed || is_terminal_node(current) {
                continue;
            }
            sqlx::query(
                "UPDATE orchestration_node_executions SET status = ?, updated_at = ? WHERE id = ? AND status NOT IN ('succeeded', 'failed', 'cancelled')",
            )
            .bind(observed)
            .bind(snapshot.state.updated_at)
            .bind(node_execution_id)
            .execute(&self.pool)
            .await?;
            self.append_event(
                orchestration_run_id,
                correlation_id,
                OrchestrationEventPayload::NodeStatusChanged {
                    node_execution_id,
                    status: observed,
                },
            )
            .await?;
        }
        self.resume_serial_queues_for_run(orchestration_run_id)
            .await?;
        self.refresh_run_projection(orchestration_run_id, correlation_id)
            .await?;
        Ok(unreachable)
    }

    pub async fn refresh_run_projection(
        &self,
        orchestration_run_id: Uuid,
        correlation_id: Uuid,
    ) -> Result<OrchestrationRunStatus, OrchestrationServiceError> {
        let current: OrchestrationRunStatus =
            sqlx::query_scalar("SELECT status FROM orchestration_runs WHERE id = ?")
                .bind(orchestration_run_id)
                .fetch_one(&self.pool)
                .await?;
        if current.is_terminal() {
            return Ok(current);
        }
        let nodes: Vec<(String, OrchestrationNodeStatus)> = sqlx::query_as(
            "SELECT node_key, status FROM orchestration_node_executions WHERE orchestration_run_id = ? ORDER BY stable_order, iteration",
        )
        .bind(orchestration_run_id)
        .fetch_all(&self.pool)
        .await?;
        let plan: sqlx::types::Json<OrchestrationPlanSnapshot> =
            sqlx::query_scalar("SELECT plan_snapshot FROM orchestration_runs WHERE id = ?")
                .bind(orchestration_run_id)
                .fetch_one(&self.pool)
                .await?;
        if current != OrchestrationRunStatus::Cancelling {
            let fail_fast_failed = nodes.iter().any(|(node_key, status)| {
                *status == OrchestrationNodeStatus::Failed
                    && plan.0.nodes.iter().any(|node| {
                        node.node_key == *node_key
                            && node.failure_policy == OrchestrationFailurePolicy::FailFast
                    })
            });
            if fail_fast_failed {
                self.apply_fail_fast_cancellation(orchestration_run_id, correlation_id)
                    .await?;
                let apply = self
                    .append_event(
                        orchestration_run_id,
                        correlation_id,
                        OrchestrationEventPayload::LifecycleChanged {
                            status: OrchestrationRunStatus::Failed,
                        },
                    )
                    .await?;
                return if matches!(
                    apply,
                    OrchestrationReducerApply::Applied
                        | OrchestrationReducerApply::AppliedDegraded
                        | OrchestrationReducerApply::IgnoredTerminalRegression
                ) {
                    Ok(OrchestrationRunStatus::Failed)
                } else {
                    Ok(current)
                };
            }
        }
        let facts = nodes
            .iter()
            .map(|(node_key, status)| {
                let failure_policy = plan
                    .nodes
                    .iter()
                    .find(|node| node.node_key == *node_key)
                    .map(|node| node.failure_policy)
                    .unwrap_or(OrchestrationFailurePolicy::FailFast);
                (*status, failure_policy)
            })
            .collect::<Vec<_>>();
        let projected = project_run_status(current, &facts);
        if projected != current {
            self.append_event(
                orchestration_run_id,
                correlation_id,
                OrchestrationEventPayload::LifecycleChanged { status: projected },
            )
            .await?;
        }
        Ok(projected)
    }

    async fn apply_fail_fast_cancellation(
        &self,
        orchestration_run_id: Uuid,
        correlation_id: Uuid,
    ) -> Result<(), OrchestrationServiceError> {
        let now = Utc::now();
        let undispatched: Vec<Uuid> = sqlx::query_scalar(
            r#"
            UPDATE orchestration_node_executions
            SET status = 'cancelled', updated_at = ?
            WHERE orchestration_run_id = ?
              AND status IN ('pending', 'ready')
              AND NOT EXISTS (
                  SELECT 1 FROM orchestration_agent_run_links links
                  WHERE links.node_execution_id = orchestration_node_executions.id
              )
            RETURNING id
            "#,
        )
        .bind(now)
        .bind(orchestration_run_id)
        .fetch_all(&self.pool)
        .await?;
        for node_execution_id in undispatched {
            self.append_event(
                orchestration_run_id,
                correlation_id,
                OrchestrationEventPayload::NodeStatusChanged {
                    node_execution_id,
                    status: OrchestrationNodeStatus::Cancelled,
                },
            )
            .await?;
        }

        let active: Vec<(Uuid, Uuid)> = sqlx::query_as(
            r#"
            UPDATE orchestration_node_executions
            SET status = 'cancelling', updated_at = ?
            WHERE orchestration_run_id = ?
              AND status NOT IN ('succeeded', 'failed', 'cancelled', 'cancelling')
              AND EXISTS (
                  SELECT 1 FROM orchestration_agent_run_links links
                  WHERE links.node_execution_id = orchestration_node_executions.id
              )
            RETURNING id, (
                SELECT links.agent_run_id
                FROM orchestration_agent_run_links links
                WHERE links.node_execution_id = orchestration_node_executions.id
            )
            "#,
        )
        .bind(now)
        .bind(orchestration_run_id)
        .fetch_all(&self.pool)
        .await?;
        for (node_execution_id, agent_run_id) in active {
            self.append_event(
                orchestration_run_id,
                correlation_id,
                OrchestrationEventPayload::NodeStatusChanged {
                    node_execution_id,
                    status: OrchestrationNodeStatus::Cancelling,
                },
            )
            .await?;
            let idempotency_key =
                format!("orchestration:{orchestration_run_id}:node:{node_execution_id}:cancel");
            self.enqueue_command(AgentRunPortCommandEnvelope {
                schema_version: executors::runtime::ORCHESTRATION_COMMAND_SCHEMA_VERSION,
                command_id: stable_operation_id(orchestration_run_id, node_execution_id, "cancel"),
                idempotency_key,
                agent_run_id,
                orchestration_run_id: Some(orchestration_run_id),
                orchestration_node_execution_id: Some(node_execution_id),
                correlation_id,
                created_at: now,
                command: AgentRunPortCommand::Cancel {
                    reason: "fail_fast sibling cancellation".to_string(),
                },
            })
            .await?;
        }
        Ok(())
    }

    pub async fn query_agent_run(
        &self,
        agent_run_id: Uuid,
    ) -> Result<RunState, OrchestrationServiceError> {
        Ok(self.port.query(agent_run_id).await?.state)
    }

    pub async fn subscribe_agent_run(
        &self,
        agent_run_id: Uuid,
    ) -> Result<impl Stream<Item = executors::runtime::AgentEventEnvelope>, OrchestrationServiceError>
    {
        Ok(self.port.subscribe(agent_run_id).await?)
    }

    async fn append_event(
        &self,
        run_id: Uuid,
        correlation_id: Uuid,
        payload: OrchestrationEventPayload,
    ) -> Result<OrchestrationReducerApply, OrchestrationServiceError> {
        let now = Utc::now();
        if !OrchestrationLeaseRecord::acquire(
            &self.pool,
            "dispatcher",
            run_id,
            &self.owner_id,
            now,
            Duration::seconds(30),
        )
        .await?
        {
            return Err(OrchestrationServiceError::Persistence(
                OrchestrationPersistenceError::IdempotencyConflict {
                    entity: "orchestration lease",
                    key: run_id.to_string(),
                },
            ));
        }
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(last_event_sequence, 0) + 1 FROM orchestration_state WHERE orchestration_run_id = ?",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(OrchestrationServiceError::MissingState(run_id))?;
        let event = OrchestrationEventEnvelope {
            schema_version: ORCHESTRATION_EVENT_SCHEMA_VERSION,
            payload_version: ORCHESTRATION_EVENT_PAYLOAD_VERSION,
            event_id: Uuid::new_v4(),
            orchestration_run_id: run_id,
            sequence: u64::try_from(sequence).unwrap_or(u64::MAX),
            correlation_id,
            timestamp: now,
            payload,
        };
        let result = OrchestrationEventRecord::append_and_project(&self.pool, &event).await;
        OrchestrationLeaseRecord::release(&self.pool, "dispatcher", run_id, &self.owner_id).await?;
        Ok(result?)
    }
}

fn stable_node_execution_id(run_id: Uuid, node_key: &str, iteration: u32) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(run_id.as_bytes());
    hasher.update(node_key.as_bytes());
    hasher.update(iteration.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

fn stable_operation_id(run_id: Uuid, node_execution_id: Uuid, operation: &str) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(run_id.as_bytes());
    hasher.update(node_execution_id.as_bytes());
    hasher.update(operation.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

fn stable_join_operation_id(
    run_id: Uuid,
    join_node_execution_id: Uuid,
    node_execution_id: Uuid,
    operation: &str,
) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(run_id.as_bytes());
    hasher.update(join_node_execution_id.as_bytes());
    hasher.update(node_execution_id.as_bytes());
    hasher.update(operation.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

fn stable_join_event_id(
    run_id: Uuid,
    join_node_execution_id: Uuid,
    source_node_execution_id: Uuid,
) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(run_id.as_bytes());
    hasher.update(join_node_execution_id.as_bytes());
    hasher.update(source_node_execution_id.as_bytes());
    hasher.update(b"join-decision");
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

fn is_terminal_node(status: OrchestrationNodeStatus) -> bool {
    matches!(
        status,
        OrchestrationNodeStatus::Succeeded
            | OrchestrationNodeStatus::Failed
            | OrchestrationNodeStatus::Cancelled
    )
}

fn node_status_from_agent(status: AgentRunStatus) -> OrchestrationNodeStatus {
    match status {
        AgentRunStatus::Pending | AgentRunStatus::Starting | AgentRunStatus::Running => {
            OrchestrationNodeStatus::Running
        }
        AgentRunStatus::AwaitingInput => OrchestrationNodeStatus::AwaitingInput,
        AgentRunStatus::AwaitingApproval => OrchestrationNodeStatus::AwaitingApproval,
        AgentRunStatus::Cancelling => OrchestrationNodeStatus::Cancelling,
        AgentRunStatus::Succeeded => OrchestrationNodeStatus::Succeeded,
        AgentRunStatus::Cancelled => OrchestrationNodeStatus::Cancelled,
        AgentRunStatus::Failed | AgentRunStatus::Crashed | AgentRunStatus::AuditFailed => {
            OrchestrationNodeStatus::Failed
        }
    }
}

fn project_run_status(
    current: OrchestrationRunStatus,
    facts: &[(OrchestrationNodeStatus, OrchestrationFailurePolicy)],
) -> OrchestrationRunStatus {
    if facts.is_empty() {
        return current;
    }
    // Terminal orchestration decisions are monotonic. Late child facts may
    // still be audited and consumed for deduplication, but they must never
    // reopen a completed/failed/cancelled parent run into Running or Waiting.
    if current.is_terminal() {
        return current;
    }
    let all_terminal = facts.iter().all(|(status, _)| is_terminal_node(*status));
    if current == OrchestrationRunStatus::Cancelling {
        return if all_terminal {
            OrchestrationRunStatus::Cancelled
        } else {
            OrchestrationRunStatus::Cancelling
        };
    }
    if all_terminal {
        let has_fail_fast_failure = facts.iter().any(|(status, failure_policy)| {
            matches!(
                status,
                OrchestrationNodeStatus::Failed | OrchestrationNodeStatus::Cancelled
            ) && *failure_policy == OrchestrationFailurePolicy::FailFast
        });
        if has_fail_fast_failure {
            return OrchestrationRunStatus::Failed;
        }
        if facts
            .iter()
            .any(|(status, _)| *status == OrchestrationNodeStatus::Succeeded)
        {
            return OrchestrationRunStatus::Succeeded;
        }
        // Child cancellation alone never turns the parent into a user-cancel
        // terminal. With no successful partial result the aggregate failed.
        return OrchestrationRunStatus::Failed;
    }
    let has_runnable = facts.iter().any(|(status, _)| {
        matches!(
            status,
            OrchestrationNodeStatus::Ready | OrchestrationNodeStatus::Running
        )
    });
    if has_runnable {
        return OrchestrationRunStatus::Running;
    }
    if facts
        .iter()
        .any(|(status, _)| *status == OrchestrationNodeStatus::AwaitingApproval)
    {
        return OrchestrationRunStatus::WaitingForApproval;
    }
    if facts
        .iter()
        .any(|(status, _)| *status == OrchestrationNodeStatus::AwaitingInput)
    {
        return OrchestrationRunStatus::WaitingForInput;
    }
    OrchestrationRunStatus::Running
}

pub fn evaluate_join(
    policy: OrchestrationJoinPolicy,
    failure_policy: OrchestrationFailurePolicy,
    remaining: executors::runtime::RemainingUpstreamsPolicy,
    statuses: &[OrchestrationNodeStatus],
) -> JoinDecision {
    if statuses.is_empty() {
        return JoinDecision::Waiting;
    }
    let successful = statuses
        .iter()
        .filter(|status| **status == OrchestrationNodeStatus::Succeeded)
        .count();
    let terminal = statuses.iter().all(|status| {
        matches!(
            status,
            OrchestrationNodeStatus::Succeeded
                | OrchestrationNodeStatus::Failed
                | OrchestrationNodeStatus::Cancelled
        )
    });
    let has_failure = statuses.iter().any(|status| {
        matches!(
            status,
            OrchestrationNodeStatus::Failed | OrchestrationNodeStatus::Cancelled
        )
    });
    match policy {
        OrchestrationJoinPolicy::All
            if failure_policy == OrchestrationFailurePolicy::FailFast && has_failure =>
        {
            JoinDecision::Failed
        }
        OrchestrationJoinPolicy::All if !terminal => JoinDecision::Waiting,
        OrchestrationJoinPolicy::All if successful == statuses.len() => JoinDecision::Ready,
        OrchestrationJoinPolicy::All
            if failure_policy == OrchestrationFailurePolicy::AllowPartial && successful > 0 =>
        {
            JoinDecision::Ready
        }
        OrchestrationJoinPolicy::All => JoinDecision::Failed,
        OrchestrationJoinPolicy::Any if successful > 0 => {
            if remaining == executors::runtime::RemainingUpstreamsPolicy::CancelRemaining {
                JoinDecision::ReadyAndCancelRemaining
            } else {
                JoinDecision::Ready
            }
        }
        OrchestrationJoinPolicy::Any if terminal => JoinDecision::Failed,
        OrchestrationJoinPolicy::Any => JoinDecision::Waiting,
        // `each` consumes each successful source independently; it must not
        // wait for unrelated upstreams to reach terminal state. The durable
        // source-consumption key prevents replay from creating duplicates.
        OrchestrationJoinPolicy::Each
            if failure_policy == OrchestrationFailurePolicy::FailFast && has_failure =>
        {
            JoinDecision::Failed
        }
        OrchestrationJoinPolicy::Each if successful > 0 => JoinDecision::Ready,
        OrchestrationJoinPolicy::Each if terminal => JoinDecision::Failed,
        OrchestrationJoinPolicy::Each => JoinDecision::Waiting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_projection_only_blocks_when_no_runnable_branch_exists() {
        assert_eq!(
            project_run_status(
                OrchestrationRunStatus::Running,
                &[
                    (
                        OrchestrationNodeStatus::AwaitingInput,
                        OrchestrationFailurePolicy::FailFast,
                    ),
                    (
                        OrchestrationNodeStatus::Running,
                        OrchestrationFailurePolicy::FailFast,
                    ),
                ],
            ),
            OrchestrationRunStatus::Running
        );
        assert_eq!(
            project_run_status(
                OrchestrationRunStatus::Running,
                &[(
                    OrchestrationNodeStatus::AwaitingInput,
                    OrchestrationFailurePolicy::FailFast,
                )],
            ),
            OrchestrationRunStatus::WaitingForInput
        );
        assert_eq!(
            project_run_status(
                OrchestrationRunStatus::Running,
                &[(
                    OrchestrationNodeStatus::AwaitingApproval,
                    OrchestrationFailurePolicy::FailFast,
                )],
            ),
            OrchestrationRunStatus::WaitingForApproval
        );
    }

    #[test]
    fn cancelling_projection_waits_for_children_and_never_reopens() {
        assert_eq!(
            project_run_status(
                OrchestrationRunStatus::Cancelling,
                &[(
                    OrchestrationNodeStatus::Cancelling,
                    OrchestrationFailurePolicy::FailFast,
                )],
            ),
            OrchestrationRunStatus::Cancelling
        );
        assert_eq!(
            project_run_status(
                OrchestrationRunStatus::Cancelling,
                &[(
                    OrchestrationNodeStatus::Cancelled,
                    OrchestrationFailurePolicy::FailFast,
                )],
            ),
            OrchestrationRunStatus::Cancelled
        );
        assert_eq!(
            project_run_status(
                OrchestrationRunStatus::Succeeded,
                &[(
                    OrchestrationNodeStatus::Running,
                    OrchestrationFailurePolicy::FailFast,
                )],
            ),
            OrchestrationRunStatus::Succeeded
        );
    }

    #[test]
    fn provider_terminal_statuses_map_to_explicit_node_states() {
        assert_eq!(
            node_status_from_agent(AgentRunStatus::AwaitingApproval),
            OrchestrationNodeStatus::AwaitingApproval
        );
        assert_eq!(
            node_status_from_agent(AgentRunStatus::Cancelled),
            OrchestrationNodeStatus::Cancelled
        );
        assert_eq!(
            node_status_from_agent(AgentRunStatus::Crashed),
            OrchestrationNodeStatus::Failed
        );
    }

    #[test]
    fn allow_partial_only_consumes_successful_sources() {
        assert_eq!(
            evaluate_join(
                OrchestrationJoinPolicy::All,
                OrchestrationFailurePolicy::AllowPartial,
                executors::runtime::RemainingUpstreamsPolicy::Continue,
                &[
                    OrchestrationNodeStatus::Succeeded,
                    OrchestrationNodeStatus::Failed,
                    OrchestrationNodeStatus::Running,
                ],
            ),
            JoinDecision::Waiting
        );
        assert_eq!(
            evaluate_join(
                OrchestrationJoinPolicy::All,
                OrchestrationFailurePolicy::AllowPartial,
                executors::runtime::RemainingUpstreamsPolicy::Continue,
                &[
                    OrchestrationNodeStatus::Succeeded,
                    OrchestrationNodeStatus::Failed,
                    OrchestrationNodeStatus::Cancelled,
                ],
            ),
            JoinDecision::Ready
        );
        assert_eq!(
            evaluate_join(
                OrchestrationJoinPolicy::All,
                OrchestrationFailurePolicy::AllowPartial,
                executors::runtime::RemainingUpstreamsPolicy::Continue,
                &[
                    OrchestrationNodeStatus::Failed,
                    OrchestrationNodeStatus::Cancelled
                ],
            ),
            JoinDecision::Failed
        );
    }

    #[test]
    fn fail_fast_does_not_wait_for_unrelated_upstreams_after_failure() {
        assert_eq!(
            evaluate_join(
                OrchestrationJoinPolicy::All,
                OrchestrationFailurePolicy::FailFast,
                executors::runtime::RemainingUpstreamsPolicy::Continue,
                &[
                    OrchestrationNodeStatus::Failed,
                    OrchestrationNodeStatus::Running
                ],
            ),
            JoinDecision::Failed
        );
    }

    #[test]
    fn each_fail_fast_does_not_hide_a_failed_upstream() {
        assert_eq!(
            evaluate_join(
                OrchestrationJoinPolicy::Each,
                OrchestrationFailurePolicy::FailFast,
                executors::runtime::RemainingUpstreamsPolicy::Continue,
                &[
                    OrchestrationNodeStatus::Failed,
                    OrchestrationNodeStatus::Succeeded
                ],
            ),
            JoinDecision::Failed
        );
        assert_eq!(
            evaluate_join(
                OrchestrationJoinPolicy::Each,
                OrchestrationFailurePolicy::AllowPartial,
                executors::runtime::RemainingUpstreamsPolicy::Continue,
                &[
                    OrchestrationNodeStatus::Failed,
                    OrchestrationNodeStatus::Succeeded
                ],
            ),
            JoinDecision::Ready
        );
    }

    #[test]
    fn any_join_is_replay_safe_and_respects_remaining_policy() {
        let statuses = [
            OrchestrationNodeStatus::Succeeded,
            OrchestrationNodeStatus::Running,
        ];
        assert_eq!(
            evaluate_join(
                OrchestrationJoinPolicy::Any,
                OrchestrationFailurePolicy::FailFast,
                executors::runtime::RemainingUpstreamsPolicy::Continue,
                &statuses,
            ),
            JoinDecision::Ready
        );
        assert_eq!(
            evaluate_join(
                OrchestrationJoinPolicy::Any,
                OrchestrationFailurePolicy::FailFast,
                executors::runtime::RemainingUpstreamsPolicy::CancelRemaining,
                &statuses,
            ),
            JoinDecision::ReadyAndCancelRemaining
        );
        assert_eq!(
            evaluate_join(
                OrchestrationJoinPolicy::Any,
                OrchestrationFailurePolicy::FailFast,
                executors::runtime::RemainingUpstreamsPolicy::Continue,
                &[
                    OrchestrationNodeStatus::Failed,
                    OrchestrationNodeStatus::Cancelled,
                ],
            ),
            JoinDecision::Failed
        );
    }

    #[test]
    fn partial_success_projects_success_but_all_cancelled_projects_failure() {
        assert_eq!(
            project_run_status(
                OrchestrationRunStatus::Running,
                &[
                    (
                        OrchestrationNodeStatus::Succeeded,
                        OrchestrationFailurePolicy::AllowPartial,
                    ),
                    (
                        OrchestrationNodeStatus::Failed,
                        OrchestrationFailurePolicy::AllowPartial,
                    ),
                ],
            ),
            OrchestrationRunStatus::Succeeded
        );
        assert_eq!(
            project_run_status(
                OrchestrationRunStatus::Running,
                &[
                    (
                        OrchestrationNodeStatus::Cancelled,
                        OrchestrationFailurePolicy::AllowPartial,
                    ),
                    (
                        OrchestrationNodeStatus::Failed,
                        OrchestrationFailurePolicy::AllowPartial,
                    ),
                ],
            ),
            OrchestrationRunStatus::Failed
        );
    }
}
