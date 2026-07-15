use rand::Rng;
use sea_query::{Alias, Expr, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sha2::{Digest, Sha256};
use sqlx::pool::PoolConnection;
use sqlx::{PgConnection, PgPool, Postgres};
use uuid::Uuid;

use crate::db::TenantDb;
use crate::error::YorishiroError;

#[derive(Iden)]
enum ApiKeys {
    Table,
    Id,
    WorkspaceId,
    KeyHash,
    KeyPrefix,
    Scope,
    UserId,
    LastUsedAt,
}

const KEY_PREFIX_BYTES: usize = 6;
const KEY_SECRET_BYTES: usize = 24;

/// Permission level held by an API key. Declaration order feeds the derived `Ord`, so any
/// code relying on the `Read < Write < Schema` hierarchy (a higher scope subsumes lower ones)
/// depends on this exact ordering. The `serde` representation matches the DB `scope` column
/// ('read'/'write'/'schema'), so REST/MCP adapters don't need a separate mapping.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyScope {
    Read,
    Write,
    Schema,
}

impl ApiKeyScope {
    fn as_db_str(self) -> &'static str {
        match self {
            ApiKeyScope::Read => "read",
            ApiKeyScope::Write => "write",
            ApiKeyScope::Schema => "schema",
        }
    }

    fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "read" => Some(ApiKeyScope::Read),
            "write" => Some(ApiKeyScope::Write),
            "schema" => Some(ApiKeyScope::Schema),
            _ => None,
        }
    }

    /// Whether a key with this scope can perform an operation requiring `required`.
    /// A higher scope subsumes lower ones (a `write` key is also allowed to `read`).
    pub fn satisfies(self, required: ApiKeyScope) -> bool {
        self >= required
    }
}

/// Workspace, tenant, and scope information resolved by API key authentication. Serves as
/// the starting point for both the subsequent RLS context setup
/// (`TenantDb::acquire_for_workspace`) and scope enforcement. An API key is always scoped to
/// exactly one workspace; `tenant_id` (the workspace's owning tenant) is carried alongside it
/// for tenant-level concerns such as billing checks.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub api_key_id: Uuid,
    pub workspace_id: Uuid,
    pub tenant_id: Uuid,
    pub scope: ApiKeyScope,
    /// The human user this key was issued for, if any. `None` for keys not attributed to a
    /// specific person (e.g. pure service/automation keys).
    pub user_id: Option<Uuid>,
}

pub struct CreatedApiKey {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub scope: ApiKeyScope,
    pub user_id: Option<Uuid>,
    /// The raw API key string. Only its hash is stored in the DB, so this return value is
    /// the only place it can ever be obtained. Callers must make sure to surface it to the user.
    pub plaintext: String,
}

