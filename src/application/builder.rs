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
use crate::application::resources::Resources;
use crate::application::{EngineError, EngineResult};
use crate::routing::{RouterState, Routes};
use axum::Router;
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
    // The pre-routing proxy function (engine spec §4/§5). `None` → no proxy;
    // the request goes straight to the router.
    proxy: Option<crate::proxy::ProxyFn>,
    // The Vite IPC endpoint for the one-port dev proxy (AP2.1-3). `None` →
    // pass-through. Only meaningful with the `dev-proxy` feature.
    #[cfg(feature = "dev-proxy")]
    dev_proxy_endpoint: Option<crate::dev_proxy::endpoint::IpcEndpoint>,
}

/// The state closure: given the started [`Resources`] and the [`Lifecycle`],
/// produce the application state `S`.
pub type StateFn<S> = Arc<dyn Fn(&Resources, &Lifecycle) -> S + Send + Sync>;

impl<S: RouterState> Application<S> {
    /// Begin building an application.
    #[must_use]
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
            #[cfg(feature = "dev-proxy")]
            dev_proxy_endpoint: None,
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
    /// Only available with the `dev-proxy` feature. The endpoint is
    /// normally resolved from `ARCATURE_VITE_IPC` (set by `arc dev`); this
    /// method lets a caller override it explicitly (e.g. tests).
    #[cfg(feature = "dev-proxy")]
    #[must_use]
    pub fn dev_proxy_endpoint(mut self, endpoint: Option<std::path::PathBuf>) -> Self {
        self.dev_proxy_endpoint =
            endpoint.map(crate::dev_proxy::endpoint::IpcEndpoint::new);
        self
    }

    /// Build the application. For the stateless app (`S = ()`) this is the
    /// final step before `run()`. For a stateful app, the state is produced
    /// at run time from the started resources.
    #[must_use]
    pub fn build(self) -> Application<S> {
        Application {
            router: self.router,
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
        use crate::axum::ServiceExt as _;
        use tower::Layer as _;
        let service = self.router.into_service();
        let service = crate::proxy::ProxyLayer::new(self.proxy.clone()).layer(service);
        #[cfg(feature = "dev-proxy")]
        let service = {
            let endpoint = self.dev_proxy_endpoint.clone();
            crate::dev_proxy::DevProxyLayer::new(endpoint).layer(service)
        };
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

        // Compose the request pipeline. The pre-routing `ProxyLayer` wraps the
        // router-as-service so the application's proxy function runs *before*
        // route selection; the `DevProxyLayer` is the outermost layer, forwarding
        // Vite requests to the IPC endpoint when `arc dev` set one (AP2.1-3).
        // When neither is configured both layers are zero-overhead pass-throughs.
        use crate::axum::ServiceExt as _;
        use tower::Layer as _;
        let service = router.into_service();
        let service = crate::proxy::ProxyLayer::new(self.proxy.clone()).layer(service);
        #[cfg(feature = "dev-proxy")]
        let service = {
            let endpoint = self.dev_proxy_endpoint.clone();
            crate::dev_proxy::DevProxyLayer::new(endpoint).layer(service)
        };

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
        if let Ok(v) = env::var(key) {
            if let Ok(p) = v.parse::<u16>() {
                return p;
            }
        }
    }
    configured
}
