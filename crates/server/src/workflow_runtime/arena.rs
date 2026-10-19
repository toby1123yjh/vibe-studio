use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use chrono::Utc;
use db::models::{
    arena_group::{
        ArenaCandidate, ArenaCandidatePurpose, ArenaGroup, ArenaGroupError, ArenaLifecycleStatus,
        ArenaMode, CreateArenaCandidate, CreateArenaGroup,
    },
    requests::WorkspaceRepoInput,
    session::{CreateSession, Session},
    workspace::{CreateWorkspace, Workspace, WorkspaceError, WorkspaceKind},
    workspace_repo::WorkspaceRepo,
};
use deployment::Deployment;
use executors::{
    executors::{BaseCodingAgent, provider_adapter::DirectProvider},
    profile::{ExecutorConfig, ExecutorConfigs},
    provider_policy::direct_provider_capability_snapshot,
    runtime::{
        AGENT_REQUEST_PAYLOAD_VERSION, AGENT_REQUEST_SCHEMA_VERSION, AgentRunIntent,
        AgentRunPortCommand, AgentRunPortCommandEnvelope, AgentRunRequestEnvelope, AgentRunStatus,
        AgentRuntimeMessageRole, CanonicalMessage, EachDownstreamExecution,
        ORCHESTRATION_COMMAND_SCHEMA_VERSION, ORCHESTRATION_PLAN_SCHEMA_VERSION,
        OrchestrationFailurePolicy, OrchestrationJoinPolicy, OrchestrationPlanNode,
        OrchestrationPlanSnapshot, OrchestrationProductKind, OrchestrationRetryPolicy,
        RemainingUpstreamsPolicy, RunAttemptMode, RunAttemptRequest, RunState, WorkspaceMode,
        WorkspaceReference,
    },
};
use serde_json::Value;
use services::services::{container::ContainerService, orchestration::OrchestrationService};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

const WORKFLOW_ARENA_MIN_ATTEMPTS: usize = 2;

#[derive(Debug, Clone)]
struct PreparedArenaAttempt {
    attempt_id: Option<String>,
    display_name: Option<String>,
    branch_name: String,
    prompt: String,
    executor_config: ExecutorConfig,
}

