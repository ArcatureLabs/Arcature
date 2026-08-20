//! The request pipeline: the order in which layers wrap the router.
//!
//! # The order is a contract
//!
//! An application's behaviour depends on layer order as much as on which
//! layers are present, so the order is fixed here rather than being whatever
//! order the builder methods happened to be called in. `.inertia()` before
//! `.csrf()` and `.csrf()` before `.inertia()` produce the same pipeline.
//!
//! Outermost first — a request travels down this list and a response travels
//! back up it:
//!
//! | # | Stage | Why here |
//! |---|---|---|
//! | 1 | `DevProxy` | Vite requests must never reach application routing. Outermost so it also catches the HMR WebSocket upgrade. Only with the `dev-proxy` feature, and a pass-through unless `arc dev` set an IPC endpoint. |
//! | 2 | `Proxy` | Pre-routing: it rewrites the URI, so it has to run before route selection or the rewrite would miss. |
//! | 3 | `Compression` | Outermost of the response-shaping layers, so it sees the final body — including one a layer below produced instead of a handler. |
//! | 4 | `SecurityHeaders` | Outside the body limit and the timeout **on purpose**: a `413` and a `408` are responses a browser renders too, and they need `nosniff` and a framing policy as much as a page does. |
//! | 5 | `CORS` | Answers a preflight without waking anything below. Inside `SecurityHeaders` so the preflight response still carries them. |
//! | 6 | `RequestId` | Before the access log, which reads the id out of extensions — and before everything that can produce a response, so every response carries `x-request-id`. |
//! | 7 | `AccessLog` | Directly inside `RequestId`. Outside the panic catcher, the body limit and the timeout, so a `500`, a `413` and a `408` are all logged rather than vanishing. |
//! | 8 | `CatchPanic` | Turns a panic below into a `500` instead of a dropped connection. Inside the access log so the `500` is recorded; outside everything that runs application code. |
//! | 9 | `BodyLimit` | Before anything that reads a body, so an oversized upload is rejected without being buffered. |
//! | 10 | `Timeout` | Bounds everything inside it. Outside the router so a slow handler cannot hold a connection open. |
//! | 11 | `Session` | Must load the session before CSRF (token lookup) and before handlers extract it. |
//! | 12 | `CSRF` | After the session, before the handler: an unsafe request is rejected before it can act. |
//! | 13 | `Inertia` | Inserts `InertiaConfig` and `InertiaRequest` into extensions; the `Inertia` extractor fails without it. Innermost of the framework layers so a rejection from CSRF or a timeout is *not* dressed up as an Inertia response. |
//! | 14 | user `.layer()`s | Applied in call order, wrapping the router directly. Innermost by design: a user layer sees a request that has already been limited, timed, and authenticated. |
//! | 15 | Router | Route matching and the handler. |
//! | 16 | `StaticFiles` | The router's *fallback*, so it only sees requests no route matched. Inside every layer above, which is what gives a served file the same compression, headers and access log as a handler response. |
//!
//! Stages 1 and 2 wrap the router *as a service* (they are `tower::Layer`s
//! over the whole `axum::Router`, applied after `with_state`); stages 3-14
//! wrap it as a `Router`. That split is why this module has two functions
//! rather than one.
//!
//! Every stage from 3 down is **off unless asked for**. An application that
//! calls nothing but `.routes()` gets a bare router, which is what makes the
//! order above readable: each entry is a decision someone made.
//!
//! # Where the layers come from
//!
//! [`RouterLayer`] type-erases each layer to `Fn(Router<S>) -> Router<S>`,
//! which is what lets `InertiaLayer`, `SessionManagerLayer`, `CsrfLayer` and a
//! user's own `tower::Layer` — none of which share a type — sit in one ordered
//! struct.

use crate::routing::{RouterLayer, RouterState};
use axum::Router;

/// The layers an [`ApplicationBuilder`](super::ApplicationBuilder) has
/// collected, held in slots rather than in call order so that
/// [`Pipeline::apply`] can impose the documented order.
pub(crate) struct Pipeline<S: RouterState> {
    /// Response compression, enabled by `.compression()`.
    pub compression: bool,
    /// Response security headers, set by `.security_headers(..)`.
    pub security_headers: Option<crate::http::SecurityHeaders>,
    /// The CORS layer, built by `.cors(..)`.
    pub cors: Option<RouterLayer<S>>,
    /// Request-id generation and echo, enabled by `.request_id()`.
    #[cfg(feature = "observe")]
    pub request_id: bool,
    /// One access-log line per request, enabled by `.access_log()`.
    #[cfg(feature = "observe")]
    pub access_log: bool,
    /// Turn a panic into a `500`, enabled by `.catch_panic()`.
    pub catch_panic: bool,
    /// Maximum request body size in bytes. `None` leaves the body unbounded.
    pub body_limit: Option<usize>,
    /// Whole-request timeout. `None` leaves requests unbounded.
    pub timeout: Option<std::time::Duration>,
    /// The session layer, built by `.session(config, store)`.
    pub session: Option<RouterLayer<S>>,
    /// The CSRF layer, built by `.csrf(config)`.
    pub csrf: Option<RouterLayer<S>>,
    /// The Inertia layer, built by `.inertia(config)`.
    pub inertia: Option<RouterLayer<S>>,
    /// User layers, in the order `.layer()` was called.
    pub user: Vec<RouterLayer<S>>,
    /// The document-root file server, installed as the router's fallback.
    /// `None` leaves whatever fallback the routes defined.
    pub static_files: Option<crate::assets::StaticFiles>,
}

