use axum::Json;
use serde::Serialize;
use uuid::Uuid;
use yorishiro_core::services::auth::ApiKeyScope;

use crate::http::middleware::auth::AuthContext;

#[derive(Serialize)]
pub struct WhoAmIResponse {
    workspace_id: Uuid,
    tenant_id: Uuid,
    scope: ApiKeyScope,
    /// The user this key was issued for, if it was created with `admin create-api-key --user`.
    user_id: Option<Uuid>,
}

pub async fn whoami(AuthContext(ctx): AuthContext) -> Json<WhoAmIResponse> {
    Json(WhoAmIResponse {
        workspace_id: ctx.workspace_id,
        tenant_id: ctx.tenant_id,
        scope: ctx.scope,
        user_id: ctx.user_id,
    })
}
