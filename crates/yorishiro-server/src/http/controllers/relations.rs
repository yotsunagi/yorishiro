use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use yorishiro_core::relations::{self, RelationRecord};

use crate::error::ApiError;
use crate::http::middleware::auth::{Authorized, ReadScope, WriteScope};

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateRelationRequest {
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    #[schema(value_type = Option<Object>)]
    pub properties: Option<Value>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListRelationsParams {
    pub source_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub relation_type: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[utoipa::path(
    post,
    path = "/api/relations",
    request_body = CreateRelationRequest,
    responses(
        (status = 201, description = "Relation created", body = RelationRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "The source, target, or relation_type does not exist", body = crate::error::ApiErrorBody),
        (status = 409, description = "An identical relation already exists", body = crate::error::ApiErrorBody),
        (status = 422, description = "relation_type conflicts with the entity_type of source/target", body = crate::error::ApiErrorBody),
    ),
    tag = "relations",
)]
pub async fn create_relation(
    mut authorized: Authorized<WriteScope>,
    Json(body): Json<CreateRelationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let input = relations::CreateRelationInput {
        source_id: body.source_id,
        target_id: body.target_id,
        relation_type: body.relation_type,
        properties: body.properties.unwrap_or_else(|| serde_json::json!({})),
    };
    let record = relations::create(authorized.conn(), workspace_id, input).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    get,
    path = "/api/relations/{id}",
    params(("id" = Uuid, Path, description = "Relation ID")),
    responses(
        (status = 200, description = "Relation retrieved", body = RelationRecord),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "Relation not found", body = crate::error::ApiErrorBody),
    ),
    tag = "relations",
)]
pub async fn get_relation(
    mut authorized: Authorized<ReadScope>,
    Path(id): Path<Uuid>,
) -> Result<Json<RelationRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record = relations::get(authorized.conn(), workspace_id, id).await?;
    Ok(Json(record))
}

#[utoipa::path(
    delete,
    path = "/api/relations/{id}",
    params(("id" = Uuid, Path, description = "Relation ID")),
    responses(
        (status = 204, description = "Relation deleted"),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 404, description = "Relation not found", body = crate::error::ApiErrorBody),
    ),
    tag = "relations",
)]
pub async fn delete_relation(
    mut authorized: Authorized<WriteScope>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    relations::delete(authorized.conn(), workspace_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/relations",
    params(ListRelationsParams),
    responses(
        (status = 200, description = "List of relations retrieved", body = Vec<RelationRecord>),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
    ),
    tag = "relations",
)]
pub async fn list_relations(
    mut authorized: Authorized<ReadScope>,
    Query(params): Query<ListRelationsParams>,
) -> Result<Json<Vec<RelationRecord>>, ApiError> {
    let default = relations::ListRelationsQuery::default();
    let query = relations::ListRelationsQuery {
        source_id: params.source_id,
        target_id: params.target_id,
        relation_type: params.relation_type,
        limit: params.limit.unwrap_or(default.limit),
        offset: params.offset.unwrap_or(default.offset),
    };

    let workspace_id = authorized.ctx.workspace_id;
    let records = relations::list(authorized.conn(), workspace_id, query).await?;
    Ok(Json(records))
}
