//! PostgreSQL statement text for the password-reset store.
//!
//! Placeholders are `$n`, numbered in order of appearance so the bind order is
//! the same as the `?` dialects. `now()` is the server clock, which is what
//! every expiry comparison is made against -- a link outlives its deadline by
//! however far the reader's clock is fast, so the reader does not get a vote.

/// Every statement the password-reset store issues against PostgreSQL.
pub(crate) mod sql {
    /// Insert a token that must not already exist.
    /// `DO NOTHING` reports the clash as zero rows affected rather than as an
    /// error, which is what lets `issue` retry with a fresh id instead of
    /// parsing a driver-specific constraint name.
    /// Binds: id, secret digest, subject, expires at, created at.
    pub(crate) const INSERT_NEW: &str = r#"INSERT INTO arcature_password_resets
    (id, secret_digest, subject, expires_at, created_at)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (id) DO NOTHING"#;

    /// Read one live token by its public id. The expiry is part of the
    /// predicate, not a check the caller makes afterwards, so an expired link
    /// is invisible from the instant it expires whether or not the sweep has
    /// run.
    /// Binds: id.
    pub(crate) const FIND_LIVE: &str = r#"SELECT secret_digest, subject
  FROM arcature_password_resets
 WHERE id = $1 AND expires_at > now()"#;

    /// Clear every link belonging to one subject.
    ///
    /// This is both the revocation path and the spend: it is the
    /// compare-and-swap that makes a reset single-use, because two requests
    /// that both pass the digest check race here and exactly one of them sees
    /// a row affected.
    /// Binds: subject.
    pub(crate) const DELETE_FOR: &str = "DELETE FROM arcature_password_resets WHERE subject = $1";

    /// Delete every token whose deadline has passed. No binds.
    pub(crate) const DELETE_EXPIRED: &str =
        "DELETE FROM arcature_password_resets WHERE expires_at <= now()";

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_password_resets_schema_migrations (
    version    TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
)"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_password_resets_schema_migrations WHERE version = $1";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str = "INSERT INTO arcature_password_resets_schema_migrations (version) VALUES ($1) ON CONFLICT DO NOTHING";

    /// Serialise concurrent migrators. Session-scoped, so it must be released.
    /// A key of its own -- the job queue uses `71420001`, the session store
    /// `71420002`, and the API token store `71420003` -- because the four
    /// schemas are independent and sharing a key would make an application
    /// that migrates all of them wait on itself for no reason.
    pub(crate) const LOCK: Option<&str> = Some("SELECT pg_advisory_lock(71420004)");

    /// Release [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> = Some("SELECT pg_advisory_unlock(71420004)");

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/postgres/0001_password_resets.sql");
}
