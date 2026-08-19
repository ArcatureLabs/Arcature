//! Job subsystem error types.
//!
//! Each error is a typed enum (no raw `String` errors). The handler-returned
//! [`JobError`] carries the retry classification the handler decides; the
//! framework never reclassifies.

use std::fmt;

use sqlx::Error as SqlxError;

// ---------------------------------------------------------------------------
// JobError — the error a handler returns.
// ---------------------------------------------------------------------------

/// The error a job handler returns.
///
/// The handler, not the framework, decides whether a failure is retryable.
/// The framework respects that classification: [`Retryable`](Self::Retryable)
/// is retried per backoff (bounded by `max_attempts`); [`Permanent`](Self::Permanent)
/// is marked dead immediately.
#[derive(Debug)]
pub enum JobError {
    /// Transient: retry per backoff, bounded by `max_attempts`; dead when exhausted.
    Retryable(Box<dyn std::error::Error + Send + Sync + 'static>),
    /// Permanent: dead immediately, never retried.
    Permanent(Box<dyn std::error::Error + Send + Sync + 'static>),
}

impl JobError {
    /// Wrap a retryable error.
    pub fn retryable<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Retryable(Box::new(error))
    }

    /// Wrap a permanent error.
    pub fn permanent<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Permanent(Box::new(error))
    }

    /// A retryable error from a displayable message.
    pub fn retryable_msg<M>(message: M) -> Self
    where
        M: Into<String>,
    {
        Self::Retryable(Box::new(MessageError(message.into())))
    }

    /// A permanent error from a displayable message.
    pub fn permanent_msg<M>(message: M) -> Self
    where
        M: Into<String>,
    {
        Self::Permanent(Box::new(MessageError(message.into())))
    }

    /// Whether this is a retryable error.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable(_))
    }

    /// Whether this is a permanent error.
    pub fn is_permanent(&self) -> bool {
        matches!(self, Self::Permanent(_))
    }

    /// The message stored in `last_error` (truncated to 4096 bytes on a UTF-8
    /// char boundary). The payload is never part of this message.
    pub(crate) fn stored_message(&self) -> String {
        truncate_for_storage(&self.to_string())
    }
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Retryable(inner) => write!(f, "retryable: {inner}"),
            Self::Permanent(inner) => write!(f, "permanent: {inner}"),
        }
    }
}

impl std::error::Error for JobError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Retryable(inner) | Self::Permanent(inner) => Some(inner.as_ref()),
        }
    }
}

/// A simple string-backed error used by [`JobError::retryable_msg`] and
/// [`JobError::permanent_msg`].
#[derive(Debug)]
pub struct MessageError(pub String);

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MessageError {}

/// The maximum number of bytes stored in `last_error`.
pub(crate) const LAST_ERROR_MAX_BYTES: usize = 4096;

/// Truncate a string for storage in `last_error`, on a UTF-8 char boundary,
/// with an ellipsis.
pub(crate) fn truncate_for_storage(value: &str) -> String {
    if value.len() <= LAST_ERROR_MAX_BYTES {
        return value.to_string();
    }
    let mut end = LAST_ERROR_MAX_BYTES;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = String::from(&value[..end]);
    truncated.push_str("...");
    truncated
}

// ---------------------------------------------------------------------------
// EnqueueError
// ---------------------------------------------------------------------------

/// An error from enqueueing a job.
#[derive(Debug)]
pub enum EnqueueError {
    /// The event payload could not be serialized.
    Serialize {
        source: serde_json::Error,
    },
    /// The payload exceeded the size limit.
    PayloadTooLarge {
        size: usize,
        limit: usize,
    },
    /// The job kind was invalid.
    InvalidKind {
        reason: String,
    },
    /// A database error during the insert.
    Database {
        source: SqlxError,
    },
}

