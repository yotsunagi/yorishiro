use sqlx::Postgres;
use sqlx::pool::PoolConnection;

use crate::db::TenantDb;
use crate::error::{ResultExt, YorishiroError};

use super::authenticate::authenticate;
use super::{ApiKeyScope, AuthContext, touch_last_used};

/// Enforces that an authenticated context satisfies the required scope, returning
/// `YorishiroError::ScopeInsufficient` when it doesn't.
pub fn require_scope(ctx: &AuthContext, required: ApiKeyScope) -> Result<(), YorishiroError> {
    if ctx.scope.satisfies(required) {
        Ok(())
    } else {
        Err(YorishiroError::ScopeInsufficient {
            message: format!(
                "this operation requires {required:?} scope but the API key has {:?} scope",
                ctx.scope
            ),
            hint: "Reissue an API key with sufficient scope".into(),
        })
    }
}

/// The single entry point for authorization: validates the presented raw key, confirms it
/// satisfies the required scope, and returns a connection with the RLS context already set.
/// REST and MCP adapters have no way to obtain a `&mut PgConnection` except through this
/// function, which structurally prevents a scope check from being forgotten.
pub async fn authorize(
    tenant_db: &TenantDb,
    presented_key: &str,
    required: ApiKeyScope,
) -> Result<(AuthContext, PoolConnection<Postgres>), YorishiroError> {
    let ctx = authenticate(tenant_db.pool(), presented_key).await?;
    require_scope(&ctx, required)?;

    let mut conn = tenant_db
        .acquire_for_workspace(ctx.tenant_id, ctx.workspace_id)
        .await
        .internal()?;

    if let Err(err) = touch_last_used(&mut conn, ctx.workspace_id, ctx.api_key_id).await {
        tracing::warn!(error = %err, "failed to update api key last_used_at");
    }

    Ok((ctx, conn))
}

/// A connection-free variant of `authorize`, used on paths (search queries) that need to run
/// a slow step — like embedding generation — before touching the DB. `authorize` holds a
/// connection for the handler's entire lifetime, which would tie up a pool connection during
/// embedding generation (unbounded wait time with LocalOnnx due to in-process serialization),
/// letting pool exhaustion spill over onto unrelated endpoints. This function only performs
/// authentication and scope validation, updating `last_used_at` through a short-lived
/// connection that's returned immediately.
pub async fn authorize_scope(
    tenant_db: &TenantDb,
    presented_key: &str,
    required: ApiKeyScope,
) -> Result<AuthContext, YorishiroError> {
    let ctx = authenticate(tenant_db.pool(), presented_key).await?;
    require_scope(&ctx, required)?;

    match tenant_db
        .acquire_for_workspace(ctx.tenant_id, ctx.workspace_id)
        .await
    {
        Ok(mut conn) => {
            if let Err(err) = touch_last_used(&mut conn, ctx.workspace_id, ctx.api_key_id).await {
                tracing::warn!(error = %err, "failed to update api key last_used_at");
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to acquire connection to touch last_used_at");
        }
    }

    Ok(ctx)
}
