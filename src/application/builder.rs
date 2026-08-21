//! The [`Application`] builder and composition root.
//!
//! A normal generated app builds an `Application` in `bootstrap/app.rs`:
//!
//! ```ignore
//! pub fn app() -> Application<AppState> {
//!     Application::new()
//!         .routes(web::routes())
//!         .bind("127.0.0.1")
//!         .port(3000)
//! }
//! ```
//!
//! `main.rs` then calls `app().run_with_state(state_fn).await`, where
//! `state_fn` reads the started subsystem handles from [`Resources`] and
//! returns the application state struct.

use crate::application::lifecycle::Lifecycle;
use crate::application::pipeline::Pipeline;
use crate::application::resources::Resources;
// Both are used only by `run`/`serve`/`run_with_state`, which need the
// certified runtime and so are gated on `macros`.
#[cfg(feature = "macros")]
use crate::application::{EngineError, EngineResult};
use crate::routing::{RouteTable, RouterLayer, RouterState, Routes};
use axum::Router;
#[cfg(feature = "macros")]
use std::net::SocketAddr;
use std::sync::Arc;

/// The default bind address.
pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1";
/// The default port.
pub const DEFAULT_PORT: u16 = 3000;

/// The application composition root. Generic over the state type `S` that
/// extractors see via `axum::extract::State`. The stateless app uses `S = ()`.
pub struct Application<S: RouterState = ()> {
    router: Router<S>,
    bind_addr: String,
    port: u16,
    // Carried so `config()` can hand it back. `port` here is the *configured*
    // port; the one actually bound is `resolve_port(port)`, which may differ.
    app_config: crate::config::AppConfig,
    // The lifecycle handle. Created by the builder rather than at serve time
    // because the health endpoints -- composed into the router by `build()` --
    // have to share the same one. Two `Lifecycle`s would mean `/up/ready`
    // reporting on a state machine nothing ever advances.
    #[cfg_attr(
        not(feature = "macros"),
        expect(dead_code, reason = "consumed only by the `macros`-gated serve path")
    )]
    lifecycle: Lifecycle,
    // The health handle, kept so the serve path can publish the started
    // subsystems into it. `None` when the app called `.health(false)`.
    #[cfg_attr(
        not(feature = "macros"),
        expect(dead_code, reason = "consumed only by the `macros`-gated serve path")
    )]
    health: Option<crate::application::health::Health>,
    // Subsystem configs (feature-gated). Each is `Some` when the app opts in.
    //
    // Every field below -- and `proxy` after them -- is consumed only by the
    // `macros`-gated serve path (`run`/`serve`/`run_with_state`), which is the
    // only code that starts subsystems and composes the service-level stages.
    // An application built without the certified runtime is driven through
    // `into_router`, which stops at the router level and reads none of them.
    // Hence the repeated `expect`: with `macros` off these really are dead,
    // and that is the intended shape rather than an oversight.
    #[cfg(feature = "database")]
    #[cfg_attr(
        not(feature = "macros"),
        expect(dead_code, reason = "consumed only by the `macros`-gated serve path")
    )]
    database_config: Option<crate::database::DatabaseConfig>,
    #[cfg(feature = "jobs")]
    #[cfg_attr(
        not(feature = "macros"),
        expect(dead_code, reason = "consumed only by the `macros`-gated serve path")
    )]
    jobs_registry: Option<crate::jobs::Registry>,
    #[cfg(feature = "jobs")]
    #[cfg_attr(
        not(feature = "macros"),
        expect(dead_code, reason = "consumed only by the `macros`-gated serve path")
    )]
    worker_config: Option<crate::jobs::WorkerConfig>,
    #[cfg(feature = "jobs")]
    #[cfg_attr(
        not(feature = "macros"),
        expect(dead_code, reason = "consumed only by the `macros`-gated serve path")
    )]
    scheduler: Option<crate::jobs::Scheduler>,
    #[cfg(feature = "cache")]
    #[cfg_attr(
        not(feature = "macros"),
        expect(dead_code, reason = "consumed only by the `macros`-gated serve path")
    )]
    cache_config: Option<crate::cache::CacheConfig>,
    #[cfg(feature = "storage-fs")]
    #[cfg_attr(
        not(feature = "macros"),
        expect(dead_code, reason = "consumed only by the `macros`-gated serve path")
    )]
    storage_config: Option<crate::storage::StorageConfig>,
    #[cfg(feature = "mail")]
    #[cfg_attr(
        not(feature = "macros"),
        expect(dead_code, reason = "consumed only by the `macros`-gated serve path")
    )]
    mail_config: Option<crate::mail::SmtpConfig>,
    // The pre-routing proxy function (engine spec §4/§5). `None` → no proxy;
    // the request goes straight to the router.
    #[cfg_attr(
        not(feature = "macros"),
        expect(dead_code, reason = "consumed only by the `macros`-gated serve path")
    )]
    proxy: Option<crate::proxy::ProxyFn>,
    // The Vite IPC endpoint for the one-port dev proxy (AP2.1-3). `None` →
    // pass-through. Only meaningful with the `dev-proxy` feature.
    #[cfg(feature = "dev-proxy")]
    #[cfg_attr(
        not(feature = "macros"),
        expect(dead_code, reason = "consumed only by the `macros`-gated serve path")
    )]
    dev_proxy_endpoint: Option<crate::dev_proxy::endpoint::IpcEndpoint>,
}

/// The state closure: given the started [`Resources`] and the [`Lifecycle`],
/// produce the application state `S`.
pub type StateFn<S> = Arc<dyn Fn(&Resources, &Lifecycle) -> S + Send + Sync>;

