use std::marker::PhantomData;

use axum::extract::{FromRef, FromRequestParts};
use axum::http::header;
use axum::http::request::Parts;
use sqlx::PgConnection;
use sqlx::pool::PoolConnection;
use yorishiro_core::YorishiroError;
use yorishiro_core::auth;
use yorishiro_core::auth::ApiKeyScope;
use yorishiro_core::db::TenantDb;

use crate::error::ApiError;

/// `Authorization: Bearer <key>`ヘッダから素のAPIキー文字列を取り出す。
/// `AuthContext`/`Authorized<R>`の両extractorで共有する。
fn extract_bearer_key(parts: &Parts) -> Result<&str, ApiError> {
    parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(ApiError(YorishiroError::Unauthenticated))
}

/// 認証済みリクエストの唯一の入口。ハンドラの引数にこの型を要求すること自体が
/// 「このルートは認証必須」という表明になり、認証チェックの付け忘れを
/// コンパイル時に防ぐ（`Extension<T>`を素で使うと付け忘れても黙って通ってしまう）。
pub struct AuthContext(pub auth::AuthContext);

impl<S> FromRequestParts<S> for AuthContext
where
    TenantDb: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let presented_key = extract_bearer_key(parts)?;

        let db = TenantDb::from_ref(state);
        let ctx = auth::authenticate(db.pool(), presented_key).await?;

        // last_used_atの更新は認証結果には影響しないベストエフォート処理。
        // 失敗してもリクエストは継続させる。
        match db.acquire_for_tenant(ctx.tenant_id).await {
            Ok(mut conn) => {
                if let Err(err) =
                    auth::touch_last_used(&mut conn, ctx.tenant_id, ctx.api_key_id).await
                {
                    tracing::warn!(error = %err, "failed to update api key last_used_at");
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to acquire connection to touch last_used_at");
            }
        }

        Ok(AuthContext(ctx))
    }
}

/// エンドポイントが要求するAPIキーscopeを型として宣言するためのマーカー。
/// `Authorized<R>`の型パラメータに使う。
pub trait RequiredScope {
    const SCOPE: ApiKeyScope;
}

pub struct ReadScope;
impl RequiredScope for ReadScope {
    const SCOPE: ApiKeyScope = ApiKeyScope::Read;
}

pub struct WriteScope;
impl RequiredScope for WriteScope {
    const SCOPE: ApiKeyScope = ApiKeyScope::Write;
}

pub struct SchemaScope;
impl RequiredScope for SchemaScope {
    const SCOPE: ApiKeyScope = ApiKeyScope::Schema;
}

/// 認証・要求scopeの検証・RLSコンテキスト設定済みコネクションの取得を一括で行う
/// extractor。`R`（`ReadScope`/`WriteScope`/`SchemaScope`）がそのままハンドラの
/// シグネチャ上でscope要件の宣言になる。MCPアダプタの`Authorized`と同じく、この型を
/// 経由しない限り`&mut PgConnection`を得る手段がない構造にすることで、スコープ
/// チェックの呼び忘れを構造的に防ぐ（コアロジックは`yorishiro_core::auth::authorize`
/// として両アダプタで共有している）。
pub struct Authorized<R> {
    pub ctx: auth::AuthContext,
    conn: PoolConnection<sqlx::Postgres>,
    _scope: PhantomData<R>,
}

impl<R> Authorized<R> {
    pub fn conn(&mut self) -> &mut PgConnection {
        &mut self.conn
    }
}

impl<S, R> FromRequestParts<S> for Authorized<R>
where
    TenantDb: FromRef<S>,
    S: Send + Sync,
    R: RequiredScope,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let presented_key = extract_bearer_key(parts)?;

        let db = TenantDb::from_ref(state);
        let (ctx, conn) = auth::authorize(&db, presented_key, R::SCOPE).await?;

        Ok(Authorized {
            ctx,
            conn,
            _scope: PhantomData,
        })
    }
}
