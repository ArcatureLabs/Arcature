//! Sessions kept in the application's own database.
//!
//! Without this, an application's only ready-made option is
//! `tower-sessions`' `MemoryStore`, which is a `HashMap` in one process: every
//! deploy logs every user out, and a second replica cannot see the first
//! one's sessions. [`DbSessionStore`] puts them in one table next to the
//! application's own data, so a restart is a restart and a replica is a
//! replica.
//!
//! Behind the `session-store-db` feature, which is off by default.
//!
//! ```ignore
//! use arcature::auth::session_store::DbSessionStore;
//!
//! let store = DbSessionStore::connect_lazy(&config.database);
//! store.migrate().await?;
//! ```
//!
//! # Two decisions worth knowing about
//!
//! **The row key is a digest, not the session id.** A session id is a bearer
//! credential; a table full of them is a table full of logins, readable by
//! every backup, replica, and reporting account that can see the database.
//! `arcature_sessions.id` is the SHA-256 of the id instead, which is all a
//! lookup needs and nothing an attacker can use.
//!
//! **Expiry is enforced by the query.** Every read carries
//! `expires_at > now()`, evaluated by the database, so an expired session
//! stops working the instant it expires. [`DbSessionStore::sweep_expired`]
//! exists to reclaim disk, not to make expiry true -- a sweep that never runs
//! wastes space and nothing else.
//!
//! # Why it is written here rather than pulled in
//!
//! `tower-sessions-sqlx-store` exists and does roughly this. It is not used
//! for two reasons: this crate is on SQLx 0.9, which that store does not
//! track, and a store is about two hundred lines of SQL that has to match
//! Arcature's own dialect seam anyway. A dependency is a thing to watch for
//! advisories forever; this one would have bought little.
//!
//! # Layout
//!
//! * `dialect/` -- the statement text, one module per dialect, plus how each
//!   one stores an expiry. Nothing outside it names a driver.
//! * `migrate.rs` -- the embedded per-dialect migration and its runner.
//! * `store.rs` -- [`DbSessionStore`] and its `tower_sessions` impls.
//! * `tests.rs` -- the round-trip tests, which need a live database and so
//!   are gated on `test-kit` as well.

mod dialect;
mod error;
mod migrate;
mod store;

// Needs `test-kit` for the "is a test database configured, and is it safe to
// write to" decision, which lives there and must have exactly one spelling.
#[cfg(all(test, feature = "test-kit"))]
mod tests;

pub use error::SessionStoreError;
pub use store::DbSessionStore;
