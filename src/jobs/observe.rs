//! The job observability seam.
//!
//! A trait the application implements; the default implementation is a no-op.
//! Events carry only safe data: never the serialized payload, never the panic
//! message. The callback is `&self` + `Sync` (sharable via `Arc` across
//! dispatch tasks), and must not block (called inline on the dispatch path).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// An event emitted by the worker during a job's lifecycle.
#[derive(Debug, Clone)]
pub enum Event {
    /// A job was claimed and its handler is starting.
    Started {
        job_id: uuid::Uuid,
        kind: String,
        version: i16,
        attempt: i32,
    },
    /// A job completed successfully.
    Succeeded {
        job_id: uuid::Uuid,
        attempt: i32,
        duration: Duration,
    },
    /// A job was retried (will run again at `next_run_at`).
    Retried {
        job_id: uuid::Uuid,
        attempt: i32,
        duration: Duration,
        message: String,
        next_run_at: chrono::DateTime<chrono::Utc>,
    },
    /// A job failed permanently (dead).
    Failed {
        job_id: uuid::Uuid,
        attempt: i32,
        duration: Duration,
        message: String,
        reason: FailReason,
    },
}

/// The reason a job failed permanently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailReason {
    /// The handler returned a permanent error.
    Permanent,
    /// The retry budget was exhausted.
    Exhausted,
    /// The payload did not deserialize to the registered schema.
    Malformed,
    /// No handler was registered for this kind and version.
    Unknown,
    /// The handler panicked.
    Panic,
    /// The job exceeded its per-attempt timeout.
    Timeout,
}

impl fmt::Display for FailReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Permanent => f.write_str("permanent"),
            Self::Exhausted => f.write_str("exhausted"),
            Self::Malformed => f.write_str("malformed"),
            Self::Unknown => f.write_str("unknown"),
            Self::Panic => f.write_str("panic"),
            Self::Timeout => f.write_str("timeout"),
        }
    }
}

/// The observer trait. Implement this to record job lifecycle events.
pub trait Observer: Send + Sync + 'static {
    /// Called when a job lifecycle event occurs. The default is a no-op.
    fn observe(&self, _event: &Event) {}
}

/// A shared observer handle (`Arc<dyn Observer>`).
pub type SharedObserver = Arc<dyn Observer>;

/// A no-op observer (the default).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopObserver;

impl Observer for NoopObserver {}
