use std::sync::Arc;

use axum::extract::FromRef;
use yorishiro_core::db::TenantDb;
use yorishiro_core::embedding::EmbeddingProvider;

/// REST/MCP双方のハンドラが共有するアプリケーション状態。
/// `TenantDb`単体ではなくこの構造体をaxumのStateにすることで、
/// 検索系ハンドラが`EmbeddingProvider`にもアクセスできるようにする。
#[derive(Clone)]
pub struct AppState {
    pub tenant_db: TenantDb,
    pub embedding_provider: Arc<dyn EmbeddingProvider>,
}

impl FromRef<AppState> for TenantDb {
    fn from_ref(state: &AppState) -> Self {
        state.tenant_db.clone()
    }
}
