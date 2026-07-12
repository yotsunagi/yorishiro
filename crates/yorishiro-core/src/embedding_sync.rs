use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::embedding::EmbeddingProvider;
use crate::entities::EntityRecord;
use crate::error::YorishiroError;
use crate::metaschema::EntityTypeDef;
use crate::schemas;

/// `x-embed`が付いたフィールドの値を`"field: value"`の形式で連結し、埋め込み対象テキストを
/// 合成する。フィールド名を残すのは、値だけを裸で並べるより埋め込みモデルにとって
/// 意味的な文脈が保たれるため。対象フィールドが無い、またはいずれも値を持たない場合は
/// `None`を返し、呼び出し側でembedding API呼び出し自体をスキップできるようにする。
fn compose_embedding_text(entity_type_def: &EntityTypeDef, data: &Value) -> Option<String> {
    let parts: Vec<String> = entity_type_def
        .fields
        .iter()
        .filter(|(_, field_def)| field_def.x_embed)
        .filter_map(|(name, _)| match data.get(name) {
            Some(Value::String(s)) => Some(format!("{name}: {s}")),
            Some(Value::Null) | None => None,
            Some(other) => Some(format!("{name}: {other}")),
        })
        .collect();

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// entityの`x-embed`フィールドから埋め込みベクトルを生成し、`entities.embedding`列を更新する。
/// `x-embed`対象フィールドがスキーマに無い、または値が無い場合は何もせず`Ok(())`を返す
/// （embeddingは補助的な機能であり、entity自体の永続化を妨げてはならないため）。
///
/// 呼び出し側への注意点:
/// - `entities::create`/`entities::update`の両方の後で呼ぶこと。dataが変わるどちらの
///   経路でもembeddingの再生成が必要。
/// - `entities::create`/`update`と同一トランザクションでは呼ばないこと。内部でHTTP経由の
///   embedding API呼び出し（最大30秒）を行うため、同一トランザクション内で呼ぶとその間
///   DB接続・行ロックを保持し続け、コネクションプール枯渇やロック競合を招く。
pub async fn sync_embedding(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    entity_id: Uuid,
    snapshot_updated_at: DateTime<Utc>,
    entity_type_def: &EntityTypeDef,
    data: &Value,
    provider: &dyn EmbeddingProvider,
) -> Result<(), YorishiroError> {
    let Some(text) = compose_embedding_text(entity_type_def, data) else {
        return Ok(());
    };

    let vector = provider.embed(&text).await?;

    // `updated_at`の一致を書き込み条件に含めることで、同一entityへのupdateが連続した際に
    // embedding API呼び出しの所要時間差で完了順が入れ替わっても、古いdataから計算した
    // ベクトルが新しいものを上書きしないようにする（embedding書き込み自体はupdated_atを
    // 変更しないため、この条件が後続の正当な同期を妨げることはない）。
    let result = sqlx::query(
        "UPDATE entities SET embedding = $1 \
         WHERE tenant_id = $2 AND id = $3 AND updated_at = $4",
    )
    .bind(pgvector::Vector::from(vector))
    .bind(tenant_id)
    .bind(entity_id)
    .bind(snapshot_updated_at)
    .execute(&mut *conn)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))?;

    if result.rows_affected() == 0 {
        tracing::debug!(
            %entity_id,
            "sync_embedding: entity was deleted or updated since this snapshot, write skipped"
        );
    }

    Ok(())
}