#[derive(Debug, Clone)]
struct ArenaCandidateLaunch {
    workspace: Workspace,
    workspace_path: String,
    node_key: String,
    prompt: String,
    executor_config: ExecutorConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ArenaCandidateRuntime {
    orchestration_run_id: Uuid,
    agent_run_id: Uuid,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArenaNodeAttemptRequest {
    pub attempt_id: Option<String>,
    pub display_name: Option<String>,
    pub branch_name: String,
    pub prompt: String,
    pub executor_config: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArenaNodeRequest {
    pub run_id: Uuid,
    pub node_id: String,
    pub iteration: i64,
    pub issue_id: Uuid,
    pub main_workspace_id: Uuid,
    pub prompt: String,
    pub attempts: Vec<ArenaNodeAttemptRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaNodeExecution {
    pub arena_group_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaWinnerRequest {
    pub run_id: Uuid,
    pub node_id: String,
    pub arena_group_id: Uuid,
    pub main_workspace_id: Uuid,
    pub candidate_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaWinnerExecution {
    pub output_text: String,
    /// Readable, applied files relative to the main Agent working directory.
    /// Deleted paths are changes, but cannot be handed to a file consumer.
    pub artifact_paths: Vec<String>,
}

#[async_trait]
pub trait WorkflowArenaCreator: Send + Sync {
    async fn create_arena(&self, request: ArenaNodeRequest)
    -> Result<ArenaNodeExecution, ApiError>;
}

#[async_trait]
pub trait WorkflowArenaWinnerApplier: Send + Sync {
    async fn apply_winner(
        &self,
        request: ArenaWinnerRequest,
    ) -> Result<ArenaWinnerExecution, ApiError>;
}

#[derive(Debug, Clone, Copy)]
pub struct NoopWorkflowArenaCreator;

#[async_trait]
impl WorkflowArenaCreator for NoopWorkflowArenaCreator {
    async fn create_arena(
        &self,
        _request: ArenaNodeRequest,
    ) -> Result<ArenaNodeExecution, ApiError> {
        Err(ApiError::BadRequest(
            "Workflow arena nodes require an arena creator".to_string(),
        ))
    }
}

#[derive(Clone)]
pub struct DeploymentWorkflowArenaCreator {
    deployment: DeploymentImpl,
}

impl DeploymentWorkflowArenaCreator {
    pub fn new(deployment: DeploymentImpl) -> Self {
        Self { deployment }
    }
}

#[async_trait]
impl WorkflowArenaCreator for DeploymentWorkflowArenaCreator {
    async fn create_arena(
        &self,
        request: ArenaNodeRequest,
    ) -> Result<ArenaNodeExecution, ApiError> {
        create_deployment_arena(&self.deployment, request).await
    }
}

#[derive(Clone)]
pub struct DeploymentWorkflowArenaWinnerApplier {
    deployment: DeploymentImpl,
}

impl DeploymentWorkflowArenaWinnerApplier {
    pub fn new(deployment: DeploymentImpl) -> Self {
        Self { deployment }
    }
}

#[async_trait]
impl WorkflowArenaWinnerApplier for DeploymentWorkflowArenaWinnerApplier {
    async fn apply_winner(
        &self,
        request: ArenaWinnerRequest,
    ) -> Result<ArenaWinnerExecution, ApiError> {
        apply_deployment_arena_winner(&self.deployment, request).await
    }
}

async fn create_deployment_arena(
    deployment: &DeploymentImpl,
    request: ArenaNodeRequest,
) -> Result<ArenaNodeExecution, ApiError> {
    if request.attempts.len() < WORKFLOW_ARENA_MIN_ATTEMPTS {
        return Err(ApiError::BadRequest(format!(
            "Workflow arena requires at least {WORKFLOW_ARENA_MIN_ATTEMPTS} attempts, got {}",
            request.attempts.len()
        )));
    }
    if request.prompt.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Workflow arena prompt must not be empty.".to_string(),
        ));
    }
    let attempts = prepare_arena_attempts(request.attempts.clone()).await?;

    let pool = &deployment.db().pool;
    let project_id = issue_project_id(pool, request.issue_id).await?;
    let main_workspace = Workspace::find_by_id(pool, request.main_workspace_id)
        .await?
        .ok_or(ApiError::Workspace(WorkspaceError::WorkspaceNotFound))?;
    let base_repos = WorkspaceRepo::find_by_workspace_id(pool, main_workspace.id).await?;
    if base_repos.is_empty() {
        return Err(ApiError::BadRequest(
            "Workflow arena requires the main workflow workspace to have at least one repository"
                .to_string(),
        ));
    }

    if ArenaGroup::find_active_by_issue_id(pool, request.issue_id)
        .await?
        .is_some()
    {
        return Err(ApiError::BadRequest(format!(
            "issue {} already has an active arena group; close, adopt, promote, or dissolve it first",
            request.issue_id
        )));
    }

    let task_id: Uuid = sqlx::query_scalar(
        r#"
        SELECT node.task_id
        FROM node_executions node
        JOIN tasks task ON task.id = node.task_id
        JOIN workflow_runs run ON run.id = node.run_id
        JOIN workflow_attempts attempt ON attempt.id = run.attempt_id
        WHERE node.run_id = ?
          AND node.node_id = ?
          AND node.iteration = ?
          AND task.parent_task_id = attempt.task_id
          AND task.execution_kind = 'arena'
        "#,
    )
    .bind(request.run_id)
    .bind(&request.node_id)
    .bind(request.iteration)
    .fetch_one(pool)
    .await?;
    let group_id = Uuid::new_v4();
    let mut transaction = pool.begin().await?;
    let group = ArenaGroup::create(
        &mut transaction,
        &CreateArenaGroup {
            id: group_id,
            task_id,
            prompt: request.prompt.clone(),
            base_branch: main_workspace.branch.clone(),
            mode: ArenaMode::Implementation,
        },
    )
    .await?;
    let binding = sqlx::query(
        r#"
        UPDATE node_executions
        SET arena_group_id = ?,
            updated_at = datetime('now', 'subsec')
        WHERE run_id = ? AND node_id = ? AND iteration = ?
          AND task_id = ? AND arena_group_id IS NULL
        "#,
    )
    .bind(group.id)
    .bind(request.run_id)
    .bind(&request.node_id)
    .bind(request.iteration)
    .bind(task_id)
    .execute(&mut *transaction)
    .await?;
    if binding.rows_affected() != 1 {
        return Err(ApiError::Conflict(format!(
            "Workflow Arena node `{}` iteration {} is missing or already materialized",
            request.node_id, request.iteration
        )));
    }
    transaction.commit().await?;

    let creation_result = async {
        let mut candidates = Vec::with_capacity(attempts.len());
        for (idx, attempt) in attempts.into_iter().enumerate() {
            candidates.push(
                create_workflow_arena_candidate(
                    deployment,
                    pool,
                    &group,
                    project_id,
                    request.issue_id,
                    &main_workspace,
                    &base_repos,
                    attempt,
                    idx,
                )
                .await?,
            );
        }
        launch_arena_candidates(deployment, &request, &group, &candidates).await
    }
    .await;
    if let Err(error) = creation_result {
        if let Err(cleanup_error) = cleanup_failed_workflow_arena(pool, group.id).await {
            tracing::warn!(
                arena_group_id = %group.id,
                "failed to clean up partially-created workflow arena: {cleanup_error:#}"
            );
        }
        return Err(error);
    }

    deployment
        .track_if_analytics_allowed(
            "workflow_arena_group_created",
            serde_json::json!({
                "arena_group_id": group.id.to_string(),
                "issue_id": request.issue_id.to_string(),
                "run_id": request.run_id.to_string(),
                "node_id": request.node_id,
            }),
        )
        .await;

    Ok(ArenaNodeExecution {
        arena_group_id: group.id,
    })
}

async fn prepare_arena_attempts(
    attempts: Vec<ArenaNodeAttemptRequest>,
) -> Result<Vec<PreparedArenaAttempt>, ApiError> {
    let mut prepared = Vec::with_capacity(attempts.len());
    for (attempt_index, attempt) in attempts.into_iter().enumerate() {
        let prompt = attempt.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(ApiError::BadRequest(format!(
                "Workflow arena attempt {} prompt must not be empty.",
                attempt_index + 1
            )));
        }
        let executor_config = executor_config_from_value(attempt.executor_config).await?;
        validate_canonical_executor_config(&executor_config)?;
        prepared.push(PreparedArenaAttempt {
            attempt_id: attempt.attempt_id,
            display_name: attempt.display_name,
            branch_name: attempt.branch_name,
            prompt,
            executor_config,
        });
    }
    Ok(prepared)
}

async fn issue_project_id(pool: &SqlitePool, issue_id: Uuid) -> Result<Uuid, ApiError> {
    sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM local_issues WHERE id = ?")
        .bind(issue_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("issue {issue_id} was not found")))
}

#[allow(clippy::too_many_arguments)]
async fn create_workflow_arena_candidate(
    deployment: &DeploymentImpl,
    pool: &SqlitePool,
    group: &ArenaGroup,
    _project_id: Uuid,
    _issue_id: Uuid,
    main_workspace: &Workspace,
    base_repos: &[WorkspaceRepo],
    attempt: PreparedArenaAttempt,
    attempt_index: usize,
) -> Result<ArenaCandidateLaunch, ApiError> {
    let workspace_name = attempt.display_name.clone().unwrap_or_else(|| {
        attempt
            .attempt_id
            .clone()
            .unwrap_or_else(|| format!("Arena Candidate {}", attempt_index + 1))
    });
    let workspace_id = Uuid::new_v4();
    let workspace = Workspace::create(
        pool,
        &CreateWorkspace {
            branch: attempt.branch_name,
            name: Some(workspace_name),
        },
        workspace_id,
    )
    .await?;

    let mut managed_workspace = deployment
        .workspace_manager()
        .load_managed_workspace(workspace)
        .await?;
    for repo in base_repos {
        let repo_input = WorkspaceRepoInput {
            repo_id: repo.repo_id,
            target_branch: main_workspace.branch.clone(),
        };
        managed_workspace
            .add_repository(&repo_input, deployment.git())
            .await
            .map_err(ApiError::from)?;
    }

    let workspace = managed_workspace.workspace.clone();
    let mut transaction = pool.begin().await?;
    ArenaCandidate::create(
        &mut transaction,
        &CreateArenaCandidate {
            id: Uuid::new_v4(),
            arena_group_id: group.id,
            workspace_id: workspace.id,
            purpose: ArenaCandidatePurpose::Attempt,
            sort_order: attempt_index as i64,
        },
    )
    .await?;
    transaction.commit().await?;

    let workspace_path = deployment
        .container()
        .ensure_container_exists(&workspace)
        .await?;

    Ok(ArenaCandidateLaunch {
        workspace,
        workspace_path,
        node_key: format!("candidate:{}", workspace_id),
        prompt: attempt.prompt,
        executor_config: attempt.executor_config,
    })
}

async fn launch_arena_candidates(
    deployment: &DeploymentImpl,
    request: &ArenaNodeRequest,
    group: &ArenaGroup,
    candidates: &[ArenaCandidateLaunch],
) -> Result<Uuid, ApiError> {
    let pool = &deployment.db().pool;
    let orchestration_run_id = stable_arena_identity(
        request.run_id,
        group.id,
        0,
        &format!("{}:orchestration", request.node_id),
    );
    let plan = OrchestrationPlanSnapshot {
        schema_version: ORCHESTRATION_PLAN_SCHEMA_VERSION,
        plan_id: stable_arena_identity(orchestration_run_id, group.id, 0, "plan"),
        source_definition_id: group.id,
        source_definition_version: arena_plan_version(request, candidates)?,
        product_kind: OrchestrationProductKind::Arena,
        workspace_mode: WorkspaceMode::IsolatedWorktree,
        nodes: candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                let provider = direct_provider_for_agent(candidate.executor_config.executor)?;
                Ok(OrchestrationPlanNode {
                    node_key: candidate.node_key.clone(),
                    stable_order: u32::try_from(index).unwrap_or(u32::MAX),
                    dependencies: Vec::new(),
                    join: OrchestrationJoinPolicy::All,
                    failure_policy: OrchestrationFailurePolicy::AllowPartial,
                    remaining_upstreams: RemainingUpstreamsPolicy::Continue,
                    each_downstream_execution: EachDownstreamExecution::Parallel,
                    retry: OrchestrationRetryPolicy::default(),
                    runtime_profile_id: Some(candidate.executor_config.profile_id().cache_key()),
                    provider_id: Some(provider.id().to_string()),
                    provider_config: Some(
                        serde_json::to_value(&candidate.executor_config).map_err(|error| {
                            ApiError::BadRequest(format!(
                                "Cannot snapshot workflow arena candidate config: {error}"
                            ))
                        })?,
                    ),
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?,
        created_at: Utc::now(),
    };
    let orchestration =
        OrchestrationService::new(pool.clone(), Arc::new(deployment.agent_run_port().clone()));
    let orchestration_run_id = orchestration
        .start_run(
            orchestration_run_id,
            stable_arena_identity(orchestration_run_id, group.id, 0, "start-request"),
            &format!(
                "workflow-arena:{}:{}:{}:start",
                request.run_id, request.node_id, group.id
            ),
            request.run_id,
            &plan,
        )
        .await
        .map_err(orchestration_api_error)?;

    for candidate in candidates {
        enqueue_arena_candidate(
            &orchestration,
            pool,
            request,
            orchestration_run_id,
            candidate,
        )
        .await?;
    }
    while orchestration
        .deliver_next()
        .await
        .map_err(orchestration_api_error)?
    {}
    Ok(orchestration_run_id)
}

async fn enqueue_arena_candidate<P>(
    orchestration: &OrchestrationService<P>,
    pool: &SqlitePool,
    workflow_request: &ArenaNodeRequest,
    orchestration_run_id: Uuid,
    candidate: &ArenaCandidateLaunch,
) -> Result<ArenaCandidateRuntime, ApiError>
where
    P: executors::runtime::AgentRunPort + 'static,
{
    let provider = direct_provider_for_agent(candidate.executor_config.executor)?;
    let runtime_profile_id = candidate.executor_config.profile_id().cache_key();
    let node_execution_id =
        stable_arena_node_execution_id(orchestration_run_id, &candidate.node_key, 0);
    let request_id =
        stable_arena_identity(orchestration_run_id, node_execution_id, 0, "agent-request");
    let agent_run_id =
        stable_arena_identity(orchestration_run_id, node_execution_id, 0, "agent-run");
    let turn_id = stable_arena_identity(orchestration_run_id, node_execution_id, 0, "turn");
    let run_attempt_id =
        stable_arena_identity(orchestration_run_id, node_execution_id, 0, "attempt-1");
    let session_id = stable_arena_identity(orchestration_run_id, node_execution_id, 0, "session");
    let session = match Session::find_by_id(pool, session_id).await? {
        Some(session) => session,
        None => {
            Session::create(
                pool,
                &CreateSession {
                    executor: Some(candidate.executor_config.executor.to_string()),
                    name: candidate.workspace.name.clone(),
                },
                session_id,
                candidate.workspace.id,
            )
            .await?
        }
    };
    if session.workspace_id != candidate.workspace.id {
        return Err(ApiError::Conflict(format!(
            "Arena candidate session {session_id} belongs to workspace {} instead of {}",
            session.workspace_id, candidate.workspace.id
        )));
    }

    let workspace = WorkspaceReference {
        workspace_id: candidate.workspace.id,
        mode: match candidate.workspace.workspace_kind {
            WorkspaceKind::DirectFolder => WorkspaceMode::SharedWorkspace,
            WorkspaceKind::Worktree => WorkspaceMode::IsolatedWorktree,
        },
        path: candidate.workspace_path.clone(),
    };
    let created_at = Utc::now();
    let idempotency_key = format!(
        "workflow-arena:{}:{}:candidate:{}:create",
        workflow_request.run_id, workflow_request.node_id, candidate.workspace.id
    );
    let run_request = AgentRunRequestEnvelope {
        schema_version: AGENT_REQUEST_SCHEMA_VERSION,
        payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
        request_id,
        idempotency_key: idempotency_key.clone(),
        session_id,
        agent_run_id,
        turn_id,
        correlation_id: workflow_request.run_id,
        intent: AgentRunIntent::Initial,
        runtime_profile_id: runtime_profile_id.clone(),
        provider_id: provider.id().to_string(),
        workspace: workspace.clone(),
        input: CanonicalMessage {
            message_id: stable_arena_identity(orchestration_run_id, node_execution_id, 0, "input"),
            role: AgentRuntimeMessageRole::User,
            content: candidate.prompt.clone(),
        },
        created_at,
    };
    let attempt = RunAttemptRequest {
        schema_version: AGENT_REQUEST_SCHEMA_VERSION,
        payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
        request_id,
        idempotency_key: format!("{idempotency_key}:attempt:1"),
        session_id,
        agent_run_id,
        turn_id,
        run_attempt_id,
        attempt_number: 1,
        correlation_id: workflow_request.run_id,
        mode: RunAttemptMode::Launch,
        transport: provider.transport(),
        runtime_profile_id: runtime_profile_id.clone(),
        provider_id: provider.id().to_string(),
        workspace,
        capability_snapshot: direct_provider_capability_snapshot(provider, runtime_profile_id),
        executor_config: candidate.executor_config.clone(),
        selected_skills: None,
        reset_to_message_id: None,
        provider_session: None,
        created_at,
    };
    orchestration
        .enqueue_command(AgentRunPortCommandEnvelope {
            schema_version: ORCHESTRATION_COMMAND_SCHEMA_VERSION,
            command_id: stable_arena_identity(
                orchestration_run_id,
                node_execution_id,
                0,
                "create-command",
            ),
            idempotency_key,
            agent_run_id,
            orchestration_run_id: Some(orchestration_run_id),
            orchestration_node_execution_id: Some(node_execution_id),
            correlation_id: workflow_request.run_id,
            created_at,
            command: AgentRunPortCommand::Create {
                request: run_request,
                attempt,
            },
        })
        .await
        .map_err(orchestration_api_error)?;
    Ok(ArenaCandidateRuntime {
        orchestration_run_id,
        agent_run_id,
    })
}

async fn executor_config_from_value(value: Option<Value>) -> Result<ExecutorConfig, ApiError> {
    if let Some(value) = value {
        return serde_json::from_value(value).map_err(|err| {
            ApiError::BadRequest(format!(
                "Invalid workflow arena attempt executor config: {err}"
            ))
        });
    }

    let profile_id = ExecutorConfigs::get_cached()
        .get_recommended_executor_profile()
        .await
        .map_err(|err| {
            ApiError::BadRequest(format!(
                "No available executor profile for workflow arena attempt: {err}"
            ))
        })?;
    Ok(ExecutorConfig::from(profile_id))
}

fn validate_canonical_executor_config(config: &ExecutorConfig) -> Result<(), ApiError> {
    direct_provider_for_agent(config.executor)?;
    Ok(())
}

fn direct_provider_for_agent(agent: BaseCodingAgent) -> Result<DirectProvider, ApiError> {
    match agent {
        BaseCodingAgent::Gemini => Ok(DirectProvider::Gemini),
        BaseCodingAgent::Codex => Ok(DirectProvider::Codex),
        BaseCodingAgent::ClaudeCode => Ok(DirectProvider::ClaudeCode),
        BaseCodingAgent::OhMyPi => Ok(DirectProvider::OhMyPi),
        BaseCodingAgent::Opencode => Ok(DirectProvider::Opencode),
        BaseCodingAgent::DeepseekHarness => Ok(DirectProvider::DeepseekHarness),
        #[cfg(feature = "qa-mode")]
        BaseCodingAgent::QaMock => Err(ApiError::BadRequest(
            "QA mock is not a production Agent Runtime provider".to_string(),
        )),
    }
}

fn stable_arena_identity(
    orchestration_run_id: Uuid,
    node_execution_id: Uuid,
    iteration: u32,
    purpose: &str,
) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(orchestration_run_id.as_bytes());
    hasher.update(node_execution_id.as_bytes());
    hasher.update(iteration.to_le_bytes());
    hasher.update(purpose.as_bytes());
    uuid_from_digest(hasher.finalize())
}

fn stable_arena_node_execution_id(
    orchestration_run_id: Uuid,
    node_key: &str,
    iteration: u32,
) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(orchestration_run_id.as_bytes());
    hasher.update(node_key.as_bytes());
    hasher.update(iteration.to_le_bytes());
    uuid_from_digest(hasher.finalize())
}

fn uuid_from_digest(digest: impl AsRef<[u8]>) -> Uuid {
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_ref()[..16]);
    Uuid::from_bytes(bytes)
}

