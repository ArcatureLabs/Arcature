//! Completion decision logic and the fenced persistence mutations.
//!
//! Every completion mutation fences on `id = ? AND status = 'running' AND
//! claim_token = ?`. A stale worker (lease expired, sweep requeued, another
//! worker reclaimed with a fresh token) affects zero rows, so the stale
//! observer event is suppressed (not emitted). This is genuine fencing, not
//! time comparison.

use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::config::RetryPolicy;
use super::dialect::{JobDb, JobPool, sql, stored_time};
use super::error::{JobError, WorkerError, truncate_for_storage};

/// What every fenced mutation below accepts: the application's pool, or a
/// connection borrowed from it. Spelled out once so the mutations read the
/// same in all three dialects.
pub trait Fenced<'e>: sqlx::Executor<'e, Database = JobDb> {}
impl<'e, E: sqlx::Executor<'e, Database = JobDb>> Fenced<'e> for E {}

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
    executor: impl for<'e> Fenced<'e>,
    job_id: Uuid,
    claim_token: Uuid,
) -> Result<ClaimTransition, WorkerError> {
    let rows = sqlx::query(sql::MARK_SUCCEEDED)
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
    executor: impl for<'e> Fenced<'e>,
    job_id: Uuid,
    claim_token: Uuid,
    available_at: DateTime<Utc>,
    message: String,
) -> Result<ClaimTransition, WorkerError> {
    let message = truncate_for_storage(&message);
    let rows = sqlx::query(sql::MARK_RETRY)
        .bind(stored_time(available_at))
        .bind(message)
        .bind(job_id)
        .bind(claim_token)
        .execute(executor)
        .await?
        .rows_affected();
    Ok(ClaimTransition::from_affected(rows))
}

/// Mark a job as dead. Fenced on the claim token. The message is
/// pre-truncated.
pub(crate) async fn mark_dead(
    executor: impl for<'e> Fenced<'e>,
    job_id: Uuid,
    claim_token: Uuid,
    error_kind: ErrorKind,
    message: String,
) -> Result<ClaimTransition, WorkerError> {
    let message = truncate_for_storage(&message);
    let rows = sqlx::query(sql::MARK_DEAD)
        .bind(message)
        .bind(error_kind.as_str())
        .bind(job_id)
        .bind(claim_token)
        .execute(executor)
        .await?
        .rows_affected();
    Ok(ClaimTransition::from_affected(rows))
}

/// Refresh the lease for a running job. Returns `true` if the lease was
/// refreshed (claim still owned), `false` if the claim was lost (lease expired,
/// sweep requeued to another worker).
pub(crate) async fn heartbeat(
    executor: impl for<'e> Fenced<'e>,
    job_id: Uuid,
    claim_token: Uuid,
    lease: Duration,
) -> Result<bool, WorkerError> {
    let lease_seconds = lease.as_secs().min(i32::MAX as u64) as i32;
    let rows = sqlx::query(sql::HEARTBEAT)
        .bind(lease_seconds)
        .bind(job_id)
        .bind(claim_token)
        .execute(executor)
        .await?
        .rows_affected();
    Ok(rows > 0)
}