impl<S: RouterState> Application<S> {
    /// Begin building an application.
    ///
    /// Returns the builder rather than `Self`: an `Application` only exists
    /// once `build()` has composed the pipeline, so there is no half-built
    /// `Application` to hand back.
    #[expect(
        clippy::new_ret_no_self,
        reason = "type-state builder: `Application` is only reachable via `build()`"
    )]
    pub fn new() -> ApplicationBuilder<S> {
        ApplicationBuilder {
            router: Router::new(),
            bind_addr: DEFAULT_BIND_ADDR.to_string(),
            port: DEFAULT_PORT,
            app_config: crate::config::AppConfig::new(),
            lifecycle: Lifecycle::new(),
            // On by default. An orchestrator that cannot probe an instance
            // has to guess, and it guesses by killing things.
            health: true,
            health_prefix: crate::application::health::DEFAULT_PREFIX.to_string(),
            route_table: RouteTable::empty(),
            // On by default. There is nothing to configure and no cost worth
            // naming, and a `redirect().route(..)` that answers `400` in a
            // default build would read as a broken framework.
            redirect_mapper: true,
            #[cfg(feature = "database")]
            database_config: None,
            #[cfg(feature = "jobs")]
            jobs_registry: None,
            #[cfg(feature = "jobs")]
            worker_config: None,
            #[cfg(feature = "jobs")]
            scheduler: None,
            #[cfg(feature = "cache")]
            cache_config: None,
            #[cfg(feature = "storage-fs")]
            storage_config: None,
            #[cfg(feature = "mail")]
            mail_config: None,
            proxy: None,
            // Read `ARCATURE_VITE_IPC` once, here. `arc dev` sets it to the
            // path Vite's `middlewareMode` server listens on, and an app that
            // did nothing but `Application::new()` has to pick it up on its
            // own -- otherwise the one-port dev topology needs an explicit
            // call the scaffold does not make, and Vite requests 404.
            // Unset (production) leaves the layer a pass-through.
            #[cfg(feature = "dev-proxy")]
            dev_proxy_endpoint: crate::dev_proxy::config::endpoint_from_env(),
            #[cfg(feature = "uag")]
            uag_graph: None,
            pipeline: Pipeline::new(),
            _state: std::marker::PhantomData,
        }
    }

    /// The bind address.
    #[must_use]
    pub fn bind_addr(&self) -> &str {
        &self.bind_addr
    }

    /// The bind port, as configured.
    ///
    /// The port actually listened on may differ: the environment is consulted
    /// at serve time, not here. See
    /// [`ApplicationBuilder::config`] for the precedence.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The [`AppConfig`](crate::config::AppConfig) this application was built
    /// with, or the default if [`ApplicationBuilder::config`] was never
    /// called.
    #[must_use]
    pub fn config(&self) -> &crate::config::AppConfig {
        &self.app_config
    }

    /// Consume the application and return the fully composed router.
    ///
    /// The router-level pipeline (body limit, timeout, session, CSRF, Inertia,
    /// user layers) is already applied; the service-level stages (proxy, dev
    /// proxy) are not, because they wrap the router as a service rather than
    /// as a `Router`.
    ///
    /// This exists so an application can be driven as a `tower::Service`
    /// without binding a socket — which is how the pipeline order is tested,
    /// and how the test kit will boot an app in-process.
    pub fn into_router(self) -> Router<S> {
        self.router
    }
}

impl<S: RouterState> Default for Application<S> {
    fn default() -> Self {
        // `Default` is only well-defined for the stateless app; for a stateful
        // app the builder is the entry point.
        let builder = Self::new();
        builder.build_stateless()
    }
}

/// The consuming builder for [`Application`].
pub struct ApplicationBuilder<S: RouterState = ()> {
    router: Router<S>,
    bind_addr: String,
    port: u16,
    // The top-level config. Defaulted rather than optional: every field has
    // a meaningful default, so `None` and `Some(AppConfig::new())` would mean
    // the same thing and only one of them would need handling at each use.
    app_config: crate::config::AppConfig,
    // Created here, not at serve time: `build()` composes the health
    // endpoints into the router and they need this exact handle.
    lifecycle: Lifecycle,
    // Whether to register the health endpoints at all, and where.
    health: bool,
    health_prefix: String,
    // The name -> path-template snapshot taken as routes come in. `Routes<S>`
    // is turned into a `Router<S>` immediately, which drops the names, so the
    // table has to be kept here or `redirect().route(..)` has nothing to
    // resolve against.
    route_table: RouteTable,
    // Whether to install the redirect mapper. On by default; see
    // `redirect_mapper`.
    redirect_mapper: bool,
    #[cfg(feature = "database")]
    database_config: Option<crate::database::DatabaseConfig>,
    #[cfg(feature = "jobs")]
    jobs_registry: Option<crate::jobs::Registry>,
    #[cfg(feature = "jobs")]
    worker_config: Option<crate::jobs::WorkerConfig>,
    #[cfg(feature = "jobs")]
    scheduler: Option<crate::jobs::Scheduler>,
    #[cfg(feature = "cache")]
    cache_config: Option<crate::cache::CacheConfig>,
    #[cfg(feature = "storage-fs")]
    storage_config: Option<crate::storage::StorageConfig>,
    #[cfg(feature = "mail")]
    mail_config: Option<crate::mail::SmtpConfig>,
    proxy: Option<crate::proxy::ProxyFn>,
    #[cfg(feature = "dev-proxy")]
    dev_proxy_endpoint: Option<crate::dev_proxy::endpoint::IpcEndpoint>,
    // The application graph the dev-only UAG endpoint serves, if one was
    // handed over. Kept as the graph rather than as a finished artifact
    // because the artifact also needs the page contracts, and `.uag_endpoint`
    // and `.page_contracts` can be called in either order.
    #[cfg(feature = "uag")]
    uag_graph: Option<crate::dx::application_graph::ApplicationGraph>,
    // The router-level layers, held in slots so their order is the documented
    // one rather than the order the builder methods were called in. See
    // [`crate::application::pipeline`].
    pipeline: Pipeline<S>,
    _state: std::marker::PhantomData<S>,
}

impl<S: RouterState> ApplicationBuilder<S> {
    /// Set the routes. Replaces any prior routes.
    #[must_use]
    pub fn routes(mut self, routes: Routes<S>) -> Self {
        self.route_table = routes.table();
        self.router = routes.into_router();
        self
    }

    /// Merge additional routes into the existing router.
    #[must_use]
    pub fn merge_routes(mut self, routes: Routes<S>) -> Self {
        // Later names win, matching `Router::merge` and `Routes::merge`.
        self.route_table = self
            .route_table
            .iter()
            .map(|(name, path)| (name.clone(), path.clone()))
            .chain(routes.table().iter().map(|(n, p)| (n.clone(), p.clone())))
            .collect();
        self.router = self.router.merge(routes.into_router());
        self
    }

    /// Set the bind address (default `127.0.0.1`).
    #[must_use]
    pub fn bind(mut self, addr: impl Into<String>) -> Self {
        self.bind_addr = addr.into();
        self
    }

