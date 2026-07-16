use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use super::*;
use crate::db::TenantDb;
use crate::error::YorishiroError;
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
