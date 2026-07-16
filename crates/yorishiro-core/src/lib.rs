pub mod db;
pub mod error;
pub mod metaschema;
pub mod models;
pub mod repositories;
pub mod services;
pub mod templates;

pub use error::{ResultExt, YorishiroError};

/// Shared test-only fixtures. `tenancy::create_tenant`/`create_workspace` themselves depend on
/// `PgPool` and enforce caps unrelated to what most other modules' tests need, so this crosses
/// that dependency out entirely: a minimal, direct sea-query insert against
/// `identity.tenants`/`identity.workspaces`, safe for any test module to call without pulling
/// in tenancy's cap-checking logic. Consolidates what used to be a near-identical raw-SQL
/// helper copy-pasted across a dozen test modules.
#[cfg(test)]
pub(crate) mod test_support {
    use sea_query::{Alias, Iden, PostgresQueryBuilder, Query};
    use sea_query_binder::SqlxBinder;
    use sqlx::PgPool;
    use uuid::Uuid;

    #[derive(Iden)]
    enum Tenants {
        Table,
        Id,
        Name,
    }

    #[derive(Iden)]
    enum Workspaces {
        Table,
        Id,
        TenantId,
        Name,
    }

    pub async fn seed_tenant(pool: &PgPool, name: &str) -> Uuid {
        let (sql, values) = Query::insert()
            .into_table((Alias::new("identity"), Tenants::Table))
            .columns([Tenants::Name])
            .values_panic([name.into()])
            .returning(Query::returning().columns([Tenants::Id]))
            .build_sqlx(PostgresQueryBuilder);
        let (id,): (Uuid,) = sqlx::query_as_with(&sql, values)
            .fetch_one(pool)
            .await
            .unwrap();
        id
    }

    pub async fn seed_workspace(pool: &PgPool, tenant_id: Uuid, name: &str) -> Uuid {
        let (sql, values) = Query::insert()
            .into_table((Alias::new("identity"), Workspaces::Table))
            .columns([Workspaces::TenantId, Workspaces::Name])
            .values_panic([tenant_id.into(), name.into()])
            .returning(Query::returning().columns([Workspaces::Id]))
            .build_sqlx(PostgresQueryBuilder);
        let (id,): (Uuid,) = sqlx::query_as_with(&sql, values)
            .fetch_one(pool)
            .await
            .unwrap();
        id
    }

    /// Seeds a tenant plus one workspace under it, returning `(tenant_id, workspace_id)` --
    /// the shape almost every test needs.
    pub async fn seed_tenant_and_workspace(pool: &PgPool) -> (Uuid, Uuid) {
        let tenant_id = seed_tenant(pool, "test-tenant").await;
        let workspace_id = seed_workspace(pool, tenant_id, "test-workspace").await;
        (tenant_id, workspace_id)
    }
}