impl EnqueueError {
    pub(crate) fn serialize(source: serde_json::Error) -> Self {
        Self::Serialize { source }
    }
    pub(crate) fn payload_too_large(size: usize, limit: usize) -> Self {
        Self::PayloadTooLarge { size, limit }
    }
    pub(crate) fn invalid_kind(reason: impl Into<String>) -> Self {
        Self::InvalidKind {
            reason: reason.into(),
        }
    }
    pub(crate) fn database(source: SqlxError) -> Self {
        Self::Database { source }
    }
}

impl fmt::Display for EnqueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize { source } => write!(f, "job payload serialization failed: {source}"),
            Self::PayloadTooLarge { size, limit } => {
                write!(f, "job payload too large: {size} bytes exceeds {limit}-byte limit")
            }
            Self::InvalidKind { reason } => write!(f, "invalid job kind: {reason}"),
            Self::Database { source } => write!(f, "job enqueue database error: {source}"),
        }
    }
}

impl std::error::Error for EnqueueError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize { source } => Some(source),
            Self::Database { source } => Some(source),
            _ => None,
        }
    }
}

impl From<SqlxError> for EnqueueError {
    fn from(source: SqlxError) -> Self {
        Self::database(source)
    }
}

// ---------------------------------------------------------------------------
// RegisterError
// ---------------------------------------------------------------------------

/// An error from registering a job handler.
#[derive(Debug)]
pub enum RegisterError {
    /// A handler is already registered for this kind and version.
    AlreadyRegistered {
        kind: String,
        version: i16,
    },
    /// The job kind was invalid.
    InvalidKind {
        reason: String,
    },
    /// The payload version was invalid (must be >= 1).
    InvalidVersion {
        version: i16,
    },
}

impl RegisterError {
    pub(crate) fn already_registered(kind: impl Into<String>, version: i16) -> Self {
        Self::AlreadyRegistered {
            kind: kind.into(),
            version,
        }
    }
    pub(crate) fn invalid_kind(reason: impl Into<String>) -> Self {
        Self::InvalidKind {
            reason: reason.into(),
        }
    }
    pub(crate) fn invalid_version(version: i16) -> Self {
        Self::InvalidVersion { version }
    }
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRegistered { kind, version } => {
                write!(f, "handler already registered for kind {kind:?} version {version}")
            }
            Self::InvalidKind { reason } => write!(f, "invalid job kind: {reason}"),
            Self::InvalidVersion { version } => {
                write!(f, "invalid payload version {version}: must be >= 1")
            }
        }
    }
}

impl std::error::Error for RegisterError {}

// ---------------------------------------------------------------------------
// WorkerError
// ---------------------------------------------------------------------------

/// An error from the worker lifecycle (not from individual job execution).
///
/// Per-job failures (panic, malformed, unknown, timeout) are not
/// [`WorkerError`] variants; they become dead or retry rows. `WorkerError`
/// is only lifecycle failure (the worker cannot keep running).
#[derive(Debug)]
pub enum WorkerError {
    /// The worker configuration was invalid.
    InvalidConfig {
        source: WorkerConfigError,
    },
    /// The retry policy was invalid.
    InvalidRetryPolicy {
        source: RetryPolicyError,
    },
    /// A database error in the claim/sweep/heartbeat path.
    Database {
        source: SqlxError,
    },
}

impl WorkerError {
    pub(crate) fn database(source: SqlxError) -> Self {
        Self::Database { source }
    }
}

impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig { source } => write!(f, "worker config invalid: {source}"),
            Self::InvalidRetryPolicy { source } => {
                write!(f, "worker retry policy invalid: {source}")
            }
            Self::Database { source } => write!(f, "worker database error: {source}"),
        }
    }
}

impl std::error::Error for WorkerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidConfig { source } => Some(source),
            Self::InvalidRetryPolicy { source } => Some(source),
            Self::Database { source } => Some(source),
        }
    }
}

impl From<SqlxError> for WorkerError {
    fn from(source: SqlxError) -> Self {
        Self::database(source)
    }
}