impl<S: RouterState> Pipeline<S> {
    /// An empty pipeline: no framework layers, no user layers.
    pub fn new() -> Self {
        Pipeline {
            compression: false,
            security_headers: None,
            cors: None,
            #[cfg(feature = "observe")]
            request_id: false,
            #[cfg(feature = "observe")]
            access_log: false,
            catch_panic: false,
            body_limit: None,
            timeout: None,
            session: None,
            csrf: None,
            inertia: None,
            user: Vec::new(),
            static_files: None,
        }
    }

    /// Wrap `router` in the router-level stages (3 through 14 in the table
    /// above), in the documented order.
    ///
    /// `Router::layer` wraps everything already on the router, so the *last*
    /// layer applied ends up outermost. The stages are therefore applied
    /// inside-out: user layers first, the body limit last.
    pub fn apply(self, router: Router<S>) -> Router<S> {
        // 16 — the document root, as the router's fallback. Set first so
        // every layer below wraps it too: a file served from `public/` gets
        // the same treatment as a handler response.
        let router = match self.static_files {
            Some(service) => router.fallback_service(service),
            None => router,
        };

        // 14 — user layers. Applied in reverse of call order so that the first
        // `.layer()` call ends up outermost among them, matching the reading
        // order of the builder chain.
        let router = self
            .user
            .into_iter()
            .rev()
            .fold(router, |router, layer| layer.apply(router));

        // 13 — Inertia.
        let router = match self.inertia {
            Some(layer) => layer.apply(router),
            None => router,
        };

        // 12 — CSRF.
        let router = match self.csrf {
            Some(layer) => layer.apply(router),
            None => router,
        };

        // 11 — session.
        let router = match self.session {
            Some(layer) => layer.apply(router),
            None => router,
        };

        // 10 — timeout.
        let router = match self.timeout {
            Some(duration) => router.layer(tower_http::timeout::TimeoutLayer::with_status_code(
                axum::http::StatusCode::REQUEST_TIMEOUT,
                duration,
            )),
            None => router,
        };

        // 9 — body limit.
        let router = match self.body_limit {
            Some(bytes) => router.layer(tower_http::limit::RequestBodyLimitLayer::new(bytes)),
            None => router,
        };

        // 8 — panic catcher. The default responder is replaced so the body is
        // a `Problem`, not the panic message: a panic payload routinely
        // carries a file path, a SQL fragment, or a value that was never meant
        // to leave the process.
        let router = if self.catch_panic {
            router.layer(tower_http::catch_panic::CatchPanicLayer::custom(
                panic_response,
            ))
        } else {
            router
        };

        // 7 — access log.
        #[cfg(feature = "observe")]
        let router = if self.access_log {
            router.layer(crate::observe::AccessLogLayer)
        } else {
            router
        };

        // 6 — request id.
        #[cfg(feature = "observe")]
        let router = if self.request_id {
            router.layer(crate::observe::RequestIdLayer)
        } else {
            router
        };

        // 5 — CORS.
        let router = match self.cors {
            Some(layer) => layer.apply(router),
            None => router,
        };

        // 4 — security headers.
        let router = match self.security_headers {
            Some(headers) => router.layer(headers),
            None => router,
        };

        // 3 — compression.
        if self.compression {
            router.layer(tower_http::compression::CompressionLayer::new())
        } else {
            router
        }
    }
}

/// Turn a caught panic into an RFC 9457 `Problem`.
///
/// The payload is deliberately discarded rather than reported: a panic message
/// is written for a developer reading a backtrace, and routinely contains a
/// file path, a SQL fragment, or the value that caused the panic. The details
/// still reach the operator -- `tower-http` logs the panic and its backtrace --
/// they just do not reach the client.
fn panic_response(_payload: Box<dyn std::any::Any + Send + 'static>) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    crate::api::Problem::of(crate::api::ProblemKind::Internal).into_response()
}

impl<S: RouterState> Default for Pipeline<S> {
    fn default() -> Self {
        Self::new()
    }
}

/// Wrap a stateless router in the service-level stages (1 and 2 in the table
/// above) and return the service to serve.
///
/// Both stages are zero-overhead pass-throughs when unconfigured: `ProxyLayer`
/// with `None` forwards, and `DevProxyLayer` with no endpoint forwards. This
/// is shared by [`Application::serve`](super::Application::serve) and
/// [`Application::run_with_state`](super::Application::run_with_state) so the
/// two entry points cannot drift apart.
#[cfg(feature = "macros")]
pub(crate) fn compose_service(
    router: Router<()>,
    proxy: Option<crate::proxy::ProxyFn>,
    #[cfg(feature = "dev-proxy")] dev_proxy: Option<crate::dev_proxy::endpoint::IpcEndpoint>,
) -> impl tower::Service<
    axum::extract::Request,
    Response = axum::response::Response,
    Error = std::convert::Infallible,
    Future: Send,
> + Clone
+ Send
+ 'static {
    use tower::Layer as _;

    // 2 — the pre-routing proxy, immediately outside the router.
    let service = crate::proxy::ProxyLayer::new(proxy).layer(router.into_service());

    // 1 — the dev proxy, outermost.
    #[cfg(feature = "dev-proxy")]
    let service = crate::dev_proxy::DevProxyLayer::new(dev_proxy).layer(service);

    service
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_pipeline_leaves_the_router_alone() {
        let router: Router<()> = Router::new().route("/", axum::routing::get(|| async { "ok" }));
        let _ = Pipeline::new().apply(router);
    }
}
