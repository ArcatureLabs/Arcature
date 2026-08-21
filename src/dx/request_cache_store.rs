//! `RequestCache` -- the per-request memo store `#[request_cache]` resolves
//! through.
//!
//! A memoized resolver needs somewhere to remember what it already computed.
//! The usual answers -- a `thread_local!`, a `task_local!`, a process-wide
//! registry -- all make the store *ambient*: reachable from anywhere, alive
//! for longer than the request, and impossible to reason about when a
//! handler moves work onto another task. Cross-request bleed in a memo store
//! is not a performance bug, it is one user reading another user's profile.
//!
//! So the store is a value, and the only place it lives is the request's own
//! extensions. Getting one means extracting it from the request; there is no
//! other door. Two extractions inside one request hand back clones of the
//! same `Arc`, so they share entries; two requests never can, because
//! neither can name the other's `Parts`.
//!
//! # Why entries are `Arc<dyn Any>`
//!
//! Resolvers return different types, and a per-request map has to hold all
//! of them. Arcature avoids `TypeId`/`Any` in its *global* machinery --
//! there is no service locator, no process-wide `TypeId`-keyed container to
//! look a dependency up in. This map is the opposite of that: it is
//! per-request, private to the request, and its keys are resolver names the
//! macro computed, not types. `Extensions` is already exactly this shape, so
//! putting a second, narrower one inside it adds no capability that was not
//! already reachable.
//!
//! A downcast mismatch is therefore possible only if two resolvers declare
//! the same `name` with different return types. That is treated as a miss
//! and recomputed rather than surfaced as an error: the resolver is the
//! source of truth, and a silently wrong value is the one outcome worth
//! ruling out.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt::{Display, Write as _};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

/// The identity of one memoized computation within a request.
///
/// Built by `#[request_cache]`-generated code from the resolver's declared
/// `name` and the values of its declared key fields.
///
/// Field values are length-prefixed when rendered, so no value can forge a
/// field boundary: a single field holding `"1;b=1:2;"` cannot collide with
/// two fields holding `"1"` and `"2"`. A collision here would hand back
/// another computation's result, which is the failure mode this type exists
/// to prevent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestCacheKey {
    resolver: &'static str,
    fields: String,
}

impl RequestCacheKey {
    /// Start a key for the named resolver.
    #[must_use]
    pub const fn new(resolver: &'static str) -> Self {
        Self {
            resolver,
            fields: String::new(),
        }
    }

    /// Add one key field's name and value.
    #[must_use]
    pub fn field<T>(mut self, name: &str, value: &T) -> Self
    where
        T: Display + ?Sized,
    {
        let rendered = value.to_string();
        // Writing to a `String` cannot fail; the result is discarded rather
        // than unwrapped so building a memo key can never panic a request.
        let _ = write!(self.fields, "{name}={}:{rendered};", rendered.len());
        self
    }

    /// The resolver this key belongs to.
    #[must_use]
    pub const fn resolver(&self) -> &'static str {
        self.resolver
    }
}

impl Display for RequestCacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.resolver, self.fields)
    }
}

/// The memo store for one request.
///
/// Cloning is cheap and shares the entries -- that is the point: every
/// extraction within a request clones the same store. Obtain one by taking
/// `RequestCache` as a handler parameter (it is an Axum extractor) and
/// thread it into the resolvers that need it.
///
/// The store needs no layer to be installed. The extractor puts a fresh
/// store into the request's extensions the first time it is asked for one,
/// so a route that never memoizes anything pays nothing, and an application
/// that wires no middleware still gets correct behavior.
#[derive(Clone, Default)]
pub struct RequestCache {
    entries: Arc<Mutex<BTreeMap<RequestCacheKey, Arc<dyn Any + Send + Sync>>>>,
}

impl RequestCache {
    /// Create an empty store.
    ///
    /// Handlers do not call this -- they extract a `RequestCache`, which
    /// binds it to the request. It is public so a test can drive a resolver
    /// without building an HTTP request.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The value already computed for `key` in this request, if any.
    #[must_use]
    pub fn get<T>(&self, key: &RequestCacheKey) -> Option<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        let entries = self.entries();
        entries.get(key)?.downcast_ref::<T>().cloned()
    }

    /// Record the value computed for `key`, replacing any previous entry.
    pub fn insert<T>(&self, key: &RequestCacheKey, value: T)
    where
        T: Send + Sync + 'static,
    {
        self.entries().insert(key.clone(), Arc::new(value));
    }

    /// The number of memoized entries in this request.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries().len()
    }

    /// Whether nothing has been memoized in this request yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries().is_empty()
    }

    /// The memoized keys, in deterministic order -- what an inspector shows
    /// for "what did this request memoize?".
    #[must_use]
    pub fn keys(&self) -> Vec<RequestCacheKey> {
        self.entries().keys().cloned().collect()
    }

    /// Lock the entries, recovering from a poisoned lock.
    ///
    /// A poisoned mutex means another task panicked while holding it. A
    /// plain map has no invariant a panic can leave half-applied, and
    /// failing the rest of the request over an unrelated panic is a worse
    /// outcome than continuing, so the guard is taken back.
    fn entries(&self) -> MutexGuard<'_, BTreeMap<RequestCacheKey, Arc<dyn Any + Send + Sync>>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl std::fmt::Debug for RequestCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestCache")
            .field("entries", &self.len())
            .finish_non_exhaustive()
    }
}

