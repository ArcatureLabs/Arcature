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
//! | 3 | `BodyLimit` | Before anything that reads a body, so an oversized upload is rejected without being buffered. |
//! | 4 | `Timeout` | Bounds everything inside it. Outside the router so a slow handler cannot hold a connection open. |
//! | 5 | `Session` | Must load the session before CSRF (token lookup) and before handlers extract it. |
//! | 6 | `CSRF` | After the session, before the handler: an unsafe request is rejected before it can act. |
//! | 7 | `Inertia` | Inserts `InertiaConfig` and `InertiaRequest` into extensions; the `Inertia` extractor fails without it. Innermost of the framework layers so a rejection from CSRF or a timeout is *not* dressed up as an Inertia response. |
//! | 8 | user `.layer()`s | Applied in call order, wrapping the router directly. Innermost by design: a user layer sees a request that has already been limited, timed, and authenticated. |
//! | 9 | Router | Route matching and the handler. |
//!
//! Stages 1 and 2 wrap the router *as a service* (they are `tower::Layer`s
//! over the whole `axum::Router`, applied after `with_state`); stages 3-8 wrap
//! it as a `Router`. That split is why this module has two functions rather
//! than one.
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
}

impl<S: RouterState> Pipeline<S> {
    /// An empty pipeline: no framework layers, no user layers.
    pub fn new() -> Self {
        Pipeline {
            body_limit: None,
            timeout: None,
            session: None,
            csrf: None,
            inertia: None,
            user: Vec::new(),
        }
    }

    /// Wrap `router` in the router-level stages (3 through 8 in the table
    /// above), in the documented order.
    ///
    /// `Router::layer` wraps everything already on the router, so the *last*
    /// layer applied ends up outermost. The stages are therefore applied
    /// inside-out: user layers first, the body limit last.
    pub fn apply(self, router: Router<S>) -> Router<S> {
        // 8 — user layers. Applied in reverse of call order so that the first
        // `.layer()` call ends up outermost among them, matching the reading
        // order of the builder chain.
        let router = self
            .user
            .into_iter()
            .rev()
            .fold(router, |router, layer| layer.apply(router));

        // 7 — Inertia.
        let router = match self.inertia {
            Some(layer) => layer.apply(router),
            None => router,
        };

        // 6 — CSRF.
        let router = match self.csrf {
            Some(layer) => layer.apply(router),
            None => router,
        };

        // 5 — session.
        let router = match self.session {
            Some(layer) => layer.apply(router),
            None => router,
        };

        // 4 — timeout.
        let router = match self.timeout {
            Some(duration) => router.layer(tower_http::timeout::TimeoutLayer::with_status_code(
                axum::http::StatusCode::REQUEST_TIMEOUT,
                duration,
            )),
            None => router,
        };

        // 3 — body limit.
        match self.body_limit {
            Some(bytes) => router.layer(tower_http::limit::RequestBodyLimitLayer::new(bytes)),
            None => router,
        }
    }
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
