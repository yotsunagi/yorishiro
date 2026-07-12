mod entities;
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
use yorishiro_core::auth::{self, ApiKeyScope, AuthContext};

use crate::state::AppState;

/// 各ドメインの`#[tool_router]`実装から合成される、Yorishiro MCPサーバー本体。
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
            "Yorishiro（依り代）はユーザー定義スキーマ・マルチテナントのナレッジストアです。\
             各ツール呼び出しは`Authorization: Bearer <api-key>`ヘッダーによる認証が必須で、\
             APIキーのscope（read/write/schema、上位が下位を包含）に応じて呼び出せるツールが制限されます。",
        )
    }
}

/// 認証・スコープ検証を通過した呼び出しが手にする、RLSコンテキスト設定済みの
/// コネクションと認証情報。
pub(super) struct Authorized {
    pub(super) ctx: AuthContext,
    conn: PoolConnection<sqlx::Postgres>,
}

impl Authorized {
    pub(super) fn conn(&mut self) -> &mut PgConnection {
        &mut self.conn
    }
}

/// 認証・スコープ不足のいずれでもない失敗をこの型が表す機会はない
/// （どちらも呼び出し側でハンドリングされる）ため、`authorize`の戻り値は
/// 「プロトコルレベルの失敗（`Err`）」と「スコープ不足という業務結果（`Ok`のバリアント）」
/// の2つに分ける。前者はAPIキー欠落・無効というエージェント側の再試行が無意味な
/// 失敗、後者はエージェントが状況を理解して行動を変えられる情報だからである。
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

/// 全ツールハンドラの唯一の入口。この関数を経由しない限り`&mut PgConnection`を
/// 得る手段がない構造にすることで、スコープチェックの呼び忘れを構造的に防ぐ。
/// 認証・認可のコアロジック自体は`yorishiro_core::auth::authorize`にREST側と
/// 共有されており、ここではその結果をMCPプロトコルの2種類の失敗表現
/// （プロトコルレベルの`ErrorData` / tool結果レベルの`CallToolResult`）へ
/// 振り分けるだけにする。
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

/// `authorize`のコネクション非保持版の結果。`AuthzOutcome`と同じ2分法だが、
/// 成功時にコネクションを含まない。
pub(super) enum ScopeOutcome {
    Verified(AuthContext),
    ScopeDenied(CallToolResult),
}

/// `authorize`のコネクション非保持版。埋め込み生成のような時間のかかる処理を挟む
/// ツール（検索）で、その間プール接続を占有しないために使う。処理後のDBアクセスは
/// `state.tenant_db.acquire_for_tenant`で自前取得すること。
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

/// ビジネスロジックのエラーをtool呼び出し結果（`is_error: true`）へ変換する。
/// `Internal`は詳細をログへ残し、クライアントには汎用メッセージのみ返す
/// （RESTアダプタの`ApiError`と同じ方針）。
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
