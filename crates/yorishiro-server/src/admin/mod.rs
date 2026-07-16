mod commands;

use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};
use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::repositories::tenancy::{self, MembershipRole};
use yorishiro_core::services::auth::ApiKeyScope;

use commands::{create_api_key, list_api_keys, resync_embeddings, revoke_api_key};

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
