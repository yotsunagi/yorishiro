use serde_json::json;
use sqlx::PgPool;

use super::*;
use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;

fn project_task_schema() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": { "fields": { "title": { "type": "string", "required": true } } },
            "project": { "fields": { "title": { "type": "string", "required": true } } }
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

async fn seed_task_and_project(
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> (entities::EntityRecord, entities::EntityRecord) {
    schemas::create_schema(conn, workspace_id, project_task_schema())
        .await
        .unwrap();

    let task = entities::create(
        conn,
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

    let project = entities::create(
        conn,
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

    (task, project)
}

#[sqlx::test(migrations = "../../migrations")]
async fn creates_and_fetches_relation(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id).await;

    let created = create(
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

    assert_eq!(created.relation_type, "belongs_to");
    assert_eq!(created.properties, json!({}));

    let fetched = get(&mut conn, workspace_id, created.id).await.unwrap();
    assert_eq!(fetched.source_id, task.id);
    assert_eq!(fetched.target_id, project.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_relation_type_with_mismatched_source_target(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id).await;

    // reversed: belongs_to expects source=task target=project, not the other way around.
    let err = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: project.id,
            target_id: task.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::RelationTypeMismatch { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_relation_with_nonexistent_source(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (_, project) = seed_task_and_project(&mut conn, workspace_id).await;

    let err = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: Uuid::nil(),
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_unknown_relation_type(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id).await;

    let err = create(
        &mut conn,
        workspace_id,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "no_such_relation".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_duplicate_relation(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id).await;

    create(
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

    let err = create(
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
    .unwrap_err();

    assert!(matches!(err, YorishiroError::Conflict { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_entity_cascades_relation_deletion(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id).await;

    let relation = create(
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

    entities::delete(&mut conn, workspace_id, task.id)
        .await
        .unwrap();

    let err = get(&mut conn, workspace_id, relation.id).await.unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn deletes_relation(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id).await;

    let relation = create(
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

    delete(&mut conn, workspace_id, relation.id).await.unwrap();

    let err = get(&mut conn, workspace_id, relation.id).await.unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_reports_not_found_for_missing_relation(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();

    let err = delete(&mut conn, workspace_id, Uuid::nil())
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn enforces_tenant_isolation(pool: PgPool) {
    let (tenant_a_tenant, tenant_a) = seed_workspace(&pool).await;
    let (tenant_b_tenant, tenant_b) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);

    let mut conn_a = db
        .acquire_for_workspace(tenant_a_tenant, tenant_a)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn_a, tenant_a).await;
    let relation = create(
        &mut conn_a,
        tenant_a,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap();

    let mut conn_b = db
        .acquire_for_workspace(tenant_b_tenant, tenant_b)
        .await
        .unwrap();
    let result = get(&mut conn_b, tenant_b, relation.id).await;
    assert!(matches!(result, Err(YorishiroError::NotFound { .. })));

    // tenant_b can't see tenant_a's entities either, so the source/target existence check itself reports NotFound.
    let cross_tenant_err = create(
        &mut conn_b,
        tenant_b,
        CreateRelationInput {
            source_id: task.id,
            target_id: project.id,
            relation_type: "belongs_to".into(),
            properties: Value::Null,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(cross_tenant_err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn lists_relations_filtered_by_source(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id).await;

    create(
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

    let relations = list(
        &mut conn,
        workspace_id,
        ListRelationsQuery {
            source_id: Some(task.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0].target_id, project.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn neighbors_returns_both_directions_with_relation_type(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, project) = seed_task_and_project(&mut conn, workspace_id).await;

    create(
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

    let from_task = neighbors(&mut conn, workspace_id, task.id, DEFAULT_NEIGHBORS_LIMIT)
        .await
        .unwrap();
    assert_eq!(from_task.len(), 1);
    assert_eq!(from_task[0].direction, "out");
    assert_eq!(from_task[0].relation_type, "belongs_to");
    assert_eq!(from_task[0].entity.id, project.id);

    let from_project = neighbors(&mut conn, workspace_id, project.id, DEFAULT_NEIGHBORS_LIMIT)
        .await
        .unwrap();
    assert_eq!(from_project.len(), 1);
    assert_eq!(from_project[0].direction, "in");
    assert_eq!(from_project[0].relation_type, "belongs_to");
    assert_eq!(from_project[0].entity.id, task.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn neighbors_is_empty_when_no_relations_exist(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    let (task, _project) = seed_task_and_project(&mut conn, workspace_id).await;

    let result = neighbors(&mut conn, workspace_id, task.id, DEFAULT_NEIGHBORS_LIMIT)
        .await
        .unwrap();
    assert!(result.is_empty());
}
