use sea_query::{Alias, Asterisk, Expr, Func, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use uuid::Uuid;

use super::get_tenant;
use super::memberships::TenantMemberships;
use crate::error::{ResultExt, YorishiroError};
use crate::models::tenancy::WorkspaceRecord;

#[derive(Iden)]
enum Workspaces {
    Table,
    Id,
    TenantId,
    Name,
    MaxEntities,
    CreatedAt,
}

/// Creates a workspace under `tenant_id`, enforcing the tenant's `max_workspaces` cap. `NULL`
/// means unlimited, which is the default so self-hosted deployments are never capped unless an
/// operator explicitly sets a limit.
pub async fn create_workspace(
    pool: &PgPool,
    tenant_id: Uuid,
    name: &str,
    max_entities: Option<i32>,
) -> Result<WorkspaceRecord, YorishiroError> {
    let tenant = get_tenant(pool, tenant_id).await?;

    if let Some(max) = tenant.max_workspaces {
        let (sql, values) = Query::select()
            .expr(Func::count(Expr::col(Asterisk)))
            .from((Alias::new("identity"), Workspaces::Table))
            .and_where(Expr::col(Workspaces::TenantId).eq(tenant_id))
            .build_sqlx(PostgresQueryBuilder);
        let (count,): (i64,) = sqlx::query_as_with(&sql, values)
            .fetch_one(pool)
            .await
            .internal()?;
        if count >= i64::from(max) {
            return Err(YorishiroError::Conflict {
                message: format!(
                    "tenant '{tenant_id}' has reached its workspace limit ({max}); \
                     raise max_workspaces or delete an existing workspace"
                ),
            });
        }
    }

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Workspaces::Table))
        .columns([
            Workspaces::TenantId,
            Workspaces::Name,
            Workspaces::MaxEntities,
        ])
        .values_panic([tenant_id.into(), name.into(), max_entities.into()])
        .returning(Query::returning().columns(workspace_columns()))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, WorkspaceRecord, _>(&sql, values)
        .fetch_one(pool)
        .await
        .internal()
}

fn workspace_columns() -> [Workspaces; 5] {
    [
        Workspaces::Id,
        Workspaces::TenantId,
        Workspaces::Name,
        Workspaces::MaxEntities,
        Workspaces::CreatedAt,
    ]
}

pub async fn list_workspaces(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Vec<WorkspaceRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(workspace_columns())
        .from((Alias::new("identity"), Workspaces::Table))
        .and_where(Expr::col(Workspaces::TenantId).eq(tenant_id))
        .order_by(Workspaces::CreatedAt, Order::Asc)
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, WorkspaceRecord, _>(&sql, values)
        .fetch_all(pool)
        .await
        .internal()
}

/// Every workspace `user_id` can log into, across all of their tenant memberships -- used to
/// resolve `POST /auth/login`'s `workspace_id` automatically when the caller omits it and the
/// answer is unambiguous (see `rest::identity::login`).
pub async fn list_workspaces_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<WorkspaceRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns([
            (Workspaces::Table, Workspaces::Id),
            (Workspaces::Table, Workspaces::TenantId),
            (Workspaces::Table, Workspaces::Name),
            (Workspaces::Table, Workspaces::MaxEntities),
            (Workspaces::Table, Workspaces::CreatedAt),
        ])
        .from((Alias::new("identity"), Workspaces::Table))
        .inner_join(
            (Alias::new("identity"), TenantMemberships::Table),
            Expr::col((TenantMemberships::Table, TenantMemberships::TenantId))
                .equals((Workspaces::Table, Workspaces::TenantId)),
        )
        .and_where(Expr::col((TenantMemberships::Table, TenantMemberships::UserId)).eq(user_id))
        .order_by((Workspaces::Table, Workspaces::CreatedAt), Order::Asc)
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, WorkspaceRecord, _>(&sql, values)
        .fetch_all(pool)
        .await
        .internal()
}

pub async fn get_workspace(
    pool: &PgPool,
    workspace_id: Uuid,
) -> Result<WorkspaceRecord, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(workspace_columns())
        .from((Alias::new("identity"), Workspaces::Table))
        .and_where(Expr::col(Workspaces::Id).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, WorkspaceRecord, _>(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!("workspace '{workspace_id}' was not found"),
        })
}

/// Deletes a workspace and everything under it. `identity.workspaces`'s foreign keys from
/// `content.entities`/`content.relations`/`content.schemas`/`identity.api_keys` are all
/// `ON DELETE CASCADE` (see the initial migration), so this one statement is enough --
/// callers don't need to delete those rows themselves first.
pub async fn delete_workspace(pool: &PgPool, workspace_id: Uuid) -> Result<(), YorishiroError> {
    let (sql, values) = Query::delete()
        .from_table((Alias::new("identity"), Workspaces::Table))
        .and_where(Expr::col(Workspaces::Id).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);

    let result = sqlx::query_with(&sql, values)
        .execute(pool)
        .await
        .internal()?;

    if result.rows_affected() == 0 {
        Err(YorishiroError::NotFound {
            message: format!("workspace '{workspace_id}' was not found"),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::repositories::tenancy::create_tenant;

    #[sqlx::test(migrations = "../../migrations")]
    async fn creates_tenant_and_workspace(pool: PgPool) {
        let tenant = create_tenant(&pool, "acme", None).await.unwrap();
        let workspace = create_workspace(&pool, tenant.id, "default", None)
            .await
            .unwrap();
        assert_eq!(workspace.tenant_id, tenant.id);

        let workspaces = list_workspaces(&pool, tenant.id).await.unwrap();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].id, workspace.id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn enforces_max_workspaces(pool: PgPool) {
        let tenant = create_tenant(&pool, "capped", Some(1)).await.unwrap();
        create_workspace(&pool, tenant.id, "first", None)
            .await
            .unwrap();

        let err = create_workspace(&pool, tenant.id, "second", None)
            .await
            .unwrap_err();
        assert!(matches!(err, YorishiroError::Conflict { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_workspace_rejects_unknown_tenant(pool: PgPool) {
        let err = create_workspace(&pool, Uuid::nil(), "orphan", None)
            .await
            .unwrap_err();
        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }
}