/// Axum extractor: the memo store belonging to *this* request.
///
/// The first extraction in a request creates the store and parks it in the
/// request's extensions; later extractions -- in a middleware, in another
/// argument position, in a nested extractor -- find it there and clone the
/// handle. Extraction cannot fail, so a resolver never has to handle "no
/// cache configured".
impl<S> FromRequestParts<S> for RequestCache
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(existing) = parts.extensions.get::<Self>() {
            return Ok(existing.clone());
        }
        let cache = Self::new();
        parts.extensions.insert(cache.clone());
        Ok(cache)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(resolver: &'static str, user: u64) -> RequestCacheKey {
        RequestCacheKey::new(resolver).field("user_id", &user)
    }

    fn empty_parts() -> Parts {
        axum::http::Request::builder()
            .body(())
            .expect("a request with no headers is always buildable")
            .into_parts()
            .0
    }

    #[test]
    fn a_value_inserted_under_a_key_is_returned_for_that_key() {
        let cache = RequestCache::new();
        cache.insert(&key("load_profile", 7), "Ada".to_string());
        assert_eq!(
            cache.get::<String>(&key("load_profile", 7)),
            Some("Ada".to_string())
        );
    }

    #[test]
    fn a_different_key_field_value_is_a_different_entry() {
        let cache = RequestCache::new();
        cache.insert(&key("load_profile", 7), "Ada".to_string());
        assert_eq!(cache.get::<String>(&key("load_profile", 8)), None);
    }

    #[test]
    fn a_different_resolver_name_is_a_different_entry() {
        let cache = RequestCache::new();
        cache.insert(&key("load_profile", 7), "Ada".to_string());
        assert_eq!(cache.get::<String>(&key("load_settings", 7)), None);
    }

    #[test]
    fn a_clone_shares_entries_with_the_original() {
        let cache = RequestCache::new();
        let handle = cache.clone();
        handle.insert(&key("load_profile", 1), 42u32);
        assert_eq!(cache.get::<u32>(&key("load_profile", 1)), Some(42));
    }

    #[test]
    fn two_stores_never_share_entries() {
        let one = RequestCache::new();
        let other = RequestCache::new();
        one.insert(&key("load_profile", 1), 42u32);
        assert_eq!(other.get::<u32>(&key("load_profile", 1)), None);
    }

    #[test]
    fn a_type_mismatch_reads_as_a_miss_rather_than_a_wrong_value() {
        let cache = RequestCache::new();
        cache.insert(&key("load_profile", 1), 42u32);
        assert_eq!(cache.get::<String>(&key("load_profile", 1)), None);
    }

    #[test]
    fn a_field_value_cannot_forge_a_second_field() {
        let forged = RequestCacheKey::new("load").field("a", "1;b=1:2;");
        let genuine = RequestCacheKey::new("load").field("a", "1").field("b", "2");
        assert_ne!(forged, genuine);
    }

    #[test]
    fn keys_are_listed_in_deterministic_order() {
        let cache = RequestCache::new();
        cache.insert(&key("zeta", 1), 1u8);
        cache.insert(&key("alpha", 1), 2u8);
        let names: Vec<&str> = cache.keys().iter().map(RequestCacheKey::resolver).collect();
        assert_eq!(names, ["alpha", "zeta"]);
    }

    #[test]
    fn a_key_renders_its_resolver_and_fields() {
        assert_eq!(
            key("load_profile", 7).to_string(),
            "load_profile(user_id=1:7;)"
        );
    }

    #[tokio::test]
    async fn extracting_twice_from_one_request_yields_the_same_store() {
        let mut parts = empty_parts();

        let first = RequestCache::from_request_parts(&mut parts, &())
            .await
            .expect("extraction is infallible");
        first.insert(&key("load_profile", 1), 42u32);

        let second = RequestCache::from_request_parts(&mut parts, &())
            .await
            .expect("extraction is infallible");
        assert_eq!(second.get::<u32>(&key("load_profile", 1)), Some(42));
    }

    #[tokio::test]
    async fn a_second_request_starts_with_an_empty_store() {
        let mut first_parts = empty_parts();
        let first = RequestCache::from_request_parts(&mut first_parts, &())
            .await
            .expect("extraction is infallible");
        first.insert(&key("load_profile", 1), 42u32);

        let mut second_parts = empty_parts();
        let second = RequestCache::from_request_parts(&mut second_parts, &())
            .await
            .expect("extraction is infallible");
        assert!(second.is_empty());
        assert_eq!(second.get::<u32>(&key("load_profile", 1)), None);
    }
}
