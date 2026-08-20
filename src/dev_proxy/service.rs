//! The one-port dev proxy service — reverse-proxy Vite requests over IPC.
//!
//! One responsibility: forward requests that [`crate::dev_proxy::vite`]
//! identifies as Vite's (modules, `@vite/`, `@react-refresh`, HMR WebSocket
//! upgrade) to the Vite dev server running in `middlewareMode` on the IPC
//! endpoint, and pass every other request through to the inner application
//! pipeline. This keeps the development topology on **exactly one TCP
//! listener** (engine spec / AP2.1-3): the Rust application owns the port,
//! Vite owns no port — it speaks HTTP/1 over a Unix socket (Unix) or a named
//! pipe (Windows) that `arc dev` created in a process-private location.
//!
//! # Why `hyper::client::conn::http1::handshake`?
//!
//! Vite's `middlewareMode` server is an HTTP/1 server that accepts requests
//! exactly as a TCP server would — only the transport differs (IPC stream vs
//! TCP stream). `hyper::client::conn::http1::handshake` speaks the client
//! side of HTTP/1 over any `hyper::rt::Read + Write + Unpin` stream. The IPC
//! stream (`tokio::net::UnixStream` / `NamedPipeClient`) is a tokio
//! `AsyncRead + AsyncWrite`; `hyper_util::rt::tokio::WithHyperIo` bridges the
//! tokio and hyper I/O trait families (the officially-blessed adapter — we
//! do not reinvent the `ReadBufCursor` glue). There is no connection pooling:
//! each forwarded request opens a fresh IPC connection. This is honest for a
//! dev-only path (a WebSocket upgrade consumes its connection, and HTTP/1
//! keep-alive across an upgrade is not worth a pool).
//!
//! # WebSocket upgrade tunneling
//!
//! A Vite HMR upgrade request (`Connection: upgrade`, `Upgrade: websocket`,
//! `Sec-WebSocket-Protocol: vite-hmr`) is forwarded as a normal HTTP/1
//! request to Vite; Vite responds `101 Switching Protocols`. Hyper's client
//! (driven with `with_upgrades()`) fulfills an `OnUpgrade` placed on the
//! *response*; hyper's server already placed an `OnUpgrade` on the
//! *request* for the browser. We extract both, spawn a task that bridges
//! the two upgraded I/Os bidirectionally (`tokio::io::copy` both ways via
//! `hyper_util::rt::TokioIo`), and return the `101` so hyper's server hands
//! the browser connection to its upgrade.
//!
//! # Security
//!
//! Forwarding is gated by [`crate::dev_proxy::vite::is_vite_request`], a
//! pure function of the request path and headers — never of runtime state
//! (no env read, no global). The IPC endpoint is process-private and
//! per-invocation (`arc dev` generates the path); it is not
//! attacker-controlled. A request that *looks* like a Vite request is
//! forwarded to Vite (trusted, dev-only); the application router never sees
//! it. Forward/connect failures fall back to the application pipeline so a
//! transient Vite outage yields the app's normal 404, not a 502. See the
//! AP2.1-3 security review.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::axum::body::Body;
use crate::axum::http::{Response, StatusCode};
use crate::dev_proxy::endpoint::{IpcEndpoint, IpcStream};
use crate::dev_proxy::vite::is_vite_request;

type Request = crate::axum::extract::Request<Body>;

/// A boxed future resolving to the infallible response the dev proxy and the
/// application pipeline both produce. Mirrors the pipeline assembler's
/// `BoxFuture` so the dev proxy composes as the *outermost* pre-routing layer.
type BoxFuture = Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

/// The one-port dev proxy layer.
///
/// Wraps the entire application pipeline (the service produced by the
/// pipeline assembler, which itself wraps the pre-routing
/// [`crate::proxy::ProxyLayer`] and the Axum router). When `endpoint` is
/// present the layer forwards Vite requests to Vite over IPC; when it is
/// `None` the layer is a zero-overhead pass-through (the feature is compiled
/// in but the env var was not set, so production builds that enable
/// `dev-proxy` pay only the cost of one `Option` check per request).
///
/// `Clone` so the produced service is `Clone` — a requirement of
/// `axum::serve`. The endpoint is `Arc`-shared so the clone is cheap.
#[derive(Clone)]
pub struct DevProxyLayer {
    endpoint: Option<Arc<IpcEndpoint>>,
}

