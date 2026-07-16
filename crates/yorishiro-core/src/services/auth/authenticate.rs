use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

use super::{ApiKeyScope, ApiKeys, AuthContext, hash_key};

/// Verifies a presented raw API key and resolves the workspace, tenant, and scope it belongs
/// to.
///
/// At this point neither the workspace nor the tenant is known yet (so RLS's
/// `app.current_workspace`/`app.current_tenant` can't be set), which is why this calls the
/// SECURITY DEFINER function `identity.authenticate_api_key` over a connection acquired
/// directly from `pool`. That function bypasses RLS on the `api_keys`/`workspaces` tables for
/// verification purposes only, and limits the columns it returns to
/// id/workspace_id/tenant_id/scope (never the `key_hash` itself).
pub async fn authenticate(
    pool: &PgPool,
    presented_key: &str,
) -> Result<AuthContext, YorishiroError> {
    let key_hash = hash_key(presented_key);

    // Calling a SECURITY DEFINER function as the FROM-clause row source has no first-class
    // sea-query form (it isn't a table, so `.from()` can't target it without falling back to
    // `Expr::cust()` -- which would just hide a raw SQL string inside a builder call rather
    // than actually building the query). This stays raw SQL for the same reason the session
    // commands in `db.rs` do.
    let row: Option<(Uuid, Uuid, Uuid, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, workspace_id, tenant_id, scope, user_id FROM identity.authenticate_api_key($1)",
    )
    .bind(key_hash)
    .fetch_optional(pool)
    .await
    .internal()?;

    let (api_key_id, workspace_id, tenant_id, scope_str, user_id) =
        row.ok_or(YorishiroError::Unauthenticated)?;
    let scope = ApiKeyScope::from_db_str(&scope_str).ok_or_else(|| {
        YorishiroError::Internal(anyhow::anyhow!(
            "unknown api key scope in database: {scope_str}"
        ))
    })?;

    Ok(AuthContext {
        api_key_id,
        workspace_id,
        tenant_id,
        scope,
        user_id,
    })
}

/// Records the API key's last-used timestamp. This is a best-effort update that doesn't
/// affect authentication outcomes, so callers don't need to fail the whole request if it errors.
pub async fn touch_last_used(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    api_key_id: Uuid,
) -> Result<(), YorishiroError> {
    let (sql, values) = Query::update()
        .table((Alias::new("identity"), ApiKeys::Table))
        .values([(ApiKeys::LastUsedAt, Expr::current_timestamp().into())])
        .and_where(Expr::col(ApiKeys::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(ApiKeys::Id).eq(api_key_id))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_with(&sql, values)
        .execute(&mut *conn)
        .await
        .internal()?;
    Ok(())
}
