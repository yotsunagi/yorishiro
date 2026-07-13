use chrono::{DateTime, Utc};
use sea_query::{Expr, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::entities;
use crate::error::YorishiroError;
use crate::schemas;

#[derive(Iden)]
enum Relations {
    Table,
    Id,
    TenantId,
    SourceId,
    TargetId,
    RelationType,
    Properties,
    CreatedAt,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct RelationRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    #[schema(value_type = Object)]
    pub properties: Value,
    pub created_at: DateTime<Utc>,
}

pub struct CreateRelationInput {
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    pub properties: Value,
}

const DEFAULT_LIST_LIMIT: i64 = 50;

pub struct ListRelationsQuery {
    pub source_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub relation_type: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl Default for ListRelationsQuery {
    fn default() -> Self {
        Self {
            source_id: None,
            target_id: None,
            relation_type: None,
            limit: DEFAULT_LIST_LIMIT,
            offset: 0,
        }
    }
}

/// Validates that relation_type doesn't conflict with the source/target entity_types.
/// The metaschema definition is resolved against the schema the source entity was actually
/// created with (the row `entities.schema_id` points to) — as with `entities::update`, so
/// existing relationships between entities don't silently break even as the active schema
/// evolves.
async fn validate_relation_type(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    source: &entities::EntityRecord,
    target: &entities::EntityRecord,
    relation_type: &str,
) -> Result<(), YorishiroError> {
    let schema = schemas::get_by_id(conn, tenant_id, source.schema_id).await?;

    let relation_def = schema
        .definition
        .relation_types
        .get(relation_type)
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!(
                "relation_type '{relation_type}' is not defined in schema '{}'",
                schema.definition.name
            ),
        })?;

    if relation_def.source != source.entity_type || relation_def.target != target.entity_type {
        return Err(YorishiroError::RelationTypeMismatch {
            message: format!(
                "relation_type '{relation_type}' expects source='{}' target='{}', \
                 but got source='{}' target='{}'",
                relation_def.source, relation_def.target, source.entity_type, target.entity_type
            ),
        });
    }

    Ok(())
}

