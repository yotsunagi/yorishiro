use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use utoipa::IntoParams;
use yorishiro_core::search::{self, SearchHit};

use crate::auth::{Authorized, ReadScope};
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
        (status = 200, description = "自然文クエリによるベクトル類似検索の結果", body = Vec<SearchHit>),
        (status = 401, description = "認証情報が無効", body = crate::error::ApiErrorBody),
        (status = 403, description = "scopeが不足している", body = crate::error::ApiErrorBody),
    ),
    tag = "search",
)]
pub async fn search_entities(
    State(state): State<AppState>,
    mut authorized: Authorized<ReadScope>,
    Query(params): Query<SearchEntitiesParams>,
) -> Result<Json<Vec<SearchHit>>, ApiError> {
    let default = search::SearchQuery::default();
    let query = search::SearchQuery {
        entity_type: params.entity_type,
        limit: params.limit.unwrap_or(default.limit),
    };

    let tenant_id = authorized.ctx.tenant_id;
    let hits = search::search_by_text(
        authorized.conn(),
        tenant_id,
        state.embedding_provider.as_ref(),
        &params.query_text,
        query,
    )
    .await?;
    Ok(Json(hits))
}
