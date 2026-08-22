//! SQLite statement text for the session store.
//!
//! Placeholders are `?`, bound in order of appearance. Expiries are epoch
//! milliseconds in an `INTEGER` column, so "now" is spelled
//! `CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)` -- 2440587.5 is
//! the Julian day of the Unix epoch. SQLite has no `now()`, and
//! `CURRENT_TIMESTAMP` yields text whose format does not compare correctly
//! against anything else the store writes. The expression is spelled out at
//! every use because `concat!` cannot splice a `const`, and building the
//! statement at runtime would give up the `&'static str` SQL rule.

/// Every statement the session store issues against SQLite.
pub(crate) mod sql {
    /// Insert a session that must not already exist.
    ///
    /// `OR IGNORE` reports the clash as zero rows affected rather than as an
    /// error, which is what lets `create` retry with a fresh id instead of
    /// matching on a driver-specific constraint message.
    /// Binds: id digest, data, expires at.
    pub(crate) const INSERT_NEW: &str =
        "INSERT OR IGNORE INTO arcature_sessions (id, data, expires_at) VALUES (?, ?, ?)";

    /// Insert a session, overwriting whatever is under that id.
    /// Binds: id digest, data, expires at.
    pub(crate) const UPSERT: &str = r#"INSERT INTO arcature_sessions (id, data, expires_at)
VALUES (?, ?, ?)
ON CONFLICT (id) DO UPDATE
   SET data = excluded.data, expires_at = excluded.expires_at"#;

    /// Load a live session. The expiry is part of the predicate, not a check
    /// the caller makes afterwards, so an expired row is invisible from the
    /// instant it expires whether or not the sweep has run.
    /// Binds: id digest.
    pub(crate) const LOAD: &str = r#"SELECT data, expires_at
  FROM arcature_sessions
 WHERE id = ?
   AND expires_at > CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)"#;

    /// Binds: id digest.
    pub(crate) const DELETE: &str = "DELETE FROM arcature_sessions WHERE id = ?";

    /// Delete every session whose expiry has passed. No binds.
    pub(crate) const DELETE_EXPIRED: &str = r#"DELETE FROM arcature_sessions
 WHERE expires_at <= CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)"#;

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_sessions_schema_migrations (
    version    TEXT PRIMARY KEY,
    applied_at INTEGER NOT NULL
        DEFAULT (CAST((julianday('now')-2440587.5)*86400000 AS INTEGER))
)"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_sessions_schema_migrations WHERE version = ?";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str =
        "INSERT OR IGNORE INTO arcature_sessions_schema_migrations (version) VALUES (?)";

    /// SQLite has no advisory lock, and needs none here: every statement in
    /// the migration is idempotent (`IF NOT EXISTS`, `INSERT OR IGNORE`) and
    /// SQLite serialises writers anyway, so two migrators racing converge on
    /// the same schema instead of conflicting.
    pub(crate) const LOCK: Option<&str> = None;

    /// See [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> = None;

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/sqlite/0001_sessions.sql");
}
