use serde::Serialize;
use utoipa::ToSchema;

use crate::models::entities::EntityRecord;
use crate::models::relations::RelationRecord;
use crate::models::schemas::SchemaRecord;

/// One line of a JSONL export: a tagged union so schema/entity/relation records can be
/// told apart on read-back without a separate line-position convention.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "kind", content = "record", rename_all = "snake_case")]
pub enum ExportRecord {
    Schema(SchemaRecord),
    Entity(EntityRecord),
    Relation(RelationRecord),
}
