//! WebSocket endpoint wrapper over `axum::extract::ws`.

use std::future::Future;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

use super::channel::{Broadcast, ChannelError};
use super::error::{RealtimeError, admission_status};
use super::origin::OriginPolicy;
use super::registry::Registry;
use super::shutdown::ShutdownConfig;

/// A WebSocket authorizer. The closure returns `Some(broadcast)` to admit
/// (the returned broadcast is the channel to subscribe to) or `None` to deny.
/// Channel names never implicitly authorize.
pub trait Authorizer: Clone + Send + Sync + 'static {
    /// Authorize a connection. Returns the broadcast to subscribe to, or
    /// `None` to deny.
    fn authorize(
        &self,
        headers: &HeaderMap,
        channel_id: &str,
    ) -> impl Future<Output = Option<Broadcast>> + Send;
}

/// A trivial authorizer that admits all connections to a fixed broadcast
/// (for tests).
#[derive(Clone)]
pub struct AllowAll {
    broadcast: Broadcast,
}

impl AllowAll {
    /// Create an authorizer that admits all to the given broadcast.
    #[must_use]
    pub fn new(broadcast: Broadcast) -> Self {
        Self { broadcast }
    }
}

impl Authorizer for AllowAll {
    fn authorize(
        &self,
        _headers: &HeaderMap,
        _channel_id: &str,
    ) -> impl Future<Output = Option<Broadcast>> + Send {
        let broadcast = self.broadcast.clone();
        std::future::ready(Some(broadcast))
    }
}

/// WebSocket limits (message size, frame size, heartbeat).
#[derive(Debug, Clone, Copy)]
pub struct WsLimits {
    /// The maximum message size in bytes.
    pub max_message_size: usize,
    /// The maximum frame size in bytes.
    pub max_frame_size: usize,
    /// The heartbeat ping interval.
    pub heartbeat_interval: Duration,
    /// The heartbeat timeout (how long to wait for a pong).
    pub heartbeat_timeout: Duration,
}

impl WsLimits {
    /// Conservative defaults: 64 KiB caps, 20s heartbeat, 40s timeout.
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            max_message_size: 64 * 1024,
            max_frame_size: 64 * 1024,
            heartbeat_interval: Duration::from_secs(20),
            heartbeat_timeout: Duration::from_secs(40),
        }
    }
}

/// A WebSocket endpoint. Cheap to clone.
#[derive(Clone)]
pub struct WebSocketEndpoint<A: Authorizer> {
    authorizer: A,
    origin: OriginPolicy,
    registry: Registry,
    limits: WsLimits,
    shutdown: ShutdownConfig,
}

impl<A: Authorizer> std::fmt::Debug for WebSocketEndpoint<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketEndpoint")
            .field("limits", &self.limits)
            .field("max_connections", &self.shutdown.max_connections())
            .finish_non_exhaustive()
    }
}

impl<A: Authorizer> WebSocketEndpoint<A> {
    /// Create a WebSocket endpoint.
    #[must_use]
    pub fn new(
        authorizer: A,
        origin: OriginPolicy,
        registry: Registry,
        limits: WsLimits,
        shutdown: ShutdownConfig,
    ) -> Self {
        Self {
            authorizer,
            origin,
            registry,
            limits,
            shutdown,
        }
    }

    /// Handle a WebSocket upgrade request. Admission order: origin, authz,
    /// connection limit (all pre-upgrade, return clean HTTP status), then
    /// `ws.max_message_size(...).max_frame_size(...).on_upgrade(run_connection)`.
    pub async fn handle(
        self,
        ws: WebSocketUpgrade,
        headers: HeaderMap,
        channel_id: String,
    ) -> Response {
        // Origin check.
        let origin_header = headers.get("origin").cloned();
        if matches!(
            self.origin.authorize(origin_header.as_ref()),
            super::origin::OriginDecision::Denied
        ) {
            return admission_status(&RealtimeError::Origin).into_response();
        }

        // Authorization.
        let broadcast = match self.authorizer.authorize(&headers, &channel_id).await {
            Some(b) => b,
            None => return admission_status(&RealtimeError::Unauthorized).into_response(),
        };

        // Connection limit.
        let guard = match self.registry.acquire(self.shutdown.max_connections()) {
            Ok(g) => g,
            Err(e) => return admission_status(&e).into_response(),
        };

        let limits = self.limits;
        let shutdown = self.shutdown.clone();
        ws.max_message_size(limits.max_message_size)
            .max_frame_size(limits.max_frame_size)
            .on_upgrade(move |socket| run_connection(socket, broadcast, guard, limits, shutdown))
    }
}

/// The connection loop. Reads from the client and the broadcast, pings on
/// heartbeat (from `WsLimits`), and closes gracefully on drain or when a
/// pong is not received within the heartbeat timeout.
async fn run_connection(
    mut socket: WebSocket,
    broadcast: Broadcast,
    _guard: super::registry::ConnectionGuard,
    limits: WsLimits,
    shutdown: ShutdownConfig,
) {
    use futures::sink::SinkExt;

    let mut sub = broadcast.subscribe();
    let mut heartbeat = tokio::time::interval(limits.heartbeat_interval);
    heartbeat.tick().await; // first tick is immediate

    // Track the last pong time. A healthy client responds to pings; if the
    // time since the last pong exceeds the heartbeat timeout, the peer is
    // presumed dead and the connection is closed.
    let mut last_pong = tokio::time::Instant::now();

    loop {
        tokio::select! {
            _ = shutdown.drain_notified() => {
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(msg)) => {
                        match msg {
                            Message::Close(_) => break,
                            Message::Pong(_) => {
                                last_pong = tokio::time::Instant::now();
                            }
                            _ => {}
                        }
                    }
                    Some(Err(_)) | None => break,
                }
            }
            res = sub.recv() => {
                match res {
                    Ok(payload) => {
                        let _ = socket
                            .send(Message::Binary(bytes::Bytes::from(
                                payload.as_bytes().to_vec(),
                            )))
                            .await;
                    }
                    Err(ChannelError::Closed) => break,
                    Err(ChannelError::Lagged) => continue,
                    Err(ChannelError::Full) => continue,
                }
            }
            _ = heartbeat.tick() => {
                // Enforce pong timeout: if no pong within the timeout window,
                // the peer is dead. Close the connection.
                let elapsed = tokio::time::Instant::now().duration_since(last_pong);
                if elapsed > limits.heartbeat_timeout {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
                let _ = socket.send(Message::Ping(bytes::Bytes::new())).await;
            }
        }
    }
}
