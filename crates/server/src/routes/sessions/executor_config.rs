use std::str::FromStr;

use axum::{Extension, extract::State, response::Json};
use db::models::session::Session;
use deployment::Deployment;
use executors::{
    executors::BaseCodingAgent, profile::ExecutorConfig, runtime::ProviderSessionReference,
};
use sqlx::{SqlitePool, types::Json as SqlJson};
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

pub(super) async fn get_executor_config(
    Extension(session): Extension<Session>,
    State(deployment): State<DeploymentImpl>,
) -> Result<Json<ApiResponse<Option<ExecutorConfig>>>, ApiError> {
    let config = read_executor_config(&deployment.db().pool, session.id).await?;
    Ok(Json(ApiResponse::success(config)))
}

/// Read the immutable launch request, not script processes or mutable presets.
async fn read_executor_config(
    pool: &SqlitePool,
    session_id: Uuid,
) -> Result<Option<ExecutorConfig>, ApiError> {
    if let Some(config) =
        crate::workflow_runtime::mcp_launch::captured_main_agent_config(pool, session_id).await?
    {
        return Ok(Some(config));
    }
    let config = sqlx::query_scalar::<_, SqlJson<ExecutorConfig>>(
        r#"
        SELECT json_extract(attempt.request_envelope, '$.executor_config')
        FROM agent_run_attempts attempt
        JOIN agent_runs run ON run.id = attempt.agent_run_id
        WHERE run.session_id = ?
        ORDER BY julianday(attempt.created_at) DESC, attempt.rowid DESC
        LIMIT 1
        "#,
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    if let Some(config) = config {
        return Ok(Some(config.0));
    }

    // Native adoption can bind a session before its first VK run launches.
    let binding = sqlx::query_scalar::<_, SqlJson<ProviderSessionReference>>(
        "SELECT session_reference FROM agent_provider_sessions WHERE session_id = ? LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    binding
        .map(|binding| config_from_binding(&binding.0))
        .transpose()
}

fn config_from_binding(binding: &ProviderSessionReference) -> Result<ExecutorConfig, ApiError> {
    if let Some(config) = binding
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("profile_context"))
        .and_then(|context| context.get("executor_config"))
    {
        return serde_json::from_value(config.clone())
            .map_err(|error| ApiError::Database(sqlx::Error::Decode(Box::new(error))));
    }
    let (executor, variant) = binding.runtime_profile_id.split_once(':').map_or(
        (binding.runtime_profile_id.as_str(), None),
        |(executor, variant)| (executor, Some(variant.to_owned())),
    );
    let executor = BaseCodingAgent::from_str(executor)
        .map_err(|error| ApiError::Database(sqlx::Error::Decode(Box::new(error))))?;
    Ok(ExecutorConfig {
        variant,
        ..ExecutorConfig::new(executor)
    })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use executors::runtime::PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION;
    use serde_json::json;

    use super::*;

    fn binding(profile: &str) -> ProviderSessionReference {
        ProviderSessionReference {
            schema_version: PROVIDER_SESSION_REFERENCE_SCHEMA_VERSION,
            provider_id: "codex".into(),
            runtime_profile_id: profile.into(),
            provider_session_id: "native-session".into(),
            observed_at: Utc::now(),
            metadata: None,
        }
    }

    #[test]
    fn native_binding_preserves_profile_and_adoption_overrides() {
        let mut reference = binding("CODEX:PLAN");
        assert_eq!(
            config_from_binding(&reference).unwrap().variant.as_deref(),
            Some("PLAN")
        );
        let config = ExecutorConfig {
            variant: Some("PLAN".into()),
            model_id: Some("provider/custom/model".into()),
            agent_id: Some("plan".into()),
            reasoning_id: Some("high".into()),
            ..ExecutorConfig::new(BaseCodingAgent::Codex)
        };
        reference.metadata = Some(json!({"profile_context": {"executor_config": config}}));
        assert_eq!(config_from_binding(&reference).unwrap(), config);
        assert!(config_from_binding(&binding("UNKNOWN:PLAN")).is_err());
    }

    #[tokio::test]
    async fn empty_session_does_not_borrow_another_sessions_binding() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE agent_runs (id BLOB, session_id BLOB);
             CREATE TABLE agent_run_attempts (agent_run_id BLOB, request_envelope TEXT, created_at TEXT);
             CREATE TABLE agent_provider_sessions (session_id BLOB, session_reference TEXT);
             CREATE TABLE workflow_main_session_bindings (session_id BLOB PRIMARY KEY, main_agent_config_json TEXT NOT NULL);",
        ).execute(&pool).await.unwrap();
        let owner = Uuid::new_v4();
        sqlx::query("INSERT INTO agent_provider_sessions VALUES (?, ?)")
            .bind(owner)
            .bind(SqlJson(binding("CODEX:PLAN")))
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            read_executor_config(&pool, Uuid::new_v4())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            read_executor_config(&pool, owner)
                .await
                .unwrap()
                .unwrap()
                .variant
                .as_deref(),
            Some("PLAN")
        );
    }

    #[tokio::test]
    async fn latest_attempt_restores_all_overrides_and_respects_session_boundary() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE agent_runs (id BLOB, session_id BLOB);
             CREATE TABLE agent_run_attempts (agent_run_id BLOB, request_envelope TEXT, created_at TEXT);
             CREATE TABLE agent_provider_sessions (session_id BLOB, session_reference TEXT);
             CREATE TABLE workflow_main_session_bindings (session_id BLOB PRIMARY KEY, main_agent_config_json TEXT NOT NULL);",
        ).execute(&pool).await.unwrap();
        let session_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let sibling_run_id = Uuid::new_v4();
        for (run, session) in [(run_id, session_id), (sibling_run_id, Uuid::new_v4())] {
            sqlx::query("INSERT INTO agent_runs VALUES (?, ?)")
                .bind(run)
                .bind(session)
                .execute(&pool)
                .await
                .unwrap();
        }
        let expected = ExecutorConfig {
            variant: Some("PLAN".into()),
            model_id: Some("provider/model:latest".into()),
            agent_id: Some("plan".into()),
            reasoning_id: Some("high".into()),
            permission_policy: Some(executors::model_selector::PermissionPolicy::Supervised),
            provider_options: Some(executors::profile::ProviderRunOptions::Codex {
                service_tier: Some("priority".into()),
                plan_mode_reasoning_effort: Some("high".into()),
            }),
            ..ExecutorConfig::new(BaseCodingAgent::Codex)
        };
        // Mixed SQLite/RFC3339 dates must order by time, not their separator.
        for (run, date, config) in [
            (
                run_id,
                "2026-09-07T09:00:00Z",
                ExecutorConfig::new(BaseCodingAgent::Codex),
            ),
            (run_id, "2026-09-07 10:00:00", expected.clone()),
            (
                sibling_run_id,
                "2026-09-07T11:00:00Z",
                ExecutorConfig::new(BaseCodingAgent::ClaudeCode),
            ),
        ] {
            sqlx::query("INSERT INTO agent_run_attempts VALUES (?, ?, ?)")
                .bind(run)
                .bind(SqlJson(json!({"executor_config": config})))
                .bind(date)
                .execute(&pool)
                .await
                .unwrap();
        }
        assert_eq!(
            read_executor_config(&pool, session_id).await.unwrap(),
            Some(expected)
        );
        // A malformed latest snapshot is an error, never fallback to an older run/default.
        sqlx::query("INSERT INTO agent_run_attempts VALUES (?, ?, '2026-09-07T12:00:00Z')")
            .bind(run_id)
            .bind("{}")
            .execute(&pool)
            .await
            .unwrap();
        assert!(read_executor_config(&pool, session_id).await.is_err());
    }
}
