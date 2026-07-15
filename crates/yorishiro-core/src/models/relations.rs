use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::models::entities::EntityRecord;

#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct RelationRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    #[schema(value_type = Object)]
    pub properties: Value,
    pub created_at: DateTime<Utc>,
}

pub struct CreateRelationInput {
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    pub properties: Value,
}

pub const DEFAULT_LIST_LIMIT: i64 = 50;

pub struct ListRelationsQuery {
    pub source_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub relation_type: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl Default for ListRelationsQuery {
    fn default() -> Self {
        Self {
            source_id: None,
            target_id: None,
            relation_type: None,
            limit: DEFAULT_LIST_LIMIT,
            offset: 0,
        }
    }
}

pub const DEFAULT_NEIGHBORS_LIMIT: i64 = 20;

/// A relation together with the entity on the other end of it, relative to the entity
/// `neighbors` was called for. `direction` is `"out"` when the queried entity is the
/// relation's source (the neighbor is the target) and `"in"` when it's the target (the
/// neighbor is the source).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Neighbor {
    pub relation_id: Uuid,
    pub relation_type: String,
    pub direction: String,
    #[schema(value_type = Object)]
    pub properties: Value,
    pub entity: EntityRecord,
}
