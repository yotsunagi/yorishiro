use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;
use yorishiro_core::db::TenantDb;
use yorishiro_core::services::auth::{ApiKeyScope, create_api_key};

use super::helpers::*;
use crate::*;

#[sqlx::test(migrations = "../../migrations")]
async fn whoami_requires_authentication(pool: PgPool) {
    let app = build_app(test_state(pool), None);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/whoami")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn whoami_rejects_an_unknown_key(pool: PgPool) {
    let app = build_app(test_state(pool), None);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/whoami")
                .header("authorization", "Bearer ysr_does_not_exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn whoami_returns_tenant_and_scope_for_a_valid_key(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let created = create_api_key(&mut conn, workspace_id, ApiKeyScope::Write, None)
        .await
        .unwrap();
    drop(conn);

    let app = build_app(
        AppState::new(db, pool.clone(), Arc::new(UnreachableEmbeddingProvider)),
        None,
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri("/whoami")
                .header("authorization", format!("Bearer {}", created.plaintext))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["tenant_id"], tenant_id.to_string());
    assert_eq!(json["workspace_id"], workspace_id.to_string());
    assert_eq!(json["scope"], "write");
    assert!(json["user_id"].is_null());
}
