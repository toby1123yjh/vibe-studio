//! Host-owned, read-only monitoring. Registered inside signed relay routes.
mod context;
mod query;
mod snapshot;
#[cfg(test)]
mod tests;
pub mod types;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    routing::{get, post},
};
use deployment::Deployment;
use serde::Deserialize;
use types::*;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

#[derive(Debug, Default, Deserialize)]
struct ChildrenQuery {
    workflow_run_id: Option<Uuid>,
    snapshot_id: Option<Uuid>,
    root_snapshot_id: Option<Uuid>,
    #[serde(default)]
    page: u32,
}

#[derive(Debug, Default, Deserialize)]
struct ContextQuery {
    session_id: Option<Uuid>,
    agent_run_id: Option<Uuid>,
    #[serde(default)]
    refresh: bool,
}

async fn dashboard_query(
    State(deployment): State<DeploymentImpl>,
    Json(query): Json<DashboardQuery>,
) -> Result<Json<ApiResponse<DashboardPageResponse>>, ApiError> {
    Ok(Json(ApiResponse::success(
        query::execute(&deployment.db().pool, query).await?,
    )))
}

async fn children(
    State(deployment): State<DeploymentImpl>,
    Path((kind, id)): Path<(DashboardWorkKind, Uuid)>,
    Query(query): Query<ChildrenQuery>,
) -> Result<Json<ApiResponse<DashboardChildrenPage>>, ApiError> {
    if query.page > 100_000 {
        return Err(ApiError::BadRequest(
            "Dashboard child page exceeds the supported range".into(),
        ));
    }
    let scope = query::data_scope(&deployment.db().pool);
    let filters = match query.root_snapshot_id {
        Some(id) => snapshot::filters(&scope, id).ok_or_else(|| {
            ApiError::Conflict("The Dashboard list snapshot expired; refresh the list".into())
        })?,
        None => DashboardFilters::default(),
    };
    let read = query::DashboardRead::load(&deployment.db().pool, false).await?;
    let work = DashboardWorkRef { kind, id };
    let response = query::child_page(
        &scope,
        read.works.iter().find(|w| w.row.work == work),
        &DashboardExpandedPage {
            work: work.clone(),
            workflow_run_id: query.workflow_run_id,
            snapshot_id: query.snapshot_id,
            page: query.page,
        },
        &filters,
    )?;
    Ok(Json(ApiResponse::success(response)))
}

async fn context(
    State(deployment): State<DeploymentImpl>,
    Path((kind, id)): Path<(DashboardWorkKind, Uuid)>,
    Query(query): Query<ContextQuery>,
) -> Result<Json<ApiResponse<DashboardContext>>, ApiError> {
    let response = context::read(
        &deployment.db().pool,
        DashboardWorkRef { kind, id },
        query.session_id,
        query.agent_run_id,
        query.refresh,
    )
    .await?;
    Ok(Json(ApiResponse::success(response)))
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/dashboard/query", post(dashboard_query))
        .route("/dashboard/works/{kind}/{id}/children", get(children))
        .route("/dashboard/works/{kind}/{id}/context", get(context))
        .layer(DefaultBodyLimit::max(64 * 1024))
}
