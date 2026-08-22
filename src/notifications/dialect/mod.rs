//! The per-dialect seam for the notification inbox.
//!
//! The store is one implementation. Everything that genuinely differs between
//! PostgreSQL, SQLite, and MySQL 8 -- the statement text, the placeholder
//! style, and the storage representation of a timestamp -- lives behind this
//! module, and nothing else in [`crate::notifications`] mentions a driver by
//! name.
//!
//! This mirrors [`crate::tokens`]'s, [`crate::auth::session_store`]'s, and
//! [`crate::jobs`]'s seams rather than sharing code with them, for the reason
//! stated in each: a generic seam parameterised over table name, history
//! table, and lock would couple subsystems that have no reason to change
//! together, so a schema change in one could break another's migration.

use chrono::{DateTime, Utc};

use super::channel::NotificationError;

/// The SQLx database the store speaks. Chosen by the `db-*` features; the
/// mutual-exclusion check lives in [`crate::database`].
pub(crate) type NotificationDb = crate::database::Driver;

/// The connection pool the store runs over -- the application's own pool.
pub type NotificationPool = crate::database::Pool;

/// How a timestamp is stored in this dialect.
///
/// PostgreSQL and MySQL have real timestamp types and SQLx binds
/// `DateTime<Utc>` straight into them. SQLite has no timestamp type: a value
/// bound as text would have to be compared as text, and text comparison of
/// timestamps is only correct while every writer agrees on the format down to
/// the digit. Epoch milliseconds are compared as integers, which is correct
/// no matter who wrote the row.
#[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
pub(crate) type StoredTime = DateTime<Utc>;

/// How a timestamp is stored in this dialect. See the PostgreSQL/MySQL
/// variant of this alias for why SQLite differs.
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
/// The two instants this store keeps are "when it was said" and "when it was
/// read"; neither is asked to distinguish a millisecond.
#[cfg(feature = "db-sqlite")]
pub(crate) fn stored_time(at: DateTime<Utc>) -> StoredTime {
    at.timestamp_millis()
}

/// Read a timestamp back out of this dialect's storage representation.
///
/// # Errors
///
/// Never fails on this dialect; the signature matches SQLite's so the store
/// has one code path.
#[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
pub(crate) fn restored_time(stored: StoredTime) -> Result<DateTime<Utc>, NotificationError> {
    Ok(stored)
}

/// Read a timestamp back out of this dialect's storage representation.
///
/// # Errors
///
/// Returns [`NotificationError::Timestamp`] when the stored millisecond count
/// is not a time `chrono` can represent, which means the row was written by
/// something other than this store.
#[cfg(feature = "db-sqlite")]
pub(crate) fn restored_time(stored: StoredTime) -> Result<DateTime<Utc>, NotificationError> {
    DateTime::from_timestamp_millis(stored)
        .ok_or_else(|| NotificationError::Timestamp(stored.to_string()))
}

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
