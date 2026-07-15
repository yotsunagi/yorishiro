use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::entities::{self, EntityRecord};
use crate::error::YorishiroError;
use crate::relations::{self, RelationRecord};
use crate::schemas::{self, SchemaRecord};

/// One line of a JSONL export: a tagged union so schema/entity/relation records can be
/// told apart on read-back without a separate line-position convention.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "kind", content = "record", rename_all = "snake_case")]
pub enum ExportRecord {
    Schema(SchemaRecord),
    Entity(EntityRecord),
    Relation(RelationRecord),
}

/// Fetches every schema (all versions, including archived), entity, and relation for the
/// tenant, for a full-tenant data export. Schemas come first so a reader can resolve the
/// entity_types/relation_types that the entities and relations after them reference.
pub async fn export_all(
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<Vec<ExportRecord>, YorishiroError> {
    let mut records = Vec::new();
    records.extend(
        schemas::export_all(conn, workspace_id)
            .await?
            .into_iter()
            .map(ExportRecord::Schema),
    );
    records.extend(
        entities::export_all(conn, workspace_id)
            .await?
            .into_iter()
            .map(ExportRecord::Entity),
    );
    records.extend(
        relations::export_all(conn, workspace_id)
            .await?
            .into_iter()
            .map(ExportRecord::Relation),
    );
    Ok(records)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sqlx::PgPool;

    use super::*;
    use crate::db::TenantDb;
    use crate::metaschema::MetaSchemaDefinition;
    use crate::relations::CreateRelationInput;

    fn task_schema() -> MetaSchemaDefinition {
        serde_json::from_value(json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "title": { "type": "string", "required": true } } }
            },
            "relation_types": {
                "blocks": { "source": "task", "target": "task" }
            }
        }))
        .unwrap()
    }

    async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
        crate::test_support::seed_tenant_and_workspace(pool).await
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn exports_schemas_entities_and_relations_for_the_tenant(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();

        schemas::create_schema(&mut conn, workspace_id, task_schema())
            .await
            .unwrap();
        let a = entities::create(
            &mut conn,
            workspace_id,
            entities::CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "a" }),
            },
            None,
        )
        .await
        .unwrap();
        let b = entities::create(
            &mut conn,
            workspace_id,
            entities::CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "b" }),
            },
            None,
        )
        .await
        .unwrap();
        relations::create(
            &mut conn,
            workspace_id,
            CreateRelationInput {
                source_id: a.id,
                target_id: b.id,
                relation_type: "blocks".into(),
                properties: json!(null),
            },
        )
        .await
        .unwrap();

        let records = export_all(&mut conn, workspace_id).await.unwrap();

        let schema_count = records
            .iter()
            .filter(|r| matches!(r, ExportRecord::Schema(_)))
            .count();
        let entity_count = records
            .iter()
            .filter(|r| matches!(r, ExportRecord::Entity(_)))
            .count();
        let relation_count = records
            .iter()
            .filter(|r| matches!(r, ExportRecord::Relation(_)))
            .count();
        assert_eq!(schema_count, 1);
        assert_eq!(entity_count, 2);
        assert_eq!(relation_count, 1);

        let json = serde_json::to_value(&records[0]).unwrap();
        assert_eq!(json["kind"], "schema");
        assert!(json["record"]["definition"].is_object());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn export_is_empty_for_a_tenant_with_no_data(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();

        let records = export_all(&mut conn, workspace_id).await.unwrap();
        assert!(records.is_empty());
    }
}
