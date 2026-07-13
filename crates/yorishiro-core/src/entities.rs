use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ValidationDetail, YorishiroError};
use crate::metaschema;
use crate::schemas;

/// A row in the `entities` table. `embedding` is managed separately by the
/// search/embedding pipeline, so this module's CRUD doesn't touch it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct EntityRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub schema_id: Uuid,
    pub schema_version: i32,
    pub entity_type: String,
    #[schema(value_type = Object)]
    pub data: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct CreateEntityInput {
    pub schema_name: String,
    pub entity_type: String,
    pub data: Value,
}

const DEFAULT_LIST_LIMIT: i64 = 50;

pub struct ListEntitiesQuery {
    pub entity_type: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl Default for ListEntitiesQuery {
    fn default() -> Self {
        Self {
            entity_type: None,
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

/// Creates a new entity: resolves the schema name to its currently active schema, checks
/// that the entity_type exists in that version, validates `data`, and persists the result.
pub async fn create(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    input: CreateEntityInput,
) -> Result<EntityRecord, YorishiroError> {
    let schema = schemas::get_active_schema(conn, tenant_id, &input.schema_name).await?;
    let entity_type_def = resolve_entity_type(&schema.definition, &input.entity_type)?;
    validate_data(entity_type_def, &input.data)?;

    sqlx::query_as::<_, EntityRecord>(
        "INSERT INTO entities (tenant_id, schema_id, schema_version, entity_type, data) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, tenant_id, schema_id, schema_version, entity_type, data, created_at, updated_at",
    )
    .bind(tenant_id)
    .bind(schema.id)
    .bind(schema.version)
    .bind(&input.entity_type)
    .bind(&input.data)
    .fetch_one(&mut *conn)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))
}

pub async fn get(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
) -> Result<EntityRecord, YorishiroError> {
    sqlx::query_as::<_, EntityRecord>(
        "SELECT id, tenant_id, schema_id, schema_version, entity_type, data, created_at, updated_at \
         FROM entities WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(id)
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
pub async fn update(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
    data: Value,
) -> Result<EntityRecord, YorishiroError> {
    let existing = get(conn, tenant_id, id).await?;
    let schema = schemas::get_by_id(conn, tenant_id, existing.schema_id).await?;
    let entity_type_def = resolve_entity_type(&schema.definition, &existing.entity_type)?;
    validate_data(entity_type_def, &data)?;

    sqlx::query_as::<_, EntityRecord>(
        "UPDATE entities SET data = $1, updated_at = now() \
         WHERE tenant_id = $2 AND id = $3 \
         RETURNING id, tenant_id, schema_id, schema_version, entity_type, data, created_at, updated_at",
    )
    .bind(&data)
    .bind(tenant_id)
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|err| YorishiroError::Internal(err.into()))?
    .ok_or_else(|| YorishiroError::NotFound {
        message: format!("entity '{id}' was not found"),
    })
}

pub async fn delete(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
) -> Result<(), YorishiroError> {
    let result = sqlx::query("DELETE FROM entities WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(id)
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
    tenant_id: Uuid,
    query: ListEntitiesQuery,
) -> Result<Vec<EntityRecord>, YorishiroError> {
    let limit = query.limit.clamp(1, 200);
    let offset = query.offset.max(0);

    sqlx::query_as::<_, EntityRecord>(
        "SELECT id, tenant_id, schema_id, schema_version, entity_type, data, created_at, updated_at \
         FROM entities \
         WHERE tenant_id = $1 AND ($2::text IS NULL OR entity_type = $2) \
         ORDER BY created_at DESC \
         LIMIT $3 OFFSET $4",
    )
    .bind(tenant_id)
    .bind(query.entity_type)
    .bind(limit)
    .bind(offset)
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

    async fn seed_tenant(pool: &PgPool) -> Uuid {
        let (id,): (Uuid,) = sqlx::query_as("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
            .bind("test-tenant")
            .fetch_one(pool)
            .await
            .unwrap();
        id
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn creates_and_fetches_entity(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();

        schemas::create_schema(&mut conn, tenant_id, task_schema())
            .await
            .unwrap();

        let created = create(
            &mut conn,
            tenant_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "buy milk" }),
            },
        )
        .await
        .unwrap();

        assert_eq!(created.entity_type, "task");
        assert_eq!(created.schema_version, 1);

        let fetched = get(&mut conn, tenant_id, created.id).await.unwrap();
        assert_eq!(fetched.data["title"], "buy milk");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_invalid_data(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        schemas::create_schema(&mut conn, tenant_id, task_schema())
            .await
            .unwrap();

        let err = create(
            &mut conn,
            tenant_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({}),
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(err, YorishiroError::ValidationFailed { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_unknown_entity_type(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        schemas::create_schema(&mut conn, tenant_id, task_schema())
            .await
            .unwrap();

        let err = create(
            &mut conn,
            tenant_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "nonexistent".into(),
                data: json!({}),
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn enforces_tenant_isolation(pool: PgPool) {
        let tenant_a = seed_tenant(&pool).await;
        let tenant_b = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);

        let mut conn_a = db.acquire_for_tenant(tenant_a).await.unwrap();
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
        )
        .await
        .unwrap();

        let mut conn_b = db.acquire_for_tenant(tenant_b).await.unwrap();
        let result = get(&mut conn_b, tenant_b, entity.id).await;
        assert!(matches!(result, Err(YorishiroError::NotFound { .. })));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_validates_against_creation_time_schema_version(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();

        schemas::create_schema(&mut conn, tenant_id, task_schema())
            .await
            .unwrap();
        let entity = create(
            &mut conn,
            tenant_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "v1 task" }),
            },
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
        schemas::create_schema(&mut conn, tenant_id, v2)
            .await
            .unwrap();

        let updated = update(
            &mut conn,
            tenant_id,
            entity.id,
            json!({ "title": "v1 task updated" }),
        )
        .await
        .unwrap();
        assert_eq!(updated.schema_version, 1);
        assert_eq!(updated.data["title"], "v1 task updated");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn delete_removes_entity(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        schemas::create_schema(&mut conn, tenant_id, task_schema())
            .await
            .unwrap();
        let entity = create(
            &mut conn,
            tenant_id,
            CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "to delete" }),
            },
        )
        .await
        .unwrap();

        delete(&mut conn, tenant_id, entity.id).await.unwrap();
        let err = get(&mut conn, tenant_id, entity.id).await.unwrap_err();
        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn list_filters_by_entity_type(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        schemas::create_schema(
            &mut conn,
            tenant_id,
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
                tenant_id,
                CreateEntityInput {
                    schema_name: "task-management".into(),
                    entity_type: entity_type.into(),
                    data: json!({ "title": title }),
                },
            )
            .await
            .unwrap();
        }

        let tasks = list(
            &mut conn,
            tenant_id,
            ListEntitiesQuery {
                entity_type: Some("task".into()),
                limit: 10,
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|e| e.entity_type == "task"));
    }
}
