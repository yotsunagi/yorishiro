use axum::Json;
use serde::Serialize;
use uuid::Uuid;
use yorishiro_core::auth::ApiKeyScope;

use crate::auth::AuthContext;

#[derive(Serialize)]
pub struct WhoAmIResponse {
    tenant_id: Uuid,
    scope: ApiKeyScope,
}

pub async fn whoami(AuthContext(ctx): AuthContext) -> Json<WhoAmIResponse> {
    Json(WhoAmIResponse {
        tenant_id: ctx.tenant_id,
        scope: ctx.scope,
    })
}
