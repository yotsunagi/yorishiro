use std::sync::Arc;

use axum::extract::FromRef;
use tokio::sync::Semaphore;
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::db::TenantDb;
use yorishiro_core::embedding::EmbeddingProvider;
use yorishiro_core::embedding_sync;
use yorishiro_core::entities::EntityRecord;

/// バックグラウンドembedding同期の同時実行数上限。同期タスクはembedding API呼び出し
/// （最大数十秒）の間プール接続を1本占有するため、無制限にspawnすると書き込みバースト時に
/// リクエスト処理用の接続（プール全体で20本）が枯渇する。上限を超えた分は破棄されるのでは
/// なくsemaphore上で接続を持たずに待機する。
const EMBEDDING_SYNC_MAX_CONCURRENCY: usize = 4;

/// REST/MCP双方のハンドラが共有するアプリケーション状態。
/// `TenantDb`単体ではなくこの構造体をaxumのStateにすることで、
/// 検索系ハンドラが`EmbeddingProvider`にもアクセスできるようにする。
#[derive(Clone)]
pub struct AppState {
    pub tenant_db: TenantDb,
    pub embedding_provider: Arc<dyn EmbeddingProvider>,
    embedding_sync_permits: Arc<Semaphore>,
}

impl AppState {
    pub fn new(tenant_db: TenantDb, embedding_provider: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            tenant_db,
            embedding_provider,
            embedding_sync_permits: Arc::new(Semaphore::new(EMBEDDING_SYNC_MAX_CONCURRENCY)),
        }
    }

    /// entityのcreate/update成功後にembedding列の同期をバックグラウンドで行う。
    /// embedding API呼び出しは最大数十秒かかりうるため、リクエストの応答は待たせず、
    /// リクエストが使っていたコネクションとも切り離して新しくプールから取得する
    /// （`sync_embedding`のdocコメントにある同一トランザクション禁止の制約を満たす）。
    /// 失敗はログに残すのみ: embeddingは補助機能であり、entity本体の書き込み成否に
    /// 影響させない。
    pub fn spawn_embedding_sync(
        &self,
        tenant_id: Uuid,
        record: EntityRecord,
    ) -> tokio::task::JoinHandle<()> {
        let db = self.tenant_db.clone();
        let provider = Arc::clone(&self.embedding_provider);
        let permits = Arc::clone(&self.embedding_sync_permits);
        tokio::spawn(async move {
            // permitを取ってからコネクションを取得する順序が重要:
            // 逆にすると待機中のタスク全員が接続を抱え込み、制限の意味がなくなる。
            let Ok(_permit) = permits.acquire_owned().await else {
                // Semaphoreはcloseしない運用なので到達しない。
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
