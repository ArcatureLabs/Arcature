//! The Arcature cache handle: one multiplexed Valkey/Redis connection with
//! typed operations, plus the `Cache::remember` cache-aside facade.
//!
//! `Cache` holds exactly one [`redis::aio::MultiplexedConnection`] -- a cheap
//! to clone, thread-safe connection that multiplexes commands over a single
//! TCP socket. There is no connection pool and no second pool; the
//! multiplexed connection is the recommended redis-rs model for async use.
//!
//! `Cache` is `Clone + Send + Sync + 'static` so it works as normal Axum
//! state.
//!
//! # Failure semantics -- miss is not an error
//!
//! A cache miss (key absent) returns `Ok(None)` from [`Cache::get`] -- it is
//! **not** an error. A backend failure (connection lost, server down,
//! timeout) returns [`CacheError::Backend`]. A cache outage never silently
//! becomes a cache miss; callers who want fail-open semantics must handle
//! `Backend` explicitly.

use std::future::Future;
use std::time::Duration;

use redis::AsyncCommands;

use crate::cache::config::CacheConfig;
use crate::cache::error::{CacheConnectError, CacheError, CacheHealthError};
use crate::cache::namespace::Namespace;

/// The Arcature cache handle: one multiplexed Valkey/Redis connection with
/// typed operations.
#[derive(Clone)]
pub struct Cache {
    pub(crate) connection: redis::aio::MultiplexedConnection,
    pub(crate) response_timeout: Duration,
    pub(crate) max_payload_size: Option<usize>,
    pub(crate) namespace: Namespace,
}

impl Cache {
    pub(crate) fn from_parts(
        connection: redis::aio::MultiplexedConnection,
        response_timeout: Duration,
        max_payload_size: Option<usize>,
        namespace: Namespace,
    ) -> Self {
        Self {
            connection,
            response_timeout,
            max_payload_size,
            namespace,
        }
    }

    /// Clone the multiplexed connection for a single operation and apply the
    /// configured response timeout.
    pub(crate) fn connection_for_op(&self) -> redis::aio::MultiplexedConnection {
        let mut conn = self.connection.clone();
        conn.set_response_timeout(self.response_timeout);
        conn
    }

    /// Resolve a caller key through the namespace prefix.
    pub(crate) fn resolve_key(&self, key: &str) -> String {
        self.namespace.resolve(key)
    }

    /// Get a clone of the underlying [`redis::aio::MultiplexedConnection`]
    /// for direct use of the full redis-rs API.
    ///
    /// Keys passed directly to the returned connection are **not** namespaced
    /// -- the caller is responsible for prefixing if they want namespace
    /// isolation.
    #[must_use]
    pub fn connection(&self) -> redis::aio::MultiplexedConnection {
        self.connection.clone()
    }
}

impl Cache {
    /// Connect to Valkey/Redis using resolved configuration.
    ///
    /// Validates the configuration (zero timeout, zero payload limit) before
    /// any expensive async work runs, then opens one multiplexed async
    /// connection.
    ///
    /// # Errors
    ///
    /// Returns [`CacheConnectError::Config`] if the configuration is invalid,
    /// or [`CacheConnectError::Backend`] if the connection cannot be
    /// established.
    pub async fn connect(config: CacheConfig) -> Result<Cache, CacheConnectError> {
        config.validate().map_err(CacheConnectError::config)?;
        let connection = config
            .client()
            .get_multiplexed_async_connection()
            .await
            .map_err(CacheConnectError::backend)?;
        Ok(Cache::from_parts(
            connection,
            config.response_timeout_setting(),
            config.max_payload_size_setting(),
            config.namespace_setting().clone(),
        ))
    }

    /// Construct a `Cache` from an existing `MultiplexedConnection`. This is
    /// the escape hatch: an expert can build and configure the connection
    /// themselves, then hand it to Arcature.
    #[must_use]
    pub fn from_connection(
        config: CacheConfig,
        connection: redis::aio::MultiplexedConnection,
    ) -> Cache {
        Cache::from_parts(
            connection,
            config.response_timeout_setting(),
            config.max_payload_size_setting(),
            config.namespace_setting().clone(),
        )
    }

