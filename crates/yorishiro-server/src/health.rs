use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use crate::state::AppState;

/// DB疎通確認に割り当てる上限時間。オーケストレータ（k8s等）のヘルスチェック
/// タイムアウト（通常数秒）より十分短くし、DBが応答不能な場合でも
/// `/health`自体がハングしてオーケストレータ側のタイムアウトに引っかかるより先に
/// 503を返せるようにする。
const DB_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

/// `/health`ハンドラ。固定で`{"status":"ok"}`を返すだけだと、DB障害時や
/// コネクションプール枯渇時にもオーケストレータへ「正常」と偽り続けてしまい、
/// 異常なインスタンスの検知・排除ができなくなる。そのため実際に軽量なクエリで
/// DBへの疎通を確認し、失敗時は503（Service Unavailable）を返す。
///
/// RLSテナントコンテキストは不要な軽量チェックのため、`TenantDb::acquire_for_tenant`
/// （`app.current_tenant`の設定を伴う）ではなく、プールから直接コネクションを
/// 取得するだけにとどめる。
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

    async fn health_response(pool: PgPool) -> axum::response::Response {
        let state = AppState::new(
            TenantDb::new(pool),
            std::sync::Arc::new(UnreachableEmbeddingProvider),
        );
        let app = build_app(state);

        app.oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
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

    /// `Pool::close()`はプールを共有する全クローンに波及し、以降の`acquire()`は
    /// 即座に`Error::PoolClosed`を返すようになる。実際のDB障害・プール枯渇を
    /// 再現する最も手軽な方法として、これを使いDB到達不能時に503が返る
    /// パスを検証する。
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
