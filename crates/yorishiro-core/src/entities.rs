use chrono::{DateTime, Utc};
use sea_query::extension::postgres::PgExpr;
use sea_query::{Alias, Expr, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde::Serialize;
use serde_json::Value;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ValidationDetail, YorishiroError};
use crate::metaschema;
use crate::schemas;

#[derive(Iden)]
enum Entities {
    Table,
    Id,
    WorkspaceId,
    SchemaId,
    SchemaVersion,
    EntityType,
    Data,
    CreatedAt,
    UpdatedAt,
    CreatedBy,
    UpdatedBy,
}

/// A row in the `entities` table. `embedding` is managed separately by the
/// search/embedding pipeline, so this module's CRUD doesn't touch it. `created_by`/
/// `updated_by` are `None` for entities touched by an unattributed (service/automation) API
/// key, since there's no user to record.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct EntityRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub schema_id: Uuid,
    pub schema_version: i32,
    pub entity_type: String,
    #[schema(value_type = Object)]
    pub data: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub updated_by: Option<Uuid>,
}

pub struct CreateEntityInput {
    pub schema_name: String,
    pub entity_type: String,
    pub data: Value,
}

const DEFAULT_LIST_LIMIT: i64 = 50;

pub struct ListEntitiesQuery {
    pub entity_type: Option<String>,
    /// JSONB containment filter (`data @> filter`), e.g. `{"status": "active"}`.
    pub filter: Option<Value>,
    pub limit: i64,
    pub offset: i64,
}

impl Default for ListEntitiesQuery {
    fn default() -> Self {
        Self {
            entity_type: None,
            filter: None,
            limit: DEFAULT_LIST_LIMIT,
            offset: 0,
        }
    }
}

/// Escapes `~`/`/` per RFC 6901 before embedding a value as a JSON Pointer segment.
fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// Represents where a validation error occurred as a JSON Pointer. For `required`
/// violations, `instance_path()` alone only points at the containing object and doesn't
/// say which property is missing, so the missing property name is appended.
fn error_field_pointer(err: &jsonschema::ValidationError<'_>) -> String {
    let base = err.instance_path().to_string();
    if let jsonschema::error::ValidationErrorKind::Required { property } = err.kind()
        && let Some(name) = property.as_str()
    {
        format!("{base}/{}", escape_pointer_segment(name))
    } else {
        base
    }
}

/// Validates `data` against the JSON Schema generated from the entity_type definition.
/// Reuses `entity_type_to_json_schema`'s schema as-is so validation logic isn't duplicated
/// between entities and the MCP inputSchema.
fn validate_data(
    entity_type_def: &metaschema::EntityTypeDef,
    data: &Value,
) -> Result<(), YorishiroError> {
    let schema = metaschema::entity_type_to_json_schema(entity_type_def);
    let validator = jsonschema::validator_for(&schema)
        .map_err(|err| YorishiroError::Internal(anyhow::anyhow!(err.to_string())))?;

    let details: Vec<ValidationDetail> = validator
        .iter_errors(data)
        .map(|err| ValidationDetail {
            field: error_field_pointer(&err),
            problem: err.to_string(),
        })
        .collect();

    if details.is_empty() {
        Ok(())
    } else {
        Err(YorishiroError::ValidationFailed {
            message: "entity data does not conform to its schema".into(),
            details,
            hint: "Check the entity_type field definitions against the submitted data".into(),
        })
    }
}

fn resolve_entity_type<'a>(
    definition: &'a metaschema::MetaSchemaDefinition,
    entity_type: &str,
) -> Result<&'a metaschema::EntityTypeDef, YorishiroError> {
    definition
        .entity_types
        .get(entity_type)
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!(
                "entity_type '{entity_type}' is not defined in schema '{}'",
                definition.name
            ),
        })
}

/// Checks the workspace's `max_entities` cap (billing/quota enforcement) before an insert.
/// `NULL` means unlimited, which is the default so self-hosted deployments are never capped
/// unless an operator explicitly sets a limit. The app role only has SELECT on
/// `identity.workspaces`, which is enough to read this column without needing write access to
/// the control-plane schema.
async fn check_entity_quota(
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    let max_entities: Option<i32> =
        sqlx::query_scalar("SELECT max_entities FROM identity.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|err| YorishiroError::Internal(err.into()))?
            .flatten();

    let Some(max) = max_entities else {
        return Ok(());
    };

    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM content.entities WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(|err| YorishiroError::Internal(err.into()))?;

    if count >= i64::from(max) {
        Err(YorishiroError::Conflict {
            message: format!(
                "workspace '{workspace_id}' has reached its entity limit ({max}); \
                 raise max_entities or delete existing entities"
            ),
        })
    } else {
        Ok(())
    }
}

/// Creates a new entity: resolves the schema name to its currently active schema, checks
/// that the entity_type exists in that version, validates `data`, and persists the result.
/// `created_by` is the acting user's ID (from `AuthContext::user_id`), or `None` for an
/// unattributed service/automation API key.
pub async fn create(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    input: CreateEntityInput,
    created_by: Option<Uuid>,
) -> Result<EntityRecord, YorishiroError> {
    check_entity_quota(conn, workspace_id).await?;
    let schema = schemas::get_active_schema(conn, workspace_id, &input.schema_name).await?;
    let entity_type_def = resolve_entity_type(&schema.definition, &input.entity_type)?;
    validate_data(entity_type_def, &input.data)?;

    let (sql, values) = Query::insert()
        .into_table((Alias::new("content"), Entities::Table))
        .columns([
            Entities::WorkspaceId,
            Entities::SchemaId,
            Entities::SchemaVersion,
            Entities::EntityType,
            Entities::Data,
            Entities::CreatedBy,
        ])
        .values_panic([
            workspace_id.into(),
            schema.id.into(),
            schema.version.into(),
            input.entity_type.into(),
            input.data.into(),
            created_by.into(),
        ])
        .returning(Query::returning().columns([
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
        ]))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, EntityRecord, _>(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))
}

