use serde::Serialize;
use utoipa::ToSchema;

use crate::models::entities::EntityRecord;
use crate::models::relations::DEFAULT_NEIGHBORS_LIMIT;

pub const DEFAULT_RECALL_LIMIT: i64 = DEFAULT_NEIGHBORS_LIMIT;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RecallRelation {
    pub relation_type: String,
    pub direction: String,
    /// The connected entity. Shallow (only `x-embed` fields in `data`) by default; pass
    /// `full: true` to `recall_context` to get every field instead.
    pub neighbor: EntityRecord,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RecallContext {
    /// The requested entity, always with its full `data`.
    pub entity: EntityRecord,
    pub relations: Vec<RecallRelation>,
    /// `true` when more neighbors exist beyond `limit` than are included above.
    pub truncated: bool,
}
