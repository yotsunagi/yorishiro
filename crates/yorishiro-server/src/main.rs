use std::sync::Arc;

use anyhow::Result;
use axum::{Router, routing::get};
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tower_http::cors::{AllowHeaders, AllowMethods, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;
use yorishiro_core::db::TenantDb;
use yorishiro_core::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use yorishiro_core::embedding_onnx::{LocalOnnxConfig, LocalOnnxProvider};

mod auth;
mod error;
mod health;
mod mcp;
mod rest;
mod state;
mod whoami;

use state::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let bind_addr = std::env::var("YSR_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());

    // マイグレーションはCREATE ROLE/GRANT/ALTER TABLE等の管理者権限を要するため、
    // `SET ROLE`でRLS実効ロールへ切り替わる前の一時的な管理プールで実行する。
    // 実行後は破棄し、以降のリクエスト処理は必ず`tenant_db`（yorishiro_appロール）
    // 経由で行う。
    {
        let admin_pool = sqlx::PgPool::connect(&database_url).await?;
        sqlx::migrate!("../../migrations").run(&admin_pool).await?;
    }

    let tenant_db = TenantDb::connect(&database_url, 20).await?;
    let embedding_provider = build_embedding_provider()?;
    let state = AppState::new(tenant_db, embedding_provider);
    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("listening on {bind_addr}");
    axum::serve(listener, app).await?;

    Ok(())
}

/// 環境変数からembeddingsプロバイダを構築する。`YSR_EMBEDDING_PROVIDER`で
/// `openai`（OpenAI互換API、デフォルト）と`local`（ローカルONNXモデル）を切り替える。
/// `entities.embedding`列は`vector(768)`固定のため、次元数が一致しない設定は
/// 起動時に弾く（localの場合はさらにプローブ推論でモデルの実出力次元も検証される）。
fn build_embedding_provider() -> Result<Arc<dyn EmbeddingProvider>> {
    let dimensions: usize = std::env::var("YSR_EMBEDDING_DIMENSIONS")
        .unwrap_or_else(|_| "768".into())
        .parse()?;
    if dimensions != 768 {
        anyhow::bail!(
            "YSR_EMBEDDING_DIMENSIONS must be 768 (entities.embedding is vector(768)), got {dimensions}"
        );
    }

    let kind = std::env::var("YSR_EMBEDDING_PROVIDER").unwrap_or_else(|_| "openai".into());
    match kind.as_str() {
        "openai" => {
            let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                base_url: std::env::var("YSR_EMBEDDING_BASE_URL")
                    .expect("YSR_EMBEDDING_BASE_URL must be set"),
                api_key: std::env::var("YSR_EMBEDDING_API_KEY").unwrap_or_default(),
                model: std::env::var("YSR_EMBEDDING_MODEL")
                    .expect("YSR_EMBEDDING_MODEL must be set"),
                dimensions,
                send_dimensions_param: std::env::var("YSR_EMBEDDING_SEND_DIMENSIONS_PARAM")
                    .map(|v| v == "true")
                    .unwrap_or(true),
            });
            Ok(Arc::new(provider))
        }
        "local" => {
            let max_sequence_length: usize = std::env::var("YSR_ONNX_MAX_SEQUENCE_LENGTH")
                .unwrap_or_else(|_| "512".into())
                .parse()?;
            let provider = LocalOnnxProvider::load(LocalOnnxConfig {
                model_path: std::env::var("YSR_ONNX_MODEL_PATH")
                    .expect("YSR_ONNX_MODEL_PATH must be set when YSR_EMBEDDING_PROVIDER=local")
                    .into(),
                tokenizer_path: std::env::var("YSR_ONNX_TOKENIZER_PATH")
                    .expect("YSR_ONNX_TOKENIZER_PATH must be set when YSR_EMBEDDING_PROVIDER=local")
                    .into(),
                dimensions,
                max_sequence_length,
            })?;
            Ok(Arc::new(provider))
        }
        other => {
            anyhow::bail!("unknown YSR_EMBEDDING_PROVIDER '{other}' (expected 'openai' or 'local')")
        }
    }
}

