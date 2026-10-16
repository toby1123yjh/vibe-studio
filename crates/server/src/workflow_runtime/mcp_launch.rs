//! Main-Session configuration is captured at preparation, not re-resolved
//! from a publication or user preset on each turn. This is a read-only
//! preflight; only the actual local launch issues a scoped credential.
use deployment::Deployment;
use executors::{profile::ExecutorConfig, workflow_mcp::matches_snapshot};
use sqlx::{SqlitePool, types::Json};
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

pub async fn captured_main_agent_config(
    pool: &SqlitePool,
    session_id: Uuid,
) -> Result<Option<ExecutorConfig>, ApiError> {
    sqlx::query_scalar::<_, Json<ExecutorConfig>>(
        "SELECT main_agent_config_json FROM workflow_main_session_bindings WHERE session_id = ?",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .map(|config| config.map(|config| config.0))
    .map_err(Into::into)
}

pub async fn validate_main_agent_config(
    pool: &SqlitePool,
    session_id: Uuid,
    requested: &ExecutorConfig,
) -> Result<(), ApiError> {
    if let Some(expected) = captured_main_agent_config(pool, session_id).await?
        && !matches_snapshot(&expected, requested)
    {
        return Err(ApiError::Conflict(
            "Workflow main Session uses its captured Agent, model, reasoning and permissions; create a new main Session to change them".into(),
        ));
    }
    Ok(())
}

pub async fn validate_main_agent_launch(
    deployment: &DeploymentImpl,
    session_id: Uuid,
    workspace_id: Uuid,
    requested: &ExecutorConfig,
) -> Result<(), ApiError> {
    let Some(expected) = captured_main_agent_config(&deployment.db().pool, session_id).await?
    else {
        return Ok(());
    };
    if !matches_snapshot(&expected, requested) {
        return Err(ApiError::Conflict(
            "Workflow main Agent configuration cannot change after Session preparation".into(),
        ));
    }
    let context = super::management::read_workflow_context(&deployment.db().pool, session_id)
        .await?
        .ok_or_else(|| {
            ApiError::Forbidden("Workflow main Session binding is unavailable".into())
        })?;
    if context.workspace_id != workspace_id {
        return Err(ApiError::Forbidden(
            "Workflow main Session workspace changed".into(),
        ));
    }
    deployment
        .agent_run_port()
        .validate_workflow_mcp_launch()
        .await
        .map_err(|error| ApiError::BadGateway(error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use executors::{executors::BaseCodingAgent, model_selector::PermissionPolicy};

    use super::*;

    #[tokio::test]
    async fn prepared_main_session_locks_full_snapshot_before_first_turn() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE workflow_main_session_bindings (session_id BLOB PRIMARY KEY,main_agent_config_json TEXT NOT NULL)")
            .execute(&pool).await.unwrap();
        let session_id = Uuid::new_v4();
        let config = ExecutorConfig {
            model_id: Some("configured-model".into()),
            reasoning_id: Some("high".into()),
            permission_policy: Some(PermissionPolicy::Supervised),
            provider_options: Some(executors::profile::ProviderRunOptions::Codex {
                service_tier: Some("priority".into()),
                plan_mode_reasoning_effort: Some("high".into()),
            }),
            ..ExecutorConfig::new(BaseCodingAgent::Codex)
        };
        sqlx::query("INSERT INTO workflow_main_session_bindings VALUES (?,?)")
            .bind(session_id)
            .bind(Json(&config))
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            captured_main_agent_config(&pool, session_id).await.unwrap(),
            Some(config.clone())
        );
        assert!(
            validate_main_agent_config(&pool, session_id, &config)
                .await
                .is_ok()
        );
        let mut alias = config.clone();
        alias.variant = Some("DEFAULT".into());
        assert!(
            validate_main_agent_config(&pool, session_id, &alias)
                .await
                .is_ok()
        );
        for changed in [
            ExecutorConfig {
                model_id: Some("another".into()),
                ..config.clone()
            },
            ExecutorConfig {
                reasoning_id: None,
                ..config.clone()
            },
            ExecutorConfig {
                permission_policy: None,
                ..config.clone()
            },
            ExecutorConfig {
                provider_options: None,
                ..config.clone()
            },
            ExecutorConfig::new(BaseCodingAgent::ClaudeCode),
        ] {
            assert!(
                validate_main_agent_config(&pool, session_id, &changed)
                    .await
                    .is_err()
            );
        }
        assert!(
            validate_main_agent_config(
                &pool,
                Uuid::new_v4(),
                &ExecutorConfig::new(BaseCodingAgent::ClaudeCode)
            )
            .await
            .is_ok()
        );
    }
}