impl From<WorkerConfigError> for WorkerError {
    fn from(source: WorkerConfigError) -> Self {
        Self::InvalidConfig { source }
    }
}

impl From<RetryPolicyError> for WorkerError {
    fn from(source: RetryPolicyError) -> Self {
        Self::InvalidRetryPolicy { source }
    }
}

// ---------------------------------------------------------------------------
// MigrateError
// ---------------------------------------------------------------------------

/// An error from applying the jobs schema migrations.
#[derive(Debug)]
pub enum MigrateError {
    /// A database error during migration.
    Database {
        source: SqlxError,
    },
}

impl MigrateError {
    pub(crate) fn database(source: SqlxError) -> Self {
        Self::Database { source }
    }
}

impl fmt::Display for MigrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database { source } => write!(f, "arcature_jobs migration failed: {source}"),
        }
    }
}

impl std::error::Error for MigrateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database { source } => Some(source),
        }
    }
}

impl From<SqlxError> for MigrateError {
    fn from(source: SqlxError) -> Self {
        Self::database(source)
    }
}

// ---------------------------------------------------------------------------
// RetryPolicyError
// ---------------------------------------------------------------------------

/// An error from an invalid retry policy.
#[derive(Debug, Clone, PartialEq)]
pub enum RetryPolicyError {
    /// The multiplier is NaN or infinite.
    MultiplierNotFinite {
        multiplier: f64,
    },
    /// The multiplier is negative (would produce a negative delay).
    MultiplierNegative {
        multiplier: f64,
    },
}

impl fmt::Display for RetryPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MultiplierNotFinite { multiplier } => {
                write!(f, "retry multiplier ({multiplier}) must be finite; a NaN or infinite multiplier would produce an invalid delay")
            }
            Self::MultiplierNegative { multiplier } => {
                write!(f, "retry multiplier ({multiplier}) must not be negative; a negative multiplier would produce a negative delay")
            }
        }
    }
}

impl std::error::Error for RetryPolicyError {}

// ---------------------------------------------------------------------------
// WorkerConfigError
// ---------------------------------------------------------------------------

/// An error from an invalid worker configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerConfigError {
    /// The job timeout exceeds the lease (would guarantee duplicate delivery).
    JobTimeoutExceedsLease {
        job_timeout: std::time::Duration,
        lease: std::time::Duration,
    },
    /// The heartbeat interval is not below the lease (would be futile).
    HeartbeatIntervalNotBelowLease {
        heartbeat_interval: std::time::Duration,
        lease: std::time::Duration,
    },
}

impl fmt::Display for WorkerConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::JobTimeoutExceedsLease {
                job_timeout,
                lease,
            } => {
                write!(
                    f,
                    "job_timeout ({job_timeout:?}) must not exceed lease ({lease:?}); a timeout longer than the lease would guarantee duplicate delivery"
                )
            }
            Self::HeartbeatIntervalNotBelowLease {
                heartbeat_interval,
                lease,
            } => {
                write!(
                    f,
                    "heartbeat_interval ({heartbeat_interval:?}) must be below lease ({lease:?}); a heartbeat at or above the lease would never refresh in time"
                )
            }
        }
    }
}

impl std::error::Error for WorkerConfigError {}

// ---------------------------------------------------------------------------
// SchedulerError
// ---------------------------------------------------------------------------

/// A typed error from the scheduler.
#[derive(Debug)]
pub enum SchedulerError {
    /// A job could not be enqueued.
    Enqueue(EnqueueError),
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enqueue(e) => write!(f, "scheduler enqueue failed: {e}"),
        }
    }
}

impl std::error::Error for SchedulerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Enqueue(e) => Some(e),
        }
    }
}

impl From<EnqueueError> for SchedulerError {
    fn from(e: EnqueueError) -> Self {
        Self::Enqueue(e)
    }
}
