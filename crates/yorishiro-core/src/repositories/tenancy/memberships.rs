use sea_query::{Alias, Expr, Iden, OnConflict, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use uuid::Uuid;

use super::get_tenant;
use super::users::Users;
use crate::error::{ResultExt, YorishiroError};
use crate::models::tenancy::{MembershipRecord, MembershipRole};

#[derive(Iden)]
pub(super) enum TenantMemberships {
    Table,
    TenantId,
    UserId,
    Role,
    CreatedAt,
}

/// Adds (or updates the role of) a user's membership in a tenant.
pub async fn add_member(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<(), YorishiroError> {
    get_tenant(pool, tenant_id).await?;

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), TenantMemberships::Table))
        .columns([
            TenantMemberships::TenantId,
            TenantMemberships::UserId,
            TenantMemberships::Role,
        ])
        .values_panic([tenant_id.into(), user_id.into(), role.as_db_str().into()])
        .on_conflict(
            OnConflict::columns([TenantMemberships::TenantId, TenantMemberships::UserId])
                .update_column(TenantMemberships::Role)
                .to_owned(),
        )
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_with(&sql, values)
        .execute(pool)
        .await
        .internal()?;
    Ok(())
}

/// Looks up a single user's role within a tenant, or `None` if they aren't a member.
pub async fn get_membership_role(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<MembershipRole>, YorishiroError> {
    let (sql, values) = Query::select()
        .column(TenantMemberships::Role)
        .from((Alias::new("identity"), TenantMemberships::Table))
        .and_where(Expr::col(TenantMemberships::TenantId).eq(tenant_id))
        .and_where(Expr::col(TenantMemberships::UserId).eq(user_id))
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<(String,)> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?;

    row.map(|(role,)| {
        MembershipRole::from_db_str(&role).ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!(
                "unknown membership role in database: {role}"
            ))
        })
    })
    .transpose()
}

pub async fn list_members(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Vec<MembershipRecord>, YorishiroError> {
    #[derive(sqlx::FromRow)]
    struct MembershipRow {
        user_id: Uuid,
        email: String,
        display_name: Option<String>,
        role: String,
    }

    let (sql, values) = Query::select()
        .expr_as(Expr::col((Users::Table, Users::Id)), Alias::new("user_id"))
        .columns([
            (Users::Table, Users::Email),
            (Users::Table, Users::DisplayName),
        ])
        .column((TenantMemberships::Table, TenantMemberships::Role))
        .from((Alias::new("identity"), TenantMemberships::Table))
        .inner_join(
            (Alias::new("identity"), Users::Table),
            Expr::col((Users::Table, Users::Id))
                .equals((TenantMemberships::Table, TenantMemberships::UserId)),
        )
        .and_where(Expr::col((TenantMemberships::Table, TenantMemberships::TenantId)).eq(tenant_id))
        .order_by(
            (TenantMemberships::Table, TenantMemberships::CreatedAt),
            Order::Asc,
        )
        .build_sqlx(PostgresQueryBuilder);

    let rows: Vec<MembershipRow> = sqlx::query_as_with(&sql, values)
        .fetch_all(pool)
        .await
        .internal()?;

    rows.into_iter()
        .map(|row| {
            let role = MembershipRole::from_db_str(&row.role).ok_or_else(|| {
                YorishiroError::Internal(anyhow::anyhow!(
                    "unknown membership role in database: {}",
                    row.role
                ))
            })?;
            Ok(MembershipRecord {
                user_id: row.user_id,
                email: row.email,
                display_name: row.display_name,
                role,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::repositories::tenancy::{create_tenant, create_user};

    #[sqlx::test(migrations = "../../migrations")]
    async fn adds_and_lists_members(pool: PgPool) {
        let tenant = create_tenant(&pool, "team", None).await.unwrap();
        let user = create_user(&pool, "carol@example.com", "pw", Some("Carol"))
            .await
            .unwrap();

        add_member(&pool, tenant.id, user.id, MembershipRole::Admin)
            .await
            .unwrap();

        let members = list_members(&pool, tenant.id).await.unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].user_id, user.id);
        assert_eq!(members[0].role, MembershipRole::Admin);

        // Re-adding the same user updates the role instead of erroring.
        add_member(&pool, tenant.id, user.id, MembershipRole::Viewer)
            .await
            .unwrap();
        let members = list_members(&pool, tenant.id).await.unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].role, MembershipRole::Viewer);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn get_membership_role_resolves_and_defaults_to_none(pool: PgPool) {
        let tenant = create_tenant(&pool, "team", None).await.unwrap();
        let user = create_user(&pool, "erin@example.com", "pw", None)
            .await
            .unwrap();

        assert_eq!(
            get_membership_role(&pool, tenant.id, user.id)
                .await
                .unwrap(),
            None
        );

        add_member(&pool, tenant.id, user.id, MembershipRole::Member)
            .await
            .unwrap();
        assert_eq!(
            get_membership_role(&pool, tenant.id, user.id)
                .await
                .unwrap(),
            Some(MembershipRole::Member)
        );
    }

    #[test]
    fn max_scope_mirrors_role_privilege_order() {
        use crate::services::auth::ApiKeyScope;

        assert_eq!(MembershipRole::Owner.max_scope(), ApiKeyScope::Schema);
        assert_eq!(MembershipRole::Admin.max_scope(), ApiKeyScope::Schema);
        assert_eq!(MembershipRole::Member.max_scope(), ApiKeyScope::Write);
        assert_eq!(MembershipRole::Viewer.max_scope(), ApiKeyScope::Read);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn add_member_rejects_unknown_tenant(pool: PgPool) {
        let user = create_user(&pool, "dave@example.com", "pw", None)
            .await
            .unwrap();
        let err = add_member(&pool, Uuid::nil(), user.id, MembershipRole::Member)
            .await
            .unwrap_err();
        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }
}
