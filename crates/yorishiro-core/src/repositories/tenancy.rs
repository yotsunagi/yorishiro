//! Control-plane CRUD for tenants, workspaces, users, and tenant memberships. Everything here
//! operates on a raw `&PgPool` rather than an RLS-scoped connection: callers (the admin CLI)
//! connect using the migration/admin role, which is the only role permitted to touch
//! `identity.tenants`/`identity.users`/`identity.tenant_memberships` at all (see the
//! role-separation migration).

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use sea_query::{
    Alias, Asterisk, Expr, Func, Iden, OnConflict, Order, PostgresQueryBuilder, Query,
};
use sea_query_binder::SqlxBinder;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::YorishiroError;

pub use crate::models::tenancy::*;

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

#[derive(Iden)]
enum Workspaces {
    Table,
    Id,
    TenantId,
    Name,
    MaxEntities,
    CreatedAt,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
    Email,
    PasswordHash,
    DisplayName,
    CreatedAt,
}

#[derive(Iden)]
enum TenantMemberships {
    Table,
    TenantId,
    UserId,
    Role,
    CreatedAt,
}

#[derive(Iden)]
enum Invites {
    Table,
    Id,
    TenantId,
    Email,
    Role,
    TokenHash,
    ExpiresAt,
    UsedAt,
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
            .map_err(|err| YorishiroError::Internal(err.into()))?;
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
        .map_err(|err| YorishiroError::Internal(err.into()))
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
        .map_err(|err| YorishiroError::Internal(err.into()))?
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
        .map_err(|err| YorishiroError::Internal(err.into()))
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
        .map_err(|err| YorishiroError::Internal(err.into()))?
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
        .map_err(|err| YorishiroError::Internal(err.into()))?
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
        .map_err(|err| YorishiroError::Internal(err.into()))
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
            .map_err(|err| YorishiroError::Internal(err.into()))?;
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
        .map_err(|err| YorishiroError::Internal(err.into()))
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
        .map_err(|err| YorishiroError::Internal(err.into()))
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
        .map_err(|err| YorishiroError::Internal(err.into()))
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
        .map_err(|err| YorishiroError::Internal(err.into()))?
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!("workspace '{workspace_id}' was not found"),
        })
}

fn hash_password(password: &str) -> Result<String, YorishiroError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| YorishiroError::Internal(anyhow::anyhow!("failed to hash password: {err}")))
}

fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Creates a human user account. Passwords are hashed with argon2 (the current OWASP
/// recommendation for password storage) before ever reaching the database.
pub async fn create_user(
    pool: &PgPool,
    email: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<UserRecord, YorishiroError> {
    let password_hash = hash_password(password)?;
    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Users::Table))
        .columns([Users::Email, Users::PasswordHash, Users::DisplayName])
        .values_panic([email.into(), password_hash.into(), display_name.into()])
        .returning(Query::returning().columns([
            Users::Id,
            Users::Email,
            Users::DisplayName,
            Users::CreatedAt,
        ]))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, UserRecord, _>(&sql, values)
        .fetch_one(pool)
        .await
        .map_err(|err| {
            if let sqlx::Error::Database(db_err) = &err
                && db_err.is_unique_violation()
            {
                YorishiroError::Conflict {
                    message: format!("a user with email '{email}' already exists"),
                }
            } else {
                YorishiroError::Internal(err.into())
            }
        })
}

/// Looks up an existing user by email, without touching their password hash. Used by member
/// management (adding an *existing* account to another tenant) to resolve an email to a
/// `user_id` before calling `add_member` -- as opposed to signup, which creates the account.
pub async fn get_user_by_email(
    pool: &PgPool,
    email: &str,
) -> Result<Option<UserRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns([
            Users::Id,
            Users::Email,
            Users::DisplayName,
            Users::CreatedAt,
        ])
        .from((Alias::new("identity"), Users::Table))
        .and_where(Expr::col(Users::Email).eq(email))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, UserRecord, _>(&sql, values)
        .fetch_optional(pool)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))
}