pub async fn get(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<EntityRecord, YorishiroError> {
    let (sql, values) = Query::select()
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
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Entities::Id).eq(id))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, EntityRecord, _>(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!("entity '{id}' was not found"),
        })
}

/// Fully replaces an existing entity's `data`. Validation is done against the schema
/// version the entity was actually created with (i.e. the row `entities.schema_id` points
/// to), so existing entities don't silently break compatibility even if the active version
/// has since moved on.
/// `updated_by` is the acting user's ID, or `None` for an unattributed service/automation
/// API key -- this overwrites whatever `updated_by` the previous update (if any) left behind.
pub async fn update(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    id: Uuid,
    data: Value,
    updated_by: Option<Uuid>,
) -> Result<EntityRecord, YorishiroError> {
    let existing = get(conn, workspace_id, id).await?;
    let schema = schemas::get_by_id(conn, workspace_id, existing.schema_id).await?;
    let entity_type_def = resolve_entity_type(&schema.definition, &existing.entity_type)?;
    validate_data(entity_type_def, &data)?;

    let (sql, values) = Query::update()
        .table((Alias::new("content"), Entities::Table))
        .value(Entities::Data, data)
        .value(Entities::UpdatedAt, Expr::cust("now()"))
        .value(Entities::UpdatedBy, updated_by)
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Entities::Id).eq(id))
        .returning(Query::returning().columns([
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
        ]))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, EntityRecord, _>(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!("entity '{id}' was not found"),
        })
}

pub async fn delete(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<(), YorishiroError> {
    let (sql, values) = Query::delete()
        .from_table((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Entities::Id).eq(id))
        .build_sqlx(PostgresQueryBuilder);

    let result = sqlx::query_with(&sql, values)
        .execute(&mut *conn)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    if result.rows_affected() == 0 {
        Err(YorishiroError::NotFound {
            message: format!("entity '{id}' was not found"),
        })
    } else {
        Ok(())
    }
}

pub async fn list(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    query: ListEntitiesQuery,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    let limit = query.limit.clamp(1, 200);
    let offset = query.offset.max(0);

    let mut builder = Query::select();
    builder
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
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id));
    if let Some(entity_type) = query.entity_type {
        builder.and_where(Expr::col(Entities::EntityType).eq(entity_type));
    }
    if let Some(filter) = query.filter {
        builder.and_where(Expr::col(Entities::Data).contains(filter));
    }
    builder
        .order_by(Entities::CreatedAt, Order::Desc)
        .limit(limit as u64)
        .offset(offset as u64);
    let (sql, values) = builder.build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, EntityRecord, _>(&sql, values)
        .fetch_all(&mut *conn)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))
}