fn random_hex(byte_len: usize) -> String {
    let mut bytes = vec![0u8; byte_len];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_key(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}

/// Issues a new API key of the form `ysr_<prefix>_<secret>`, where only the `secret` part
/// (192 bits) is the actual credential. SHA-256 is sufficient here rather than a slow KDF
/// like bcrypt/argon2, since API keys already carry enough entropy that offline
/// brute-forcing isn't a realistic threat.
pub async fn create_api_key(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    scope: ApiKeyScope,
    user_id: Option<Uuid>,
) -> Result<CreatedApiKey, YorishiroError> {
    let prefix = format!("ysr_{}", random_hex(KEY_PREFIX_BYTES));
    let secret = random_hex(KEY_SECRET_BYTES);
    let plaintext = format!("{prefix}_{secret}");
    let key_hash = hash_key(&plaintext);

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), ApiKeys::Table))
        .columns([
            ApiKeys::WorkspaceId,
            ApiKeys::KeyHash,
            ApiKeys::KeyPrefix,
            ApiKeys::Scope,
            ApiKeys::UserId,
        ])
        .values_panic([
            workspace_id.into(),
            key_hash.into(),
            prefix.into(),
            scope.as_db_str().into(),
            user_id.into(),
        ])
        .returning(Query::returning().columns([ApiKeys::Id]))
        .build_sqlx(PostgresQueryBuilder);

    let (id,): (Uuid,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    Ok(CreatedApiKey {
        id,
        workspace_id,
        scope,
        user_id,
        plaintext,
    })
}

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
    .map_err(|err| YorishiroError::Internal(err.into()))?;

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
        .map_err(|err| YorishiroError::Internal(err.into()))?;
    Ok(())
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
        .map_err(|err| YorishiroError::Internal(err.into()))?;

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

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use sqlx::postgres::PgPoolOptions;

    use super::*;
    use crate::db::TenantDb;
    use crate::test_support;

    #[derive(Iden)]
    enum Users {
        Table,
        Id,
        Email,
        PasswordHash,
    }

    /// Seeds a tenant plus one workspace under it, returning `(tenant_id, workspace_id)`.
    async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
        test_support::seed_tenant_and_workspace(pool).await
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn authenticates_a_freshly_created_key(pool: PgPool) {
        let (tenant_id, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db
            .acquire_for_workspace(tenant_id, workspace_id)
            .await
            .unwrap();

        let created = create_api_key(&mut conn, workspace_id, ApiKeyScope::Write, None)
            .await
            .unwrap();

        let ctx = authenticate(&pool, &created.plaintext).await.unwrap();

        assert_eq!(ctx.tenant_id, tenant_id);
        assert_eq!(ctx.workspace_id, workspace_id);
        assert_eq!(ctx.api_key_id, created.id);
        assert_eq!(ctx.scope, ApiKeyScope::Write);
        assert_eq!(ctx.user_id, None);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn resolves_the_attributed_user(pool: PgPool) {
        let (tenant_id, workspace_id) = seed_workspace(&pool).await;
        let (sql, values) = Query::insert()
            .into_table((Alias::new("identity"), Users::Table))
            .columns([Users::Email, Users::PasswordHash])
            .values_panic(["attributed@example.com".into(), "hash".into()])
            .returning(Query::returning().columns([Users::Id]))
            .build_sqlx(PostgresQueryBuilder);
        let (user_id,): (Uuid,) = sqlx::query_as_with(&sql, values)
            .fetch_one(&pool)
            .await
            .unwrap();

        let db = TenantDb::new(pool.clone());
        let mut conn = db
            .acquire_for_workspace(tenant_id, workspace_id)
            .await
            .unwrap();
        let created = create_api_key(&mut conn, workspace_id, ApiKeyScope::Write, Some(user_id))
            .await
            .unwrap();

        let ctx = authenticate(&pool, &created.plaintext).await.unwrap();
        assert_eq!(ctx.user_id, Some(user_id));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_an_unknown_key(pool: PgPool) {
        let err = authenticate(&pool, "ysr_does_not_exist_at_all")
            .await
            .unwrap_err();

        assert!(matches!(err, YorishiroError::Unauthenticated));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn resolves_the_correct_workspace_among_several(pool: PgPool) {
        let (tenant_a, workspace_a) = seed_workspace(&pool).await;
        let (tenant_b, workspace_b) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool.clone());

        let mut conn_a = db
            .acquire_for_workspace(tenant_a, workspace_a)
            .await
            .unwrap();
        let key_a = create_api_key(&mut conn_a, workspace_a, ApiKeyScope::Read, None)
            .await
            .unwrap();

        let mut conn_b = db
            .acquire_for_workspace(tenant_b, workspace_b)
            .await
            .unwrap();
        let key_b = create_api_key(&mut conn_b, workspace_b, ApiKeyScope::Read, None)
            .await
            .unwrap();

        let ctx_a = authenticate(&pool, &key_a.plaintext).await.unwrap();
        let ctx_b = authenticate(&pool, &key_b.plaintext).await.unwrap();

        assert_eq!(ctx_a.workspace_id, workspace_a);
        assert_eq!(ctx_b.workspace_id, workspace_b);
    }

    #[test]
    fn scope_hierarchy_allows_higher_scopes_to_satisfy_lower_requirements() {
        assert!(ApiKeyScope::Write.satisfies(ApiKeyScope::Read));
        assert!(ApiKeyScope::Schema.satisfies(ApiKeyScope::Write));
        assert!(!ApiKeyScope::Read.satisfies(ApiKeyScope::Write));
        assert!(!ApiKeyScope::Write.satisfies(ApiKeyScope::Schema));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn require_scope_rejects_insufficient_scope(pool: PgPool) {
        let (tenant_id, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db
            .acquire_for_workspace(tenant_id, workspace_id)
            .await
            .unwrap();

        let created = create_api_key(&mut conn, workspace_id, ApiKeyScope::Read, None)
            .await
            .unwrap();
        let ctx = authenticate(&pool, &created.plaintext).await.unwrap();

        let err = require_scope(&ctx, ApiKeyScope::Write).unwrap_err();
        assert!(matches!(err, YorishiroError::ScopeInsufficient { .. }));
    }

    /// Verifies that `authenticate_api_key` is actually needed: authentication must still
    /// succeed over a connection that went through the same `SET ROLE yorishiro_app` that
    /// `TenantDb::connect` uses in production (which can't bypass RLS and has no
    /// `app.current_tenant`/`app.current_workspace` set).
    #[sqlx::test(migrations = "../../migrations")]
    async fn authenticates_over_a_connection_that_cannot_bypass_rls(pool: PgPool) {
        let (tenant_id, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db
            .acquire_for_workspace(tenant_id, workspace_id)
            .await
            .unwrap();
        let created = create_api_key(&mut conn, workspace_id, ApiKeyScope::Read, None)
            .await
            .unwrap();

        let restricted_pool = PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    // Same session-control statement as `db.rs`'s `TenantDb::connect` --
                    // no query-builder form, stays raw SQL.
                    sqlx::query("SET ROLE yorishiro_app")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(pool.connect_options().as_ref().clone())
            .await
            .unwrap();

        let ctx = authenticate(&restricted_pool, &created.plaintext)
            .await
            .unwrap();

        assert_eq!(ctx.tenant_id, tenant_id);
        assert_eq!(ctx.workspace_id, workspace_id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn authorize_returns_a_usable_connection_for_a_sufficient_scope(pool: PgPool) {
        let (tenant_id, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db
            .acquire_for_workspace(tenant_id, workspace_id)
            .await
            .unwrap();
        let created = create_api_key(&mut conn, workspace_id, ApiKeyScope::Write, None)
            .await
            .unwrap();
        drop(conn);

        let (ctx, mut conn) = authorize(&db, &created.plaintext, ApiKeyScope::Read)
            .await
            .unwrap();

        assert_eq!(ctx.tenant_id, tenant_id);
        assert_eq!(ctx.workspace_id, workspace_id);
        // The returned connection already has its RLS context set, so it can read this
        // workspace's own api_keys row without issue.
        let (sql, values) = Query::select()
            .expr(sea_query::Func::count(Expr::col(sea_query::Asterisk)))
            .from((Alias::new("identity"), ApiKeys::Table))
            .build_sqlx(PostgresQueryBuilder);
        let count: (i64,) = sqlx::query_as_with(&sql, values)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(count.0, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn authorize_rejects_insufficient_scope_without_acquiring_a_connection(pool: PgPool) {
        let (tenant_id, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db
            .acquire_for_workspace(tenant_id, workspace_id)
            .await
            .unwrap();
        let created = create_api_key(&mut conn, workspace_id, ApiKeyScope::Read, None)
            .await
            .unwrap();
        drop(conn);

        let err = authorize(&db, &created.plaintext, ApiKeyScope::Write)
            .await
            .unwrap_err();

        assert!(matches!(err, YorishiroError::ScopeInsufficient { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn authorize_rejects_an_unknown_key(pool: PgPool) {
        let db = TenantDb::new(pool);

        let err = authorize(&db, "ysr_does_not_exist_at_all", ApiKeyScope::Read)
            .await
            .unwrap_err();

        assert!(matches!(err, YorishiroError::Unauthenticated));
    }
}
