//! Job enqueue: `JobRequest<J>`, `EnqueuedJob`, `JobStatus`, and the
//! `insert_job` function used by the `Jobs` facade.

use std::marker::PhantomData;

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use super::config::JobModel;
use super::dialect::{JobDb, sql, stored_time};
use super::error::EnqueueError;
use super::validate::validate_kind;

// ---------------------------------------------------------------------------
// JobStatus
// ---------------------------------------------------------------------------

/// The lifecycle status of an enqueued job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Running,
    Succeeded,
    Dead,
    Cancelled,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => f.write_str("pending"),
            Self::Running => f.write_str("running"),
            Self::Succeeded => f.write_str("succeeded"),
            Self::Dead => f.write_str("dead"),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl JobStatus {
    /// Parse a status string from the database.
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "dead" => Some(Self::Dead),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// EnqueuedJob
// ---------------------------------------------------------------------------

/// The result of enqueueing a job: the new row's id and status.
#[derive(Debug, Clone, Copy)]
pub struct EnqueuedJob {
    /// The UUID of the newly inserted job row.
    pub id: Uuid,
    /// The status (always `Pending` on a fresh enqueue).
    pub status: JobStatus,
}

// ---------------------------------------------------------------------------
// JobRequest<J>
// ---------------------------------------------------------------------------

/// A request to enqueue a job of type `J`.
///
/// Built via [`JobRequest::new`] from a [`JobModel<J>`] and a payload, then
/// optionally configured with [`run_at`](Self::run_at), [`delay`](Self::delay),
/// and [`max_attempts`](Self::max_attempts) before passing to
/// [`Jobs::enqueue`](super::Jobs::enqueue).
pub struct JobRequest<J> {
    kind: &'static str,
    version: i16,
    max_attempts_default: u32,
    max_attempts_override: Option<u32>,
    payload: serde_json::Value,
    run_at: Option<DateTime<Utc>>,
    _job: PhantomData<J>,
}

impl<J> JobRequest<J> {
    /// Create a new enqueue request. The payload is serialized and size-checked
    /// before any database round-trip.
    pub fn new(model: &JobModel<J>, payload: &J) -> Result<Self, EnqueueError>
    where
        J: Serialize + serde::de::DeserializeOwned,
    {
        model
            .validate_identity()
            .map_err(EnqueueError::invalid_kind)?;

        // Also validate the kind via the shared rules (the model stores a
        // &'static str, but we re-check in case of a hand-built model).
        validate_kind(model.kind()).map_err(EnqueueError::invalid_kind)?;

        let payload = serde_json::to_value(payload).map_err(EnqueueError::serialize)?;
        let size = serde_json::to_vec(&payload)
            .map_err(EnqueueError::serialize)?
            .len();
        if size > model.max_payload_bytes() {
            return Err(EnqueueError::payload_too_large(
                size,
                model.max_payload_bytes(),
            ));
        }
        Ok(Self {
            kind: model.kind(),
            version: model.version(),
            max_attempts_default: model.max_attempts(),
            max_attempts_override: None,
            payload,
            run_at: None,
            _job: PhantomData,
        })
    }

    /// Schedule the job to run at a specific time (UTC). When set, the job is
    /// not claimable until `run_at`.
    pub fn run_at(mut self, run_at: DateTime<Utc>) -> Self {
        self.run_at = Some(run_at);
        self
    }

    /// Schedule the job to run after a delay from now.
    pub fn delay(self, delay: std::time::Duration) -> Self {
        self.run_at(Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default())
    }

    /// Override the model's default max attempts (floored to 1).
    pub fn max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts_override = Some(if attempts < 1 { 1 } else { attempts });
        self
    }

    /// The job kind string.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// The payload version.
    pub fn version(&self) -> i16 {
        self.version
    }

    /// The serialized payload.
    pub fn payload(&self) -> &serde_json::Value {
        &self.payload
    }

    /// The scheduled run time, if set.
    pub fn run_at_ts(&self) -> Option<DateTime<Utc>> {
        self.run_at
    }

    /// The effective max attempts (override or model default).
    pub fn effective_max_attempts(&self) -> u32 {
        self.max_attempts_override
            .unwrap_or(self.max_attempts_default)
    }
}

// ---------------------------------------------------------------------------
// insert_job
// ---------------------------------------------------------------------------

/// Insert a job row into `arcature_jobs`.
///
/// The id is generated here rather than by the database. Only PostgreSQL and
/// SQLite could return a server-generated id from an `INSERT`; MySQL has no
/// `RETURNING`, and a second `SELECT` to recover the id would be a round trip
/// bought for nothing. Generating it client-side also drops the PostgreSQL
/// `gen_random_uuid()` default, and with it the pgcrypto dependency.
///
/// A job with no `run_at` is available immediately, and `run_at` and
/// `available_at` start equal; the worker only ever reads `available_at`,
/// and a retry moves that one alone so `run_at` keeps recording what was
/// originally asked for.
pub(crate) async fn insert_job<'c, E>(
    executor: E,
    request: &JobRequest<impl Serialize + serde::de::DeserializeOwned>,
) -> Result<EnqueuedJob, EnqueueError>
where
    E: sqlx::Executor<'c, Database = JobDb>,
{
    let id = Uuid::new_v4();
    let max_attempts: i32 = request.effective_max_attempts().min(i32::MAX as u32) as i32;
    let run_at = stored_time(request.run_at_ts().unwrap_or_else(Utc::now));

    sqlx::query(sql::INSERT)
        .bind(id)
        .bind(request.kind())
        .bind(request.version())
        .bind(sqlx::types::Json(request.payload()))
        .bind(max_attempts)
        .bind(run_at)
        .bind(run_at)
        .execute(executor)
        .await
        .map_err(EnqueueError::database)?;

    Ok(EnqueuedJob {
        id,
        status: JobStatus::Pending,
    })
}