/// Fetches every entity for the tenant, with no pagination limit, for a full-tenant export.
pub async fn export_all(
    conn: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    sqlx::query_as::<_, EntityRecord>(
        "SELECT id, workspace_id, schema_id, schema_version, entity_type, data, created_at, \
         updated_at, created_by, updated_by \
         FROM content.entities WHERE workspace_id = $1 ORDER BY created_at",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sqlx::PgPool;

    use super::*;
    use crate::db::TenantDb;
    use crate::metaschema::MetaSchemaDefinition;

    fn task_schema() -> MetaSchemaDefinition {
        serde_json::from_value(json!({
            "name": "task-management",
            "entity_types": {
                "task": {
                    "fields": {
                        "title": { "type": "string", "required": true }
                    }
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn default_list_query_uses_a_sensible_page_size() {
        let query = ListEntitiesQuery::default();
        assert_eq!(query.limit, DEFAULT_LIST_LIMIT);
        assert_eq!(query.offset, 0);
        assert!(query.entity_type.is_none());
    }

    #[test]
    fn missing_required_field_points_at_the_missing_property() {
        let def = task_schema();
        let entity_type_def = &def.entity_types["task"];

        let err = validate_data(entity_type_def, &json!({})).unwrap_err();
        match err {
            YorishiroError::ValidationFailed { details, .. } => {
                assert!(
                    details.iter().any(|d| d.field == "/title"),
                    "details: {details:?}"
                );
            }
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
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
    async fn creates_and_fetches_entity(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();

        schemas::create_schema(&mut conn, workspace_id, task_schema())
            .await
            .unwrap();

        let created = create(
            &mut conn,
            workspace_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "buy milk" }),
            },
            None,
        )
        .await
        .unwrap();

        assert_eq!(created.entity_type, "task");
        assert_eq!(created.schema_version, 1);

        let fetched = get(&mut conn, workspace_id, created.id).await.unwrap();
        assert_eq!(fetched.data["title"], "buy milk");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_invalid_data(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();
        schemas::create_schema(&mut conn, workspace_id, task_schema())
            .await
            .unwrap();

        let err = create(
            &mut conn,
            workspace_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({}),
            },
            None,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, YorishiroError::ValidationFailed { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_unknown_entity_type(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();
        schemas::create_schema(&mut conn, workspace_id, task_schema())
            .await
            .unwrap();

        let err = create(
            &mut conn,
            workspace_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "nonexistent".into(),
                data: json!({}),
            },
            None,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, YorishiroError::NotFound { .. }));
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
        schemas::create_schema(&mut conn_a, tenant_a, task_schema())
            .await
            .unwrap();
        let entity = create(
            &mut conn_a,
            tenant_a,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "tenant a task" }),
            },
            None,
        )
        .await
        .unwrap();

        let mut conn_b = db
            .acquire_for_workspace(tenant_b_tenant, tenant_b)
            .await
            .unwrap();
        let result = get(&mut conn_b, tenant_b, entity.id).await;
        assert!(matches!(result, Err(YorishiroError::NotFound { .. })));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_validates_against_creation_time_schema_version(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();

        schemas::create_schema(&mut conn, workspace_id, task_schema())
            .await
            .unwrap();
        let entity = create(
            &mut conn,
            workspace_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "v1 task" }),
            },
            None,
        )
        .await
        .unwrap();

        let v2: MetaSchemaDefinition = serde_json::from_value(json!({
            "name": "task-management",
            "entity_types": {
                "task": {
                    "fields": {
                        "title": { "type": "string", "required": true },
                        "priority": { "type": "integer", "required": true }
                    }
                }
            }
        }))
        .unwrap();
        schemas::create_schema(&mut conn, workspace_id, v2)
            .await
            .unwrap();

        let updated = update(
            &mut conn,
            workspace_id,
            entity.id,
            json!({ "title": "v1 task updated" }),
            None,
        )
        .await
        .unwrap();
        assert_eq!(updated.schema_version, 1);
        assert_eq!(updated.data["title"], "v1 task updated");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn delete_removes_entity(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();
        schemas::create_schema(&mut conn, workspace_id, task_schema())
            .await
            .unwrap();
        let entity = create(
            &mut conn,
            workspace_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "to delete" }),
            },
            None,
        )
        .await
        .unwrap();

        delete(&mut conn, workspace_id, entity.id).await.unwrap();
        let err = get(&mut conn, workspace_id, entity.id).await.unwrap_err();
        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn list_filters_by_entity_type(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();
        schemas::create_schema(
            &mut conn,
            workspace_id,
            serde_json::from_value(json!({
                "name": "task-management",
                "entity_types": {
                    "task": { "fields": { "title": { "type": "string", "required": true } } },
                    "project": { "fields": { "title": { "type": "string", "required": true } } }
                }
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        for (entity_type, title) in [
            ("task", "task one"),
            ("task", "task two"),
            ("project", "project one"),
        ] {
            create(
                &mut conn,
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
        }

        let tasks = list(
            &mut conn,
            workspace_id,
            ListEntitiesQuery {
                entity_type: Some("task".into()),
                filter: None,
                limit: 10,
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|e| e.entity_type == "task"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn list_filters_by_data_field_value(pool: PgPool) {
        let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db
            .acquire_for_workspace(workspace_id_tenant, workspace_id)
            .await
            .unwrap();
        schemas::create_schema(&mut conn, workspace_id, task_schema())
            .await
            .unwrap();

        for (title, status) in [
            ("task one", "active"),
            ("task two", "done"),
            ("task three", "active"),
        ] {
            create(
                &mut conn,
                workspace_id,
                CreateEntityInput {
                    schema_name: "task-management".into(),
                    entity_type: "task".into(),
                    data: json!({ "title": title, "status": status }),
                },
                None,
            )
            .await
            .unwrap();
        }

        let active = list(
            &mut conn,
            workspace_id,
            ListEntitiesQuery {
                entity_type: None,
                filter: Some(json!({ "status": "active" })),
                limit: 10,
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(active.len(), 2);
        assert!(active.iter().all(|e| e.data["status"] == "active"));
    }
}
