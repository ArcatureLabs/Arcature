//! Application configuration.
//!
//! Config is Rust, not a DSL. Each subsystem owns a typed config struct built
//! with constructors and builder methods. Secrets live in the environment
//! (`.env` for local dev), never in `arcature.toml`.
//!
//! ```ignore
//! pub fn app() -> AppConfig {
//!     AppConfig::new()
//!         .name(env_or("APP_NAME", "Arcature"))
//!         .url(env_or("APP_URL", "http://localhost:3000"))
//! }
//! ```

use std::env;

/// The environment variable `arc dev` sets to the IPC path Vite's
/// `middlewareMode` server listens on.
///
/// It lives here rather than in [`crate::dev_proxy`] because two unrelated
/// subsystems consult it and only one of them is feature-gated: the dev proxy
/// decides whether to forward, and [`crate::assets`] decides whether entries
/// resolve to source paths or to hashed build output. Two spellings of the
/// same name would let those two disagree.
pub const VITE_IPC_ENV: &str = "ARCATURE_VITE_IPC";

/// The environment variable `arc dev` sets to the IPC path the application
/// itself must listen on.
///
/// It is the mirror image of [`VITE_IPC_ENV`]. Under `cargo run --features
/// dev` the application owns the TCP port and forwards Vite's requests to
/// Vite; under `arc dev` the supervisor owns the only TCP port and the
/// application is a child process listening here instead. Unset means the
/// application binds its configured TCP address, which is what production
/// does.
pub const APP_IPC_ENV: &str = "ARCATURE_APP_IPC";

/// Read an environment variable, returning the default when unset or empty.
#[must_use]
pub fn env_or(key: &str, default: &str) -> String {
    env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Read a required environment variable. Returns a [`crate::Error::Config`]
/// when unset or empty.
pub fn env_required(key: &str) -> crate::Result<String> {
    env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| crate::Error::Config(format!("{key} is required but not set")))
}

/// Read an environment variable as a typed value via `FromStr`.
pub fn env_parsed<T>(key: &str, default: T) -> T
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Top-level application configuration.
///
/// Hand this to [`ApplicationBuilder::config`](crate::application::ApplicationBuilder::config)
/// and the framework reads it; build it yourself, or take the four
/// conventional environment variables with [`AppConfig::from_env`].
///
/// The fields are public *and* there are same-named builder methods. That is
/// deliberate rather than an oversight: `config.url` reads the value and
/// `config.url("..")` sets it, and Rust resolves the two by whether a call
/// follows. Reading through a `*_value()` getter purely to dodge the name
/// collision was the worse trade.
///
/// # What each field is allowed to do
///
/// [`AppConfig::env`] gates **nothing**, and no other environment variable is
/// allowed to either. Every protection that could plausibly key off a
/// deployment environment keys off something an operator cannot reach:
///
/// - Release redaction of 5xx messages
///   ([`ErrorMapping`](crate::http::error_mapping::ErrorMapping)) and the
///   dev-only UAG endpoint
///   ([`UagEndpoint`](crate::application::uag_endpoint::UagEndpoint)) key off
///   `cfg!(debug_assertions)`, decided when the binary is built.
/// - The security headers and `Strict-Transport-Security`
///   ([`SecurityHeaders::with_hsts`](crate::http::security::SecurityHeaders::with_hsts))
///   are explicit builder opt-ins, so they are decided by code in `main`.
///
/// `APP_ENV` is a string an operator can set. A build whose protections could
/// be switched off by an environment variable has protections in name only --
/// anyone who could reach the process environment would be able to downgrade
/// a production binary to development behaviour without redeploying it. This
/// field is for what an application wants to *display* and *log*.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// The human-readable application name (`APP_NAME`).
    pub name: String,
    /// The externally reachable base URL, no trailing slash (`APP_URL`).
    ///
    /// This is the one thing the framework cannot infer from a request:
    /// an absolute link in outgoing mail, or an OAuth `redirect_uri`, is
    /// built while no request is in scope, and behind a reverse proxy the
    /// `Host` header is not authoritative anyway.
    pub url: String,
    /// The deployment environment (`APP_ENV`). Display and logging only --
    /// see the type documentation.
    pub env: AppEnvironment,
    /// The port to listen on (`APP_PORT`). Overridable at run time; see
    /// [`ApplicationBuilder::config`](crate::application::ApplicationBuilder::config)
    /// for the full precedence.
    pub port: u16,
}

/// The deployment environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEnvironment {
    Development,
    Production,
    Test,
}

impl AppEnvironment {
    /// Parse from a string (`development`/`production`/`test`).
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "production" | "prod" => AppEnvironment::Production,
            "test" => AppEnvironment::Test,
            _ => AppEnvironment::Development,
        }
    }

    /// Whether this is the production environment.
    #[must_use]
    pub fn is_production(self) -> bool {
        matches!(self, AppEnvironment::Production)
    }

    /// Whether this is the development environment.
    #[must_use]
    pub fn is_development(self) -> bool {
        matches!(self, AppEnvironment::Development)
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl AppConfig {
    /// A new config with development defaults.
    #[must_use]
    pub fn new() -> Self {
        AppConfig {
            name: "Arcature".to_string(),
            url: "http://localhost:3000".to_string(),
            env: AppEnvironment::Development,
            port: 3000,
        }
    }

    /// Set the application name.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the application URL.
    #[must_use]
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Set the environment.
    #[must_use]
    pub fn environment(mut self, env: AppEnvironment) -> Self {
        self.env = env;
        self
    }

    /// Set the bind port.
    #[must_use]
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Load the config from the environment (the canonical generated-app path).
    /// Reads `APP_NAME`, `APP_URL`, `APP_ENV`, `APP_PORT`.
    #[must_use]
    pub fn from_env() -> Self {
        AppConfig::new()
            .name(env_or("APP_NAME", "Arcature"))
            .url(env_or("APP_URL", "http://localhost:3000"))
            .environment(AppEnvironment::parse(&env_or("APP_ENV", "development")))
            .port(env_parsed("APP_PORT", 3000))
    }
}
