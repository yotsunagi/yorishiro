use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::embedding::EmbeddingProvider;
use crate::entities::EntityRecord;
use crate::error::YorishiroError;

const DEFAULT_SEARCH_LIMIT: i64 = 10;

pub struct SearchQuery {
    pub entity_type: Option<String>,
    pub limit: i64,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            entity_type: None,
            limit: DEFAULT_SEARCH_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SearchHit {
    pub entity: EntityRecord,
    /// pgvectorのコサイン距離（`<=>`演算子）。0に近いほど類似している。
    pub distance: f64,
}

#[derive(sqlx::FromRow)]
struct SearchRow {
    id: Uuid,
    tenant_id: Uuid,
    schema_id: Uuid,
    schema_version: i32,
    entity_type: String,
    data: Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    distance: f64,
}

impl SearchRow {
    fn into_hit(self) -> SearchHit {
        SearchHit {
            entity: EntityRecord {
                id: self.id,
                tenant_id: self.tenant_id,
                schema_id: self.schema_id,
                schema_version: self.schema_version,
                entity_type: self.entity_type,
                data: self.data,
                created_at: self.created_at,
                updated_at: self.updated_at,
            },
            distance: self.distance,
        }
    }
}

/// クエリテキストを埋め込みベクトルへ変換する。`search_by_vector`と組にして使う。
/// リクエスト経路ではDBコネクションを取得する前にこちらを先に呼ぶこと:
/// 埋め込み生成は外部API呼び出しやローカル推論の直列化待ちで長時間かかりうるため、
/// コネクションを保持したまま待つとプール枯渇が無関係なエンドポイントへ波及する。
pub async fn embed_query(
    provider: &dyn EmbeddingProvider,
    query_text: &str,
) -> Result<Vec<f32>, YorishiroError> {
    provider.embed(query_text).await
}

/// 埋め込み済みのクエリベクトルで、`entities.embedding`列に対するコサイン距離
/// （HNSWインデックス`entities_embedding_hnsw`利用）で近い順にentityを返す。
/// embeddingが未設定（NULL）のentityは対象外（x-embedフィールドを持たない
/// entity_typeや、embedding生成がまだ行われていないentity）。
pub async fn search_by_vector(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    vector: Vec<f32>,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    let limit = query.limit.clamp(1, 200);

    let rows = sqlx::query_as::<_, SearchRow>(
        "SELECT id, tenant_id, schema_id, schema_version, entity_type, data, created_at, updated_at, \
                embedding <=> $1 AS distance \
         FROM entities \
         WHERE tenant_id = $2 \
           AND embedding IS NOT NULL \
           AND ($3::text IS NULL OR entity_type = $3) \
         ORDER BY embedding <=> $1 \
         LIMIT $4",
    )
    .bind(pgvector::Vector::from(vector))
    .bind(tenant_id)
    .bind(query.entity_type)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))?;

    Ok(rows.into_iter().map(SearchRow::into_hit).collect())
}

/// `embed_query` + `search_by_vector`の合成。埋め込み生成中も`conn`を保持し続けるため、
/// リクエスト経路では使わず、コネクション保持が問題にならないテスト・バッチ用途に限ること
/// （リクエスト経路のハンドラはコネクション取得前に`embed_query`を呼ぶ）。
pub async fn search_by_text(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    provider: &dyn EmbeddingProvider,
    query_text: &str,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    let vector = embed_query(provider, query_text).await?;
    search_by_vector(conn, tenant_id, vector, query).await
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::json;
    use sqlx::PgPool;
    use std::collections::HashMap;

    use super::*;
    use crate::db::TenantDb;
    use crate::embedding_sync;
    use crate::entities::{self, CreateEntityInput};
    use crate::metaschema::MetaSchemaDefinition;
    use crate::schemas;

    const DIM: usize = 768;

    fn unit_vector(index: usize) -> Vec<f32> {
        let mut v = vec![0.0_f32; DIM];
        v[index] = 1.0;
        v
    }

    /// テキスト→ベクトルの対応関係をテストごとに明示的に決められるフェイクプロバイダ。
    /// 未登録のテキストが渡されたらpanicし、テストの前提崩れを即座に検出する。
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

    async fn seed_tenant(pool: &PgPool) -> Uuid {
        let (id,): (Uuid,) = sqlx::query_as("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
            .bind("test-tenant")
            .fetch_one(pool)
            .await
            .unwrap();
        id
    }

    async fn seed_embedded_entity(
        conn: &mut PgConnection,
        tenant_id: Uuid,
        entity_type: &str,
        title: &str,
        vector: Vec<f32>,
    ) -> entities::EntityRecord {
        let entity = entities::create(
            conn,
            tenant_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: entity_type.into(),
                data: json!({ "title": title }),
            },
        )
        .await
        .unwrap();

        let schema = schemas::get_by_id(conn, tenant_id, entity.schema_id)
            .await
            .unwrap();
        let entity_type_def = &schema.definition.entity_types[entity_type];
        // compose_embedding_textは"field: value"形式でテキストを合成するため、
        // フィクスチャのキーもそれに合わせる。
        let provider = MapProvider::new([(format!("title: {title}"), vector)]);

        embedding_sync::sync_embedding(
            conn,
            tenant_id,
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
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        schemas::create_schema(&mut conn, tenant_id, task_schema_with_embed())
            .await
            .unwrap();

        let apple = seed_embedded_entity(
            &mut conn,
            tenant_id,
            "task",
            "apple pie recipe",
            unit_vector(0),
        )
        .await;
        let car = seed_embedded_entity(
            &mut conn,
            tenant_id,
            "task",
            "car engine repair",
            unit_vector(1),
        )
        .await;

        let query_provider = MapProvider::new([("fruit dessert", unit_vector(0))]);
        let hits = search_by_text(
            &mut conn,
            tenant_id,
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
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        schemas::create_schema(&mut conn, tenant_id, task_schema_with_embed())
            .await
            .unwrap();

        // projectのほうがクエリに近いベクトルを持つが、entity_typeでtaskに絞る。
        let task =
            seed_embedded_entity(&mut conn, tenant_id, "task", "distant task", unit_vector(5))
                .await;
        seed_embedded_entity(
            &mut conn,
            tenant_id,
            "project",
            "close project",
            unit_vector(0),
        )
        .await;

        let query_provider = MapProvider::new([("query", unit_vector(0))]);
        let hits = search_by_text(
            &mut conn,
            tenant_id,
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
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        schemas::create_schema(&mut conn, tenant_id, task_schema_with_embed())
            .await
            .unwrap();

        // sync_embeddingを呼ばないため、embeddingはNULLのまま。
        entities::create(
            &mut conn,
            tenant_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "never embedded" }),
            },
        )
        .await
        .unwrap();

        let query_provider = MapProvider::new([("query", unit_vector(0))]);
        let hits = search_by_text(
            &mut conn,
            tenant_id,
            &query_provider,
            "query",
            SearchQuery::default(),
        )
        .await
        .unwrap();

        assert!(hits.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn enforces_tenant_isolation(pool: PgPool) {
        let tenant_a = seed_tenant(&pool).await;
        let tenant_b = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);

        let mut conn_a = db.acquire_for_tenant(tenant_a).await.unwrap();
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

        let mut conn_b = db.acquire_for_tenant(tenant_b).await.unwrap();
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
}