/// ルーティング構成そのものは`main`と統合テストの双方から同一の形で使う必要が
/// あるため、`AppState`を渡すだけでアプリを組み立てられる関数として切り出す。
fn build_app(state: AppState) -> Router {
    let cors = build_cors_layer();
    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(mcp::YorishiroMcpServer::new(state.clone()))
        },
        LocalSessionManager::default().into(),
        Default::default(),
    );

    Router::new()
        .route("/health", get(health::health_check))
        .route("/whoami", get(whoami::whoami))
        .nest_service("/mcp", mcp_service)
        .merge(rest::router())
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", rest::ApiDoc::openapi()))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn build_cors_layer() -> CorsLayer {
    let origins_str = std::env::var("YSR_CORS_ORIGINS").unwrap_or_default();

    let layer = if origins_str.is_empty() {
        CorsLayer::new()
    } else {
        let origins: Vec<_> = origins_str
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        CorsLayer::new().allow_origin(origins)
    };

    layer
        .allow_methods(AllowMethods::any())
        .allow_headers(AllowHeaders::list([
            "authorization".parse().unwrap(),
            "content-type".parse().unwrap(),
        ]))
        .expose_headers(["x-request-id".parse().unwrap()])
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;
    use uuid::Uuid;
    use yorishiro_core::YorishiroError;
    use yorishiro_core::auth::{ApiKeyScope, create_api_key};

    use super::*;

    /// テストではリモートembeddingsサービスを呼びたくないため、次元数だけを
    /// 満たすダミープロバイダを使う（呼び出されれば即エラーにする）。
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

    fn test_state(pool: PgPool) -> AppState {
        AppState::new(TenantDb::new(pool), Arc::new(UnreachableEmbeddingProvider))
    }

    /// embedding配線のend-to-endテスト用に、決定的なベクトルを返すプロバイダ。
    /// 全テキストが同一ベクトルになるため、クエリとentityの距離は常に0＝必ずヒットする。
    struct FixedEmbeddingProvider;

    #[async_trait]
    impl EmbeddingProvider for FixedEmbeddingProvider {
        fn dimensions(&self) -> usize {
            768
        }

        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
            Ok(texts.iter().map(|_| vec![0.1_f32; 768]).collect())
        }
    }

    async fn seed_tenant(pool: &PgPool) -> Uuid {
        let (id,): (Uuid,) = sqlx::query_as("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
            .bind("test-tenant")
            .fetch_one(pool)
            .await
            .unwrap();
        id
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn whoami_requires_authentication(pool: PgPool) {
        let app = build_app(test_state(pool));

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
        let app = build_app(test_state(pool));

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
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let created = create_api_key(&mut conn, tenant_id, ApiKeyScope::Write)
            .await
            .unwrap();
        drop(conn);

        let app = build_app(AppState::new(db, Arc::new(UnreachableEmbeddingProvider)));
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
        assert_eq!(json["scope"], "write");
    }

    /// `text/event-stream`のボディから`data: {...}`行を抽出し、JSONとしてパースする。
    /// streamable-httpは複数イベントを`\n\n`区切りで返すが、単発リクエストへの
    /// 応答は末尾のイベントに載っているため、それを対象にする。
    fn parse_sse_json(body: &str) -> serde_json::Value {
        body.split("\n\n")
            .filter_map(|event| event.lines().find_map(|line| line.strip_prefix("data: ")))
            .filter_map(|data| serde_json::from_str::<serde_json::Value>(data).ok())
            .last()
            .unwrap_or_else(|| panic!("no `data:` line found in SSE body: {body:?}"))
    }

    async fn mcp_post(
        app: &Router,
        session_id: Option<&str>,
        auth_header: Option<&str>,
        body: serde_json::Value,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");

        if let Some(session_id) = session_id {
            builder = builder.header("mcp-session-id", session_id);
        }
        if let Some(auth_header) = auth_header {
            builder = builder.header("authorization", auth_header);
        }

        app.clone()
            .oneshot(builder.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    /// initialize + notifications/initialized のハンドシェイクを行い、
    /// 以降のtools/call呼び出しで使うセッションIDを返す。
    async fn mcp_handshake(app: &Router) -> String {
        let response = mcp_post(
            app,
            None,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": { "name": "yorishiro-test", "version": "0.0.0" },
                },
            }),
        )
        .await;
        if response.status() != StatusCode::OK {
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            panic!(
                "initialize failed: {status} {}",
                String::from_utf8_lossy(&body)
            );
        }
        let session_id = response
            .headers()
            .get("mcp-session-id")
            .expect("initialize response must carry Mcp-Session-Id")
            .to_str()
            .unwrap()
            .to_string();

        let response = mcp_post(
            app,
            Some(&session_id),
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        session_id
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn mcp_tool_call_without_authorization_header_is_a_protocol_error(pool: PgPool) {
        let app = build_app(test_state(pool));
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

    /// 各ツールの必須引数を型だけ満たすダミー値で埋める。認可チェックは
    /// 引数デシリアライズの後に走るため、このテストの目的（認可漏れの検出）
    /// を成立させるには引数自体を有効な形にしておく必要がある。
    fn dummy_arguments_for_tool(name: &str) -> serde_json::Value {
        const NIL_UUID: &str = "00000000-0000-0000-0000-000000000000";
        match name {
            "create_entity" => serde_json::json!({
                "schema_name": "dummy", "entity_type": "dummy", "data": {},
            }),
            "get_entity" => serde_json::json!({ "id": NIL_UUID }),
            "update_entity" => serde_json::json!({ "id": NIL_UUID, "data": {} }),
            "delete_entity" => serde_json::json!({ "id": NIL_UUID }),
            "list_entities" => serde_json::json!({}),
            "create_relation" => serde_json::json!({
                "source_id": NIL_UUID, "target_id": NIL_UUID, "relation_type": "dummy",
            }),
            "get_relation" => serde_json::json!({ "id": NIL_UUID }),
            "delete_relation" => serde_json::json!({ "id": NIL_UUID }),
            "list_relations" => serde_json::json!({}),
            "get_active_schema" => serde_json::json!({ "name": "dummy" }),
            "get_schema_by_id" => serde_json::json!({ "schema_id": NIL_UUID }),
            "create_schema" => serde_json::json!({ "definition": {} }),
            "get_entity_type_json_schema" => serde_json::json!({
                "schema_name": "dummy", "entity_type": "dummy",
            }),
            "search_entities" => serde_json::json!({ "query_text": "dummy" }),
            other => panic!("no dummy arguments registered for tool `{other}`"),
        }
    }

    /// 個別のツールでの確認漏れが将来紛れ込まないよう、`tools/list`で列挙した
    /// 全ツールに対して機械的に「Authorizationヘッダーが無ければ一律に
    /// プロトコルエラーになる」ことを検証する。
    #[sqlx::test(migrations = "../../migrations")]
    async fn every_registered_tool_requires_an_authorization_header(pool: PgPool) {
        let app = build_app(test_state(pool));
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
            14,
            "expected 14 registered tools, got {tool_names:?}"
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
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let created = create_api_key(&mut conn, tenant_id, ApiKeyScope::Read)
            .await
            .unwrap();
        drop(conn);

        let app = build_app(AppState::new(db, Arc::new(UnreachableEmbeddingProvider)));
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
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let created = create_api_key(&mut conn, tenant_id, ApiKeyScope::Read)
            .await
            .unwrap();
        drop(conn);

        let app = build_app(AppState::new(db, Arc::new(UnreachableEmbeddingProvider)));
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

    async fn rest_request(
        app: &Router,
        method: &str,
        uri: &str,
        auth_header: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(auth_header) = auth_header {
            builder = builder.header("authorization", auth_header);
        }
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

    async fn rest_json_body(response: axum::response::Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_openapi_json_is_served(pool: PgPool) {
        let app = build_app(test_state(pool));

        let response = rest_request(&app, "GET", "/api-docs/openapi.json", None, None).await;
        assert_eq!(response.status(), StatusCode::OK);

        let json = rest_json_body(response).await;
        assert!(json["openapi"].is_string());
        assert!(
            json["paths"]["/api/entities"]["post"].is_object(),
            "openapi doc must document POST /api/entities: {json}"
        );
        assert_eq!(
            json["components"]["securitySchemes"]["bearer_auth"]["scheme"],
            "bearer"
        );

        // pathsのresponses/request_bodyが参照する全$refがcomponents.schemasに
        // 実在することを確認する（utoipaの`paths(...)`列挙による自動収集が
        // 期待通り機能しているかの回帰テスト）。
        let schemas = json["components"]["schemas"].as_object().unwrap();
        let json_str = json.to_string();
        let dangling: Vec<&str> = json_str
            .split("\"$ref\":\"#/components/schemas/")
            .skip(1)
            .map(|part| part.split('"').next().unwrap())
            .filter(|name| !schemas.contains_key(*name))
            .collect();
        assert!(dangling.is_empty(), "dangling $ref found: {dangling:?}");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_entities_endpoint_requires_authentication(pool: PgPool) {
        let app = build_app(test_state(pool));

        let response = rest_request(&app, "GET", "/api/entities", None, None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_entities_endpoint_rejects_an_unknown_bearer_token(pool: PgPool) {
        let app = build_app(test_state(pool));

        let response = rest_request(
            &app,
            "GET",
            "/api/entities",
            Some("Bearer not-a-real-key"),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_create_entity_rejects_insufficient_scope(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let created = create_api_key(&mut conn, tenant_id, ApiKeyScope::Read)
            .await
            .unwrap();
        drop(conn);

        let app = build_app(AppState::new(db, Arc::new(UnreachableEmbeddingProvider)));

        let response = rest_request(
            &app,
            "POST",
            "/api/entities",
            Some(&format!("Bearer {}", created.plaintext)),
            Some(serde_json::json!({
                "schema_name": "does-not-matter",
                "entity_type": "does-not-matter",
                "data": {},
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_entity_crud_round_trip(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let schema_key = create_api_key(&mut conn, tenant_id, ApiKeyScope::Schema)
            .await
            .unwrap();
        let write_key = create_api_key(&mut conn, tenant_id, ApiKeyScope::Write)
            .await
            .unwrap();
        drop(conn);

        let app = build_app(AppState::new(db, Arc::new(UnreachableEmbeddingProvider)));
        let schema_auth = format!("Bearer {}", schema_key.plaintext);
        let write_auth = format!("Bearer {}", write_key.plaintext);

        let response = rest_request(
            &app,
            "POST",
            "/api/schemas",
            Some(&schema_auth),
            Some(serde_json::json!({
                "name": "task-management",
                "entity_types": {
                    "task": { "fields": { "title": { "type": "string", "required": true } } }
                },
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = rest_request(
            &app,
            "POST",
            "/api/entities",
            Some(&write_auth),
            Some(serde_json::json!({
                "schema_name": "task-management",
                "entity_type": "task",
                "data": { "title": "buy milk" },
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = rest_json_body(response).await;
        let entity_id = created["id"].as_str().unwrap().to_string();
        assert_eq!(created["data"]["title"], "buy milk");

        let response = rest_request(
            &app,
            "GET",
            &format!("/api/entities/{entity_id}"),
            Some(&write_auth),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let fetched = rest_json_body(response).await;
        assert_eq!(fetched["id"], entity_id);

        let response = rest_request(
            &app,
            "PUT",
            &format!("/api/entities/{entity_id}"),
            Some(&write_auth),
            Some(serde_json::json!({ "data": { "title": "buy milk and eggs" } })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let updated = rest_json_body(response).await;
        assert_eq!(updated["data"]["title"], "buy milk and eggs");

        let response = rest_request(&app, "GET", "/api/entities", Some(&write_auth), None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let listed = rest_json_body(response).await;
        assert_eq!(listed.as_array().unwrap().len(), 1);

        let response = rest_request(
            &app,
            "DELETE",
            &format!("/api/entities/{entity_id}"),
            Some(&write_auth),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = rest_request(
            &app,
            "GET",
            &format!("/api/entities/{entity_id}"),
            Some(&write_auth),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// entity作成→バックグラウンドembedding同期→ベクトル検索ヒット、という
    /// FR-4の本番経路全体をREST経由で検証する。embedding同期はfire-and-forgetの
    /// バックグラウンドタスクなので、検索がヒットするまで短い間隔でポーリングする。
    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_created_entity_becomes_searchable(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let schema_key = create_api_key(&mut conn, tenant_id, ApiKeyScope::Schema)
            .await
            .unwrap();
        let write_key = create_api_key(&mut conn, tenant_id, ApiKeyScope::Write)
            .await
            .unwrap();
        drop(conn);

        let app = build_app(AppState::new(db, Arc::new(FixedEmbeddingProvider)));
        let schema_auth = format!("Bearer {}", schema_key.plaintext);
        let write_auth = format!("Bearer {}", write_key.plaintext);

        let response = rest_request(
            &app,
            "POST",
            "/api/schemas",
            Some(&schema_auth),
            Some(serde_json::json!({
                "name": "task-management",
                "entity_types": {
                    "task": {
                        "fields": {
                            "title": { "type": "string", "required": true, "x-embed": true }
                        }
                    }
                },
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = rest_request(
            &app,
            "POST",
            "/api/entities",
            Some(&write_auth),
            Some(serde_json::json!({
                "schema_name": "task-management",
                "entity_type": "task",
                "data": { "title": "write quarterly report" },
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = rest_json_body(response).await;
        let entity_id = created["id"].as_str().unwrap().to_string();

        let mut hits = serde_json::Value::Null;
        for _ in 0..50 {
            let response = rest_request(
                &app,
                "GET",
                "/api/search?query_text=quarterly%20report",
                Some(&write_auth),
                None,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = rest_json_body(response).await;
            if !body.as_array().unwrap().is_empty() {
                hits = body;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        let hits = hits
            .as_array()
            .expect("entity did not become searchable within 5s (embedding sync not wired?)");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["entity"]["id"], entity_id.as_str());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_enforces_tenant_isolation(pool: PgPool) {
        let tenant_a = seed_tenant(&pool).await;
        let tenant_b = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());

        let mut conn_a = db.acquire_for_tenant(tenant_a).await.unwrap();
        let schema_key_a = create_api_key(&mut conn_a, tenant_a, ApiKeyScope::Schema)
            .await
            .unwrap();
        let write_key_a = create_api_key(&mut conn_a, tenant_a, ApiKeyScope::Write)
            .await
            .unwrap();
        drop(conn_a);

        let mut conn_b = db.acquire_for_tenant(tenant_b).await.unwrap();
        let read_key_b = create_api_key(&mut conn_b, tenant_b, ApiKeyScope::Read)
            .await
            .unwrap();
        drop(conn_b);

        let app = build_app(AppState::new(db, Arc::new(UnreachableEmbeddingProvider)));
        let schema_auth_a = format!("Bearer {}", schema_key_a.plaintext);
        let write_auth_a = format!("Bearer {}", write_key_a.plaintext);
        let read_auth_b = format!("Bearer {}", read_key_b.plaintext);

        let response = rest_request(
            &app,
            "POST",
            "/api/schemas",
            Some(&schema_auth_a),
            Some(serde_json::json!({
                "name": "task-management",
                "entity_types": {
                    "task": { "fields": { "title": { "type": "string", "required": true } } }
                },
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = rest_request(
            &app,
            "POST",
            "/api/entities",
            Some(&write_auth_a),
            Some(serde_json::json!({
                "schema_name": "task-management",
                "entity_type": "task",
                "data": { "title": "buy milk" },
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = rest_json_body(response).await;
        let entity_id = created["id"].as_str().unwrap().to_string();

        // テナントBのキーではテナントAのエンティティは存在しないものとして404になる。
        let response = rest_request(
            &app,
            "GET",
            &format!("/api/entities/{entity_id}"),
            Some(&read_auth_b),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // 一覧取得もテナントBからは空になる。
        let response = rest_request(&app, "GET", "/api/entities", Some(&read_auth_b), None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let listed = rest_json_body(response).await;
        assert_eq!(listed.as_array().unwrap().len(), 0);
    }

    /// relations向けのフィクスチャ: task/projectの2 entity_typeとbelongs_to relation_typeを
    /// 持つスキーマを登録し、task/projectのエンティティを1件ずつ作って両IDを返す。
    async fn seed_task_and_project(
        app: &Router,
        schema_auth: &str,
        write_auth: &str,
    ) -> (String, String) {
        let response = rest_request(
            app,
            "POST",
            "/api/schemas",
            Some(schema_auth),
            Some(serde_json::json!({
                "name": "task-management",
                "entity_types": {
                    "task": { "fields": { "title": { "type": "string", "required": true } } },
                    "project": { "fields": { "name": { "type": "string", "required": true } } }
                },
                "relation_types": {
                    "belongs_to": { "source": "task", "target": "project" }
                },
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = rest_request(
            app,
            "POST",
            "/api/entities",
            Some(write_auth),
            Some(serde_json::json!({
                "schema_name": "task-management",
                "entity_type": "task",
                "data": { "title": "buy milk" },
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let task_id = rest_json_body(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();

        let response = rest_request(
            app,
            "POST",
            "/api/entities",
            Some(write_auth),
            Some(serde_json::json!({
                "schema_name": "task-management",
                "entity_type": "project",
                "data": { "name": "groceries" },
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let project_id = rest_json_body(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();

        (task_id, project_id)
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_relation_crud_round_trip(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let schema_key = create_api_key(&mut conn, tenant_id, ApiKeyScope::Schema)
            .await
            .unwrap();
        let write_key = create_api_key(&mut conn, tenant_id, ApiKeyScope::Write)
            .await
            .unwrap();
        drop(conn);

        let app = build_app(AppState::new(db, Arc::new(UnreachableEmbeddingProvider)));
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

        // 同一のリレーションの重複作成は409。
        let response = rest_request(
            &app,
            "POST",
            "/api/relations",
            Some(&write_auth),
            Some(create_body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);

        // relation_typeのsource/targetと矛盾する向き（project→task）は422。
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

    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_schema_endpoints_round_trip(pool: PgPool) {
        let tenant_id = seed_tenant(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db.acquire_for_tenant(tenant_id).await.unwrap();
        let schema_key = create_api_key(&mut conn, tenant_id, ApiKeyScope::Schema)
            .await
            .unwrap();
        let write_key = create_api_key(&mut conn, tenant_id, ApiKeyScope::Write)
            .await
            .unwrap();
        drop(conn);

        let app = build_app(AppState::new(db, Arc::new(UnreachableEmbeddingProvider)));
        let schema_auth = format!("Bearer {}", schema_key.plaintext);
        let write_auth = format!("Bearer {}", write_key.plaintext);

        // スキーマ登録はschema scope必須: write scopeキーでは403。
        let definition_v1 = serde_json::json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "title": { "type": "string", "required": true } } }
            },
        });
        let response = rest_request(
            &app,
            "POST",
            "/api/schemas",
            Some(&write_auth),
            Some(definition_v1.clone()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let response = rest_request(
            &app,
            "POST",
            "/api/schemas",
            Some(&schema_auth),
            Some(definition_v1),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = rest_json_body(response).await;
        assert_eq!(created["schema"]["version"], 1);
        assert_eq!(created["diff"]["is_breaking"], false);
        let schema_v1_id = created["schema"]["id"].as_str().unwrap().to_string();

        let response = rest_request(
            &app,
            "GET",
            "/api/schemas/active/task-management",
            Some(&write_auth),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let active = rest_json_body(response).await;
        assert_eq!(active["id"], schema_v1_id.as_str());

        let response = rest_request(
            &app,
            "GET",
            &format!("/api/schemas/{schema_v1_id}"),
            Some(&write_auth),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // entity_typeのJSON Schema投影。requiredにtitleが含まれる。
        let response = rest_request(
            &app,
            "GET",
            "/api/schemas/active/task-management/entity-types/task/json-schema",
            Some(&write_auth),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let json_schema = rest_json_body(response).await;
        assert_eq!(json_schema["type"], "object");
        assert!(
            json_schema["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("title"))
        );

        // 必須フィールド追加を含むv2は破壊的変更としてdiffに報告され、activeが切り替わる。
        let response = rest_request(
            &app,
            "POST",
            "/api/schemas",
            Some(&schema_auth),
            Some(serde_json::json!({
                "name": "task-management",
                "entity_types": {
                    "task": {
                        "fields": {
                            "title": { "type": "string", "required": true },
                            "due": { "type": "string", "required": true }
                        }
                    }
                },
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let created_v2 = rest_json_body(response).await;
        assert_eq!(created_v2["schema"]["version"], 2);
        assert_eq!(created_v2["diff"]["is_breaking"], true);

        let response = rest_request(
            &app,
            "GET",
            "/api/schemas/active/task-management",
            Some(&write_auth),
            None,
        )
        .await;
        let active = rest_json_body(response).await;
        assert_eq!(active["version"], 2);
    }
}
