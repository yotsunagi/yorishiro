use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::auth::{self, ApiKeyScope};
use yorishiro_core::tenancy::{self, MembershipRole};

use crate::error::ApiError;
use crate::state::AppState;

/// These two endpoints are the only ones in the whole API reachable without a bearer token --
/// by design, since their entire purpose is to hand one out. They read/write
/// `identity.users`/`identity.tenant_memberships`/`identity.invites` through `state.identity_pool`
/// (the admin/migration-role pool) rather than `state.tenant_db`, exactly like the admin CLI,
/// since no tenant/workspace context exists yet for RLS to scope by.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SignupRequest {
    /// The plaintext token from an `admin create-invite` (or hosted dashboard) invitation.
    pub invite_token: String,
    pub password: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SignupResponse {
    pub user_id: Uuid,
    pub email: String,
    pub tenant_id: Uuid,
    pub role: MembershipRole,
    /// The workspaces the new member can now log into. The client picks one and passes its id
    /// to `/auth/login`.
    pub workspaces: Vec<WorkspaceSummary>,
}

#[utoipa::path(
    post,
    path = "/auth/signup",
    request_body = SignupRequest,
    responses(
        (status = 201, description = "Account created from a valid invite", body = SignupResponse),
        (status = 409, description = "A user with this email already exists", body = crate::error::ApiErrorBody),
        (status = 422, description = "Invite token is invalid, expired, or already used", body = crate::error::ApiErrorBody),
        (status = 429, description = "Too many requests from this caller; retry later"),
    ),
    security(()),
    tag = "auth",
)]
pub async fn signup(
    State(state): State<AppState>,
    Json(body): Json<SignupRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let invite = tenancy::redeem_invite(&state.identity_pool, &body.invite_token)
        .await?
        .ok_or_else(|| YorishiroError::ValidationFailed {
            message: "invite token is invalid, expired, or already used".into(),
            details: vec![],
            hint: "ask a tenant admin for a fresh invite".into(),
        })?;

    let user = tenancy::create_user(
        &state.identity_pool,
        &invite.email,
        &body.password,
        body.display_name.as_deref(),
    )
    .await?;

    tenancy::add_member(&state.identity_pool, invite.tenant_id, user.id, invite.role).await?;

    let workspaces = tenancy::list_workspaces(&state.identity_pool, invite.tenant_id)
        .await?
        .into_iter()
        .map(|workspace| WorkspaceSummary {
            id: workspace.id,
            name: workspace.name,
        })
        .collect();

    Ok((
        StatusCode::CREATED,
        Json(SignupResponse {
            user_id: user.id,
            email: user.email,
            tenant_id: invite.tenant_id,
            role: invite.role,
            workspaces,
        }),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    /// Which of the account's workspaces to issue an API key for -- a key is always scoped to
    /// exactly one workspace, same as one created through `admin create-api-key`. Omit this
    /// when the account can only ever log into one workspace (true for every community-edition
    /// deployment, since `YORISHIRO_MAX_TENANTS` defaults to a single tenant with one
    /// workspace) -- it resolves automatically. An account with access to more than one
    /// workspace must specify which one explicitly (422 otherwise).
    pub workspace_id: Option<Uuid>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoginResponse {
    /// The freshly issued API key's plaintext. Shown only in this response -- only its hash is
    /// ever persisted, so it cannot be recovered afterward.
    pub api_key: String,
    pub api_key_id: Uuid,
    pub workspace_id: Uuid,
    pub scope: ApiKeyScope,
    pub user_id: Uuid,
}

#[utoipa::path(
    post,
    path = "/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "A freshly issued API key, scoped to the caller's membership role", body = LoginResponse),
        (status = 401, description = "Invalid email or password", body = crate::error::ApiErrorBody),
        (status = 403, description = "Not a member of this workspace's tenant", body = crate::error::ApiErrorBody),
        (status = 404, description = "Workspace not found", body = crate::error::ApiErrorBody),
        (status = 422, description = "workspace_id omitted, and the account has zero or multiple workspaces to choose from", body = crate::error::ApiErrorBody),
        (status = 429, description = "Too many requests from this caller; retry later"),
    ),
    security(()),
    tag = "auth",
)]
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    // Credentials are checked before the workspace is looked up, so a request with a bad
    // password never reveals whether `workspace_id` exists.
    let user = tenancy::verify_login(&state.identity_pool, &body.email, &body.password)
        .await?
        .ok_or(YorishiroError::Unauthenticated)?;

    let workspace = match body.workspace_id {
        Some(workspace_id) => tenancy::get_workspace(&state.identity_pool, workspace_id).await?,
        // Every community-edition deployment has exactly one workspace by default (see
        // YORISHIRO_MAX_TENANTS), so resolving it automatically here means the login form
        // never needs to ask for a workspace id in the common case.
        None => {
            let mut workspaces =
                tenancy::list_workspaces_for_user(&state.identity_pool, user.id).await?;
            match workspaces.len() {
                1 => workspaces.pop().expect("len() == 1 checked above"),
                0 => {
                    return Err(YorishiroError::ScopeInsufficient {
                        message: "this account is not a member of any tenant".into(),
                        hint: "ask a tenant admin to add you as a member first".into(),
                    }
                    .into());
                }
                _ => {
                    return Err(YorishiroError::ValidationFailed {
                        message: "this account has access to more than one workspace".into(),
                        details: vec![],
                        hint: "specify workspace_id explicitly".into(),
                    }
                    .into());
                }
            }
        }
    };

    let role = tenancy::get_membership_role(&state.identity_pool, workspace.tenant_id, user.id)
        .await?
        .ok_or_else(|| YorishiroError::ScopeInsufficient {
            message: "this account is not a member of the tenant that owns this workspace".into(),
            hint: "ask a tenant admin to add you as a member first".into(),
        })?;

    let mut conn = state
        .identity_pool
        .acquire()
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;
    let created =
        auth::create_api_key(&mut conn, workspace.id, role.max_scope(), Some(user.id)).await?;

    Ok(Json(LoginResponse {
        api_key: created.plaintext,
        api_key_id: created.id,
        workspace_id: created.workspace_id,
        scope: created.scope,
        user_id: user.id,
    }))
}
