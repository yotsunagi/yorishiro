use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::pool::PoolConnection;
use sqlx::{PgConnection, PgPool, Postgres};
use uuid::Uuid;

use crate::db::TenantDb;
use crate::error::YorishiroError;

const KEY_PREFIX_BYTES: usize = 6;
const KEY_SECRET_BYTES: usize = 24;

/// APIキーが持つ権限レベル。宣言順が`Ord`の導出に使われるため、
/// `Read < Write < Schema`という階層（上位scopeは下位の権限を包含する）を
/// 前提にする箇所は必ずこの並びに依存する。`serde`表現はDBの`scope`列
/// （'read'/'write'/'schema'）と一致させてあり、REST/MCPアダプタ側で
/// 別途マッピングを持たずに済む。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyScope {
    Read,
    Write,
    Schema,
}

impl ApiKeyScope {
    fn as_db_str(self) -> &'static str {
        match self {
            ApiKeyScope::Read => "read",
            ApiKeyScope::Write => "write",
            ApiKeyScope::Schema => "schema",
        }
    }

    fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "read" => Some(ApiKeyScope::Read),
            "write" => Some(ApiKeyScope::Write),
            "schema" => Some(ApiKeyScope::Schema),
            _ => None,
        }
    }

    /// このscopeを持つキーで`required`が要求する操作を行えるか。
    /// 上位scopeは下位の権限を包含する（`write`キーは`read`操作も許可される）。
    pub fn satisfies(self, required: ApiKeyScope) -> bool {
        self >= required
    }
}

/// APIキー認証で確定したテナント・スコープ情報。以降のRLSコンテキスト設定
/// （`TenantDb::acquire_for_tenant`）とスコープ強制の両方の起点になる。
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub api_key_id: Uuid,
    pub tenant_id: Uuid,
    pub scope: ApiKeyScope,
}

pub struct CreatedApiKey {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub scope: ApiKeyScope,
    /// 生のAPIキー文字列。DBにはハッシュしか保存されないため、この呼び出しの
    /// 戻り値以外で二度と取得できない。呼び出し側は確実にユーザーへ提示すること。
    pub plaintext: String,
}

fn random_hex(byte_len: usize) -> String {
    let mut bytes = vec![0u8; byte_len];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_key(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}

/// 新しいAPIキーを発行する。キー自体は`ysr_<prefix>_<secret>`の形式で、
/// `secret`部分（192bit）だけが実質的な認証情報。APIキーは十分に高い
/// エントロピーを持つため、ハッシュ関数はbcrypt/argon2のような低速なものではなく
/// SHA-256で十分（オフライン総当たりが現実的な脅威にならないため）。
pub async fn create_api_key(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    scope: ApiKeyScope,
) -> Result<CreatedApiKey, YorishiroError> {
    let prefix = format!("ysr_{}", random_hex(KEY_PREFIX_BYTES));
    let secret = random_hex(KEY_SECRET_BYTES);
    let plaintext = format!("{prefix}_{secret}");
    let key_hash = hash_key(&plaintext);

    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO api_keys (tenant_id, key_hash, key_prefix, scope) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(tenant_id)
    .bind(key_hash)
    .bind(&prefix)
    .bind(scope.as_db_str())
    .fetch_one(&mut *conn)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))?;

    Ok(CreatedApiKey {
        id,
        tenant_id,
        scope,
        plaintext,
    })
}

