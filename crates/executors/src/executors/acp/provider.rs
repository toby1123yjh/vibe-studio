//! Shared transport mechanics for provider-owned ACP adapters.

use std::{path::Path, sync::Arc};

use super::{
    AcpAgentHarness,
    session_config::{
        AcpDialect, is_catalog_context_env, observed_catalog, scoped_catalog_identity,
    },
};
use crate::{
    agent_settings::native_model_defaults_for,
    approvals::ExecutorApprovalService,
    command::{CmdOverrides, CommandBuildError, CommandBuilder, apply_overrides},
    env::ExecutionEnv,
    executor_discovery::ExecutorDiscoveredOptions,
    executors::{
        AppendPrompt, ExecutorError, SpawnedChild,
        provider_adapter::{DirectControl, DirectIntent, DirectProvider},
    },
    model_selector::{ModelCatalogSource, ModelInfo, ModelSelectorConfig, PermissionPolicy},
    workflow_mcp::{FORWARDED_ENV_KEYS, ScopedWorkflowMcp, WorkflowMcpReadiness},
};

pub(crate) fn build_command(
    base: &str,
    native_args: &[&str],
    overrides: &CmdOverrides,
) -> Result<CommandBuilder, CommandBuildError> {
    apply_overrides(
        CommandBuilder::new(base).extend_params(native_args.iter().copied()),
        overrides,
    )
}

pub(crate) fn encode_control(control: DirectControl) -> Result<Vec<u8>, serde_json::Error> {
    if matches!(
        control,
        DirectControl::Cancel | DirectControl::Approve { .. }
    ) {
        // This is a typed owned-peer route, not a native frame. The host
        // audits only the exact bytes returned by successful peer delivery.
        return Ok(Vec::new());
    }
    Err(serde_json::Error::io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "ACP controls require the active provider connection and its native session id",
    )))
}

pub(crate) fn workflow_mcp_servers(
    env: &ExecutionEnv,
) -> Result<Vec<agent_client_protocol::McpServer>, ExecutorError> {
    let Some(config) = ScopedWorkflowMcp::from_execution_env(env)? else {
        return Ok(Vec::new());
    };
    // DSH scrubs ambient TOKEN vars and neither provider expands Gemini's
    // placeholders. These are execution-only protocol values, never written
    // to native settings, profiles or audit/request logs.
    let explicit_env = FORWARDED_ENV_KEYS
        .into_iter()
        .filter_map(|key| {
            env.vars
                .get(key)
                .map(|value| agent_client_protocol::EnvVariable::new(key, value.clone()))
        })
        .collect();
    Ok(vec![agent_client_protocol::McpServer::Stdio(
        agent_client_protocol::McpServerStdio::new(&config.server_name, &config.executable)
            .args(config.args().into_iter().map(str::to_owned).collect())
            .env(explicit_env),
    )])
}

pub(crate) struct AcpProviderLaunch<'a> {
    pub dialect: AcpDialect,
    pub namespace: &'static str,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub mode: Option<&'a str>,
    pub append_prompt: &'a AppendPrompt,
    pub cmd: &'a CmdOverrides,
    pub approvals: Option<Arc<dyn ExecutorApprovalService>>,
}

impl AcpProviderLaunch<'_> {
    pub async fn launch(
        self,
        intent: DirectIntent,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        command: crate::command::CommandParts,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        if !current_dir.is_absolute() {
            return Err(ExecutorError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "ACP requires an absolute working directory",
            )));
        }
        let mut env = env.clone().with_profile(self.cmd);
        let readiness = WorkflowMcpReadiness::start(&mut env).await?;
        let mut identity_cmd = self.cmd.clone();
        for (key, value) in &env.vars {
            if !is_catalog_context_env(key) {
                identity_cmd
                    .env
                    .get_or_insert_with(Default::default)
                    .insert(key.clone(), value.clone());
            }
        }
        let mut harness = AcpAgentHarness::with_session_namespace(self.namespace)
            .with_dialect(self.dialect)
            .with_mcp_servers(workflow_mcp_servers(&env)?)
            .with_catalog_identity(scoped_catalog_identity(
                self.dialect,
                current_dir,
                &identity_cmd,
            ))
            .with_workflow_readiness(readiness);
        if let Some(model) = self.model {
            harness = harness.with_model(model);
        }
        if let Some(effort) = self.effort {
            harness = harness.with_reasoning_effort(effort);
        }
        if let Some(mode) = self.mode {
            harness = harness.with_mode(mode);
        }
        let prompt = self.append_prompt.combine_prompt(prompt);
        match (intent, session_id) {
            (DirectIntent::Initial, None) | (DirectIntent::Review, None) => {
                harness
                    .spawn_with_command(
                        current_dir,
                        prompt,
                        command,
                        &env,
                        self.cmd,
                        self.approvals,
                    )
                    .await
            }
            (
                DirectIntent::FollowUp | DirectIntent::Resume | DirectIntent::Review,
                Some(session),
            ) => {
                harness
                    .spawn_native_resume_with_command(
                        current_dir,
                        prompt,
                        session,
                        command,
                        &env,
                        self.cmd,
                        self.approvals,
                    )
                    .await
            }
            _ => Err(ExecutorError::FollowUpNotSupported(
                "Native ACP continuation requires an explicit provider session".to_string(),
            )),
        }
    }
}

