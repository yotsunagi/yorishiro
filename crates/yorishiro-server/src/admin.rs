use anyhow::{Context, Result, bail};
use clap::{Subcommand, ValueEnum};
use sea_query::{Alias, Expr, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::auth::{self, ApiKeyScope, CreatedApiKey};
use yorishiro_core::tenancy::{self, MembershipRole};

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

/// Subcommands under `yorishiro-server admin`. API keys are stored only as SHA-256 hashes and
/// user passwords only as argon2 hashes, so neither can be provisioned by hand in SQL — this
/// CLI is the only bootstrap mechanism.
#[derive(Subcommand)]
pub enum AdminCommand {
    /// Create a new tenant, along with a default workspace under it.
    CreateTenant {
        name: String,
        /// Cap on the number of workspaces this tenant may create. Omit for unlimited
        /// (the default, appropriate for self-hosted deployments).
        #[arg(long)]
        max_workspaces: Option<i32>,
    },
    /// List all tenants.
    ListTenants,
    /// Create an additional workspace under a tenant (see `admin list-tenants` for the tenant ID).
    CreateWorkspace {
        tenant_id: Uuid,
        name: String,
        /// Cap on the number of entities this workspace may hold. Omit for unlimited.
        #[arg(long)]
        max_entities: Option<i32>,
    },
    /// List workspaces under a tenant.
    ListWorkspaces { tenant_id: Uuid },
    /// Create a human user account.
    CreateUser {
        email: String,
        password: String,
        #[arg(long)]
        display_name: Option<String>,
    },
    /// Add (or change the role of) a user's membership in a tenant.
    AddMember {
        tenant_id: Uuid,
        user_id: Uuid,
        role: RoleArg,
    },
    /// List a tenant's members.
    ListMembers { tenant_id: Uuid },
    /// Create an invite token for an email to join a tenant with a given role. Signup is
    /// invite-only; there is no self-service, unauthenticated account creation.
    CreateInvite {
        tenant_id: Uuid,
        email: String,
        role: RoleArg,
        /// How long the invite stays redeemable. Defaults to 7 days.
        #[arg(long, default_value_t = 168)]
        ttl_hours: i64,
    },
    /// Issue a new API key for a workspace (see `admin list-workspaces <tenant-id>` for the
    /// workspace ID).
    CreateApiKey {
        workspace_id: Uuid,
        scope: ScopeArg,
        /// Attribute the key to a specific user (see `admin list-members <tenant-id>`). The
        /// requested scope is capped by that user's tenant role (owner/admin: schema,
        /// member: write, viewer: read); omit for an unattributed service key.
        #[arg(long)]
        user: Option<Uuid>,
    },
    /// List API keys for a workspace.
    ListApiKeys { workspace_id: Uuid },
    /// Revoke (delete) an API key (see `admin list-api-keys <workspace-id>` for the key ID).
    RevokeApiKey { key_id: Uuid },
    /// Re-sync embeddings for entities whose embedding is still missing.
    ResyncEmbeddings { workspace_id: Uuid },
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

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RoleArg {
    Owner,
    Admin,
    Member,
    Viewer,
}

impl From<RoleArg> for MembershipRole {
    fn from(value: RoleArg) -> Self {
        match value {
            RoleArg::Owner => MembershipRole::Owner,
            RoleArg::Admin => MembershipRole::Admin,
            RoleArg::Member => MembershipRole::Member,
            RoleArg::Viewer => MembershipRole::Viewer,
        }
    }
}

/// Entry point for the admin subcommands. Unlike a plain server start (no args), this
/// operates on the database directly using the DATABASE_URL connection role (the admin
/// role that can run migrations and is the only role with write access to `identity.tenants`/
/// `identity.users`/`identity.tenant_memberships`).
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
        AdminCommand::CreateTenant {
            name,
            max_workspaces,
        } => {
            let tenant = tenancy::create_tenant(&pool, &name, max_workspaces).await?;
            let workspace = tenancy::create_workspace(&pool, tenant.id, "default", None).await?;
            println!("tenant created");
            println!("  id:            {}", tenant.id);
            println!("  name:          {}", tenant.name);
            println!("  max_workspaces: {}", format_limit(tenant.max_workspaces));
            println!("default workspace created");
            println!("  id:   {}", workspace.id);
            println!("  name: {}", workspace.name);
        }
        AdminCommand::ListTenants => {
            let tenants = tenancy::list_tenants(&pool).await?;
            if tenants.is_empty() {
                println!("no tenants (create one with `admin create-tenant <name>`)");
            }
            for tenant in tenants {
                println!(
                    "{}  {:<24} max_workspaces={}",
                    tenant.id,
                    tenant.name,
                    format_limit(tenant.max_workspaces)
                );
            }
        }
        AdminCommand::CreateWorkspace {
            tenant_id,
            name,
            max_entities,
        } => {
            let workspace = tenancy::create_workspace(&pool, tenant_id, &name, max_entities)
                .await
                .map_err(anyhow::Error::from)?;
            println!("workspace created");
            println!("  id:           {}", workspace.id);
            println!("  tenant id:    {}", workspace.tenant_id);
            println!("  name:         {}", workspace.name);
            println!("  max_entities: {}", format_limit(workspace.max_entities));
        }
        AdminCommand::ListWorkspaces { tenant_id } => {
            let workspaces = tenancy::list_workspaces(&pool, tenant_id)
                .await
                .map_err(anyhow::Error::from)?;
            if workspaces.is_empty() {
                println!("no workspaces for tenant {tenant_id}");
            }
            for workspace in workspaces {
                println!(
                    "{}  {:<24} max_entities={}",
                    workspace.id,
                    workspace.name,
                    format_limit(workspace.max_entities)
                );
            }
        }
        AdminCommand::CreateUser {
            email,
            password,
            display_name,
        } => {
            let user = tenancy::create_user(&pool, &email, &password, display_name.as_deref())
                .await
                .map_err(anyhow::Error::from)?;
            println!("user created");
            println!("  id:    {}", user.id);
            println!("  email: {}", user.email);
        }
        AdminCommand::AddMember {
            tenant_id,
            user_id,
            role,
        } => {
            tenancy::add_member(&pool, tenant_id, user_id, role.into())
                .await
                .map_err(anyhow::Error::from)?;
            println!("membership added: user {user_id} is now {role:?} of tenant {tenant_id}");
        }
        AdminCommand::ListMembers { tenant_id } => {
            let members = tenancy::list_members(&pool, tenant_id)
                .await
                .map_err(anyhow::Error::from)?;
            if members.is_empty() {
                println!("no members for tenant {tenant_id}");
            }
            for member in members {
                println!("{}  {:<8?} {}", member.user_id, member.role, member.email);
            }
        }
        AdminCommand::CreateInvite {
            tenant_id,
            email,
            role,
            ttl_hours,
        } => {
            let (invite, token) = tenancy::create_invite(
                &pool,
                tenant_id,
                &email,
                role.into(),
                chrono::Duration::hours(ttl_hours),
            )
            .await
            .map_err(anyhow::Error::from)?;
            println!("invite created (the plaintext token is shown ONLY once — send it now)");
            println!("  token:      {token}");
            println!("  invite id:  {}", invite.id);
            println!("  tenant id:  {}", invite.tenant_id);
            println!("  email:      {}", invite.email);
            println!("  role:       {:?}", invite.role);
            println!(
                "  expires at: {}",
                invite.expires_at.format("%Y-%m-%d %H:%M UTC")
            );
        }
        AdminCommand::CreateApiKey {
            workspace_id,
            scope,
            user,
        } => {
            let scope = ApiKeyScope::from(scope);
            let created = create_api_key(&pool, workspace_id, scope, user).await?;
            println!("api key created (the plaintext key is shown ONLY once — store it now)");
            println!("  key:          {}", created.plaintext);
            println!("  key id:       {}", created.id);
            println!("  workspace id: {}", created.workspace_id);
            println!("  scope:        {scope:?}");
            if let Some(user_id) = created.user_id {
                println!("  user id:      {user_id}");
            }
        }
        AdminCommand::ListApiKeys { workspace_id } => {
            let keys = list_api_keys(&pool, workspace_id).await?;
            if keys.is_empty() {
                println!("no api keys for workspace {workspace_id}");
            }
            for key in keys {
                println!(
                    "{}  {:<8} prefix={}  user={}  created={}  last_used={}",
                    key.id,
                    key.scope,
                    key.key_prefix,
                    key.user_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "-".into()),
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
        AdminCommand::ResyncEmbeddings { workspace_id } => {
            let provider = crate::build_embedding_provider()
                .context("embedding provider must be configured (see .env.example)")?;
            let report = resync_embeddings(&pool, workspace_id, provider.as_ref()).await?;
            println!(
                "resync finished: {} entities had no embedding, {} synced, {} failed \
                 (entities whose entity_type has no x-embed field stay without embedding)",
                report.candidates, report.synced, report.failed,
            );
        }
    }

    Ok(())
}

fn format_limit(limit: Option<i32>) -> String {
    match limit {
        Some(n) => n.to_string(),
        None => "unlimited".to_string(),
    }
}

async fn create_api_key(
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
struct ApiKeySummary {
    id: Uuid,
    scope: String,
    key_prefix: String,
    user_id: Option<Uuid>,
    created_at: chrono::DateTime<chrono::Utc>,
    last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn list_api_keys(pool: &PgPool, workspace_id: Uuid) -> Result<Vec<ApiKeySummary>> {
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
async fn revoke_api_key(pool: &PgPool, key_id: Uuid) -> Result<()> {
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
    workspace_id: Uuid,
    provider: &dyn yorishiro_core::embedding::EmbeddingProvider,
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
            let record = yorishiro_core::entities::get(&mut conn, workspace_id, entity_id).await?;
            yorishiro_core::embedding_sync::sync_embedding_for_record(
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
        yorishiro_core::schemas::create_schema(&mut conn, workspace_id, definition)
            .await
            .unwrap();
        // core's create doesn't write the embedding (that's the adapter's background sync
        // job), so this entity reproduces one left behind by a failed sync.
        let entity = yorishiro_core::entities::create(
            &mut conn,
            workspace_id,
            yorishiro_core::entities::CreateEntityInput {
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
