use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::metaschema::MetaSchemaDefinition;

/// Represents a row in the `schemas` table. `definition` is JSONB in the DB, but the
/// application layer always treats it as a parsed `MetaSchemaDefinition`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SchemaRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub version: i32,
    pub definition: MetaSchemaDefinition,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

/// A row in a schema listing. A lightweight summary that omits the `definition` body,
/// used as the entry point for MCP clients (LLMs) to discover what schemas exist for a
/// tenant.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct SchemaSummary {
    pub id: Uuid,
    pub name: String,
    pub version: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
}
