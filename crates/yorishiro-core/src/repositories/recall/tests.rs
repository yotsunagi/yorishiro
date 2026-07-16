use serde_json::json;
use sqlx::PgPool;

use super::*;
use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;
use crate::repositories::relations::CreateRelationInput;

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
    crate::test_support::seed_tenant_and_workspace(pool).await
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
