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
use crate::routing::{RouterLayer, RouterState, Routes};
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
            pipeline: Pipeline::new(),
            _state: std::marker::PhantomData,
        }
    }

    /// The bind address.
    #[must_use]
    pub fn bind_addr(&self) -> &str {
        &self.bind_addr
    }

    /// The bind port.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
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
        self.router = routes.into_router();
        self
    }

    /// Merge additional routes into the existing router.
    #[must_use]
    pub fn merge_routes(mut self, routes: Routes<S>) -> Self {
        self.router = self.router.merge(routes.into_router());
        self
    }

    /// Set the bind address (default `127.0.0.1`).
    #[must_use]
    pub fn bind(mut self, addr: impl Into<String>) -> Self {
        self.bind_addr = addr.into();
        self
    }

    /// Set the port (default `3000`). Also honored via `APP_PORT`/`PORT` env
    /// at run time.
    #[must_use]
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
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

    /// Build the application. For the stateless app (`S = ()`) this is the
    /// final step before `run()`. For a stateful app, the state is produced
    /// at run time from the started resources.
    #[must_use]
    pub fn build(self) -> Application<S> {
        Application {
            // The router-level pipeline is composed here, once, so that
            // `serve` and `run_with_state` cannot disagree about it.
            router: self.pipeline.apply(self.router),
            bind_addr: self.bind_addr,
            port: self.port,
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
        let lifecycle = Lifecycle::new();
        let _resources = Resources::empty();
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
    #[cfg_attr(
        not(any(
            feature = "database",
            feature = "jobs",
            feature = "cache",
            feature = "storage-fs",
            feature = "mail"
        )),
        expect(
            unused_mut,
            reason = "with no subsystem to start, neither `self` nor `resources` is mutated"
        )
    )]
    pub async fn run_with_state(mut self, state_fn: StateFn<S>) -> EngineResult<()> {
        use tokio::net::TcpListener;

        let port = resolve_port(self.port);
        let addr: SocketAddr = format!("{}:{}", self.bind_addr, port)
            .parse()
            .map_err(|_| EngineError::InvalidPort(port))?;

        let listener =
            TcpListener::bind(addr)
                .await
                .map_err(|source| EngineError::BindListener {
                    address: addr.to_string(),
                    source,
                })?;

        let lifecycle = Lifecycle::new();

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
        use crate::axum::ServiceExt as _;

        lifecycle.mark_ready();

        axum::serve(listener, service.into_make_service())
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|source| EngineError::Serve { source })?;

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

#[cfg(feature = "macros")]
fn resolve_port(configured: u16) -> u16 {
    // Allow `PORT` and `ARCATURE_BACKEND_PORT` to override the configured port.
    use std::env;
    for key in ["PORT", "ARCATURE_BACKEND_PORT"] {
        if let Ok(v) = env::var(key)
            && let Ok(p) = v.parse::<u16>()
        {
            return p;
        }
    }
    configured
}
