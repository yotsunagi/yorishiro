use anyhow::{Context, Result, bail};
use sea_query::{Alias, Expr, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::repositories::tenancy;
use yorishiro_core::services::auth::{self, ApiKeyScope, CreatedApiKey};

#[derive(Iden)]
enum Workspaces {
    Table,
    Id,
    TenantId,
}

#[derive(Iden)]
enum ApiKeys {
    Table,
    Id,
    WorkspaceId,
    Scope,
    KeyPrefix,
    UserId,
    CreatedAt,
    LastUsedAt,
}

#[derive(Iden)]
enum Entities {
    Table,
    Id,
    WorkspaceId,
    Embedding,
}

pub(super) async fn create_api_key(
    pool: &PgPool,
    workspace_id: Uuid,
    scope: ApiKeyScope,
    user_id: Option<Uuid>,
) -> Result<CreatedApiKey> {
    // Check the workspace exists up front so the error is clearer than a raw FK violation.
    let (sql, values) = Query::select()
        .column(Workspaces::TenantId)
        .from((Alias::new("identity"), Workspaces::Table))
        .and_where(Expr::col(Workspaces::Id).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);
    let tenant_id: Option<(Uuid,)> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await?;
    let Some((tenant_id,)) = tenant_id else {
        bail!(
            "workspace '{workspace_id}' does not exist (see `admin list-workspaces <tenant-id>`)"
        );
    };

    if let Some(user_id) = user_id {
        let role = tenancy::get_membership_role(pool, tenant_id, user_id).await?;
        let Some(role) = role else {
            bail!(
                "user '{user_id}' is not a member of tenant '{tenant_id}' \
                 (see `admin add-member`)"
            );
        };
        let max_scope = role.max_scope();
        if scope > max_scope {
            bail!(
                "user '{user_id}' has role {role:?} in this tenant, which permits at most \
                 {max_scope:?} scope keys (requested {scope:?})"
            );
        }
    }

    let mut conn = pool.acquire().await?;
    let created = auth::create_api_key(&mut conn, workspace_id, scope, user_id)
        .await
        .context("failed to create api key")?;
    Ok(created)
}

#[derive(sqlx::FromRow)]
pub(super) struct ApiKeySummary {
    pub(super) id: Uuid,
    pub(super) scope: String,
    pub(super) key_prefix: String,
    pub(super) user_id: Option<Uuid>,
    pub(super) created_at: chrono::DateTime<chrono::Utc>,
    pub(super) last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub(super) async fn list_api_keys(pool: &PgPool, workspace_id: Uuid) -> Result<Vec<ApiKeySummary>> {
    let (sql, values) = Query::select()
        .columns([
            ApiKeys::Id,
            ApiKeys::Scope,
            ApiKeys::KeyPrefix,
            ApiKeys::UserId,
            ApiKeys::CreatedAt,
            ApiKeys::LastUsedAt,
        ])
        .from((Alias::new("identity"), ApiKeys::Table))
        .and_where(Expr::col(ApiKeys::WorkspaceId).eq(workspace_id))
        .order_by(ApiKeys::CreatedAt, Order::Asc)
        .build_sqlx(PostgresQueryBuilder);
    let rows: Vec<ApiKeySummary> = sqlx::query_as_with(&sql, values).fetch_all(pool).await?;
    Ok(rows)
}

/// Authentication looks up the key in the database on every request, so deleting the row
/// revokes it immediately.
pub(super) async fn revoke_api_key(pool: &PgPool, key_id: Uuid) -> Result<()> {
    let (sql, values) = Query::delete()
        .from_table((Alias::new("identity"), ApiKeys::Table))
        .and_where(Expr::col(ApiKeys::Id).eq(key_id))
        .build_sqlx(PostgresQueryBuilder);
    let result = sqlx::query_with(&sql, values).execute(pool).await?;
    if result.rows_affected() == 0 {
        bail!("api key '{key_id}' does not exist (see `admin list-api-keys <workspace-id>`)");
    }
    Ok(())
}

pub(super) struct ResyncReport {
    pub(super) candidates: usize,
    pub(super) synced: usize,
    pub(super) failed: usize,
}

/// Re-syncs embeddings for entities whose `embedding` column is still NULL. An operational
/// recovery command for entities that fell out of search due to a failed background sync
/// (e.g. a transient embedding API outage or a process killed mid-deploy).
pub(super) async fn resync_embeddings(
    pool: &PgPool,
    workspace_id: Uuid,
    provider: &dyn yorishiro_core::services::embedding::EmbeddingProvider,
) -> Result<ResyncReport> {
    let (sql, values) = Query::select()
        .column(Entities::Id)
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Entities::Embedding).is_null())
        .build_sqlx(PostgresQueryBuilder);
    let ids: Vec<(Uuid,)> = sqlx::query_as_with(&sql, values).fetch_all(pool).await?;

    let mut report = ResyncReport {
        candidates: ids.len(),
        synced: 0,
        failed: 0,
    };
    let mut conn = pool.acquire().await?;
    for (entity_id,) in ids {
        let result = async {
            let record =
                yorishiro_core::repositories::entities::get(&mut conn, workspace_id, entity_id)
                    .await?;
            yorishiro_core::services::embedding::sync::sync_embedding_for_record(
                &mut conn,
                workspace_id,
                &record,
                provider,
            )
            .await
        }
        .await;

        match result {
            Ok(()) => report.synced += 1,
            Err(err) => {
                report.failed += 1;
                eprintln!("  failed to resync entity {entity_id}: {err}");
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use yorishiro_core::repositories::tenancy::MembershipRole;

    async fn seed_workspace(pool: &PgPool) -> Uuid {
        let tenant = tenancy::create_tenant(pool, "bootstrap-tenant", None)
            .await
            .unwrap();
        let workspace = tenancy::create_workspace(pool, tenant.id, "default", None)
            .await
            .unwrap();
        workspace.id
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn creates_workspace_and_issues_a_usable_key(pool: PgPool) {
        let workspace_id = seed_workspace(&pool).await;

        let created = create_api_key(&pool, workspace_id, ApiKeyScope::Write, None)
            .await
            .unwrap();
        assert_eq!(created.workspace_id, workspace_id);
        assert!(created.plaintext.starts_with("ysr_"));
        assert_eq!(created.user_id, None);

        // Confirm the issued key actually authenticates, not just that creation returned Ok.
        let ctx = auth::authenticate(&pool, &created.plaintext).await.unwrap();
        assert_eq!(ctx.workspace_id, workspace_id);
        assert_eq!(ctx.scope, ApiKeyScope::Write);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_key_creation_for_unknown_workspace(pool: PgPool) {
        let result = create_api_key(&pool, Uuid::nil(), ApiKeyScope::Read, None).await;
        let Err(err) = result else {
            panic!("key creation should fail for an unknown workspace");
        };
        assert!(err.to_string().contains("does not exist"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_api_key_for_user_is_capped_by_their_role(pool: PgPool) {
        let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
        let workspace = tenancy::create_workspace(&pool, tenant.id, "default", None)
            .await
            .unwrap();
        let user = tenancy::create_user(&pool, "viewer@example.com", "pw", None)
            .await
            .unwrap();
        tenancy::add_member(&pool, tenant.id, user.id, MembershipRole::Viewer)
            .await
            .unwrap();

        // A viewer may be issued a read-scope key...
        let created = create_api_key(&pool, workspace.id, ApiKeyScope::Read, Some(user.id))
            .await
            .unwrap();
        assert_eq!(created.user_id, Some(user.id));

        // ...but not a write- or schema-scope one.
        let result = create_api_key(&pool, workspace.id, ApiKeyScope::Write, Some(user.id)).await;
        let Err(err) = result else {
            panic!("a viewer should not be issuable a write-scope key");
        };
        assert!(err.to_string().contains("Viewer"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_api_key_rejects_a_user_who_is_not_a_member(pool: PgPool) {
        let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
        let workspace = tenancy::create_workspace(&pool, tenant.id, "default", None)
            .await
            .unwrap();
        let user = tenancy::create_user(&pool, "outsider@example.com", "pw", None)
            .await
            .unwrap();

        let result = create_api_key(&pool, workspace.id, ApiKeyScope::Read, Some(user.id)).await;
        let Err(err) = result else {
            panic!("a non-member should not be issuable an api key");
        };
        assert!(err.to_string().contains("not a member"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn revoked_key_no_longer_authenticates(pool: PgPool) {
        let workspace_id = seed_workspace(&pool).await;
        let created = create_api_key(&pool, workspace_id, ApiKeyScope::Read, None)
            .await
            .unwrap();
        auth::authenticate(&pool, &created.plaintext).await.unwrap();

        let listed = list_api_keys(&pool, workspace_id).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);

        revoke_api_key(&pool, created.id).await.unwrap();

        let result = auth::authenticate(&pool, &created.plaintext).await;
        assert!(matches!(
            result,
            Err(yorishiro_core::YorishiroError::Unauthenticated)
        ));
        assert!(list_api_keys(&pool, workspace_id).await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn resync_fills_missing_embeddings(pool: PgPool) {
        use async_trait::async_trait;
        use yorishiro_core::YorishiroError;
        use yorishiro_core::services::embedding::EmbeddingProvider;

        struct FixedProvider;

        #[async_trait]
        impl EmbeddingProvider for FixedProvider {
            fn dimensions(&self) -> usize {
                768
            }

            async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
                Ok(texts.iter().map(|_| vec![0.2_f32; 768]).collect())
            }
        }

        let workspace_id = seed_workspace(&pool).await;
        let mut conn = pool.acquire().await.unwrap();
        let definition = serde_json::from_value(serde_json::json!({
            "name": "task-management",
            "entity_types": {
                "task": {
                    "fields": { "title": { "type": "string", "required": true, "x-embed": true } }
                }
            }
        }))
        .unwrap();
        yorishiro_core::repositories::schemas::create_schema(&mut conn, workspace_id, definition)
            .await
            .unwrap();
        // core's create doesn't write the embedding (that's the adapter's background sync
        // job), so this entity reproduces one left behind by a failed sync.
        let entity = yorishiro_core::repositories::entities::create(
            &mut conn,
            workspace_id,
            yorishiro_core::repositories::entities::CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: serde_json::json!({ "title": "orphaned" }),
            },
            None,
        )
        .await
        .unwrap();
        drop(conn);

        let report = resync_embeddings(&pool, workspace_id, &FixedProvider)
            .await
            .unwrap();
        assert_eq!(report.candidates, 1);
        assert_eq!(report.synced, 1);
        assert_eq!(report.failed, 0);

        let (sql, values) = Query::select()
            .expr(Expr::col(Entities::Embedding).is_not_null())
            .from((Alias::new("content"), Entities::Table))
            .and_where(Expr::col(Entities::Id).eq(entity.id))
            .build_sqlx(PostgresQueryBuilder);
        let (has_embedding,): (bool,) = sqlx::query_as_with(&sql, values)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(has_embedding);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn creates_tenant_workspace_user_and_membership(pool: PgPool) {
        let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
        let user = tenancy::create_user(&pool, "owner@example.com", "pw", None)
            .await
            .unwrap();
        tenancy::add_member(&pool, tenant.id, user.id, MembershipRole::Owner)
            .await
            .unwrap();

        let members = tenancy::list_members(&pool, tenant.id).await.unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].user_id, user.id);
        assert_eq!(members[0].role, MembershipRole::Owner);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn enforces_workspace_limit_on_create_workspace(pool: PgPool) {
        let tenant = tenancy::create_tenant(&pool, "capped", Some(1))
            .await
            .unwrap();
        // create_tenant alone doesn't create a workspace here (unlike the CLI's CreateTenant
        // handler, which additionally creates a "default" one); this test drives
        // tenancy::create_workspace directly to check the cap.
        tenancy::create_workspace(&pool, tenant.id, "first", None)
            .await
            .unwrap();

        let err = tenancy::create_workspace(&pool, tenant.id, "second", None)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            yorishiro_core::YorishiroError::Conflict { .. }
        ));
    }
}
