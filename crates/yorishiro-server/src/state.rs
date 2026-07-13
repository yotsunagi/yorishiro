use std::sync::Arc;

use axum::extract::FromRef;
use tokio::sync::Semaphore;
use tokio_util::task::TaskTracker;
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::db::TenantDb;
use yorishiro_core::embedding::EmbeddingProvider;
use yorishiro_core::embedding_sync;
use yorishiro_core::entities::EntityRecord;

/// Cap on concurrent background embedding syncs. Each sync task holds a pool connection for
/// the duration of the embedding API call (up to tens of seconds), so spawning without limit
/// would exhaust the connections needed for request handling (20 total in the pool) during a
/// write burst. Tasks beyond the cap aren't dropped — they wait on the semaphore without
/// holding a connection.
const EMBEDDING_SYNC_MAX_CONCURRENCY: usize = 4;

/// Application state shared by both the REST and MCP handlers. Using this struct as axum's
/// `State` — rather than `TenantDb` alone — lets search handlers also reach the
/// `EmbeddingProvider`.
#[derive(Clone)]
pub struct AppState {
    pub tenant_db: TenantDb,
    pub embedding_provider: Arc<dyn EmbeddingProvider>,
    embedding_sync_permits: Arc<Semaphore>,
    embedding_tasks: TaskTracker,
}

impl AppState {
    pub fn new(tenant_db: TenantDb, embedding_provider: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            tenant_db,
            embedding_provider,
            embedding_sync_permits: Arc::new(Semaphore::new(EMBEDDING_SYNC_MAX_CONCURRENCY)),
            embedding_tasks: TaskTracker::new(),
        }
    }

    /// Tracker used to wait for in-flight embedding syncs during graceful shutdown.
    /// `main` calls `close()` + `wait()` on it after the HTTP server stops.
    pub fn embedding_tasks(&self) -> &TaskTracker {
        &self.embedding_tasks
    }

    /// Syncs the `embedding` column in the background after an entity create/update
    /// succeeds. The embedding API call can take up to tens of seconds, so the request isn't
    /// made to wait for it, and a fresh connection is acquired from the pool instead of
    /// reusing the request's own connection (satisfying the no-same-transaction constraint
    /// documented on `sync_embedding`). Failures are only logged: embedding is an auxiliary
    /// feature and must not affect whether the entity write itself succeeds.
    pub fn spawn_embedding_sync(
        &self,
        tenant_id: Uuid,
        record: EntityRecord,
    ) -> tokio::task::JoinHandle<()> {
        let db = self.tenant_db.clone();
        let provider = Arc::clone(&self.embedding_provider);
        let permits = Arc::clone(&self.embedding_sync_permits);
        // Spawning through the TaskTracker lets graceful shutdown wait for the embedding
        // sync of an already-written entity to finish (an immediate SIGTERM exit would lose
        // the sync, leaving that entity permanently missing from search).
        self.embedding_tasks.spawn(async move {
            // The order matters: acquire the permit before the connection. Reversing it
            // would let every waiting task hold a connection, defeating the point of the cap.
            let Ok(_permit) = permits.acquire_owned().await else {
                // Unreachable in practice: the semaphore is never closed.
                return;
            };

            let result = async {
                let mut conn = db
                    .acquire_for_tenant(tenant_id)
                    .await
                    .map_err(|err| YorishiroError::Internal(err.into()))?;
                embedding_sync::sync_embedding_for_record(
                    &mut conn,
                    tenant_id,
                    &record,
                    provider.as_ref(),
                )
                .await
            }
            .await;

            if let Err(err) = result {
                tracing::warn!(entity_id = %record.id, error = %err, "embedding sync failed");
            }
        })
    }
}

impl FromRef<AppState> for TenantDb {
    fn from_ref(state: &AppState) -> Self {
        state.tenant_db.clone()
    }
}
