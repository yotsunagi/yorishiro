use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use yorishiro_core::db::TenantDb;
use yorishiro_core::services::auth::{ApiKeyScope, create_api_key};

use super::helpers::*;
use crate::*;

#[sqlx::test(migrations = "../../migrations")]
async fn mcp_tool_call_without_authorization_header_is_a_protocol_error(pool: PgPool) {
    let app = build_app(test_state(pool), None);
    let session_id = mcp_handshake(&app).await;

    let response = mcp_post(
        &app,
        Some(&session_id),
        None,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "list_entities", "arguments": {} },
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = parse_sse_json(std::str::from_utf8(&body).unwrap());

    assert!(
        json.get("error").is_some(),
        "expected a JSON-RPC error for a missing Authorization header, got {json}"
    );
}

/// Mechanically verifies, for every tool enumerated by `tools/list`, that a missing
/// Authorization header always produces a protocol error — so that an oversight in one
/// tool's checks can't slip in unnoticed in the future.
#[sqlx::test(migrations = "../../migrations")]
async fn every_registered_tool_requires_an_authorization_header(pool: PgPool) {
    let app = build_app(test_state(pool), None);
    let session_id = mcp_handshake(&app).await;

    let response = mcp_post(
        &app,
        Some(&session_id),
        None,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = parse_sse_json(std::str::from_utf8(&body).unwrap());
    let tools = json["result"]["tools"]
        .as_array()
        .expect("tools/list must return a tools array");
    let tool_names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool must have a name"))
        .collect();
    assert_eq!(
        tool_names.len(),
        17,
        "expected 17 registered tools, got {tool_names:?}"
    );

    for (index, name) in tool_names.iter().enumerate() {
        let response = mcp_post(
            &app,
            Some(&session_id),
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 100 + index as i64,
                "method": "tools/call",
                "params": { "name": name, "arguments": dummy_arguments_for_tool(name) },
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = parse_sse_json(std::str::from_utf8(&body).unwrap());

        assert!(
            json.get("error").is_some(),
            "tool `{name}` did not reject a call missing an Authorization header: {json}"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn mcp_tool_call_with_insufficient_scope_returns_a_tool_error(pool: PgPool) {
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
    let session_id = mcp_handshake(&app).await;

    let response = mcp_post(
        &app,
        Some(&session_id),
        Some(&format!("Bearer {}", created.plaintext)),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "create_entity",
                "arguments": {
                    "schema_name": "does-not-matter",
                    "entity_type": "does-not-matter",
                    "data": {},
                },
            },
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = parse_sse_json(std::str::from_utf8(&body).unwrap());

    assert_eq!(json["result"]["isError"], true);
}

#[sqlx::test(migrations = "../../migrations")]
async fn mcp_tool_call_with_sufficient_scope_succeeds(pool: PgPool) {
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
    let session_id = mcp_handshake(&app).await;

    let response = mcp_post(
        &app,
        Some(&session_id),
        Some(&format!("Bearer {}", created.plaintext)),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "list_entities", "arguments": {} },
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = parse_sse_json(std::str::from_utf8(&body).unwrap());

    assert_eq!(json["result"]["isError"], serde_json::Value::Bool(false));
}
