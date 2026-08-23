//! PostgreSQL statement text for the remember-me store.
//!
//! Placeholders are `$N`. The numbering follows the order the *store* binds
//! in, which is the order the placeholders first appear in the text -- and in
//! [`FIND_LIVE`] that order puts the grace cutoff before the series, because
//! the cutoff is used in the select list and the series in the `WHERE`. The
//! other two dialects bind positionally, so keeping one bind order for all
//! three is what lets the store have one call site per statement.

/// Every statement the remember-me store issues against PostgreSQL.
pub(crate) mod sql {
    /// Insert a token that must not already exist.
    ///
    /// `previous_digest` and `rotated_at` are left out of the column list
    /// rather than bound as nulls: a token that has never rotated has no
    /// previous secret, and writing that as an absent column rather than an
    /// explicit `NULL` keeps the two spellings of "never rotated" from
    /// diverging.
    ///
    /// `ON CONFLICT DO NOTHING` reports a clash as zero rows affected rather
    /// than as an error, which is what lets `issue` retry with a fresh series
    /// instead of matching on a driver-specific constraint message.
    /// Binds: series, secret digest, subject, expires at, created at.
    pub(crate) const INSERT_NEW: &str = r#"INSERT INTO arcature_remember_tokens
    (series, secret_digest, subject, expires_at, created_at)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (series) DO NOTHING"#;

    /// Read one live token by its series, together with the two facts the
    /// theft rule needs about its previous secret.
    ///
    /// The expiry is part of the predicate, not a check the caller makes
    /// afterwards, so a lapsed cookie is invisible from the instant it lapses
    /// whether or not the sweep has run.
    ///
    /// The third column answers "was the last rotation recent enough to still
    /// accept the secret it replaced?" as an integer, because that question is
    /// about a stored timestamp and is therefore cheapest to get right where
    /// the timestamp lives. It is `BIGINT` and not `boolean` for the reason
    /// the migrator's count is: PostgreSQL would hand back a real boolean,
    /// SQLite and MySQL would hand back an integer, and decoding is typed.
    /// Binds: grace cutoff, series.
    pub(crate) const FIND_LIVE: &str = r#"SELECT secret_digest,
       previous_digest,
       CASE WHEN rotated_at IS NOT NULL AND rotated_at > $1
            THEN CAST(1 AS BIGINT) ELSE CAST(0 AS BIGINT) END,
       subject
  FROM arcature_remember_tokens
 WHERE series = $2
   AND expires_at > now()"#;

    /// Replace a token's secret with a fresh one, keeping the series and
    /// remembering the secret that was just retired.
    ///
    /// The `secret_digest = $4` in the predicate is what makes this a
    /// compare-and-swap rather than a write: two requests that arrive with the
    /// same cookie both read the same row, both compute the same match, and
    /// then exactly one of them updates it. The loser learns it lost from
    /// `rows_affected`, and that is the only way to tell a legitimate
    /// concurrent use -- a browser restoring twenty tabs at once -- from a
    /// replay of a secret that was already retired.
    ///
    /// There is deliberately no `expires_at > now()` here, although the row
    /// could in principle lapse between the read and this write. Adding it
    /// would give `rows_affected = 0` two meanings, and the store's whole
    /// reading of that value depends on it having one. The row that gets
    /// rotated a microsecond after it expired is dead to every subsequent
    /// read anyway.
    /// Binds: new secret digest, rotated at, series, old secret digest.
    pub(crate) const ROTATE: &str = r#"UPDATE arcature_remember_tokens
   SET previous_digest = secret_digest,
       secret_digest = $1,
       rotated_at = $2
 WHERE series = $3
   AND secret_digest = $4"#;

    /// Delete one token by its series: an ordinary sign-out on one device.
    /// Binds: series.
    pub(crate) const DELETE_SERIES: &str = "DELETE FROM arcature_remember_tokens WHERE series = $1";

    /// Delete every token belonging to one subject.
    ///
    /// Both "sign out everywhere" and the theft cascade. They are one
    /// statement because they are one action: the response to a stolen cookie
    /// is that none of this subject's cookies are trusted any more, including
    /// the one the legitimate user is holding.
    /// Binds: subject.
    pub(crate) const DELETE_FOR: &str = "DELETE FROM arcature_remember_tokens WHERE subject = $1";

    /// Delete every token whose deadline has passed. No binds.
    pub(crate) const DELETE_EXPIRED: &str =
        "DELETE FROM arcature_remember_tokens WHERE expires_at <= now()";

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_remember_tokens_schema_migrations (
    version    TEXT        PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
)"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_remember_tokens_schema_migrations WHERE version = $1";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str = r#"INSERT INTO arcature_remember_tokens_schema_migrations (version)
VALUES ($1)
ON CONFLICT (version) DO NOTHING"#;

    /// Serialise concurrent migrators. Session-scoped, so it must be released.
    ///
    /// A key of its own, the next free one after the password-reset store's
    /// `71420004`. Sharing a key with another subsystem would make an
    /// application that migrates several of them at startup wait on itself.
    /// `tests/advisory_locks.rs` is the registry and fails if two subsystems
    /// ever claim the same number.
    pub(crate) const LOCK: Option<&str> = Some("SELECT pg_advisory_lock(71420005)");

    /// Release [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> = Some("SELECT pg_advisory_unlock(71420005)");

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/postgres/0001_remember_tokens.sql");
}
