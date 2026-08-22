//! The per-dialect seam for the session store.
//!
//! The store is one implementation. Everything that genuinely differs between
//! PostgreSQL, SQLite, and MySQL 8 -- the statement text, the placeholder
//! style, the spelling of "now", the storage representation of an expiry, and
//! which of the three upsert grammars applies -- lives behind this module,
//! and nothing else in `crate::auth::session_store` mentions a driver by
//! name.
//!
//! # Why the upsert has three spellings
//!
//! Every dialect can write "insert this row, or overwrite the one already
//! there", and each spells it differently: PostgreSQL and SQLite take
//! `ON CONFLICT ... DO UPDATE`, MySQL takes `REPLACE INTO`. `REPLACE` is the
//! one that would be wrong on a table with a foreign key or an
//! auto-increment column, because it deletes the old row before inserting the
//! new one -- `arcature_sessions` has neither, and the alternative
//! (`ON DUPLICATE KEY UPDATE`) either binds the same two values twice or uses
//! the `VALUES()` function MySQL 8.0.20 deprecated. All three spellings take
//! the same three binds in the same order, so the Rust side stays one path.

#[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
use chrono::{DateTime, Utc};
use time::OffsetDateTime;

use super::error::SessionStoreError;

/// The SQLx database the store speaks. Chosen by the `db-*` features; the
/// mutual-exclusion check lives in [`crate::database`].
pub(crate) type SessionDb = crate::database::Driver;

/// The connection pool the store runs over.
pub(crate) type SessionPool = crate::database::Pool;

/// How an expiry is stored in this dialect.
///
/// PostgreSQL and MySQL have real timestamp types and SQLx binds
/// `DateTime<Utc>` straight into them. SQLite has no timestamp type: a value
/// bound as text would have to be compared as text, and text comparison of
/// timestamps is only correct while every writer agrees on the format down to
/// the digit. Epoch milliseconds are compared as integers, which is correct
/// no matter who wrote the row.
#[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
pub(crate) type StoredTime = DateTime<Utc>;

/// How an expiry is stored in this dialect. See the PostgreSQL/MySQL variant
/// of this alias for why SQLite differs.
#[cfg(feature = "db-sqlite")]
pub(crate) type StoredTime = i64;

/// Convert an expiry into this dialect's storage representation.
///
/// # Errors
///
/// Returns [`SessionStoreError::Expiry`] when the instant is outside the
/// range the column can hold. Reachable only from a caller that set an expiry
/// tens of thousands of years out, but silently storing a different instant
/// than the one asked for is how a session outlives its expiry.
#[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
pub(crate) fn stored_time(at: OffsetDateTime) -> Result<StoredTime, SessionStoreError> {
    DateTime::from_timestamp(at.unix_timestamp(), at.nanosecond())
        .ok_or_else(|| SessionStoreError::Expiry(at.to_string()))
}

/// Convert an expiry into this dialect's storage representation.
///
/// SQLite stores epoch milliseconds, so sub-millisecond precision is dropped.
/// A session expiry is a wall-clock deadline minutes or hours away; a
/// millisecond either side of it is not a distinction the store is asked to
/// keep.
///
/// # Errors
///
/// Returns [`SessionStoreError::Expiry`] when the instant does not fit in an
/// `i64` of milliseconds.
#[cfg(feature = "db-sqlite")]
pub(crate) fn stored_time(at: OffsetDateTime) -> Result<StoredTime, SessionStoreError> {
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| SessionStoreError::Expiry(at.to_string()))
}

/// Read an expiry back out of this dialect's storage representation.
///
/// # Errors
///
/// Returns [`SessionStoreError::Expiry`] when the stored value is not a time
/// `OffsetDateTime` can represent, which means the row was written by
/// something other than this store.
#[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
pub(crate) fn restored_time(stored: StoredTime) -> Result<OffsetDateTime, SessionStoreError> {
    let nanos = i128::from(stored.timestamp()) * 1_000_000_000
        + i128::from(stored.timestamp_subsec_nanos());
    OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .map_err(|_| SessionStoreError::Expiry(stored.to_string()))
}

/// Read an expiry back out of this dialect's storage representation.
///
/// # Errors
///
/// Returns [`SessionStoreError::Expiry`] when the stored millisecond count is
/// not a time `OffsetDateTime` can represent.
#[cfg(feature = "db-sqlite")]
pub(crate) fn restored_time(stored: StoredTime) -> Result<OffsetDateTime, SessionStoreError> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(stored) * 1_000_000)
        .map_err(|_| SessionStoreError::Expiry(stored.to_string()))
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
