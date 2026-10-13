use std::path::Path;

use chrono::{DateTime, Utc};
use db::models::{
    agent_runtime::{AgentProviderSessionRecord, AgentRuntimePersistenceError},
    session::{Session, SessionError},
    workspace::{Workspace, WorkspaceKind},
};
use deployment::Deployment;
use executors::{
    actions::SelectedSkill,
    executors::provider_adapter::DirectProvider,
    profile::{ExecutorConfig, runtime_profile_ids_match},
    provider_policy::direct_provider_capability_snapshot,
    runtime::{
        AGENT_REQUEST_PAYLOAD_VERSION, AGENT_REQUEST_SCHEMA_VERSION, AgentCapability,
        AgentRunIntent, AgentRunPort, AgentRunPortError, AgentRunPortSnapshot,
        AgentRunRequestEnvelope, AgentRuntimeMessageRole, CanonicalMessage, CapabilitySnapshot,
        CapabilityState, PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION, ProviderSessionReference,
        RunAttemptMode, RunAttemptRequest, WorkspaceMode, WorkspaceReference,
    },
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{SqlitePool, types::Json};
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

pub(crate) struct AgentRunLaunch {
    pub intent: AgentRunIntent,
    pub mode: RunAttemptMode,
    pub prompt: String,
    pub selected_skills: Option<Vec<SelectedSkill>>,
    pub executor_config: ExecutorConfig,
    pub provider_session: Option<ProviderSessionReference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRunDispatch {
    Immediate,
    Reserved,
}

pub(crate) async fn has_active_agent_run_for_session(
    pool: &SqlitePool,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM agent_runs
            WHERE session_id = ?
              AND status NOT IN ('succeeded', 'failed', 'cancelled', 'crashed', 'audit_failed')
        )
        "#,
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
}

pub(super) async fn has_active_agent_run_for_workspace(
    pool: &SqlitePool,
    workspace_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM agent_runs
            WHERE workspace_id = ?
              AND status NOT IN ('succeeded', 'failed', 'cancelled', 'crashed', 'audit_failed')
        )
        "#,
    )
    .bind(workspace_id)
    .fetch_one(pool)
    .await
}

pub(crate) async fn validate_session_executor(
    pool: &SqlitePool,
    session: &Session,
    executor_config: &ExecutorConfig,
) -> Result<(), ApiError> {
    crate::workflow_runtime::mcp_launch::validate_main_agent_config(
        pool,
        session.id,
        executor_config,
    )
    .await?;
    let requested = executor_config.executor.to_string();
    if let Some(expected) = session.executor.as_ref()
        && expected != &requested
    {
        return Err(ApiError::Session(SessionError::ExecutorMismatch {
            expected: expected.clone(),
            actual: requested,
        }));
    }
    if session.executor.is_none() {
        Session::update_executor(pool, session.id, &requested).await?;
    }
    Ok(())
}

pub(crate) fn direct_provider(
    executor_config: &ExecutorConfig,
) -> Result<DirectProvider, ApiError> {
    DirectProvider::from_base_agent(executor_config.executor).ok_or_else(|| {
        ApiError::BadRequest(format!(
            "{} is not a V1 direct Agent Runtime provider",
            executor_config.executor
        ))
    })
}

pub(crate) async fn latest_provider_session(
    pool: &SqlitePool,
    session_id: Uuid,
    provider: DirectProvider,
    runtime_profile_id: &str,
) -> Result<Option<ProviderSessionReference>, sqlx::Error> {
    let reference = sqlx::query_scalar::<_, Json<ProviderSessionReference>>(
        r#"
        SELECT session_reference
        FROM agent_provider_sessions
        WHERE session_id = ? AND provider_id = ?
        LIMIT 1
        "#,
    )
    .bind(session_id)
    .bind(provider.id())
    .fetch_optional(pool)
    .await?;
    Ok(reference.map(|reference| reference.0).filter(|reference| {
        runtime_profile_ids_match(&reference.runtime_profile_id, runtime_profile_id)
    }))
}

