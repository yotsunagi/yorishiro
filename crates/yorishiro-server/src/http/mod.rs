//! The HTTP-facing layer: REST controllers, the MCP adapter (a second protocol surface over
//! the same domain logic), and the middleware both share (bearer-token auth, rate limiting).
//! Routing itself lives one level up, in `crate::routes`, which mounts `controllers::router()`
//! and `mcp::YorishiroMcpServer` onto one `axum::Router`.

pub(crate) mod controllers;
pub(crate) mod mcp;
pub(crate) mod middleware;
