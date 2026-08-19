//! Cache subsystem: Valkey/Redis key-value cache over one multiplexed
//! connection, with the `Cache::remember` cache-aside facade.
//!
//! This module owns the integration seam between an Arcature application and
//! a Valkey-compatible (Redis protocol) server: one multiplexed connection,
//! with [redis-rs] as the underlying client.
//!
//! # What this module owns
//!
//! * **One [`redis::aio::MultiplexedConnection`]** per [`Cache`], the
//!   recommended redis-rs async model -- cheap to clone, thread-safe, and
//!   multiplexes commands over a single socket. No connection pool, no second
//!   pool.
//! * **Typed operations:** [`Cache::get`]/[`Cache::set`]/[`Cache::put`] (JSON
//!   values), [`Cache::get_bytes`]/[`Cache::set_bytes`] (raw bytes),
//!   [`Cache::forget`] (delete), [`Cache::exists`], [`Cache::incr`]/
//!   [`Cache::decr`], [`Cache::expire`], [`Cache::ttl`], and the atomic
//!   compare-and-set primitives [`Cache::set_if_absent`] and
//!   [`Cache::set_if_absent_with_ttl`].
//! * **The [`Cache::remember`] cache-aside facade:** get a value if it exists,
//!   otherwise compute it via a loader closure, store it with a TTL, and
//!   return it.
//! * **Deterministic key namespacing** via [`Namespace`].
//! * **Explicit lifecycle:** [`Cache::connect`], [`Cache::from_connection`],
//!   [`Cache::ping`], [`Cache::close`].
//! * **Resolved configuration:** [`CacheConfig`] -- accepted explicitly, no
//!   environment access inside the library, credentials redacted.
//!
//! # What this module does not own
//!
//! It does not reimplement the Redis/RESP protocol, a cache server, a
//! connection pool, TLS, or cryptography. It does not implement a
//! distributed-lock subsystem or a client-side caching layer.
//!
//! # Security note -- credentials are never logged
//!
//! [`CacheConfig`] implements `Debug` manually and never exposes the password
//! or the full connection URL.

pub mod config;
pub mod error;
pub mod namespace;
pub mod store;

pub use config::CacheConfig;
pub use error::{
    CacheConfigError, CacheConnectError, CacheError, CacheHealthError,
};
pub use namespace::Namespace;
pub use store::Cache;

// Re-export the certified redis-rs crate so downstream code targets the
// Arcature-pinned version.
pub use redis;
