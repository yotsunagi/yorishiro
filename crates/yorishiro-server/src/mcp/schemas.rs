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
use yorishiro_core::YorishiroError;
use yorishiro_core::auth::ApiKeyScope;
use yorishiro_core::metaschema::{self, MetaSchemaDefinition};
use yorishiro_core::schemas;

use super::{AuthzOutcome, YorishiroMcpServer, authorize, err_to_tool_result, ok_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetActiveSchemaArgs {
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSchemaByIdArgs {
    pub schema_id: Uuid,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateSchemaArgs {
    /// JSON object conforming to `MetaSchemaDefinition`
    /// (name/description/entity_types/relation_types). If a schema with the same
    /// name already exists, whether the change is breaking or non-breaking is
    /// detected automatically and it is registered as a new version.
    pub definition: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetEntityTypeJsonSchemaArgs {
    /// Name of the active schema.
    pub schema_name: String,
    /// entity_type name within that schema.
    pub entity_type: String,
}

#[tool_router(vis = "pub(crate)", router = tool_router_schemas)]
impl YorishiroMcpServer {
    #[tool(
        description = "List summaries of all schemas registered for the tenant (all versions, including archived). Use this to discover what schemas exist (requires read scope)"
    )]
    pub async fn list_schemas(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match schemas::list(authorized.conn(), tenant_id).await {
            Ok(summaries) => ok_json(summaries),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(
        description = "Get the currently active schema definition by name (requires read scope)"
    )]
    pub async fn get_active_schema(
        &self,
        Parameters(args): Parameters<GetActiveSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match schemas::get_active_schema(authorized.conn(), tenant_id, &args.name).await {
            Ok(record) => ok_json(record),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(
        description = "Get a specific version of a schema definition by ID (requires read scope)"
    )]
    pub async fn get_schema_by_id(
        &self,
        Parameters(args): Parameters<GetSchemaByIdArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        match schemas::get_by_id(authorized.conn(), tenant_id, args.schema_id).await {
            Ok(record) => ok_json(record),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(
        description = "Register a new schema, or add a new version to an existing schema \
                           (requires schema scope)"
    )]
    pub async fn create_schema(
        &self,
        Parameters(args): Parameters<CreateSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Schema).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let definition: MetaSchemaDefinition = match serde_json::from_value(args.definition) {
            Ok(definition) => definition,
            Err(err) => {
                return Ok(err_to_tool_result(YorishiroError::ValidationFailed {
                    message: format!("invalid schema definition: {err}"),
                    details: vec![],
                    hint: "Check the structure of MetaSchemaDefinition \
                           (name/description/entity_types/relation_types)"
                        .into(),
                }));
            }
        };

        let tenant_id = authorized.ctx.tenant_id;
        match schemas::create_schema(authorized.conn(), tenant_id, definition).await {
            Ok((record, diff)) => ok_json(serde_json::json!({
                "schema": record,
                "diff": diff,
            })),
            Err(err) => Ok(err_to_tool_result(err)),
        }
    }

    #[tool(
        description = "Get a specific entity_type within the active schema as a JSON Schema \
                           (requires read scope). Use this to let an agent learn field types, \
                           required fields, enums, etc. ahead of time."
    )]
    pub async fn get_entity_type_json_schema(
        &self,
        Parameters(args): Parameters<GetEntityTypeJsonSchemaArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut authorized = match authorize(&self.state, &parts, ApiKeyScope::Read).await? {
            AuthzOutcome::Authorized(a) => a,
            AuthzOutcome::ScopeDenied(result) => return Ok(result),
        };

        let tenant_id = authorized.ctx.tenant_id;
        let record =
            match schemas::get_active_schema(authorized.conn(), tenant_id, &args.schema_name).await
            {
                Ok(record) => record,
                Err(err) => return Ok(err_to_tool_result(err)),
            };

        match record.definition.entity_types.get(&args.entity_type) {
            Some(entity_type_def) => {
                ok_json(metaschema::entity_type_to_json_schema(entity_type_def))
            }
            None => Ok(err_to_tool_result(YorishiroError::NotFound {
                message: format!(
                    "entity_type '{}' not found in schema '{}'",
                    args.entity_type, args.schema_name
                ),
            })),
        }
    }
}