/// Sweep expired leases. Expired-lease rows with `attempts >= max_attempts`
/// go `dead` (`exhausted`); those with attempts remaining go back to
/// `pending`. Returns the total number of rows affected (dead + requeued).
///
/// This is what makes a crashed worker's jobs claimable again: the crashed
/// process never releases anything, so recovery is entirely a function of the
/// lease having elapsed on the database's own clock.
pub async fn sweep_expired_leases(pool: &JobPool, batch: i64) -> Result<u64, WorkerError> {
    let dead = sqlx::query(sql::SWEEP_DEAD)
        .bind(batch)
        .execute(pool)
        .await?
        .rows_affected();

    let requeued = sqlx::query(sql::SWEEP_REQUEUE)
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
pub async fn cancel(executor: impl for<'e> Fenced<'e>, job_id: Uuid) -> Result<u64, WorkerError> {
    let rows = sqlx::query(sql::CANCEL)
        .bind(job_id)
        .execute(executor)
        .await?
        .rows_affected();
    Ok(rows)
}

/// Requeue a dead job: reset attempts to 0, set status to `pending`, clear
/// error and failed_at. Returns the number of rows affected.
pub async fn requeue_dead(
    executor: impl for<'e> Fenced<'e>,
    job_id: Uuid,
) -> Result<u64, WorkerError> {
    let rows = sqlx::query(sql::REQUEUE_DEAD)
        .bind(job_id)
        .execute(executor)
        .await?
        .rows_affected();
    Ok(rows)
}

// The fencing above is one `WHERE` clause, repeated across four statements in
// each of the three dialect files, and evaluated by the database rather than
// by the worker. Reading the code cannot tell you whether it matches nothing
// at the moment it must: only the server can. These tests need one; see
// `crate::jobs::test_support` for how they skip without one.
#[cfg(all(test, feature = "test-kit"))]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::jobs::test_support::{enqueue, queue, row};

    /// A lease short enough to expire inside a test. The schema's
    /// `lease_seconds >= 1` check is the floor.
    const SHORT_LEASE: Duration = Duration::from_secs(1);

    /// Long enough that the server's clock has passed `locked_at + 1s`
    /// however the two disagree about the sub-second part.
    const PAST_THE_LEASE: Duration = Duration::from_millis(1_600);

    /// Produce the situation fencing exists for: a job whose original worker
    /// still believes it owns the claim, and a second worker that actually
    /// does.
    ///
    /// The zombie is made the way production makes one -- the lease elapses
    /// and the sweep requeues the job -- rather than by inventing a token.
    /// A fabricated token would prove the `WHERE` clause rejects a value that
    /// was never in the column, which is not the failure that happens.
    async fn zombie_and_owner(
        pool: &JobPool,
    ) -> (
        /* job */ Uuid,
        /* zombie */ Uuid,
        /* owner */ Uuid,
    ) {
        let enqueued = enqueue(pool, 1).await;
        let job = enqueued[0];

        let first = crate::jobs::admin::claim_jobs(pool, "zombie", SHORT_LEASE, 1)
            .await
            .expect("the first claim");
        assert_eq!(first.len(), 1, "the only pending job was not claimed");
        let zombie = first[0].claim_token;

        tokio::time::sleep(PAST_THE_LEASE).await;
        let swept = sweep_expired_leases(pool, 10)
            .await
            .expect("sweep expired leases");
        assert_eq!(swept, 1, "the expired lease was not swept");

        let second = crate::jobs::admin::claim_jobs(pool, "owner", Duration::from_secs(60), 1)
            .await
            .expect("the second claim");
        assert_eq!(second.len(), 1, "the requeued job was not claimable again");
        let owner = second[0].claim_token;

        assert_ne!(zombie, owner, "the reclaim reused the token it fences on");
        (job, zombie, owner)
    }

    /// The zombie's success is refused, and the owner's is not.
    ///
    /// This is the whole point of the token. Without it the second worker's
    /// run would be overwritten by the first worker's stale result -- the job
    /// would be marked succeeded by a process whose work nobody waited for,
    /// and the row would look identical to a correct completion.
    #[tokio::test]
    async fn a_zombie_worker_cannot_complete_a_job_it_no_longer_owns() {
        let Some(fixture) = queue().await else {
            return;
        };
        let pool = fixture.pool();
        let (job, zombie, owner) = zombie_and_owner(pool).await;

        let refused = mark_succeeded(pool, job, zombie)
            .await
            .expect("run the fenced update");
        assert_eq!(
            refused,
            ClaimTransition::Lost,
            "a stale worker completed a job it had lost"
        );

        let (status, attempts, token) = row(pool, job).await;
        assert_eq!(status, "running", "the row left the owner's hands");
        assert_eq!(attempts, 2, "the reclaim did not count as an attempt");
        assert_eq!(
            token,
            Some(owner),
            "the row is no longer fenced to the owner"
        );

        let accepted = mark_succeeded(pool, job, owner)
            .await
            .expect("run the fenced update");
        assert_eq!(
            accepted,
            ClaimTransition::Updated,
            "the current owner was fenced out of its own job"
        );

        let (status, _, token) = row(pool, job).await;
        assert_eq!(status, "succeeded");
        assert_eq!(token, None, "a finished job kept its claim token");
    }

    /// Every completion path is fenced, not only the successful one.
    ///
    /// A retry or a death written by a zombie is worse than a stale success:
    /// it moves a job another worker is actively running back to `pending`,
    /// so the same work runs a third time, or buries it in `dead` while the
    /// owner is about to report that it worked.
    #[tokio::test]
    async fn a_zombie_worker_cannot_retry_kill_or_heartbeat_a_lost_job() {
        let Some(fixture) = queue().await else {
            return;
        };
        let pool = fixture.pool();
        let (job, zombie, owner) = zombie_and_owner(pool).await;

        let retried = mark_retry(pool, job, zombie, Utc::now(), "stale retry".to_owned())
            .await
            .expect("run the fenced update");
        assert_eq!(
            retried,
            ClaimTransition::Lost,
            "a stale worker requeued a job"
        );

        let killed = mark_dead(
            pool,
            job,
            zombie,
            ErrorKind::Unknown,
            "stale death".to_owned(),
        )
        .await
        .expect("run the fenced update");
        assert_eq!(killed, ClaimTransition::Lost, "a stale worker killed a job");

        let refreshed = heartbeat(pool, job, zombie, Duration::from_secs(600))
            .await
            .expect("run the fenced update");
        assert!(
            !refreshed,
            "a stale worker extended a lease it did not hold"
        );

        // None of the three may have touched the row.
        let (status, attempts, token) = row(pool, job).await;
        assert_eq!(status, "running");
        assert_eq!(attempts, 2);
        assert_eq!(token, Some(owner));

        // And the owner's own heartbeat still works, so the assertions above
        // are about the token rather than about the statements being broken.
        let refreshed = heartbeat(pool, job, owner, Duration::from_secs(600))
            .await
            .expect("run the fenced update");
        assert!(refreshed, "the owner could not refresh its own lease");
    }
}
