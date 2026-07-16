mod entities;
mod recall;
mod relations;
mod schemas;
mod search;

use http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool_handler};
use sqlx::PgConnection;
use sqlx::pool::PoolConnection;
use yorishiro_core::YorishiroError;
use yorishiro_core::services::auth::{self, ApiKeyScope, AuthContext};

use crate::state::AppState;

/// Yorishiro MCP server, assembled from each domain's `#[tool_router]` implementation.
#[derive(Clone)]
pub struct YorishiroMcpServer {
    state: AppState,
    tool_router: ToolRouter<Self>,
}

impl YorishiroMcpServer {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router_entities()
                + Self::tool_router_recall()
                + Self::tool_router_relations()
                + Self::tool_router_search()
                + Self::tool_router_schemas(),
        }
    }
}

#[tool_handler(router = self.tool_router.clone())]
impl ServerHandler for YorishiroMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Yorishiro is a multi-tenant knowledge store with user-defined schemas. \
             Every tool call requires authentication via an `Authorization: Bearer <api-key>` \
             header, and the tools available depend on the API key's scope \
             (read/write/schema, where higher scopes include the permissions of lower ones).",
        )
    }
}

/// Auth context plus a connection with RLS already configured, held by calls
/// that passed authentication and scope checks.
pub(super) struct Authorized {
    pub(super) ctx: AuthContext,
    conn: PoolConnection<sqlx::Postgres>,
}

impl Authorized {
    pub(super) fn conn(&mut self) -> &mut PgConnection {
        &mut self.conn
    }
}

/// `authorize` splits its outcome into two kinds rather than a single failure case:
/// a protocol-level failure (`Err`) and a scope-insufficient business outcome
/// (`Ok` variant). The former is a dead end an agent can't usefully retry
/// (missing/invalid API key); the latter is information an agent can act on.
pub(super) enum AuthzOutcome {
    Authorized(Authorized),
    ScopeDenied(CallToolResult),
}

fn extract_bearer_key(parts: &Parts) -> Result<&str, ErrorData> {
    parts
        .headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| {
            ErrorData::invalid_request("missing or malformed Authorization header", None)
        })
}

/// The sole entry point for every tool handler. Because there is no other way to
/// obtain a `&mut PgConnection`, forgetting the scope check is structurally
/// impossible. The actual auth/authz logic is shared with the REST adapter via
/// `yorishiro_core::services::auth::authorize`; this just routes its result into the MCP
/// protocol's two failure shapes (`ErrorData` at the protocol level,
/// `CallToolResult` at the tool-result level).
pub(super) async fn authorize(
    state: &AppState,
    parts: &Parts,
    required: ApiKeyScope,
) -> Result<AuthzOutcome, ErrorData> {
    let presented_key = extract_bearer_key(parts)?;

    match auth::authorize(&state.tenant_db, presented_key, required).await {
        Ok((ctx, conn)) => Ok(AuthzOutcome::Authorized(Authorized { ctx, conn })),
        Err(err @ YorishiroError::ScopeInsufficient { .. }) => {
            Ok(AuthzOutcome::ScopeDenied(err_to_tool_result(err)))
        }
        Err(YorishiroError::Unauthenticated) => {
            Err(ErrorData::invalid_request("authentication failed", None))
        }
        Err(err) => Err(ErrorData::internal_error(err.to_string(), None)),
    }
}

/// The connection-less counterpart to `authorize`'s result. Same two-way split as
/// `AuthzOutcome`, but the success variant carries no connection.
pub(super) enum ScopeOutcome {
    Verified(AuthContext),
    ScopeDenied(CallToolResult),
}

/// Connection-less version of `authorize`, for tools (search) that run a slow step
/// such as embedding generation in between, so the pool connection isn't held idle
/// during it. Acquire a connection afterward via `state.tenant_db.acquire_for_workspace`.
pub(super) async fn authorize_scope_only(
    state: &AppState,
    parts: &Parts,
    required: ApiKeyScope,
) -> Result<ScopeOutcome, ErrorData> {
    let presented_key = extract_bearer_key(parts)?;

    match auth::authorize_scope(&state.tenant_db, presented_key, required).await {
        Ok(ctx) => Ok(ScopeOutcome::Verified(ctx)),
        Err(err @ YorishiroError::ScopeInsufficient { .. }) => {
            Ok(ScopeOutcome::ScopeDenied(err_to_tool_result(err)))
        }
        Err(YorishiroError::Unauthenticated) => {
            Err(ErrorData::invalid_request("authentication failed", None))
        }
        Err(err) => Err(ErrorData::internal_error(err.to_string(), None)),
    }
}

/// Converts a business-logic error into a tool call result (`is_error: true`).
/// `Internal` errors are logged with detail but only a generic message reaches
/// the client, matching the REST adapter's `ApiError` policy.
pub(super) fn err_to_tool_result(err: YorishiroError) -> CallToolResult {
    match err {
        YorishiroError::Internal(err) => {
            tracing::error!(error = %err, "internal error in mcp tool handler");
            CallToolResult::error(vec![ContentBlock::text("internal server error")])
        }
        other => CallToolResult::error(vec![ContentBlock::text(other.to_string())]),
    }
}

pub(super) fn ok_json(value: impl serde::Serialize) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string(&value)
        .map_err(|err| ErrorData::internal_error(err.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Authenticates the caller and verifies scope, acquiring an RLS-scoped DB connection. Expands
/// to the `Authorized` value on success; on a scope-denied outcome it early-returns the tool
/// result. A macro rather than a function because it early-returns from the enclosing handler
/// (which must return `Result<CallToolResult, ErrorData>`).
macro_rules! authorized {
    ($state:expr, $parts:expr, $scope:expr) => {
        match $crate::http::mcp::authorize($state, $parts, $scope).await? {
            $crate::http::mcp::AuthzOutcome::Authorized(authorized) => authorized,
            $crate::http::mcp::AuthzOutcome::ScopeDenied(result) => {
                return ::core::result::Result::Ok(result);
            }
        }
    };
}
pub(crate) use authorized;

/// Connection-less counterpart to `authorized!`, for handlers (search) that do slow work before
/// touching the DB. Expands to the `AuthContext` on success, else early-returns the scope-denied
/// result.
macro_rules! verified {
    ($state:expr, $parts:expr, $scope:expr) => {
        match $crate::http::mcp::authorize_scope_only($state, $parts, $scope).await? {
            $crate::http::mcp::ScopeOutcome::Verified(ctx) => ctx,
            $crate::http::mcp::ScopeOutcome::ScopeDenied(result) => {
                return ::core::result::Result::Ok(result);
            }
        }
    };
}
pub(crate) use verified;
