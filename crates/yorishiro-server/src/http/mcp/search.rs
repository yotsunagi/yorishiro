use http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use yorishiro_core::YorishiroError;
use yorishiro_core::repositories::search;
use yorishiro_core::services::auth::ApiKeyScope;

use super::{YorishiroMcpServer, err_to_tool_result, ok_json, verified};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchEntitiesArgs {
    /// Natural-language query text. Vectorized via the embedding provider and
    /// matched against entities' `x-embed` field by cosine distance. Also used, as-is, for
    /// an auxiliary pg_trgm fuzzy text match against entities that have no embedding.
    pub query_text: String,
    pub entity_type: Option<String>,
    /// JSONB containment filter matched against entity data, e.g. `{"status": "active"}`.
    pub filter: Option<Value>,
    /// Upper bound on the number of results returned (defaults to 10 if omitted).
    pub limit: Option<i64>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_search)]
impl YorishiroMcpServer {
    #[tool(
        description = "Vector similarity search over entities using a natural-language query (requires read scope)"
    )]
    pub async fn search_entities(
        &self,
        Parameters(args): Parameters<SearchEntitiesArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let ctx = verified!(&self.state, &parts, ApiKeyScope::Read);

        let default = search::SearchQuery::default();
        let query = search::SearchQuery {
            entity_type: args.entity_type,
            filter: args.filter,
            limit: args.limit.unwrap_or(default.limit),
        };

        // Embedding generation happens before acquiring a DB connection, for the
        // same reason as the REST adapter: don't hold a pool connection while
        // waiting on the LocalOnnx provider's serialized inference.
        let vector =
            match search::embed_query(self.state.embedding_provider.as_ref(), &args.query_text)
                .await
            {
                Ok(vector) => vector,
                Err(err) => return Ok(err_to_tool_result(err)),
            };

        let workspace_id = ctx.workspace_id;
        let mut conn = match self
            .state
            .tenant_db
            .acquire_for_workspace(ctx.tenant_id, workspace_id)
            .await
        {
            Ok(conn) => conn,
            Err(err) => {
                return Ok(err_to_tool_result(YorishiroError::Internal(err.into())));
            }
        };

        match search::search_by_vector(&mut conn, workspace_id, vector, &args.query_text, query)
            .await
        {
            Ok(hits) => ok_json(hits),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }
}
