use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use sea_query::{Alias, Expr, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::get_tenant;
use crate::error::{ResultExt, YorishiroError};
use crate::models::tenancy::{InviteRecord, MembershipRole};

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
        .internal()?;

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
        .internal()?;

    row.map(InviteRow::into_record).transpose()
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::repositories::tenancy::create_tenant;

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
