use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::entities;
use crate::error::YorishiroError;
use crate::schemas;

/// `relations`テーブルの1行。
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

/// relation_typeがsource/targetのentity_typeと矛盾していないかを検証する。
/// メタスキーマ上の定義は、sourceエンティティが実際に作成された時点のスキーマ
/// （entities.schema_idが指す行）に対して解決する。entities::updateと同じく、
/// 有効なスキーマが進んでいても既存entity同士の関係を勝手に壊さないため。
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

/// 新規relationを作成する。source/target双方のentityが存在し、relation_typeが
/// メタスキーマ上のsource/target制約と一致することを検証したうえで永続化する。
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

    sqlx::query_as::<_, RelationRecord>(
        "INSERT INTO relations (tenant_id, source_id, target_id, relation_type, properties) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, tenant_id, source_id, target_id, relation_type, properties, created_at",
    )
    .bind(tenant_id)
    .bind(input.source_id)
    .bind(input.target_id)
    .bind(&input.relation_type)
    .bind(&properties)
    .fetch_one(&mut *conn)
    .await
    .map_err(|err| match err.as_database_error() {
        Some(db_err) if db_err.is_unique_violation() => YorishiroError::Conflict {
            message: format!(
                "relation '{}' between '{}' and '{}' already exists",
                input.relation_type, input.source_id, input.target_id
            ),
        },
        // source/targetの存在確認とINSERTの間に、別トランザクションから当該entityが
        // 削除されるTOCTOUウィンドウがある。FK違反はそのレースの顕在化であり、
        // 事前チェックと同じくNotFoundとして扱う。
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
    sqlx::query_as::<_, RelationRecord>(
        "SELECT id, tenant_id, source_id, target_id, relation_type, properties, created_at \
         FROM relations WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(id)
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
    let result = sqlx::query("DELETE FROM relations WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(id)
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

    sqlx::query_as::<_, RelationRecord>(
        "SELECT id, tenant_id, source_id, target_id, relation_type, properties, created_at \
         FROM relations \
         WHERE tenant_id = $1 \
           AND ($2::uuid IS NULL OR source_id = $2) \
           AND ($3::uuid IS NULL OR target_id = $3) \
           AND ($4::text IS NULL OR relation_type = $4) \
         ORDER BY created_at DESC \
         LIMIT $5 OFFSET $6",
    )
    .bind(tenant_id)
    .bind(query.source_id)
    .bind(query.target_id)
    .bind(query.relation_type)
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

        // tenant_bからはtenant_aのentityも見えないため、source/targetの存在確認自体がNotFoundになる。
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
