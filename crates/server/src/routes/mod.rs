use axum::{
    Router,
    routing::{IntoMakeService, get},
};
use tower_http::{compression::CompressionLayer, validate_request::ValidateRequestHeaderLayer};

use crate::{DeploymentImpl, middleware};

pub mod agent_commands;
pub mod agent_install;
pub mod agent_runs;
pub mod agent_settings;
pub mod agent_tools;
pub mod approvals;
pub mod config;
pub mod containers;
pub mod dashboard;
pub mod filesystem;
// pub mod github;
pub mod attachments;
pub mod events;
pub mod execution_data;
pub mod execution_processes;
pub mod frontend;
pub mod git_import;
pub mod health;
pub mod host_relay;
pub mod integrations;
pub mod local_remote;
pub mod oauth;
pub mod organizations;
pub mod preview;
pub(crate) mod project_store;
pub mod relay_auth;
pub mod releases;
pub mod remote;
pub mod repo;
pub mod scheduled_tasks;
pub mod scratch;
pub mod search;
pub mod sessions;
pub mod ssh_session;
pub mod tags;
pub mod terminal;
pub mod webrtc;
pub mod workflow_files;
pub mod workflow_management;
pub mod workflows;
pub mod workspaces;

pub fn router(deployment: DeploymentImpl) -> IntoMakeService<Router> {
    let relay_signed_routes = Router::new()
        .route("/health", get(health::health_check))
        .merge(config::router())
        .merge(agent_commands::router())
        .merge(agent_install::router())
        .merge(agent_settings::router())
        .merge(agent_tools::router())
        .merge(containers::router(&deployment))
        .merge(workspaces::router(&deployment))
        .merge(agent_runs::router(&deployment))
        .merge(execution_processes::router(&deployment))
        .merge(execution_data::router())
        .merge(dashboard::router())
        .merge(tags::router(&deployment))
        .merge(oauth::router())
        .merge(organizations::router())
        .merge(filesystem::router())
        .merge(repo::router())
        .merge(git_import::router())
        .merge(integrations::admin_router())
        .merge(workflow_management::router())
        .merge(workflow_files::router())
        .merge(events::router(&deployment))
        .merge(approvals::router())
        .merge(scratch::router(&deployment))
        .merge(search::router(&deployment))
        .merge(preview::api_router())
        .merge(releases::router())
        .merge(sessions::router(&deployment))
        .merge(terminal::router())
        .route("/ssh-session", get(ssh_session::ssh_session_ws))
        .nest("/local", local_remote::router(&deployment))
        .nest("/remote", remote::router())
        .merge(webrtc::router())
        .nest("/attachments", attachments::routes())
        .layer(axum::middleware::from_fn_with_state(
            deployment.clone(),
            middleware::sign_relay_response,
        ))
        .layer(axum::middleware::from_fn_with_state(
            deployment.clone(),
            middleware::require_relay_request_signature,
        ))
        .with_state(deployment.clone());

    let api_routes = Router::new()
        .nest("/integrations/v1", integrations::router(&deployment))
        .merge(relay_auth::router())
        .merge(host_relay::router(&deployment))
        .merge(relay_signed_routes)
        .layer(ValidateRequestHeaderLayer::custom(
            middleware::validate_origin,
        ))
        .layer(axum::middleware::from_fn(middleware::log_server_errors))
        .with_state(deployment);

    Router::new()
        .route("/", get(frontend::serve_frontend_root))
        .route("/{*path}", get(frontend::serve_frontend))
        .nest("/api", api_routes)
        .layer(CompressionLayer::new())
        .into_make_service()
}
