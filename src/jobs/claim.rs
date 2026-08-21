//! The claim: how a worker takes exclusive ownership of a batch of jobs.
//!
//! The claim is the one place where the three dialects genuinely disagree, so
//! it has two implementations behind one signature. See
//! [`crate::jobs::dialect`] for why. Both produce the same thing: rows moved
//! to `running`, `attempts` incremented, and a fresh per-batch `claim_token`
//! that every later mutation fences on.

use std::time::Duration;

use sqlx::Row;
use uuid::Uuid;

use super::dialect::{JobPool, sql};
use super::error::WorkerError;

/// A job row claimed by a worker. The `claim_token` is a per-claim fencing
/// UUID; every completion mutation fences on
/// `(id, status = 'running', claim_token)`.
#[derive(Debug, Clone)]
pub struct ClaimedJob {
    /// The job row id.
    pub id: Uuid,
    /// The per-claim fencing token.
    pub claim_token: Uuid,
    /// The job kind.
    pub kind: String,
    /// The payload version.
    pub version: i16,
    /// The serialized payload.
    pub payload: serde_json::Value,
    /// The post-increment attempt count (the claim already did `+ 1`).
    pub attempts: i32,
    /// The max attempts for this job.
    pub max_attempts: i32,
    /// The lease duration in seconds.
    pub lease_seconds: i32,
}

/// Clamp a lease to what the `lease_seconds` column can hold.
fn lease_seconds(lease: Duration) -> i32 {
    lease.as_secs().min(i32::MAX as u64) as i32
}

/// Claim a batch of pending jobs.
///
/// PostgreSQL does it in one statement: an `UPDATE ... RETURNING` over a
/// `FOR UPDATE SKIP LOCKED` subquery. The implicit statement transaction is
/// enough -- a row another claimer holds is skipped, not waited on, and the
/// claimed rows come back in the same round trip.
///
/// `attempts` is incremented at claim time, so [`ClaimedJob::attempts`] is
/// the post-increment count.
#[cfg(feature = "db-postgres")]
pub async fn claim_jobs(
    pool: &JobPool,
    worker_id: &str,
    lease: Duration,
    batch: i64,
) -> Result<Vec<ClaimedJob>, WorkerError> {
    let claim_token = Uuid::new_v4();
    let rows = sqlx::query(sql::CLAIM)
        .bind(worker_id)
        .bind(claim_token)
        .bind(lease_seconds(lease))
        .bind(batch)
        .fetch_all(pool)
        .await?;

    let mut claimed = Vec::with_capacity(rows.len());
    for row in &rows {
        claimed.push(ClaimedJob {
            id: row.try_get("id")?,
            claim_token: row.try_get("claim_token")?,
            kind: row.try_get("kind")?,
            version: row.try_get("version")?,
            payload: row.try_get("payload")?,
            attempts: row.try_get("attempts")?,
            max_attempts: row.try_get("max_attempts")?,
            lease_seconds: row.try_get("lease_seconds")?,
        });
    }
    Ok(claimed)
}

/// Claim a batch of pending jobs.
///
/// SQLite and MySQL both need an explicit transaction, for opposite reasons,
/// and the resulting shape is the same: pick the rows, then mark each one.
///
/// * MySQL 8 has `SKIP LOCKED` but no `RETURNING`, so the pick is a locking
///   `SELECT` and the mark is a separate `UPDATE`. The rows are already
///   locked by this transaction, so the marks cannot contend.
/// * SQLite has neither, and does not need them: `BEGIN IMMEDIATE` takes the
///   database write lock before the pick, so no other claimer can be reading
///   or writing these rows at all. There is nothing to skip because there is
///   no concurrent writer to skip past.
///
/// A row whose mark affects zero rows was taken between the pick and the mark
/// (possible only where the pick does not lock) and is dropped from the
/// batch rather than reported as claimed.
///
/// `attempts` is incremented at claim time, so [`ClaimedJob::attempts`] is
/// the post-increment count.
#[cfg(any(feature = "db-sqlite", feature = "db-mysql"))]
pub async fn claim_jobs(
    pool: &JobPool,
    worker_id: &str,
    lease: Duration,
    batch: i64,
) -> Result<Vec<ClaimedJob>, WorkerError> {
    use sqlx::Connection;

    let claim_token = Uuid::new_v4();
    let lease_seconds = lease_seconds(lease);

    let mut conn = pool.acquire().await?;
    if let Some(setup) = sql::SESSION_SETUP {
        sqlx::query(setup).execute(&mut *conn).await?;
    }
    let mut tx = conn.begin_with(sql::BEGIN).await?;

    let picked = sqlx::query(sql::CLAIM_PICK)
        .bind(batch)
        .fetch_all(&mut *tx)
        .await?;

    let mut claimed = Vec::with_capacity(picked.len());
    for row in &picked {
        let id: Uuid = row.try_get("id")?;
        let marked = sqlx::query(sql::CLAIM_MARK)
            .bind(worker_id)
            .bind(claim_token)
            .bind(lease_seconds)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if marked == 0 {
            continue;
        }
        claimed.push(ClaimedJob {
            id,
            claim_token,
            kind: row.try_get("kind")?,
            version: row.try_get("version")?,
            payload: row.try_get("payload")?,
            // The mark did `attempts + 1`; the picked row is pre-increment.
            attempts: row.try_get::<i32, _>("attempts")? + 1,
            max_attempts: row.try_get("max_attempts")?,
            lease_seconds,
        });
    }

    tx.commit().await?;
    Ok(claimed)
}
