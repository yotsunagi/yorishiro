use std::sync::Arc;

use axum::extract::FromRef;
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::db::TenantDb;
use yorishiro_core::embedding::EmbeddingProvider;
use yorishiro_core::embedding_sync;
use yorishiro_core::entities::EntityRecord;

/// REST/MCP双方のハンドラが共有するアプリケーション状態。
/// `TenantDb`単体ではなくこの構造体をaxumのStateにすることで、
/// 検索系ハンドラが`EmbeddingProvider`にもアクセスできるようにする。
#[derive(Clone)]
pub struct AppState {
    pub tenant_db: TenantDb,
    pub embedding_provider: Arc<dyn EmbeddingProvider>,
}

impl AppState {
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
        tokio::spawn(async move {
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
