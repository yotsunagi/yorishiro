use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use utoipa::IntoParams;
use yorishiro_core::YorishiroError;
use yorishiro_core::search::{self, SearchHit};

use crate::auth::{ReadScope, Verified};
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Deserialize, IntoParams)]
pub struct SearchEntitiesParams {
    pub query_text: String,
    pub entity_type: Option<String>,
    pub limit: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/search",
    params(SearchEntitiesParams),
    responses(
        (status = 200, description = "Vector similarity search results for a natural-language query", body = Vec<SearchHit>),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
    ),
    tag = "search",
)]
pub async fn search_entities(
    State(state): State<AppState>,
    // `Verified`, not `Authorized`: no connection is acquired here, since one
    // isn't needed until after the slow embedding call below.
    verified: Verified<ReadScope>,
    Query(params): Query<SearchEntitiesParams>,
) -> Result<Json<Vec<SearchHit>>, ApiError> {
    let default = search::SearchQuery::default();
    let query = search::SearchQuery {
        entity_type: params.entity_type,
        limit: params.limit.unwrap_or(default.limit),
    };

    // Embedding generation happens before acquiring a DB connection. The
    // LocalOnnx provider serializes inference within the process, so holding a
    // connection while waiting would let pool exhaustion spill over to other
    // endpoints too.
    let vector = search::embed_query(state.embedding_provider.as_ref(), &params.query_text).await?;

    let tenant_id = verified.ctx.tenant_id;
    let mut conn = state
        .tenant_db
        .acquire_for_tenant(tenant_id)
        .await
        .map_err(|err| ApiError(YorishiroError::Internal(err.into())))?;
    let hits = search::search_by_vector(&mut conn, tenant_id, vector, query).await?;
    Ok(Json(hits))
}
