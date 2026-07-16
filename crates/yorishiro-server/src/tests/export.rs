use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use yorishiro_core::db::TenantDb;
use yorishiro_core::services::auth::{ApiKeyScope, create_api_key};

use super::helpers::*;
use crate::*;

#[sqlx::test(migrations = "../../migrations")]
async fn rest_export_jsonl_streams_every_record_for_the_tenant(pool: PgPool) {
    let (tenant_id_tenant, tenant_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id_tenant, tenant_id)
        .await
        .unwrap();
    let schema_key = create_api_key(&mut conn, tenant_id, ApiKeyScope::Schema, None)
        .await
        .unwrap();
    let write_key = create_api_key(&mut conn, tenant_id, ApiKeyScope::Write, None)
        .await
        .unwrap();
    drop(conn);

    let app = build_app(
        AppState::new(db, pool.clone(), Arc::new(UnreachableEmbeddingProvider)),
        None,
    );
    let schema_auth = format!("Bearer {}", schema_key.plaintext);
    let write_auth = format!("Bearer {}", write_key.plaintext);

    let response = rest_request(
        &app,
        "POST",
        "/api/schemas",
        Some(&schema_auth),
        Some(serde_json::json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "title": { "type": "string", "required": true } } }
            },
            "relation_types": {
                "blocks": { "source": "task", "target": "task" }
            },
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = rest_request(
        &app,
        "POST",
        "/api/entities",
        Some(&write_auth),
        Some(serde_json::json!({
            "schema_name": "task-management", "entity_type": "task", "data": { "title": "a" },
        })),
    )
    .await;
    let a = rest_json_body(response).await;

    let response = rest_request(
        &app,
        "POST",
        "/api/entities",
        Some(&write_auth),
        Some(serde_json::json!({
            "schema_name": "task-management", "entity_type": "task", "data": { "title": "b" },
        })),
    )
    .await;
    let b = rest_json_body(response).await;

    let response = rest_request(
        &app,
        "POST",
        "/api/relations",
        Some(&write_auth),
        Some(serde_json::json!({
            "source_id": a["id"], "target_id": b["id"], "relation_type": "blocks",
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = rest_request(&app, "GET", "/api/export.jsonl", Some(&write_auth), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/x-ndjson"
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let lines: Vec<serde_json::Value> = std::str::from_utf8(&body)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    assert_eq!(lines.iter().filter(|l| l["kind"] == "schema").count(), 1);
    assert_eq!(lines.iter().filter(|l| l["kind"] == "entity").count(), 2);
    assert_eq!(lines.iter().filter(|l| l["kind"] == "relation").count(), 1);

    let response = rest_request(&app, "GET", "/api/export.jsonl", None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
