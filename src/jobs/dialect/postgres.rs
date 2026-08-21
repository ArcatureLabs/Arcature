//! PostgreSQL statement text.
//!
//! Placeholders are `$n`, numbered in order of appearance so the bind order is
//! the same as the `?` dialects. `now()` is the server clock, which is what
//! the lease arithmetic compares against.

/// Every statement the queue issues against PostgreSQL.
pub(crate) mod sql {
    /// Claim a batch. `FOR UPDATE SKIP LOCKED` picks rows no other claimer
    /// holds, and `RETURNING` hands them back in the same round trip.
    /// Binds: worker id, claim token, lease seconds, batch size.
    pub(crate) const CLAIM: &str = r#"UPDATE arcature_jobs
   SET status          = 'running',
       attempts        = attempts + 1,
       locked_by       = $1,
       locked_at       = now(),
       claim_token     = $2,
       lease_seconds   = $3,
       last_error      = NULL,
       last_error_kind = NULL,
       updated_at      = now()
 WHERE id IN (
     SELECT id FROM arcature_jobs
      WHERE status = 'pending'
        AND available_at <= now()
      ORDER BY available_at, id
      LIMIT $4
      FOR UPDATE SKIP LOCKED
 )
 RETURNING id, claim_token, kind, version, payload, attempts, max_attempts, lease_seconds"#;

    /// Insert one job row. Binds: id, kind, version, payload, max attempts,
    /// run at, available at.
    pub(crate) const INSERT: &str = r#"INSERT INTO arcature_jobs
       (id, kind, version, payload, max_attempts, run_at, available_at)
VALUES ($1, $2, $3, $4, $5, $6, $7)"#;

    /// Binds: job id, claim token.
    pub(crate) const MARK_SUCCEEDED: &str = r#"UPDATE arcature_jobs
   SET status = 'succeeded', locked_by = NULL, locked_at = NULL,
       claim_token = NULL, last_error = NULL, last_error_kind = NULL,
       failed_at = NULL, updated_at = now()
 WHERE id = $1 AND status = 'running' AND claim_token = $2"#;

    /// Binds: available at, message, job id, claim token.
    pub(crate) const MARK_RETRY: &str = r#"UPDATE arcature_jobs
   SET status = 'pending', available_at = $1, locked_by = NULL,
       locked_at = NULL, claim_token = NULL, last_error = $2,
       last_error_kind = 'retryable', failed_at = NULL, updated_at = now()
 WHERE id = $3 AND status = 'running' AND claim_token = $4"#;

    /// Binds: message, error kind, job id, claim token.
    pub(crate) const MARK_DEAD: &str = r#"UPDATE arcature_jobs
   SET status = 'dead', locked_by = NULL, locked_at = NULL,
       claim_token = NULL, last_error = $1, last_error_kind = $2,
       failed_at = now(), updated_at = now()
 WHERE id = $3 AND status = 'running' AND claim_token = $4"#;

    /// Binds: lease seconds, job id, claim token.
    pub(crate) const HEARTBEAT: &str = r#"UPDATE arcature_jobs
   SET locked_at = now(), lease_seconds = $1, updated_at = now()
 WHERE id = $2 AND status = 'running' AND claim_token = $3"#;

    /// Binds: job id, claim token.
    pub(crate) const RELEASE_CLAIM: &str = r#"UPDATE arcature_jobs
   SET status = 'pending', available_at = now(), locked_by = NULL,
       locked_at = NULL, claim_token = NULL, updated_at = now()
 WHERE id = $1 AND status = 'running' AND claim_token = $2"#;

    /// Binds: batch size.
    pub(crate) const SWEEP_DEAD: &str = r#"UPDATE arcature_jobs
   SET status = 'dead', locked_by = NULL, locked_at = NULL,
       claim_token = NULL,
       last_error = 'job exhausted its max_attempts after a crash',
       last_error_kind = 'exhausted', failed_at = now(), updated_at = now()
 WHERE id IN (
     SELECT id FROM arcature_jobs
      WHERE status = 'running'
        AND locked_at IS NOT NULL
        AND locked_at + (lease_seconds || ' seconds')::interval < now()
        AND attempts >= max_attempts
      LIMIT $1
      FOR UPDATE SKIP LOCKED
 )"#;

    /// Binds: batch size.
    pub(crate) const SWEEP_REQUEUE: &str = r#"UPDATE arcature_jobs
   SET status = 'pending', available_at = now(), locked_by = NULL,
       locked_at = NULL, claim_token = NULL, updated_at = now()
 WHERE id IN (
     SELECT id FROM arcature_jobs
      WHERE status = 'running'
        AND locked_at IS NOT NULL
        AND locked_at + (lease_seconds || ' seconds')::interval < now()
        AND attempts < max_attempts
      LIMIT $1
      FOR UPDATE SKIP LOCKED
 )"#;

    /// Binds: job id.
    pub(crate) const CANCEL: &str = r#"UPDATE arcature_jobs
   SET status = 'cancelled', locked_by = NULL, locked_at = NULL,
       claim_token = NULL, updated_at = now()
 WHERE id = $1 AND status IN ('pending', 'running')"#;

    /// Binds: job id.
    pub(crate) const REQUEUE_DEAD: &str = r#"UPDATE arcature_jobs
   SET status = 'pending', attempts = 0, available_at = now(),
       locked_by = NULL, locked_at = NULL, claim_token = NULL,
       last_error = NULL, last_error_kind = NULL, failed_at = NULL,
       updated_at = now()
 WHERE id = $1 AND status = 'dead'"#;

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_jobs_schema_migrations (
    version    TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
)"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_jobs_schema_migrations WHERE version = $1";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str =
        "INSERT INTO arcature_jobs_schema_migrations (version) VALUES ($1) ON CONFLICT DO NOTHING";

    /// Serialise concurrent migrators. Session-scoped, so it must be released.
    pub(crate) const LOCK: Option<&str> = Some("SELECT pg_advisory_lock(71420001)");

    /// Release [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> = Some("SELECT pg_advisory_unlock(71420001)");

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/postgres/0001_jobs.sql");
}
