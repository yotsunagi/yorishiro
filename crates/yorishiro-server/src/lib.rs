use std::sync::Arc;

use anyhow::Result;
use axum::{Router, routing::get};
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tower_http::cors::{AllowHeaders, AllowMethods, CorsLayer};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;
use yorishiro_core::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use yorishiro_core::embedding_onnx::{LocalOnnxConfig, LocalOnnxProvider};

pub mod admin;
mod auth;
mod error;
mod health;
pub mod logging;
mod mcp;
mod rate_limit;
mod rest;
mod state;
mod whoami;

pub use state::AppState;

/// Starts a graceful shutdown on either SIGTERM (the standard stop signal from container
/// orchestrators) or Ctrl-C.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install ctrl-c handler");
    };

    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received, draining connections");
}

/// Builds the embeddings provider from environment variables. `YSR_EMBEDDING_PROVIDER`
/// switches between `local` (a local ONNX model, the default -- needs no external service or
/// API key, just the model files under `models/`) and `openai` (an OpenAI-compatible API, for
/// operators already running something like Ollama/LM Studio). The `entities.embedding`
/// column is fixed at `vector(768)`, so a mismatched dimension count is rejected at startup
/// (for `local`, a probe inference further verifies the model's actual output dimension).
pub fn build_embedding_provider() -> Result<Arc<dyn EmbeddingProvider>> {
    let dimensions: usize = std::env::var("YSR_EMBEDDING_DIMENSIONS")
        .unwrap_or_else(|_| "768".into())
        .parse()?;
    if dimensions != 768 {
        anyhow::bail!(
            "YSR_EMBEDDING_DIMENSIONS must be 768 (entities.embedding is vector(768)), got {dimensions}"
        );
    }

    let kind = std::env::var("YSR_EMBEDDING_PROVIDER").unwrap_or_else(|_| "local".into());
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
                    .unwrap_or_else(|_| "models/model.onnx".into())
                    .into(),
                tokenizer_path: std::env::var("YSR_ONNX_TOKENIZER_PATH")
                    .unwrap_or_else(|_| "models/tokenizer.json".into())
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

/// The routing configuration itself needs to be identical between `main` and the
/// integration tests, so it's factored into a function that builds the app from just an
/// `AppState`. The setup/login SPA (see `web/`, compiled into the binary via `yorishiro-web`)
/// is always mounted as the fallback for any path not matched by an API route -- this is how
/// this process's first-run setup wizard and the hosted dashboard's static assets share the
/// same `web/` tree while each process opts in independently (`YSR_WEB_DIR` here vs.
/// `YORISHIRO_HOSTED_WEB_DIR` in `yorishiro-hosted-server`). `web_dir`, when set, serves that
/// SPA from a real directory on disk instead of the compiled-in copy, for local iteration on
/// `web/` without a rebuild -- see `yorishiro_web::fallback_service`. Exposed publicly so a
/// deployment that wants a single process (e.g. `yorishiro-hosted-server` embedding the full
/// community server) can build this same router and merge its own routes into it, rather than
/// running two separate processes.
pub fn build_app(state: AppState, web_dir: Option<String>) -> Router {
    let cors = build_cors_layer();
    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(mcp::YorishiroMcpServer::new(state.clone()))
        },
        LocalSessionManager::default().into(),
        Default::default(),
    );

    let router = Router::new()
        .route("/up", get(health::up_check))
        .route("/health", get(health::health_check))
        .route("/whoami", get(whoami::whoami))
        .nest_service("/mcp", mcp_service)
        .merge(rest::router())
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", rest::ApiDoc::openapi()))
        .layer(cors)
        .layer(
            // The default span/response levels are DEBUG, which a production `RUST_LOG=info`
            // silently drops — raised to INFO so the access log (method, path, status,
            // latency) actually reaches whichever target `logging::init` selected.
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(tracing::Level::INFO))
                .on_response(DefaultOnResponse::new().level(tracing::Level::INFO)),
        )
        .with_state(state);

    router.fallback_service(yorishiro_web::fallback_service(web_dir))
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
    use yorishiro_core::tenancy;

    use super::*;
    use yorishiro_core::db::TenantDb;

    /// Tests shouldn't call out to a remote embeddings service, so this dummy provider only
    /// satisfies the dimension count (and errors immediately if actually invoked).
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
        AppState::new(
            TenantDb::new(pool.clone()),
            pool,
            Arc::new(UnreachableEmbeddingProvider),
        )
    }

    /// A provider that returns a deterministic vector, for end-to-end tests of the embedding
    /// wiring. Every text maps to the same vector, so the distance between query and entity
    /// is always 0 — guaranteeing a hit.
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

    async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
        let (tenant_id,): (Uuid,) =
            sqlx::query_as("INSERT INTO identity.tenants (name) VALUES ($1) RETURNING id")
                .bind("test-tenant")
                .fetch_one(pool)
                .await
                .unwrap();
        let (workspace_id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO identity.workspaces (tenant_id, name) VALUES ($1, $2) RETURNING id",
        )
        .bind(tenant_id)
        .bind("test-workspace")
        .fetch_one(pool)
        .await
        .unwrap();
        (tenant_id, workspace_id)
    }

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

    /// Extracts the `data: {...}` line from a `text/event-stream` body and parses it as JSON.
    /// streamable-http returns multiple events separated by `\n\n`, but the response to a
    /// single request is carried in the last one, so that's the one targeted.
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

    /// Performs the initialize + notifications/initialized handshake and returns the
    /// session ID to use for subsequent tools/call requests.
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
        let app = build_app(test_state(pool), None);
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

    /// Fills each tool's required arguments with dummy values that only satisfy their types.
    /// The authorization check runs after argument deserialization, so for this test's goal
    /// (catching missing authorization checks) to hold, the arguments themselves must
    /// already be well-formed.
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
            "list_schemas" => serde_json::json!({}),
            "get_active_schema" => serde_json::json!({ "name": "dummy" }),
            "get_schema_by_id" => serde_json::json!({ "schema_id": NIL_UUID }),
            "create_schema" => serde_json::json!({ "definition": {} }),
            "get_entity_type_json_schema" => serde_json::json!({
                "schema_name": "dummy", "entity_type": "dummy",
            }),
            "search_entities" => serde_json::json!({ "query_text": "dummy" }),
            "recall_context" => serde_json::json!({ "entity_id": NIL_UUID }),
            "list_templates" => serde_json::json!({}),
            other => panic!("no dummy arguments registered for tool `{other}`"),
        }
    }

    /// Mechanically verifies, for every tool enumerated by `tools/list`, that a missing
    /// Authorization header always produces a protocol error — so that an oversight in one
    /// tool's checks can't slip in unnoticed in the future.
    #[sqlx::test(migrations = "../../migrations")]
    async fn every_registered_tool_requires_an_authorization_header(pool: PgPool) {
        let app = build_app(test_state(pool), None);
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
            17,
            "expected 17 registered tools, got {tool_names:?}"
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
        let (tenant_id_tenant, tenant_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db
            .acquire_for_workspace(tenant_id_tenant, tenant_id)
            .await
            .unwrap();
        let created = create_api_key(&mut conn, tenant_id, ApiKeyScope::Read, None)
            .await
            .unwrap();
        drop(conn);

        let app = build_app(
            AppState::new(db, pool.clone(), Arc::new(UnreachableEmbeddingProvider)),
            None,
        );
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
        let (tenant_id_tenant, tenant_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db
            .acquire_for_workspace(tenant_id_tenant, tenant_id)
            .await
            .unwrap();
        let created = create_api_key(&mut conn, tenant_id, ApiKeyScope::Read, None)
            .await
            .unwrap();
        drop(conn);

        let app = build_app(
            AppState::new(db, pool.clone(), Arc::new(UnreachableEmbeddingProvider)),
            None,
        );
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
        let app = build_app(test_state(pool), None);

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

        // Verify every $ref referenced by paths' responses/request_body actually exists
        // under components.schemas (a regression test for utoipa's `paths(...)` listing
        // correctly auto-collecting schemas).
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
        let app = build_app(test_state(pool), None);

        let response = rest_request(&app, "GET", "/api/entities", None, None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_entities_endpoint_rejects_an_unknown_bearer_token(pool: PgPool) {
        let app = build_app(test_state(pool), None);

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
        let (tenant_id_tenant, tenant_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db
            .acquire_for_workspace(tenant_id_tenant, tenant_id)
            .await
            .unwrap();
        let created = create_api_key(&mut conn, tenant_id, ApiKeyScope::Read, None)
            .await
            .unwrap();
        drop(conn);

        let app = build_app(
            AppState::new(db, pool.clone(), Arc::new(UnreachableEmbeddingProvider)),
            None,
        );

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

    /// Verifies the full production path over REST: entity creation, background
    /// embedding sync, then a vector search hit. The embedding sync is a fire-and-forget
    /// background task, so this polls at short intervals until the search hits.
    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_created_entity_becomes_searchable(pool: PgPool) {
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
            AppState::new(db, pool.clone(), Arc::new(FixedEmbeddingProvider)),
            None,
        );
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
        let (tenant_a_tenant, tenant_a) = seed_workspace(&pool).await;
        let (tenant_b_tenant, tenant_b) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool.clone());

        let mut conn_a = db
            .acquire_for_workspace(tenant_a_tenant, tenant_a)
            .await
            .unwrap();
        let schema_key_a = create_api_key(&mut conn_a, tenant_a, ApiKeyScope::Schema, None)
            .await
            .unwrap();
        let write_key_a = create_api_key(&mut conn_a, tenant_a, ApiKeyScope::Write, None)
            .await
            .unwrap();
        drop(conn_a);

        let mut conn_b = db
            .acquire_for_workspace(tenant_b_tenant, tenant_b)
            .await
            .unwrap();
        let read_key_b = create_api_key(&mut conn_b, tenant_b, ApiKeyScope::Read, None)
            .await
            .unwrap();
        drop(conn_b);

        let app = build_app(
            AppState::new(db, pool.clone(), Arc::new(UnreachableEmbeddingProvider)),
            None,
        );
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

        // Tenant B's key sees tenant A's entity as if it doesn't exist, hence 404.
        let response = rest_request(
            &app,
            "GET",
            &format!("/api/entities/{entity_id}"),
            Some(&read_auth_b),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = rest_request(&app, "GET", "/api/entities", Some(&read_auth_b), None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let listed = rest_json_body(response).await;
        assert_eq!(listed.as_array().unwrap().len(), 0);
    }

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

    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_schema_endpoints_round_trip(pool: PgPool) {
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

        // Registering a schema requires schema scope: a write-scope key gets 403.
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

        // v2, which adds a required field, is reported in the diff as a breaking change,
        // and active switches to it.
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

        let response = rest_request(&app, "GET", "/api/schemas", Some(&write_auth), None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let listed = rest_json_body(response).await;
        let listed = listed.as_array().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["version"], 1);
        assert_eq!(listed[0]["status"], "archived");
        assert_eq!(listed[1]["version"], 2);
        assert_eq!(listed[1]["status"], "active");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_creates_a_schema_from_a_built_in_template(pool: PgPool) {
        let (tenant_id_tenant, tenant_id) = seed_workspace(&pool).await;
        let db = TenantDb::new(pool.clone());
        let mut conn = db
            .acquire_for_workspace(tenant_id_tenant, tenant_id)
            .await
            .unwrap();
        let schema_key = create_api_key(&mut conn, tenant_id, ApiKeyScope::Schema, None)
            .await
            .unwrap();
        drop(conn);

        let app = build_app(
            AppState::new(db, pool.clone(), Arc::new(UnreachableEmbeddingProvider)),
            None,
        );
        let schema_auth = format!("Bearer {}", schema_key.plaintext);

        let response = rest_request(&app, "GET", "/api/templates", Some(&schema_auth), None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let templates = rest_json_body(response).await;
        let templates = templates.as_array().unwrap();
        assert!(templates.iter().any(|t| t["id"] == "task-management"));

        let response = rest_request(
            &app,
            "POST",
            "/api/schemas",
            Some(&schema_auth),
            Some(serde_json::json!({ "template_id": "task-management" })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = rest_json_body(response).await;
        assert_eq!(created["schema"]["name"], "task-management");
        assert_eq!(created["schema"]["version"], 1);

        let response = rest_request(
            &app,
            "POST",
            "/api/schemas",
            Some(&schema_auth),
            Some(serde_json::json!({ "template_id": "does-not-exist" })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rest_export_jsonl_streams_every_record_for_the_tenant(pool: PgPool) {
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
                "relation_types": {
                    "blocks": { "source": "task", "target": "task" }
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
                "schema_name": "task-management", "entity_type": "task", "data": { "title": "a" },
            })),
        )
        .await;
        let a = rest_json_body(response).await;

        let response = rest_request(
            &app,
            "POST",
            "/api/entities",
            Some(&write_auth),
            Some(serde_json::json!({
                "schema_name": "task-management", "entity_type": "task", "data": { "title": "b" },
            })),
        )
        .await;
        let b = rest_json_body(response).await;

        let response = rest_request(
            &app,
            "POST",
            "/api/relations",
            Some(&write_auth),
            Some(serde_json::json!({
                "source_id": a["id"], "target_id": b["id"], "relation_type": "blocks",
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);

        let response =
            rest_request(&app, "GET", "/api/export.jsonl", Some(&write_auth), None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/x-ndjson"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let lines: Vec<serde_json::Value> = std::str::from_utf8(&body)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();

        assert_eq!(lines.iter().filter(|l| l["kind"] == "schema").count(), 1);
        assert_eq!(lines.iter().filter(|l| l["kind"] == "entity").count(), 2);
        assert_eq!(lines.iter().filter(|l| l["kind"] == "relation").count(), 1);

        let response = rest_request(&app, "GET", "/api/export.jsonl", None, None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn signup_consumes_invite_and_creates_membership(pool: PgPool) {
        let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
        let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None)
            .await
            .unwrap();
        let (_invite, token) = tenancy::create_invite(
            &pool,
            tenant.id,
            "new@example.com",
            tenancy::MembershipRole::Member,
            chrono::Duration::hours(1),
        )
        .await
        .unwrap();

        let app = build_app(test_state(pool.clone()), None);

        let response = rest_request(
            &app,
            "POST",
            "/auth/signup",
            None,
            Some(serde_json::json!({
                "invite_token": token,
                "password": "hunter2-hunter2",
                "display_name": "New Member",
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = rest_json_body(response).await;
        assert_eq!(body["email"], "new@example.com");
        assert_eq!(body["tenant_id"], tenant.id.to_string());
        assert_eq!(body["role"], "member");
        assert_eq!(body["workspaces"][0]["id"], workspace.id.to_string());

        let role = tenancy::get_membership_role(
            &pool,
            tenant.id,
            Uuid::parse_str(body["user_id"].as_str().unwrap()).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(role, Some(tenancy::MembershipRole::Member));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn signup_rejects_an_already_used_invite_token(pool: PgPool) {
        let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
        tenancy::create_workspace(&pool, tenant.id, "main", None)
            .await
            .unwrap();
        let (_invite, token) = tenancy::create_invite(
            &pool,
            tenant.id,
            "reuse@example.com",
            tenancy::MembershipRole::Member,
            chrono::Duration::hours(1),
        )
        .await
        .unwrap();

        let app = build_app(test_state(pool), None);
        let signup_body = Some(serde_json::json!({
            "invite_token": token,
            "password": "hunter2-hunter2",
        }));

        let response = rest_request(&app, "POST", "/auth/signup", None, signup_body.clone()).await;
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = rest_request(&app, "POST", "/auth/signup", None, signup_body).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn login_issues_an_api_key_scoped_to_the_members_role(pool: PgPool) {
        let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
        let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None)
            .await
            .unwrap();
        let (_invite, token) = tenancy::create_invite(
            &pool,
            tenant.id,
            "member@example.com",
            tenancy::MembershipRole::Member,
            chrono::Duration::hours(1),
        )
        .await
        .unwrap();

        let app = build_app(test_state(pool), None);

        let response = rest_request(
            &app,
            "POST",
            "/auth/signup",
            None,
            Some(serde_json::json!({
                "invite_token": token,
                "password": "hunter2-hunter2",
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = rest_request(
            &app,
            "POST",
            "/auth/login",
            None,
            Some(serde_json::json!({
                "email": "member@example.com",
                "password": "hunter2-hunter2",
                "workspace_id": workspace.id,
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = rest_json_body(response).await;
        assert_eq!(body["scope"], "write");
        let api_key = body["api_key"].as_str().unwrap();

        let response = rest_request(
            &app,
            "GET",
            "/api/entities",
            Some(&format!("Bearer {api_key}")),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn login_rejects_an_incorrect_password(pool: PgPool) {
        let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
        let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None)
            .await
            .unwrap();
        tenancy::create_user(&pool, "someone@example.com", "correct-horse", None)
            .await
            .unwrap();

        let app = build_app(test_state(pool), None);

        let response = rest_request(
            &app,
            "POST",
            "/auth/login",
            None,
            Some(serde_json::json!({
                "email": "someone@example.com",
                "password": "wrong-password",
                "workspace_id": workspace.id,
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn auth_endpoints_are_rate_limited_per_caller(pool: PgPool) {
        let app = build_app(test_state(pool), None);

        // The test driver never populates `ConnectInfo`, so every call here falls back to the
        // same shared bucket -- exercising the same "no requester info" path a request behind
        // an unconfigured proxy would take, while still proving the middleware is wired in.
        let mut saw_too_many_requests = false;
        for _ in 0..15 {
            let response = rest_request(
                &app,
                "POST",
                "/auth/login",
                None,
                Some(serde_json::json!({
                    "email": "nobody@example.com",
                    "password": "wrong",
                    "workspace_id": Uuid::nil(),
                })),
            )
            .await;
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                saw_too_many_requests = true;
                break;
            }
        }

        assert!(
            saw_too_many_requests,
            "expected /auth/login to start returning 429 after repeated calls from the same caller"
        );
    }

    /// Issues an API key attributed to `user_id`, scoped to `role`'s max scope -- exactly what
    /// `/auth/login` would hand out for that role.
    async fn issue_key_for(
        pool: &PgPool,
        tenant_id: Uuid,
        workspace_id: Uuid,
        user_id: Uuid,
        role: tenancy::MembershipRole,
    ) -> String {
        let db = TenantDb::new(pool.clone());
        let mut conn = db
            .acquire_for_workspace(tenant_id, workspace_id)
            .await
            .unwrap();
        create_api_key(&mut conn, workspace_id, role.max_scope(), Some(user_id))
            .await
            .unwrap()
            .plaintext
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn owner_can_list_and_add_members(pool: PgPool) {
        let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
        let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None)
            .await
            .unwrap();
        let owner = tenancy::create_user(&pool, "owner@example.com", "hunter2-hunter2", None)
            .await
            .unwrap();
        tenancy::add_member(&pool, tenant.id, owner.id, tenancy::MembershipRole::Owner)
            .await
            .unwrap();
        let owner_key = issue_key_for(
            &pool,
            tenant.id,
            workspace.id,
            owner.id,
            tenancy::MembershipRole::Owner,
        )
        .await;

        // The invitee must already have an account before they can be added by email.
        let invitee = tenancy::create_user(&pool, "invitee@example.com", "hunter2-hunter2", None)
            .await
            .unwrap();

        let app = build_app(test_state(pool.clone()), None);

        let response = rest_request(
            &app,
            "POST",
            "/api/members",
            Some(&format!("Bearer {owner_key}")),
            Some(serde_json::json!({
                "email": "invitee@example.com",
                "role": "member",
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = rest_json_body(response).await;
        assert_eq!(body["user_id"], invitee.id.to_string());
        assert_eq!(body["role"], "member");

        let response = rest_request(
            &app,
            "GET",
            "/api/members",
            Some(&format!("Bearer {owner_key}")),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = rest_json_body(response).await;
        let emails: Vec<&str> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["email"].as_str().unwrap())
            .collect();
        assert!(emails.contains(&"owner@example.com"));
        assert!(emails.contains(&"invitee@example.com"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn add_member_rejects_an_email_with_no_account(pool: PgPool) {
        let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
        let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None)
            .await
            .unwrap();
        let owner = tenancy::create_user(&pool, "owner@example.com", "hunter2-hunter2", None)
            .await
            .unwrap();
        tenancy::add_member(&pool, tenant.id, owner.id, tenancy::MembershipRole::Owner)
            .await
            .unwrap();
        let owner_key = issue_key_for(
            &pool,
            tenant.id,
            workspace.id,
            owner.id,
            tenancy::MembershipRole::Owner,
        )
        .await;

        let app = build_app(test_state(pool), None);

        let response = rest_request(
            &app,
            "POST",
            "/api/members",
            Some(&format!("Bearer {owner_key}")),
            Some(serde_json::json!({
                "email": "nobody@example.com",
                "role": "member",
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn member_role_cannot_manage_members(pool: PgPool) {
        let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
        let workspace = tenancy::create_workspace(&pool, tenant.id, "main", None)
            .await
            .unwrap();
        let member = tenancy::create_user(&pool, "member@example.com", "hunter2-hunter2", None)
            .await
            .unwrap();
        tenancy::add_member(&pool, tenant.id, member.id, tenancy::MembershipRole::Member)
            .await
            .unwrap();
        let member_key = issue_key_for(
            &pool,
            tenant.id,
            workspace.id,
            member.id,
            tenancy::MembershipRole::Member,
        )
        .await;

        let app = build_app(test_state(pool), None);

        let response = rest_request(
            &app,
            "GET",
            "/api/members",
            Some(&format!("Bearer {member_key}")),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn members_endpoints_require_authentication(pool: PgPool) {
        let app = build_app(test_state(pool), None);

        let response = rest_request(&app, "GET", "/api/members", None, None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
