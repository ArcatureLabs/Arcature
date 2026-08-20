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
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) env: AppEnvironment,
    pub(crate) port: u16,
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

    // Accessors named to avoid clashing with the fluent builder methods
    // above.
    //
    // Nothing reads these yet: `AppConfig` parses `APP_NAME`, `APP_URL`,
    // `APP_ENV` and `APP_PORT` and the application then ignores all four.
    // They are the inputs the security headers, the HSTS decision and the
    // release-mode error redaction all need, so they are kept rather than
    // deleted.
    #[expect(dead_code, reason = "consumed once AppConfig reaches the builder")]
    #[must_use]
    pub(crate) fn name_value(&self) -> &str {
        &self.name
    }

    #[expect(dead_code, reason = "consumed once AppConfig reaches the builder")]
    #[must_use]
    pub(crate) fn url_value(&self) -> &str {
        &self.url
    }

    #[expect(dead_code, reason = "consumed once AppConfig reaches the builder")]
    #[must_use]
    pub(crate) fn environment_value(&self) -> AppEnvironment {
        self.env
    }

    #[expect(dead_code, reason = "consumed once AppConfig reaches the builder")]
    #[must_use]
    pub(crate) fn port_value(&self) -> u16 {
        self.port
    }
}
