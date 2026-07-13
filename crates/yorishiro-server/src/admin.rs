use anyhow::{Context, Result, bail};
use clap::{Subcommand, ValueEnum};
use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::auth::{self, ApiKeyScope, CreatedApiKey};

/// Subcommands under `yorishiro-server admin`. API keys are stored only as SHA-256 hashes,
/// so issuing one can't be done by hand in SQL — this CLI is the only bootstrap mechanism.
#[derive(Subcommand)]
pub enum AdminCommand {
    /// Create a new tenant.
    CreateTenant { name: String },
    /// Issue a new API key for a tenant (see `admin list-tenants` for the tenant ID).
    CreateApiKey { tenant_id: Uuid, scope: ScopeArg },
    /// List API keys for a tenant.
    ListApiKeys { tenant_id: Uuid },
    /// Revoke (delete) an API key (see `admin list-api-keys <tenant-id>` for the key ID).
    RevokeApiKey { key_id: Uuid },
    /// Re-sync embeddings for entities whose embedding is still missing.
    ResyncEmbeddings { tenant_id: Uuid },
    /// List all tenants.
    ListTenants,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ScopeArg {
    Read,
    Write,
    Schema,
}

impl From<ScopeArg> for ApiKeyScope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::Read => ApiKeyScope::Read,
            ScopeArg::Write => ApiKeyScope::Write,
            ScopeArg::Schema => ApiKeyScope::Schema,
        }
    }
}

/// Entry point for the admin subcommands. Unlike a plain server start (no args), this
/// operates on the database directly using the DATABASE_URL connection role (the admin
/// role that can run migrations).
pub async fn run(command: AdminCommand) -> Result<()> {
    let database_url =
        std::env::var("DATABASE_URL").context("DATABASE_URL must be set for admin commands")?;
    let pool = PgPool::connect(&database_url)
        .await
        .context("failed to connect to database")?;
    // Apply the same migrations the server runs on startup so this also works against a
    // fresh database that has never been started (a no-op if already applied).
    sqlx::migrate!("../../migrations").run(&pool).await?;

    match command {
        AdminCommand::CreateTenant { name } => {
            let tenant_id = create_tenant(&pool, &name).await?;
            println!("tenant created");
            println!("  id:   {tenant_id}");
            println!("  name: {name}");
        }
        AdminCommand::CreateApiKey { tenant_id, scope } => {
            let scope = ApiKeyScope::from(scope);
            let created = create_api_key(&pool, tenant_id, scope).await?;
            println!("api key created (the plaintext key is shown ONLY once — store it now)");
            println!("  key:       {}", created.plaintext);
            println!("  key id:    {}", created.id);
            println!("  tenant id: {}", created.tenant_id);
            println!("  scope:     {scope:?}");
        }
        AdminCommand::ListApiKeys { tenant_id } => {
            let keys = list_api_keys(&pool, tenant_id).await?;
            if keys.is_empty() {
                println!("no api keys for tenant {tenant_id}");
            }
            for key in keys {
                println!(
                    "{}  {:<8} prefix={}  created={}  last_used={}",
                    key.id,
                    key.scope,
                    key.key_prefix,
                    key.created_at.format("%Y-%m-%d %H:%M"),
                    key.last_used_at
                        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "never".into()),
                );
            }
        }
        AdminCommand::RevokeApiKey { key_id } => {
            revoke_api_key(&pool, key_id).await?;
            println!("api key {key_id} revoked (takes effect on the next request)");
        }
        AdminCommand::ResyncEmbeddings { tenant_id } => {
            let provider = crate::build_embedding_provider()
                .context("embedding provider must be configured (see .env.example)")?;
            let report = resync_embeddings(&pool, tenant_id, provider.as_ref()).await?;
            println!(
                "resync finished: {} entities had no embedding, {} synced, {} failed \
                 (entities whose entity_type has no x-embed field stay without embedding)",
                report.candidates, report.synced, report.failed,
            );
        }
        AdminCommand::ListTenants => {
            let tenants = list_tenants(&pool).await?;
            if tenants.is_empty() {
                println!("no tenants (create one with `admin create-tenant <name>`)");
            }
            for (id, name) in tenants {
                println!("{id}  {name}");
            }
        }
    }

    Ok(())
}

