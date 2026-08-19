//! Completion decision logic and the fenced persistence mutations.
//!
//! Every completion mutation fences on `id = $1 AND status = 'running' AND
//! claim_token = $2`. A stale worker (lease expired, sweep requeued, another
//! worker reclaimed with a fresh token) affects zero rows, so the stale
//! observer event is suppressed (not emitted). This is genuine fencing, not
//! time comparison.

use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::postgres::PgExecutor;
use uuid::Uuid;

use super::config::RetryPolicy;
use super::error::{JobError, WorkerError, truncate_for_storage};

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// The state passed to `decide`: the attempt count and the max attempts.
#[derive(Debug, Clone)]
pub(crate) struct JobState {
    pub attempts: i32,
    pub max_attempts: i32,
}

/// The outcome of running the handler (before the completion decision).
#[derive(Debug)]
pub(crate) enum DispatchOutcome {
    Succeeded,
    /// The payload did not deserialize to the registered schema (poison job).
    Malformed,
    /// The job exceeded its per-attempt timeout.
    Timeout,
    /// The handler returned an error.
    HandlerError(JobError),
}

/// The decision made by `decide`.
#[derive(Debug, Clone)]
pub(crate) enum Outcome {
    Succeeded,
    Retry {
        available_at: DateTime<Utc>,
    },
    Dead {
        error_kind: ErrorKind,
        message: String,
    },
}

/// The kind of permanent failure.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ErrorKind {
    Permanent,
    Exhausted,
    Malformed,
    Unknown,
    Panic,
    Timeout,
}

impl ErrorKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Permanent => "permanent",
            Self::Exhausted => "exhausted",
            Self::Malformed => "malformed",
            Self::Unknown => "unknown",
            Self::Panic => "panic",
            Self::Timeout => "timeout",
        }
    }
}

/// Whether a completion transition updated the row or was fenced out (lost).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimTransition {
    Updated,
    Lost,
}

impl ClaimTransition {
    fn from_affected(rows: u64) -> Self {
        if rows > 0 { Self::Updated } else { Self::Lost }
    }
}

// ---------------------------------------------------------------------------
// decide — the pure completion decision
// ---------------------------------------------------------------------------

