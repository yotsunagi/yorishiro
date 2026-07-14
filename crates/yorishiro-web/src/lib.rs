//! Serves the Yorishiro setup/login/admin-dashboard SPA (`web/` at the repo root), compiled
//! into the binary at build time via `rust-embed`. This is what lets `yorishiro-server` (and,
//! via that crate's `build_app`, `yorishiro-hosted-server` too -- see that repo) serve a working
//! web UI without a deployment needing to separately fetch and place a `web/` directory
//! alongside the binary; the release tarball and Docker image both only ever shipped the binary
//! itself.
//!
//! An operator actively iterating on `web/`'s contents can still point at a real directory on
//! disk instead of the compiled-in copy (`YSR_WEB_DIR` in yorishiro-server,
//! `YORISHIRO_HOSTED_WEB_DIR` in yorishiro-hosted-server) -- see [`fallback_service`]. That
//! directory is read fresh on every request, so edits show up without a rebuild.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/"]
struct Assets;

/// Maps a request path to the asset path it should serve: `/` (and the empty path) map to
/// `index.html`, same as `ServeDir`'s default `index_file` behavior; everything else is used
/// as-is, relative to `web/`.
fn asset_path(uri_path: &str) -> &str {
    match uri_path.trim_start_matches('/') {
        "" => "index.html",
        other => other,
    }
}

fn respond(path: &str, bytes: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(bytes))
        // A well-formed content-type header value and a non-streaming body never fail to
        // build a response.
        .expect("building a static-asset response is infallible")
}

fn serve_embedded(path: &str) -> Response {
    match Assets::get(path) {
        Some(file) => respond(path, file.data.into_owned()),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn serve_from_disk(dir: &Path, path: &str) -> Response {
    match tokio::fs::read(dir.join(path)).await {
        Ok(bytes) => respond(path, bytes),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A fallback service (for `Router::fallback_service`) that serves the SPA's static files.
/// `override_dir`, when `Some`, serves from that directory on disk instead of the assets
/// compiled into the binary -- see the module docs.
pub fn fallback_service(override_dir: Option<String>) -> MethodRouter {
    let override_dir = override_dir.map(PathBuf::from);
    get(move |uri: Uri| {
        let override_dir = override_dir.clone();
        async move {
            let path = asset_path(uri.path()).to_string();
            match override_dir {
                Some(dir) => serve_from_disk(&dir, &path).await,
                None => serve_embedded(&path),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::to_bytes;
    use tower::ServiceExt;

    async fn get(router: Router, uri: &str) -> Response {
        router
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn serves_index_html_at_root_from_embedded_assets() {
        let router = Router::new().fallback_service(fallback_service(None));

        let response = get(router, "/").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8(body.to_vec()).unwrap().contains("<html"));
    }

    #[tokio::test]
    async fn serves_a_named_asset_with_the_right_content_type() {
        let router = Router::new().fallback_service(fallback_service(None));

        let response = get(router, "/app.js").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/javascript"
        );
    }

    #[tokio::test]
    async fn missing_embedded_asset_is_404() {
        let router = Router::new().fallback_service(fallback_service(None));

        let response = get(router, "/does-not-exist.txt").await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn override_dir_serves_from_disk_instead_of_embedded_assets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<html>from disk</html>").unwrap();
        let router = Router::new().fallback_service(fallback_service(Some(
            dir.path().to_str().unwrap().to_string(),
        )));

        let response = get(router, "/").await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.as_ref(), b"<html>from disk</html>");
    }

    #[tokio::test]
    async fn override_dir_404s_on_a_missing_file_without_falling_back_to_embedded_assets() {
        let dir = tempfile::tempdir().unwrap();
        let router = Router::new().fallback_service(fallback_service(Some(
            dir.path().to_str().unwrap().to_string(),
        )));

        // index.html exists in the embedded assets but not in the (empty) override dir.
        let response = get(router, "/").await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
