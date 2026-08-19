//! Cache subsystem error types.
//!
//! No secret material is ever embedded in any variant. The connection URL
//! may contain a password but it is never stored in the error.

use std::fmt;

/// Operation failure from [`crate::cache::Cache`] methods.
///
/// A **cache miss is not an error**: [`crate::cache::Cache::get`] returns
/// `Ok(None)` when a key is absent. [`CacheError`] is returned only when the
/// backend itself failed, a value could not be decoded, or a payload exceeded
/// the configured size limit.
#[derive(Debug)]
pub enum CacheError {
    /// The redis/Valkey backend returned an error. The upstream error is
    /// preserved for source chaining.
    Backend {
        /// The upstream `redis::RedisError`.
        source: redis::RedisError,
    },
    /// A stored value could not be (de)serialized as the requested type.
    Decode {
        /// The upstream `serde_json::Error`.
        source: serde_json::Error,
    },
    /// A value or TTL exceeded the configured size/range limit.
    PayloadTooLarge {
        /// The offending size in bytes (for values) or seconds (for TTLs).
        size: usize,
        /// The configured limit that was exceeded.
        limit: usize,
    },
    /// A `set_*_with_ttl` call was given a zero TTL.
    ZeroTtl,
}

impl CacheError {
    pub(crate) fn backend(source: redis::RedisError) -> Self {
        Self::Backend { source }
    }
}

impl fmt::Display for CacheError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend { source } => write!(formatter, "cache backend error: {source}"),
            Self::Decode { source } => write!(formatter, "cache value decode error: {source}"),
            Self::PayloadTooLarge { size, limit } => {
                write!(formatter, "payload size {size} exceeds limit {limit}")
            }
            Self::ZeroTtl => write!(formatter, "ttl must not be zero"),
        }
    }
}

impl std::error::Error for CacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend { source } => Some(source),
            Self::Decode { source } => Some(source),
            Self::PayloadTooLarge { .. } | Self::ZeroTtl => None,
        }
    }
}

impl From<redis::RedisError> for CacheError {
    fn from(source: redis::RedisError) -> Self {
        Self::Backend { source }
    }
}

/// Configuration validation failure for [`crate::cache::CacheConfig`] or
/// [`crate::cache::Namespace`].
#[derive(Debug)]
pub enum CacheConfigError {
    /// The connection URL could not be parsed as a Redis/Valkey connect info.
    InvalidUrl { detail: String },
    /// `response_timeout` is zero, which would make every operation time out
    /// instantly.
    ZeroResponseTimeout,
    /// A payload size limit of zero would reject every value.
    ZeroPayloadLimit,
    /// A namespace prefix was empty.
    EmptyNamespace,
    /// A namespace prefix ended with the namespace separator `:`.
    NamespaceEndsWithSeparator,
    /// A namespace prefix contained a control character.
    NamespaceContainsControlChar,
}

impl fmt::Display for CacheConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl { detail } => {
                write!(formatter, "invalid cache connection URL: {detail}")
            }
            Self::ZeroResponseTimeout => write!(formatter, "response_timeout must not be zero"),
            Self::ZeroPayloadLimit => write!(formatter, "max_payload_size must not be zero"),
            Self::EmptyNamespace => write!(formatter, "namespace prefix must not be empty"),
            Self::NamespaceEndsWithSeparator => {
                write!(
                    formatter,
                    "namespace prefix must not end with the separator ':'"
                )
            }
            Self::NamespaceContainsControlChar => {
                write!(
                    formatter,
                    "namespace prefix must not contain control characters"
                )
            }
        }
    }
}

impl std::error::Error for CacheConfigError {}

/// Failure from [`crate::cache::Cache::connect`]: either the configuration was
/// invalid (caught before any expensive async work) or the multiplexed
/// connection could not be established.
#[derive(Debug)]
pub enum CacheConnectError {
    /// Configuration validation failed before any connection attempt.
    Config {
        /// The specific configuration error.
        source: CacheConfigError,
    },
    /// The multiplexed connection could not be established (network, auth,
    /// server unavailable).
    Backend {
        /// The upstream `redis::RedisError`.
        source: redis::RedisError,
    },
}

impl CacheConnectError {
    pub(crate) fn config(source: CacheConfigError) -> Self {
        Self::Config { source }
    }

    pub(crate) fn backend(source: redis::RedisError) -> Self {
        Self::Backend { source }
    }
}

impl fmt::Display for CacheConnectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config { source } => write!(formatter, "cache configuration invalid: {source}"),
            Self::Backend { source } => write!(formatter, "cache connection failed: {source}"),
        }
    }
}

impl std::error::Error for CacheConnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config { source } => Some(source),
            Self::Backend { source } => Some(source),
        }
    }
}

/// Health-check failure from [`crate::cache::Cache::ping`].
#[derive(Debug)]
pub struct CacheHealthError {
    source: redis::RedisError,
}

impl CacheHealthError {
    pub(crate) fn new(source: redis::RedisError) -> Self {
        Self { source }
    }

    /// The underlying redis-rs error.
    #[must_use]
    pub fn source_error(&self) -> &redis::RedisError {
        &self.source
    }
}

impl fmt::Display for CacheHealthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "cache ping failed: {}", self.source)
    }
}

impl std::error::Error for CacheHealthError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
