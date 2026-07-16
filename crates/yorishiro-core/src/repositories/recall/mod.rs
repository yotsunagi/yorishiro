use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::entities::EntityRecord;
use crate::repositories::entities;
use crate::repositories::relations;
use crate::repositories::schemas;

pub use crate::models::recall::*;

/// Reduces `entity.data` down to only the fields marked `x-embed` in its entity_type
/// definition. Falls back to an empty body if the entity's schema version no longer defines
/// that entity_type at all, rather than failing the whole recall for one neighbor.
async fn shallow_copy(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    mut entity: EntityRecord,
) -> Result<EntityRecord, YorishiroError> {
    let schema = schemas::get_by_id(conn, workspace_id, entity.schema_id).await?;
    let fields = schema
        .definition
        .entity_types
        .get(&entity.entity_type)
        .map(|def| &def.fields);

    let mut shallow = Map::new();
    if let (Some(fields), Value::Object(data)) = (fields, &entity.data) {
        for (name, field_def) in fields {
            if field_def.x_embed
                && let Some(value) = data.get(name)
            {
                shallow.insert(name.clone(), value.clone());
            }
        }
    }
    entity.data = Value::Object(shallow);
    Ok(entity)
}

/// Fetches an entity's full body together with its relations and connected neighbors in one
/// call, so a caller doesn't need `entity_get` + `list_relations` + `entity_get` per neighbor
/// round trips. Neighbors are shallow (only `x-embed` fields) unless `full` is set.
pub async fn recall_context(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    entity_id: Uuid,
    limit: i64,
    full: bool,
) -> Result<RecallContext, YorishiroError> {
    let limit = limit.clamp(1, 200);
    let entity = entities::get(conn, workspace_id, entity_id).await?;

    let mut neighbors = relations::neighbors(conn, workspace_id, entity_id, limit + 1).await?;
    let truncated = neighbors.len() as i64 > limit;
    neighbors.truncate(limit as usize);

    let mut relations_out = Vec::with_capacity(neighbors.len());
    for neighbor in neighbors {
        let neighbor_entity = if full {
            neighbor.entity
        } else {
            shallow_copy(conn, workspace_id, neighbor.entity).await?
        };
        relations_out.push(RecallRelation {
            relation_type: neighbor.relation_type,
            direction: neighbor.direction,
            neighbor: neighbor_entity,
        });
    }

    Ok(RecallContext {
        entity,
        relations: relations_out,
        truncated,
    })
}

#[cfg(test)]
mod tests;