pub(super) fn explicit_provider_session(
    provider: DirectProvider,
    runtime_profile_id: &str,
    provider_session_id: Option<&str>,
    observed_at: DateTime<Utc>,
) -> Option<ProviderSessionReference> {
    provider_session_id
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
        .map(|session_id| ProviderSessionReference {
            schema_version: PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION,
            provider_id: provider.id().to_string(),
            runtime_profile_id: runtime_profile_id.to_string(),
            provider_session_id: session_id.to_string(),
            observed_at,
            metadata: Some(serde_json::json!({ "source": "explicit_native_resume" })),
        })
}

pub(super) fn native_adoption_reference(
    mut reference: ProviderSessionReference,
    executor_config: &ExecutorConfig,
    selected_skills: Option<&Vec<SelectedSkill>>,
    runtime_scope_path: &Path,
    native_source_scope_path: Option<&Path>,
) -> ProviderSessionReference {
    let profile_context = native_adoption_profile_context(executor_config, selected_skills);
    let profile_fingerprint = native_adoption_profile_fingerprint(&profile_context);
    reference.metadata = Some(serde_json::json!({
        "source": "native_adopted",
        "profile_fingerprint": profile_fingerprint,
        "profile_context": profile_context,
        "scope_path": runtime_scope_path.to_string_lossy().to_string(),
        "native_source_scope_path": native_source_scope_path
            .map(|path| path.to_string_lossy().to_string()),
    }));
    reference
}

fn native_adoption_profile_context(
    executor_config: &ExecutorConfig,
    selected_skills: Option<&Vec<SelectedSkill>>,
) -> Value {
    let mut skills = selected_skills.cloned().unwrap_or_default();
    skills.sort_by(|left, right| {
        left.name.cmp(&right.name).then_with(|| {
            left.path
                .to_string_lossy()
                .cmp(&right.path.to_string_lossy())
        })
    });
    serde_json::json!({
        "executor_config": executor_config,
        "selected_skills": skills,
    })
}

fn native_adoption_profile_fingerprint(profile_context: &Value) -> String {
    format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(profile_context).expect("profile context serializes"))
    )
}

fn provider_session_source(reference: &ProviderSessionReference) -> Option<&str> {
    reference
        .metadata
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("source"))
        .and_then(Value::as_str)
}

pub(super) fn validate_native_resume_identity(
    provider: DirectProvider,
    provider_session_id: &str,
    scope_path: &Path,
) -> Result<(), ApiError> {
    let provider_uses_authoritative_resume = matches!(
        provider,
        DirectProvider::Gemini
            | DirectProvider::OhMyPi
            | DirectProvider::Opencode
            | DirectProvider::DeepseekHarness
    );
    let provider_session_id = provider_session_id.trim();
    executors::executors::provider_adapter::validate_native_session_id(
        provider,
        provider_session_id,
    )
    .map_err(ApiError::BadRequest)?;
    if scope_path.as_os_str().is_empty() {
        return Err(ApiError::BadRequest(
            "Native session adoption requires a non-empty working directory scope".to_string(),
        ));
    }

    let preview = super::native_history::get_native_agent_session_preview(
        provider.id(),
        provider_session_id,
        1,
        &[scope_path.to_path_buf()],
        false,
    );
    if preview.is_none() {
        if provider_uses_authoritative_resume {
            // Gemini and Oh My Pi can resume an explicit native id even when
            // their local history directory is unavailable to VK. The native
            // adapter/CLI is authoritative for whether that id exists; do not
            // reject it or silently substitute VK history here.
            return Ok(());
        }
        return Err(ApiError::BadRequest(format!(
            "Native session {provider_session_id:?} was not found in the selected {} working directory",
            provider.id()
        )));
    }
    Ok(())
}

