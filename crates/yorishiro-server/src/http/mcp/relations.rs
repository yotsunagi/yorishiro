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
use uuid::Uuid;
use yorishiro_core::repositories::relations;
use yorishiro_core::services::auth::ApiKeyScope;

use super::{AuthzOutcome, YorishiroMcpServer, authorize, err_to_tool_result, ok_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateRelationArgs {
    pub source_id: Uuid,
    pub target_id: Uuid,
    /// relation_type name declared in the schema's `relation_types` definition.
    pub relation_type: String,
    /// Arbitrary properties attached to the relation (JSON object, defaults to an empty object if omitted).
    pub properties: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRelationArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteRelationArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRelationsArgs {
    pub source_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub relation_type: Option<String>,
    /// Maximum number of results (defaults to 50 if omitted).
    pub limit: Option<i64>,
    /// Number of records to skip (defaults to 0 if omitted).
    pub offset: Option<i64>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_relations)]
impl YorishiroMcpServer {
    #[tool(
        description = "Create a relation between two entities (requires write scope). \
                           No update operation is provided; to change a relation, delete it and recreate it."
    )]
    pub async fn create_relation(
        &self,
        Parameters(args): Parameters<CreateRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Write).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let input = relations::CreateRelationInput {
            source_id: args.source_id,
            target_id: args.target_id,
            relation_type: args.relation_type,
            properties: args.properties.unwrap_or_else(|| serde_json::json!({})),
        };

        let workspace_id = authorized.ctx.workspace_id;
        match relations::create(authorized.conn(), workspace_id, input).await {
            Ok(record) => ok_json(record),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "Get a single relation by ID (requires read scope)")]
    pub async fn get_relation(
        &self,
        Parameters(args): Parameters<GetRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let workspace_id = authorized.ctx.workspace_id;
        match relations::get(authorized.conn(), workspace_id, args.id).await {
            Ok(record) => ok_json(record),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "Delete a relation (requires write scope)")]
    pub async fn delete_relation(
        &self,
        Parameters(args): Parameters<DeleteRelationArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Write).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let workspace_id = authorized.ctx.workspace_id;
        match relations::delete(authorized.conn(), workspace_id, args.id).await {
            Ok(()) => ok_json(serde_json::json!({ "deleted": true })),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "List relations (requires read scope)")]
    pub async fn list_relations(
        &self,
        Parameters(args): Parameters<ListRelationsArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let default = relations::ListRelationsQuery::default();
        let query = relations::ListRelationsQuery {
            source_id: args.source_id,
            target_id: args.target_id,
            relation_type: args.relation_type,
            limit: args.limit.unwrap_or(default.limit),
            offset: args.offset.unwrap_or(default.offset),
        };

        let workspace_id = authorized.ctx.workspace_id;
        match relations::list(authorized.conn(), workspace_id, query).await {
            Ok(records) => ok_json(records),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }
}
