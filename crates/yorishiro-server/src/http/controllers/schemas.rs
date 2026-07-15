use axum::Json;
use axum::extract::Path;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::metaschema::{self, MetaSchemaDefinition, VersioningDiff};
use yorishiro_core::repositories::schemas::{self, SchemaRecord, SchemaSummary};
use yorishiro_core::templates::{self, TemplateSummary};

use crate::error::ApiError;
use crate::http::middleware::auth::{Authorized, ReadScope, SchemaScope};

#[derive(Debug, Serialize, ToSchema)]
pub struct CreateSchemaResponse {
    pub schema: SchemaRecord,
    pub diff: VersioningDiff,
}

#[utoipa::path(
    get,
    path = "/api/schemas",
    responses(
        (status = 200, description = "Summary list of all schemas for the tenant (all versions, including archived)", body = Vec<SchemaSummary>),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
    ),
    tag = "schemas",
)]
pub async fn list_schemas(
    mut authorized: Authorized<ReadScope>,
) -> Result<Json<Vec<SchemaSummary>>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let summaries = schemas::list(authorized.conn(), workspace_id).await?;
    Ok(Json(summaries))
}

#[utoipa::path(
    get,
    path = "/api/schemas/active/{name}",
    params(("name" = String, Path, description = "Schema name")),
    responses(
        (status = 200, description = "Currently active schema definition retrieved", body = SchemaRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "No active schema exists with the given name", body = crate::error::ApiErrorBody),
    ),
    tag = "schemas",
)]
pub async fn get_active_schema(
    mut authorized: Authorized<ReadScope>,
    Path(name): Path<String>,
) -> Result<Json<SchemaRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record = schemas::get_active_schema(authorized.conn(), workspace_id, &name).await?;
    Ok(Json(record))
}

#[utoipa::path(
    get,
    path = "/api/schemas/{schema_id}",
    params(("schema_id" = Uuid, Path, description = "Schema ID (specific version)")),
    responses(
        (status = 200, description = "Schema definition for the specified version retrieved", body = SchemaRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "The specified schema does not exist", body = crate::error::ApiErrorBody),
    ),
    tag = "schemas",
)]
pub async fn get_schema_by_id(
    mut authorized: Authorized<ReadScope>,
    Path(schema_id): Path<Uuid>,
) -> Result<Json<SchemaRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record = schemas::get_by_id(authorized.conn(), workspace_id, schema_id).await?;
    Ok(Json(record))
}

/// Either an inline schema definition, or a reference to a built-in template's ID (see
/// `GET /api/templates`). Untagged so existing clients posting a flat `MetaSchemaDefinition`
/// body keep working unchanged.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum CreateSchemaRequest {
    Definition(MetaSchemaDefinition),
    Template { template_id: String },
}

#[utoipa::path(
    post,
    path = "/api/schemas",
    request_body = CreateSchemaRequest,
    responses(
        (status = 201, description = "Schema newly registered, or added as a new version", body = CreateSchemaResponse),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "The specified template_id does not exist", body = crate::error::ApiErrorBody),
        (status = 409, description = "Version conflict due to concurrent creation", body = crate::error::ApiErrorBody),
        (status = 422, description = "The schema definition itself is invalid", body = crate::error::ApiErrorBody),
    ),
    tag = "schemas",
)]
pub async fn create_schema(
    mut authorized: Authorized<SchemaScope>,
    Json(body): Json<CreateSchemaRequest>,
) -> Result<(axum::http::StatusCode, Json<CreateSchemaResponse>), ApiError> {
    let definition = match body {
        CreateSchemaRequest::Definition(definition) => definition,
        CreateSchemaRequest::Template { template_id } => templates::get_template(&template_id)?,
    };

    let workspace_id = authorized.ctx.workspace_id;
    let (schema, diff) =
        schemas::create_schema(authorized.conn(), workspace_id, definition).await?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(CreateSchemaResponse { schema, diff }),
    ))
}

#[utoipa::path(
    get,
    path = "/api/templates",
    responses(
        (status = 200, description = "Built-in schema templates available for schema creation", body = Vec<TemplateSummary>),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
    ),
    tag = "schemas",
)]
pub async fn list_templates(
    _authorized: Authorized<ReadScope>,
) -> Result<Json<Vec<TemplateSummary>>, ApiError> {
    Ok(Json(templates::list_templates()))
}

#[utoipa::path(
    get,
    path = "/api/schemas/active/{name}/entity-types/{entity_type}/json-schema",
    params(
        ("name" = String, Path, description = "Name of the active schema"),
        ("entity_type" = String, Path, description = "Name of the entity_type within the schema"),
    ),
    responses(
        (status = 200, description = "Result of projecting the entity_type as a JSON Schema", body = Value),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "The specified schema or entity_type does not exist", body = crate::error::ApiErrorBody),
    ),
    tag = "schemas",
)]
pub async fn get_entity_type_json_schema(
    mut authorized: Authorized<ReadScope>,
    Path((name, entity_type)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record = schemas::get_active_schema(authorized.conn(), workspace_id, &name).await?;

    let entity_type_def = record
        .definition
        .entity_types
        .get(&entity_type)
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!("entity_type '{entity_type}' not found in schema '{name}'"),
        })?;

    Ok(Json(metaschema::entity_type_to_json_schema(
        entity_type_def,
    )))
}
