use axum::{Router, routing::get};
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tower_http::cors::{AllowHeaders, AllowMethods, CorsLayer};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::http::{controllers, mcp};
use crate::state::AppState;
use crate::{health, whoami};

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
