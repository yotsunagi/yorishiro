use serde_json::json;
use sqlx::PgPool;

use super::*;
use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;

fn task_schema() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "title": { "type": "string", "required": true }
                }
            }
        }
    }))
    .unwrap()
}

#[test]
fn default_list_query_uses_a_sensible_page_size() {
    let query = ListEntitiesQuery::default();
    assert_eq!(query.limit, DEFAULT_LIST_LIMIT);
    assert_eq!(query.offset, 0);
    assert!(query.entity_type.is_none());
}

#[test]
fn missing_required_field_points_at_the_missing_property() {
    let def = task_schema();
    let entity_type_def = &def.entity_types["task"];

    let err = validate_data(entity_type_def, &json!({})).unwrap_err();
    match err {
        YorishiroError::ValidationFailed { details, .. } => {
            assert!(
                details.iter().any(|d| d.field == "/title"),
                "details: {details:?}"
            );
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
    crate::test_support::seed_tenant_and_workspace(pool).await
}

#[sqlx::test(migrations = "../../migrations")]
async fn creates_and_fetches_entity(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, workspace_id, task_schema())
        .await
        .unwrap();

    let created = create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "buy milk" }),
        },
        None,
    )
    .await
    .unwrap();

    assert_eq!(created.entity_type, "task");
    assert_eq!(created.schema_version, 1);

    let fetched = get(&mut conn, workspace_id, created.id).await.unwrap();
    assert_eq!(fetched.data["title"], "buy milk");
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_invalid_data(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema())
        .await
        .unwrap();

    let err = create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({}),
        },
        None,
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::ValidationFailed { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_unknown_entity_type(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema())
        .await
        .unwrap();

    let err = create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "nonexistent".into(),
            data: json!({}),
        },
        None,
    )
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
    schemas::create_schema(&mut conn_a, tenant_a, task_schema())
        .await
        .unwrap();
    let entity = create(
        &mut conn_a,
        tenant_a,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "tenant a task" }),
        },
        None,
    )
    .await
    .unwrap();

    let mut conn_b = db
        .acquire_for_workspace(tenant_b_tenant, tenant_b)
        .await
        .unwrap();
    let result = get(&mut conn_b, tenant_b, entity.id).await;
    assert!(matches!(result, Err(YorishiroError::NotFound { .. })));
}

#[sqlx::test(migrations = "../../migrations")]
async fn update_validates_against_creation_time_schema_version(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();

    schemas::create_schema(&mut conn, workspace_id, task_schema())
        .await
        .unwrap();
    let entity = create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "v1 task" }),
        },
        None,
    )
    .await
    .unwrap();

    let v2: MetaSchemaDefinition = serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "title": { "type": "string", "required": true },
                    "priority": { "type": "integer", "required": true }
                }
            }
        }
    }))
    .unwrap();
    schemas::create_schema(&mut conn, workspace_id, v2)
        .await
        .unwrap();

    let updated = update(
        &mut conn,
        workspace_id,
        entity.id,
        json!({ "title": "v1 task updated" }),
        None,
    )
    .await
    .unwrap();
    assert_eq!(updated.schema_version, 1);
    assert_eq!(updated.data["title"], "v1 task updated");
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_removes_entity(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema())
        .await
        .unwrap();
    let entity = create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "to delete" }),
        },
        None,
    )
    .await
    .unwrap();

    delete(&mut conn, workspace_id, entity.id).await.unwrap();
    let err = get(&mut conn, workspace_id, entity.id).await.unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_filters_by_entity_type(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(
        &mut conn,
        workspace_id,
        serde_json::from_value(json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "title": { "type": "string", "required": true } } },
                "project": { "fields": { "title": { "type": "string", "required": true } } }
            }
        }))
        .unwrap(),
    )
    .await
    .unwrap();

    for (entity_type, title) in [
        ("task", "task one"),
        ("task", "task two"),
        ("project", "project one"),
    ] {
        create(
            &mut conn,
            workspace_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: entity_type.into(),
                data: json!({ "title": title }),
            },
            None,
        )
        .await
        .unwrap();
    }

    let tasks = list(
        &mut conn,
        workspace_id,
        ListEntitiesQuery {
            entity_type: Some("task".into()),
            filter: None,
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().all(|e| e.entity_type == "task"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_filters_by_data_field_value(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema())
        .await
        .unwrap();

    for (title, status) in [
        ("task one", "active"),
        ("task two", "done"),
        ("task three", "active"),
    ] {
        create(
            &mut conn,
            workspace_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": title, "status": status }),
            },
            None,
        )
        .await
        .unwrap();
    }

    let active = list(
        &mut conn,
        workspace_id,
        ListEntitiesQuery {
            entity_type: None,
            filter: Some(json!({ "status": "active" })),
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap();
    assert_eq!(active.len(), 2);
    assert!(active.iter().all(|e| e.data["status"] == "active"));
}