fn arena_plan_version(
    request: &ArenaNodeRequest,
    candidates: &[ArenaCandidateLaunch],
) -> Result<String, ApiError> {
    let mut hasher = Sha256::new();
    hasher.update(request.run_id.as_bytes());
    hasher.update(request.node_id.as_bytes());
    hasher.update(request.prompt.as_bytes());
    for candidate in candidates {
        hasher.update(candidate.workspace.id.as_bytes());
        hasher.update(candidate.node_key.as_bytes());
        hasher.update(candidate.prompt.as_bytes());
        hasher.update(
            serde_json::to_vec(&candidate.executor_config).map_err(|error| {
                ApiError::BadRequest(format!(
                    "Cannot snapshot workflow arena candidate config: {error}"
                ))
            })?,
        );
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn orchestration_api_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::BadRequest(format!(
        "Canonical arena orchestration rejected the operation: {error}"
    ))
}

async fn cleanup_failed_workflow_arena(
    pool: &SqlitePool,
    arena_group_id: Uuid,
) -> Result<(), ApiError> {
    let mut transaction = pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE workspaces
        SET archived = TRUE,
            updated_at = datetime('now', 'subsec')
        WHERE id IN (
            SELECT workspace_id
            FROM arena_candidates
            WHERE arena_group_id = ?
        )
        "#,
    )
    .bind(arena_group_id)
    .execute(&mut *transaction)
    .await?;
    let closed = sqlx::query(
        r#"
        UPDATE arena_groups
        SET lifecycle_status = 'closed',
            closed_at = datetime('now', 'subsec'),
            updated_at = datetime('now', 'subsec')
        WHERE id = ?
        "#,
    )
    .bind(arena_group_id)
    .execute(&mut *transaction)
    .await?;
    if closed.rows_affected() != 1 {
        return Err(ApiError::Conflict(format!(
            "Workflow Arena group {arena_group_id} disappeared during failure cleanup"
        )));
    }
    transaction.commit().await?;
    Ok(())
}

