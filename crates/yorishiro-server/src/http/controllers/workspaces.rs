use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use yorishiro_core::repositories::tenancy::WorkspaceRecord;
use yorishiro_core::repositories::{entities, relations, schemas, tenancy};
use yorishiro_core::{ResultExt, YorishiroError};

use crate::error::ApiError;
use crate::http::controllers::require_tenant_admin;
use crate::http::middleware::auth::AuthContext;
use crate::state::AppState;

/// Fetches a workspace and confirms it belongs to `tenant_id`, so a caller can never probe or
/// act on another tenant's workspace by guessing its id -- `identity.workspaces` has no RLS of
/// its own (it's read through the admin `identity_pool`), so this check is the only thing
/// enforcing that boundary for these handlers.
async fn get_workspace_in_tenant(
    state: &AppState,
    tenant_id: Uuid,
    workspace_id: Uuid,
) -> Result<WorkspaceRecord, ApiError> {
    let workspace = tenancy::get_workspace(&state.identity_pool, workspace_id).await?;
    if workspace.tenant_id != tenant_id {
        return Err(YorishiroError::NotFound {
            message: format!("workspace '{workspace_id}' was not found"),
        }
        .into());
    }
    Ok(workspace)
}

#[utoipa::path(
    get,
    path = "/api/workspaces",
    responses(
        (status = 200, description = "Workspaces belonging to the caller's tenant", body = Vec<WorkspaceRecord>),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
    ),
    tag = "workspaces",
)]
pub async fn list_workspaces(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
) -> Result<Json<Vec<WorkspaceRecord>>, ApiError> {
    let workspaces = tenancy::list_workspaces(&state.identity_pool, ctx.tenant_id).await?;
    Ok(Json(workspaces))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateWorkspaceRequest {
    pub name: String,
    /// Cap on the number of entities this workspace may hold. Omit for unlimited.
    pub max_entities: Option<i32>,
}

#[utoipa::path(
    post,
    path = "/api/workspaces",
    request_body = CreateWorkspaceRequest,
    responses(
        (status = 201, description = "Workspace created", body = WorkspaceRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Not a tenant owner/admin", body = crate::error::ApiErrorBody),
        (status = 409, description = "The tenant has reached its workspace limit", body = crate::error::ApiErrorBody),
    ),
    tag = "workspaces",
)]
pub async fn create_workspace(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Json(body): Json<CreateWorkspaceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_tenant_admin(&state, ctx.tenant_id, ctx.user_id).await?;

    let workspace = tenancy::create_workspace(
        &state.identity_pool,
        ctx.tenant_id,
        &body.name,
        body.max_entities,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(workspace)))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceDetail {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub max_entities: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub entity_count: i64,
    pub relation_count: i64,
    /// Currently *active* schemas only (one per distinct schema name) -- not a raw row count,
    /// which would also include archived versions.
    pub schema_count: i64,
}

#[utoipa::path(
    get,
    path = "/api/workspaces/{id}",
    params(("id" = Uuid, Path, description = "Workspace ID")),
    responses(
        (status = 200, description = "Workspace detail, including entity/relation/schema counts", body = WorkspaceDetail),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 404, description = "Workspace not found", body = crate::error::ApiErrorBody),
    ),
    tag = "workspaces",
)]
pub async fn get_workspace(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkspaceDetail>, ApiError> {
    let workspace = get_workspace_in_tenant(&state, ctx.tenant_id, id).await?;

    let mut conn = state
        .tenant_db
        .acquire_for_workspace(ctx.tenant_id, workspace.id)
        .await
        .internal()?;
    let entity_count = entities::count(&mut conn, workspace.id).await?;
    let relation_count = relations::count(&mut conn, workspace.id).await?;
    let schema_count = schemas::count_active(&mut conn, workspace.id).await?;

    Ok(Json(WorkspaceDetail {
        id: workspace.id,
        tenant_id: workspace.tenant_id,
        name: workspace.name,
        max_entities: workspace.max_entities,
        created_at: workspace.created_at,
        entity_count,
        relation_count,
        schema_count,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/workspaces/{id}",
    params(("id" = Uuid, Path, description = "Workspace ID")),
    responses(
        (status = 204, description = "Workspace (and everything under it) deleted"),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Not a tenant owner/admin", body = crate::error::ApiErrorBody),
        (status = 404, description = "Workspace not found", body = crate::error::ApiErrorBody),
        (status = 409, description = "This is the tenant's only workspace", body = crate::error::ApiErrorBody),
    ),
    tag = "workspaces",
)]
pub async fn delete_workspace(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    require_tenant_admin(&state, ctx.tenant_id, ctx.user_id).await?;
    get_workspace_in_tenant(&state, ctx.tenant_id, id).await?;

    // A tenant with zero workspaces has no way to issue itself a new API key through this
    // server's own REST API (login/create-workspace both require one), so this would be a
    // self-lockout rather than a reversible mistake.
    let remaining = tenancy::list_workspaces(&state.identity_pool, ctx.tenant_id).await?;
    if remaining.len() <= 1 {
        return Err(YorishiroError::Conflict {
            message: "cannot delete a tenant's only remaining workspace".into(),
        }
        .into());
    }

    tenancy::delete_workspace(&state.identity_pool, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
