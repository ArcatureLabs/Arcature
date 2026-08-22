//! MySQL 8 statement text for the remember-me store.
//!
//! Placeholders are `?`, bound in order of appearance. "Now" is
//! `UTC_TIMESTAMP(6)` rather than `NOW(6)`: `NOW()` follows the session time
//! zone, and `arcature_remember_tokens.expires_at` is a `DATETIME(6)` holding
//! UTC. A connection that happened to be set to a different time zone would
//! otherwise move every deadline by that offset.
//!
//! One thing worth stating because the store depends on it: MySQL reports
//! *changed* rows in `rows_affected`, not matched rows, so an `UPDATE` that
//! writes a row's existing values reports zero. That would be a trap for a
//! compare-and-swap whose new value could equal the old one. It is safe here
//! because [`ROTATE`](sql::ROTATE) always writes a freshly generated secret
//! digest, which never equals the digest it replaces.

/// Every statement the remember-me store issues against MySQL.
pub(crate) mod sql {
    /// Insert a token that must not already exist.
    ///
    /// `INSERT IGNORE` reports the clash as zero rows affected rather than as
    /// an error, which is what lets `issue` retry with a fresh series instead
    /// of matching on a driver-specific constraint message.
    /// Binds: series, secret digest, subject, expires at, created at.
    pub(crate) const INSERT_NEW: &str = r#"INSERT IGNORE INTO arcature_remember_tokens
    (series, secret_digest, subject, expires_at, created_at)
VALUES (?, ?, ?, ?, ?)"#;

    /// Read one live token by its series. See the PostgreSQL twin for what the
    /// third column is and why it is an integer rather than a boolean. It is
    /// `CAST(... AS SIGNED)` and not `AS BIGINT`, because MySQL's `CAST` has no
    /// `BIGINT` target -- `SIGNED` is how it spells the same 64-bit integer.
    /// Binds: grace cutoff, series.
    pub(crate) const FIND_LIVE: &str = r#"SELECT secret_digest,
       previous_digest,
       CASE WHEN rotated_at IS NOT NULL AND rotated_at > ?
            THEN CAST(1 AS SIGNED) ELSE CAST(0 AS SIGNED) END,
       subject
  FROM arcature_remember_tokens
 WHERE series = ?
   AND expires_at > UTC_TIMESTAMP(6)"#;

    /// Replace a token's secret with a fresh one. See the PostgreSQL twin:
    /// the `secret_digest = ?` in the predicate is what makes this a
    /// compare-and-swap, and the deliberate absence of an expiry predicate is
    /// what keeps `rows_affected = 0` meaning exactly one thing. See this
    /// module's own note for why MySQL's changed-rows counting does not
    /// disturb that reading.
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
    pub(crate) const DELETE_EXPIRED: &str =
        "DELETE FROM arcature_remember_tokens WHERE expires_at <= UTC_TIMESTAMP(6)";

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_remember_tokens_schema_migrations (
    version    VARCHAR(191) NOT NULL PRIMARY KEY,
    applied_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
) ENGINE=InnoDB"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_remember_tokens_schema_migrations WHERE version = ?";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str =
        "INSERT IGNORE INTO arcature_remember_tokens_schema_migrations (version) VALUES (?)";

    /// Serialise concurrent migrators. Session-scoped, so it must be released.
    /// A lock name of its own, for the same reason the PostgreSQL key is its
    /// own: the schemas are independent, and sharing a name would make an
    /// application that migrates all of them wait on itself.
    pub(crate) const LOCK: Option<&str> =
        Some("SELECT GET_LOCK('arcature_remember_tokens_migrate', 10)");

    /// Release [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> =
        Some("SELECT RELEASE_LOCK('arcature_remember_tokens_migrate')");

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/mysql/0001_remember_tokens.sql");
}
