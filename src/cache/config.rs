//! Cache configuration with credential redaction.
//!
//! The connection URL may contain a password (`redis://:secret@host/db`). It
//! must never appear in `Debug`, `Display`, error output, or logs.

use std::fmt;
use std::time::Duration;

use crate::cache::error::CacheConfigError;
use crate::cache::namespace::Namespace;

/// Resolved cache configuration.
///
/// Construct with [`CacheConfig::new`], then override settings with the builder
/// methods, then pass to [`crate::cache::Cache::connect`].
///
/// # Credential redaction
///
/// `CacheConfig` implements `Debug` manually. It never exposes the password or
/// the full connection URL. Only non-sensitive connection parameters (host,
/// port, database) and the settings (response timeout, payload limit,
/// namespace) appear in `Debug` output.
#[derive(Clone)]
pub struct CacheConfig {
    /// The redis-rs client, which stores the parsed connection info internally.
    client: redis::Client,
    /// Non-sensitive metadata for redacted `Debug`/`Display`.
    host: String,
    port: u16,
    database: u8,
    response_timeout: Duration,
    max_payload_size: Option<usize>,
    namespace: Namespace,
}

impl CacheConfig {
    /// Parse a Redis/Valkey connection URL into resolved configuration.
    ///
    /// Accepts any URL form that `redis::Client::open` understands:
    /// `redis://host:port/db`, `redis://:pass@host/db`, `rediss://` (TLS),
    /// or `unix://` socket paths.
    ///
    /// # Errors
    ///
    /// Returns [`CacheConfigError::InvalidUrl`] if the string cannot be parsed
    /// as a redis connection info. Cross-field validation (zero timeout,
    /// zero payload limit) runs lazily in [`crate::cache::Cache::connect`].
    pub fn new(url: &str) -> Result<Self, CacheConfigError> {
        let client = redis::Client::open(url).map_err(|error| CacheConfigError::InvalidUrl {
            detail: error.to_string(),
        })?;
        let (host, port, database) = extract_connection_metadata(&client);
        Ok(Self {
            client,
            host,
            port,
            database,
            response_timeout: Duration::from_secs(5),
            max_payload_size: Some(64 * 1024 * 1024),
            namespace: Namespace::none(),
        })
    }

    /// Override the response timeout for cache operations.
    ///
    /// Defaults to 5 seconds. A zero timeout is rejected by
    /// [`Cache::connect`](crate::cache::Cache::connect) before any expensive
    /// async work runs.
    #[must_use]
    pub fn response_timeout(mut self, timeout: Duration) -> Self {
        self.response_timeout = timeout;
        self
    }

    /// Override the maximum payload size for values.
    ///
    /// Defaults to 64 MiB. `None` disables the limit. A zero limit is rejected
    /// during [`Cache::connect`](crate::cache::Cache::connect).
    #[must_use]
    pub fn max_payload_size(mut self, limit: Option<usize>) -> Self {
        self.max_payload_size = limit;
        self
    }

    /// Set the key namespace prefix applied to all keys.
    #[must_use]
    pub fn namespace(mut self, namespace: Namespace) -> Self {
        self.namespace = namespace;
        self
    }

    pub(crate) fn client(&self) -> &redis::Client {
        &self.client
    }

    pub(crate) fn response_timeout_setting(&self) -> Duration {
        self.response_timeout
    }

    pub(crate) fn max_payload_size_setting(&self) -> Option<usize> {
        self.max_payload_size
    }

    pub(crate) fn namespace_setting(&self) -> &Namespace {
        &self.namespace
    }

    /// Validate the full configuration. Called by [`crate::cache::Cache::connect`]
    /// before any expensive async work.
    pub(crate) fn validate(&self) -> Result<(), CacheConfigError> {
        if self.response_timeout.is_zero() {
            return Err(CacheConfigError::ZeroResponseTimeout);
        }
        if let Some(limit) = self.max_payload_size
            && limit == 0
        {
            return Err(CacheConfigError::ZeroPayloadLimit);
        }
        Ok(())
    }
}

impl fmt::Debug for CacheConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CacheConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("response_timeout", &self.response_timeout)
            .field("max_payload_size", &self.max_payload_size)
            .field("namespace", &self.namespace)
            .finish()
    }
}

impl fmt::Display for CacheConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}/{}", self.host, self.port, self.database)
    }
}

/// Extract non-sensitive connection metadata (host, port, database) from a
/// redis client for the redacted `Debug`/`Display` impls.
fn extract_connection_metadata(client: &redis::Client) -> (String, u16, u8) {
    let info = client.get_connection_info();
    let (host, port) = match info.addr() {
        redis::ConnectionAddr::Tcp(host, port) => (host.clone(), *port),
        redis::ConnectionAddr::TcpTls { host, port, .. } => (host.clone(), *port),
        redis::ConnectionAddr::Unix(path) => (path.display().to_string(), 0),
        _ => (String::new(), 0),
    };
    let database = info.redis_settings().db().max(0) as u8;
    (host, port, database)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_url() {
        let config = CacheConfig::new("redis://localhost:6379/0").expect("valid URL");
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 6379);
        assert_eq!(config.database, 0);
    }

    #[test]
    fn parses_url_with_password() {
        let config = CacheConfig::new("redis://:secret@localhost:6379/1").expect("valid URL");
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 6379);
        assert_eq!(config.database, 1);
    }

    #[test]
    fn debug_does_not_expose_credentials() {
        let config = CacheConfig::new("redis://:secret_pass@localhost:6379/0").expect("valid URL");
        let debug_output = format!("{config:?}");
        assert!(
            !debug_output.contains("secret_pass"),
            "debug leaked password: {debug_output}"
        );
        assert!(debug_output.contains("localhost"));
        assert!(debug_output.contains("6379"));
    }

    #[test]
    fn display_does_not_expose_credentials() {
        let config = CacheConfig::new("redis://:secret_pass@localhost:6379/0").expect("valid URL");
        let display_output = format!("{config}");
        assert!(!display_output.contains("secret_pass"));
        assert!(display_output.contains("localhost:6379/0"));
    }

    #[test]
    fn defaults_are_sensible() {
        let config = CacheConfig::new("redis://localhost:6379/0").expect("valid URL");
        assert_eq!(config.response_timeout_setting(), Duration::from_secs(5));
        assert_eq!(config.max_payload_size_setting(), Some(64 * 1024 * 1024));
    }

    #[test]
    fn overrides_apply() {
        let config = CacheConfig::new("redis://localhost:6379/0")
            .expect("valid URL")
            .response_timeout(Duration::from_secs(30))
            .max_payload_size(Some(1024));
        assert_eq!(config.response_timeout_setting(), Duration::from_secs(30));
        assert_eq!(config.max_payload_size_setting(), Some(1024));
    }
}
