use http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use yorishiro_core::auth::ApiKeyScope;
use yorishiro_core::search;

use super::{AuthzOutcome, YorishiroMcpServer, authorize, err_to_tool_result, ok_json};

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
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let default = search::SearchQuery::default();
        let query = search::SearchQuery {
            entity_type: args.entity_type,
            limit: args.limit.unwrap_or(default.limit),
        };

        let tenant_id = authorized.ctx.tenant_id;
        let result = search::search_by_text(
            authorized.conn(),
            tenant_id,
            self.state.embedding_provider.as_ref(),
            &args.query_text,
            query,
        )
        .await;

        match result {
            Ok(hits) => ok_json(hits),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }
}
