//! MySQL 8 statement text.
//!
//! Placeholders are `?`, bound in order of appearance. "Now" is
//! `UTC_TIMESTAMP(6)` rather than `NOW(6)`: `NOW()` follows the session time
//! zone, and the queue's `DATETIME(6)` columns hold UTC.
//!
//! MySQL has `SKIP LOCKED` but no `RETURNING`, and it refuses a subquery that
//! reads the table an `UPDATE` is writing. Both push the claim into an
//! explicit transaction, and both push the sweep onto `UPDATE ... ORDER BY
//! ... LIMIT` instead of the `WHERE id IN (SELECT ...)` the other dialects
//! use.

/// Every statement the queue issues against MySQL.
pub(crate) mod sql {
    /// MySQL needs no per-connection setup for the queue.
    pub(crate) const SESSION_SETUP: Option<&str> = None;

    /// The claim runs in a real transaction so the locking `SELECT` and the
    /// marking `UPDATE` cannot be separated.
    pub(crate) const BEGIN: &str = "START TRANSACTION";

    /// Pick claimable rows, locking them against other claimers and skipping
    /// rows another claimer already holds.
    /// Binds: batch size.
    pub(crate) const CLAIM_PICK: &str = r#"SELECT id, kind, version, payload, attempts, max_attempts
  FROM arcature_jobs
 WHERE status = 'pending' AND available_at <= UTC_TIMESTAMP(6)
 ORDER BY available_at, id
 LIMIT ?
 FOR UPDATE SKIP LOCKED"#;

    /// Mark one picked row as claimed.
    /// Binds: worker id, claim token, lease seconds, job id.
    pub(crate) const CLAIM_MARK: &str = r#"UPDATE arcature_jobs
   SET status = 'running', attempts = attempts + 1, locked_by = ?,
       locked_at = UTC_TIMESTAMP(6), claim_token = ?, lease_seconds = ?,
       last_error = NULL, last_error_kind = NULL,
       updated_at = UTC_TIMESTAMP(6)
 WHERE id = ? AND status = 'pending'"#;

    /// Insert one job row. Binds: id, kind, version, payload, max attempts,
    /// run at, available at.
    pub(crate) const INSERT: &str = r#"INSERT INTO arcature_jobs
       (id, kind, version, payload, max_attempts, run_at, available_at)
VALUES (?, ?, ?, ?, ?, ?, ?)"#;

    /// Binds: job id, claim token.
    pub(crate) const MARK_SUCCEEDED: &str = r#"UPDATE arcature_jobs
   SET status = 'succeeded', locked_by = NULL, locked_at = NULL,
       claim_token = NULL, last_error = NULL, last_error_kind = NULL,
       failed_at = NULL, updated_at = UTC_TIMESTAMP(6)
 WHERE id = ? AND status = 'running' AND claim_token = ?"#;

    /// Binds: available at, message, job id, claim token.
    pub(crate) const MARK_RETRY: &str = r#"UPDATE arcature_jobs
   SET status = 'pending', available_at = ?, locked_by = NULL,
       locked_at = NULL, claim_token = NULL, last_error = ?,
       last_error_kind = 'retryable', failed_at = NULL,
       updated_at = UTC_TIMESTAMP(6)
 WHERE id = ? AND status = 'running' AND claim_token = ?"#;

    /// Binds: message, error kind, job id, claim token.
    pub(crate) const MARK_DEAD: &str = r#"UPDATE arcature_jobs
   SET status = 'dead', locked_by = NULL, locked_at = NULL,
       claim_token = NULL, last_error = ?, last_error_kind = ?,
       failed_at = UTC_TIMESTAMP(6), updated_at = UTC_TIMESTAMP(6)
 WHERE id = ? AND status = 'running' AND claim_token = ?"#;

    /// Binds: lease seconds, job id, claim token.
    pub(crate) const HEARTBEAT: &str = r#"UPDATE arcature_jobs
   SET locked_at = UTC_TIMESTAMP(6), lease_seconds = ?,
       updated_at = UTC_TIMESTAMP(6)
 WHERE id = ? AND status = 'running' AND claim_token = ?"#;

    /// Binds: job id, claim token.
    pub(crate) const RELEASE_CLAIM: &str = r#"UPDATE arcature_jobs
   SET status = 'pending', available_at = UTC_TIMESTAMP(6),
       locked_by = NULL, locked_at = NULL, claim_token = NULL,
       updated_at = UTC_TIMESTAMP(6)
 WHERE id = ? AND status = 'running' AND claim_token = ?"#;

    /// Binds: batch size. `ORDER BY ... LIMIT` on the `UPDATE` itself
    /// replaces the `WHERE id IN (SELECT ...)` the other dialects use:
    /// MySQL rejects a subquery over the table being updated.
    pub(crate) const SWEEP_DEAD: &str = r#"UPDATE arcature_jobs
   SET status = 'dead', locked_by = NULL, locked_at = NULL,
       claim_token = NULL,
       last_error = 'job exhausted its max_attempts after a crash',
       last_error_kind = 'exhausted', failed_at = UTC_TIMESTAMP(6),
       updated_at = UTC_TIMESTAMP(6)
 WHERE status = 'running'
   AND locked_at IS NOT NULL
   AND locked_at + INTERVAL lease_seconds SECOND < UTC_TIMESTAMP(6)
   AND attempts >= max_attempts
 ORDER BY locked_at
 LIMIT ?"#;

    /// Binds: batch size. See [`SWEEP_DEAD`] for the `ORDER BY ... LIMIT`.
    pub(crate) const SWEEP_REQUEUE: &str = r#"UPDATE arcature_jobs
   SET status = 'pending', available_at = UTC_TIMESTAMP(6),
       locked_by = NULL, locked_at = NULL, claim_token = NULL,
       updated_at = UTC_TIMESTAMP(6)
 WHERE status = 'running'
   AND locked_at IS NOT NULL
   AND locked_at + INTERVAL lease_seconds SECOND < UTC_TIMESTAMP(6)
   AND attempts < max_attempts
 ORDER BY locked_at
 LIMIT ?"#;

    /// Binds: job id.
    pub(crate) const CANCEL: &str = r#"UPDATE arcature_jobs
   SET status = 'cancelled', locked_by = NULL, locked_at = NULL,
       claim_token = NULL, updated_at = UTC_TIMESTAMP(6)
 WHERE id = ? AND status IN ('pending', 'running')"#;

    /// Binds: job id.
    pub(crate) const REQUEUE_DEAD: &str = r#"UPDATE arcature_jobs
   SET status = 'pending', attempts = 0,
       available_at = UTC_TIMESTAMP(6), locked_by = NULL, locked_at = NULL,
       claim_token = NULL, last_error = NULL, last_error_kind = NULL,
       failed_at = NULL, updated_at = UTC_TIMESTAMP(6)
 WHERE id = ? AND status = 'dead'"#;

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_jobs_schema_migrations (
    version    VARCHAR(191) NOT NULL PRIMARY KEY,
    applied_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
) ENGINE=InnoDB"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_jobs_schema_migrations WHERE version = ?";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str =
        "INSERT IGNORE INTO arcature_jobs_schema_migrations (version) VALUES (?)";

    /// Serialise concurrent migrators. Session-scoped, so it must be
    /// released.
    pub(crate) const LOCK: Option<&str> = Some("SELECT GET_LOCK('arcature_jobs_migrate', 10)");

    /// Release [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> = Some("SELECT RELEASE_LOCK('arcature_jobs_migrate')");

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/mysql/0001_jobs.sql");
}
