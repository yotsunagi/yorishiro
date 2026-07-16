use sea_query::{Alias, Asterisk, Expr, Func, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::tenancy::TenantRecord;

#[derive(Iden)]
enum Tenants {
    Table,
    Id,
    Name,
    Plan,
    MaxWorkspaces,
    StripeCustomerId,
    CreatedAt,
}

/// Creates a tenant, enforcing the system-wide tenant cap from `YORISHIRO_MAX_TENANTS` (`0` or
/// unset means unlimited). This is a deployment-wide limit rather than a per-tenant column, since
/// it bounds a deployment to a single tenant without needing a settings table: `yorishiro-server`
/// defaults this to `1` (single-tenant) and deployments that want multiple tenants set it to `0`
/// or a higher count. It is enforced only in application code (there is no anti-tampering against
/// an operator who edits the source or the env var directly) — like the rest of this module's
/// caps, it exists for product consistency, not as a security boundary against whoever controls
/// the deployment.
pub async fn create_tenant(
    pool: &PgPool,
    name: &str,
    max_workspaces: Option<i32>,
) -> Result<TenantRecord, YorishiroError> {
    create_tenant_with_cap(pool, name, max_workspaces, max_tenants_from_env()?).await
}

/// Reads and parses `YORISHIRO_MAX_TENANTS`. Unset or `0` means unlimited; a negative or
/// non-integer value is a misconfiguration and fails loudly rather than silently falling back to
/// unlimited.
pub fn max_tenants_from_env() -> Result<Option<i32>, YorishiroError> {
    match std::env::var("YORISHIRO_MAX_TENANTS") {
        Ok(raw) => {
            let parsed = raw.parse::<i32>().map_err(|_| {
                YorishiroError::Internal(anyhow::anyhow!(
                    "YORISHIRO_MAX_TENANTS must be an integer, got '{raw}'"
                ))
            })?;
            match parsed {
                0 => Ok(None),
                n if n < 0 => Err(YorishiroError::Internal(anyhow::anyhow!(
                    "YORISHIRO_MAX_TENANTS must not be negative, got '{raw}'"
                ))),
                n => Ok(Some(n)),
            }
        }
        Err(_) => Ok(None),
    }
}

/// Cap-checking logic factored out of `create_tenant` so tests can exercise it without mutating
/// the process-wide `YORISHIRO_MAX_TENANTS` env var (which would race against other tests running
/// concurrently in the same test binary).
async fn create_tenant_with_cap(
    pool: &PgPool,
    name: &str,
    max_workspaces: Option<i32>,
    max_tenants: Option<i32>,
) -> Result<TenantRecord, YorishiroError> {
    if let Some(max) = max_tenants {
        let (sql, values) = Query::select()
            .expr(Func::count(Expr::col(Asterisk)))
            .from((Alias::new("identity"), Tenants::Table))
            .build_sqlx(PostgresQueryBuilder);
        let (count,): (i64,) = sqlx::query_as_with(&sql, values)
            .fetch_one(pool)
            .await
            .internal()?;
        if count >= i64::from(max) {
            return Err(YorishiroError::Conflict {
                message: format!(
                    "this deployment has reached its tenant limit ({max}, set via \
                     YORISHIRO_MAX_TENANTS); raise or unset that variable to create another tenant"
                ),
            });
        }
    }

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Tenants::Table))
        .columns([Tenants::Name, Tenants::MaxWorkspaces])
        .values_panic([name.into(), max_workspaces.into()])
        .returning(Query::returning().columns([
            Tenants::Id,
            Tenants::Name,
            Tenants::Plan,
            Tenants::MaxWorkspaces,
            Tenants::StripeCustomerId,
            Tenants::CreatedAt,
        ]))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, TenantRecord, _>(&sql, values)
        .fetch_one(pool)
        .await
        .internal()
}

fn tenant_columns() -> [Tenants; 6] {
    [
        Tenants::Id,
        Tenants::Name,
        Tenants::Plan,
        Tenants::MaxWorkspaces,
        Tenants::StripeCustomerId,
        Tenants::CreatedAt,
    ]
}

pub async fn get_tenant(pool: &PgPool, tenant_id: Uuid) -> Result<TenantRecord, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(tenant_columns())
        .from((Alias::new("identity"), Tenants::Table))
        .and_where(Expr::col(Tenants::Id).eq(tenant_id))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, TenantRecord, _>(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!("tenant '{tenant_id}' was not found"),
        })
}

pub async fn list_tenants(pool: &PgPool) -> Result<Vec<TenantRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(tenant_columns())
        .from((Alias::new("identity"), Tenants::Table))
        .order_by(Tenants::CreatedAt, Order::Asc)
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, TenantRecord, _>(&sql, values)
        .fetch_all(pool)
        .await
        .internal()
}

