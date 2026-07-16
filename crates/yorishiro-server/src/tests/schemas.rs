use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use yorishiro_core::db::TenantDb;
use yorishiro_core::services::auth::{ApiKeyScope, create_api_key};

use super::helpers::*;
use crate::*;

#[sqlx::test(migrations = "../../migrations")]
async fn rest_schema_endpoints_round_trip(pool: PgPool) {
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

    // Registering a schema requires schema scope: a write-scope key gets 403.
    let definition_v1 = serde_json::json!({
        "name": "task-management",
        "entity_types": {
            "task": { "fields": { "title": { "type": "string", "required": true } } }
        },
    });
    let response = rest_request(
        &app,
        "POST",
        "/api/schemas",
        Some(&write_auth),
        Some(definition_v1.clone()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = rest_request(
        &app,
        "POST",
        "/api/schemas",
        Some(&schema_auth),
        Some(definition_v1),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = rest_json_body(response).await;
    assert_eq!(created["schema"]["version"], 1);
    assert_eq!(created["diff"]["is_breaking"], false);
    let schema_v1_id = created["schema"]["id"].as_str().unwrap().to_string();

    let response = rest_request(
        &app,
        "GET",
        "/api/schemas/active/task-management",
        Some(&write_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let active = rest_json_body(response).await;
    assert_eq!(active["id"], schema_v1_id.as_str());

    let response = rest_request(
        &app,
        "GET",
        &format!("/api/schemas/{schema_v1_id}"),
        Some(&write_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = rest_request(
        &app,
        "GET",
        "/api/schemas/active/task-management/entity-types/task/json-schema",
        Some(&write_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json_schema = rest_json_body(response).await;
    assert_eq!(json_schema["type"], "object");
    assert!(
        json_schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("title"))
    );

    // v2, which adds a required field, is reported in the diff as a breaking change,
    // and active switches to it.
    let response = rest_request(
        &app,
        "POST",
        "/api/schemas",
        Some(&schema_auth),
        Some(serde_json::json!({
            "name": "task-management",
            "entity_types": {
                "task": {
                    "fields": {
                        "title": { "type": "string", "required": true },
                        "due": { "type": "string", "required": true }
                    }
                }
            },
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created_v2 = rest_json_body(response).await;
    assert_eq!(created_v2["schema"]["version"], 2);
    assert_eq!(created_v2["diff"]["is_breaking"], true);

    let response = rest_request(
        &app,
        "GET",
        "/api/schemas/active/task-management",
        Some(&write_auth),
        None,
    )
    .await;
    let active = rest_json_body(response).await;
    assert_eq!(active["version"], 2);

    let response = rest_request(&app, "GET", "/api/schemas", Some(&write_auth), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let listed = rest_json_body(response).await;
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["version"], 1);
    assert_eq!(listed[0]["status"], "archived");
    assert_eq!(listed[1]["version"], 2);
    assert_eq!(listed[1]["status"], "active");
}

#[sqlx::test(migrations = "../../migrations")]
async fn rest_creates_a_schema_from_a_built_in_template(pool: PgPool) {
    let (tenant_id_tenant, tenant_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id_tenant, tenant_id)
        .await
        .unwrap();
    let schema_key = create_api_key(&mut conn, tenant_id, ApiKeyScope::Schema, None)
        .await
        .unwrap();
    drop(conn);

    let app = build_app(
        AppState::new(db, pool.clone(), Arc::new(UnreachableEmbeddingProvider)),
        None,
    );
    let schema_auth = format!("Bearer {}", schema_key.plaintext);

    let response = rest_request(&app, "GET", "/api/templates", Some(&schema_auth), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let templates = rest_json_body(response).await;
    let templates = templates.as_array().unwrap();
    assert!(templates.iter().any(|t| t["id"] == "task-management"));

    let response = rest_request(
        &app,
        "POST",
        "/api/schemas",
        Some(&schema_auth),
        Some(serde_json::json!({ "template_id": "task-management" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = rest_json_body(response).await;
    assert_eq!(created["schema"]["name"], "task-management");
    assert_eq!(created["schema"]["version"], 1);

    let response = rest_request(
        &app,
        "POST",
        "/api/schemas",
        Some(&schema_auth),
        Some(serde_json::json!({ "template_id": "does-not-exist" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