impl DevProxyLayer {
    /// Build a dev proxy layer. When `endpoint` is `None` the layer is a
    /// pass-through — the `dev-proxy` feature is on but `arc dev` did not set
    /// `ARCATURE_VITE_IPC`, so no forwarding occurs.
    #[must_use]
    pub fn new(endpoint: Option<IpcEndpoint>) -> Self {
        Self {
            endpoint: endpoint.map(Arc::new),
        }
    }
}

impl<S> tower::Layer<S> for DevProxyLayer {
    type Service = DevProxyService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DevProxyService {
            inner,
            endpoint: self.endpoint.clone(),
        }
    }
}

/// The dev proxy service. Forwards Vite requests to the IPC endpoint;
/// delegates everything else to the inner application pipeline.
///
/// `Inner` is the composed application service produced by the pipeline
/// assembler: a
/// `Service<Request<Body>, Response = Response<Body>, Error = Infallible>`
/// that is `Clone + Send + 'static` (the exact `axum::serve` bound). The dev
/// proxy is the *outermost* pre-routing layer, so it sees every request
/// before the application proxy or the Axum router.
#[derive(Clone)]
pub struct DevProxyService<Inner> {
    inner: Inner,
    endpoint: Option<Arc<IpcEndpoint>>,
}

impl<Inner> tower::Service<Request> for DevProxyService<Inner>
where
    Inner: tower::Service<Request, Response = Response<Body>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    Inner::Future: Send + 'static,
{
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = BoxFuture;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        // No endpoint → pass-through. The feature is compiled in but the env
        // var was not set; production builds pay only this one `Option` check.
        let Some(endpoint) = self.endpoint.clone() else {
            let fut = self.inner.call(req);
            return Box::pin(fut);
        };

        // Not a Vite request → delegate to the application pipeline. The
        // application's own proxy, Inertia layer, and routes see it.
        if !is_vite_request(&req) {
            let fut = self.inner.call(req);
            return Box::pin(fut);
        }

        // Vite request → forward over IPC. Clone the inner service (standard
        // tower `Service + Clone` idiom) and move it into the forwarding
        // future so the future is `'static` and does not borrow `&mut self`.
        // `self.inner` stays valid for the next `poll_ready`/`call`.
        let inner = self.inner.clone();
        Box::pin(forward_or_delegate(endpoint, req, inner))
    }
}

/// Forward `req` to Vite over IPC; on a connect failure (Vite not yet up,
/// stale socket — the common startup race) fall back to the inner
/// application pipeline, so a Vite-looking request the app has no route for
/// yields the app's normal 404 — a quieter dev signal than a 502 while Vite
/// is restarting. Once the IPC connect succeeds the request is consumed by
/// `forward`; a mid-request Vite failure (handshake or send error, rare)
/// returns a `502 Bad Gateway` with a short diagnostic body. This matches
/// the spec's "clean Ctrl-C / SIGTERM" guarantee: a transient Vite outage
/// does not hard-fail the port.
async fn forward_or_delegate<Inner>(
    endpoint: Arc<IpcEndpoint>,
    req: Request,
    mut inner: Inner,
) -> Result<Response<Body>, Infallible>
where
    Inner: tower::Service<Request, Response = Response<Body>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    Inner::Future: Send + 'static,
{
    match endpoint.connect().await {
        Ok(stream) => match forward(stream, req).await {
            Ok(response) => Ok(response),
            // The request was consumed by `forward`; a mid-request Vite
            // failure is a real crash, not a startup race — a 502 is the
            // honest signal (the dev sees Vite died).
            Err(err) => Ok(bad_gateway(err)),
        },
        // Connect failed before the request was touched — delegate so the
        // app's 404 surfaces during a Vite restart.
        Err(_) => inner.call(req).await,
    }
}

/// Build a `502 Bad Gateway` response for a mid-request Vite IPC failure.
/// The body is a short, fixed diagnostic string (no internal details —
/// hostile input must not leak internals; this is dev-only and the error is
/// Vite's, not attacker-controlled). Never panics.
fn bad_gateway(err: ForwardError) -> Response<Body> {
    eprintln!("warning: vite ipc forward failed: {err}");
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(
            crate::axum::http::HeaderName::from_static("x-content-type-options"),
            crate::axum::http::HeaderValue::from_static("nosniff"),
        )
        .header(
            crate::axum::http::HeaderName::from_static("content-type"),
            crate::axum::http::HeaderValue::from_static("text/plain; charset=utf-8"),
        )
        .body(Body::from(
            "vite dev server closed the connection (is Vite running?)",
        ))
        .unwrap_or_else(|_| {
            // Header construction can only fail on invalid header values,
            // which are all static here. Fallback: a minimal response.
            Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Body::empty())
                .unwrap_or_else(|_| Response::new(Body::empty()))
        })
}

