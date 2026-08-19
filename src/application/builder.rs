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
    #[cfg(feature = "cache")]
    cache_config: Option<crate::cache::CacheConfig>,
    #[cfg(feature = "storage-fs")]
    storage_config: Option<crate::storage::StorageConfig>,
    #[cfg(feature = "mail")]
    mail_config: Option<crate::mail::SmtpConfig>,
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
            #[cfg(feature = "cache")]
            cache_config: None,
            #[cfg(feature = "storage-fs")]
            storage_config: None,
            #[cfg(feature = "mail")]
            mail_config: None,
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
    #[cfg(feature = "cache")]
    cache_config: Option<crate::cache::CacheConfig>,
    #[cfg(feature = "storage-fs")]
    storage_config: Option<crate::storage::StorageConfig>,
    #[cfg(feature = "mail")]
    mail_config: Option<crate::mail::SmtpConfig>,
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
            #[cfg(feature = "cache")]
            cache_config: self.cache_config,
            #[cfg(feature = "storage-fs")]
            storage_config: self.storage_config,
            #[cfg(feature = "mail")]
            mail_config: self.mail_config,
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
        axum::serve(listener, self.router)
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
    /// (database, cache, storage, mail), builds the state from the started
    /// resources, applies it to the router, marks readiness, serves with
    /// graceful shutdown, and tears subsystems down in reverse.
    ///
    /// Requires the `macros` feature.
    #[cfg(feature = "macros")]
    pub async fn run_with_state(self, state_fn: StateFn<S>) -> EngineResult<()> {
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

        lifecycle.mark_ready();

        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|source| EngineError::Serve { source })?;

        lifecycle.begin_drain();

        // Reverse-order shutdown: mail → storage → cache → database.
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