    /// Set the port (default `3000`). See [`config`](Self::config) for how a
    /// port set here interacts with the environment at run time.
    #[must_use]
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Supply the top-level [`AppConfig`].
    ///
    /// `port` becomes the port this process listens on, as though
    /// [`port`](Self::port) had been called. `name` and `url` are read on the
    /// startup line: the framework's one unprompted log record names the
    /// application and the base URL it believes it is reachable at, so a
    /// deployment that is answering on an address nobody expected says so in
    /// its first line of output rather than in a broken link three days
    /// later.
    ///
    /// Beyond that line, `url` is reached through
    /// [`AppConfig::absolute_url`](crate::config::AppConfig::absolute_url) --
    /// the accessor any subsystem that needs a link with no request in scope
    /// is meant to call. `env` is read by nothing and is forbidden from
    /// gating behaviour (see [`AppConfig`]).
    ///
    /// # Port precedence
    ///
    /// Highest wins:
    ///
    /// 1. `ARCATURE_BACKEND_PORT` -- set by `arc dev`, whose supervisor owns
    ///    the only TCP listener. It outranks everything, including `PORT`,
    ///    because a `PORT` left in a developer's `.env` would otherwise send
    ///    the child to the address the supervisor is already bound to and
    ///    break the one-port guarantee with a message about the port being
    ///    in use.
    /// 2. `PORT` -- the platform convention. A host that assigns a port has
    ///    to be able to override the source.
    /// 3. `APP_PORT` -- the application's own `.env`.
    /// 4. Whatever `.config(..)` or `.port(..)` last set, or [`DEFAULT_PORT`].
    ///
    /// The first three are read once, at serve time, in
    /// [`resolve_port`]. Because `.config(..)` writes the same field
    /// `.port(..)` does, the two compose by call order: the later call wins.
    #[must_use]
    pub fn config(mut self, config: crate::config::AppConfig) -> Self {
        self.port = config.port;
        self.app_config = config;
        self
    }

    /// Enable the database subsystem with the given config.
    #[cfg(feature = "database")]
    #[must_use]
    pub fn database(mut self, config: crate::database::DatabaseConfig) -> Self {
        self.database_config = Some(config);
        self
    }

    /// Enable the job queue subsystem with a handler registry. The worker and
    /// optional scheduler are started automatically from the shared database
    /// pool during startup, so the `database` feature must also be enabled.
    ///
    /// When `scheduler` is also set via [`scheduler`](Self::scheduler), both
    /// the worker and the scheduler run until shutdown.
    #[cfg(feature = "jobs")]
    #[must_use]
    pub fn jobs(mut self, registry: crate::jobs::Registry) -> Self {
        self.jobs_registry = Some(registry);
        self
    }

    /// Override the worker configuration (concurrency, lease, heartbeat, etc.).
    /// Only meaningful when [`jobs`](Self::jobs) is set. If omitted, the worker
    /// uses [`WorkerConfig::default`](crate::jobs::WorkerConfig::default).
    #[cfg(feature = "jobs")]
    #[must_use]
    pub fn worker_config(mut self, config: crate::jobs::WorkerConfig) -> Self {
        self.worker_config = Some(config);
        self
    }

    /// Register a recurring-job scheduler. The scheduler enqueues jobs on a
    /// cadence; the worker (started via [`jobs`](Self::jobs)) claims and runs
    /// them. Without a worker the scheduler would only enqueue jobs that no
    /// one runs, so this is only meaningful when [`jobs`](Self::jobs) is set.
    #[cfg(feature = "jobs")]
    #[must_use]
    pub fn scheduler(mut self, scheduler: crate::jobs::Scheduler) -> Self {
        self.scheduler = Some(scheduler);
        self
    }

    /// Enable the cache subsystem.
    #[cfg(feature = "cache")]
    #[must_use]
    pub fn cache(mut self, config: crate::cache::CacheConfig) -> Self {
        self.cache_config = Some(config);
        self
    }

    /// Enable the storage subsystem.
    #[cfg(feature = "storage-fs")]
    #[must_use]
    pub fn storage(mut self, config: crate::storage::StorageConfig) -> Self {
        self.storage_config = Some(config);
        self
    }

    /// Enable the mail subsystem.
    #[cfg(feature = "mail")]
    #[must_use]
    pub fn mail(mut self, config: crate::mail::SmtpConfig) -> Self {
        self.mail_config = Some(config);
        self
    }

