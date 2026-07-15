use chrono::{DateTime, Utc};
use sea_query::extension::postgres::{PgBinOper, PgExpr};
use sea_query::{Alias, BinOper, Expr, Func, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde::Serialize;
use serde_json::Value;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::embedding::EmbeddingProvider;
use crate::entities::EntityRecord;
use crate::error::YorishiroError;

#[derive(Iden)]
enum Entities {
    Table,
    Id,
    WorkspaceId,
    SchemaId,
    SchemaVersion,
    EntityType,
    Data,
    Embedding,
    CreatedAt,
    UpdatedAt,
    CreatedBy,
    UpdatedBy,
}

const DEFAULT_SEARCH_LIMIT: i64 = 10;

pub struct SearchQuery {
    pub entity_type: Option<String>,
    /// JSONB containment filter (`data @> filter`), e.g. `{"status": "active"}`.
    pub filter: Option<Value>,
    pub limit: i64,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            entity_type: None,
            filter: None,
            limit: DEFAULT_SEARCH_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SearchHit {
    pub entity: EntityRecord,
    /// pgvector cosine distance (the `<=>` operator). Closer to 0 means more similar. `None`
    /// when the entity has no embedding and was only surfaced through the pg_trgm fuzzy
    /// text match on `query_text`.
    pub distance: Option<f64>,
}

#[derive(sqlx::FromRow)]
struct SearchRow {
    id: Uuid,
    workspace_id: Uuid,
    schema_id: Uuid,
    schema_version: i32,
    entity_type: String,
    data: Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    created_by: Option<Uuid>,
    updated_by: Option<Uuid>,
    distance: Option<f64>,
}

impl SearchRow {
    fn into_hit(self) -> SearchHit {
        SearchHit {
            entity: EntityRecord {
                id: self.id,
                workspace_id: self.workspace_id,
                schema_id: self.schema_id,
                schema_version: self.schema_version,
                entity_type: self.entity_type,
                data: self.data,
                created_at: self.created_at,
                updated_at: self.updated_at,
                created_by: self.created_by,
                updated_by: self.updated_by,
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
/// first. As an auxiliary path, entities with no embedding are also included when
/// `query_text` is a pg_trgm fuzzy match (`data::text % query_text`) against their data —
/// this catches keyword/typo matches that vector search would miss (e.g. entity_types with
/// no `x-embed` field, or embedding generation that hasn't run yet). Vector matches are
/// always ranked ahead of trgm-only matches; trgm-only matches are ordered by similarity.
pub async fn search_by_vector(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    vector: Vec<f32>,
    query_text: &str,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    let limit = query.limit.clamp(1, 200);

    let distance = Expr::col(Entities::Embedding).binary(
        BinOper::PgOperator(PgBinOper::CosineDistance),
        Expr::val(pgvector::Vector::from(vector)),
    );
    let data_as_text = Expr::col(Entities::Data).cast_as(Alias::new("text"));
    let similarity = Func::cust(Alias::new("similarity"))
        .args([data_as_text.clone(), Expr::val(query_text).into()]);

    let mut select = Query::select();
    select
        .columns([
            Entities::Id,
            Entities::WorkspaceId,
            Entities::SchemaId,
            Entities::SchemaVersion,
            Entities::EntityType,
            Entities::Data,
            Entities::CreatedAt,
            Entities::UpdatedAt,
            Entities::CreatedBy,
            Entities::UpdatedBy,
        ])
        .expr_as(distance.clone(), Alias::new("distance"))
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .and_where(
            Expr::col(Entities::Embedding)
                .is_not_null()
                .or(data_as_text.binary(
                    BinOper::PgOperator(PgBinOper::Similarity),
                    Expr::val(query_text),
                )),
        )
        .order_by_expr(Expr::col(Entities::Embedding).is_null(), Order::Asc)
        .order_by_expr(distance, Order::Asc)
        .order_by_expr(similarity.into(), Order::Desc)
        .limit(limit as u64);

    if let Some(entity_type) = query.entity_type {
        select.and_where(Expr::col(Entities::EntityType).eq(entity_type));
    }
    if let Some(filter) = query.filter {
        select.and_where(Expr::col(Entities::Data).contains(Expr::val(filter)));
    }

    let (sql, values) = select.build_sqlx(PostgresQueryBuilder);

    let rows = sqlx::query_as_with::<_, SearchRow, _>(&sql, values)
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
    workspace_id: Uuid,
    provider: &dyn EmbeddingProvider,
    query_text: &str,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    let vector = embed_query(provider, query_text).await?;
    search_by_vector(conn, workspace_id, vector, query_text, query).await
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
}
