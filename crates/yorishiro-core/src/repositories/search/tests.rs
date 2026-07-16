use async_trait::async_trait;
use serde_json::json;
use sqlx::PgPool;
use std::collections::HashMap;

use super::*;
use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;
use crate::repositories::entities::{self, CreateEntityInput};
use crate::repositories::schemas;
use crate::services::embedding::sync as embedding_sync;

const DIM: usize = 768;

fn unit_vector(index: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; DIM];
    v[index] = 1.0;
    v
}

/// A fake provider that lets each test explicitly fix the text→vector mapping. Panics
/// if given unregistered text, catching broken test assumptions immediately.
struct MapProvider {
    vectors: HashMap<String, Vec<f32>>,
}

impl MapProvider {
    fn new<K: Into<String>>(pairs: impl IntoIterator<Item = (K, Vec<f32>)>) -> Self {
        Self {
            vectors: pairs.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for MapProvider {
    fn dimensions(&self) -> usize {
        DIM
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Ok(texts
            .iter()
            .map(|text| {
                self.vectors
                    .get(*text)
                    .unwrap_or_else(|| panic!("no fixture vector registered for '{text}'"))
                    .clone()
            })
            .collect())
    }
}

fn task_schema_with_embed() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": { "fields": { "title": { "type": "string", "x-embed": true } } },
            "project": { "fields": { "title": { "type": "string", "x-embed": true } } }
        }
    }))
    .unwrap()
}

async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
    crate::test_support::seed_tenant_and_workspace(pool).await
}

async fn seed_embedded_entity(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    entity_type: &str,
    title: &str,
    vector: Vec<f32>,
) -> entities::EntityRecord {
    let entity = entities::create(
        conn,
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

    let schema = schemas::get_by_id(conn, workspace_id, entity.schema_id)
        .await
        .unwrap();
    let entity_type_def = &schema.definition.entity_types[entity_type];
    // compose_embedding_text builds text in "field: value" form, so the fixture key matches that format.
    let provider = MapProvider::new([(format!("title: {title}"), vector)]);

    embedding_sync::sync_embedding(
        conn,
        workspace_id,
        entity.id,
        entity.updated_at,
        entity_type_def,
        &entity.data,
        &provider,
    )
    .await
    .unwrap();

    entity
}

#[sqlx::test(migrations = "../../migrations")]
async fn returns_closest_entities_first(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema_with_embed())
        .await
        .unwrap();

    let apple = seed_embedded_entity(
        &mut conn,
        workspace_id,
        "task",
        "apple pie recipe",
        unit_vector(0),
    )
    .await;
    let car = seed_embedded_entity(
        &mut conn,
        workspace_id,
        "task",
        "car engine repair",
        unit_vector(1),
    )
    .await;

    let query_provider = MapProvider::new([("fruit dessert", unit_vector(0))]);
    let hits = search_by_text(
        &mut conn,
        workspace_id,
        &query_provider,
        "fruit dessert",
        SearchQuery::default(),
    )
    .await
    .unwrap();

    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].entity.id, apple.id);
    assert!(hits[0].distance < hits[1].distance);
    assert_eq!(hits[1].entity.id, car.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn filters_by_entity_type(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema_with_embed())
        .await
        .unwrap();

    // project has a vector closer to the query, but we filter to entity_type=task.
    let task = seed_embedded_entity(
        &mut conn,
        workspace_id,
        "task",
        "distant task",
        unit_vector(5),
    )
    .await;
    seed_embedded_entity(
        &mut conn,
        workspace_id,
        "project",
        "close project",
        unit_vector(0),
    )
    .await;

    let query_provider = MapProvider::new([("query", unit_vector(0))]);
    let hits = search_by_text(
        &mut conn,
        workspace_id,
        &query_provider,
        "query",
        SearchQuery {
            entity_type: Some("task".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity.id, task.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn excludes_entities_without_an_embedding(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema_with_embed())
        .await
        .unwrap();

    // embedding stays NULL since sync_embedding is never called.
    entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "never embedded" }),
        },
        None,
    )
    .await
    .unwrap();

    let query_provider = MapProvider::new([("query", unit_vector(0))]);
    let hits = search_by_text(
        &mut conn,
        workspace_id,
        &query_provider,
        "query",
        SearchQuery::default(),
    )
    .await
    .unwrap();

    assert!(hits.is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn surfaces_entities_without_an_embedding_via_trigram_fuzzy_match(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema_with_embed())
        .await
        .unwrap();

    // embedding stays NULL since sync_embedding is never called; only a close text
    // match on `data` can surface this entity.
    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "widget assembly line status" }),
        },
        None,
    )
    .await
    .unwrap();

    let query_provider = MapProvider::new([("widget assembly line status", unit_vector(0))]);
    let hits = search_by_text(
        &mut conn,
        workspace_id,
        &query_provider,
        "widget assembly line status",
        SearchQuery::default(),
    )
    .await
    .unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity.id, entity.id);
    assert!(hits[0].distance.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn filters_by_data_field_value(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema_with_embed())
        .await
        .unwrap();

    let active = seed_embedded_entity(
        &mut conn,
        workspace_id,
        "task",
        "active one",
        unit_vector(0),
    )
    .await;
    let active_entity = entities::update(
        &mut conn,
        workspace_id,
        active.id,
        json!({ "title": "active one", "status": "active" }),
        None,
    )
    .await
    .unwrap();
    let done =
        seed_embedded_entity(&mut conn, workspace_id, "task", "done one", unit_vector(0)).await;
    entities::update(
        &mut conn,
        workspace_id,
        done.id,
        json!({ "title": "done one", "status": "done" }),
        None,
    )
    .await
    .unwrap();

    let query_provider = MapProvider::new([("query", unit_vector(0))]);
    let hits = search_by_text(
        &mut conn,
        workspace_id,
        &query_provider,
        "query",
        SearchQuery {
            filter: Some(json!({ "status": "active" })),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity.id, active_entity.id);
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
    schemas::create_schema(&mut conn_a, tenant_a, task_schema_with_embed())
        .await
        .unwrap();
    seed_embedded_entity(
        &mut conn_a,
        tenant_a,
        "task",
        "tenant a task",
        unit_vector(0),
    )
    .await;

    let mut conn_b = db
        .acquire_for_workspace(tenant_b_tenant, tenant_b)
        .await
        .unwrap();
    let query_provider = MapProvider::new([("query", unit_vector(0))]);
    let hits = search_by_text(
        &mut conn_b,
        tenant_b,
        &query_provider,
        "query",
        SearchQuery::default(),
    )
    .await
    .unwrap();

    assert!(hits.is_empty());
}