/// `entities::create`/`update`の戻り値（`EntityRecord`）だけを頼りに、embedding同期に
/// 必要なスキーマ定義を自前で解決して`sync_embedding`を呼ぶ。record内のdataは
/// そのentityが検証されたスキーマバージョン（`record.schema_id`）に属するため、
/// activeバージョンではなくIDでの取得が正しい。
///
/// アダプタ層がレスポンス返却後のバックグラウンドタスクから呼ぶことを想定した入口で、
/// `sync_embedding`と同じくcreate/updateとは別コネクション・別トランザクションで呼ぶこと。
pub async fn sync_embedding_for_record(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    record: &EntityRecord,
    provider: &dyn EmbeddingProvider,
) -> Result<(), YorishiroError> {
    let schema = schemas::get_by_id(conn, tenant_id, record.schema_id).await?;
    let entity_type_def = schema
        .definition
        .entity_types
        .get(&record.entity_type)
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!(
                "entity_type '{}' is not defined in schema '{}'",
                record.entity_type, schema.definition.name
            ),
        })?;

    sync_embedding(
        conn,
        tenant_id,
        record.id,
        record.updated_at,
        entity_type_def,
        &record.data,
        provider,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use serde_json::json;
    use sqlx::PgPool;
    use sqlx::Row;

    use super::*;
    use crate::db::TenantDb;
    use crate::entities::{self, CreateEntityInput};
    use crate::metaschema::MetaSchemaDefinition;
    use crate::schemas;

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

    async fn seed_tenant(pool: &PgPool) -> Uuid {
        let (id,): (Uuid,) = sqlx::query_as("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
            .bind("test-tenant")
            .fetch_one(pool)
            .await
            .unwrap();
        id
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn writes_embedding_for_x_embed_field(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        schemas::create_schema(&mut conn, tenant_id, task_schema_with_embed())
            .await
            .unwrap();

        let entity = entities::create(
            &mut conn,
            tenant_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "write report", "priority": 1 }),
            },
        )
        .await
        .unwrap();

        let schema = schemas::get_by_id(&mut conn, tenant_id, entity.schema_id)
            .await
            .unwrap();
        let entity_type_def = &schema.definition.entity_types["task"];
        let provider = FakeProvider::new(768);

        sync_embedding(
            &mut conn,
            tenant_id,
            entity.id,
            entity.updated_at,
            entity_type_def,
            &entity.data,
            &provider,
        )
        .await
        .unwrap();

        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

        let row = sqlx::query(
            "SELECT embedding IS NOT NULL AS has_embedding FROM entities WHERE id = $1",
        )
        .bind(entity.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        let has_embedding: bool = row.get("has_embedding");
        assert!(has_embedding);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn sync_for_record_resolves_schema_and_writes_embedding(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        schemas::create_schema(&mut conn, tenant_id, task_schema_with_embed())
            .await
            .unwrap();

        let entity = entities::create(
            &mut conn,
            tenant_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "write report" }),
            },
        )
        .await
        .unwrap();

        let provider = FakeProvider::new(768);
        sync_embedding_for_record(&mut conn, tenant_id, &entity, &provider)
            .await
            .unwrap();

        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

        let row = sqlx::query(
            "SELECT embedding IS NOT NULL AS has_embedding FROM entities WHERE id = $1",
        )
        .bind(entity.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        let has_embedding: bool = row.get("has_embedding");
        assert!(has_embedding);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn skips_embedding_when_no_x_embed_field_is_defined(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        schemas::create_schema(&mut conn, tenant_id, task_schema_without_embed())
            .await
            .unwrap();

        let entity = entities::create(
            &mut conn,
            tenant_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "no embedding here" }),
            },
        )
        .await
        .unwrap();

        let schema = schemas::get_by_id(&mut conn, tenant_id, entity.schema_id)
            .await
            .unwrap();
        let entity_type_def = &schema.definition.entity_types["task"];
        let provider = FakeProvider::new(768);

        sync_embedding(
            &mut conn,
            tenant_id,
            entity.id,
            entity.updated_at,
            entity_type_def,
            &entity.data,
            &provider,
        )
        .await
        .unwrap();

        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);

        let row = sqlx::query(
            "SELECT embedding IS NOT NULL AS has_embedding FROM entities WHERE id = $1",
        )
        .bind(entity.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        let has_embedding: bool = row.get("has_embedding");
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
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        schemas::create_schema(&mut conn, tenant_id, task_schema_with_embed())
            .await
            .unwrap();

        let entity = entities::create(
            &mut conn,
            tenant_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "write report" }),
            },
        )
        .await
        .unwrap();

        let schema = schemas::get_by_id(&mut conn, tenant_id, entity.schema_id)
            .await
            .unwrap();
        let entity_type_def = &schema.definition.entity_types["task"];

        let err = sync_embedding(
            &mut conn,
            tenant_id,
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
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        schemas::create_schema(&mut conn, tenant_id, task_schema_with_embed())
            .await
            .unwrap();

        let entity = entities::create(
            &mut conn,
            tenant_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "will be deleted" }),
            },
        )
        .await
        .unwrap();

        let schema = schemas::get_by_id(&mut conn, tenant_id, entity.schema_id)
            .await
            .unwrap();
        let entity_type_def = &schema.definition.entity_types["task"];

        entities::delete(&mut conn, tenant_id, entity.id)
            .await
            .unwrap();

        let provider = FakeProvider::new(768);
        let result = sync_embedding(
            &mut conn,
            tenant_id,
            entity.id,
            entity.updated_at,
            entity_type_def,
            &entity.data,
            &provider,
        )
        .await;

        assert!(result.is_ok());
    }
}