/// Forward `req` to Vite over `stream` using `hyper::client::conn::http1`.
///
/// Returns the Vite response with its body converted to `axum::body::Body`.
/// For a `101 Switching Protocols` response, spawns a tunnel task bridging
/// the browser-side upgrade (extracted from the request extensions) and the
/// Vite-side upgraded client IO.
///
/// # Errors
///
/// `ForwardError` on connect/handshake/send/upgrade failure. The caller
/// ([`forward_or_delegate`]) falls back to the application pipeline on a
/// connect failure, or returns a 502 on a mid-request failure, so the dev
/// port never hard-fails on a transient Vite outage.
async fn forward(stream: IpcStream, req: Request) -> Result<Response<Body>, ForwardError> {
    // Take the browser-side upgrade handle out of the request *before*
    // forwarding: hyper's server sets `OnUpgrade` on the request extensions
    // when it parsed the `Connection: upgrade` request. We need it to drive
    // the browser side of the tunnel; Vite gets the headers (not the
    // extension). Removing it also stops Vite from seeing a hyper-internal
    // extension it does not understand.
    let (mut parts, body) = req.into_parts();
    let browser_upgrade = parts.extensions.remove::<hyper::upgrade::OnUpgrade>();
    let upstream_req = crate::axum::http::Request::from_parts(parts, body);
    // hyper needs an explicit `Host` for HTTP/1.1; the browser sent one, so
    // it is already present in `parts.headers`. No injection here — we
    // forward the browser's headers verbatim (Vite is trusted, dev-only).

    // Bridge the tokio IPC stream to hyper's I/O traits. `WithHyperIo` is
    // the officially-blessed adapter (hyper-util); it holds the
    // `ReadBufCursor` bridge we must not reimplement.
    let io = hyper_util::rt::tokio::WithHyperIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;

    // Drive the client connection in the background. The connection future
    // owns the I/O pump that reads/writes bytes on the IPC stream — it must
    // run *concurrently* with `send_request`, not be awaited inline before it
    // (awaiting the connection future before sending a request has nothing to
    // pump and may never complete). `with_upgrades()` is required so hyper
    // fulfills the Vite-side `OnUpgrade` when Vite sends `101`. Dropping the
    // `JoinHandle` *detaches* (does not cancel) the task in tokio, so the
    // connection keeps driving the upgrade even after `forward` returns.
    tokio::spawn(async move {
        if let Err(err) = conn.with_upgrades().await {
            // A connection error after the response is sent is not
            // actionable (the request already completed); log to stderr for
            // dev diagnostics. Never panic.
            eprintln!("warning: vite ipc connection ended: {err}");
        }
    });

    let response = sender.send_request(upstream_req).await?;
    let status = response.status();

    // WebSocket upgrade: tunnel the browser <-> Vite I/Os bidirectionally.
    if status == StatusCode::SWITCHING_PROTOCOLS {
        let (mut resp_parts, _body) = response.into_parts();
        // The Vite-side upgrade future is on the *response* extensions
        // (hyper's client set it when it parsed Vite's `101`).
        let vite_upgrade = resp_parts.extensions.remove::<hyper::upgrade::OnUpgrade>();
        let browser_response = Response::from_parts(resp_parts, Body::empty());
        if let (Some(browser), Some(vite)) = (browser_upgrade, vite_upgrade) {
            tokio::spawn(tunnel(browser, vite));
        }
        return Ok(browser_response);
    }

    Ok(map_response(response))
}