/// Verifies an email/password pair against the stored argon2 hash, returning the matching
/// user on success. Backs the `/auth/login` REST endpoint.
pub async fn verify_login(
    pool: &PgPool,
    email: &str,
    password: &str,
) -> Result<Option<UserRecord>, YorishiroError> {
    #[derive(sqlx::FromRow)]
    struct UserWithHash {
        id: Uuid,
        email: String,
        display_name: Option<String>,
        created_at: DateTime<Utc>,
        password_hash: String,
    }

    let (sql, values) = Query::select()
        .columns([
            Users::Id,
            Users::Email,
            Users::DisplayName,
            Users::CreatedAt,
            Users::PasswordHash,
        ])
        .from((Alias::new("identity"), Users::Table))
        .and_where(Expr::col(Users::Email).eq(email))
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<UserWithHash> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    let Some(row) = row else {
        return Ok(None);
    };

    if verify_password(password, &row.password_hash) {
        Ok(Some(UserRecord {
            id: row.id,
            email: row.email,
            display_name: row.display_name,
            created_at: row.created_at,
        }))
    } else {
        Ok(None)
    }
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
        .map_err(|err| YorishiroError::Internal(err.into()))?;
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
        .map_err(|err| YorishiroError::Internal(err.into()))?;

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
        .map_err(|err| YorishiroError::Internal(err.into()))?;

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

const INVITE_TOKEN_BYTES: usize = 24;

fn random_invite_token() -> String {
    let mut bytes = [0u8; INVITE_TOKEN_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_invite_token(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}

#[derive(sqlx::FromRow)]
struct InviteRow {
    id: Uuid,
    tenant_id: Uuid,
    email: String,
    role: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

fn invite_columns() -> [Invites; 6] {
    [
        Invites::Id,
        Invites::TenantId,
        Invites::Email,
        Invites::Role,
        Invites::ExpiresAt,
        Invites::CreatedAt,
    ]
}

impl InviteRow {
    fn into_record(self) -> Result<InviteRecord, YorishiroError> {
        let role = MembershipRole::from_db_str(&self.role).ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!(
                "unknown membership role in database: {}",
                self.role
            ))
        })?;
        Ok(InviteRecord {
            id: self.id,
            tenant_id: self.tenant_id,
            email: self.email,
            role,
            expires_at: self.expires_at,
            created_at: self.created_at,
        })
    }
}

/// Creates an invite token for `email` to join `tenant_id` with `role`. Returns the record
/// alongside the plaintext token: like API keys, only its SHA-256 hash is persisted (a KDF
/// like argon2 isn't needed here either, for the same reason -- the token already carries
/// enough entropy that offline brute-forcing isn't realistic), so this is the only place the
/// plaintext is ever available. Callers must surface it themselves (e.g. print it, or send it
/// by email once a transactional-email integration exists).
pub async fn create_invite(
    pool: &PgPool,
    tenant_id: Uuid,
    email: &str,
    role: MembershipRole,
    ttl: Duration,
) -> Result<(InviteRecord, String), YorishiroError> {
    get_tenant(pool, tenant_id).await?;

    let token = random_invite_token();
    let token_hash = hash_invite_token(&token);
    let expires_at = Utc::now() + ttl;

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Invites::Table))
        .columns([
            Invites::TenantId,
            Invites::Email,
            Invites::Role,
            Invites::TokenHash,
            Invites::ExpiresAt,
        ])
        .values_panic([
            tenant_id.into(),
            email.into(),
            role.as_db_str().into(),
            token_hash.into(),
            expires_at.into(),
        ])
        .returning(Query::returning().columns(invite_columns()))
        .build_sqlx(PostgresQueryBuilder);

    let row: InviteRow = sqlx::query_as_with(&sql, values)
        .fetch_one(pool)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    Ok((row.into_record()?, token))
}

