//! PostgreSQL statement text for the API token store.
//!
//! Placeholders are `$n`, numbered in order of appearance so the bind order
//! is the same as the `?` dialects. `now()` is the server clock, which is
//! what every expiry comparison is made against -- a token outlives its
//! expiry by however far the reader's clock is fast, so the reader does not
//! get a vote.

/// Every statement the API token store issues against PostgreSQL.
pub(crate) mod sql {
    /// Insert a token that must not already exist.
    /// `DO NOTHING` reports the clash as zero rows affected rather than as an
    /// error, which is what lets `issue` retry with a fresh id instead of
    /// parsing a driver-specific constraint name.
    /// Binds: id, secret digest, tokenable id, name, abilities, expires at,
    /// created at.
    pub(crate) const INSERT_NEW: &str = r#"INSERT INTO arcature_api_tokens
    (id, secret_digest, tokenable_id, name, abilities, expires_at, created_at)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (id) DO NOTHING"#;

    /// Read one live token by its public id. The expiry is part of the
    /// predicate, not a check the caller makes afterwards, so an expired
    /// token is invisible from the instant it expires whether or not the
    /// sweep has run.
    /// Binds: id.
    pub(crate) const FIND: &str = r#"SELECT tokenable_id, name, abilities, expires_at, created_at
  FROM arcature_api_tokens
 WHERE id = $1 AND expires_at > now()"#;

    /// Every live token issued to one subject, newest first.
    /// Binds: tokenable id.
    pub(crate) const LIST_FOR: &str = r#"SELECT id, tokenable_id, name, abilities, expires_at, created_at
  FROM arcature_api_tokens
 WHERE tokenable_id = $1 AND expires_at > now()
 ORDER BY created_at DESC, id"#;

    /// Binds: id.
    pub(crate) const DELETE: &str = "DELETE FROM arcature_api_tokens WHERE id = $1";

    /// Binds: tokenable id.
    pub(crate) const DELETE_FOR: &str = "DELETE FROM arcature_api_tokens WHERE tokenable_id = $1";

    /// Delete every token whose expiry has passed. No binds.
    pub(crate) const DELETE_EXPIRED: &str =
        "DELETE FROM arcature_api_tokens WHERE expires_at <= now()";

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_api_tokens_schema_migrations (
    version    TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
)"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_api_tokens_schema_migrations WHERE version = $1";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str = "INSERT INTO arcature_api_tokens_schema_migrations (version) VALUES ($1) ON CONFLICT DO NOTHING";

    /// Serialise concurrent migrators. Session-scoped, so it must be
    /// released. A key of its own -- the job queue uses `71420001` and the
    /// session store `71420002` -- because the three schemas are independent
    /// and sharing a key would make an application that migrates all of them
    /// wait on itself for no reason.
    pub(crate) const LOCK: Option<&str> = Some("SELECT pg_advisory_lock(71420003)");

    /// Release [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> = Some("SELECT pg_advisory_unlock(71420003)");

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/postgres/0001_api_tokens.sql");
}
