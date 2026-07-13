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
use yorishiro_core::auth::ApiKeyScope;
use yorishiro_core::entities;

use super::{AuthzOutcome, YorishiroMcpServer, authorize, err_to_tool_result, ok_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateEntityArgs {
    /// Name of the schema this entity conforms to. The tenant's current active version is used.
    pub schema_name: String,
    /// entity_type name declared in the schema.
    pub entity_type: String,
    /// Entity body (JSON object) conforming to the schema's `fields` definition.
    pub data: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetEntityArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateEntityArgs {
    pub id: Uuid,
    /// Replacement entity body. Validated against the schema version in effect when the entity was created.
    pub data: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteEntityArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListEntitiesArgs {
    pub entity_type: Option<String>,
    /// Maximum number of results (defaults to 50 if omitted).
    pub limit: Option<i64>,
    /// Number of records to skip (defaults to 0 if omitted).
    pub offset: Option<i64>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_entities)]
impl YorishiroMcpServer {
    #[tool(description = "Create a new entity (requires write scope)")]
    pub async fn create_entity(
        &self,
        Parameters(args): Parameters<CreateEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Write).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let input = entities::CreateEntityInput {
            schema_name: args.schema_name,
            entity_type: args.entity_type,
            data: args.data,
        };

        let tenant_id = authorized.ctx.tenant_id;
        match entities::create(authorized.conn(), tenant_id, input).await {
            Ok(record) => {
                self.state.spawn_embedding_sync(tenant_id, record.clone());
                ok_json(record)
            }
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "Get a single entity by ID (requires read scope)")]
    pub async fn get_entity(
        &self,
        Parameters(args): Parameters<GetEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match entities::get(authorized.conn(), tenant_id, args.id).await {
            Ok(record) => ok_json(record),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "Replace the data of an existing entity (requires write scope)")]
    pub async fn update_entity(
        &self,
        Parameters(args): Parameters<UpdateEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Write).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match entities::update(authorized.conn(), tenant_id, args.id, args.data).await {
            Ok(record) => {
                self.state.spawn_embedding_sync(tenant_id, record.clone());
                ok_json(record)
            }
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "Delete an entity (requires write scope)")]
    pub async fn delete_entity(
        &self,
        Parameters(args): Parameters<DeleteEntityArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Write).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match entities::delete(authorized.conn(), tenant_id, args.id).await {
            Ok(()) => ok_json(serde_json::json!({ "deleted": true })),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(description = "List entities (requires read scope)")]
    pub async fn list_entities(
        &self,
        Parameters(args): Parameters<ListEntitiesArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let default = entities::ListEntitiesQuery::default();
        let query = entities::ListEntitiesQuery {
            entity_type: args.entity_type,
            limit: args.limit.unwrap_or(default.limit),
            offset: args.offset.unwrap_or(default.offset),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match entities::list(authorized.conn(), tenant_id, query).await {
            Ok(records) => ok_json(records),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }
}