/// Creates a new relation: verifies both the source and target entities exist and that
/// relation_type matches the metaschema's source/target constraint, then persists it.
pub async fn create(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    input: CreateRelationInput,
) -> Result<RelationRecord, YorishiroError> {
    let source = entities::get(conn, tenant_id, input.source_id).await?;
    let target = entities::get(conn, tenant_id, input.target_id).await?;
    validate_relation_type(conn, tenant_id, &source, &target, &input.relation_type).await?;

    let properties = if input.properties.is_null() {
        json!({})
    } else {
        input.properties
    };

    let (sql, values) = Query::insert()
        .into_table(Relations::Table)
        .columns([
            Relations::TenantId,
            Relations::SourceId,
            Relations::TargetId,
            Relations::RelationType,
            Relations::Properties,
        ])
        .values_panic([
            tenant_id.into(),
            input.source_id.into(),
            input.target_id.into(),
            input.relation_type.clone().into(),
            properties.into(),
        ])
        .returning(Query::returning().columns([
            Relations::Id,
            Relations::TenantId,
            Relations::SourceId,
            Relations::TargetId,
            Relations::RelationType,
            Relations::Properties,
            Relations::CreatedAt,
        ]))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, RelationRecord, _>(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .map_err(|err| match err.as_database_error() {
            Some(db_err) if db_err.is_unique_violation() => YorishiroError::Conflict {
                message: format!(
                    "relation '{}' between '{}' and '{}' already exists",
                    input.relation_type, input.source_id, input.target_id
                ),
            },
            // There's a TOCTOU window between checking source/target existence and the INSERT,
            // during which another transaction could delete the entity. An FK violation is that
            // race surfacing, so it's treated as NotFound just like the upfront check.
            Some(db_err) if db_err.is_foreign_key_violation() => YorishiroError::NotFound {
                message: format!(
                    "source '{}' or target '{}' no longer exists",
                    input.source_id, input.target_id
                ),
            },
            _ => YorishiroError::Internal(err.into()),
        })
}

pub async fn get(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
) -> Result<RelationRecord, YorishiroError> {
    let (sql, values) = Query::select()
        .columns([
            Relations::Id,
            Relations::TenantId,
            Relations::SourceId,
            Relations::TargetId,
            Relations::RelationType,
            Relations::Properties,
            Relations::CreatedAt,
        ])
        .from(Relations::Table)
        .and_where(Expr::col(Relations::TenantId).eq(tenant_id))
        .and_where(Expr::col(Relations::Id).eq(id))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, RelationRecord, _>(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?
        .ok_or_else(|| YorishiroError::NotFound {
            message: format!("relation '{id}' was not found"),
        })
}

pub async fn delete(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    id: Uuid,
) -> Result<(), YorishiroError> {
    let (sql, values) = Query::delete()
        .from_table(Relations::Table)
        .and_where(Expr::col(Relations::TenantId).eq(tenant_id))
        .and_where(Expr::col(Relations::Id).eq(id))
        .build_sqlx(PostgresQueryBuilder);

    let result = sqlx::query_with(&sql, values)
        .execute(&mut *conn)
        .await
        .map_err(|err| YorishiroError::Internal(err.into()))?;

    if result.rows_affected() == 0 {
        Err(YorishiroError::NotFound {
            message: format!("relation '{id}' was not found"),
        })
    } else {
        Ok(())
    }
}

pub async fn list(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    query: ListRelationsQuery,
) -> Result<Vec<RelationRecord>, YorishiroError> {
    let limit = query.limit.clamp(1, 200);
    let offset = query.offset.max(0);

    let mut builder = Query::select();
    builder
        .columns([
            Relations::Id,
            Relations::TenantId,
            Relations::SourceId,
            Relations::TargetId,
            Relations::RelationType,
            Relations::Properties,
            Relations::CreatedAt,
        ])
        .from(Relations::Table)
        .and_where(Expr::col(Relations::TenantId).eq(tenant_id));
    if let Some(source_id) = query.source_id {
        builder.and_where(Expr::col(Relations::SourceId).eq(source_id));
    }
    if let Some(target_id) = query.target_id {
        builder.and_where(Expr::col(Relations::TargetId).eq(target_id));
    }
    if let Some(relation_type) = query.relation_type {
        builder.and_where(Expr::col(Relations::RelationType).eq(relation_type));
    }
    builder
        .order_by(Relations::CreatedAt, Order::Desc)
        .limit(limit as u64)
        .offset(offset as u64);
    let (sql, values) = builder.build_sqlx(PostgresQueryBuilder);

    sqlx::query_as_with::<_, RelationRecord, _>(&sql, values)
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

    fn project_task_schema() -> MetaSchemaDefinition {
        serde_json::from_value(json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "title": { "type": "string", "required": true } } },
                "project": { "fields": { "title": { "type": "string", "required": true } } }
            },
            "relation_types": {
                "belongs_to": { "source": "task", "target": "project" }
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

    async fn seed_task_and_project(
        conn: &mut PgConnection,
        tenant_id: Uuid,
    ) -> (entities::EntityRecord, entities::EntityRecord) {
        schemas::create_schema(conn, tenant_id, project_task_schema())
            .await
            .unwrap();

        let task = entities::create(
            conn,
            tenant_id,
            entities::CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "task".into(),
                data: json!({ "title": "write report" }),
            },
        )
        .await
        .unwrap();

        let project = entities::create(
            conn,
            tenant_id,
            entities::CreateEntityInput {
                schema_name: "task-management".into(),
                entity_type: "project".into(),
                data: json!({ "title": "Q3 roadmap" }),
            },
        )
        .await
        .unwrap();

        (task, project)
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn creates_and_fetches_relation(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let (task, project) = seed_task_and_project(&mut conn, tenant_id).await;

        let created = create(
            &mut conn,
            tenant_id,
            CreateRelationInput {
                source_id: task.id,
                target_id: project.id,
                relation_type: "belongs_to".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap();

        assert_eq!(created.relation_type, "belongs_to");
        assert_eq!(created.properties, json!({}));

        let fetched = get(&mut conn, tenant_id, created.id).await.unwrap();
        assert_eq!(fetched.source_id, task.id);
        assert_eq!(fetched.target_id, project.id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_relation_type_with_mismatched_source_target(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let (task, project) = seed_task_and_project(&mut conn, tenant_id).await;

        // reversed: belongs_to expects source=task target=project, not the other way around.
        let err = create(
            &mut conn,
            tenant_id,
            CreateRelationInput {
                source_id: project.id,
                target_id: task.id,
                relation_type: "belongs_to".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(err, YorishiroError::RelationTypeMismatch { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_relation_with_nonexistent_source(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let (_, project) = seed_task_and_project(&mut conn, tenant_id).await;

        let err = create(
            &mut conn,
            tenant_id,
            CreateRelationInput {
                source_id: Uuid::nil(),
                target_id: project.id,
                relation_type: "belongs_to".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_unknown_relation_type(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let (task, project) = seed_task_and_project(&mut conn, tenant_id).await;

        let err = create(
            &mut conn,
            tenant_id,
            CreateRelationInput {
                source_id: task.id,
                target_id: project.id,
                relation_type: "no_such_relation".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_duplicate_relation(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let (task, project) = seed_task_and_project(&mut conn, tenant_id).await;

        create(
            &mut conn,
            tenant_id,
            CreateRelationInput {
                source_id: task.id,
                target_id: project.id,
                relation_type: "belongs_to".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap();

        let err = create(
            &mut conn,
            tenant_id,
            CreateRelationInput {
                source_id: task.id,
                target_id: project.id,
                relation_type: "belongs_to".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(err, YorishiroError::Conflict { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn deleting_entity_cascades_relation_deletion(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let (task, project) = seed_task_and_project(&mut conn, tenant_id).await;

        let relation = create(
            &mut conn,
            tenant_id,
            CreateRelationInput {
                source_id: task.id,
                target_id: project.id,
                relation_type: "belongs_to".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap();

        entities::delete(&mut conn, tenant_id, task.id)
            .await
            .unwrap();

        let err = get(&mut conn, tenant_id, relation.id).await.unwrap_err();
        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn deletes_relation(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let (task, project) = seed_task_and_project(&mut conn, tenant_id).await;

        let relation = create(
            &mut conn,
            tenant_id,
            CreateRelationInput {
                source_id: task.id,
                target_id: project.id,
                relation_type: "belongs_to".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap();

        delete(&mut conn, tenant_id, relation.id).await.unwrap();

        let err = get(&mut conn, tenant_id, relation.id).await.unwrap_err();
        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn delete_reports_not_found_for_missing_relation(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();

        let err = delete(&mut conn, tenant_id, Uuid::nil()).await.unwrap_err();
        assert!(matches!(err, YorishiroError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn enforces_tenant_isolation(pool: PgPool) {
        let tenant_a = seed_tenant(&pool).await;
        let tenant_b = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);

        let mut conn_a = db.acquire_for_tenant(tenant_a).await.unwrap();
        let (task, project) = seed_task_and_project(&mut conn_a, tenant_a).await;
        let relation = create(
            &mut conn_a,
            tenant_a,
            CreateRelationInput {
                source_id: task.id,
                target_id: project.id,
                relation_type: "belongs_to".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap();

        let mut conn_b = db.acquire_for_tenant(tenant_b).await.unwrap();
        let result = get(&mut conn_b, tenant_b, relation.id).await;
        assert!(matches!(result, Err(YorishiroError::NotFound { .. })));

        // tenant_b can't see tenant_a's entities either, so the source/target existence check itself reports NotFound.
        let cross_tenant_err = create(
            &mut conn_b,
            tenant_b,
            CreateRelationInput {
                source_id: task.id,
                target_id: project.id,
                relation_type: "belongs_to".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(cross_tenant_err, YorishiroError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn lists_relations_filtered_by_source(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool);
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let (task, project) = seed_task_and_project(&mut conn, tenant_id).await;

        create(
            &mut conn,
            tenant_id,
            CreateRelationInput {
                source_id: task.id,
                target_id: project.id,
                relation_type: "belongs_to".into(),
                properties: Value::Null,
            },
        )
        .await
        .unwrap();

        let relations = list(
            &mut conn,
            tenant_id,
            ListRelationsQuery {
                source_id: Some(task.id),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].target_id, project.id);
    }
}
