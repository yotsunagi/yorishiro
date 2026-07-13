use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{Connection, PgConnection};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::metaschema::{self, MetaSchemaDefinition, VersioningDiff, validate_definition};

/// Represents a row in the `schemas` table. `definition` is JSONB in the DB, but the
/// application layer always treats it as a parsed `MetaSchemaDefinition`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SchemaRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub version: i32,
    pub definition: MetaSchemaDefinition,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct SchemaRow {
    id: Uuid,
    tenant_id: Uuid,
    name: String,
    version: i32,
    definition: Value,
    status: String,
    created_at: DateTime<Utc>,
}

impl SchemaRow {
    fn into_record(self) -> Result<SchemaRecord, YorishiroError> {
        let definition = serde_json::from_value(self.definition)
            .map_err(|err| YorishiroError::Internal(err.into()))?;
        Ok(SchemaRecord {
            id: self.id,
            tenant_id: self.tenant_id,
            name: self.name,
            version: self.version,
            definition,
            status: self.status,
            created_at: self.created_at,
        })
    }
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

/// Lists all of a tenant's schemas (every version, including archived) ordered by name
/// and version.
pub async fn list(
    conn: &mut PgConnection,
    tenant_id: Uuid,
) -> Result<Vec<SchemaSummary>, YorishiroError> {
    sqlx::query_as::<_, SchemaSummary>(
        "SELECT id, name, version, status, created_at \
         FROM schemas WHERE tenant_id = $1 ORDER BY name, version",
    )
    .bind(tenant_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))
}

/// Fetches the currently active schema (the latest version with status='active') for
/// the given tenant and name.
pub async fn get_active_schema(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    name: &str,
) -> Result<SchemaRecord, YorishiroError> {
    let row = sqlx::query_as::<_, SchemaRow>(
        "SELECT id, tenant_id, name, version, definition, status, created_at \
         FROM schemas WHERE tenant_id = $1 AND name = $2 AND status = 'active' \
         ORDER BY version DESC LIMIT 1",
    )
    .bind(tenant_id)
    .bind(name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))?;

    match row {
        Some(row) => row.into_record(),
        None => Err(YorishiroError::NotFound {
            message: format!("no active schema named '{name}'"),
        }),
    }
}

/// Fetches a specific schema version by id (used to resolve the version an entity
/// references).
pub async fn get_by_id(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    schema_id: Uuid,
) -> Result<SchemaRecord, YorishiroError> {
    let row = sqlx::query_as::<_, SchemaRow>(
        "SELECT id, tenant_id, name, version, definition, status, created_at \
         FROM schemas WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(schema_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))?;

    match row {
        Some(row) => row.into_record(),
        None => Err(YorishiroError::NotFound {
            message: format!("schema '{schema_id}' was not found"),
        }),
    }
}

/// Registers a new schema definition, after validating it with `validate_definition`. If no
/// schema of this name exists yet, creates version 1 as active; otherwise computes a
/// a `versioning::diff`, archives the previous active version, and always inserts
/// the new definition as the next version (reporting whether the diff is breaking).
///
/// Concurrent creates for the same (tenant_id, name) are serialized with an advisory lock:
/// without it, reading the active version and then archiving-it-plus-inserting the new one
/// would race, letting concurrent calls fail on the UNIQUE(tenant_id, name, version)
/// constraint or archive a version another call just committed as active.
pub async fn create_schema(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    definition: MetaSchemaDefinition,
) -> Result<(SchemaRecord, VersioningDiff), YorishiroError> {
    validate_definition(&definition)?;

    let name = definition.name.clone();

    let mut tx = conn
        .begin()
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("{tenant_id}:{name}"))
        .execute(&mut *tx)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    let previous_row = sqlx::query_as::<_, SchemaRow>(
        "SELECT id, tenant_id, name, version, definition, status, created_at \
         FROM schemas WHERE tenant_id = $1 AND name = $2 AND status = 'active' \
         ORDER BY version DESC LIMIT 1",
    )
    .bind(tenant_id)
    .bind(&name)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))?;

    let (next_version, diff) = match previous_row {
        Some(row) => {
            let previous = row.into_record()?;
            let diff = metaschema::diff(&previous.definition, &definition);
            (previous.version + 1, diff)
        }
        None => (
            1,
            VersioningDiff {
                is_breaking: false,
                reasons: Vec::new(),
            },
        ),
    };

    sqlx::query(
        "UPDATE schemas SET status = 'archived' \
         WHERE tenant_id = $1 AND name = $2 AND status = 'active'",
    )
    .bind(tenant_id)
    .bind(&name)
    .execute(&mut *tx)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))?;

    let definition_json =
        serde_json::to_value(&definition).map_err(|err| YorishiroError::Internal(err.into()))?;

    let row = sqlx::query_as::<_, SchemaRow>(
        "INSERT INTO schemas (tenant_id, name, version, definition, status) \
         VALUES ($1, $2, $3, $4, 'active') \
         RETURNING id, tenant_id, name, version, definition, status, created_at",
    )
    .bind(tenant_id)
    .bind(&name)
    .bind(next_version)
    .bind(definition_json)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if err
            .as_database_error()
            .is_some_and(|db_err| db_err.is_unique_violation())
        {
            YorishiroError::Conflict {
                message: format!(
                    "schema '{name}' version {next_version} already exists (concurrent create?)"
                ),
            }
        } else {
            YorishiroError::Internal(err.into())
        }
    })?;

    tx.commit()
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    Ok((row.into_record()?, diff))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sqlx::PgPool;

    use super::*;
    use crate::db::TenantDb;

    fn task_schema(with_priority: bool) -> MetaSchemaDefinition {
        let fields = if with_priority {
            json!({
                "title": { "type": "string", "required": true },
                "priority": { "type": "integer" }
            })
        } else {
            json!({ "title": { "type": "string", "required": true } })
        };
        serde_json::from_value(json!({
            "name": "task-management",
            "entity_types": { "task": { "fields": fields } }
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
    async fn creates_first_version_as_active(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();

        let (record, diff) = create_schema(&mut conn, tenant_id, task_schema(false))
            .await
            .unwrap();
        assert_eq!(record.version, 1);
        assert_eq!(record.status, "active");
        assert!(!diff.is_breaking);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn creating_new_version_archives_previous_and_reports_breaking_diff(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();

        let (v1, _) = create_schema(&mut conn, tenant_id, task_schema(false))
            .await
            .unwrap();

        let mut required_priority = task_schema(true);
        required_priority
            .entity_types
            .get_mut("task")
            .unwrap()
            .fields
            .get_mut("priority")
            .unwrap()
            .required = true;

        let (v2, diff) = create_schema(&mut conn, tenant_id, required_priority)
            .await
            .unwrap();
        assert_eq!(v2.version, 2);
        assert!(diff.is_breaking, "reasons: {:?}", diff.reasons);

        let archived = get_by_id(&mut conn, tenant_id, v1.id).await.unwrap();
        assert_eq!(archived.status, "archived");

        let active = get_active_schema(&mut conn, tenant_id, "task-management")
            .await
            .unwrap();
        assert_eq!(active.id, v2.id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn get_active_schema_reports_not_found_when_absent(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();

        let err = get_active_schema(&mut conn, tenant_id, "does-not-exist")
            .await
            .unwrap_err();
        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }
}
