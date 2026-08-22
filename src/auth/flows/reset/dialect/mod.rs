//! The per-dialect seam for the password-reset store.
//!
//! The store is one implementation. Everything that genuinely differs between
//! PostgreSQL, SQLite, and MySQL 8 -- the statement text, the placeholder
//! style, the spelling of "now", and the storage representation of a timestamp
//! -- lives behind this module, and nothing else in [`super`] mentions a
//! driver by name.
//!
//! This mirrors [`crate::tokens`]'s, [`crate::auth::session_store`]'s, and
//! [`crate::jobs`]'s seams rather than sharing code with them, for the reason
//! the second of those states: a generic seam parameterised over table name,
//! history table, and lock would couple subsystems that have no reason to
//! change together, so a schema change in one could break another's migration.

use chrono::{DateTime, Utc};

/// The SQLx database the store speaks. Chosen by the `db-*` features; the
/// mutual-exclusion check lives in [`crate::database`].
pub(crate) type ResetDb = crate::database::Driver;

/// The connection pool the store runs over -- the application's own pool.
pub type ResetPool = crate::database::Pool;

/// How a timestamp is stored in this dialect.
///
/// See [`crate::tokens`]'s seam for the full argument. In one line: SQLite has
/// no timestamp type, and comparing timestamps as text is only correct while
/// every writer agrees on the format down to the digit.
#[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
pub(crate) type StoredTime = DateTime<Utc>;

/// How a timestamp is stored in this dialect. See the PostgreSQL/MySQL variant
/// of this alias for why SQLite differs.
#[cfg(feature = "db-sqlite")]
pub(crate) type StoredTime = i64;

/// Convert an instant into this dialect's storage representation.
#[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
pub(crate) fn stored_time(at: DateTime<Utc>) -> StoredTime {
    at
}

/// Convert an instant into this dialect's storage representation.
///
/// SQLite stores epoch milliseconds, so sub-millisecond precision is dropped.
/// A reset deadline is an hour away; a millisecond either side of it is not a
/// distinction the store is asked to keep.
#[cfg(feature = "db-sqlite")]
pub(crate) fn stored_time(at: DateTime<Utc>) -> StoredTime {
    at.timestamp_millis()
}

// There is deliberately no `restored_time` twin here, unlike the seam this
// one is modelled on. The store never reads a timestamp back: expiry is a
// predicate the database evaluates, and the only instant a caller is told
// about is the one `issue` computed and still holds in memory. A conversion
// nothing calls is a conversion nothing tests.

#[cfg(feature = "db-postgres")]
mod postgres;
#[cfg(feature = "db-postgres")]
pub(crate) use postgres::sql;

#[cfg(feature = "db-sqlite")]
mod sqlite;
#[cfg(feature = "db-sqlite")]
pub(crate) use sqlite::sql;

#[cfg(feature = "db-mysql")]
mod mysql;
#[cfg(feature = "db-mysql")]
pub(crate) use mysql::sql;
