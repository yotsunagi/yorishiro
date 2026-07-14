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
}
