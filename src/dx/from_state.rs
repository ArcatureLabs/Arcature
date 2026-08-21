//! `*FromState<S>` -- the seams between application state and the subsystem
//! handles Arcature owns.
//!
//! Each trait answers one question for one resource: *given this
//! application's state type, where is the handle?* The
//! [`Resolve<S>`](super::resolve::Resolve) impls in
//! [`resolve`](super::resolve) delegate to these, so
//! [`Inject<Cache>`](super::Inject) works the moment an application says
//! where its cache lives.
//!
//! # Why a trait per resource, and not one blanket rule
//!
//! A single `FromState<S>` trait with a blanket
//! `impl<S, T: FromState<S>> Resolve<S> for T` would read better and would
//! also break the two things that matter most: the `#[service]` macro's
//! generated `impl Resolve<S> for MyService`, and the documented one-line
//! escape hatch `impl Resolve<AppState> for StripeClient`. Both would
//! overlap the blanket impl as far as coherence is concerned -- rustc cannot
//! know a downstream type will *not* also implement `FromState`. So the
//! seams stay separate and explicit, which is the same reason
//! [`DbFromState`](super::db_from_state::DbFromState) is its own trait.
//!
//! # What an application writes
//!
//! For the common composite state, one line per resource it wants injected:
//!
//! ```ignore
//! #[derive(Clone)]
//! struct AppState { db: Db, cache: Cache }
//!
//! impl CacheFromState<AppState> for Cache {
//!     fn cache_from_state(state: &AppState) -> Cache { state.cache.clone() }
//! }
//! ```
//!
//! When the state *is* the handle (a single-resource application, or a
//! test), the identity impl below already covers it.

/// How to obtain a [`Cache`](crate::cache::Cache) handle from state `S`.
#[cfg(feature = "cache")]
pub trait CacheFromState<S>: Send + Sync + 'static {
    /// Extract a `Cache` handle from the state.
    fn cache_from_state(state: &S) -> crate::cache::Cache;
}

/// The simplest case: the state IS the `Cache`.
#[cfg(feature = "cache")]
impl CacheFromState<crate::cache::Cache> for crate::cache::Cache {
    fn cache_from_state(state: &crate::cache::Cache) -> crate::cache::Cache {
        state.clone()
    }
}

/// How to obtain a [`Storage`](crate::storage::Storage) handle from state
/// `S`.
#[cfg(feature = "storage-fs")]
pub trait StorageFromState<S>: Send + Sync + 'static {
    /// Extract a `Storage` handle from the state.
    fn storage_from_state(state: &S) -> crate::storage::Storage;
}

/// The simplest case: the state IS the `Storage`.
#[cfg(feature = "storage-fs")]
impl StorageFromState<crate::storage::Storage> for crate::storage::Storage {
    fn storage_from_state(state: &crate::storage::Storage) -> crate::storage::Storage {
        state.clone()
    }
}

/// How to obtain a [`Mail`](crate::mail::Mail) handle from state `S`.
#[cfg(feature = "mail")]
pub trait MailFromState<S>: Send + Sync + 'static {
    /// Extract a `Mail` handle from the state.
    fn mail_from_state(state: &S) -> crate::mail::Mail;
}

/// The simplest case: the state IS the `Mail` facade.
#[cfg(feature = "mail")]
impl MailFromState<crate::mail::Mail> for crate::mail::Mail {
    fn mail_from_state(state: &crate::mail::Mail) -> crate::mail::Mail {
        state.clone()
    }
}

/// How to obtain a [`Jobs`](crate::jobs::Jobs) handle from state `S`.
#[cfg(feature = "jobs")]
pub trait JobsFromState<S>: Send + Sync + 'static {
    /// Extract a `Jobs` handle from the state.
    fn jobs_from_state(state: &S) -> crate::jobs::Jobs;
}

/// The simplest case: the state IS the `Jobs` facade.
#[cfg(feature = "jobs")]
impl JobsFromState<crate::jobs::Jobs> for crate::jobs::Jobs {
    fn jobs_from_state(state: &crate::jobs::Jobs) -> crate::jobs::Jobs {
        state.clone()
    }
}

/// How to obtain an event [`Dispatcher`](crate::events::Dispatcher) from
/// state `S`.
#[cfg(feature = "events")]
pub trait EventsFromState<S>: Send + Sync + 'static {
    /// Extract a `Dispatcher` from the state.
    fn events_from_state(state: &S) -> crate::events::Dispatcher;
}

/// The simplest case: the state IS the `Dispatcher`.
#[cfg(feature = "events")]
impl EventsFromState<crate::events::Dispatcher> for crate::events::Dispatcher {
    fn events_from_state(state: &crate::events::Dispatcher) -> crate::events::Dispatcher {
        state.clone()
    }
}
