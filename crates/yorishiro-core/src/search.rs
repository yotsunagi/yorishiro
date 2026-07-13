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
    /// pgvector cosine distance (the `<=>` operator). Closer to 0 means more similar.
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

/// Converts query text into an embedding vector; used together with `search_by_vector`. On
/// request paths, call this before acquiring a DB connection: embedding generation can take
/// a long time (external API calls or waiting on serialized local inference), and holding a
/// connection while waiting would let pool exhaustion spill over onto unrelated endpoints.
pub async fn embed_query(
    provider: &dyn EmbeddingProvider,
    query_text: &str,
) -> Result<Vec<f32>, YorishiroError> {
    provider.embed(query_text).await
}

/// Returns entities ordered by cosine distance between the given embedding vector and the
/// `entities.embedding` column (using the `entities_embedding_hnsw` HNSW index), closest
/// first. Entities with no embedding (NULL) are excluded — either because their entity_type
/// has no x-embed field, or because embedding generation hasn't run yet.
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

/// Composes `embed_query` + `search_by_vector`. Because this holds `conn` for the duration
/// of embedding generation, don't use it on request paths — reserve it for tests and batch
/// jobs where holding a connection isn't a problem (request handlers call `embed_query`
/// before acquiring a connection).
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
        // compose_embedding_text builds text in "field: value" form, so the fixture key matches that format.
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

        // project has a vector closer to the query, but we filter to entity_type=task.
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

        // embedding stays NULL since sync_embedding is never called.
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
