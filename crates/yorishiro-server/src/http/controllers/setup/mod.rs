use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use sea_query::{Alias, Asterisk, Func, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use yorishiro_core::repositories::tenancy::{self, MembershipRole};
use yorishiro_core::services::auth;
use yorishiro_core::{ResultExt, YorishiroError};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Iden)]
enum Tenants {
    Table,
}

/// Whether the community-edition first-run setup wizard is enabled at all. Gated on
/// `YORISHIRO_MAX_TENANTS` resolving to an actual cap (`yorishiro-server` defaults this to `1`;
/// hosted deployments set it to `0`, i.e. unlimited) rather than a separate flag, so the wizard
/// can never be enabled on a deployment that lacks the tenant cap that makes it safe -- without
/// that cap, anyone could hit `POST /setup` between a hosted deploy and its first real tenant and
/// claim ownership of the whole deployment.
fn wizard_enabled() -> bool {
    matches!(tenancy::max_tenants_from_env(), Ok(Some(_)))
}

async fn tenant_count(pool: &sqlx::PgPool) -> Result<i64, YorishiroError> {
    let (sql, values) = Query::select()
        .expr(Func::count(sea_query::Expr::col(Asterisk)))
        .from((Alias::new("identity"), Tenants::Table))
        .build_sqlx(PostgresQueryBuilder);
    let (count,): (i64,) = sqlx::query_as_with(&sql, values)
        .fetch_one(pool)
        .await
        .internal()?;
    Ok(count)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SetupStatusResponse {
    /// True when the wizard is enabled and no tenant exists yet -- the client should show the
    /// setup form instead of the login form.
    pub setup_required: bool,
}

#[utoipa::path(
    get,
    path = "/setup/status",
    responses(
        (status = 200, description = "Whether first-run setup should be shown", body = SetupStatusResponse),
    ),
    security(()),
    tag = "auth",
)]
pub async fn status(State(state): State<AppState>) -> Result<Json<SetupStatusResponse>, ApiError> {
    let setup_required = wizard_enabled() && tenant_count(&state.identity_pool).await? == 0;
    Ok(Json(SetupStatusResponse { setup_required }))
}

/// Unlike `/auth/signup`, which redeems an invite into an *existing* tenant, this creates the
/// deployment's first tenant/workspace from scratch -- there is no one to invite from yet. Only
/// email/password are asked for (see `web/`'s setup screen); the tenant and workspace get fixed
/// default names, matching a self-hosted deployment's "one operator, one tenant" reality.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetupRequest {
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SetupResponse {
    pub user_id: Uuid,
    pub email: String,
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    /// A freshly issued API key, scoped to the new owner account -- shown only here, same as
    /// `/auth/login`'s, so the setup screen can log straight into the dashboard afterward.
    pub api_key: String,
}

#[utoipa::path(
    post,
    path = "/setup",
    request_body = SetupRequest,
    responses(
        (status = 201, description = "Deployment initialized: tenant, workspace, and owner account created", body = SetupResponse),
        (status = 404, description = "The setup wizard is disabled on this deployment (YORISHIRO_MAX_TENANTS resolves to unlimited)", body = crate::error::ApiErrorBody),
        (status = 409, description = "This deployment has already been set up", body = crate::error::ApiErrorBody),
        (status = 429, description = "Too many requests from this caller; retry later"),
    ),
    security(()),
    tag = "auth",
)]
pub async fn setup(
    State(state): State<AppState>,
    Json(body): Json<SetupRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if !wizard_enabled() {
        return Err(YorishiroError::NotFound {
            message: "the setup wizard is not enabled on this deployment".into(),
        }
        .into());
    }
    if tenant_count(&state.identity_pool).await? > 0 {
        return Err(YorishiroError::Conflict {
            message: "this deployment has already been set up".into(),
        }
        .into());
    }

    let tenant = tenancy::create_tenant(&state.identity_pool, "default", None).await?;
    let workspace =
        tenancy::create_workspace(&state.identity_pool, tenant.id, "default", None).await?;
    let user = tenancy::create_user(
        &state.identity_pool,
        &body.email,
        &body.password,
        body.display_name.as_deref(),
    )
    .await?;
    tenancy::add_member(
        &state.identity_pool,
        tenant.id,
        user.id,
        MembershipRole::Owner,
    )
    .await?;

    let mut conn = state.identity_pool.acquire().await.internal()?;
    let created = auth::create_api_key(
        &mut conn,
        workspace.id,
        MembershipRole::Owner.max_scope(),
        Some(user.id),
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(SetupResponse {
            user_id: user.id,
            email: user.email,
            tenant_id: tenant.id,
            workspace_id: workspace.id,
            api_key: created.plaintext,
        }),
    ))
}

#[cfg(test)]
mod tests;