/// 提示された生のAPIキー文字列を検証し、紐づくテナントとscopeを解決する。
///
/// この時点ではまだテナントが確定していない（RLSの`app.current_tenant`を
/// 設定しようがない）ため、`pool`から直接引いたコネクションで呼び出す
/// SECURITY DEFINER関数`authenticate_api_key`を使う。この関数はDB内で
/// api_keysテーブルのRLSを検証専用にバイパスする代わりに、返す列を
/// id/tenant_id/scopeだけに絞ってある（`key_hash`自体は返さない）。
pub async fn authenticate(
    pool: &PgPool,
    presented_key: &str,
) -> Result<AuthContext, YorishiroError> {
    let key_hash = hash_key(presented_key);

    let row: Option<(Uuid, Uuid, String)> =
        sqlx::query_as("SELECT id, tenant_id, scope FROM authenticate_api_key($1)")
            .bind(key_hash)
            .fetch_optional(pool)
            .await
            .map_err(|err| YorishiroError::Internal(err.into()))?;

    let (api_key_id, tenant_id, scope_str) = row.ok_or(YorishiroError::Unauthenticated)?;
    let scope = ApiKeyScope::from_db_str(&scope_str).ok_or_else(|| {
        YorishiroError::Internal(anyhow::anyhow!(
            "unknown api key scope in database: {scope_str}"
        ))
    })?;

    Ok(AuthContext {
        api_key_id,
        tenant_id,
        scope,
    })
}

/// 認証済みのcontextが要求scopeを満たすことを強制する。満たさない場合は
/// `YorishiroError::ScopeInsufficient`を返す。
pub fn require_scope(ctx: &AuthContext, required: ApiKeyScope) -> Result<(), YorishiroError> {
    if ctx.scope.satisfies(required) {
        Ok(())
    } else {
        Err(YorishiroError::ScopeInsufficient {
            message: format!(
                "this operation requires {required:?} scope but the API key has {:?} scope",
                ctx.scope
            ),
            hint: "十分なscopeを持つAPIキーを発行し直してください".into(),
        })
    }
}

/// APIキーの最終使用時刻を記録する。認証の成否には影響しないベストエフォートな
/// 記録なので、呼び出し側はこの関数のエラーでリクエスト全体を失敗させる必要はない。
pub async fn touch_last_used(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    api_key_id: Uuid,
) -> Result<(), YorishiroError> {
    sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(api_key_id)
        .execute(&mut *conn)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;
    Ok(())
}

