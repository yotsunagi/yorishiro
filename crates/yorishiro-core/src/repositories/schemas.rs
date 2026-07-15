use chrono::{DateTime, Utc};
use sea_query::{Alias, Expr, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde_json::Value;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::metaschema::{self, MetaSchemaDefinition, VersioningDiff, validate_definition};

pub use crate::models::schemas::*;

#[derive(Iden)]
enum Schemas {
    Table,
    Id,
    WorkspaceId,
    Name,
    Version,
    Definition,
    Status,
    CreatedAt,
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

/// Lists all of a tenant's schemas (every version, including archived) ordered by name
/// and version.
pub async fn list(
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<Vec<SchemaSummary>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns([
            Schemas::Id,
            Schemas::Name,
            Schemas::Version,
            Schemas::Status,
            Schemas::CreatedAt,
        ])
        .from((Alias::new("content"), Schemas::Table))
        .and_where(Expr::col(Schemas::WorkspaceId).eq(workspace_id))
        .order_by(Schemas::Name, Order::Asc)
        .order_by(Schemas::Version, Order::Asc)
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, SchemaSummary, _>(&sql, values)
        .fetch_all(&mut *conn)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))
}

fn schema_columns() -> [Schemas; 7] {
    [
        Schemas::Id,
        Schemas::WorkspaceId,
        Schemas::Name,
        Schemas::Version,
        Schemas::Definition,
        Schemas::Status,
        Schemas::CreatedAt,
    ]
}

/// Fetches the currently active schema (the latest version with status='active') for
/// the given tenant and name.
pub async fn get_active_schema(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    name: &str,
) -> Result<SchemaRecord, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(schema_columns())
        .from((Alias::new("content"), Schemas::Table))
        .and_where(Expr::col(Schemas::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Schemas::Name).eq(name))
        .and_where(Expr::col(Schemas::Status).eq("active"))
        .order_by(Schemas::Version, Order::Desc)
        .limit(1)
        .build_sqlx(PostgresQueryBuilder);
    let row: Option<SchemaRow> = sqlx::query_as_with(&sql, values)
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
    let (sql, values) = Query::select()
        .columns(schema_columns())
        .from((Alias::new("content"), Schemas::Table))
        .and_where(Expr::col(Schemas::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Schemas::Id).eq(schema_id))
        .build_sqlx(PostgresQueryBuilder);
    let row: Option<SchemaRow> = sqlx::query_as_with(&sql, values)
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
    let (sql, values) = Query::select()
        .columns(schema_columns())
        .from((Alias::new("content"), Schemas::Table))
        .and_where(Expr::col(Schemas::WorkspaceId).eq(workspace_id))
        .order_by(Schemas::Name, Order::Asc)
        .order_by(Schemas::Version, Order::Asc)
        .build_sqlx(PostgresQueryBuilder);
    let rows: Vec<SchemaRow> = sqlx::query_as_with(&sql, values)
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

    // `pg_advisory_xact_lock(...)` is a lock-acquisition function call, not a table operation --
    // no SELECT/INSERT/UPDATE/DELETE form exists for sea-query to build, same category as the
    // session commands in `db.rs`/`auth.rs`.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("{workspace_id}:{name}"))
        .execute(&mut *tx)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    let (sql, values) = Query::select()
        .columns(schema_columns())
        .from((Alias::new("content"), Schemas::Table))
        .and_where(Expr::col(Schemas::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Schemas::Name).eq(&name))
        .and_where(Expr::col(Schemas::Status).eq("active"))
        .order_by(Schemas::Version, Order::Desc)
        .limit(1)
        .build_sqlx(PostgresQueryBuilder);
    let previous_row: Option<SchemaRow> = sqlx::query_as_with(&sql, values)
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

    let (sql, values) = Query::update()
        .table((Alias::new("content"), Schemas::Table))
        .values([(Schemas::Status, "archived".into())])
        .and_where(Expr::col(Schemas::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Schemas::Name).eq(&name))
        .and_where(Expr::col(Schemas::Status).eq("active"))
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values)
        .execute(&mut *tx)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    let definition_json =
        serde_json::to_value(&definition).map_err(|err| YorishiroError::Internal(err.into()))?;

    let (sql, values) = Query::insert()
        .into_table((Alias::new("content"), Schemas::Table))
        .columns([
            Schemas::WorkspaceId,
            Schemas::Name,
            Schemas::Version,
            Schemas::Definition,
            Schemas::Status,
        ])
        .values_panic([
            workspace_id.into(),
            name.clone().into(),
            next_version.into(),
            definition_json.into(),
            "active".into(),
        ])
        .returning(Query::returning().columns(schema_columns()))
        .build_sqlx(PostgresQueryBuilder);
    let row: SchemaRow = sqlx::query_as_with(&sql, values)
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
        crate::test_support::seed_tenant_and_workspace(pool).await
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
