//! SQLite statement text.
//!
//! Placeholders are `?`, bound in order of appearance. Timestamps are epoch
//! milliseconds in `INTEGER` columns, so "now" is spelled
//! `CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)` -- 2440587.5 is
//! the Julian day of the Unix epoch. SQLite has no `now()`, and
//! `CURRENT_TIMESTAMP` yields text whose format does not compare correctly
//! against anything else the queue writes. The expression is spelled out at
//! every use because `concat!` cannot splice a `const`, and building the
//! statement at runtime would give up the `&'static str` SQL rule.

/// Every statement the queue issues against SQLite.
pub(crate) mod sql {
    /// Wait rather than fail when another connection holds the write lock.
    /// A claim transaction is a couple of round trips, so a claimer that
    /// waits is waiting on another claimer, never on a job running.
    pub(crate) const SESSION_SETUP: Option<&str> = Some("PRAGMA busy_timeout = 5000");

    /// Take the write lock up front. A deferred `BEGIN` upgrades to a write
    /// lock only at the first write, which can fail with `SQLITE_BUSY` after
    /// the read has already happened -- exactly the window in which two
    /// claimers could pick the same row.
    pub(crate) const BEGIN: &str = "BEGIN IMMEDIATE";

    /// Pick claimable rows. No locking clause: the enclosing `BEGIN
    /// IMMEDIATE` already excludes every other writer, so there is no other
    /// claimer's lock to skip.
    /// Binds: batch size.
    pub(crate) const CLAIM_PICK: &str = r#"SELECT id, kind, version, payload, attempts, max_attempts
  FROM arcature_jobs
 WHERE status = 'pending'
   AND available_at <= CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)
 ORDER BY available_at, id
 LIMIT ?"#;

    /// Mark one picked row as claimed.
    /// Binds: worker id, claim token, lease seconds, job id.
    pub(crate) const CLAIM_MARK: &str = r#"UPDATE arcature_jobs
   SET status = 'running', attempts = attempts + 1, locked_by = ?,
       locked_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER),
       claim_token = ?, lease_seconds = ?, last_error = NULL,
       last_error_kind = NULL,
       updated_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)
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
       failed_at = NULL,
       updated_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)
 WHERE id = ? AND status = 'running' AND claim_token = ?"#;

    /// Binds: available at, message, job id, claim token.
    pub(crate) const MARK_RETRY: &str = r#"UPDATE arcature_jobs
   SET status = 'pending', available_at = ?, locked_by = NULL,
       locked_at = NULL, claim_token = NULL, last_error = ?,
       last_error_kind = 'retryable', failed_at = NULL,
       updated_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)
 WHERE id = ? AND status = 'running' AND claim_token = ?"#;

    /// Binds: message, error kind, job id, claim token.
    pub(crate) const MARK_DEAD: &str = r#"UPDATE arcature_jobs
   SET status = 'dead', locked_by = NULL, locked_at = NULL,
       claim_token = NULL, last_error = ?, last_error_kind = ?,
       failed_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER),
       updated_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)
 WHERE id = ? AND status = 'running' AND claim_token = ?"#;

    /// Binds: lease seconds, job id, claim token.
    pub(crate) const HEARTBEAT: &str = r#"UPDATE arcature_jobs
   SET locked_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER),
       lease_seconds = ?,
       updated_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)
 WHERE id = ? AND status = 'running' AND claim_token = ?"#;

    /// Binds: job id, claim token.
    pub(crate) const RELEASE_CLAIM: &str = r#"UPDATE arcature_jobs
   SET status = 'pending',
       available_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER),
       locked_by = NULL, locked_at = NULL, claim_token = NULL,
       updated_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)
 WHERE id = ? AND status = 'running' AND claim_token = ?"#;

    /// Binds: batch size.
    pub(crate) const SWEEP_DEAD: &str = r#"UPDATE arcature_jobs
   SET status = 'dead', locked_by = NULL, locked_at = NULL,
       claim_token = NULL,
       last_error = 'job exhausted its max_attempts after a crash',
       last_error_kind = 'exhausted',
       failed_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER),
       updated_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)
 WHERE id IN (
     SELECT id FROM arcature_jobs
      WHERE status = 'running'
        AND locked_at IS NOT NULL
        AND locked_at + lease_seconds * 1000
            < CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)
        AND attempts >= max_attempts
      LIMIT ?
 )"#;

    /// Binds: batch size.
    pub(crate) const SWEEP_REQUEUE: &str = r#"UPDATE arcature_jobs
   SET status = 'pending',
       available_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER),
       locked_by = NULL, locked_at = NULL, claim_token = NULL,
       updated_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)
 WHERE id IN (
     SELECT id FROM arcature_jobs
      WHERE status = 'running'
        AND locked_at IS NOT NULL
        AND locked_at + lease_seconds * 1000
            < CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)
        AND attempts < max_attempts
      LIMIT ?
 )"#;

    /// Binds: job id.
    pub(crate) const CANCEL: &str = r#"UPDATE arcature_jobs
   SET status = 'cancelled', locked_by = NULL, locked_at = NULL,
       claim_token = NULL,
       updated_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)
 WHERE id = ? AND status IN ('pending', 'running')"#;

    /// Binds: job id.
    pub(crate) const REQUEUE_DEAD: &str = r#"UPDATE arcature_jobs
   SET status = 'pending', attempts = 0,
       available_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER),
       locked_by = NULL, locked_at = NULL, claim_token = NULL,
       last_error = NULL, last_error_kind = NULL, failed_at = NULL,
       updated_at = CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)
 WHERE id = ? AND status = 'dead'"#;

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_jobs_schema_migrations (
    version    TEXT PRIMARY KEY,
    applied_at INTEGER NOT NULL
        DEFAULT (CAST((julianday('now')-2440587.5)*86400000 AS INTEGER))
)"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_jobs_schema_migrations WHERE version = ?";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str =
        "INSERT OR IGNORE INTO arcature_jobs_schema_migrations (version) VALUES (?)";

    /// SQLite has no advisory lock, and needs none here: every statement in
    /// the migration is idempotent (`IF NOT EXISTS`, `INSERT OR IGNORE`) and
    /// SQLite serialises writers anyway, so two migrators racing converge on
    /// the same schema instead of conflicting.
    pub(crate) const LOCK: Option<&str> = None;

    /// See [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> = None;

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/sqlite/0001_jobs.sql");
}

