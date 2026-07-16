use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde_json::json;
use sqlx::PgPool;

use super::*;
use crate::db::TenantDb;
use crate::metaschema::MetaSchemaDefinition;
use crate::repositories::entities::{self, CreateEntityInput};
use crate::repositories::schemas;

struct FakeProvider {
    dimensions: usize,
    calls: AtomicUsize,
}

impl FakeProvider {
    fn new(dimensions: usize) -> Self {
        Self {
            dimensions,
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for FakeProvider {
    fn dimensions(&self) -> usize {
        self.dimensions
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(texts
            .iter()
            .map(|_| vec![0.5_f32; self.dimensions])
            .collect())
    }
}

struct FailingProvider;

#[async_trait]
impl EmbeddingProvider for FailingProvider {
    fn dimensions(&self) -> usize {
        768
    }

    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Err(YorishiroError::Internal(anyhow::anyhow!(
            "embedding provider unavailable"
        )))
    }
}

fn task_schema_with_embed() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "title": { "type": "string", "required": true, "x-embed": true },
                    "priority": { "type": "integer" }
                }
            }
        }
    }))
    .unwrap()
}

fn task_schema_without_embed() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": { "title": { "type": "string", "required": true } }
            }
        }
    }))
    .unwrap()
}

async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
    crate::test_support::seed_tenant_and_workspace(pool).await
}

async fn has_embedding(conn: &mut PgConnection, entity_id: Uuid) -> bool {
    let (sql, values) = Query::select()
        .expr_as(
            Expr::col(Entities::Embedding).is_not_null(),
            Alias::new("has_embedding"),
        )
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::Id).eq(entity_id))
        .build_sqlx(PostgresQueryBuilder);
    let (has_embedding,): (bool,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    has_embedding
}

#[sqlx::test(migrations = "../../migrations")]
async fn writes_embedding_for_x_embed_field(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema_with_embed())
        .await
        .unwrap();

    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "write report", "priority": 1 }),
        },
        None,
    )
    .await
    .unwrap();

    let schema = schemas::get_by_id(&mut conn, workspace_id, entity.schema_id)
        .await
        .unwrap();
    let entity_type_def = &schema.definition.entity_types["task"];
    let provider = FakeProvider::new(768);

    sync_embedding(
        &mut conn,
        workspace_id,
        entity.id,
        entity.updated_at,
        entity_type_def,
        &entity.data,
        &provider,
    )
    .await
    .unwrap();

    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

    let has_embedding = has_embedding(&mut conn, entity.id).await;
    assert!(has_embedding);
}

#[sqlx::test(migrations = "../../migrations")]
async fn sync_for_record_resolves_schema_and_writes_embedding(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema_with_embed())
        .await
        .unwrap();

    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "write report" }),
        },
        None,
    )
    .await
    .unwrap();

    let provider = FakeProvider::new(768);
    sync_embedding_for_record(&mut conn, workspace_id, &entity, &provider)
        .await
        .unwrap();

    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

    let has_embedding = has_embedding(&mut conn, entity.id).await;
    assert!(has_embedding);
}

#[sqlx::test(migrations = "../../migrations")]
async fn skips_embedding_when_no_x_embed_field_is_defined(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema_without_embed())
        .await
        .unwrap();

    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "no embedding here" }),
        },
        None,
    )
    .await
    .unwrap();

    let schema = schemas::get_by_id(&mut conn, workspace_id, entity.schema_id)
        .await
        .unwrap();
    let entity_type_def = &schema.definition.entity_types["task"];
    let provider = FakeProvider::new(768);

    sync_embedding(
        &mut conn,
        workspace_id,
        entity.id,
        entity.updated_at,
        entity_type_def,
        &entity.data,
        &provider,
    )
    .await
    .unwrap();

    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);

    let has_embedding = has_embedding(&mut conn, entity.id).await;
    assert!(!has_embedding);
}

#[test]
fn composes_text_from_multiple_x_embed_fields_in_field_name_order() {
    let def: MetaSchemaDefinition = serde_json::from_value(json!({
        "name": "article",
        "entity_types": {
            "post": {
                "fields": {
                    "title": { "type": "string", "x-embed": true },
                    "body": { "type": "string", "x-embed": true },
                    "views": { "type": "integer" }
                }
            }
        }
    }))
    .unwrap();

    let text = compose_embedding_text(
        &def.entity_types["post"],
        &json!({ "title": "hello", "body": "world", "views": 42 }),
    )
    .unwrap();

    assert_eq!(text, "body: world\ntitle: hello");
}

#[test]
fn skips_x_embed_field_when_value_is_null() {
    let def: MetaSchemaDefinition = serde_json::from_value(json!({
        "name": "article",
        "entity_types": {
            "post": {
                "fields": {
                    "title": { "type": "string", "x-embed": true },
                    "subtitle": { "type": "string", "x-embed": true }
                }
            }
        }
    }))
    .unwrap();

    let text = compose_embedding_text(
        &def.entity_types["post"],
        &json!({ "title": "hello", "subtitle": null }),
    )
    .unwrap();

    assert_eq!(text, "title: hello");
}

#[sqlx::test(migrations = "../../migrations")]
async fn propagates_provider_errors(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema_with_embed())
        .await
        .unwrap();

    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "write report" }),
        },
        None,
    )
    .await
    .unwrap();

    let schema = schemas::get_by_id(&mut conn, workspace_id, entity.schema_id)
        .await
        .unwrap();
    let entity_type_def = &schema.definition.entity_types["task"];

    let err = sync_embedding(
        &mut conn,
        workspace_id,
        entity.id,
        entity.updated_at,
        entity_type_def,
        &entity.data,
        &FailingProvider,
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::Internal(_)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn silently_succeeds_when_entity_no_longer_exists(pool: PgPool) {
    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool);
    let mut conn = db
        .acquire_for_workspace(workspace_id_tenant, workspace_id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, workspace_id, task_schema_with_embed())
        .await
        .unwrap();

    let entity = entities::create(
        &mut conn,
        workspace_id,
        CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: json!({ "title": "will be deleted" }),
        },
        None,
    )
    .await
    .unwrap();

    let schema = schemas::get_by_id(&mut conn, workspace_id, entity.schema_id)
        .await
        .unwrap();
    let entity_type_def = &schema.definition.entity_types["task"];

    entities::delete(&mut conn, workspace_id, entity.id)
        .await
        .unwrap();

    let provider = FakeProvider::new(768);
    let result = sync_embedding(
        &mut conn,
        workspace_id,
        entity.id,
        entity.updated_at,
        entity_type_def,
        &entity.data,
        &provider,
    )
    .await;

    assert!(result.is_ok());
}
