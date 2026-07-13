//! Control-plane CRUD for tenants, workspaces, users, and tenant memberships. Everything here
//! operates on a raw `&PgPool` rather than an RLS-scoped connection: callers (the admin CLI)
//! connect using the migration/admin role, which is the only role permitted to touch
//! `identity.tenants`/`identity.users`/`identity.tenant_memberships` at all (see the
//! role-separation migration).

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::YorishiroError;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TenantRecord {
    pub id: Uuid,
    pub name: String,
    pub plan: Option<String>,
    pub max_workspaces: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WorkspaceRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub max_entities: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct UserRecord {
    pub id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Mirrors the `identity.tenant_memberships.role` check constraint
/// (`owner`/`admin`/`member`/`viewer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MembershipRole {
    Owner,
    Admin,
    Member,
    Viewer,
}

impl MembershipRole {
    fn as_db_str(self) -> &'static str {
        match self {
            MembershipRole::Owner => "owner",
            MembershipRole::Admin => "admin",
            MembershipRole::Member => "member",
            MembershipRole::Viewer => "viewer",
        }
    }

    fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(MembershipRole::Owner),
            "admin" => Some(MembershipRole::Admin),
            "member" => Some(MembershipRole::Member),
            "viewer" => Some(MembershipRole::Viewer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MembershipRecord {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub role: MembershipRole,
}

pub async fn create_tenant(
    pool: &PgPool,
    name: &str,
    max_workspaces: Option<i32>,
) -> Result<TenantRecord, YorishiroError> {
    sqlx::query_as::<_, TenantRecord>(
        "INSERT INTO identity.tenants (name, max_workspaces) VALUES ($1, $2) \
         RETURNING id, name, plan, max_workspaces, created_at",
    )
    .bind(name)
    .bind(max_workspaces)
    .fetch_one(pool)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))
}

pub async fn get_tenant(pool: &PgPool, tenant_id: Uuid) -> Result<TenantRecord, YorishiroError> {
    sqlx::query_as::<_, TenantRecord>(
        "SELECT id, name, plan, max_workspaces, created_at FROM identity.tenants WHERE id = $1",
    )
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))?
    .ok_or_else(|| YorishiroError::NotFound {
        message: format!("tenant '{tenant_id}' was not found"),
    })
}

pub async fn list_tenants(pool: &PgPool) -> Result<Vec<TenantRecord>, YorishiroError> {
    sqlx::query_as::<_, TenantRecord>(
        "SELECT id, name, plan, max_workspaces, created_at FROM identity.tenants ORDER BY created_at",
    )
    .fetch_all(pool)
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
        let (count,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM identity.workspaces WHERE tenant_id = $1")
                .bind(tenant_id)
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

    sqlx::query_as::<_, WorkspaceRecord>(
        "INSERT INTO identity.workspaces (tenant_id, name, max_entities) VALUES ($1, $2, $3) \
         RETURNING id, tenant_id, name, max_entities, created_at",
    )
    .bind(tenant_id)
    .bind(name)
    .bind(max_entities)
    .fetch_one(pool)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))
}

pub async fn list_workspaces(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Vec<WorkspaceRecord>, YorishiroError> {
    sqlx::query_as::<_, WorkspaceRecord>(
        "SELECT id, tenant_id, name, max_entities, created_at FROM identity.workspaces \
         WHERE tenant_id = $1 ORDER BY created_at",
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))
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
    sqlx::query_as::<_, UserRecord>(
        "INSERT INTO identity.users (email, password_hash, display_name) VALUES ($1, $2, $3) \
         RETURNING id, email, display_name, created_at",
    )
    .bind(email)
    .bind(password_hash)
    .bind(display_name)
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

/// Verifies an email/password pair against the stored argon2 hash, returning the matching
/// user on success. Not yet wired to an HTTP endpoint (this server is driven by API keys, not
/// interactive sessions) but kept alongside `create_user` so an eventual login flow can reuse
/// the same password storage/verification path rather than inventing a second one.
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

    let row: Option<UserWithHash> = sqlx::query_as(
        "SELECT id, email, display_name, created_at, password_hash FROM identity.users \
         WHERE email = $1",
    )
    .bind(email)
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
    sqlx::query(
        "INSERT INTO identity.tenant_memberships (tenant_id, user_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT (tenant_id, user_id) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(role.as_db_str())
    .execute(pool)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))?;
    Ok(())
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

    let rows: Vec<MembershipRow> = sqlx::query_as(
        "SELECT u.id AS user_id, u.email, u.display_name, m.role \
         FROM identity.tenant_memberships m \
         JOIN identity.users u ON u.id = m.user_id \
         WHERE m.tenant_id = $1 ORDER BY m.created_at",
    )
    .bind(tenant_id)
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
