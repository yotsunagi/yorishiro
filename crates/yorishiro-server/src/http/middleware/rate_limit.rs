use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// A per-key fixed-window rate limiter for `/auth/signup` and `/auth/login` -- the only two
/// endpoints reachable without a bearer token, and therefore the only ones exposed to
/// unauthenticated credential/invite-token brute-forcing. Keyed by client IP; falls back to a
/// single shared bucket when no `ConnectInfo` is present on the request (e.g. tests driven
/// through `Router::oneshot`, which never populates it).
pub struct RateLimiter {
    max_requests: u32,
    window: Duration,
    buckets: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            max_requests,
            window,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// `YSR_AUTH_RATE_LIMIT_MAX` (default 10) requests per `YSR_AUTH_RATE_LIMIT_WINDOW_SECS`
    /// (default 60) seconds, per client IP.
    pub fn from_env() -> Self {
        let max_requests = std::env::var("YSR_AUTH_RATE_LIMIT_MAX")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        let window_secs = std::env::var("YSR_AUTH_RATE_LIMIT_WINDOW_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);
        Self::new(max_requests, Duration::from_secs(window_secs))
    }

    /// Returns `true` if this call is within the limit, `false` if `key` has exhausted its
    /// quota for the current window. The window resets lazily on the first call after it
    /// elapses, rather than on a background timer.
    fn allow(&self, key: &str) -> bool {
        let mut buckets = self.buckets.lock().expect("rate limiter mutex poisoned");
        let now = Instant::now();
        let entry = buckets.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= self.max_requests
    }
}

pub async fn enforce(
    State(limiter): State<std::sync::Arc<RateLimiter>>,
    req: Request,
    next: Next,
) -> Response {
    let key = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    if !limiter.allow(&key) {
        // Logged so an operator can see abuse (credential/invite-token brute-forcing) that
        // the access log would otherwise show only as anonymous 429s.
        tracing::warn!(client = %key, path = %req.uri().path(), "auth rate limit exceeded");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_requests_within_the_limit() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));
        assert!(limiter.allow("1.2.3.4"));
        assert!(limiter.allow("1.2.3.4"));
        assert!(limiter.allow("1.2.3.4"));
    }

    #[test]
    fn rejects_requests_past_the_limit() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        assert!(limiter.allow("1.2.3.4"));
        assert!(limiter.allow("1.2.3.4"));
        assert!(!limiter.allow("1.2.3.4"));
    }

    #[test]
    fn tracks_separate_keys_independently() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        assert!(limiter.allow("1.2.3.4"));
        assert!(limiter.allow("5.6.7.8"));
        assert!(!limiter.allow("1.2.3.4"));
    }

    #[test]
    fn resets_after_the_window_elapses() {
        let limiter = RateLimiter::new(1, Duration::from_millis(50));
        assert!(limiter.allow("1.2.3.4"));
        assert!(!limiter.allow("1.2.3.4"));
        std::thread::sleep(Duration::from_millis(60));
        assert!(limiter.allow("1.2.3.4"));
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn logs_a_warning_when_the_rate_limit_is_exceeded() {
        use axum::Router;
        use axum::body::Body;
        use axum::http::Request;
        use axum::routing::get;
        use tower::ServiceExt;

        let limiter = std::sync::Arc::new(RateLimiter::new(1, Duration::from_secs(60)));
        let app = Router::new()
            .route("/probe", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(limiter, enforce));

        // First request consumes the only allowed slot for this test's shared bucket
        // (no ConnectInfo is populated by `oneshot`, so every call falls into "unknown").
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(!logs_contain("auth rate limit exceeded"));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(logs_contain("auth rate limit exceeded"));
    }
}