    /// Install the pre-routing proxy function (engine spec §4/§5).
    ///
    /// The `proxy` function runs *before* route selection and can continue,
    /// redirect, rewrite, mutate request headers, or short-circuit with an
    /// early response. It is a pure, synchronous decision from a
    /// [`ProxyRequest`](crate::proxy::ProxyRequest) borrow to a
    /// [`ProxyAction`](crate::proxy::ProxyAction) — no async, no I/O. The
    /// engine performs the actual HTTP work (setting status/headers,
    /// rewriting the URI, delegating to the router). Pass `None` (the
    /// default) to leave the router as the outermost layer.
    #[must_use]
    pub fn proxy<F>(mut self, proxy: F) -> Self
    where
        F: Fn(crate::proxy::ProxyRequest<'_>) -> crate::proxy::ProxyAction + Send + Sync + 'static,
    {
        self.proxy = Some(std::sync::Arc::new(proxy));
        self
    }

    /// Set the Vite IPC endpoint for the one-port dev proxy (AP2.1-3). When
    /// `Some(path)`, the dev proxy forwards Vite-looking requests
    /// (`/@vite/`, `/src/...`, HMR WebSocket) to the Vite dev server over
    /// IPC; everything else reaches the application pipeline. When `None`,
    /// the dev proxy is a zero-overhead pass-through.
    ///
    /// Only available with the `dev-proxy` feature. The endpoint is already
    /// resolved from `ARCATURE_VITE_IPC` (set by `arc dev`) when the builder
    /// is created; this method overrides that, including back to `None` to
    /// switch the dev proxy off in a process where the variable is set.
    #[cfg(feature = "dev-proxy")]
    #[must_use]
    pub fn dev_proxy_endpoint(mut self, endpoint: Option<std::path::PathBuf>) -> Self {
        self.dev_proxy_endpoint = endpoint.map(crate::dev_proxy::endpoint::IpcEndpoint::new);
        self
    }

    /// Attach a [`tower::Layer`] to the whole application.
    ///
    /// This is the general escape hatch, and the one method the rest of the
    /// pipeline is built on: `tower_http` layers, third-party Tower
    /// middleware, and anything else that wraps an `axum::Router`.
    ///
    /// User layers are the innermost stage of the pipeline — a request
    /// reaching one has already passed the body limit, the timeout, the
    /// session, CSRF, and Inertia. Among themselves they nest in call order,
    /// first call outermost. See [`crate::application::pipeline`] for the full
    /// order and the reasoning behind it.
    ///
    /// ```ignore
    /// Application::new()
    ///     .routes(web::routes())
    ///     .layer(tower_http::compression::CompressionLayer::new())
    /// ```
    #[must_use]
    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: tower::Layer<axum::routing::Route> + Clone + Send + Sync + 'static,
        L::Service: tower::Service<crate::routing::Request, Error = std::convert::Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        <L::Service as tower::Service<crate::routing::Request>>::Response:
            axum::response::IntoResponse + 'static,
        <L::Service as tower::Service<crate::routing::Request>>::Future: Send + 'static,
    {
        self.pipeline.user.push(RouterLayer::from_layer(layer));
        self
    }

    /// Compress responses (`gzip`, `br`) when the client asks for it.
    ///
    /// Outermost of the response-shaping stages, so it sees the final body --
    /// including one produced by a layer below rather than by a handler.
    /// Off unless called: compressing an already-compressed image or a
    /// two-byte JSON response costs CPU and saves nothing, and `tower-http`
    /// only skips the obvious cases.
    #[must_use]
    pub fn compression(mut self) -> Self {
        self.pipeline.compression = true;
        self
    }

    /// Add security headers to every response.
    ///
    /// Installed outside the body limit and the timeout, so a `413` and a
    /// `408` carry them too -- a browser renders those pages as readily as it
    /// renders a handler's. See [`SecurityHeaders`](crate::http::SecurityHeaders)
    /// for which headers are always on and which are opt-in.
    ///
    /// ```ignore
    /// Application::new()
    ///     .routes(web::routes())
    ///     .security_headers(SecurityHeaders::new().with_hsts())
    /// ```
    #[must_use]
    pub fn security_headers(mut self, headers: crate::http::SecurityHeaders) -> Self {
        self.pipeline.security_headers = Some(headers);
        self
    }

    /// Install a CORS policy.
    ///
    /// The layer is a parameter rather than a set of builder options because
    /// a CORS policy is entirely application-specific and `tower-http`'s
    /// builder already expresses it -- wrapping it would only add a second
    /// vocabulary for the same thing.
    ///
    /// ```ignore
    /// use tower_http::cors::{Any, CorsLayer};
    ///
    /// Application::new()
    ///     .routes(api::routes())
    ///     .cors(CorsLayer::new().allow_origin("https://acme.test".parse::<HeaderValue>()?))
    /// ```
    #[must_use]
    pub fn cors(mut self, layer: tower_http::cors::CorsLayer) -> Self {
        self.pipeline.cors = Some(RouterLayer::from_layer(layer));
        self
    }

    /// Give every request an id and echo it as `x-request-id`.
    ///
    /// An inbound `x-request-id` is reused so a trace survives the hop from a
    /// reverse proxy; otherwise one is generated. Installed above the access
    /// log, which reads the id out of request extensions -- switching this off
    /// while leaving the log on produces log lines with an empty id rather
    /// than an error.
    #[cfg(feature = "observe")]
    #[must_use]
    pub fn request_id(mut self) -> Self {
        self.pipeline.request_id = true;
        self
    }

    /// Emit one `tracing` line per request: method, path, status, duration,
    /// request id.
    ///
    /// Installed outside the panic catcher, the body limit and the timeout, so
    /// a `500`, a `413` and a `408` are all logged. Pair with
    /// [`request_id`](Self::request_id) for the id to be populated.
    #[cfg(feature = "observe")]
    #[must_use]
    pub fn access_log(mut self) -> Self {
        self.pipeline.access_log = true;
        self
    }

    /// Turn a panic below this point into a `500` carrying an RFC 9457
    /// `Problem`.
    ///
    /// Without it a panicking handler drops the connection, which a client
    /// sees as a network failure rather than a server error -- and which no
    /// access log line explains, because no response was ever produced.
    ///
    /// The panic payload never reaches the client: it is written for a
    /// developer reading a backtrace and routinely contains a file path, a SQL
    /// fragment, or the value that caused the panic. `tower-http` logs it for
    /// the operator.
    #[must_use]
    pub fn catch_panic(mut self) -> Self {
        self.pipeline.catch_panic = true;
        self
    }

    /// Serve a document root for requests no route matched.
    ///
    /// Installs [`StaticFiles`](crate::assets::StaticFiles) as the router's
    /// **fallback**, so it never shadows a route: a request reaches it only
    /// after route matching has failed. `Cache-Control` is chosen per path --
    /// a hashed file under the build prefix is immutable for a year, anything
    /// else revalidates. See [`crate::assets`].
    ///
    /// This is the production half of the one-port story. In development the
    /// dev proxy hands Vite requests to Vite; in production nothing is
    /// running but this process, so the built assets have to come from here.
    ///
    /// Replaces any fallback the routes defined. An application that wants
    /// its own 404 page should render it from a catch-all route rather than
    /// from a fallback.
    #[must_use]
    pub fn static_files(mut self, config: &crate::assets::AssetsConfig) -> Self {
        self.pipeline.static_files = Some(crate::assets::StaticFiles::new(config));
        self
    }

    /// Limit the request body to `bytes`.
    ///
    /// Applied outside everything that reads a body, so an oversized upload is
    /// refused with `413 Payload Too Large` without being buffered. No limit
    /// is applied unless this is called.
    #[must_use]
    pub fn body_limit(mut self, bytes: usize) -> Self {
        self.pipeline.body_limit = Some(bytes);
        self
    }

    /// Bound the total time spent on a request.
    ///
    /// A request still in flight when the deadline passes gets
    /// `408 Request Timeout`. No timeout is applied unless this is called.
    #[must_use]
    pub fn timeout(mut self, duration: std::time::Duration) -> Self {
        self.pipeline.timeout = Some(duration);
        self
    }

    /// Enable native Inertia with the given configuration.
    ///
    /// Installs [`InertiaLayer`](crate::inertia::InertiaLayer), which puts the
    /// resolved config and the parsed request context into request extensions.
    /// **The [`Inertia`](crate::inertia::Inertia) extractor fails without
    /// it**: a handler taking `inertia: Inertia` in an application that never
    /// called this method returns `500 inertia adapter error`.
    #[cfg(feature = "inertia")]
    #[must_use]
    pub fn inertia(mut self, config: crate::inertia::InertiaConfig) -> Self {
        self.pipeline.inertia = Some(RouterLayer::from_layer(crate::inertia::InertiaLayer::new(
            config,
        )));
        self
    }

    /// Enable sessions, backed by `store`.
    ///
    /// The store is a parameter because the session layer is generic over it
    /// and Arcature does not pick one for you: an in-memory store for tests, a
    /// database or Redis store in production.
    ///
    /// # Errors
    ///
    /// Returns [`SessionBuildError`](crate::auth::SessionBuildError) if the
    /// configuration is internally inconsistent — a `__Host-` cookie without
    /// `Secure`, a zero max-age, a signing key of the wrong length. Failing
    /// here rather than at request time means a misconfigured session cannot
    /// reach production silently.
    #[cfg(feature = "auth")]
    pub fn session<Store>(
        mut self,
        config: crate::auth::SessionConfig,
        store: Store,
    ) -> std::result::Result<Self, crate::auth::SessionBuildError>
    where
        Store: tower_sessions::SessionStore + Clone,
    {
        let layer = config.into_layer(store)?;
        self.pipeline.session = Some(RouterLayer::from_layer(layer));
        Ok(self)
    }

    /// Enable double-submit-cookie CSRF protection.
    ///
    /// Runs after the session and before the handler, so an unsafe request
    /// with a missing or mismatched token is rejected before it can act.
    /// Bearer-token requests are exempt (an `Authorization: Bearer` request is
    /// not sent automatically by a browser, so it is not forgeable the same
    /// way).
    #[cfg(feature = "auth")]
    #[must_use]
    pub fn csrf(mut self, config: crate::auth::CsrfConfig) -> Self {
        self.pipeline.csrf = Some(RouterLayer::from_layer(
            crate::auth::CsrfLayer::with_config(config),
        ));
        self
    }

    /// Turn the health endpoints on or off. On by default.
    ///
    /// They are `GET {prefix}`, `GET {prefix}/live` and `GET {prefix}/ready`
    /// under [`health::DEFAULT_PREFIX`](crate::application::health::DEFAULT_PREFIX),
    /// and they are merged *beside* the router rather than layered over it --
    /// no session load, no maintenance `503`, no access-log line on a request
    /// an orchestrator makes every few seconds.
    ///
    /// Turn them off only when something in front of the application already
    /// owns those paths. An application with no probe endpoint is one an
    /// orchestrator can only judge by whether the port accepts a connection,
    /// which a wedged process does perfectly well.
    #[must_use]
    pub fn health(mut self, enabled: bool) -> Self {
        self.health = enabled;
        self
    }

    /// Mount the health endpoints somewhere other than `/up`.
    ///
    /// Implies `.health(true)`: asking for a prefix and getting no endpoints
    /// would be the wrong reading of this call.
    #[must_use]
    pub fn health_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.health_prefix = prefix.into();
        self.health = true;
        self
    }

