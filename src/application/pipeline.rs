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
//! | 3 | `Health` | Merged *beside* the application router rather than layered over it, so `/up/live` and `/up/ready` answer from the lifecycle alone — no session load, no maintenance `503`, no access-log line, on a request an orchestrator makes every few seconds. See [`crate::application::health`]. |
//! | 4 | `UagEndpoint` | Merged beside the router for the same reason health is, one stage further in: `/_arcature/uag.json` describes the process, so it must not be shaped by a session, a maintenance `503` or a rate limit. Only with the `uag` feature, only after an explicit `.uag_endpoint(..)`, and only in a debug build. See [`crate::application::uag_endpoint`]. |
//! | 5 | `Compression` | Outermost of the response-shaping layers, so it sees the final body — including one a layer below produced instead of a handler. |
//! | 6 | `SecurityHeaders` | Outside the body limit and the timeout **on purpose**: a `413` and a `408` are responses a browser renders too, and they need `nosniff` and a framing policy as much as a page does. |
//! | 7 | `CORS` | Answers a preflight without waking anything below. Inside `SecurityHeaders` so the preflight response still carries them. |
//! | 8 | `RequestId`, `TraceContext` | Before the access log, which reads the id out of extensions — and before everything that can produce a response, so every response carries `x-request-id`. |
//! | 9 | `AccessLog`, `Metrics` | Directly inside `RequestId`. Outside the panic catcher, the body limit, the timeout and the rate limiter, so a `500`, a `413`, a `408` and a `429` are all logged and counted rather than vanishing. `Metrics` shares the stage because it must see the same requests; installed as a user layer at 21 it would see none of the refusals. |
//! | 10 | `CatchPanic` | Turns a panic below into a `500` instead of a dropped connection. Inside the access log so the `500` is recorded; outside everything that runs application code. |
//! | 11 | `ErrorMapping` | Gives an RFC 9457 body to every bodiless error produced below it — the bare `404`, `405`, `408` and `413` that axum and `tower-http` emit — and redacts `text/plain` 5xx bodies in release. Inside the panic catcher, which already produces a `Problem`. |
//! | 12 | `BodyLimit` | Before anything that reads a body, so an oversized upload is rejected without being buffered. |
//! | 13 | `Timeout` | Bounds everything inside it. Outside the router so a slow handler cannot hold a connection open. |
//! | 14 | `Maintenance` | Outside the session and CSRF: a maintenance `503` must not depend on a session store that may be part of what is being maintained, and a form POST arriving during the window must get the `503` rather than a CSRF `419`. |
//! | 15 | `RateLimit` | Inside maintenance, so a request answered by a maintenance `503` costs no quota, and outside the session, so a refused request never touches the session store. After the health merge -- a throttled health probe is a self-inflicted outage. |
//! | 16 | `Session` | Must load the session before CSRF (token lookup) and before handlers extract it. |
//! | 17 | `CSRF` | After the session, before the handler: an unsafe request is rejected before it can act. |
//! | 18 | `Inertia` | Inserts `InertiaConfig` and `InertiaRequest` into extensions; the `Inertia` extractor fails without it. Innermost of the framework layers so a rejection from CSRF or a timeout is *not* dressed up as an Inertia response. |
//! | 19 | `PageContracts` | An extension carrying the [`ContractArtifact`](crate::inertia::contracts::ContractArtifact), for the dev-only UAG endpoint and `arc typegen` to read. Data, not behaviour, so its position only has to be somewhere a handler can see it. |
//! | 20 | `RedirectMapper` | Finishes a [`RedirectResponse`](crate::http::response::RedirectResponse): resolves `redirect().route(..)` against the route table and writes the flash data through the session. Inside `Inertia` because Inertia's 303-for-`PUT`/`PATCH`/`DELETE` rule has to see the *finished* redirect, not the placeholder `into_response` produced; inside `Session` because that is where the flash goes; outside the router because the builder is only readable once the handler has returned. Installed by default -- an application that never redirects by name pays one extension lookup per response. See [`crate::routing::redirect_mapper`]. |
//! | 21 | user `.layer()`s | Applied in call order, wrapping the router directly. Innermost by design: a user layer sees a request that has already been limited, timed, and authenticated. |
//! | 22 | Router | Route matching and the handler. |
//! | 23 | `StaticFiles` | The router's *fallback*, so it only sees requests no route matched. Inside every layer above, which is what gives a served file the same compression, headers and access log as a handler response. |
//!
//! Stages 1 and 2 wrap the router *as a service* (they are `tower::Layer`s
//! over the whole `axum::Router`, applied after `with_state`); stages 5-21
//! wrap it as a `Router`. That split is why this module has two functions
//! rather than one. Stages 3 and 4 are neither: they are `merge`s, which is
//! exactly what makes them exempt from the layers below them.
//!
//! Every stage from 5 down is **off unless asked for**, with one exception:
//! stage 20 is on unless refused, because `redirect().route(..)` reading as
//! broken in a default build is not a decision anyone would make on purpose.
//! An application that calls nothing but `.routes()` gets a bare router, the
//! health endpoints and the redirect mapper, which is what makes the order
//! above readable: every other entry is a decision someone made.
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
    /// The health endpoints, merged outside every router-level stage.
    /// `None` only when the application called `.health(false)`.
    pub health: Option<crate::application::health::Health>,
    /// The dev-only application-graph endpoint, merged just inside health.
    /// `Some` only after an explicit `.uag_endpoint(..)` in a debug build.
    #[cfg(feature = "uag")]
    pub uag: Option<crate::application::uag_endpoint::UagEndpoint>,
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
    /// Resolve the W3C trace context, enabled by `.trace_context()`.
    ///
    /// Stage 8 beside the request id: both must be outside the access log at
    /// 9 for the access line to carry their ids, and outside the admission
    /// stages so a refused request still joins its trace.
    #[cfg(feature = "observe")]
    pub trace_context: bool,
    /// A metrics registry to record into, enabled by `.metrics()`.
    ///
    /// Stage 9 beside the access log rather than a stage of its own, because
    /// the two see exactly the same set of requests and a reader deciding
    /// where a number came from is served by that being one answer.
    #[cfg(feature = "observe")]
    pub metrics: Option<crate::observe::Metrics>,
    /// Turn a panic into a `500`, enabled by `.catch_panic()`.
    pub catch_panic: bool,
    /// Error-response mapping, set by `.error_mapping(..)`.
    pub error_mapping: Option<crate::http::ErrorMapping>,
    /// Maximum request body size in bytes. `None` leaves the body unbounded.
    pub body_limit: Option<usize>,
    /// Whole-request timeout. `None` leaves requests unbounded.
    pub timeout: Option<std::time::Duration>,
    /// The maintenance switch, set by `.maintenance(..)`.
    pub maintenance: Option<crate::http::Maintenance>,
    /// The application-wide rate limit, set by `.rate_limit(..)`.
    pub rate_limit: Option<crate::routing::RateLimit>,
    /// The session layer, built by `.session(config, store)`.
    pub session: Option<RouterLayer<S>>,
    /// The CSRF layer, built by `.csrf(config)`.
    pub csrf: Option<RouterLayer<S>>,
    /// The Inertia layer, built by `.inertia(config)`.
    pub inertia: Option<RouterLayer<S>>,
    /// The page-contract artifact, set by `.page_contracts(..)`.
    #[cfg(feature = "inertia")]
    pub page_contracts: Option<std::sync::Arc<crate::inertia::contracts::ContractArtifact>>,
    /// The named-route redirect resolver, built by `build()` from the route
    /// table. `None` only when the application called `.redirect_mapper(false)`.
    pub redirect_mapper: Option<crate::routing::RedirectMapper>,
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
            health: None,
            #[cfg(feature = "uag")]
            uag: None,
            compression: false,
            security_headers: None,
            cors: None,
            #[cfg(feature = "observe")]
            request_id: false,
            #[cfg(feature = "observe")]
            access_log: false,
            #[cfg(feature = "observe")]
            trace_context: false,
            #[cfg(feature = "observe")]
            metrics: None,
            catch_panic: false,
            error_mapping: None,
            body_limit: None,
            timeout: None,
            maintenance: None,
            rate_limit: None,
            session: None,
            csrf: None,
            inertia: None,
            #[cfg(feature = "inertia")]
            page_contracts: None,
            redirect_mapper: None,
            user: Vec::new(),
            static_files: None,
        }
    }

    /// Wrap `router` in the router-level stages (5 through 21 in the table
    /// above) and merge the two exempt routers -- the UAG endpoint (stage 4)
    /// and the health endpoints (stage 3) -- beside the result.
    ///
    /// `Router::layer` wraps everything already on the router, so the *last*
    /// layer applied ends up outermost. The stages are therefore applied
    /// inside-out: user layers first, compression last.
    pub fn apply(self, router: Router<S>) -> Router<S> {
        // 23 — the document root, as the router's fallback. Set first so
        // every layer below wraps it too: a file served from `public/` gets
        // the same treatment as a handler response.
        let router = match self.static_files {
            Some(service) => router.fallback_service(service),
            None => router,
        };

        // 21 — user layers. Applied in reverse of call order so that the first
        // `.layer()` call ends up outermost among them, matching the reading
        // order of the builder chain.
        let router = self
            .user
            .into_iter()
            .rev()
            .fold(router, |router, layer| layer.apply(router));

        // 20 — the redirect mapper. Outside the user layers so that a
        // redirect a user layer produced is finished too, and inside Inertia
        // so Inertia sees a real `Location` and a real status when it decides
        // whether a redirect after a `PUT` has to become a `303`.
        let router = match self.redirect_mapper {
            Some(mapper) => router.layer(mapper),
            None => router,
        };

        // 19 — the page-contract artifact, as a request extension.
        #[cfg(feature = "inertia")]
        let router = match self.page_contracts {
            Some(artifact) => router.layer(axum::Extension(artifact)),
            None => router,
        };

        // 18 — Inertia.
        let router = match self.inertia {
            Some(layer) => layer.apply(router),
            None => router,
        };

        // 17 — CSRF.
        let router = match self.csrf {
            Some(layer) => layer.apply(router),
            None => router,
        };

        // 16 — session.
        let router = match self.session {
            Some(layer) => layer.apply(router),
            None => router,
        };

        // 15 — the rate limit.
        let router = match self.rate_limit {
            Some(limit) => router.layer(limit),
            None => router,
        };

        // 14 — maintenance.
        let router = match self.maintenance {
            Some(maintenance) => router.layer(maintenance),
            None => router,
        };

        // 13 — timeout.
        let router = match self.timeout {
            Some(duration) => router.layer(tower_http::timeout::TimeoutLayer::with_status_code(
                axum::http::StatusCode::REQUEST_TIMEOUT,
                duration,
            )),
            None => router,
        };

        // 12 — body limit.
        let router = match self.body_limit {
            Some(bytes) => router.layer(tower_http::limit::RequestBodyLimitLayer::new(bytes)),
            None => router,
        };

        // 11 — error mapping.
        let router = match self.error_mapping {
            Some(mapping) => router.layer(mapping),
            None => router,
        };

        // 10 — panic catcher. The default responder is replaced so the body is
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

        // 9 — metrics, then the access log outside it. Both sit here rather
        // than among the user layers at 21, and that placement is the whole
        // point: 21 is inside the body limit, the timeout, maintenance and
        // the rate limiter, so a counter installed there never sees a request
        // refused with a 413, a 408, a 503 or a 429 -- exactly the traffic an
        // incident is about. From 9 it sees them, and it agrees with the
        // access line beside it.
        #[cfg(feature = "observe")]
        let router = match self.metrics {
            Some(metrics) => router.layer(crate::observe::MetricsLayer::new(metrics)),
            None => router,
        };

        #[cfg(feature = "observe")]
        let router = if self.access_log {
            router.layer(crate::observe::AccessLogLayer)
        } else {
            router
        };

        // 8 — trace context, then the request id outside it. Both sit here
        // rather than among the user layers at 21 for the same reason the
        // metrics layer does: 21 is inside the admission stages, so a request
        // refused with a 413, a 408, a 503 or a 429 would carry no trace at
        // all. From 8 the context is resolved before anything can refuse, and
        // the access line at 9 can carry the ids.
        #[cfg(feature = "observe")]
        let router = if self.trace_context {
            router.layer(crate::observe::TraceContextLayer)
        } else {
            router
        };

        #[cfg(feature = "observe")]
        let router = if self.request_id {
            router.layer(crate::observe::RequestIdLayer)
        } else {
            router
        };

        // 7 — CORS.
        let router = match self.cors {
            Some(layer) => layer.apply(router),
            None => router,
        };

        // 6 — security headers.
        let router = match self.security_headers {
            Some(headers) => router.layer(headers),
            None => router,
        };

        // 5 — compression.
        let router = if self.compression {
            router.layer(tower_http::compression::CompressionLayer::new())
        } else {
            router
        };

        // 4 — the application-graph endpoint, merged rather than layered for
        // the same reason health is: it describes the process, so a session
        // load, a maintenance `503` or a rate limit would all be answering a
        // different question than the one asked. On the left of the merge,
        // like health, so the application router's fallback is the one that
        // survives.
        #[cfg(feature = "uag")]
        let router = match self.uag {
            Some(uag) => uag.router::<S>().merge(router),
            None => router,
        };

        // 3 — the health endpoints, merged rather than layered. This is the
        // whole point: an orchestrator's probe must not depend on a session
        // store, must not be turned into a maintenance `503`, and must not
        // write an access-log line every two seconds.
        match self.health {
            // Health on the *left*: `Router::merge` resolves two default
            // fallbacks by taking the right-hand one, and the application
            // router's default fallback is the one the stages above have
            // been layered onto. Merged the other way round, every bodiless
            // `404` would escape `ErrorMapping`, the access log and the
            // security headers.
            Some(health) => health.router::<S>().merge(router),
            None => router,
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

    /// The pipeline that a plain `.routes(..).build()` produces has to finish
    /// a named-route redirect, because stage 20 is on unless refused.
    #[tokio::test]
    async fn a_default_application_resolves_a_named_route_redirect() {
        use tower::ServiceExt as _;

        let routes: crate::routing::Routes = crate::routing::Routes::new([
            crate::routing::Route::get("/users/{id}", || async { "user" }).name("users.show"),
            crate::routing::Route::get("/go", || async {
                crate::http::response::redirect().route("users.show", 7u64)
            }),
        ]);
        let router = crate::application::Application::new()
            .routes(routes)
            .build()
            .into_router();

        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/go")
                    .body(axum::body::Body::empty())
                    .expect("a GET with an empty body is a valid request"),
            )
            .await
            .expect("the router is infallible");

        assert_eq!(
            response
                .headers()
                .get(axum::http::header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("/users/7"),
            "the mapper should have turned the name into a path"
        );
    }

    /// The escape hatch has to actually remove the layer, or an application
    /// that installs its own mapper ends up running two.
    #[tokio::test]
    async fn refusing_the_mapper_leaves_the_documented_unmapped_failure() {
        use tower::ServiceExt as _;

        let routes: crate::routing::Routes = crate::routing::Routes::new([
            crate::routing::Route::get("/users/{id}", || async { "user" }).name("users.show"),
            crate::routing::Route::get("/go", || async {
                crate::http::response::redirect().route("users.show", 7u64)
            }),
        ]);
        let router = crate::application::Application::new()
            .routes(routes)
            .redirect_mapper(false)
            .build()
            .into_router();

        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/go")
                    .body(axum::body::Body::empty())
                    .expect("a GET with an empty body is a valid request"),
            )
            .await
            .expect("the router is infallible");

        assert_eq!(
            response.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "without the mapper a named route is the fallback `400`"
        );
    }

    /// Routes merged after the first call have to be in the snapshot too,
    /// which is why the mapper is built in `build()` and not in `.routes()`.
    #[tokio::test]
    async fn a_name_declared_by_merge_routes_is_still_resolvable() {
        use tower::ServiceExt as _;

        let first: crate::routing::Routes =
            crate::routing::Routes::new([crate::routing::Route::get("/go", || async {
                crate::http::response::redirect().route("users.show", 7u64)
            })]);
        let second: crate::routing::Routes =
            crate::routing::Routes::new([crate::routing::Route::get("/users/{id}", || async {
                "user"
            })
            .name("users.show")]);
        let router = crate::application::Application::new()
            .routes(first)
            .merge_routes(second)
            .build()
            .into_router();

        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/go")
                    .body(axum::body::Body::empty())
                    .expect("a GET with an empty body is a valid request"),
            )
            .await
            .expect("the router is infallible");

        assert_eq!(
            response
                .headers()
                .get(axum::http::header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("/users/7")
        );
    }
}
