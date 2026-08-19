//! Realtime WebSocket + SSE wrappers over axum.
//!
//! Typed, thin wrappers over raw `axum::extract::ws` and
//! `axum::response::sse`. No proprietary realtime protocol, no
//! `X-Arcature-*` headers. The fan-out core is a bounded
//! `tokio::sync::broadcast` channel. Raw `axum::extract::ws` /
//! `axum::response::sse` remain first-class escape hatches.
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
