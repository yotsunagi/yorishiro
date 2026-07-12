use http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use yorishiro_core::YorishiroError;
use yorishiro_core::auth::ApiKeyScope;
use yorishiro_core::search;

use super::{ScopeOutcome, YorishiroMcpServer, authorize_scope_only, err_to_tool_result, ok_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchEntitiesArgs {
    /// 検索したい自然文のクエリ。embeddingプロバイダでベクトル化し、
    /// `x-embed`フィールドのベクトルとのコサイン距離で近いエンティティを返す。
    pub query_text: String,
    /// 指定した場合、このentity_typeのエンティティのみを検索対象にする。
    pub entity_type: Option<String>,
    /// 返す件数の上限（省略時は10）。
    pub limit: Option<i64>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_search)]
impl YorishiroMcpServer {
    #[tool(description = "自然文クエリでエンティティをベクトル類似検索する（read scope必須）")]
    pub async fn search_entities(
        &self,
        Parameters(args): Parameters<SearchEntitiesArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let ctx = match authorize_scope_only(&self.state, &parts, ApiKeyScope::Read).await? {
            ScopeOutcome::Verified(ctx) => ctx,
            ScopeOutcome::ScopeDenied(result) => return Ok(result),
        };

        let default = search::SearchQuery::default();
        let query = search::SearchQuery {
            entity_type: args.entity_type,
            limit: args.limit.unwrap_or(default.limit),
        };

        // 埋め込み生成はDBコネクション取得より先に行う（RESTアダプタと同じ理由:
        // LocalOnnxプロバイダの直列化待ちの間、プール接続を占有しない）。
        let vector =
            match search::embed_query(self.state.embedding_provider.as_ref(), &args.query_text)
                .await
            {
                Ok(vector) => vector,
                Err(err) => return Ok(err_to_tool_result(err)),
            };

        let tenant_id = ctx.tenant_id;
        let mut conn = match self.state.tenant_db.acquire_for_tenant(tenant_id).await {
            Ok(conn) => conn,
            Err(err) => {
                return Ok(err_to_tool_result(YorishiroError::Internal(err.into())));
            }
        };

        match search::search_by_vector(&mut conn, tenant_id, vector, query).await {
            Ok(hits) => ok_json(hits),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }
}
