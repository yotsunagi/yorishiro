use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;

use super::*;
use crate::state::AppState;

async fn request(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

use axum::Router;
use yorishiro_core::db::TenantDb;
use yorishiro_core::services::embedding::EmbeddingProvider;

struct UnreachableEmbeddingProvider;

#[async_trait::async_trait]
impl EmbeddingProvider for UnreachableEmbeddingProvider {
    fn dimensions(&self) -> usize {
        768
    }

    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Err(YorishiroError::Internal(anyhow::anyhow!("unreachable")))
    }
}

fn app(pool: PgPool) -> Router {
    let state = AppState::new(
        TenantDb::new(pool.clone()),
        pool,
        std::sync::Arc::new(UnreachableEmbeddingProvider),
    );
    crate::http::controllers::router().with_state(state)
}

/// `sqlx::test` runs each test on its own single-threaded runtime, so holding a non-`Send`
/// `MutexGuard` across an `.await` is sound here. See `crate::max_tenants_env_lock` for why
/// this lock is shared crate-wide rather than private to this module.
use crate::max_tenants_env_lock::{LOCK as ENV_LOCK, set as set_max_tenants};

#[sqlx::test(migrations = "../../migrations")]
#[allow(clippy::await_holding_lock)]
async fn status_reports_setup_not_required_when_wizard_disabled(pool: PgPool) {
    let _guard = ENV_LOCK.lock().unwrap();
    set_max_tenants(None);
    let app = app(pool);
    let response = request(&app, "GET", "/setup/status", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["setup_required"], false);
}

#[sqlx::test(migrations = "../../migrations")]
#[allow(clippy::await_holding_lock)]
async fn status_reports_setup_not_required_when_max_tenants_is_zero(pool: PgPool) {
    let _guard = ENV_LOCK.lock().unwrap();
    set_max_tenants(Some("0"));
    let app = app(pool);
    let response = request(&app, "GET", "/setup/status", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["setup_required"], false);
}

#[sqlx::test(migrations = "../../migrations")]
#[allow(clippy::await_holding_lock)]
async fn setup_rejects_when_wizard_disabled(pool: PgPool) {
    let _guard = ENV_LOCK.lock().unwrap();
    set_max_tenants(None);
    let app = app(pool);
    let response = request(
        &app,
        "POST",
        "/setup",
        Some(serde_json::json!({ "email": "a@example.com", "password": "hunter2-hunter2" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
#[allow(clippy::await_holding_lock)]
async fn setup_creates_tenant_workspace_and_owner(pool: PgPool) {
    let _guard = ENV_LOCK.lock().unwrap();
    set_max_tenants(Some("1"));
    let app = app(pool.clone());

    let status_response = request(&app, "GET", "/setup/status", None).await;
    assert_eq!(status_response.status(), StatusCode::OK);
    let status_body = axum::body::to_bytes(status_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
    assert_eq!(status_json["setup_required"], true);

    let response = request(
        &app,
        "POST",
        "/setup",
        Some(serde_json::json!({
            "email": "owner@example.com",
            "password": "hunter2-hunter2",
            "display_name": "Owner",
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["email"], "owner@example.com");
    assert!(json["api_key"].as_str().unwrap().starts_with("ysr_"));

    let role = tenancy::get_membership_role(
        &pool,
        Uuid::parse_str(json["tenant_id"].as_str().unwrap()).unwrap(),
        Uuid::parse_str(json["user_id"].as_str().unwrap()).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(role, Some(MembershipRole::Owner));
}

#[sqlx::test(migrations = "../../migrations")]
#[allow(clippy::await_holding_lock)]
async fn setup_rejects_once_a_tenant_already_exists(pool: PgPool) {
    let _guard = ENV_LOCK.lock().unwrap();
    set_max_tenants(Some("1"));
    tenancy::create_tenant(&pool, "existing", None)
        .await
        .unwrap();
    let app = app(pool);
    let response = request(
        &app,
        "POST",
        "/setup",
        Some(serde_json::json!({ "email": "a@example.com", "password": "hunter2-hunter2" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}