    /// Turn the named-route redirect resolver on or off. On by default.
    ///
    /// [`RedirectResponse`](crate::http::response::RedirectResponse) cannot
    /// resolve `redirect().route("users.show", id)` on its own, because
    /// `IntoResponse` sees neither the route table nor the session. It leaves
    /// the unresolved builder in the response extensions and
    /// [`RedirectMapper`](crate::routing::RedirectMapper) finishes it. Turning
    /// this off is therefore not a way to make redirects cheaper; it is a way
    /// to get a `400` for every named-route redirect and to drop every flash
    /// message. It exists so an application that installs its own mapper --
    /// against a table this builder does not know about -- can avoid running
    /// two.
    #[must_use]
    pub fn redirect_mapper(mut self, enabled: bool) -> Self {
        self.redirect_mapper = enabled;
        self
    }

    /// Install the maintenance switch.
    ///
    /// Keep the [`Maintenance`](crate::http::Maintenance) handle: it is what
    /// engages and disengages the window. The health prefix is exempted
    /// automatically.
    ///
    /// ```no_run
    /// use arcature::{Application, http::Maintenance};
    ///
    /// let maintenance = Maintenance::new();
    /// let app = Application::<()>::new()
    ///     .maintenance(maintenance.clone())
    ///     .build();
    /// // ... later, from an admin route or a signal handler:
    /// maintenance.engage();
    /// ```
    #[must_use]
    pub fn maintenance(mut self, maintenance: crate::http::Maintenance) -> Self {
        self.pipeline.maintenance = Some(maintenance);
        self
    }

    /// Install a rate limit across the whole application.
    ///
    /// Keep the handle if the same limit is also installed on a route or a
    /// group: cloning a [`RateLimit`](crate::routing::RateLimit) shares the
    /// buckets, so one value used in two places is one quota, not two.
    ///
    /// The limit sits inside the maintenance switch and outside the session,
    /// which is to say a request answered by a maintenance `503` costs no
    /// quota and a refused request never loads a session. The health
    /// endpoints are merged outside every layer here, so a probe is never
    /// throttled -- a rate-limited readiness check is a self-inflicted
    /// outage.
    ///
    /// ```no_run
    /// use arcature::{Application, routing::RateLimit};
    ///
    /// let app = Application::<()>::new()
    ///     .rate_limit(RateLimit::per_minute(60))
    ///     .build();
    /// ```
    #[must_use]
    pub fn rate_limit(mut self, limit: crate::routing::RateLimit) -> Self {
        self.pipeline.rate_limit = Some(limit);
        self
    }

    /// Install error-response mapping.
    ///
    /// Gives an RFC 9457 [`Problem`](crate::api::Problem) body to the bodiless
    /// errors axum and `tower-http` produce -- the bare `404`, `405`, `408`
    /// and `413` that never reach a handler -- and redacts `text/plain` 5xx
    /// bodies in release builds. See [`crate::http::ErrorMapping`] for the
    /// exact rules and for `with(..)`, which replaces a response outright.
    #[must_use]
    pub fn error_mapping(mut self, mapping: crate::http::ErrorMapping) -> Self {
        self.pipeline.error_mapping = Some(mapping);
        self
    }