/// Decide the outcome of a dispatch. Pure: no I/O, no mutation.
///
/// `available_at` for a retry is `now + policy.delay_for(attempts)`.
pub(crate) fn decide(
    state: &JobState,
    dispatch: &DispatchOutcome,
    policy: &RetryPolicy,
    now: DateTime<Utc>,
) -> Outcome {
    match dispatch {
        DispatchOutcome::Succeeded => Outcome::Succeeded,
        DispatchOutcome::Malformed => Outcome::Dead {
            error_kind: ErrorKind::Malformed,
            message: "payload did not deserialize to the registered schema".to_string(),
        },
        DispatchOutcome::Timeout => {
            if state.attempts >= state.max_attempts {
                Outcome::Dead {
                    error_kind: ErrorKind::Timeout,
                    message: format!(
                        "job exceeded its per-attempt timeout after {} attempt(s)",
                        state.attempts
                    ),
                }
            } else {
                Outcome::Retry {
                    available_at: now
                        + chrono::Duration::from_std(policy.delay_for(state.attempts as u32))
                            .unwrap_or_default(),
                }
            }
        }
        DispatchOutcome::HandlerError(err) => {
            let message = err.stored_message();
            if err.is_permanent() {
                Outcome::Dead {
                    error_kind: ErrorKind::Permanent,
                    message,
                }
            } else if state.attempts >= state.max_attempts {
                Outcome::Dead {
                    error_kind: ErrorKind::Exhausted,
                    message,
                }
            } else {
                Outcome::Retry {
                    available_at: now
                        + chrono::Duration::from_std(policy.delay_for(state.attempts as u32))
                            .unwrap_or_default(),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Persistence — fenced mutations
// ---------------------------------------------------------------------------

/// Mark a job as succeeded. Fenced on the claim token.
pub(crate) async fn mark_succeeded(
    executor: impl for<'e> PgExecutor<'e>,
    job_id: Uuid,
    claim_token: Uuid,
) -> Result<ClaimTransition, WorkerError> {
    let rows = sqlx::query(
        r#"UPDATE arcature_jobs
           SET status = 'succeeded',
               locked_by = NULL,
               locked_at = NULL,
               claim_token = NULL,
               last_error = NULL,
               last_error_kind = NULL,
               failed_at = NULL
           WHERE id = $1 AND status = 'running' AND claim_token = $2"#,
    )
    .bind(job_id)
    .bind(claim_token)
    .execute(executor)
    .await?
    .rows_affected();
    Ok(ClaimTransition::from_affected(rows))
}

/// Mark a job for retry. Fenced on the claim token. The message is
/// pre-truncated.
pub(crate) async fn mark_retry(
    executor: impl for<'e> PgExecutor<'e>,
    job_id: Uuid,
    claim_token: Uuid,
    available_at: DateTime<Utc>,
    message: String,
) -> Result<ClaimTransition, WorkerError> {
    let message = truncate_for_storage(&message);
    let rows = sqlx::query(
        r#"UPDATE arcature_jobs
           SET status = 'pending',
               available_at = $3,
               locked_by = NULL,
               locked_at = NULL,
               claim_token = NULL,
               last_error = $4,
               last_error_kind = 'retryable',
               failed_at = NULL
           WHERE id = $1 AND status = 'running' AND claim_token = $2"#,
    )
    .bind(job_id)
    .bind(claim_token)
    .bind(available_at)
    .bind(message)
    .execute(executor)
    .await?
    .rows_affected();
    Ok(ClaimTransition::from_affected(rows))
}

/// Mark a job as dead. Fenced on the claim token. The message is
/// pre-truncated.
pub(crate) async fn mark_dead(
    executor: impl for<'e> PgExecutor<'e>,
    job_id: Uuid,
    claim_token: Uuid,
    error_kind: ErrorKind,
    message: String,
) -> Result<ClaimTransition, WorkerError> {
    let message = truncate_for_storage(&message);
    let rows = sqlx::query(
        r#"UPDATE arcature_jobs
           SET status = 'dead',
               locked_by = NULL,
               locked_at = NULL,
               claim_token = NULL,
               last_error = $3,
               last_error_kind = $4,
               failed_at = now()
           WHERE id = $1 AND status = 'running' AND claim_token = $2"#,
    )
    .bind(job_id)
    .bind(claim_token)
    .bind(message)
    .bind(error_kind.as_str())
    .execute(executor)
    .await?
    .rows_affected();
    Ok(ClaimTransition::from_affected(rows))
}

/// Refresh the lease for a running job. Returns `true` if the lease was
/// refreshed (claim still owned), `false` if the claim was lost (lease expired,
/// sweep requeued to another worker).
pub(crate) async fn heartbeat(
    executor: impl for<'e> PgExecutor<'e>,
    job_id: Uuid,
    claim_token: Uuid,
    lease: Duration,
) -> Result<bool, WorkerError> {
    let lease_seconds = lease.as_secs().min(i32::MAX as u64) as i32;
    let rows = sqlx::query(
        r#"UPDATE arcature_jobs
           SET locked_at = now(),
               lease_seconds = $3
           WHERE id = $1 AND status = 'running' AND claim_token = $2"#,
    )
    .bind(job_id)
    .bind(claim_token)
    .bind(lease_seconds)
    .execute(executor)
    .await?
    .rows_affected();
    Ok(rows > 0)
}

/// Sweep expired leases. Expired-lease rows with `attempts >= max_attempts` go
/// `dead` (`exhausted`); those with attempts remaining go back to `pending`.
/// Returns the total number of rows affected (dead + requeued).
pub async fn sweep_expired_leases(pool: &sqlx::PgPool, batch: i64) -> Result<u64, WorkerError> {
    // Mark dead the expired-lease rows that have exhausted their attempts.
    let dead = sqlx::query(
        r#"UPDATE arcature_jobs
           SET status = 'dead',
               locked_by = NULL,
               locked_at = NULL,
               claim_token = NULL,
               last_error = 'job exhausted its max_attempts after a crash',
               last_error_kind = 'exhausted',
               failed_at = now()
           WHERE id IN (
             SELECT id FROM arcature_jobs
             WHERE status = 'running'
               AND locked_at IS NOT NULL
               AND locked_at + (lease_seconds || ' seconds')::interval < now()
               AND attempts >= max_attempts
             LIMIT $1
             FOR UPDATE SKIP LOCKED
           )"#,
    )
    .bind(batch)
    .execute(pool)
    .await?
    .rows_affected();

    // Requeue the expired-lease rows that still have attempts remaining.
    let requeued = sqlx::query(
        r#"UPDATE arcature_jobs
           SET status = 'pending',
               available_at = now(),
               locked_by = NULL,
               locked_at = NULL,
               claim_token = NULL
           WHERE id IN (
             SELECT id FROM arcature_jobs
             WHERE status = 'running'
               AND locked_at IS NOT NULL
               AND locked_at + (lease_seconds || ' seconds')::interval < now()
               AND attempts < max_attempts
             LIMIT $1
             FOR UPDATE SKIP LOCKED
           )"#,
    )
    .bind(batch)
    .execute(pool)
    .await?
    .rows_affected();

    Ok(dead + requeued)
}

// ---------------------------------------------------------------------------
// Admin operations (public, not used by the loop)
// ---------------------------------------------------------------------------

/// Cancel a job (set status to `cancelled` for `pending` or `running` rows).
/// Returns the number of rows affected.
pub async fn cancel(
    executor: impl for<'e> PgExecutor<'e>,
    job_id: Uuid,
) -> Result<u64, WorkerError> {
    let rows = sqlx::query(
        r#"UPDATE arcature_jobs
           SET status = 'cancelled',
               locked_by = NULL,
               locked_at = NULL,
               claim_token = NULL
           WHERE id = $1 AND status IN ('pending', 'running')"#,
    )
    .bind(job_id)
    .execute(executor)
    .await?
    .rows_affected();
    Ok(rows)
}

/// Requeue a dead job: reset attempts to 0, set status to `pending`, clear
/// error and failed_at. Returns the number of rows affected.
pub async fn requeue_dead(
    executor: impl for<'e> PgExecutor<'e>,
    job_id: Uuid,
) -> Result<u64, WorkerError> {
    let rows = sqlx::query(
        r#"UPDATE arcature_jobs
           SET status = 'pending',
               attempts = 0,
               available_at = now(),
               locked_by = NULL,
               locked_at = NULL,
               claim_token = NULL,
               last_error = NULL,
               last_error_kind = NULL,
               failed_at = NULL
           WHERE id = $1 AND status = 'dead'"#,
    )
    .bind(job_id)
    .execute(executor)
    .await?
    .rows_affected();
    Ok(rows)
}
