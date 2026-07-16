use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use yorishiro_core::db::TenantDb;
use yorishiro_core::services::auth::{ApiKeyScope, create_api_key};

use super::helpers::*;
use crate::*;

#[sqlx::test(migrations = "../../migrations")]
async fn rest_openapi_json_is_served(pool: PgPool) {
    let app = build_app(test_state(pool), None);

    let response = rest_request(&app, "GET", "/api-docs/openapi.json", None, None).await;
    assert_eq!(response.status(), StatusCode::OK);

    let json = rest_json_body(response).await;
    assert!(json["openapi"].is_string());
    assert!(
        json["paths"]["/api/entities"]["post"].is_object(),
        "openapi doc must document POST /api/entities: {json}"
    );
    assert_eq!(
        json["components"]["securitySchemes"]["bearer_auth"]["scheme"],
        "bearer"
    );

    // Verify every $ref referenced by paths' responses/request_body actually exists
    // under components.schemas (a regression test for utoipa's `paths(...)` listing
    // correctly auto-collecting schemas).
    let schemas = json["components"]["schemas"].as_object().unwrap();
    let json_str = json.to_string();
    let dangling: Vec<&str> = json_str
        .split("\"$ref\":\"#/components/schemas/")
        .skip(1)
        .map(|part| part.split('"').next().unwrap())
        .filter(|name| !schemas.contains_key(*name))
        .collect();
    assert!(dangling.is_empty(), "dangling $ref found: {dangling:?}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn rest_entities_endpoint_requires_authentication(pool: PgPool) {
    let app = build_app(test_state(pool), None);

    let response = rest_request(&app, "GET", "/api/entities", None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn rest_entities_endpoint_rejects_an_unknown_bearer_token(pool: PgPool) {
    let app = build_app(test_state(pool), None);

    let response = rest_request(
        &app,
        "GET",
        "/api/entities",
        Some("Bearer not-a-real-key"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn rest_create_entity_rejects_insufficient_scope(pool: PgPool) {
    let (tenant_id_tenant, tenant_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id_tenant, tenant_id)
        .await
        .unwrap();
    let created = create_api_key(&mut conn, tenant_id, ApiKeyScope::Read, None)
        .await
        .unwrap();
    drop(conn);

    let app = build_app(
        AppState::new(db, pool.clone(), Arc::new(UnreachableEmbeddingProvider)),
        None,
    );

    let response = rest_request(
        &app,
        "POST",
        "/api/entities",
        Some(&format!("Bearer {}", created.plaintext)),
        Some(serde_json::json!({
            "schema_name": "does-not-matter",
            "entity_type": "does-not-matter",
            "data": {},
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../migrations")]
async fn rest_entity_crud_round_trip(pool: PgPool) {
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
            "schema_name": "task-management",
            "entity_type": "task",
            "data": { "title": "buy milk" },
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = rest_json_body(response).await;
    let entity_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["data"]["title"], "buy milk");

    let response = rest_request(
        &app,
        "GET",
        &format!("/api/entities/{entity_id}"),
        Some(&write_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let fetched = rest_json_body(response).await;
    assert_eq!(fetched["id"], entity_id);

    let response = rest_request(
        &app,
        "PUT",
        &format!("/api/entities/{entity_id}"),
        Some(&write_auth),
        Some(serde_json::json!({ "data": { "title": "buy milk and eggs" } })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let updated = rest_json_body(response).await;
    assert_eq!(updated["data"]["title"], "buy milk and eggs");

    let response = rest_request(&app, "GET", "/api/entities", Some(&write_auth), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let listed = rest_json_body(response).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);

    let response = rest_request(
        &app,
        "DELETE",
        &format!("/api/entities/{entity_id}"),
        Some(&write_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = rest_request(
        &app,
        "GET",
        &format!("/api/entities/{entity_id}"),
        Some(&write_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Verifies the full production path over REST: entity creation, background
/// embedding sync, then a vector search hit. The embedding sync is a fire-and-forget
/// background task, so this polls at short intervals until the search hits.
#[sqlx::test(migrations = "../../migrations")]
async fn rest_created_entity_becomes_searchable(pool: PgPool) {
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
        AppState::new(db, pool.clone(), Arc::new(FixedEmbeddingProvider)),
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
                "task": {
                    "fields": {
                        "title": { "type": "string", "required": true, "x-embed": true }
                    }
                }
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
            "schema_name": "task-management",
            "entity_type": "task",
            "data": { "title": "write quarterly report" },
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = rest_json_body(response).await;
    let entity_id = created["id"].as_str().unwrap().to_string();

    let mut hits = serde_json::Value::Null;
    for _ in 0..50 {
        let response = rest_request(
            &app,
            "GET",
            "/api/search?query_text=quarterly%20report",
            Some(&write_auth),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = rest_json_body(response).await;
        if !body.as_array().unwrap().is_empty() {
            hits = body;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    let hits = hits
        .as_array()
        .expect("entity did not become searchable within 5s (embedding sync not wired?)");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["entity"]["id"], entity_id.as_str());
}

#[sqlx::test(migrations = "../../migrations")]
async fn rest_enforces_tenant_isolation(pool: PgPool) {
    let (tenant_a_tenant, tenant_a) = seed_workspace(&pool).await;
    let (tenant_b_tenant, tenant_b) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());

    let mut conn_a = db
        .acquire_for_workspace(tenant_a_tenant, tenant_a)
        .await
        .unwrap();
    let schema_key_a = create_api_key(&mut conn_a, tenant_a, ApiKeyScope::Schema, None)
        .await
        .unwrap();
    let write_key_a = create_api_key(&mut conn_a, tenant_a, ApiKeyScope::Write, None)
        .await
        .unwrap();
    drop(conn_a);

    let mut conn_b = db
        .acquire_for_workspace(tenant_b_tenant, tenant_b)
        .await
        .unwrap();
    let read_key_b = create_api_key(&mut conn_b, tenant_b, ApiKeyScope::Read, None)
        .await
        .unwrap();
    drop(conn_b);

    let app = build_app(
        AppState::new(db, pool.clone(), Arc::new(UnreachableEmbeddingProvider)),
        None,
    );
    let schema_auth_a = format!("Bearer {}", schema_key_a.plaintext);
    let write_auth_a = format!("Bearer {}", write_key_a.plaintext);
    let read_auth_b = format!("Bearer {}", read_key_b.plaintext);

    let response = rest_request(
        &app,
        "POST",
        "/api/schemas",
        Some(&schema_auth_a),
        Some(serde_json::json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "title": { "type": "string", "required": true } } }
            },
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = rest_request(
        &app,
        "POST",
        "/api/entities",
        Some(&write_auth_a),
        Some(serde_json::json!({
            "schema_name": "task-management",
            "entity_type": "task",
            "data": { "title": "buy milk" },
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = rest_json_body(response).await;
    let entity_id = created["id"].as_str().unwrap().to_string();

    // Tenant B's key sees tenant A's entity as if it doesn't exist, hence 404.
    let response = rest_request(
        &app,
        "GET",
        &format!("/api/entities/{entity_id}"),
        Some(&read_auth_b),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = rest_request(&app, "GET", "/api/entities", Some(&read_auth_b), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let listed = rest_json_body(response).await;
    assert_eq!(listed.as_array().unwrap().len(), 0);
}