/// 認可の唯一の入口。「提示された生キーを検証し、要求scopeを満たすことを確認し、
/// RLSコンテキスト設定済みのコネクションを返す」までを1つにまとめてある。
/// REST/MCP双方のアダプタがこの関数を経由しない限り`&mut PgConnection`を得る
/// 手段がない構造にすることで、スコープチェックの呼び忘れを構造的に防ぐ。
pub async fn authorize(
    tenant_db: &TenantDb,
    presented_key: &str,
    required: ApiKeyScope,
) -> Result<(AuthContext, PoolConnection<Postgres>), YorishiroError> {
    let ctx = authenticate(tenant_db.pool(), presented_key).await?;
    require_scope(&ctx, required)?;

    let mut conn = tenant_db
        .acquire_for_tenant(ctx.tenant_id)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    if let Err(err) = touch_last_used(&mut conn, ctx.tenant_id, ctx.api_key_id).await {
        tracing::warn!(error = %err, "failed to update api key last_used_at");
    }

    Ok((ctx, conn))
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use sqlx::postgres::PgPoolOptions;

    use super::*;
    use crate::db::TenantDb;

    async fn seed_tenant(pool: &PgPool) -> Uuid {
        let (id,): (Uuid,) = sqlx::query_as("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
            .bind("test-tenant")
            .fetch_one(pool)
            .await
            .unwrap();
        id
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn authenticates_a_freshly_created_key(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();

        let created = create_api_key(&mut conn, tenant_id, ApiKeyScope::Write)
            .await
            .unwrap();

        let ctx = authenticate(&pool, &created.plaintext).await.unwrap();

        assert_eq!(ctx.tenant_id, tenant_id);
        assert_eq!(ctx.api_key_id, created.id);
        assert_eq!(ctx.scope, ApiKeyScope::Write);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_an_unknown_key(pool: PgPool) {
        let err = authenticate(&pool, "ysr_does_not_exist_at_all")
            .await
            .unwrap_err();

        assert!(matches!(err, YorishiroError::Unauthenticated));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn resolves_the_correct_tenant_among_several(pool: PgPool) {
        let tenant_a = seed_tenant(&pool).await;
        let tenant_b = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());

        let mut conn_a = db.acquire_for_tenant(tenant_a).await.unwrap();
        let key_a = create_api_key(&mut conn_a, tenant_a, ApiKeyScope::Read)
            .await
            .unwrap();

        let mut conn_b = db.acquire_for_tenant(tenant_b).await.unwrap();
        let key_b = create_api_key(&mut conn_b, tenant_b, ApiKeyScope::Read)
            .await
            .unwrap();

        let ctx_a = authenticate(&pool, &key_a.plaintext).await.unwrap();
        let ctx_b = authenticate(&pool, &key_b.plaintext).await.unwrap();

        assert_eq!(ctx_a.tenant_id, tenant_a);
        assert_eq!(ctx_b.tenant_id, tenant_b);
    }

    #[test]
    fn scope_hierarchy_allows_higher_scopes_to_satisfy_lower_requirements() {
        assert!(ApiKeyScope::Write.satisfies(ApiKeyScope::Read));
        assert!(ApiKeyScope::Schema.satisfies(ApiKeyScope::Write));
        assert!(!ApiKeyScope::Read.satisfies(ApiKeyScope::Write));
        assert!(!ApiKeyScope::Write.satisfies(ApiKeyScope::Schema));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn require_scope_rejects_insufficient_scope(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();

        let created = create_api_key(&mut conn, tenant_id, ApiKeyScope::Read)
            .await
            .unwrap();
        let ctx = authenticate(&pool, &created.plaintext).await.unwrap();

        let err = require_scope(&ctx, ApiKeyScope::Write).unwrap_err();
        assert!(matches!(err, YorishiroError::ScopeInsufficient { .. }));
    }

    /// `authenticate_api_key`関数が実際に必要であることを裏取りするテスト。
    /// `TenantDb::connect`が本番で行うのと同じ`SET ROLE yorishiro_app`を経た
    /// コネクション（RLSをバイパスできない・`app.current_tenant`も未設定）
    /// でも認証が成立することを検証する。
    #[sqlx::test(migrations = "../../migrations")]
    async fn authenticates_over_a_connection_that_cannot_bypass_rls(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let created = create_api_key(&mut conn, tenant_id, ApiKeyScope::Read)
            .await
            .unwrap();

        let restricted_pool = PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("SET ROLE yorishiro_app")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(pool.connect_options().as_ref().clone())
            .await
            .unwrap();

        let ctx = authenticate(&restricted_pool, &created.plaintext)
            .await
            .unwrap();

        assert_eq!(ctx.tenant_id, tenant_id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn authorize_returns_a_usable_connection_for_a_sufficient_scope(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let created = create_api_key(&mut conn, tenant_id, ApiKeyScope::Write)
            .await
            .unwrap();
        drop(conn);

        let (ctx, mut conn) = authorize(&db, &created.plaintext, ApiKeyScope::Read)
            .await
            .unwrap();

        assert_eq!(ctx.tenant_id, tenant_id);
        // 返されたコネクションはRLSコンテキスト設定済みで、そのテナントの
        // 行を問題なく参照できることを裏取りする。
        let count: (i64,) = sqlx::query_as("SELECT count(*) FROM tenants")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(count.0, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn authorize_rejects_insufficient_scope_without_acquiring_a_connection(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let created = create_api_key(&mut conn, tenant_id, ApiKeyScope::Read)
            .await
            .unwrap();
        drop(conn);

        let err = authorize(&db, &created.plaintext, ApiKeyScope::Write)
            .await
            .unwrap_err();

        assert!(matches!(err, YorishiroError::ScopeInsufficient { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn authorize_rejects_an_unknown_key(pool: PgPool) {
        let db = TenantDb::new(pool);

        let err = authorize(&db, "ysr_does_not_exist_at_all", ApiKeyScope::Read)
            .await
            .unwrap_err();

        assert!(matches!(err, YorishiroError::Unauthenticated));
    }
}
