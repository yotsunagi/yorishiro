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

/// Shared by both the `AuthContext` and `Authorized<R>` extractors.
fn extract_bearer_key(parts: &Parts) -> Result<&str, ApiError> {
    parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(ApiError(YorishiroError::Unauthenticated))
}

/// The sole entry point for authenticated requests. Requiring this type as a handler
/// argument is itself a declaration that "this route requires authentication," which
/// prevents forgetting the auth check at compile time (a bare `Extension<T>` would
/// silently work even if the check were skipped).
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

        // Updating last_used_at is best-effort and doesn't affect the auth result;
        // the request proceeds even if it fails.
        match db
            .acquire_for_workspace(ctx.tenant_id, ctx.workspace_id)
            .await
        {
            Ok(mut conn) => {
                if let Err(err) =
                    auth::touch_last_used(&mut conn, ctx.workspace_id, ctx.api_key_id).await
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

/// Marker for declaring an endpoint's required API key scope at the type level.
/// Used as the type parameter of `Authorized<R>`.
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

/// An extractor that authenticates, verifies the required scope, and acquires a connection
/// with the RLS context already set, all in one step. `R` (`ReadScope`/`WriteScope`/
/// `SchemaScope`) doubles as the scope requirement declared in the handler signature. As
/// with the MCP adapter's `Authorized`, there is no way to obtain a `&mut PgConnection`
/// except through this type, which structurally prevents forgetting the scope check (the
/// core logic lives in `yorishiro_core::auth::authorize`, shared by both adapters).
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

/// A connection-less version of `Authorized<R>`: it only authenticates and verifies `R`'s
/// scope, without acquiring a DB connection. Handlers that do slow work (e.g. generating an
/// embedding) before touching the database — search, for instance — would otherwise hold a
/// pool connection idle through `Authorized<R>`; use this instead and call
/// `TenantDb::acquire_for_workspace` afterward.
pub struct Verified<R> {
    pub ctx: auth::AuthContext,
    _scope: PhantomData<R>,
}

impl<S, R> FromRequestParts<S> for Verified<R>
where
    TenantDb: FromRef<S>,
    S: Send + Sync,
    R: RequiredScope,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let presented_key = extract_bearer_key(parts)?;

        let db = TenantDb::from_ref(state);
        let ctx = auth::authorize_scope(&db, presented_key, R::SCOPE).await?;

        Ok(Verified {
            ctx,
            _scope: PhantomData,
        })
    }
}
