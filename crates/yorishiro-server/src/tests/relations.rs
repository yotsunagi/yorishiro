use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use yorishiro_core::db::TenantDb;
use yorishiro_core::services::auth::{ApiKeyScope, create_api_key};

use super::helpers::*;
use crate::*;

#[sqlx::test(migrations = "../../migrations")]
async fn rest_relation_crud_round_trip(pool: PgPool) {
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

    let (task_id, project_id) = seed_task_and_project(&app, &schema_auth, &write_auth).await;

    let create_body = serde_json::json!({
        "source_id": task_id,
        "target_id": project_id,
        "relation_type": "belongs_to",
    });
    let response = rest_request(
        &app,
        "POST",
        "/api/relations",
        Some(&write_auth),
        Some(create_body.clone()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = rest_json_body(response).await;
    let relation_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["relation_type"], "belongs_to");

    // Creating the same relation again is a conflict, 409.
    let response = rest_request(
        &app,
        "POST",
        "/api/relations",
        Some(&write_auth),
        Some(create_body),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    // A direction that contradicts the relation_type's declared source/target
    // (project→task) is 422.
    let response = rest_request(
        &app,
        "POST",
        "/api/relations",
        Some(&write_auth),
        Some(serde_json::json!({
            "source_id": project_id,
            "target_id": task_id,
            "relation_type": "belongs_to",
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = rest_request(
        &app,
        "GET",
        &format!("/api/relations/{relation_id}"),
        Some(&write_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let fetched = rest_json_body(response).await;
    assert_eq!(fetched["id"], relation_id.as_str());

    let response = rest_request(
        &app,
        "GET",
        &format!("/api/relations?source_id={task_id}"),
        Some(&write_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let listed = rest_json_body(response).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);

    let response = rest_request(
        &app,
        "DELETE",
        &format!("/api/relations/{relation_id}"),
        Some(&write_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = rest_request(
        &app,
        "GET",
        &format!("/api/relations/{relation_id}"),
        Some(&write_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