// SQLite reaches the same guarantee from the opposite direction: the other two
// dialects let claimers past each other, SQLite refuses to let them overlap at
// all. These tests need a SQLite file, which every machine has, but they still
// go through the same fixture as the other two so that the URL comes from the
// same place and the safety check applies; see `crate::jobs::test_support`.
#[cfg(all(test, feature = "test-kit"))]
mod tests {
    use crate::jobs::test_support::{
        JOBS, WORKERS, assert_claimed_exactly_once, drain_concurrently, enqueue, queue,
    };

    /// The property, with exclusion instead of skipping.
    ///
    /// `BEGIN IMMEDIATE` takes the database write lock before the pick, so no
    /// second claimer can read the rows this one is about to mark. A deferred
    /// `BEGIN` would take that lock only at the first write -- after the pick
    /// -- and two claimers could read the same head of the queue before either
    /// wrote. That is the failure this asserts is absent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn every_job_goes_to_exactly_one_worker() {
        let Some(fixture) = queue().await else {
            return;
        };
        let pool = fixture.pool();

        let enqueued = enqueue(pool, JOBS).await;
        let claimed = drain_concurrently(pool, (JOBS / WORKERS) as i64).await;

        assert_claimed_exactly_once(pool, &enqueued, &claimed).await;
    }

    /// Contention is waited out, not failed.
    ///
    /// The cost of `BEGIN IMMEDIATE` is that claimers serialise: seven of the
    /// eight are blocked on the write lock at any moment, and SQLite's answer
    /// to a blocked writer is `SQLITE_BUSY` unless `busy_timeout` says to
    /// wait. `SESSION_SETUP` sets it, and this is what proves the setting is
    /// actually reaching the connection -- `drain_concurrently` propagates a
    /// claim error rather than treating it as an empty batch, so a lost
    /// `PRAGMA` fails here instead of quietly halving the throughput.
    ///
    /// A batch of one maximises the number of transactions, and so the number
    /// of chances to collide: forty claims across eight connections, each one
    /// a separate exclusive write transaction.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn claimers_wait_for_the_write_lock_rather_than_failing() {
        let Some(fixture) = queue().await else {
            return;
        };
        let pool = fixture.pool();

        let enqueued = enqueue(pool, JOBS).await;
        let claimed = drain_concurrently(pool, 1).await;

        assert_claimed_exactly_once(pool, &enqueued, &claimed).await;
    }
}