async fn create_tenant(pool: &PgPool, name: &str) -> Result<Uuid> {
    let (id,): (Uuid,) = sqlx::query_as("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .context("failed to create tenant")?;
    Ok(id)
}

async fn create_api_key(
    pool: &PgPool,
    tenant_id: Uuid,
    scope: ApiKeyScope,
) -> Result<CreatedApiKey> {
    // Check the tenant exists up front so the error is clearer than a raw FK violation.
    let exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM tenants WHERE id = $1")
        .bind(tenant_id)
        .fetch_optional(pool)
        .await?;
    if exists.is_none() {
        bail!("tenant '{tenant_id}' does not exist (see `admin list-tenants`)");
    }

    let mut conn = pool.acquire().await?;
    let created = auth::create_api_key(&mut conn, tenant_id, scope)
        .await
        .context("failed to create api key")?;
    Ok(created)
}

async fn list_tenants(pool: &PgPool) -> Result<Vec<(Uuid, String)>> {
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM tenants ORDER BY created_at")
            .fetch_all(pool)
            .await?;
    Ok(rows)
}

#[derive(sqlx::FromRow)]
struct ApiKeySummary {
    id: Uuid,
    scope: String,
    key_prefix: String,
    created_at: chrono::DateTime<chrono::Utc>,
    last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn list_api_keys(pool: &PgPool, tenant_id: Uuid) -> Result<Vec<ApiKeySummary>> {
    let rows = sqlx::query_as::<_, ApiKeySummary>(
        "SELECT id, scope, key_prefix, created_at, last_used_at \
         FROM api_keys WHERE tenant_id = $1 ORDER BY created_at",
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Authentication looks up the key in the database on every request, so deleting the row
/// revokes it immediately.
async fn revoke_api_key(pool: &PgPool, key_id: Uuid) -> Result<()> {
    let result = sqlx::query("DELETE FROM api_keys WHERE id = $1")
        .bind(key_id)
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        bail!("api key '{key_id}' does not exist (see `admin list-api-keys <tenant-id>`)");
    }
    Ok(())
}

struct ResyncReport {
    candidates: usize,
    synced: usize,
    failed: usize,
}

/// Re-syncs embeddings for entities whose `embedding` column is still NULL. An operational
/// recovery command for entities that fell out of search due to a failed background sync
/// (e.g. a transient embedding API outage or a process killed mid-deploy).
async fn resync_embeddings(
    pool: &PgPool,
    tenant_id: Uuid,
    provider: &dyn yorishiro_core::embedding::EmbeddingProvider,
) -> Result<ResyncReport> {
    let ids: Vec<(Uuid,)> =
        sqlx::query_as("SELECT id FROM entities WHERE tenant_id = $1 AND embedding IS NULL")
            .bind(tenant_id)
            .fetch_all(pool)
            .await?;

    let mut report = ResyncReport {
        candidates: ids.len(),
        synced: 0,
        failed: 0,
    };
    let mut conn = pool.acquire().await?;
    for (entity_id,) in ids {
        let result = async {
            let record = yorishiro_core::entities::get(&mut conn, tenant_id, entity_id).await?;
            yorishiro_core::embedding_sync::sync_embedding_for_record(
                &mut conn, tenant_id, &record, provider,
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

    #[sqlx::test(migrations = "../../migrations")]
    async fn creates_tenant_and_issues_a_usable_key(pool: PgPool) {
        let tenant_id = create_tenant(&pool, "bootstrap-tenant").await.unwrap();

        let created = create_api_key(&pool, tenant_id, ApiKeyScope::Write)
            .await
            .unwrap();
        assert_eq!(created.tenant_id, tenant_id);
        assert!(created.plaintext.starts_with("ysr_"));

        // Confirm the issued key actually authenticates, not just that creation returned Ok.
        let ctx = auth::authenticate(&pool, &created.plaintext).await.unwrap();
        assert_eq!(ctx.tenant_id, tenant_id);
        assert_eq!(ctx.scope, ApiKeyScope::Write);

        let tenants = list_tenants(&pool).await.unwrap();
        assert_eq!(tenants.len(), 1);
        assert_eq!(tenants[0].1, "bootstrap-tenant");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_key_creation_for_unknown_tenant(pool: PgPool) {
        let result = create_api_key(&pool, Uuid::nil(), ApiKeyScope::Read).await;
        let Err(err) = result else {
            panic!("key creation should fail for an unknown tenant");
        };
        assert!(err.to_string().contains("does not exist"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn revoked_key_no_longer_authenticates(pool: PgPool) {
        let tenant_id = create_tenant(&pool, "revoke-test").await.unwrap();
        let created = create_api_key(&pool, tenant_id, ApiKeyScope::Read)
            .await
            .unwrap();
        auth::authenticate(&pool, &created.plaintext).await.unwrap();

        let listed = list_api_keys(&pool, tenant_id).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);

        revoke_api_key(&pool, created.id).await.unwrap();

        let result = auth::authenticate(&pool, &created.plaintext).await;
        assert!(matches!(
            result,
            Err(yorishiro_core::YorishiroError::Unauthenticated)
        ));
        assert!(list_api_keys(&pool, tenant_id).await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn resync_fills_missing_embeddings(pool: PgPool) {
        use async_trait::async_trait;
        use yorishiro_core::YorishiroError;
        use yorishiro_core::embedding::EmbeddingProvider;

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

        let tenant_id = create_tenant(&pool, "resync-test").await.unwrap();
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
        yorishiro_core::schemas::create_schema(&mut conn, tenant_id, definition)
            .await
            .unwrap();
        // core's create doesn't write the embedding (that's the adapter's background sync
        // job), so this entity reproduces one left behind by a failed sync.
        let entity = yorishiro_core::entities::create(
            &mut conn,
            tenant_id,
            yorishiro_core::entities::CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: serde_json::json!({ "title": "orphaned" }),
            },
        )
        .await
        .unwrap();
        drop(conn);

        let report = resync_embeddings(&pool, tenant_id, &FixedProvider)
            .await
            .unwrap();
        assert_eq!(report.candidates, 1);
        assert_eq!(report.synced, 1);
        assert_eq!(report.failed, 0);

        let (has_embedding,): (bool,) =
            sqlx::query_as("SELECT embedding IS NOT NULL FROM entities WHERE id = $1")
                .bind(entity.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(has_embedding);
    }
}