pub(crate) async fn validate_session_provider_binding(
    pool: &SqlitePool,
    session_id: Uuid,
    provider: DirectProvider,
    runtime_profile_id: &str,
    requested: Option<&ProviderSessionReference>,
    executor_config: &ExecutorConfig,
    selected_skills: Option<&Vec<SelectedSkill>>,
    requested_runtime_scope_path: Option<&Path>,
) -> Result<(), ApiError> {
    let existing = sqlx::query_as::<_, (String, Json<ProviderSessionReference>)>(
        r#"
        SELECT provider_id, session_reference
        FROM agent_provider_sessions
        WHERE session_id = ?
        LIMIT 1
        "#,
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?
    .map(|(provider_id, reference)| (provider_id, reference.0));

    if let Some((existing_provider_id, existing)) = existing {
        if existing_provider_id != provider.id() {
            return Err(ApiError::BadRequest(format!(
                "Session is bound to provider {existing_provider_id}; it cannot switch to {}",
                provider.id()
            )));
        }
        if !runtime_profile_ids_match(&existing.runtime_profile_id, runtime_profile_id) {
            return Err(ApiError::BadRequest(format!(
                "Session is bound to runtime profile {}; create a new VK session for {}",
                existing.runtime_profile_id, runtime_profile_id
            )));
        }
        if let Some(requested) = requested
            && existing.provider_session_id != requested.provider_session_id
        {
            return Err(ApiError::BadRequest(format!(
                "Session is already bound to native session {}; it cannot switch to {}",
                existing.provider_session_id, requested.provider_session_id
            )));
        }
        if provider_session_source(&existing) == Some("native_adopted") {
            let expected_scope = existing
                .metadata
                .as_ref()
                .and_then(Value::as_object)
                .and_then(|metadata| metadata.get("scope_path"))
                .and_then(Value::as_str);
            if let Some(requested_runtime_scope_path) = requested_runtime_scope_path
                && expected_scope != Some(requested_runtime_scope_path.to_string_lossy().as_ref())
            {
                return Err(ApiError::BadRequest(
                    "Native session working directory changed; create a new VK session".to_string(),
                ));
            }

            let expected = existing
                .metadata
                .as_ref()
                .and_then(Value::as_object)
                .and_then(|metadata| metadata.get("profile_fingerprint"))
                .and_then(Value::as_str);
            // Fingerprints include the serialized variant. Use the already-bound
            // spelling for the equivalent omitted/explicit DEFAULT alias only.
            let mut bound_config = executor_config.clone();
            if matches!(bound_config.variant.as_deref(), None | Some("DEFAULT")) {
                bound_config.variant = existing
                    .runtime_profile_id
                    .split_once(':')
                    .map(|(_, variant)| variant.to_string());
            }
            let actual_context = native_adoption_profile_context(&bound_config, selected_skills);
            let actual = native_adoption_profile_fingerprint(&actual_context);
            if expected != Some(actual.as_str()) {
                return Err(ApiError::BadRequest(
                    "Native session runtime profile changed; create a new VK session".to_string(),
                ));
            }
        }
    }

    if let Some(requested) = requested {
        let owner = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT session_id
            FROM agent_provider_sessions
            WHERE provider_id = ? AND provider_session_id = ?
            LIMIT 1
            "#,
        )
        .bind(provider.id())
        .bind(&requested.provider_session_id)
        .fetch_optional(pool)
        .await?;
        if owner.is_some_and(|owner| owner != session_id) {
            return Err(ApiError::BadRequest(format!(
                "Native session {} is already adopted by another VK session",
                requested.provider_session_id
            )));
        }
    }

    Ok(())
}

pub(super) async fn bind_provider_session(
    pool: &SqlitePool,
    session_id: Uuid,
    reference: &ProviderSessionReference,
) -> Result<(), ApiError> {
    AgentProviderSessionRecord::upsert(pool, Uuid::new_v4(), session_id, reference)
        .await
        .map_err(|error| match error {
            AgentRuntimePersistenceError::Database(error) => ApiError::Database(error),
            AgentRuntimePersistenceError::IdentityConflict { key, .. } => {
                ApiError::BadRequest(format!("Native session binding conflict: {key}"))
            }
            other => ApiError::BadRequest(format!("Failed to bind native session: {other}")),
        })
}

