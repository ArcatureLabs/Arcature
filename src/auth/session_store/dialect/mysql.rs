//! MySQL 8 statement text for the session store.
//!
//! Placeholders are `?`, bound in order of appearance. "Now" is
//! `UTC_TIMESTAMP(6)` rather than `NOW(6)`: `NOW()` follows the session time
//! zone, and `arcature_sessions.expires_at` is a `DATETIME(6)` holding UTC.
//! A connection that happened to be set to a different time zone would
//! otherwise move every expiry by that offset.

/// Every statement the session store issues against MySQL.
pub(crate) mod sql {
    /// Insert a session that must not already exist.
    ///
    /// `INSERT IGNORE` reports the clash as zero rows affected rather than as
    /// an error, which is what lets `create` retry with a fresh id instead of
    /// matching on a driver-specific constraint message.
    /// Binds: id digest, data, expires at.
    pub(crate) const INSERT_NEW: &str =
        "INSERT IGNORE INTO arcature_sessions (id, data, expires_at) VALUES (?, ?, ?)";

    /// Insert a session, overwriting whatever is under that id.
    ///
    /// `REPLACE` rather than `ON DUPLICATE KEY UPDATE` so the statement takes
    /// the same three binds as the other dialects; see the module
    /// documentation of the parent for why deleting-then-inserting is safe on
    /// this particular table.
    /// Binds: id digest, data, expires at.
    pub(crate) const UPSERT: &str =
        "REPLACE INTO arcature_sessions (id, data, expires_at) VALUES (?, ?, ?)";

    /// Load a live session. The expiry is part of the predicate, not a check
    /// the caller makes afterwards, so an expired row is invisible from the
    /// instant it expires whether or not the sweep has run.
    /// Binds: id digest.
    pub(crate) const LOAD: &str = r#"SELECT data, expires_at
  FROM arcature_sessions
 WHERE id = ? AND expires_at > UTC_TIMESTAMP(6)"#;

    /// Binds: id digest.
    pub(crate) const DELETE: &str = "DELETE FROM arcature_sessions WHERE id = ?";

    /// Delete every session whose expiry has passed. No binds.
    pub(crate) const DELETE_EXPIRED: &str =
        "DELETE FROM arcature_sessions WHERE expires_at <= UTC_TIMESTAMP(6)";

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_sessions_schema_migrations (
    version    VARCHAR(191) NOT NULL PRIMARY KEY,
    applied_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
) ENGINE=InnoDB"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_sessions_schema_migrations WHERE version = ?";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str =
        "INSERT IGNORE INTO arcature_sessions_schema_migrations (version) VALUES (?)";

    /// Serialise concurrent migrators. Session-scoped, so it must be
    /// released. A different lock name from the job queue's: the two schemas
    /// are independent, and sharing a name would make an application that
    /// migrates both wait on itself for no reason.
    pub(crate) const LOCK: Option<&str> = Some("SELECT GET_LOCK('arcature_sessions_migrate', 10)");

    /// Release [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> =
        Some("SELECT RELEASE_LOCK('arcature_sessions_migrate')");

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/mysql/0001_sessions.sql");
}
