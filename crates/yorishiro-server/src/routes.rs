use axum::{Router, routing::get};
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::http::controllers::{health, whoami};
use crate::http::{controllers, mcp};
use crate::state::AppState;

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
        .merge(controllers::router())
        .merge(
            SwaggerUi::new("/docs").url("/api-docs/openapi.json", controllers::ApiDoc::openapi()),
        )
        .layer(cors)
        // Copies the resolved `x-request-id` onto the response so a caller or proxy can
        // correlate its request with this server's logs.
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(
            // The default span/response levels are DEBUG, which a production `RUST_LOG=info`
            // silently drops — raised to INFO so the access log (method, path, status,
            // latency) actually reaches whichever target `logging::init` selected. The span
            // carries `request_id` so any warn/error emitted while handling a request
            // correlates with its access-log line.
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::extract::Request| {
                    let request_id = request
                        .headers()
                        .get("x-request-id")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default();
                    tracing::info_span!(
                        "request",
                        %request_id,
                        method = %request.method(),
                        uri = %request.uri(),
                    )
                })
                .on_response(DefaultOnResponse::new().level(tracing::Level::INFO)),
        )
        // Generates an `x-request-id` (UUID) when the incoming request lacks one. Added last so
        // it is the outermost layer and runs before the trace span above reads the header.
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state);

    router.fallback_service(yorishiro_web::fallback_service(web_dir))
}

fn build_cors_layer() -> CorsLayer {
    let origins_str = std::env::var("YSR_CORS_ORIGINS").unwrap_or_default();

    let layer = if !origins_str.is_empty() {
        let origins: Vec<_> = origins_str
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        CorsLayer::new().allow_origin(origins)
    } else if cfg!(debug_assertions) {
        // Debug builds only: with no explicit YSR_CORS_ORIGINS, allow any localhost/127.0.0.1
        // port so browser-based dev tools (e.g. the MCP Inspector) can reach this server
        // without requiring a manually configured origin list. Release builds never take this
        // branch, so the all-reject default (below) is unaffected in production.
        debug_local_origin_layer()
    } else {
        CorsLayer::new()
    };

    layer
        .allow_methods(AllowMethods::any())
        .allow_headers(AllowHeaders::list([
            "authorization".parse().unwrap(),
            "content-type".parse().unwrap(),
        ]))
        .expose_headers(["x-request-id".parse().unwrap()])
}

/// Matches `http://localhost:<any port>` and `http://127.0.0.1:<any port>` origins. Only
/// reached from a debug build with `YSR_CORS_ORIGINS` unset (see `build_cors_layer`).
fn debug_local_origin_layer() -> CorsLayer {
    CorsLayer::new().allow_origin(AllowOrigin::predicate(|origin, _parts| {
        origin
            .to_str()
            .map(|s| s.starts_with("http://localhost:") || s.starts_with("http://127.0.0.1:"))
            .unwrap_or(false)
    }))
}
