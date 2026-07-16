use chrono::{DateTime, Utc};
use sea_query::extension::postgres::{PgBinOper, PgExpr};
use sea_query::{Alias, BinOper, Expr, Func, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::entities::EntityRecord;
use crate::services::embedding::EmbeddingProvider;

pub use crate::models::search::*;

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
        .internal()?;

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
mod tests;
