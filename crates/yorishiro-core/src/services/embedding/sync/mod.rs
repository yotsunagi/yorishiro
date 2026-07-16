use chrono::{DateTime, Utc};
use sea_query::{Alias, Expr, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::EntityTypeDef;
use crate::models::entities::EntityRecord;
use crate::repositories::schemas;
use crate::services::embedding::EmbeddingProvider;

#[derive(Iden)]
enum Entities {
    Table,
    Id,
    WorkspaceId,
    UpdatedAt,
    Embedding,
}

/// Concatenates the values of `x-embed` fields as `"field: value"` to build the
/// text to embed. Field names are kept because bare values would lose semantic
/// context that helps the embedding model, compared to concatenating raw
/// values alone. Returns `None` when there are no such fields or all are
/// absent, so callers can skip the embedding API call entirely.
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

/// Generates an embedding vector from an entity's `x-embed` fields and updates
/// the `entities.embedding` column. Returns `Ok(())` without doing anything if
/// the schema has no `x-embed` fields or none have values (embedding is an
/// auxiliary feature and must never block persisting the entity itself).
///
/// Notes for callers:
/// - Call this after both `entities::create` and `entities::update`; either
///   path changes `data` and requires regenerating the embedding.
/// - Do not call this within the same transaction as `entities::create`/`update`.
///   It performs an embedding API call over HTTP (up to 30s), and holding a DB
///   connection and row locks for that long risks connection pool exhaustion
///   and lock contention.
pub async fn sync_embedding(
    conn: &mut PgConnection,
    workspace_id: Uuid,
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

    // Including the `updated_at` match as a write condition prevents a vector
    // computed from stale data from overwriting a newer one when consecutive
    // updates to the same entity complete out of order due to differing
    // embedding API latencies (writing the embedding itself doesn't change
    // `updated_at`, so this condition never blocks a subsequent legitimate sync).
    let (sql, values) = Query::update()
        .table((Alias::new("content"), Entities::Table))
        .values([(Entities::Embedding, pgvector::Vector::from(vector).into())])
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Entities::Id).eq(entity_id))
        .and_where(Expr::col(Entities::UpdatedAt).eq(snapshot_updated_at))
        .build_sqlx(PostgresQueryBuilder);

    let result = sqlx::query_with(&sql, values)
        .execute(&mut *conn)
        .await
        .internal()?;

    if result.rows_affected() == 0 {
        tracing::debug!(
            %entity_id,
            "sync_embedding: entity was deleted or updated since this snapshot, write skipped"
        );
    }

    Ok(())
}

/// Resolves the schema definition needed for embedding sync on its own,
/// relying only on the return value of `entities::create`/`update`
/// (`EntityRecord`), then calls `sync_embedding`. The record's data belongs to
/// the schema version it was validated against (`record.schema_id`), so
/// fetching by ID rather than the active version is correct.
///
/// This is the intended entry point for adapter layers to call from a
/// background task after returning a response; like `sync_embedding`, call it
/// from a separate connection/transaction than create/update.
pub async fn sync_embedding_for_record(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    record: &EntityRecord,
    provider: &dyn EmbeddingProvider,
) -> Result<(), YorishiroError> {
    let schema = schemas::get_by_id(conn, workspace_id, record.schema_id).await?;
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
        workspace_id,
        record.id,
        record.updated_at,
        entity_type_def,
        &record.data,
        provider,
    )
    .await
}

#[cfg(test)]
mod tests;