    /// Check backend liveness by executing `PING`.
    ///
    /// # Errors
    ///
    /// Returns [`CacheHealthError`] if the ping fails.
    pub async fn ping(&self) -> Result<(), CacheHealthError> {
        let mut conn = self.connection_for_op();
        redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map(|_| ())
            .map_err(CacheHealthError::new)
    }

    /// Drop this cache handle. The underlying connection closes when the
    /// **last** clone is dropped (Arc refcounting), not when this method is
    /// called.
    pub async fn close(&self) {
        // No explicit close: the connection closes when the last clone is
        // dropped. This method is a no-op kept for API symmetry with `Db`.
    }
}

// --- Typed operations -------------------------------------------------------

impl Cache {
    /// Get the raw bytes of a key.
    ///
    /// Returns `Ok(None)` when the key is absent -- a cache miss is **not** an
    /// error. Returns [`CacheError::Backend`] if the backend fails.
    ///
    /// # Errors
    ///
    /// * [`CacheError::Backend`] -- the backend returned an error.
    /// * [`CacheError::PayloadTooLarge`] -- the value exceeded the configured
    ///   size limit.
    pub async fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let full_key = self.resolve_key(key);
        let mut conn = self.connection_for_op();
        let value: Option<Vec<u8>> = conn.get(&full_key).await.map_err(CacheError::backend)?;
        if let Some(ref bytes) = value {
            check_payload_size(bytes.len(), self.max_payload_size)?;
        }
        Ok(value)
    }

    /// Get a key and deserialize it as `T`.
    ///
    /// Returns `Ok(None)` when the key is absent. A decode failure (corrupt or
    /// schema-mismatched JSON) returns [`CacheError::Decode`], not `Ok(None)`.
    ///
    /// # Errors
    ///
    /// * [`CacheError::Backend`] -- the backend returned an error.
    /// * [`CacheError::PayloadTooLarge`] -- the value exceeded the configured
    ///   size limit.
    /// * [`CacheError::Decode`] -- the stored value is not valid JSON for `T`.
    pub async fn get<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<T>, CacheError> {
        let full_key = self.resolve_key(key);
        let mut conn = self.connection_for_op();
        let value: Option<Vec<u8>> = conn.get(&full_key).await.map_err(CacheError::backend)?;
        match value {
            None => Ok(None),
            Some(bytes) => {
                check_payload_size(bytes.len(), self.max_payload_size)?;
                let parsed = serde_json::from_slice(&bytes)
                    .map_err(|source| CacheError::Decode { source })?;
                Ok(Some(parsed))
            }
        }
    }

    /// Set a key to the given raw bytes with no expiry.
    ///
    /// # Errors
    ///
    /// * [`CacheError::PayloadTooLarge`] -- the value exceeds the configured
    ///   size limit.
    /// * [`CacheError::Backend`] -- the backend returned an error.
    pub async fn set_bytes(&self, key: &str, value: Vec<u8>) -> Result<(), CacheError> {
        check_payload_size(value.len(), self.max_payload_size)?;
        let full_key = self.resolve_key(key);
        let mut conn = self.connection_for_op();
        conn.set::<_, _, ()>(full_key, value)
            .await
            .map_err(CacheError::backend)
    }

    /// Set a key to the JSON serialization of `value` with no expiry.
    ///
    /// # Errors
    ///
    /// * [`CacheError::Decode`] -- `value` could not be serialized as JSON.
    /// * [`CacheError::PayloadTooLarge`] -- the serialized JSON exceeds the
    ///   configured size limit.
    /// * [`CacheError::Backend`] -- the backend returned an error.
    pub async fn set<T: serde::Serialize>(&self, key: &str, value: &T) -> Result<(), CacheError> {
        let bytes = serde_json::to_vec(value).map_err(|source| CacheError::Decode { source })?;
        self.set_bytes(key, bytes).await
    }

    /// Set a key to the JSON serialization of `value` with a time-to-live.
    ///
    /// # Errors
    ///
    /// * [`CacheError::ZeroTtl`] -- `ttl` is zero.
    /// * [`CacheError::Decode`] -- `value` could not be serialized as JSON.
    /// * [`CacheError::PayloadTooLarge`] -- the serialized JSON exceeds the
    ///   configured size limit.
    /// * [`CacheError::Backend`] -- the backend returned an error.
    pub async fn put<T: serde::Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl: Duration,
    ) -> Result<(), CacheError> {
        if ttl.is_zero() {
            return Err(CacheError::ZeroTtl);
        }
        let bytes = serde_json::to_vec(value).map_err(|source| CacheError::Decode { source })?;
        check_payload_size(bytes.len(), self.max_payload_size)?;
        let full_key = self.resolve_key(key);
        let seconds = ttl.as_secs();
        let mut conn = self.connection_for_op();
        conn.set_ex::<_, _, ()>(full_key, bytes, seconds)
            .await
            .map_err(CacheError::backend)
    }

    /// Set a key to the given raw bytes with a time-to-live.
    ///
    /// # Errors
    ///
    /// * [`CacheError::ZeroTtl`] -- `ttl` is zero.
    /// * [`CacheError::PayloadTooLarge`] -- the value exceeds the configured
    ///   size limit.
    /// * [`CacheError::Backend`] -- the backend returned an error.
    pub async fn set_bytes_with_ttl(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl: Duration,
    ) -> Result<(), CacheError> {
        if ttl.is_zero() {
            return Err(CacheError::ZeroTtl);
        }
        check_payload_size(value.len(), self.max_payload_size)?;
        let full_key = self.resolve_key(key);
        let seconds = ttl.as_secs();
        let mut conn = self.connection_for_op();
        conn.set_ex::<_, _, ()>(full_key, value, seconds)
            .await
            .map_err(CacheError::backend)
    }

    /// Get a value if it exists, otherwise compute it via `loader`, store it
    /// with the given `ttl`, and return it. This is the cache-aside /
    /// lazy-loading facade.
    ///
    /// A cache miss (key absent) is **not** an error: the loader runs and the
    /// result is cached. A backend failure propagates as
    /// [`CacheError::Backend`] -- the loader is not run on a backend error,
    /// because the framework does not silently decide fail-open semantics for
    /// the caller. Callers who want fail-open (swallow `Backend` and run the
    /// loader) must handle `Backend` explicitly before calling `remember`.
    ///
    /// # Errors
    ///
    /// Propagates [`CacheError`] from the get or put path, or any error from
    /// the loader.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use arcature::cache::{Cache, CacheError};
    ///
    /// #[derive(serde::Serialize, serde::Deserialize)]
    /// struct User {
    ///     id: u64,
    /// }
    ///
    /// async fn find(id: u64) -> Result<User, CacheError> {
    ///     Ok(User { id })
    /// }
    ///
    /// async fn cached_user(cache: &Cache) -> Result<User, CacheError> {
    ///     // The loader runs only on a miss -- and never on a backend error.
    ///     cache
    ///         .remember("user:42", Duration::from_secs(300), || async { find(42).await })
    ///         .await
    /// }
    /// ```
    pub async fn remember<T, F, Fut, E>(
        &self,
        key: &str,
        ttl: Duration,
        loader: F,
    ) -> Result<T, CacheError>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: Into<CacheError>,
    {
        match self.get::<T>(key).await? {
            Some(value) => Ok(value),
            None => {
                let value = loader().await.map_err(E::into)?;
                self.put(key, &value, ttl).await?;
                Ok(value)
            }
        }
    }

    /// Delete a key. Returns the number of keys removed (`0` if absent, `1`
    /// if it existed).
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::Backend`] if the backend fails.
    pub async fn forget(&self, key: &str) -> Result<usize, CacheError> {
        let full_key = self.resolve_key(key);
        let mut conn = self.connection_for_op();
        conn.del::<_, usize>(full_key)
            .await
            .map_err(CacheError::backend)
    }

    /// Check whether a key exists.
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::Backend`] if the backend fails.
    pub async fn exists(&self, key: &str) -> Result<bool, CacheError> {
        let full_key = self.resolve_key(key);
        let mut conn = self.connection_for_op();
        let count: usize = conn
            .exists::<_, usize>(full_key)
            .await
            .map_err(CacheError::backend)?;
        Ok(count > 0)
    }

    /// Atomically increment the integer value of a key by `delta`.
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::Backend`] if the backend fails.
    pub async fn incr(&self, key: &str, delta: i64) -> Result<i64, CacheError> {
        let full_key = self.resolve_key(key);
        let mut conn = self.connection_for_op();
        conn.incr::<_, _, i64>(full_key, delta)
            .await
            .map_err(CacheError::backend)
    }

    /// Atomically decrement the integer value of a key by `delta`.
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::Backend`] if the backend fails.
    pub async fn decr(&self, key: &str, delta: i64) -> Result<i64, CacheError> {
        let full_key = self.resolve_key(key);
        let mut conn = self.connection_for_op();
        conn.decr::<_, _, i64>(full_key, delta)
            .await
            .map_err(CacheError::backend)
    }

    /// Set a time-to-live on an existing key. Returns `true` if the timeout
    /// was set, `false` if the key does not exist.
    ///
    /// # Errors
    ///
    /// * [`CacheError::ZeroTtl`] -- `ttl` is zero.
    /// * [`CacheError::Backend`] -- the backend fails.
    pub async fn expire(&self, key: &str, ttl: Duration) -> Result<bool, CacheError> {
        if ttl.is_zero() {
            return Err(CacheError::ZeroTtl);
        }
        let full_key = self.resolve_key(key);
        let seconds: i64 = ttl.as_secs() as i64;
        let mut conn = self.connection_for_op();
        let set: bool = conn
            .expire::<_, bool>(full_key, seconds)
            .await
            .map_err(CacheError::backend)?;
        Ok(set)
    }

    /// Get the remaining time-to-live of a key, in seconds.
    ///
    /// Returns `Ok(None)` if the key has no TTL (persists), `Ok(Some(0))` if
    /// the key does not exist, or `Ok(Some(secs))`.
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::Backend`] if the backend fails.
    pub async fn ttl(&self, key: &str) -> Result<Option<u64>, CacheError> {
        let full_key = self.resolve_key(key);
        let mut conn = self.connection_for_op();
        let raw: i64 = conn
            .ttl::<_, i64>(full_key)
            .await
            .map_err(CacheError::backend)?;
        match raw {
            -1 => Ok(None),
            -2 => Ok(Some(0)),
            secs if secs >= 0 => Ok(Some(secs as u64)),
            _ => Ok(Some(0)),
        }
    }

    /// Compare-and-set: set `key` to `value` only if it does not already
    /// exist. Returns `true` if the key was set.
    ///
    /// # Errors
    ///
    /// * [`CacheError::PayloadTooLarge`] -- the value exceeds the configured
    ///   size limit.
    /// * [`CacheError::Backend`] -- the backend fails.
    pub async fn set_if_absent(&self, key: &str, value: Vec<u8>) -> Result<bool, CacheError> {
        check_payload_size(value.len(), self.max_payload_size)?;
        let full_key = self.resolve_key(key);
        let mut conn = self.connection_for_op();
        let result: Option<String> = redis::cmd("SET")
            .arg(&full_key)
            .arg(value)
            .arg("NX")
            .query_async(&mut conn)
            .await
            .map_err(CacheError::backend)?;
        Ok(result.is_some())
    }

    /// Compare-and-set with TTL: set `key` to `value` only if it does not
    /// already exist, with a time-to-live. The atomic lease/lock primitive.
    ///
    /// # Errors
    ///
    /// * [`CacheError::ZeroTtl`] -- `ttl` is zero.
    /// * [`CacheError::PayloadTooLarge`] -- the value exceeds the configured
    ///   size limit.
    /// * [`CacheError::Backend`] -- the backend fails.
    pub async fn set_if_absent_with_ttl(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl: Duration,
    ) -> Result<bool, CacheError> {
        if ttl.is_zero() {
            return Err(CacheError::ZeroTtl);
        }
        check_payload_size(value.len(), self.max_payload_size)?;
        let full_key = self.resolve_key(key);
        let seconds = ttl.as_secs();
        let mut conn = self.connection_for_op();
        let result: Option<String> = redis::cmd("SET")
            .arg(&full_key)
            .arg(value)
            .arg("NX")
            .arg("EX")
            .arg(seconds)
            .query_async(&mut conn)
            .await
            .map_err(CacheError::backend)?;
        Ok(result.is_some())
    }
}

/// Enforce the configured payload size limit.
pub(crate) fn check_payload_size(size: usize, limit: Option<usize>) -> Result<(), CacheError> {
    if let Some(limit) = limit
        && size > limit
    {
        return Err(CacheError::PayloadTooLarge { size, limit });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_limit_accepts_anything() {
        assert!(check_payload_size(1_000_000_000, None).is_ok());
    }

    #[test]
    fn within_limit_passes() {
        assert!(check_payload_size(100, Some(1024)).is_ok());
    }

    #[test]
    fn exactly_at_limit_passes() {
        assert!(check_payload_size(1024, Some(1024)).is_ok());
    }

    #[test]
    fn over_limit_fails() {
        let result = check_payload_size(1025, Some(1024));
        assert!(matches!(
            result,
            Err(CacheError::PayloadTooLarge {
                size: 1025,
                limit: 1024
            })
        ));
    }

    #[test]
    fn zero_size_passes() {
        assert!(check_payload_size(0, Some(1024)).is_ok());
    }

    /// A live-backend test for the one property `remember` exists to have:
    /// a loader that fails leaves the key absent.
    ///
    /// Ignored because it needs a Valkey or Redis on
    /// `redis://127.0.0.1:6379`. Run it with
    /// `cargo test --lib -- --ignored` against one.
    #[tokio::test]
    #[ignore = "needs a live Redis on 127.0.0.1:6379"]
    async fn a_failing_loader_does_not_poison_the_cache() {
        use crate::cache::Namespace;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let namespace =
            Namespace::new(&format!("remember-test-{}", std::process::id())).expect("a namespace");
        let cache = Cache::connect(
            CacheConfig::new("redis://127.0.0.1:6379")
                .expect("a cache config")
                .namespace(namespace),
        )
        .await
        .expect("a live Redis");

        let ttl = Duration::from_secs(60);
        let calls = AtomicUsize::new(0);

        // A loader that fails must propagate the failure and store nothing.
        let failed: Result<String, CacheError> = cache
            .remember("greeting", ttl, || {
                calls.fetch_add(1, Ordering::Relaxed);
                async { Err(CacheError::ZeroTtl) }
            })
            .await;
        assert!(failed.is_err());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            cache.get::<String>("greeting").await.expect("a get"),
            None,
            "a failed load must not leave anything behind"
        );

        // A loader that succeeds stores its value.
        let loaded: String = cache
            .remember("greeting", ttl, || {
                calls.fetch_add(1, Ordering::Relaxed);
                async { Ok::<_, CacheError>("hello".to_string()) }
            })
            .await
            .expect("a successful load");
        assert_eq!(loaded, "hello");
        assert_eq!(calls.load(Ordering::Relaxed), 2);

        // And a hit does not run the loader at all -- which is why a later
        // failure cannot evict what is already there.
        let cached: String = cache
            .remember("greeting", ttl, || {
                calls.fetch_add(1, Ordering::Relaxed);
                async { Err(CacheError::ZeroTtl) }
            })
            .await
            .expect("a cache hit");
        assert_eq!(cached, "hello");
        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "the loader must not run on a hit"
        );

        cache.forget("greeting").await.expect("cleanup");
    }
}