    /// Publish the page-contract artifact as a request extension.
    ///
    /// This is what the dev-only UAG endpoint and `arc typegen` read to learn
    /// which props each page component receives. It is data, not behaviour:
    /// installing it changes no response.
    #[cfg(feature = "inertia")]
    #[must_use]
    pub fn page_contracts(
        mut self,
        artifact: impl Into<Arc<crate::inertia::contracts::ContractArtifact>>,
    ) -> Self {
        self.pipeline.page_contracts = Some(artifact.into());
        self
    }

    /// Serve the application graph from `GET /_arcature/uag.json`.
    ///
    /// `arc dev` fetches this after every restart and rewrites
    /// `resources/js/generated/`, which is how typed routes, page props and
    /// form helpers stay in step with the Rust source without a `build.rs` or
    /// a second binary on the edit path.
    ///
    /// The graph is combined with whatever
    /// [`page_contracts`](Self::page_contracts) supplied -- an empty registry
    /// if nothing did -- and serialized once, here.
    ///
    /// This is a development affordance and the registration is refused in a
    /// build with `debug_assertions` off, with one line on stderr saying so.
    /// [`crate::application::uag_endpoint`] documents the full gate and why it
    /// has three independent parts.
    ///
    /// ```no_run
    /// # use arcature::Application;
    /// # use arcature::dx::application_graph::ApplicationGraph;
    /// # fn graph() -> ApplicationGraph { ApplicationGraph::new_unchecked(Vec::new()) }
    /// #[cfg(feature = "dev")]
    /// let app = Application::<()>::new().uag_endpoint(graph()).build();
    /// ```
    #[cfg(feature = "uag")]
    #[must_use]
    pub fn uag_endpoint(mut self, graph: crate::dx::application_graph::ApplicationGraph) -> Self {
        self.uag_graph = Some(graph);
        self
    }

    /// Build the application. For the stateless app (`S = ()`) this is the
    /// final step before `run()`. For a stateful app, the state is produced
    /// at run time from the started resources.
    #[must_use]
    pub fn build(mut self) -> Application<S> {
        let health = self.health.then(|| {
            crate::application::health::Health::new(&self.health_prefix, self.lifecycle.clone())
        });
        if let Some(health) = &health {
            // Belt and braces. The health endpoints are already merged
            // outside the maintenance layer, so this exemption changes
            // nothing for them -- but an application that installs its own
            // `Maintenance` through `.layer()` puts it somewhere this
            // builder does not control, and the exemption is what keeps the
            // probes answering there too.
            if let Some(maintenance) = self.pipeline.maintenance.take() {
                self.pipeline.maintenance = Some(maintenance.allow(health.prefix()));
            }
            self.pipeline.health = Some(health.clone());
        }

        // The UAG endpoint, if one was asked for and this build is allowed to
        // serve it. Composed here rather than in `.uag_endpoint` so the page
        // contracts are already known whichever order the two were called in.
        #[cfg(feature = "uag")]
        if let Some(graph) = self.uag_graph.take() {
            if crate::application::uag_endpoint::UagEndpoint::allowed() {
                let contracts = match &self.pipeline.page_contracts {
                    Some(artifact) => crate::uag::build(&graph, artifact),
                    None => crate::uag::build(
                        &graph,
                        &crate::inertia::contracts::PageContracts::new().artifact(),
                    ),
                };
                self.pipeline.uag = Some(crate::application::uag_endpoint::UagEndpoint::new(
                    &contracts,
                ));
            } else {
                // Loud once, at boot, rather than silent: someone wired the
                // endpoint and will otherwise spend the afternoon wondering
                // why `arc typegen` cannot reach it.
                eprintln!(
                    "arcature: not serving {} -- the application graph is a                      development endpoint and this build has debug assertions off",
                    crate::application::uag_endpoint::PATH
                );
            }
        }

        // The redirect mapper, from the table `.routes()` snapshotted. Built
        // here rather than in `.routes()` so that a later `.merge_routes()`
        // is included: the mapper holds a snapshot, and the snapshot has to
        // be the final one.
        if self.redirect_mapper {
            self.pipeline.redirect_mapper = Some(crate::routing::RedirectMapper::new(
                self.route_table.clone(),
            ));
        }

        Application {
            // The router-level pipeline is composed here, once, so that
            // `serve` and `run_with_state` cannot disagree about it.
            router: self.pipeline.apply(self.router),
            bind_addr: self.bind_addr,
            port: self.port,
            app_config: self.app_config,
            lifecycle: self.lifecycle,
            health,
            #[cfg(feature = "database")]
            database_config: self.database_config,
            #[cfg(feature = "jobs")]
            jobs_registry: self.jobs_registry,
            #[cfg(feature = "jobs")]
            worker_config: self.worker_config,
            #[cfg(feature = "jobs")]
            scheduler: self.scheduler,
            #[cfg(feature = "cache")]
            cache_config: self.cache_config,
            #[cfg(feature = "storage-fs")]
            storage_config: self.storage_config,
            #[cfg(feature = "mail")]
            mail_config: self.mail_config,
            proxy: self.proxy,
            #[cfg(feature = "dev-proxy")]
            dev_proxy_endpoint: self.dev_proxy_endpoint,
        }
    }

    /// Build the stateless application directly (convenience for `S = ()`).
    #[must_use]
    pub fn build_stateless(self) -> Application<S> {
        self.build()
    }
}

impl Application<()> {
    /// Run the stateless application: bind, serve, and shut down on Ctrl-C.
    ///
    /// Requires the `macros` feature (the certified Tokio runtime).
    #[cfg(feature = "macros")]
    pub async fn run(self) -> EngineResult<()> {
        self.run_with_state(Arc::new(|_r: &Resources, _lc: &Lifecycle| ()))
            .await
    }