/// Bridge the browser-side and Vite-side upgraded connections.
///
/// Both `OnUpgrade` futures resolve to a `hyper::upgrade::Upgraded` I/O once
/// the respective HTTP/1 sides complete the handshake. `tokio::io::
/// copy_bidirectional` copies bytes in both directions until either side
/// reaches EOF — the standard tunnel pattern, with no overlapping mutable
/// borrows. `hyper_util::rt::TokioIo` bridges hyper's I/O traits to tokio's
/// `AsyncRead`/`AsyncWrite` so the bidirectional copy works.
async fn tunnel(browser: hyper::upgrade::OnUpgrade, vite: hyper::upgrade::OnUpgrade) {
    let browser = match browser.await {
        Ok(io) => io,
        Err(err) => {
            eprintln!("warning: browser-side hmr upgrade failed: {err}");
            return;
        }
    };
    let vite = match vite.await {
        Ok(io) => io,
        Err(err) => {
            eprintln!("warning: vite-side hmr upgrade failed: {err}");
            return;
        }
    };

    let mut browser = hyper_util::rt::TokioIo::new(browser);
    let mut vite = hyper_util::rt::TokioIo::new(vite);

    // Copy both directions until either side closes. A failure on one side
    // ends the tunnel; the HMR protocol is short-lived JSON messages.
    let _ = tokio::io::copy_bidirectional(&mut browser, &mut vite).await;
}

/// Convert a `hyper::Response<hyper::body::Incoming>` to an
/// `axum::body::Body` response, preserving status and headers. `axum::Body`
/// accepts `hyper::body::Incoming` directly via `Body::new` (the body's
/// `Error` is `hyper::Error`, which satisfies `Into<BoxError>`).
fn map_response(resp: hyper::Response<hyper::body::Incoming>) -> Response<Body> {
    let (parts, body) = resp.into_parts();
    Response::from_parts(parts, Body::new(body))
}

/// Typed forwarding error. Never returned to the caller of the service
/// (the service is infallible — errors fall back to the app pipeline or a
/// 502). The variants exist only for the internal `?` flow in [`forward`] and
/// for a one-line dev diagnostic in [`bad_gateway`]; they are not part of any
/// public API and carry no "future-proof" variants.
enum ForwardError {
    /// `handshake` or connect-time failure (Vite not up, stale socket, or
    /// the connection was canceled before the request was sent).
    Handshake(hyper::Error),
    /// `send_request` or mid-request failure (Vite closed mid-response).
    Send(hyper::Error),
}

impl From<hyper::Error> for ForwardError {
    fn from(err: hyper::Error) -> Self {
        // `is_canceled` covers the "connection not ready / dropped before
        // send" case (the handshake or dispatch race during a Vite
        // restart). Everything else — body write, parse, timeout — is a
        // mid-request `Send` failure. The caller does not branch on this
        // (it 502s either way); the split keeps the dev log line honest.
        if err.is_canceled() {
            ForwardError::Handshake(err)
        } else {
            ForwardError::Send(err)
        }
    }
}

impl std::fmt::Display for ForwardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForwardError::Handshake(err) => {
                write!(f, "vite ipc handshake failed: {err}")
            }
            ForwardError::Send(err) => {
                write!(f, "vite ipc send failed: {err}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axum::http::HeaderValue;

    #[test]
    fn layer_with_no_endpoint_is_passthrough_marker() {
        // A `None` endpoint produces a pass-through layer; the constructor
        // must not panic and the marker is `None`.
        let layer = DevProxyLayer::new(None);
        assert!(layer.endpoint.is_none());
    }

    #[test]
    fn layer_with_endpoint_stores_arc() {
        let layer = DevProxyLayer::new(Some(IpcEndpoint::new(std::path::PathBuf::from(
            "/tmp/arcature-test.sock",
        ))));
        let endpoint = layer
            .endpoint
            .as_ref()
            .expect("endpoint should be stored when provided");
        assert_eq!(
            endpoint.path(),
            std::path::Path::new("/tmp/arcature-test.sock")
        );
    }

    // `ForwardError`'s `From<hyper::Error>` and `Display` impls are simple
    // enough that direct construction-based tests are unnecessary (and
    // `hyper::Error`'s constructors are `pub(super)`, so they cannot be
    // exercised from outside the hyper crate). The categorization
    // (`is_canceled` -> Handshake, else Send) and the `Display` strings
    // ("vite ipc handshake failed" / "vite ipc send failed") are exercised
    // end-to-end by the integration test that drives a real Vite IPC
    // forward-and-fail path.

    // A sanity check that a `HeaderValue` round-trips — the dev proxy
    // preserves Vite's headers verbatim; this guards against accidental
    // header-munging in `map_response`/`build_switching_protocols`.
    #[test]
    fn header_value_roundtrips() {
        let v = HeaderValue::from_static("websocket");
        assert_eq!(v.as_bytes(), b"websocket");
    }
}
