//! SQLite statement text for the remember-me store.
//!
//! Placeholders are `?`, bound in order of appearance. Timestamps are epoch
//! milliseconds in an `INTEGER` column, so "now" is spelled
//! `CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)` -- 2440587.5 is
//! the Julian day of the Unix epoch. SQLite has no `now()`, and
//! `CURRENT_TIMESTAMP` yields text whose format does not compare correctly
//! against anything else the store writes. The expression is spelled out at
//! every use because `concat!` cannot splice a `const`, and building the
//! statement at runtime would give up the `&'static str` SQL rule.

/// Every statement the remember-me store issues against SQLite.
pub(crate) mod sql {
    /// Insert a token that must not already exist.
    ///
    /// `OR IGNORE` reports the clash as zero rows affected rather than as an
    /// error, which is what lets `issue` retry with a fresh series instead of
    /// matching on a driver-specific constraint message.
    /// Binds: series, secret digest, subject, expires at, created at.
    pub(crate) const INSERT_NEW: &str = r#"INSERT OR IGNORE INTO arcature_remember_tokens
    (series, secret_digest, subject, expires_at, created_at)
VALUES (?, ?, ?, ?, ?)"#;

    /// Read one live token by its series. See the PostgreSQL twin for what the
    /// third column is and why it is an integer rather than a boolean; here it
    /// is a bare `1`/`0`, because that is already SQLite's only integer type.
    /// Binds: grace cutoff, series.
    pub(crate) const FIND_LIVE: &str = r#"SELECT secret_digest,
       previous_digest,
       CASE WHEN rotated_at IS NOT NULL AND rotated_at > ? THEN 1 ELSE 0 END,
       subject
  FROM arcature_remember_tokens
 WHERE series = ?
   AND expires_at > CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)"#;

    /// Replace a token's secret with a fresh one. See the PostgreSQL twin:
    /// the `secret_digest = ?` in the predicate is what makes this a
    /// compare-and-swap, and the deliberate absence of an expiry predicate is
    /// what keeps `rows_affected = 0` meaning exactly one thing.
    /// Binds: new secret digest, rotated at, series, old secret digest.
    pub(crate) const ROTATE: &str = r#"UPDATE arcature_remember_tokens
   SET previous_digest = secret_digest,
       secret_digest = ?,
       rotated_at = ?
 WHERE series = ?
   AND secret_digest = ?"#;

    /// Delete one token by its series: an ordinary sign-out on one device.
    /// Binds: series.
    pub(crate) const DELETE_SERIES: &str = "DELETE FROM arcature_remember_tokens WHERE series = ?";

    /// Delete every token belonging to one subject -- "sign out everywhere",
    /// and the theft cascade. See the PostgreSQL twin.
    /// Binds: subject.
    pub(crate) const DELETE_FOR: &str = "DELETE FROM arcature_remember_tokens WHERE subject = ?";

    /// Delete every token whose deadline has passed. No binds.
    pub(crate) const DELETE_EXPIRED: &str = r#"DELETE FROM arcature_remember_tokens
 WHERE expires_at <= CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)"#;

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_remember_tokens_schema_migrations (
    version    TEXT PRIMARY KEY,
    applied_at INTEGER NOT NULL
        DEFAULT (CAST((julianday('now')-2440587.5)*86400000 AS INTEGER))
)"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_remember_tokens_schema_migrations WHERE version = ?";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str =
        "INSERT OR IGNORE INTO arcature_remember_tokens_schema_migrations (version) VALUES (?)";

    /// SQLite has no advisory lock, and needs none here: every statement in
    /// the migration is idempotent (`IF NOT EXISTS`, `INSERT OR IGNORE`) and
    /// SQLite serialises writers anyway, so two migrators racing converge on
    /// the same schema instead of conflicting.
    pub(crate) const LOCK: Option<&str> = None;

    /// See [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> = None;

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/sqlite/0001_remember_tokens.sql");
}
