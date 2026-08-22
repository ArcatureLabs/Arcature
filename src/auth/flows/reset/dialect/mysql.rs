//! MySQL 8 statement text for the password-reset store.
//!
//! Placeholders are `?`, bound in order of appearance. "Now" is
//! `UTC_TIMESTAMP(6)` rather than `NOW(6)`: `NOW()` follows the session time
//! zone, and `arcature_password_resets.expires_at` is a `DATETIME(6)` holding
//! UTC. A connection that happened to be set to a different time zone would
//! otherwise move every deadline by that offset.
//!
//! There is no `DELETE ... RETURNING` here, and its absence is the reason the
//! store spends a token in two statements rather than one. MySQL 8 has no such
//! clause, and writing the single-statement version for the other two dialects
//! would mean two spend paths -- the one place in this subsystem where a
//! divergence would be a correctness difference rather than a spelling one.

/// Every statement the password-reset store issues against MySQL.
pub(crate) mod sql {
    /// Insert a token that must not already exist.
    ///
    /// `INSERT IGNORE` reports the clash as zero rows affected rather than as
    /// an error, which is what lets `issue` retry with a fresh id instead of
    /// matching on a driver-specific constraint message.
    /// Binds: id, secret digest, subject, expires at, created at.
    pub(crate) const INSERT_NEW: &str = r#"INSERT IGNORE INTO arcature_password_resets
    (id, secret_digest, subject, expires_at, created_at)
VALUES (?, ?, ?, ?, ?)"#;

    /// Read one live token by its public id. The expiry is part of the
    /// predicate, not a check the caller makes afterwards, so an expired link
    /// is invisible from the instant it expires whether or not the sweep has
    /// run.
    /// Binds: id.
    pub(crate) const FIND_LIVE: &str = r#"SELECT secret_digest, subject
  FROM arcature_password_resets
 WHERE id = ? AND expires_at > UTC_TIMESTAMP(6)"#;

    /// Clear every link belonging to one subject. See the PostgreSQL twin:
    /// this delete is also the compare-and-swap that makes a reset
    /// single-use.
    /// Binds: subject.
    pub(crate) const DELETE_FOR: &str = "DELETE FROM arcature_password_resets WHERE subject = ?";

    /// Delete every token whose deadline has passed. No binds.
    pub(crate) const DELETE_EXPIRED: &str =
        "DELETE FROM arcature_password_resets WHERE expires_at <= UTC_TIMESTAMP(6)";

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_password_resets_schema_migrations (
    version    VARCHAR(191) NOT NULL PRIMARY KEY,
    applied_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
) ENGINE=InnoDB"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_password_resets_schema_migrations WHERE version = ?";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str =
        "INSERT IGNORE INTO arcature_password_resets_schema_migrations (version) VALUES (?)";

    /// Serialise concurrent migrators. Session-scoped, so it must be released.
    /// A lock name of its own, for the same reason the PostgreSQL key is its
    /// own: the schemas are independent, and sharing a name would make an
    /// application that migrates all of them wait on itself.
    pub(crate) const LOCK: Option<&str> =
        Some("SELECT GET_LOCK('arcature_password_resets_migrate', 10)");

    /// Release [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> =
        Some("SELECT RELEASE_LOCK('arcature_password_resets_migrate')");

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/mysql/0001_password_resets.sql");
}
