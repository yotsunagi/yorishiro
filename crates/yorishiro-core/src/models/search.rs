use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::models::entities::EntityRecord;

const DEFAULT_SEARCH_LIMIT: i64 = 10;

pub struct SearchQuery {
    pub entity_type: Option<String>,
    /// JSONB containment filter (`data @> filter`), e.g. `{"status": "active"}`.
    pub filter: Option<Value>,
    pub limit: i64,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            entity_type: None,
            filter: None,
            limit: DEFAULT_SEARCH_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SearchHit {
    pub entity: EntityRecord,
    /// pgvector cosine distance (the `<=>` operator). Closer to 0 means more similar. `None`
    /// when the entity has no embedding and was only surfaced through the pg_trgm fuzzy
    /// text match on `query_text`.
    pub distance: Option<f64>,
}
