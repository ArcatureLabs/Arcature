//! Realtime WebSocket + SSE wrappers over axum.
//!
//! Typed, thin wrappers over raw `axum::extract::ws` and
//! `axum::response::sse`. No proprietary realtime protocol, no
//! `X-Arcature-*` headers. The fan-out core is a bounded
//! `tokio::sync::broadcast` channel. Raw `axum::extract::ws` /
//! `axum::response::sse` remain first-class escape hatches.
//!
//! # Fan-out reaches one process
//!
//! `tokio::sync::broadcast` is a channel between tasks inside a single
//! process, so a message published on one instance reaches only the
//! subscribers connected to *that* instance. Run two instances behind a
//! load balancer and roughly half of every broadcast is missing from any
//! given client's view. Nothing errors and nothing warns -- the message is
//! delivered correctly to everyone the channel can see, and the channel
//! cannot see the other process.
//!
//! This is the one limit here that has no configuration switch, unlike
//! sessions (`session-store-db`) or rate limiting
//! ([`RateLimit::redis`](crate::routing::RateLimit)). The deployment guide
//! lists the three ways to live with it; the short version is to run one
//! instance, or to pin realtime upgrades to one instance, until a
//! cross-process bridge exists.
//!
//! Realtime is app-owned, not engine-owned: the app constructs
//! [`Broadcast`], [`Registry`], and [`ShutdownConfig`] once, stores them in
//! `AppState`, and clones [`WebSocketEndpoint`] / [`SseEndpoint`] into
//! handlers.

mod channel;
mod error;
mod origin;
mod registry;
mod shutdown;
mod sse;
mod websocket;

pub use channel::{Broadcast, ChannelError, ChannelPayload, Subscription};
pub use error::{ProtocolHint, RealtimeError, admission_status};
pub use origin::{OriginDecision, OriginPolicy, VerifiedOrigin};
pub use registry::{ConnectionGuard, Registry};
pub use shutdown::ShutdownConfig;
pub use sse::{SseEndpoint, SseLimits};
pub use websocket::{AllowAll, Authorizer, WebSocketEndpoint, WsLimits};

/// Drain all realtime connections within `bound`. Flips the shutdown config
/// to draining, then waits for the registry to reach zero live connections
/// (or the bound to elapse).
pub async fn drain(
    registry: &Registry,
    shutdown: &ShutdownConfig,
    bound: std::time::Duration,
) -> Result<(), RealtimeError> {
    shutdown.begin_drain();
    registry.drain(bound).await
}
