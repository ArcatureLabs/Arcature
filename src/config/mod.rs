//! Application configuration.
//!
//! Config is Rust, not a DSL. Each subsystem owns a typed config struct built
//! with constructors and builder methods. Secrets live in the environment
//! (`.env` for local dev), never in `arcature.toml`.
//!
//! ```
//! use arcature::prelude::*;
//!
//! pub fn app() -> AppConfig {
//!     AppConfig::new()
//!         .name(env_or("APP_NAME", "Arcature"))
//!         .url(env_or("APP_URL", "http://localhost:1183"))
//! }
//!
//! assert!(app().base_url().starts_with("http"));
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
            url: "http://localhost:1183".to_string(),
            env: AppEnvironment::Development,
            port: 1183,
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

    /// The configured base URL with any trailing slash removed.
    ///
    /// Read [`url`](Self::url) directly when you want back exactly what was
    /// configured. Read this when you are about to concatenate. `APP_URL` is
    /// written by an operator and half of them write `https://example.com/`
    /// while the other half write `https://example.com`; normalising on the
    /// way out rather than on the way in keeps the field equal to what was
    /// set, so a config that is printed or logged still shows the operator
    /// their own string.
    ///
    /// ```
    /// use arcature::config::AppConfig;
    ///
    /// let config = AppConfig::new().url("https://example.com/");
    /// assert_eq!(config.base_url(), "https://example.com");
    /// assert_eq!(config.url, "https://example.com/");
    /// ```
    #[must_use]
    pub fn base_url(&self) -> &str {
        self.url.trim_end_matches('/')
    }

    /// An absolute URL for `path`, rooted at [`base_url`](Self::base_url).
    ///
    /// This is the accessor every part of the framework that needs a link
    /// should reach for. A link built while a request is in scope could in
    /// principle come from the `Host` header, but behind a reverse proxy that
    /// header is not authoritative, and the links that matter most --- a
    /// password-reset URL in an email, an OAuth `redirect_uri`, a signed
    /// expiring URL --- are built with no request in scope at all. `APP_URL`
    /// is the only thing that knows where the application actually answers
    /// from, and this is how to spend it.
    ///
    /// # The result is always under the base
    ///
    /// `path` is joined, never substituted: leading slashes are collapsed, so
    /// a `path` that looks like a URL of its own (`https://evil.example/`) or
    /// is scheme-relative (`//evil.example/`) comes back as a path segment
    /// under the configured host rather than as a different host. That makes
    /// this safe to call with a value that reached the process from outside,
    /// though a caller doing so should still be asking why.
    ///
    /// No percent-encoding is applied, and none is applied by
    /// [`Routes::url_for`](crate::routing::Routes::url_for) either: it
    /// substitutes parameters into the path pattern verbatim. `path` is
    /// therefore expected to be a URL path a caller has already made safe.
    ///
    /// ```
    /// use arcature::config::AppConfig;
    ///
    /// let config = AppConfig::new().url("https://example.com");
    ///
    /// assert_eq!(config.absolute_url("/reset"), "https://example.com/reset");
    /// assert_eq!(config.absolute_url("reset"), "https://example.com/reset");
    /// assert_eq!(config.absolute_url(""), "https://example.com");
    /// // Not an escape hatch to another origin.
    /// assert_eq!(
    ///     config.absolute_url("//evil.example/x"),
    ///     "https://example.com/evil.example/x"
    /// );
    /// ```
    #[must_use]
    pub fn absolute_url(&self, path: &str) -> String {
        let base = self.base_url();
        let path = path.trim_start_matches('/');
        if path.is_empty() {
            return base.to_string();
        }
        format!("{base}/{path}")
    }

    /// Load the config from the environment (the canonical generated-app path).
    /// Reads `APP_NAME`, `APP_URL`, `APP_ENV`, `APP_PORT`.
    #[must_use]
    pub fn from_env() -> Self {
        AppConfig::new()
            .name(env_or("APP_NAME", "Arcature"))
            .url(env_or("APP_URL", "http://localhost:1183"))
            .environment(AppEnvironment::parse(&env_or("APP_ENV", "development")))
            .port(env_parsed("APP_PORT", 1183))
    }
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, AppEnvironment};

    #[test]
    fn base_url_drops_a_trailing_slash_without_changing_the_field() {
        let config = AppConfig::new().url("https://example.com/");
        assert_eq!(config.base_url(), "https://example.com");
        // The field still reads back what the operator wrote.
        assert_eq!(config.url, "https://example.com/");
    }

    #[test]
    fn base_url_drops_every_trailing_slash() {
        let config = AppConfig::new().url("https://example.com///");
        assert_eq!(config.base_url(), "https://example.com");
    }

    #[test]
    fn absolute_url_joins_with_exactly_one_slash() {
        let config = AppConfig::new().url("https://example.com");
        assert_eq!(config.absolute_url("/reset"), "https://example.com/reset");
        assert_eq!(config.absolute_url("reset"), "https://example.com/reset");

        let trailing = AppConfig::new().url("https://example.com/");
        assert_eq!(trailing.absolute_url("/reset"), "https://example.com/reset");
        assert_eq!(trailing.absolute_url("reset"), "https://example.com/reset");
    }

    #[test]
    fn absolute_url_of_the_root_is_the_base() {
        let config = AppConfig::new().url("https://example.com/");
        assert_eq!(config.absolute_url(""), "https://example.com");
        assert_eq!(config.absolute_url("/"), "https://example.com");
    }

    #[test]
    fn absolute_url_keeps_a_subpath_base() {
        // A reverse proxy that mounts the application under a prefix.
        let config = AppConfig::new().url("https://example.com/app/");
        assert_eq!(
            config.absolute_url("/reset"),
            "https://example.com/app/reset"
        );
    }

    // The property that makes this safe to call with a value that arrived
    // from outside: `path` is joined, never substituted, so it cannot move
    // the result to another origin.
    #[test]
    fn absolute_url_cannot_be_redirected_to_another_host() {
        let config = AppConfig::new().url("https://example.com");
        assert_eq!(
            config.absolute_url("//evil.example/x"),
            "https://example.com/evil.example/x"
        );
        assert_eq!(
            config.absolute_url("///evil.example/x"),
            "https://example.com/evil.example/x"
        );
        assert_eq!(
            config.absolute_url("https://evil.example/x"),
            "https://example.com/https://evil.example/x"
        );
    }

    #[test]
    fn the_defaults_are_the_development_ones() {
        let config = AppConfig::new();
        assert_eq!(config.name, "Arcature");
        assert_eq!(config.base_url(), "http://localhost:1183");
        assert_eq!(config.env, AppEnvironment::Development);
        assert_eq!(config.port, 1183);
    }
}
