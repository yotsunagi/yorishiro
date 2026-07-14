use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::tenancy::{self, MembershipRecord, MembershipRole};

use crate::auth::AuthContext;
use crate::error::ApiError;
use crate::state::AppState;

/// Membership management is a tenant-wide concern, independent of (and stricter than) the
/// presented API key's own scope -- a Member-role key can carry `write` scope for content
/// operations while still having no business adding or listing members. This mirrors
/// `yorishiro-hosted`'s `authenticate_tenant_admin`.
async fn require_tenant_admin(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<(), YorishiroError> {
    let user_id = user_id.ok_or(YorishiroError::Unauthenticated)?;
    tenancy::get_membership_role(&state.identity_pool, tenant_id, user_id)
        .await?
        .filter(|role| matches!(role, MembershipRole::Owner | MembershipRole::Admin))
        .ok_or_else(|| YorishiroError::ScopeInsufficient {
            message: "member management is restricted to tenant owners/admins".into(),
            hint: "ask a tenant owner to grant you the admin role".into(),
        })?;
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/members",
    responses(
        (status = 200, description = "Members of the caller's tenant", body = Vec<MembershipRecord>),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Not a tenant owner/admin", body = crate::error::ApiErrorBody),
    ),
    tag = "members",
)]
pub async fn list_members(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
) -> Result<Json<Vec<MembershipRecord>>, ApiError> {
    require_tenant_admin(&state, ctx.tenant_id, ctx.user_id).await?;
    let members = tenancy::list_members(&state.identity_pool, ctx.tenant_id).await?;
    Ok(Json(members))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AddMemberRequest {
    /// Must already have an account (created via `/auth/signup`) -- this endpoint attaches an
    /// *existing* user to the caller's tenant, it never creates one. To bring in someone with
    /// no account yet, issue them an invite instead.
    pub email: String,
    pub role: MembershipRole,
}

#[utoipa::path(
    post,
    path = "/api/members",
    request_body = AddMemberRequest,
    responses(
        (status = 201, description = "Membership added (or role changed if already a member)", body = MembershipRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Not a tenant owner/admin", body = crate::error::ApiErrorBody),
        (status = 404, description = "No user with this email has an account", body = crate::error::ApiErrorBody),
    ),
    tag = "members",
)]
pub async fn add_member(
    State(state): State<AppState>,
    AuthContext(ctx): AuthContext,
    Json(body): Json<AddMemberRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_tenant_admin(&state, ctx.tenant_id, ctx.user_id).await?;

    let user = tenancy::get_user_by_email(&state.identity_pool, &body.email)
        .await?
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!("no user with email '{}' has an account", body.email),
        })?;

    tenancy::add_member(&state.identity_pool, ctx.tenant_id, user.id, body.role).await?;

    Ok((
        StatusCode::CREATED,
        Json(MembershipRecord {
            user_id: user.id,
            email: user.email,
            display_name: user.display_name,
            role: body.role,
        }),
    ))
}