/// Updates a tenant's billing plan and `max_workspaces` cap together, since the two always
/// change in lockstep when a subscription changes tier (see `yorishiro-hosted`'s plan-to-cap
/// mapping). Existing workspaces' own `max_entities` are left untouched -- only newly created
/// workspaces pick up a plan's default cap.
pub async fn set_tenant_plan(
    pool: &PgPool,
    tenant_id: Uuid,
    plan: &str,
    max_workspaces: Option<i32>,
) -> Result<TenantRecord, YorishiroError> {
    let (sql, values) = Query::update()
        .table((Alias::new("identity"), Tenants::Table))
        .values([
            (Tenants::Plan, plan.into()),
            (Tenants::MaxWorkspaces, max_workspaces.into()),
        ])
        .and_where(Expr::col(Tenants::Id).eq(tenant_id))
        .returning(Query::returning().columns(tenant_columns()))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, TenantRecord, _>(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!("tenant '{tenant_id}' was not found"),
        })
}

/// Records the Stripe customer id created for a tenant at checkout time, so later webhook
/// events (subscription updated/deleted) -- which only carry the Stripe customer id -- can be
/// routed back to this tenant via `get_tenant_by_stripe_customer`.
pub async fn link_stripe_customer(
    pool: &PgPool,
    tenant_id: Uuid,
    stripe_customer_id: &str,
) -> Result<TenantRecord, YorishiroError> {
    let (sql, values) = Query::update()
        .table((Alias::new("identity"), Tenants::Table))
        .values([(Tenants::StripeCustomerId, stripe_customer_id.into())])
        .and_where(Expr::col(Tenants::Id).eq(tenant_id))
        .returning(Query::returning().columns(tenant_columns()))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, TenantRecord, _>(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!("tenant '{tenant_id}' was not found"),
        })
}

pub async fn get_tenant_by_stripe_customer(
    pool: &PgPool,
    stripe_customer_id: &str,
) -> Result<Option<TenantRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(tenant_columns())
        .from((Alias::new("identity"), Tenants::Table))
        .and_where(Expr::col(Tenants::StripeCustomerId).eq(stripe_customer_id))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, TenantRecord, _>(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;

    #[sqlx::test(migrations = "../../migrations")]
    async fn enforces_system_wide_tenant_cap(pool: PgPool) {
        create_tenant_with_cap(&pool, "first", None, Some(1))
            .await
            .unwrap();

        let err = create_tenant_with_cap(&pool, "second", None, Some(1))
            .await
            .unwrap_err();
        assert!(matches!(err, YorishiroError::Conflict { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn unset_tenant_cap_is_unlimited(pool: PgPool) {
        create_tenant_with_cap(&pool, "first", None, None)
            .await
            .unwrap();
        create_tenant_with_cap(&pool, "second", None, None)
            .await
            .unwrap();
    }

    /// `YORISHIRO_MAX_TENANTS` is process-wide state, so these tests serialize through this lock
    /// rather than racing the env var against each other.
    static MAX_TENANTS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn max_tenants_from_env_unset_is_unlimited() {
        let _guard = MAX_TENANTS_ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("YORISHIRO_MAX_TENANTS") };
        assert_eq!(max_tenants_from_env().unwrap(), None);
    }

    #[test]
    fn max_tenants_from_env_zero_is_unlimited() {
        let _guard = MAX_TENANTS_ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("YORISHIRO_MAX_TENANTS", "0") };
        let result = max_tenants_from_env().unwrap();
        unsafe { std::env::remove_var("YORISHIRO_MAX_TENANTS") };
        assert_eq!(result, None);
    }

    #[test]
    fn max_tenants_from_env_positive_is_the_cap() {
        let _guard = MAX_TENANTS_ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("YORISHIRO_MAX_TENANTS", "3") };
        let result = max_tenants_from_env().unwrap();
        unsafe { std::env::remove_var("YORISHIRO_MAX_TENANTS") };
        assert_eq!(result, Some(3));
    }

    #[test]
    fn max_tenants_from_env_rejects_negative() {
        let _guard = MAX_TENANTS_ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("YORISHIRO_MAX_TENANTS", "-1") };
        let result = max_tenants_from_env();
        unsafe { std::env::remove_var("YORISHIRO_MAX_TENANTS") };
        assert!(result.is_err());
    }

    #[test]
    fn max_tenants_from_env_rejects_non_integer() {
        let _guard = MAX_TENANTS_ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("YORISHIRO_MAX_TENANTS", "abc") };
        let result = max_tenants_from_env();
        unsafe { std::env::remove_var("YORISHIRO_MAX_TENANTS") };
        assert!(result.is_err());
    }
}
