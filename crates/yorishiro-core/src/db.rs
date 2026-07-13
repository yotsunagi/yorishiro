use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[derive(Clone)]
pub struct TenantDb {
    pool: PgPool,
}

impl TenantDb {
    /// Wraps a raw pool as-is. Callers must separately guarantee that `app.current_tenant`/
    /// `app.current_workspace` get reset when a connection returns to the pool (use
    /// `connect` for production). This also doesn't switch roles, so tenant isolation won't
    /// hold if `pool`'s connection role can bypass RLS — intended for direct use in
    /// migrations and tests.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Builds the production pool. The `after_connect` hook issues `SET ROLE` once per
    /// physical connection so all subsequent queries run as the `yorishiro_app` role, which
    /// cannot bypass RLS (the login role behind `database_url` can remain a superuser, since
    /// a superuser can `SET ROLE` to any role without needing membership). The
    /// `after_release` hook resets `app.current_tenant`/`app.current_workspace` before
    /// returning a connection to the pool, preventing one workspace's session state from
    /// leaking to whichever workspace borrows the connection next.
    pub async fn connect(database_url: &str, max_connections: u32) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("SET ROLE yorishiro_app")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .after_release(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("RESET app.current_tenant")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("RESET app.current_workspace")
                        .execute(&mut *conn)
                        .await?;
                    Ok(true)
                })
            })
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Sets the session variables `app.current_tenant` and `app.current_workspace` on this
    /// connection so RLS can isolate both the tenant-level control-plane rows and the
    /// workspace-scoped content rows.
    ///
    /// Using `is_local=false` (session-level) matters: `is_local=true` (equivalent to `SET
    /// LOCAL`) would be discarded as soon as the implicit single-statement transaction ends
    /// when called outside an explicit transaction, causing later queries to see
    /// `current_setting(...)` as an empty string — i.e. isolation breaks.
    pub async fn acquire_for_workspace(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("SELECT set_config('app.current_tenant', $1, false)")
            .bind(tenant_id.to_string())
            .execute(conn.as_mut())
            .await?;
        sqlx::query("SELECT set_config('app.current_workspace', $1, false)")
            .bind(workspace_id.to_string())
            .execute(conn.as_mut())
            .await?;
        Ok(conn)
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use sqlx::Row;

    use super::*;

    /// The pool `sqlx::test` provides is connected as the admin role (superuser) that ran
    /// the migrations, so `TenantDb::new` alone won't make RLS take effect. This test
    /// explicitly switches to `yorishiro_app` via `SET ROLE` and verifies that RLS actually
    /// blocks cross-tenant access — confirming the effect of the switch `TenantDb::connect`
    /// performs in production.
    /// `identity.tenants` itself has no grant for `yorishiro_app` (see the role-separation
    /// migration), so this exercises RLS through `identity.workspaces` instead, which the
    /// app role has a read-only grant on and which is scoped by the same
    /// `app.current_tenant` policy.
    #[sqlx::test(migrations = "../../migrations")]
    async fn rls_blocks_cross_tenant_access_under_restricted_role(pool: PgPool) {
        let (tenant_a,): (Uuid,) =
            sqlx::query_as("INSERT INTO identity.tenants (name) VALUES ($1) RETURNING id")
                .bind("tenant-a")
                .fetch_one(&pool)
                .await
                .unwrap();
        let (tenant_b,): (Uuid,) =
            sqlx::query_as("INSERT INTO identity.tenants (name) VALUES ($1) RETURNING id")
                .bind("tenant-b")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("INSERT INTO identity.workspaces (tenant_id, name) VALUES ($1, $2)")
            .bind(tenant_a)
            .bind("workspace-a")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO identity.workspaces (tenant_id, name) VALUES ($1, $2)")
            .bind(tenant_b)
            .bind("workspace-b")
            .execute(&pool)
            .await
            .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("SET ROLE yorishiro_app")
            .execute(conn.as_mut())
            .await
            .unwrap();
        sqlx::query("SELECT set_config('app.current_tenant', $1, false)")
            .bind(tenant_a.to_string())
            .execute(conn.as_mut())
            .await
            .unwrap();

        let rows = sqlx::query("SELECT name FROM identity.workspaces")
            .fetch_all(conn.as_mut())
            .await
            .unwrap();
        let names: Vec<String> = rows.iter().map(|row| row.get("name")).collect();

        assert_eq!(names, vec!["workspace-a".to_string()]);
    }
}
