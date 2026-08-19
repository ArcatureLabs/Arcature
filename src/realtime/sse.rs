//! SSE endpoint wrapper over `axum::response::sse`.

use std::convert::Infallible;
use std::time::Duration;

use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream::{self, StreamExt};

use super::channel::{Broadcast, ChannelError};
use super::error::{admission_status, RealtimeError};
use super::origin::OriginPolicy;
use super::registry::Registry;
use super::shutdown::ShutdownConfig;

/// SSE limits (retry interval, keep-alive interval).
#[derive(Debug, Clone, Copy)]
pub struct SseLimits {
    /// The retry interval (milliseconds) sent to the client.
    pub retry_ms: u64,
    /// The keep-alive comment interval.
    pub keep_alive_interval: Duration,
}

impl SseLimits {
    /// Conservative defaults: 3s retry, 15s keep-alive.
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            retry_ms: 3000,
            keep_alive_interval: Duration::from_secs(15),
        }
    }
}

/// An SSE endpoint. Cheap to clone.
#[derive(Clone)]
pub struct SseEndpoint {
    broadcast: Broadcast,
    origin: OriginPolicy,
    registry: Registry,
    limits: SseLimits,
    shutdown: ShutdownConfig,
}

impl std::fmt::Debug for SseEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SseEndpoint")
            .field("limits", &self.limits)
            .field("max_connections", &self.shutdown.max_connections())
            .finish_non_exhaustive()
    }
}

impl SseEndpoint {
    /// Create an SSE endpoint. Channel authorization is expected upstream via
    /// an Axum layer (SSE is a plain GET); only origin + connection limit are
    /// checked here.
    #[must_use]
    pub fn new(
        broadcast: Broadcast,
        origin: OriginPolicy,
        registry: Registry,
        limits: SseLimits,
        shutdown: ShutdownConfig,
    ) -> Self {
        Self {
            broadcast,
            origin,
            registry,
            limits,
            shutdown,
        }
    }

    /// Handle an SSE request. Admission order: origin, connection limit.
    pub async fn handle(self, headers: HeaderMap, _channel_id: String) -> Response {
        let origin_header = headers.get("origin").cloned();
        let origin_decision = self.origin.authorize(origin_header.as_ref());
        if matches!(origin_decision, super::origin::OriginDecision::Denied) {
            return admission_status(&RealtimeError::Origin).into_response();
        }

        let guard = match self.registry.acquire(self.shutdown.max_connections()) {
            Ok(g) => g,
            Err(e) => return admission_status(&e).into_response(),
        };

        let limits = self.limits;
        let mut sub = self.broadcast.subscribe();
        let shutdown = self.shutdown.clone();
        let _ = guard; // held for the lifetime of the stream

        let retry = Duration::from_millis(limits.retry_ms);
        let keep_alive = KeepAlive::new()
            .interval(limits.keep_alive_interval)
            .text("keep-alive");

        // Build the event stream. First event is a `retry:` preamble;
        // subsequent events are broadcast payloads.
        let preamble = stream::once(async move {
            Ok::<_, Infallible>(Event::default().retry(retry))
        });
        let events = stream::unfold(
            (sub, shutdown),
            move |(mut sub, shutdown)| async move {
                if shutdown.is_draining() {
                    return None;
                }
                match sub.recv().await {
                    Ok(payload) => {
                        let data = std::str::from_utf8(payload.as_bytes())
                            .unwrap_or("")
                            .to_string();
                        let event = Event::default().data(data);
                        Some((Ok(event), (sub, shutdown)))
                    }
                    Err(ChannelError::Closed) => None,
                    Err(ChannelError::Lagged) => {
                        let event = Event::default().comment("lagged");
                        Some((Ok(event), (sub, shutdown)))
                    }
                    Err(ChannelError::Full) => {
                        let event = Event::default().comment("full");
                        Some((Ok(event), (sub, shutdown)))
                    }
                }
            },
        );

        let stream = preamble.chain(events);
        Sse::new(stream).keep_alive(keep_alive).into_response()
    }
}
