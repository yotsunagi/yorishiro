use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use crate::state::AppState;

/// Upper bound for the DB connectivity probe. Kept well below the orchestrator's (e.g.
/// k8s) health check timeout (typically a few seconds) so that, even if the database is
/// unresponsive, `/health` itself returns 503 before it would hang long enough to trip the
/// orchestrator's own timeout.
const DB_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

/// The `/up` handler: a liveness probe that only confirms the process is running and able
/// to answer HTTP requests. Unlike `/health`, it never touches the database, so it stays
/// fast and healthy even during a DB outage — an orchestrator should use this to decide
/// whether to restart the process, and `/health` to decide whether to route traffic to it.
pub async fn up_check() -> (StatusCode, Json<HealthResponse>) {
    (StatusCode::OK, Json(HealthResponse { status: "ok" }))
}

/// The `/health` handler. Simply always returning `{"status":"ok"}` would keep telling the
/// orchestrator the instance is healthy even during a DB outage or pool exhaustion, making
/// it impossible to detect and evict a broken instance. So this actually probes the
/// database with a lightweight query and returns 503 (Service Unavailable) on failure.
///
/// This check doesn't need an RLS tenant context, so it just grabs a connection directly
/// from the pool instead of going through `TenantDb::acquire_for_workspace` (which also sets
/// `app.current_tenant`).
pub async fn health_check(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    match check_db(&state).await {
        Ok(()) => (StatusCode::OK, Json(HealthResponse { status: "ok" })),
        Err(err) => {
            tracing::warn!(error = %err, "health check: database unreachable");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: "unavailable",
                }),
            )
        }
    }
}

async fn check_db(state: &AppState) -> Result<(), sqlx::Error> {
    let pool = state.tenant_db.pool().clone();
    let probe = async move {
        let mut conn = pool.acquire().await?;
        sqlx::query("SELECT 1").execute(conn.as_mut()).await?;
        Ok::<(), sqlx::Error>(())
    };

    match tokio::time::timeout(DB_CHECK_TIMEOUT, probe).await {
        Ok(result) => result,
        Err(_) => Err(sqlx::Error::PoolTimedOut),
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use sqlx::PgPool;
    use tower::ServiceExt;
    use yorishiro_core::YorishiroError;
    use yorishiro_core::db::TenantDb;
    use yorishiro_core::embedding::EmbeddingProvider;

    use super::*;
    use crate::build_app;

    struct UnreachableEmbeddingProvider;

    #[async_trait]
    impl EmbeddingProvider for UnreachableEmbeddingProvider {
        fn dimensions(&self) -> usize {
            768
        }

        async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
            Err(YorishiroError::Internal(anyhow::anyhow!(
                "embedding provider should not be called in this test"
            )))
        }
    }

    async fn get_response(pool: PgPool, uri: &str) -> axum::response::Response {
        let state = AppState::new(
            TenantDb::new(pool),
            std::sync::Arc::new(UnreachableEmbeddingProvider),
        );
        let app = build_app(state);

        app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn health_response(pool: PgPool) -> axum::response::Response {
        get_response(pool, "/health").await
    }

    /// `/up` must stay healthy even when the database is unreachable, since it's a pure
    /// liveness probe — that's the property distinguishing it from `/health`.
    #[sqlx::test(migrations = "../../migrations")]
    async fn up_returns_ok_even_when_db_is_unreachable(pool: PgPool) {
        pool.close().await;

        let response = get_response(pool, "/up").await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn health_returns_ok_when_db_is_reachable(pool: PgPool) {
        let response = health_response(pool).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }

    /// `Pool::close()` affects every clone that shares the pool, and subsequent `acquire()`
    /// calls immediately return `Error::PoolClosed`. This is the easiest way to reproduce a
    /// real DB outage / pool exhaustion, so it's used here to exercise the path where an
    /// unreachable database results in a 503.
    #[sqlx::test(migrations = "../../migrations")]
    async fn health_returns_service_unavailable_when_db_is_unreachable(pool: PgPool) {
        pool.close().await;

        let response = health_response(pool).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "unavailable");
    }
}
