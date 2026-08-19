//! The SKIP LOCKED claim query and the `ClaimedJob` it returns.

use std::time::Duration;

use sqlx::postgres::PgExecutor;
use sqlx::Row;
use uuid::Uuid;

use super::error::WorkerError;

/// A job row claimed by a worker. The `claim_token` is a per-claim fencing UUID;
/// every completion mutation fences on `(id, status='running', claim_token)`.
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

/// The row shape returned by the claim UPDATE.
struct ClaimRow {
    id: Uuid,
    claim_token: Uuid,
    kind: String,
    version: i16,
    payload: serde_json::Value,
    attempts: i32,
    max_attempts: i32,
    lease_seconds: i32,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for ClaimRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            claim_token: row.try_get("claim_token")?,
            kind: row.try_get("kind")?,
            version: row.try_get("version")?,
            payload: row.try_get("payload")?,
            attempts: row.try_get("attempts")?,
            max_attempts: row.try_get("max_attempts")?,
            lease_seconds: row.try_get("lease_seconds")?,
        })
    }
}

/// Claim a batch of pending jobs using `FOR UPDATE SKIP LOCKED`.
///
/// A single `UPDATE ... RETURNING` over a `FOR UPDATE SKIP LOCKED` subquery
/// (implicit statement transaction). The same `claim_token` UUID is used for
/// the whole batch. `attempts` is incremented at claim time, so
/// [`ClaimedJob::attempts`] is the post-increment count.
pub async fn claim_jobs(
    executor: impl for<'e> PgExecutor<'e>,
    worker_id: &str,
    lease: Duration,
    batch: i64,
) -> Result<Vec<ClaimedJob>, WorkerError> {
    let lease_seconds = lease.as_secs().min(i32::MAX as u64) as i32;
    let claim_token = Uuid::new_v4();
    let rows = sqlx::query_as::<_, ClaimRow>(
        r#"UPDATE arcature_jobs
           SET
             status        = 'running',
             attempts      = attempts + 1,
             locked_by     = $1,
             locked_at     = now(),
             claim_token   = $4,
             lease_seconds = $2,
             last_error    = NULL,
             last_error_kind = NULL
           WHERE id IN (
             SELECT id FROM arcature_jobs
             WHERE status = 'pending'
               AND available_at <= now()
             ORDER BY available_at, id
             LIMIT $3
             FOR UPDATE SKIP LOCKED
           )
           RETURNING id, claim_token, kind, version, payload, attempts, max_attempts, lease_seconds"#,
    )
    .bind(worker_id)
    .bind(lease_seconds)
    .bind(batch)
    .bind(claim_token)
    .fetch_all(executor)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ClaimedJob {
            id: r.id,
            claim_token: r.claim_token,
            kind: r.kind,
            version: r.version,
            payload: r.payload,
            attempts: r.attempts,
            max_attempts: r.max_attempts,
            lease_seconds: r.lease_seconds,
        })
        .collect())
}