/// Redeems an invite token: atomically marks it used and returns the tenant/email/role it
/// grants, or `None` if the token doesn't match any invite, is already used, or has expired.
/// The lookup and the `used_at` update happen in a single statement so two concurrent
/// redemptions of the same token can't both succeed.
pub async fn redeem_invite(
    pool: &PgPool,
    raw_token: &str,
) -> Result<Option<InviteRecord>, YorishiroError> {
    let token_hash = hash_invite_token(raw_token);

    let (sql, values) = Query::update()
        .table((Alias::new("identity"), Invites::Table))
        .values([(Invites::UsedAt, Expr::current_timestamp().into())])
        .and_where(Expr::col(Invites::TokenHash).eq(token_hash))
        .and_where(Expr::col(Invites::UsedAt).is_null())
        .and_where(Expr::col(Invites::ExpiresAt).gt(Expr::current_timestamp()))
        .returning(Query::returning().columns(invite_columns()))
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<InviteRow> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    row.map(InviteRow::into_record).transpose()
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;

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

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_workspace_rejects_unknown_tenant(pool: PgPool) {
        let err = create_workspace(&pool, Uuid::nil(), "orphan", None)
            .await
            .unwrap_err();
        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn creates_user_and_verifies_login(pool: PgPool) {
        let user = create_user(&pool, "alice@example.com", "hunter2", Some("Alice"))
            .await
            .unwrap();
        assert_eq!(user.email, "alice@example.com");

        let ok = verify_login(&pool, "alice@example.com", "hunter2")
            .await
            .unwrap();
        assert!(ok.is_some());

        let bad = verify_login(&pool, "alice@example.com", "wrong-password")
            .await
            .unwrap();
        assert!(bad.is_none());

        let unknown = verify_login(&pool, "nobody@example.com", "hunter2")
            .await
            .unwrap();
        assert!(unknown.is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_duplicate_email(pool: PgPool) {
        create_user(&pool, "bob@example.com", "pw", None)
            .await
            .unwrap();
        let err = create_user(&pool, "bob@example.com", "pw2", None)
            .await
            .unwrap_err();
        assert!(matches!(err, YorishiroError::Conflict { .. }));
    }

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

    #[sqlx::test(migrations = "../../migrations")]
    async fn creates_and_redeems_an_invite(pool: PgPool) {
        let tenant = create_tenant(&pool, "team", None).await.unwrap();

        let (invite, token) = create_invite(
            &pool,
            tenant.id,
            "frank@example.com",
            MembershipRole::Member,
            Duration::hours(24),
        )
        .await
        .unwrap();
        assert_eq!(invite.tenant_id, tenant.id);
        assert_eq!(invite.email, "frank@example.com");
        assert_eq!(invite.role, MembershipRole::Member);

        let redeemed = redeem_invite(&pool, &token).await.unwrap().unwrap();
        assert_eq!(redeemed.id, invite.id);
        assert_eq!(redeemed.tenant_id, tenant.id);
        assert_eq!(redeemed.role, MembershipRole::Member);

        // A token can only be redeemed once.
        let second_attempt = redeem_invite(&pool, &token).await.unwrap();
        assert!(second_attempt.is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn redeem_invite_rejects_unknown_or_garbled_tokens(pool: PgPool) {
        let result = redeem_invite(&pool, "not-a-real-token").await.unwrap();
        assert!(result.is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn redeem_invite_rejects_an_expired_token(pool: PgPool) {
        let tenant = create_tenant(&pool, "team", None).await.unwrap();

        let (_invite, token) = create_invite(
            &pool,
            tenant.id,
            "grace@example.com",
            MembershipRole::Viewer,
            Duration::hours(-1),
        )
        .await
        .unwrap();

        let result = redeem_invite(&pool, &token).await.unwrap();
        assert!(result.is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_invite_rejects_unknown_tenant(pool: PgPool) {
        let err = create_invite(
            &pool,
            Uuid::nil(),
            "nobody@example.com",
            MembershipRole::Member,
            Duration::hours(24),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }
}
