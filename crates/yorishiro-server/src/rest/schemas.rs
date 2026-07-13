use axum::Json;
use axum::extract::Path;
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::metaschema::{self, MetaSchemaDefinition, VersioningDiff};
use yorishiro_core::schemas::{self, SchemaRecord, SchemaSummary};

use crate::auth::{Authorized, ReadScope, SchemaScope};
use crate::error::ApiError;

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
    let tenant_id = authorized.ctx.tenant_id;
    let summaries = schemas::list(authorized.conn(), tenant_id).await?;
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
    let tenant_id = authorized.ctx.tenant_id;
    let record = schemas::get_active_schema(authorized.conn(), tenant_id, &name).await?;
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
    let tenant_id = authorized.ctx.tenant_id;
    let record = schemas::get_by_id(authorized.conn(), tenant_id, schema_id).await?;
    Ok(Json(record))
}

#[utoipa::path(
    post,
    path = "/api/schemas",
    request_body = MetaSchemaDefinition,
    responses(
        (status = 201, description = "Schema newly registered, or added as a new version", body = CreateSchemaResponse),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 409, description = "Version conflict due to concurrent creation", body = crate::error::ApiErrorBody),
        (status = 422, description = "The schema definition itself is invalid", body = crate::error::ApiErrorBody),
    ),
    tag = "schemas",
)]
pub async fn create_schema(
    mut authorized: Authorized<SchemaScope>,
    Json(definition): Json<MetaSchemaDefinition>,
) -> Result<(axum::http::StatusCode, Json<CreateSchemaResponse>), ApiError> {
    let tenant_id = authorized.ctx.tenant_id;
    let (schema, diff) = schemas::create_schema(authorized.conn(), tenant_id, definition).await?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(CreateSchemaResponse { schema, diff }),
    ))
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
    let tenant_id = authorized.ctx.tenant_id;
    let record = schemas::get_active_schema(authorized.conn(), tenant_id, &name).await?;

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