/// Discovery must never create a native session just to read configuration.
/// Use provider settings and only catalogs actually observed during a run.
pub(crate) fn discover_options(
    provider: DirectProvider,
    dialect: AcpDialect,
    workdir: Option<&Path>,
    project: Option<&Path>,
    cmd: &CmdOverrides,
    model: Option<&str>,
) -> ExecutorDiscoveredOptions {
    let fallback_cwd = std::env::current_dir().ok();
    let cwd = workdir.or(project).or(fallback_cwd.as_deref());
    let native_defaults = native_model_defaults_for(provider, cwd, cmd);
    let configured_model = model.map(str::to_owned).or_else(|| {
        native_defaults
            .as_ref()
            .and_then(|defaults| defaults.model_id.clone())
    });
    let observed = cwd.and_then(|cwd| {
        observed_catalog(dialect, cwd, &scoped_catalog_identity(dialect, cwd, cmd))
    });
    let source = if observed.is_some() {
        Some(ModelCatalogSource::Observed)
    } else {
        configured_model
            .as_ref()
            .map(|_| ModelCatalogSource::Configured)
    };
    let mut selector = observed.unwrap_or_else(|| {
        let mut selector = ModelSelectorConfig {
            permissions: vec![PermissionPolicy::Auto, PermissionPolicy::Supervised],
            ..Default::default()
        };
        if let Some(model) = configured_model {
            // A configured ID is displayable, not proof of supported effort.
            selector.models.push(ModelInfo {
                id: model.clone(),
                name: model.clone(),
                provider_id: None,
                reasoning_options: Vec::new(),
                ..Default::default()
            });
            selector.default_model = Some(model);
        }
        selector
    });
    selector.native_defaults = native_defaults;
    selector.catalog_source = source;
    ExecutorDiscoveredOptions {
        model_selector: selector,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inactive_connection_never_fabricates_session_control() {
        assert!(encode_control(DirectControl::Cancel).unwrap().is_empty());
        assert!(
            encode_control(DirectControl::Approve {
                request_id: "pending".to_string(),
                approved: true,
                reason: None
            })
            .unwrap()
            .is_empty()
        );
        assert!(
            encode_control(DirectControl::Input {
                request_id: "not-supported".to_string(),
                text: "input".to_string()
            })
            .is_err()
        );
    }

    #[test]
    fn cold_discovery_preserves_opaque_config_without_fabricating_effort_or_sessions() {
        let directory = tempfile::tempdir().unwrap();
        let cmd = CmdOverrides {
            base_command_override: Some("must-never-run".into()),
            ..Default::default()
        };
        for (provider, dialect) in [
            (DirectProvider::Opencode, AcpDialect::Opencode),
            (DirectProvider::DeepseekHarness, AcpDialect::DeepseekHarness),
        ] {
            let discovered = discover_options(
                provider,
                dialect,
                Some(directory.path()),
                None,
                &cmd,
                Some(r#"["provider/name","model/name"]"#),
            );
            assert_eq!(discovered.model_selector.models.len(), 1);
            assert_eq!(
                discovered.model_selector.models[0].id,
                r#"["provider/name","model/name"]"#
            );
            assert!(
                discovered.model_selector.models[0]
                    .reasoning_options
                    .is_empty()
            );
            let unset =
                discover_options(provider, dialect, Some(directory.path()), None, &cmd, None);
            assert!(unset.model_selector.models.is_empty());
            assert!(unset.model_selector.default_model.is_none());
        }
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn observed_catalog_keeps_configured_inheritance_separate_and_prefers_execution_directory() {
        use crate::executors::acp::session_config::{ACP_CATALOG_EVENT, cache_catalog_extension};

        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("original-project");
        let cwd = root.path().join("actual-worktree");
        std::fs::create_dir_all(project.join(".gemini")).unwrap();
        std::fs::create_dir_all(cwd.join(".gemini")).unwrap();
        std::fs::write(
            project.join(".gemini/settings.json"),
            r#"{"model":{"name":"wrong-original"}}"#,
        )
        .unwrap();
        std::fs::write(
            cwd.join(".gemini/settings.json"),
            r#"{"model":{"name":"configured-native"}}"#,
        )
        .unwrap();
        let cmd = CmdOverrides {
            env: Some(std::collections::HashMap::from([(
                "GEMINI_CLI_HOME".into(),
                root.path().join("empty-home").display().to_string(),
            )])),
            ..Default::default()
        };
        let cold = discover_options(
            DirectProvider::Gemini,
            AcpDialect::Gemini,
            Some(&cwd),
            Some(&project),
            &cmd,
            None,
        );
        assert_eq!(
            cold.model_selector.catalog_source,
            Some(ModelCatalogSource::Configured)
        );
        assert_eq!(
            cold.model_selector
                .native_defaults
                .unwrap()
                .model_id
                .as_deref(),
            Some("configured-native")
        );
        assert!(cold.model_selector.models[0].reasoning_options.is_empty());
        let scope = scoped_catalog_identity(AcpDialect::Gemini, &cwd, &cmd);
        cache_catalog_extension(
            "gemini",
            &crate::runtime::AgentEventPayload::ProviderExtension {
                provider_namespace: "gemini".into(),
                provider_event: ACP_CATALOG_EVENT.into(),
                payload: serde_json::json!({"working_dir":cwd,"scope_id":scope,
                "model_selector":{"models":[{"id":"observed-native","name":"Observed","reasoning_options":[]}],
                    "providers":[],"agents":[],"permissions":[],"default_model":"observed-native"}}),
            },
        );
        let observed = discover_options(
            DirectProvider::Gemini,
            AcpDialect::Gemini,
            Some(&cwd),
            Some(&project),
            &cmd,
            None,
        );
        assert_eq!(
            observed.model_selector.catalog_source,
            Some(ModelCatalogSource::Observed)
        );
        assert_eq!(
            observed.model_selector.default_model.as_deref(),
            Some("observed-native")
        );
        assert_eq!(
            observed
                .model_selector
                .native_defaults
                .unwrap()
                .model_id
                .as_deref(),
            Some("configured-native")
        );
    }
}
