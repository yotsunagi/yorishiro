use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use yorishiro_core::relations::{self, RelationRecord};

use crate::auth::{Authorized, ReadScope, WriteScope};
use crate::error::ApiError;

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
        (status = 201, description = "リレーションを作成した", body = RelationRecord),
        (status = 401, description = "認証情報が無効", body = crate::error::ApiErrorBody),
        (status = 403, description = "scopeが不足している", body = crate::error::ApiErrorBody),
        (status = 404, description = "source/targetまたはrelation_typeが存在しない", body = crate::error::ApiErrorBody),
        (status = 409, description = "同一のリレーションが既に存在する", body = crate::error::ApiErrorBody),
        (status = 422, description = "relation_typeがsource/targetのentity_typeと矛盾する", body = crate::error::ApiErrorBody),
    ),
    tag = "relations",
)]
pub async fn create_relation(
    mut authorized: Authorized<WriteScope>,
    Json(body): Json<CreateRelationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let tenant_id = authorized.ctx.tenant_id;
    let input = relations::CreateRelationInput {
        source_id: body.source_id,
        target_id: body.target_id,
        relation_type: body.relation_type,
        properties: body.properties.unwrap_or_else(|| serde_json::json!({})),
    };
    let record = relations::create(authorized.conn(), tenant_id, input).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    get,
    path = "/api/relations/{id}",
    params(("id" = Uuid, Path, description = "リレーションID")),
    responses(
        (status = 200, description = "リレーションを取得した", body = RelationRecord),
        (status = 401, description = "認証情報が無効", body = crate::error::ApiErrorBody),
        (status = 403, description = "scopeが不足している", body = crate::error::ApiErrorBody),
        (status = 404, description = "リレーションが存在しない", body = crate::error::ApiErrorBody),
    ),
    tag = "relations",
)]
pub async fn get_relation(
    mut authorized: Authorized<ReadScope>,
    Path(id): Path<Uuid>,
) -> Result<Json<RelationRecord>, ApiError> {
    let tenant_id = authorized.ctx.tenant_id;
    let record = relations::get(authorized.conn(), tenant_id, id).await?;
    Ok(Json(record))
}

#[utoipa::path(
    delete,
    path = "/api/relations/{id}",
    params(("id" = Uuid, Path, description = "リレーションID")),
    responses(
        (status = 204, description = "リレーションを削除した"),
        (status = 401, description = "認証情報が無効", body = crate::error::ApiErrorBody),
        (status = 403, description = "scopeが不足している", body = crate::error::ApiErrorBody),
        (status = 404, description = "リレーションが存在しない", body = crate::error::ApiErrorBody),
    ),
    tag = "relations",
)]
pub async fn delete_relation(
    mut authorized: Authorized<WriteScope>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let tenant_id = authorized.ctx.tenant_id;
    relations::delete(authorized.conn(), tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/relations",
    params(ListRelationsParams),
    responses(
        (status = 200, description = "リレーションを一覧取得した", body = Vec<RelationRecord>),
        (status = 401, description = "認証情報が無効", body = crate::error::ApiErrorBody),
        (status = 403, description = "scopeが不足している", body = crate::error::ApiErrorBody),
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

    let tenant_id = authorized.ctx.tenant_id;
    let records = relations::list(authorized.conn(), tenant_id, query).await?;
    Ok(Json(records))
}
