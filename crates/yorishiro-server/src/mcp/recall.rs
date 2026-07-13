use http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;
use yorishiro_core::auth::ApiKeyScope;
use yorishiro_core::recall::{self, DEFAULT_RECALL_LIMIT};

use super::{AuthzOutcome, YorishiroMcpServer, authorize, err_to_tool_result, ok_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecallContextArgs {
    pub entity_id: Uuid,
    /// Maximum number of relations/neighbors to include (defaults to 20 if omitted).
    pub limit: Option<i64>,
    /// When true, neighbor entities include every field instead of only `x-embed` fields
    /// (defaults to false).
    pub full: Option<bool>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_recall)]
impl YorishiroMcpServer {
    #[tool(
        description = "Fetch an entity's full body together with its relations and connected neighbors in one call (requires read scope)"
    )]
    pub async fn recall_context(
        &self,
        Parameters(args): Parameters<RecallContextArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let workspace_id = authorized.ctx.workspace_id;
        let limit = args.limit.unwrap_or(DEFAULT_RECALL_LIMIT);
        let full = args.full.unwrap_or(false);
        match recall::recall_context(authorized.conn(), workspace_id, args.entity_id, limit, full)
            .await
        {
            Ok(context) => ok_json(context),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }
}
