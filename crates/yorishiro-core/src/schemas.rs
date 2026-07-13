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
    pub workspace_id: Uuid,
    pub name: String,
    pub version: i32,
    pub definition: MetaSchemaDefinition,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct SchemaRow {
    id: Uuid,
    workspace_id: Uuid,
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
            workspace_id: self.workspace_id,
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
    workspace_id: Uuid,
) -> Result<Vec<SchemaSummary>, YorishiroError> {
    sqlx::query_as::<_, SchemaSummary>(
        "SELECT id, name, version, status, created_at \
         FROM content.schemas WHERE workspace_id = $1 ORDER BY name, version",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))
}

/// Fetches the currently active schema (the latest version with status='active') for
/// the given tenant and name.
pub async fn get_active_schema(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    name: &str,
) -> Result<SchemaRecord, YorishiroError> {
    let row = sqlx::query_as::<_, SchemaRow>(
        "SELECT id, workspace_id, name, version, definition, status, created_at \
         FROM content.schemas WHERE workspace_id = $1 AND name = $2 AND status = 'active' \
         ORDER BY version DESC LIMIT 1",
    )
    .bind(workspace_id)
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
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<SchemaRecord, YorishiroError> {
    let row = sqlx::query_as::<_, SchemaRow>(
        "SELECT id, workspace_id, name, version, definition, status, created_at \
         FROM content.schemas WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
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

/// Fetches every schema version for the tenant (including archived), with no pagination
/// limit and the full `definition` body, for a full-tenant export.
pub async fn export_all(
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<Vec<SchemaRecord>, YorishiroError> {
    let rows = sqlx::query_as::<_, SchemaRow>(
        "SELECT id, workspace_id, name, version, definition, status, created_at \
         FROM content.schemas WHERE workspace_id = $1 ORDER BY name, version",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))?;

    rows.into_iter().map(SchemaRow::into_record).collect()
}

/// Registers a new schema definition, after validating it with `validate_definition`. If no
/// schema of this name exists yet, creates version 1 as active; otherwise computes a
/// a `versioning::diff`, archives the previous active version, and always inserts
/// the new definition as the next version (reporting whether the diff is breaking).
///
/// Concurrent creates for the same (workspace_id, name) are serialized with an advisory lock:
/// without it, reading the active version and then archiving-it-plus-inserting the new one
/// would race, letting concurrent calls fail on the UNIQUE(workspace_id, name, version)
/// constraint or archive a version another call just committed as active.
pub async fn create_schema(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    definition: MetaSchemaDefinition,
) -> Result<(SchemaRecord, VersioningDiff), YorishiroError> {
    validate_definition(&definition)?;

    let name = definition.name.clone();

    let mut tx = conn
        .begin()
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("{workspace_id}:{name}"))
        .execute(&mut *tx)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    let previous_row = sqlx::query_as::<_, SchemaRow>(
        "SELECT id, workspace_id, name, version, definition, status, created_at \
         FROM content.schemas WHERE workspace_id = $1 AND name = $2 AND status = 'active' \
         ORDER BY version DESC LIMIT 1",
    )
    .bind(workspace_id)
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
        "UPDATE content.schemas SET status = 'archived' \
         WHERE workspace_id = $1 AND name = $2 AND status = 'active'",
    )
    .bind(workspace_id)
    .bind(&name)
    .execute(&mut *tx)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))?;

    let definition_json =
        serde_json::to_value(&definition).map_err(|err| YorishiroError::Internal(err.into()))?;

    let row = sqlx::query_as::<_, SchemaRow>(
        "INSERT INTO content.schemas (workspace_id, name, version, definition, status) \
         VALUES ($1, $2, $3, $4, 'active') \
         RETURNING id, workspace_id, name, version, definition, status, created_at",
    )
    .bind(workspace_id)
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

    async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
        let (tenant_id,): (Uuid,) =
            sqlx::query_as("INSERT INTO identity.tenants (name) VALUES ($1) RETURNING id")
                .bind("test-tenant")
                .fetch_one(pool)
                .await
                .unwrap();
        let (workspace_id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO identity.workspaces (tenant_id, name) VALUES ($1, $2) RETURNING id",
        )
        .bind(tenant_id)
        .bind("test-workspace")
        .fetch_one(pool)
        .await
        .unwrap();
        (tenant_id, workspace_id)
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn creates_first_version_as_active(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();

        let (record, diff) = create_schema(&mut conn, workspace_id, task_schema(false))
            .await
            .unwrap();
        assert_eq!(record.version, 1);
        assert_eq!(record.status, "active");
        assert!(!diff.is_breaking);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn creating_new_version_archives_previous_and_reports_breaking_diff(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();

        let (v1, _) = create_schema(&mut conn, workspace_id, task_schema(false))
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

        let (v2, diff) = create_schema(&mut conn, workspace_id, required_priority)
            .await
            .unwrap();
        assert_eq!(v2.version, 2);
        assert!(diff.is_breaking, "reasons: {:?}", diff.reasons);

        let archived = get_by_id(&mut conn, workspace_id, v1.id).await.unwrap();
        assert_eq!(archived.status, "archived");

        let active = get_active_schema(&mut conn, workspace_id, "task-management")
            .await
            .unwrap();
        assert_eq!(active.id, v2.id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn get_active_schema_reports_not_found_when_absent(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();

        let err = get_active_schema(&mut conn, workspace_id, "does-not-exist")
            .await
            .unwrap_err();
        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }
}