async fn apply_deployment_arena_winner(
    deployment: &DeploymentImpl,
    request: ArenaWinnerRequest,
) -> Result<ArenaWinnerExecution, ApiError> {
    let pool = &deployment.db().pool;
    let group = ArenaGroup::find_by_id(pool, request.arena_group_id)
        .await?
        .ok_or_else(|| ApiError::from(ArenaGroupError::NotFound))?;
    if group.winner_candidate_id.is_some() {
        return Err(ApiError::from(ArenaGroupError::AlreadyHasWinner {
            group_id: group.id,
        }));
    }
    let main_workspace = Workspace::find_by_id(pool, request.main_workspace_id)
        .await?
        .ok_or(ApiError::Workspace(WorkspaceError::WorkspaceNotFound))?;
    let candidate = ArenaCandidate::find_by_id(pool, request.candidate_id)
        .await?
        .ok_or_else(|| {
            ApiError::from(ArenaGroupError::CandidateNotInGroup {
                group_id: request.arena_group_id,
                candidate_id: request.candidate_id,
            })
        })?;
    if candidate.arena_group_id != request.arena_group_id {
        return Err(ApiError::from(ArenaGroupError::CandidateNotInGroup {
            group_id: request.arena_group_id,
            candidate_id: request.candidate_id,
        }));
    }
    let winner_workspace = Workspace::find_by_id(pool, candidate.workspace_id)
        .await?
        .ok_or(ApiError::Workspace(WorkspaceError::WorkspaceNotFound))?;

    let main_root = deployment
        .container()
        .ensure_container_exists(&main_workspace)
        .await?;
    let winner_root = deployment
        .container()
        .ensure_container_exists(&winner_workspace)
        .await?;
    let main_root = std::path::PathBuf::from(main_root);
    let winner_root = std::path::PathBuf::from(winner_root);

    let main_repos =
        WorkspaceRepo::find_repos_with_target_branch_for_workspace(pool, main_workspace.id).await?;
    let winner_repos =
        WorkspaceRepo::find_repos_with_target_branch_for_workspace(pool, winner_workspace.id)
            .await?;
    let winner_by_repo_id = winner_repos
        .iter()
        .map(|repo| (repo.repo.id, repo))
        .collect::<HashMap<_, _>>();

    let mut changed_files = 0usize;
    let mut changed_repos = Vec::new();
    let mut artifact_paths = Vec::new();
    let control_root = super::runner::workflow_control_root(pool, main_workspace.id).await?;
    let canonical_control_root = tokio::fs::canonicalize(&control_root).await?;

    for main_repo in &main_repos {
        let winner_repo = winner_by_repo_id.get(&main_repo.repo.id).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Winner workspace {} is missing repository {}",
                winner_workspace.id, main_repo.repo.name
            ))
        })?;
        let main_repo_path = main_root.join(&main_repo.repo.name);
        let winner_repo_path = winner_root.join(&winner_repo.repo.name);
        let base_commit = deployment.git().get_base_commit(
            &winner_repo_path,
            &winner_workspace.branch,
            &winner_repo.target_branch,
        )?;
        let file_paths = deployment
            .git()
            .get_diff_file_paths(&winner_repo_path, &base_commit)?;
        let patch = deployment
            .git()
            .get_diff_patch(&winner_repo_path, &base_commit)?;

        if patch.is_empty() {
            continue;
        }

        deployment.git().apply_patch(&main_repo_path, &patch)?;
        changed_files += file_paths.len();
        changed_repos.push(main_repo.repo.name.clone());
        for relative in file_paths {
            let path = main_repo_path.join(&relative);
            // Git deletions and symlinks outside the workspace are not readable
            // deliverables. Report file changes separately from this index.
            if let Ok(path) = tokio::fs::canonicalize(path).await {
                if path.starts_with(&canonical_control_root)
                    && tokio::fs::metadata(&path).await?.is_file()
                {
                    artifact_paths.push(
                        path.strip_prefix(&canonical_control_root)
                            .map_err(|_| {
                                ApiError::BadRequest(
                                    "Arena artifact is outside its workspace".into(),
                                )
                            })?
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }
    }

    mark_arena_winner(pool, request.arena_group_id, &candidate).await?;

    let winner_state = arena_candidate_run_state(deployment, group.id, winner_workspace.id).await?;
    let winner_summary = canonical_winner_summary(winner_workspace.id, &winner_state)?;
    let diff_summary = if changed_repos.is_empty() {
        "No file changes were applied from the winner workspace.".to_string()
    } else {
        format!(
            "Applied {changed_files} changed file(s) from {}.",
            changed_repos.join(", ")
        )
    };
    let output_text = if winner_summary.trim().is_empty() {
        diff_summary
    } else {
        format!("{winner_summary}\n\n{diff_summary}")
    };

    deployment
        .track_if_analytics_allowed(
            "workflow_arena_winner_applied",
            serde_json::json!({
                "arena_group_id": request.arena_group_id.to_string(),
                "winner_candidate_id": request.candidate_id.to_string(),
                "winner_workspace_id": winner_workspace.id.to_string(),
                "run_id": request.run_id.to_string(),
                "node_id": request.node_id,
                "changed_files": changed_files,
            }),
        )
        .await;

    artifact_paths.sort();
    artifact_paths.dedup();
    Ok(ArenaWinnerExecution {
        output_text,
        artifact_paths,
    })
}

async fn arena_candidate_run_state(
    deployment: &DeploymentImpl,
    arena_group_id: Uuid,
    workspace_id: Uuid,
) -> Result<RunState, ApiError> {
    let pool = &deployment.db().pool;
    let product_kind = serde_json::to_value(OrchestrationProductKind::Arena)
        .expect("orchestration product kind should serialize")
        .as_str()
        .expect("orchestration product kind should serialize as a string")
        .to_string();
    let identity: Option<(Uuid, Uuid)> = sqlx::query_as(
        r#"
        SELECT links.orchestration_run_id, links.agent_run_id
        FROM orchestration_runs runs
        JOIN orchestration_agent_run_links links
          ON links.orchestration_run_id = runs.id
        JOIN agent_runs agent_runs
          ON agent_runs.id = links.agent_run_id
        WHERE runs.product_kind = ?
          AND runs.source_definition_id = ?
          AND agent_runs.workspace_id = ?
        ORDER BY runs.created_at DESC
        LIMIT 1
        "#,
    )
    .bind(product_kind)
    .bind(arena_group_id)
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?;
    let (orchestration_run_id, agent_run_id) = identity.ok_or_else(|| {
        ApiError::Conflict(format!(
            "Arena candidate workspace {workspace_id} has no canonical AgentRun identity"
        ))
    })?;
    let orchestration =
        OrchestrationService::new(pool.clone(), Arc::new(deployment.agent_run_port().clone()));
    orchestration
        .reconcile_run(orchestration_run_id, orchestration_run_id)
        .await
        .map_err(orchestration_api_error)?;
    orchestration
        .query_agent_run(agent_run_id)
        .await
        .map_err(orchestration_api_error)
}

fn canonical_winner_summary(
    winner_workspace_id: Uuid,
    state: &RunState,
) -> Result<String, ApiError> {
    match state.status {
        AgentRunStatus::Succeeded => Ok(state
            .terminal_output
            .as_ref()
            .map(|output| output.content.clone())
            .unwrap_or_else(|| format!("Selected arena winner workspace {winner_workspace_id}"))),
        AgentRunStatus::Pending
        | AgentRunStatus::Starting
        | AgentRunStatus::Running
        | AgentRunStatus::AwaitingInput
        | AgentRunStatus::AwaitingApproval
        | AgentRunStatus::Cancelling => Err(ApiError::Conflict(format!(
            "Arena candidate workspace {winner_workspace_id} is not terminal (canonical status: {:?})",
            state.status
        ))),
        AgentRunStatus::Failed
        | AgentRunStatus::Cancelled
        | AgentRunStatus::Crashed
        | AgentRunStatus::AuditFailed => Err(ApiError::Conflict(format!(
            "Arena candidate workspace {winner_workspace_id} cannot be selected (canonical status: {:?})",
            state.status
        ))),
    }
}

async fn mark_arena_winner(
    pool: &SqlitePool,
    arena_group_id: Uuid,
    candidate: &ArenaCandidate,
) -> Result<(), ApiError> {
    if candidate.arena_group_id != arena_group_id {
        return Err(ApiError::from(ArenaGroupError::CandidateNotInGroup {
            group_id: arena_group_id,
            candidate_id: candidate.id,
        }));
    }
    let siblings = Workspace::find_by_arena_group_id(pool, arena_group_id).await?;

    ArenaGroup::select_winner(
        pool,
        arena_group_id,
        candidate.id,
        ArenaLifecycleStatus::Adopted,
    )
    .await?;
    for sibling in siblings {
        if sibling.id == candidate.workspace_id {
            continue;
        }
        Workspace::set_archived(pool, sibling.id, true).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    #[test]
    fn canonical_arena_config_accepts_frozen_executor_overrides() {
        let mut config = ExecutorConfig::new(BaseCodingAgent::Codex);
        config.model_id = Some(Some("openai/gpt-5.6-codex".to_string()));
        config.agent_id = Some("implementation".to_string());
        config.reasoning_id = Some(Some("high".to_string()));

        validate_canonical_executor_config(&config)
            .expect("RunAttemptRequest now represents all executor overrides");
    }

    #[tokio::test]
    async fn failed_workflow_arena_keeps_node_task_cardinality_and_closes_group() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");
        for statement in [
            r#"
            CREATE TABLE tasks (
                id BLOB PRIMARY KEY,
                project_id BLOB NOT NULL,
                issue_id BLOB NOT NULL
            )
            "#,
            r#"
            CREATE TABLE arena_groups (
                id BLOB PRIMARY KEY,
                task_id BLOB NOT NULL,
                lifecycle_status TEXT NOT NULL DEFAULT 'open',
                closed_at TEXT,
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
            )
            "#,
            r#"
            CREATE TABLE workspaces (
                id BLOB PRIMARY KEY,
                archived BOOLEAN NOT NULL DEFAULT FALSE,
                updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
            )
            "#,
            r#"
            CREATE TABLE arena_candidates (
                id BLOB PRIMARY KEY,
                arena_group_id BLOB NOT NULL,
                workspace_id BLOB NOT NULL
            )
            "#,
            r#"
            CREATE TABLE node_executions (
                id BLOB PRIMARY KEY,
                task_id BLOB,
                arena_group_id BLOB
            )
            "#,
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("create cleanup fixture table");
        }

        let task_id = Uuid::new_v4();
        let group_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();
        let node_execution_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tasks (id, project_id, issue_id) VALUES (?, ?, ?)")
            .bind(task_id)
            .bind(Uuid::new_v4())
            .bind(Uuid::new_v4())
            .execute(&pool)
            .await
            .expect("insert Arena child Task");
        sqlx::query("INSERT INTO arena_groups (id, task_id) VALUES (?, ?)")
            .bind(group_id)
            .bind(task_id)
            .execute(&pool)
            .await
            .expect("insert ArenaGroup");
        sqlx::query("INSERT INTO workspaces (id) VALUES (?)")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("insert candidate workspace");
        sqlx::query(
            "INSERT INTO arena_candidates (id, arena_group_id, workspace_id) VALUES (?, ?, ?)",
        )
        .bind(Uuid::new_v4())
        .bind(group_id)
        .bind(workspace_id)
        .execute(&pool)
        .await
        .expect("insert Arena candidate");
        sqlx::query("INSERT INTO node_executions (id, task_id, arena_group_id) VALUES (?, ?, ?)")
            .bind(node_execution_id)
            .bind(task_id)
            .bind(group_id)
            .execute(&pool)
            .await
            .expect("insert Arena NodeExecution");

        cleanup_failed_workflow_arena(&pool, group_id)
            .await
            .expect("close failed Workflow Arena");

        let node_binding: (Option<Uuid>, Option<Uuid>) =
            sqlx::query_as("SELECT task_id, arena_group_id FROM node_executions WHERE id = ?")
                .bind(node_execution_id)
                .fetch_one(&pool)
                .await
                .expect("load retained NodeExecution bindings");
        let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE id = ?")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .expect("count retained Arena child Task");
        let group_status: (String, Option<String>) =
            sqlx::query_as("SELECT lifecycle_status, closed_at FROM arena_groups WHERE id = ?")
                .bind(group_id)
                .fetch_one(&pool)
                .await
                .expect("load closed ArenaGroup");
        let workspace_archived: bool =
            sqlx::query_scalar("SELECT archived FROM workspaces WHERE id = ?")
                .bind(workspace_id)
                .fetch_one(&pool)
                .await
                .expect("load archived candidate workspace");

        assert_eq!(node_binding, (Some(task_id), Some(group_id)));
        assert_eq!(task_count, 1);
        assert_eq!(group_status.0, "closed");
        assert!(group_status.1.is_some());
        assert!(workspace_archived);
    }
}
