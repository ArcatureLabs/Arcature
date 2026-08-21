//! The per-dialect seam.
//!
//! The queue is one implementation. Everything that genuinely differs between
//! PostgreSQL, SQLite, and MySQL 8 -- the statement text, the placeholder
//! style, the storage representation of a timestamp, and the shape of the
//! claim itself -- lives behind this module, and nothing else in
//! `crate::jobs` mentions a driver by name.
//!
//! # Why the claim has three shapes
//!
//! - **PostgreSQL** claims with a single `UPDATE ... RETURNING` over a
//!   `FOR UPDATE SKIP LOCKED` subquery. One round trip, no explicit
//!   transaction.
//! - **MySQL 8** has `SKIP LOCKED` but no `RETURNING`, so the claim is a
//!   transaction: a locking `SELECT ... FOR UPDATE SKIP LOCKED` picks the
//!   rows, then each picked row is marked in place.
//! - **SQLite** has neither. It serialises writers instead: `BEGIN IMMEDIATE`
//!   takes the database write lock for the whole transaction, so a claim is
//!   exclusive by construction and there is nothing to skip. Concurrent
//!   claimers block on the write lock rather than skipping rows, which is why
//!   the connection sets `busy_timeout` first.
//!
//! MySQL and SQLite share the pick-then-mark Rust code; only the `BEGIN` and
//! the pick statement differ.

use chrono::{DateTime, Utc};

/// The SQLx database the queue speaks. Chosen by the `db-*` features; the
/// mutual-exclusion check lives in [`crate::database`].
pub(crate) type JobDb = crate::database::Driver;

/// The connection pool the queue runs over -- the application's own pool.
pub type JobPool = crate::database::Pool;

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
#[cfg(feature = "db-sqlite")]
pub(crate) fn stored_time(at: DateTime<Utc>) -> StoredTime {
    at.timestamp_millis()
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