    /// Serve on an already-bound listener. The stateless escape hatch.
    #[cfg(feature = "macros")]
    pub async fn serve<L>(self, listener: L) -> EngineResult<()>
    where
        L: axum::serve::Listener,
        L::Addr: std::fmt::Debug,
    {
        let lifecycle = self.lifecycle.clone();
        // No subsystems on this path -- `serve` is the escape hatch that
        // skips ordered startup -- but the health endpoints still get the
        // (empty) resource set, so `/up/ready` reports on the lifecycle
        // rather than on nothing at all.
        if let Some(health) = &self.health {
            health.publish(Resources::empty());
        }
        lifecycle.mark_ready();
        let service = crate::application::pipeline::compose_service(
            self.router,
            self.proxy.clone(),
            #[cfg(feature = "dev-proxy")]
            self.dev_proxy_endpoint.clone(),
        );
        use crate::axum::ServiceExt as _;
        axum::serve(listener, service.into_make_service())
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|source| EngineError::Serve { source })?;
        lifecycle.begin_drain();
        lifecycle.mark_stopped();
        Ok(())
    }
}

impl<S: RouterState> Application<S> {
    /// Run the application with a state closure. Starts subsystems in order
    /// (database, jobs, cache, storage, mail), builds the state from the
    /// started resources, applies it to the router, marks readiness, serves
    /// with graceful shutdown, and tears subsystems down in reverse.
    ///
    /// The job worker (and optional scheduler) reuse the database pool and
    /// are spawned as managed tasks that exit on shutdown.
    ///
    /// Requires the `macros` feature.
    #[cfg(feature = "macros")]
    // `self` is mutated by exactly one thing: the `jobs` block's `.take()`
    // calls. The other subsystems read `&self` and mutate `resources`, which
    // carries its own `mut`. So the predicate is `jobs`, not the list of every
    // subsystem -- with `cache` on and `jobs` off, `mut self` is dead and the
    // wider predicate let the warning through. One `expect` covers both
    // bindings, which is what the all-off case needs.
    #[cfg_attr(
        not(feature = "jobs"),
        expect(
            unused_mut,
            reason = "`self` is only mutated to take the job registry and scheduler;                       with no subsystem at all, `resources` is not mutated either"
        )
    )]
    pub async fn run_with_state(mut self, state_fn: StateFn<S>) -> EngineResult<()> {
        let port = resolve_port(self.port);
        let addr: SocketAddr = format!("{}:{}", self.bind_addr, port)
            .parse()
            .map_err(|_| EngineError::InvalidPort(port))?;

        // Where this process listens is not always a port: under `arc dev`
        // the supervisor owns the only TCP listener and the application is a
        // child on a private IPC endpoint. See
        // [`crate::application::serve_ipc`].
        let listener = crate::application::serve_ipc::ServeTarget::bind(addr).await?;

        let lifecycle = self.lifecycle.clone();

        // Take ownership of the jobs fields up front: the worker needs the
        // registry by value and the scheduler is not Clone (its entries hold
        // boxed closures). Everything else borrows `&self`.
        #[cfg(feature = "jobs")]
        let jobs_registry = self.jobs_registry.take();
        #[cfg(feature = "jobs")]
        let worker_config = self.worker_config;
        #[cfg(feature = "jobs")]
        let scheduler = self.scheduler.take();

        // Ordered startup: database → cache → storage → mail.
        let mut resources = Resources::empty();
        #[cfg(feature = "database")]
        if let Some(cfg) = &self.database_config {
            let db = crate::database::Db::connect(cfg.clone())
                .await
                .map_err(|e| EngineError::Startup {
                    subsystem: "database",
                    stage: "connect",
                    source: e,
                })?;
            resources.set_db(db);
        }

        // Jobs reuse the database pool: migrate, build the facade, spawn the
        // worker (and optional scheduler) as managed tasks.
        #[cfg(feature = "jobs")]
        let jobs_runtime = crate::application::jobs_runtime::start_jobs(
            jobs_registry,
            worker_config,
            scheduler,
            &mut resources,
        )
        .await?;
        #[cfg(feature = "cache")]
        if let Some(cfg) = &self.cache_config {
            let cache = crate::cache::Cache::connect(cfg.clone())
                .await
                .map_err(|e| EngineError::Startup {
                    subsystem: "cache",
                    stage: "connect",
                    source: e.into(),
                })?;
            resources.set_cache(cache);
        }
        #[cfg(feature = "storage-fs")]
        if let Some(cfg) = &self.storage_config {
            let storage = crate::storage::Storage::connect(cfg.clone())
                .await
                .map_err(|e| EngineError::Startup {
                    subsystem: "storage",
                    stage: "connect",
                    source: e.into(),
                })?;
            resources.set_storage(storage);
        }
        #[cfg(feature = "mail")]
        if let Some(cfg) = &self.mail_config {
            let mailer =
                crate::mail::Mailer::smtp(cfg.clone()).map_err(|e| EngineError::Startup {
                    subsystem: "mail",
                    stage: "connect",
                    source: e.into(),
                })?;
            resources.set_mail(mailer);
        }

        // Hand the started subsystems to the health endpoints. Before this
        // point the readiness report lists no checks and the lifecycle is
        // still `Starting`, so `/up/ready` answers `503` -- which is the
        // honest answer for a process that has not finished booting.
        if let Some(health) = &self.health {
            health.publish(resources.clone());
        }

        // Build the application state from the started resources.
        let state = state_fn(&resources, &lifecycle);

        // Collapse the stateful router to a stateless one for serving.
        let router: Router<()> = self.router.with_state(state);

        // Compose the service-level stages of the pipeline. The router-level
        // stages were composed in `build()`; these two wrap the router as a
        // service because the proxy rewrites the URI before route selection.
        // See [`crate::application::pipeline`] for the full order.
        let service = crate::application::pipeline::compose_service(
            router,
            self.proxy.clone(),
            #[cfg(feature = "dev-proxy")]
            self.dev_proxy_endpoint.clone(),
        );

        lifecycle.mark_ready();

        // The one line a booting application prints, and the only one the
        // framework emits unprompted. It goes out *after* `mark_ready`, so
        // the address it names is one that already answers: a line printed
        // before the subsystems finished starting would invite a request the
        // process is not yet able to serve.
        //
        // Through `tracing`, not `println!`, for the same reason as
        // everything else here -- it carries a field rather than a sentence,
        // it lands in whatever the operator configured, and a process that
        // installed no subscriber stays quiet by its own choice. See
        // [`crate::observe::install_logging`].
        //
        // `app` and `url` come from `AppConfig`. They are on this record
        // rather than nowhere because the commonest deployment mistake with
        // `APP_URL` is silent: the value is only spent on links built outside
        // a request, so a wrong one produces a working service that mails out
        // unreachable addresses. Printing what the process believes about
        // itself at boot turns that into something an operator can see.
        #[cfg(feature = "observe")]
        tracing::info!(
            at = %listener.describe(),
            app = %self.app_config.name,
            url = %self.app_config.base_url(),
            "listening"
        );
        #[cfg(not(feature = "observe"))]
        eprintln!(
            "{} listening on {} ({})",
            self.app_config.name,
            listener.describe(),
            self.app_config.base_url()
        );

        listener.serve(service, shutdown_signal()).await?;

        lifecycle.begin_drain();

        // Reverse-order shutdown: mail → storage → cache → jobs → database.
        #[cfg(feature = "mail")]
        if let Some(mailer) = resources.mail() {
            let _ = mailer.shutdown().await;
        }
        #[cfg(feature = "storage-fs")]
        {
            drop(resources.storage().cloned());
        }
        #[cfg(feature = "cache")]
        if let Some(cache) = resources.cache() {
            let _ = cache.close().await;
        }
        #[cfg(feature = "jobs")]
        if let Some(runtime) = jobs_runtime {
            runtime.shutdown().await?;
        }
        #[cfg(feature = "database")]
        if let Some(db) = resources.db() {
            let _ = db.close().await;
        }

        lifecycle.mark_stopped();
        Ok(())
    }
}

