use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use yorishiro_core::entities::{self, EntityRecord};
use yorishiro_core::recall::{self, RecallContext};

use crate::error::ApiError;
use crate::http::middleware::auth::{Authorized, ReadScope, WriteScope};
use crate::state::AppState;

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateEntityRequest {
    pub schema_name: String,
    pub entity_type: String,
    #[schema(value_type = Object)]
    pub data: Value,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateEntityRequest {
    #[schema(value_type = Object)]
    pub data: Value,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListEntitiesParams {
    pub entity_type: Option<String>,
    /// JSON-encoded containment filter, e.g. `{"status":"active"}`.
    pub filter: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[utoipa::path(
    post,
    path = "/api/entities",
    request_body = CreateEntityRequest,
    responses(
        (status = 201, description = "Entity created", body = EntityRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "The specified schema or entity_type does not exist", body = crate::error::ApiErrorBody),
        (status = 422, description = "data does not conform to the schema", body = crate::error::ApiErrorBody),
    ),
    tag = "entities",
)]
pub async fn create_entity(
    State(state): State<AppState>,
    mut authorized: Authorized<WriteScope>,
    Json(body): Json<CreateEntityRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let input = entities::CreateEntityInput {
        schema_name: body.schema_name,
        entity_type: body.entity_type,
        data: body.data,
    };
    let created_by = authorized.ctx.user_id;
    let record = entities::create(authorized.conn(), workspace_id, input, created_by).await?;
    state.spawn_embedding_sync(authorized.ctx.tenant_id, workspace_id, record.clone());
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    get,
    path = "/api/entities/{id}",
    params(("id" = Uuid, Path, description = "Entity ID")),
    responses(
        (status = 200, description = "Entity retrieved", body = EntityRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "Entity not found", body = crate::error::ApiErrorBody),
    ),
    tag = "entities",
)]
pub async fn get_entity(
    mut authorized: Authorized<ReadScope>,
    Path(id): Path<Uuid>,
) -> Result<Json<EntityRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record = entities::get(authorized.conn(), workspace_id, id).await?;
    Ok(Json(record))
}

#[utoipa::path(
    put,
    path = "/api/entities/{id}",
    params(("id" = Uuid, Path, description = "Entity ID")),
    request_body = UpdateEntityRequest,
    responses(
        (status = 200, description = "Entity updated", body = EntityRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "Entity not found", body = crate::error::ApiErrorBody),
        (status = 422, description = "data does not conform to the schema", body = crate::error::ApiErrorBody),
    ),
    tag = "entities",
)]
pub async fn update_entity(
    State(state): State<AppState>,
    mut authorized: Authorized<WriteScope>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateEntityRequest>,
) -> Result<Json<EntityRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let updated_by = authorized.ctx.user_id;
    let record =
        entities::update(authorized.conn(), workspace_id, id, body.data, updated_by).await?;
    state.spawn_embedding_sync(authorized.ctx.tenant_id, workspace_id, record.clone());
    Ok(Json(record))
}

#[utoipa::path(
    delete,
    path = "/api/entities/{id}",
    params(("id" = Uuid, Path, description = "Entity ID")),
    responses(
        (status = 204, description = "Entity deleted"),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "Entity not found", body = crate::error::ApiErrorBody),
    ),
    tag = "entities",
)]
pub async fn delete_entity(
    mut authorized: Authorized<WriteScope>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    entities::delete(authorized.conn(), workspace_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/entities",
    params(ListEntitiesParams),
    responses(
        (status = 200, description = "List of entities retrieved", body = Vec<EntityRecord>),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
    ),
    tag = "entities",
)]
pub async fn list_entities(
    mut authorized: Authorized<ReadScope>,
    Query(params): Query<ListEntitiesParams>,
) -> Result<Json<Vec<EntityRecord>>, ApiError> {
    let default = entities::ListEntitiesQuery::default();
    let query = entities::ListEntitiesQuery {
        entity_type: params.entity_type,
        filter: crate::http::controllers::parse_filter_param(params.filter)?,
        limit: params.limit.unwrap_or(default.limit),
        offset: params.offset.unwrap_or(default.offset),
    };

    let workspace_id = authorized.ctx.workspace_id;
    let records = entities::list(authorized.conn(), workspace_id, query).await?;
    Ok(Json(records))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct EntityContextParams {
    /// Maximum number of relations/neighbors to include (defaults to 20 if omitted).
    pub limit: Option<i64>,
    /// When true, neighbor entities include every field instead of only `x-embed` fields
    /// (defaults to false).
    pub full: Option<bool>,
}

#[utoipa::path(
    get,
    path = "/api/entities/{id}/context",
    params(("id" = Uuid, Path, description = "Entity ID"), EntityContextParams),
    responses(
        (status = 200, description = "Entity, its relations, and connected neighbors", body = RecallContext),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "Entity not found", body = crate::error::ApiErrorBody),
    ),
    tag = "entities",
)]
pub async fn get_entity_context(
    mut authorized: Authorized<ReadScope>,
    Path(id): Path<Uuid>,
    Query(params): Query<EntityContextParams>,
) -> Result<Json<RecallContext>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let limit = params.limit.unwrap_or(recall::DEFAULT_RECALL_LIMIT);
    let full = params.full.unwrap_or(false);
    let context = recall::recall_context(authorized.conn(), workspace_id, id, limit, full).await?;
    Ok(Json(context))
}