pub(super) async fn create_agent_run(
    deployment: &DeploymentImpl,
    session: &Session,
    workspace: &Workspace,
    workspace_path: String,
    launch: AgentRunLaunch,
    dispatch: AgentRunDispatch,
) -> Result<AgentRunPortSnapshot, ApiError> {
    // Use the same gate as queued-message consumption and Session deletion.
    // The earlier route preflight alone cannot prevent competing follow-ups
    // or a system workflow handover from starting in the same native Session.
    let guard = deployment
        .queued_message_service()
        .lock_session(session.id)
        .await;
    create_agent_run_under_session_guard(
        deployment,
        session,
        workspace,
        workspace_path,
        launch,
        AgentRunOrigin {
            dispatch,
            request_id: Uuid::new_v4(),
            agent_run_id: Uuid::new_v4(),
            turn_id: Uuid::new_v4(),
            run_attempt_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            message_id: Uuid::new_v4(),
            role: AgentRuntimeMessageRole::User,
            created_at: Utc::now(),
        },
        &guard,
    )
    .await
}

/// System turns supply their durable origin before any launch is possible.
/// They never enter, consume or replace the ordinary user's queued-message slot.
pub(crate) struct AgentRunOrigin {
    pub dispatch: AgentRunDispatch,
    pub request_id: Uuid,
    pub agent_run_id: Uuid,
    pub turn_id: Uuid,
    pub run_attempt_id: Uuid,
    pub correlation_id: Uuid,
    pub message_id: Uuid,
    pub role: AgentRuntimeMessageRole,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn create_agent_run_under_session_guard(
    deployment: &DeploymentImpl,
    session: &Session,
    workspace: &Workspace,
    workspace_path: String,
    mut launch: AgentRunLaunch,
    origin: AgentRunOrigin,
    _session_guard: &tokio::sync::OwnedMutexGuard<()>,
) -> Result<AgentRunPortSnapshot, ApiError> {
    let pool = &deployment.db().pool;
    let current_session = Session::find_by_id(pool, session.id)
        .await?
        .ok_or(SessionError::NotFound)?;
    if current_session.workspace_id != workspace.id {
        return Err(ApiError::Conflict(
            "Session workspace changed before launch".into(),
        ));
    }
    if has_active_agent_run_for_session(pool, session.id).await? {
        return Err(ApiError::Conflict(
            "A coding agent is already running for this session. Queue the follow-up instead."
                .into(),
        ));
    }
    validate_session_executor(pool, &current_session, &launch.executor_config).await?;
    crate::workflow_runtime::mcp_launch::validate_main_agent_launch(
        deployment,
        session.id,
        workspace.id,
        &launch.executor_config,
    )
    .await?;
    let provider = direct_provider(&launch.executor_config)?;
    let runtime_profile_id = launch.executor_config.profile_id().cache_key();
    if launch.provider_session.is_none() {
        // Preflight may have preceded another turn's completion. Re-read the
        // native binding under the shared gate instead of opening a new native
        // conversation from a stale Initial request.
        launch.provider_session =
            latest_provider_session(pool, session.id, provider, &runtime_profile_id).await?;
        if launch.provider_session.is_some() && launch.intent == AgentRunIntent::Initial {
            launch.intent = AgentRunIntent::FollowUp;
            launch.mode = RunAttemptMode::Resume;
        }
    }
    validate_session_provider_binding(
        pool,
        session.id,
        provider,
        &runtime_profile_id,
        launch.provider_session.as_ref(),
        &launch.executor_config,
        launch.selected_skills.as_ref(),
        Some(Path::new(&workspace_path)),
    )
    .await?;
    let capability_snapshot =
        direct_provider_capability_snapshot(provider, runtime_profile_id.clone());
    validate_required_capabilities(
        launch.intent,
        launch.provider_session.is_some(),
        &capability_snapshot,
    )?;

    let AgentRunOrigin {
        dispatch,
        request_id,
        agent_run_id,
        turn_id,
        run_attempt_id,
        correlation_id,
        message_id,
        role,
        created_at,
    } = origin;
    let idempotency_key = format!("session:{}:request:{request_id}:create", session.id);
    let workspace = WorkspaceReference {
        workspace_id: workspace.id,
        mode: match workspace.workspace_kind {
            WorkspaceKind::DirectFolder => WorkspaceMode::SharedWorkspace,
            WorkspaceKind::Worktree => WorkspaceMode::IsolatedWorktree,
        },
        path: workspace_path,
    };
    let request = AgentRunRequestEnvelope {
        schema_version: AGENT_REQUEST_SCHEMA_VERSION,
        payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
        request_id,
        idempotency_key: idempotency_key.clone(),
        session_id: session.id,
        agent_run_id,
        turn_id,
        correlation_id,
        intent: launch.intent,
        runtime_profile_id: runtime_profile_id.clone(),
        provider_id: provider.id().to_string(),
        workspace: workspace.clone(),
        input: CanonicalMessage {
            message_id,
            role,
            content: launch.prompt,
        },
        created_at,
    };
    let attempt = RunAttemptRequest {
        schema_version: AGENT_REQUEST_SCHEMA_VERSION,
        payload_version: AGENT_REQUEST_PAYLOAD_VERSION,
        request_id,
        idempotency_key: format!("{idempotency_key}:attempt:1"),
        session_id: session.id,
        agent_run_id,
        turn_id,
        run_attempt_id,
        attempt_number: 1,
        correlation_id,
        mode: launch.mode,
        transport: provider.transport(),
        runtime_profile_id,
        provider_id: provider.id().to_string(),
        workspace,
        capability_snapshot,
        executor_config: launch.executor_config,
        selected_skills: launch.selected_skills.filter(|skills| !skills.is_empty()),
        reset_to_message_id: None,
        provider_session: launch.provider_session,
        created_at,
    };

    let port = deployment.agent_run_port();
    let persisted_agent_run_id = match dispatch {
        AgentRunDispatch::Immediate => port.create(request, attempt).await,
        AgentRunDispatch::Reserved => port.reserve(request, attempt).await,
    }
    .map_err(agent_run_port_error)?;
    port.query(persisted_agent_run_id)
        .await
        .map_err(agent_run_port_error)
}

fn validate_required_capabilities(
    intent: AgentRunIntent,
    has_provider_session: bool,
    snapshot: &CapabilitySnapshot,
) -> Result<(), ApiError> {
    if matches!(intent, AgentRunIntent::FollowUp) {
        if !has_provider_session {
            return Err(ApiError::BadRequest(
                "A canonical follow-up requires an observed or explicitly selected native session"
                    .to_string(),
            ));
        }
        require_native_capability(snapshot, AgentCapability::SessionResume)?;
    }
    if matches!(intent, AgentRunIntent::Review) {
        require_native_capability(snapshot, AgentCapability::Review)?;
    }
    Ok(())
}

fn require_native_capability(
    snapshot: &CapabilitySnapshot,
    capability: AgentCapability,
) -> Result<(), ApiError> {
    match snapshot.resolve(capability) {
        CapabilityState::Native => Ok(()),
        state => Err(ApiError::BadRequest(format!(
            "Provider {} cannot perform {capability:?}: frozen capability is {state:?}",
            snapshot.provider_id
        ))),
    }
}

pub(super) fn agent_run_port_error(error: AgentRunPortError) -> ApiError {
    match error {
        AgentRunPortError::Rejected(message) => ApiError::BadRequest(message),
        AgentRunPortError::NotFound(agent_run_id) => {
            ApiError::BadRequest(format!("Canonical AgentRun {agent_run_id} was not found"))
        }
        AgentRunPortError::Unavailable(message) => ApiError::BadGateway(message),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use executors::{
        actions::SelectedSkill,
        executors::{BaseCodingAgent, provider_adapter::DirectProvider},
        profile::ExecutorConfig,
        runtime::{AgentRunIntent, RunAttemptMode},
    };

    use super::{
        direct_provider, explicit_provider_session, native_adoption_profile_context,
        native_adoption_profile_fingerprint, native_adoption_reference,
        validate_native_resume_identity,
    };

    async fn active_run_test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool");
        sqlx::query(
            "CREATE TABLE agent_runs (session_id BLOB NOT NULL, workspace_id BLOB NOT NULL, status TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .expect("create test agent_runs table");
        pool
    }

    async fn provider_binding_test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool");
        sqlx::query(
            r#"
            CREATE TABLE agent_provider_sessions (
                session_id BLOB NOT NULL,
                provider_id TEXT NOT NULL,
                runtime_profile_id TEXT NOT NULL,
                provider_session_id TEXT NOT NULL,
                session_reference TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create provider binding table");
        pool
    }

    #[tokio::test]
    async fn default_alias_lookup_resumes_the_existing_native_session() {
        for (stored, requested) in [("CODEX:DEFAULT", "CODEX"), ("CODEX", "CODEX:DEFAULT")] {
            let pool = provider_binding_test_pool().await;
            let session_id = uuid::Uuid::new_v4();
            let config = ExecutorConfig::new(BaseCodingAgent::Codex);
            let reference = explicit_provider_session(
                DirectProvider::Codex,
                stored,
                Some("native-session"),
                chrono::Utc::now(),
            )
            .unwrap();
            sqlx::query("INSERT INTO agent_provider_sessions VALUES (?, ?, ?, ?, ?)")
                .bind(session_id)
                .bind("codex")
                .bind(stored)
                .bind("native-session")
                .bind(serde_json::to_string(&reference).unwrap())
                .execute(&pool)
                .await
                .unwrap();
            super::validate_session_provider_binding(
                &pool,
                session_id,
                DirectProvider::Codex,
                requested,
                None,
                &config,
                None,
                None,
            )
            .await
            .unwrap();
            let resumed =
                super::latest_provider_session(&pool, session_id, DirectProvider::Codex, requested)
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(resumed, reference);
            assert!(
                super::validate_session_provider_binding(
                    &pool,
                    session_id,
                    DirectProvider::Codex,
                    "CODEX:PLAN",
                    None,
                    &config,
                    None,
                    None
                )
                .await
                .is_err()
            );
            assert!(
                super::latest_provider_session(
                    &pool,
                    session_id,
                    DirectProvider::Codex,
                    "CODEX:PLAN"
                )
                .await
                .unwrap()
                .is_none()
            );
        }
    }

    #[test]
    fn direct_provider_mapping_covers_the_four_v1_runtimes() {
        let cases = [
            (BaseCodingAgent::Gemini, DirectProvider::Gemini),
            (BaseCodingAgent::Codex, DirectProvider::Codex),
            (BaseCodingAgent::ClaudeCode, DirectProvider::ClaudeCode),
            (BaseCodingAgent::OhMyPi, DirectProvider::OhMyPi),
            (BaseCodingAgent::Opencode, DirectProvider::Opencode),
            (
                BaseCodingAgent::DeepseekHarness,
                DirectProvider::DeepseekHarness,
            ),
        ];
        for (agent, expected) in cases {
            assert_eq!(
                direct_provider(&ExecutorConfig::new(agent)).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn explicit_native_resume_reference_is_trimmed_and_provider_scoped() {
        let observed_at = chrono::Utc::now();
        let reference = explicit_provider_session(
            DirectProvider::Codex,
            "CODEX:DEFAULT",
            Some("  native-thread  "),
            observed_at,
        )
        .expect("provider session reference");

        assert_eq!(reference.provider_session_id, "native-thread");
        assert_eq!(reference.provider_id, "codex");
        assert_eq!(reference.runtime_profile_id, "CODEX:DEFAULT");
        assert_eq!(reference.observed_at, observed_at);
    }

    #[test]
    fn explicit_native_resume_reference_rejects_blank_ids() {
        assert!(
            explicit_provider_session(
                DirectProvider::ClaudeCode,
                "CLAUDE_CODE",
                Some("  "),
                chrono::Utc::now(),
            )
            .is_none()
        );
    }

    #[test]
    fn native_adoption_reference_records_provenance_and_profile_context() {
        let config = ExecutorConfig::new(BaseCodingAgent::Codex);
        let skills = vec![SelectedSkill {
            name: "review".to_string(),
            path: PathBuf::from(".codex/skills/review"),
        }];
        let reference = native_adoption_reference(
            explicit_provider_session(
                DirectProvider::Codex,
                &config.profile_id().cache_key(),
                Some("native-session"),
                chrono::Utc::now(),
            )
            .unwrap(),
            &config,
            Some(&skills),
            std::path::Path::new("C:\\workspace"),
            Some(std::path::Path::new("C:\\source")),
        );

        let metadata = reference.metadata.expect("adoption metadata");
        assert_eq!(metadata["source"], "native_adopted");
        assert!(metadata["profile_fingerprint"].as_str().is_some());
        assert_eq!(metadata["scope_path"], "C:\\workspace");
        assert_eq!(metadata["native_source_scope_path"], "C:\\source");
    }

    #[test]
    fn native_adoption_profile_fingerprint_is_independent_of_skill_order() {
        let config = ExecutorConfig::new(BaseCodingAgent::Codex);
        let first = vec![
            SelectedSkill {
                name: "zeta".to_string(),
                path: PathBuf::from(".codex/skills/zeta"),
            },
            SelectedSkill {
                name: "alpha".to_string(),
                path: PathBuf::from(".codex/skills/alpha"),
            },
        ];
        let second = vec![first[1].clone(), first[0].clone()];
        assert_eq!(
            native_adoption_profile_fingerprint(&native_adoption_profile_context(
                &config,
                Some(&first)
            )),
            native_adoption_profile_fingerprint(&native_adoption_profile_context(
                &config,
                Some(&second)
            ))
        );
    }

    #[tokio::test]
    async fn native_adoption_binding_rejects_provider_profile_scope_and_id_changes() {
        let pool = provider_binding_test_pool().await;
        let session_id = uuid::Uuid::new_v4();
        let config = ExecutorConfig::new(BaseCodingAgent::Codex);
        let skills = vec![SelectedSkill {
            name: "review".to_string(),
            path: PathBuf::from(".codex/skills/review"),
        }];
        let reference = native_adoption_reference(
            explicit_provider_session(
                DirectProvider::Codex,
                &config.profile_id().cache_key(),
                Some("native-session"),
                chrono::Utc::now(),
            )
            .unwrap(),
            &config,
            Some(&skills),
            std::path::Path::new("C:/vk-worktree"),
            Some(std::path::Path::new("C:/native-source")),
        );
        sqlx::query(
            "INSERT INTO agent_provider_sessions (session_id, provider_id, runtime_profile_id, provider_session_id, session_reference) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(session_id)
        .bind("codex")
        .bind(&reference.runtime_profile_id)
        .bind(&reference.provider_session_id)
        .bind(serde_json::to_string(&reference).unwrap())
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            super::validate_session_provider_binding(
                &pool,
                session_id,
                DirectProvider::Codex,
                &config.profile_id().cache_key(),
                Some(&reference),
                &config,
                Some(&skills),
                Some(std::path::Path::new("C:/vk-worktree")),
            )
            .await
            .is_ok()
        );

        let mut explicit_default_config = config.clone();
        explicit_default_config.variant = Some("DEFAULT".to_string());
        super::validate_session_provider_binding(
            &pool,
            session_id,
            DirectProvider::Codex,
            "CODEX:DEFAULT",
            Some(&reference),
            &explicit_default_config,
            Some(&skills),
            Some(std::path::Path::new("C:/vk-worktree")),
        )
        .await
        .expect("equivalent DEFAULT spelling must preserve the native adoption fingerprint");

        let scope_error = super::validate_session_provider_binding(
            &pool,
            session_id,
            DirectProvider::Codex,
            &config.profile_id().cache_key(),
            Some(&reference),
            &config,
            Some(&skills),
            Some(std::path::Path::new("C:/other-worktree")),
        )
        .await
        .expect_err("scope changes must fail");
        assert!(
            scope_error
                .to_string()
                .contains("working directory changed")
        );

        let mut changed_config = config.clone();
        changed_config.model_id = Some("different-model".to_string());
        let profile_error = super::validate_session_provider_binding(
            &pool,
            session_id,
            DirectProvider::Codex,
            &config.profile_id().cache_key(),
            Some(&reference),
            &changed_config,
            Some(&skills),
            Some(std::path::Path::new("C:/vk-worktree")),
        )
        .await
        .expect_err("profile changes must fail");
        assert!(
            profile_error
                .to_string()
                .contains("runtime profile changed")
        );

        let provider_error = super::validate_session_provider_binding(
            &pool,
            session_id,
            DirectProvider::ClaudeCode,
            "CLAUDE_CODE",
            None,
            &ExecutorConfig::new(BaseCodingAgent::ClaudeCode),
            None,
            Some(std::path::Path::new("C:/vk-worktree")),
        )
        .await
        .expect_err("provider changes must fail");
        assert!(provider_error.to_string().contains("cannot switch"));
    }

    #[test]
    fn canonical_initial_and_resume_modes_remain_distinct() {
        assert_ne!(AgentRunIntent::Initial, AgentRunIntent::FollowUp);
        assert_ne!(RunAttemptMode::Launch, RunAttemptMode::Resume);
    }

    #[test]
    fn explicit_gemini_and_omp_resume_ids_remain_provider_authoritative() {
        let scope = std::path::Path::new("C:/native-source");
        for provider in [
            DirectProvider::Gemini,
            DirectProvider::OhMyPi,
            DirectProvider::Opencode,
            DirectProvider::DeepseekHarness,
        ] {
            assert!(
                validate_native_resume_identity(
                    provider,
                    "provider-owned-id-that-is-not-locally-readable",
                    scope,
                )
                .is_ok()
            );
        }
    }

    #[test]
    fn codex_and_claude_resume_ids_still_require_discoverable_history() {
        let scope = std::path::Path::new("C:/native-source");
        for provider in [DirectProvider::Codex, DirectProvider::ClaudeCode] {
            assert!(
                validate_native_resume_identity(
                    provider,
                    "definitely-not-a-local-session-id",
                    scope,
                )
                .is_err()
            );
        }
    }

    #[tokio::test]
    async fn active_run_guards_use_canonical_status_for_session_and_workspace() {
        let pool = active_run_test_pool().await;
        let session_id = uuid::Uuid::new_v4();
        let workspace_id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO agent_runs (session_id, workspace_id, status) VALUES (?, ?, ?)")
            .bind(session_id)
            .bind(workspace_id)
            .bind("running")
            .execute(&pool)
            .await
            .expect("insert running AgentRun");

        assert!(
            super::has_active_agent_run_for_session(&pool, session_id)
                .await
                .unwrap()
        );
        assert!(
            super::has_active_agent_run_for_workspace(&pool, workspace_id)
                .await
                .unwrap()
        );

        sqlx::query("UPDATE agent_runs SET status = 'succeeded'")
            .execute(&pool)
            .await
            .expect("terminalize AgentRun");
        assert!(
            !super::has_active_agent_run_for_session(&pool, session_id)
                .await
                .unwrap()
        );
        assert!(
            !super::has_active_agent_run_for_workspace(&pool, workspace_id)
                .await
                .unwrap()
        );
    }
}
