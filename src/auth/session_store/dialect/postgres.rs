//! PostgreSQL statement text for the session store.
//!
//! Placeholders are `$n`, numbered in order of appearance so the bind order is
//! the same as the `?` dialects. `now()` is the server clock, which is what
//! every expiry comparison is made against -- a session outlives its expiry
//! by however far the reader's clock is fast, so the reader does not get a
//! vote.

/// Every statement the session store issues against PostgreSQL.
pub(crate) mod sql {
    /// Insert a session that must not already exist.
    /// `DO NOTHING` reports the clash as zero rows affected rather than as an
    /// error, which is what lets `create` retry with a fresh id instead of
    /// parsing a driver-specific constraint name.
    /// Binds: id digest, data, expires at.
    pub(crate) const INSERT_NEW: &str = r#"INSERT INTO arcature_sessions (id, data, expires_at)
VALUES ($1, $2, $3)
ON CONFLICT (id) DO NOTHING"#;

    /// Insert a session, overwriting whatever is under that id.
    /// Binds: id digest, data, expires at.
    pub(crate) const UPSERT: &str = r#"INSERT INTO arcature_sessions (id, data, expires_at)
VALUES ($1, $2, $3)
ON CONFLICT (id) DO UPDATE
   SET data = EXCLUDED.data, expires_at = EXCLUDED.expires_at"#;

    /// Load a live session. The expiry is part of the predicate, not a check
    /// the caller makes afterwards, so an expired row is invisible from the
    /// instant it expires whether or not the sweep has run.
    /// Binds: id digest.
    pub(crate) const LOAD: &str = r#"SELECT data, expires_at
  FROM arcature_sessions
 WHERE id = $1 AND expires_at > now()"#;

    /// Binds: id digest.
    pub(crate) const DELETE: &str = "DELETE FROM arcature_sessions WHERE id = $1";

    /// Delete every session whose expiry has passed. No binds.
    pub(crate) const DELETE_EXPIRED: &str =
        "DELETE FROM arcature_sessions WHERE expires_at <= now()";

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_sessions_schema_migrations (
    version    TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
)"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_sessions_schema_migrations WHERE version = $1";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str = "INSERT INTO arcature_sessions_schema_migrations (version) VALUES ($1) ON CONFLICT DO NOTHING";

    /// Serialise concurrent migrators. Session-scoped, so it must be released.
    ///
    /// A key of its own, the next free one after the job queue's `71420001`.
    /// Sharing a key with another subsystem would make an application that
    /// migrates several of them at startup wait on itself.
    /// `tests/advisory_locks.rs` is the registry and fails if two subsystems
    /// ever claim the same number.
    pub(crate) const LOCK: Option<&str> = Some("SELECT pg_advisory_lock(71420002)");

    /// Release [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> = Some("SELECT pg_advisory_unlock(71420002)");

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/postgres/0001_sessions.sql");
}
