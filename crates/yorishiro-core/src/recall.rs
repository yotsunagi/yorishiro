use serde::Serialize;
use serde_json::{Map, Value};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::entities::{self, EntityRecord};
use crate::error::YorishiroError;
use crate::relations::{self, DEFAULT_NEIGHBORS_LIMIT};
use crate::schemas;

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
mod tests {
    use serde_json::json;
    use sqlx::PgPool;

    use super::*;
    use crate::db::TenantDb;
    use crate::metaschema::MetaSchemaDefinition;
    use crate::relations::CreateRelationInput;

    fn project_task_schema() -> MetaSchemaDefinition {
        serde_json::from_value(json!({
            "name": "task-management",
            "entity_types": {
                "task": {
                    "fields": {
                        "title": { "type": "string", "required": true, "x-embed": true },
                        "note": { "type": "string" }
                    }
                },
                "project": {
                    "fields": { "title": { "type": "string", "required": true, "x-embed": true } }
                }
            },
            "relation_types": {
                "belongs_to": { "source": "task", "target": "project" }
            }
        }))
        .unwrap()
    }

    async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
        let (tenant_id,): (Uuid,) =
            sqlx::query_as("INSERT INTO identity.tenants (name) VALUES ($1) RETURNING id")
                .bind("test-tenant")
                .fetch_one(pool)
                .await
                .unwrap();
        let (workspace_id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO identity.workspaces (tenant_id, name) VALUES ($1, $2) RETURNING id",
        )
        .bind(tenant_id)
        .bind("test-workspace")
        .fetch_one(pool)
        .await
        .unwrap();
        (tenant_id, workspace_id)
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn returns_entity_with_shallow_neighbors_by_default(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();
        schemas::create_schema(&mut conn, workspace_id, project_task_schema())
            .await
            .unwrap();

        let project = entities::create(
            &mut conn,
            workspace_id,
            entities::CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "project".into(),
                data: json!({ "title": "Q3 roadmap" }),
            },
            None,
        )
        .await
        .unwrap();
        let task = entities::create(
            &mut conn,
            workspace_id,
            entities::CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "write report", "note": "internal only" }),
            },
            None,
        )
        .await
        .unwrap();
        relations::create(
            &mut conn,
            workspace_id,
            CreateRelationInput {
                source_id: task.id,
                target_id: project.id,
                relation_type: "belongs_to".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap();

        let context = recall_context(
            &mut conn,
            workspace_id,
            task.id,
            DEFAULT_RECALL_LIMIT,
            false,
        )
        .await
        .unwrap();

        assert_eq!(context.entity.id, task.id);
        assert_eq!(context.entity.data["note"], "internal only");
        assert!(!context.truncated);
        assert_eq!(context.relations.len(), 1);
        assert_eq!(context.relations[0].direction, "out");
        assert_eq!(context.relations[0].relation_type, "belongs_to");
        assert_eq!(context.relations[0].neighbor.id, project.id);
        assert_eq!(context.relations[0].neighbor.data["title"], "Q3 roadmap");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn full_flag_returns_the_neighbors_entire_data(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();
        schemas::create_schema(&mut conn, workspace_id, project_task_schema())
            .await
            .unwrap();

        let project = entities::create(
            &mut conn,
            workspace_id,
            entities::CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "project".into(),
                data: json!({ "title": "Q3 roadmap" }),
            },
            None,
        )
        .await
        .unwrap();
        let task = entities::create(
            &mut conn,
            workspace_id,
            entities::CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "write report", "note": "internal only" }),
            },
            None,
        )
        .await
        .unwrap();
        relations::create(
            &mut conn,
            workspace_id,
            CreateRelationInput {
                source_id: task.id,
                target_id: project.id,
                relation_type: "belongs_to".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap();

        let context = recall_context(
            &mut conn,
            workspace_id,
            project.id,
            DEFAULT_RECALL_LIMIT,
            true,
        )
        .await
        .unwrap();

        assert_eq!(context.relations.len(), 1);
        assert_eq!(context.relations[0].neighbor.data["note"], "internal only");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn sets_truncated_when_more_neighbors_exist_than_the_limit(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();
        schemas::create_schema(&mut conn, workspace_id, project_task_schema())
            .await
            .unwrap();

        let task = entities::create(
            &mut conn,
            workspace_id,
            entities::CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "write report" }),
            },
            None,
        )
        .await
        .unwrap();

        for name in ["alpha", "beta", "gamma"] {
            let project = entities::create(
                &mut conn,
                workspace_id,
                entities::CreateEntityInput {
                    schema_name: "task-management".into(),
                    entity_type: "project".into(),
                    data: json!({ "title": name }),
                },
                None,
            )
            .await
            .unwrap();
            relations::create(
                &mut conn,
                workspace_id,
                CreateRelationInput {
                    source_id: task.id,
                    target_id: project.id,
                    relation_type: "belongs_to".into(),
                    properties: Value::Null,
                },
            )
            .await
            .unwrap();
        }

        let context = recall_context(&mut conn, workspace_id, task.id, 2, false)
            .await
            .unwrap();

        assert_eq!(context.relations.len(), 2);
        assert!(context.truncated);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn reports_not_found_for_a_missing_entity(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();

        let err = recall_context(
            &mut conn,
            workspace_id,
            Uuid::nil(),
            DEFAULT_RECALL_LIMIT,
            false,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }
}