#[cfg(feature = "macros")]
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await;
    }
}

/// Resolve the port to listen on, consulting the environment.
///
/// The order of the array is the precedence, highest first, and it is the
/// only place the three names are read. See
/// [`ApplicationBuilder::config`] for the reasoning behind each position; the
/// one that is not obvious is that `ARCATURE_BACKEND_PORT` outranks `PORT`.
/// Under `arc dev` the supervisor owns the process's only TCP listener and
/// puts the application on a private endpoint; a stale `PORT` in a
/// developer's `.env` winning that contest would aim the child at the
/// address the supervisor already holds.
///
/// A value that is present but not a `u16` is skipped rather than fatal: the
/// next source down is a better answer than refusing to boot, and the
/// alternative -- treating `PORT=""` as an error -- makes an empty variable
/// in a compose file a crash.
#[cfg(feature = "macros")]
fn resolve_port(configured: u16) -> u16 {
    port_from(configured, |key| std::env::var(key).ok())
}

/// The port sources, highest precedence first.
#[cfg(feature = "macros")]
const PORT_ENV_KEYS: [&str; 3] = ["ARCATURE_BACKEND_PORT", "PORT", "APP_PORT"];

/// [`resolve_port`] with the environment passed in.
///
/// Split out purely so the precedence can be tested. `std::env::set_var` is
/// `unsafe` in edition 2024 and this crate is `#![forbid(unsafe_code)]`, so a
/// test that sets `PORT` cannot be written at all -- and even if it could,
/// the process environment is global and Rust runs tests in threads, so two
/// such tests would race. A closure makes the ordering a pure function.
#[cfg(feature = "macros")]
fn port_from(configured: u16, lookup: impl Fn(&str) -> Option<String>) -> u16 {
    for key in PORT_ENV_KEYS {
        if let Some(value) = lookup(key)
            && let Ok(port) = value.parse::<u16>()
        {
            return port;
        }
    }
    configured
}

#[cfg(all(test, feature = "macros"))]
mod tests {
    use super::*;
    use crate::config::{AppConfig, AppEnvironment};

    /// A lookup over a fixed table, standing in for the process environment.
    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key| owned.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    #[test]
    fn nothing_in_the_environment_leaves_the_configured_port_alone() {
        assert_eq!(port_from(8080, env_of(&[])), 8080);
    }

    #[test]
    fn the_supervisor_outranks_the_platform_and_the_dotenv() {
        // The one that matters: `arc dev` owns the only TCP listener, so a
        // `PORT` left in a developer's `.env` must not aim the child at it.
        let port = port_from(
            3000,
            env_of(&[
                ("APP_PORT", "3000"),
                ("PORT", "3000"),
                ("ARCATURE_BACKEND_PORT", "41234"),
            ]),
        );
        assert_eq!(port, 41234);
    }

    #[test]
    fn the_platform_outranks_the_dotenv() {
        let port = port_from(3000, env_of(&[("APP_PORT", "3000"), ("PORT", "8080")]));
        assert_eq!(port, 8080);
    }

    #[test]
    fn app_port_is_read_when_it_is_the_only_source() {
        assert_eq!(port_from(3000, env_of(&[("APP_PORT", "9001")])), 9001);
    }

    #[test]
    fn an_unparseable_value_falls_through_instead_of_refusing_to_boot() {
        // `PORT=""` in a compose file should not be a crash, and it should
        // not shadow a source further down either.
        let port = port_from(3000, env_of(&[("PORT", ""), ("APP_PORT", "9001")]));
        assert_eq!(port, 9001);
        assert_eq!(port_from(3000, env_of(&[("PORT", "not-a-port")])), 3000);
        // 65536 does not fit a `u16`.
        assert_eq!(port_from(3000, env_of(&[("PORT", "65536")])), 3000);
    }

    #[test]
    fn config_sets_the_port_and_is_readable_back() {
        let app = Application::<()>::new()
            .config(
                AppConfig::new()
                    .name("Demo")
                    .url("https://demo.example")
                    .environment(AppEnvironment::Production)
                    .port(4321),
            )
            .build();

        assert_eq!(app.port(), 4321, "`config(..)` should set the bind port");
        assert_eq!(app.config().name, "Demo");
        assert_eq!(app.config().url, "https://demo.example");
        assert_eq!(app.config().env, AppEnvironment::Production);
    }

    #[test]
    fn config_and_port_compose_by_call_order() {
        let later_port = Application::<()>::new()
            .config(AppConfig::new().port(4321))
            .port(5555)
            .build();
        assert_eq!(later_port.port(), 5555);

        let later_config = Application::<()>::new()
            .port(5555)
            .config(AppConfig::new().port(4321))
            .build();
        assert_eq!(later_config.port(), 4321);
    }

    #[test]
    fn an_application_that_never_calls_config_still_has_one() {
        let app = Application::<()>::new().build();
        assert_eq!(app.port(), DEFAULT_PORT);
        assert_eq!(app.config().port, DEFAULT_PORT);
        assert!(app.config().env.is_development());
    }
}
