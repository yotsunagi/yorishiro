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
        (status = 200, description = "自然文クエリによるベクトル類似検索の結果", body = Vec<SearchHit>),
        (status = 401, description = "認証情報が無効", body = crate::error::ApiErrorBody),
        (status = 403, description = "scopeが不足している", body = crate::error::ApiErrorBody),
    ),
    tag = "search",
)]
pub async fn search_entities(
    State(state): State<AppState>,
    verified: Verified<ReadScope>,
    Query(params): Query<SearchEntitiesParams>,
) -> Result<Json<Vec<SearchHit>>, ApiError> {
    let default = search::SearchQuery::default();
    let query = search::SearchQuery {
        entity_type: params.entity_type,
        limit: params.limit.unwrap_or(default.limit),
    };

    // 埋め込み生成はDBコネクション取得より先に行う。LocalOnnxプロバイダでは推論が
    // プロセス内で直列化されるため、コネクションを握ったまま待つとプール枯渇が
    // 検索以外のエンドポイントにも波及する。
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
