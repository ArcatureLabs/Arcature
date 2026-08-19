//! Durable PostgreSQL background jobs.
//!
//! A `FOR UPDATE SKIP LOCKED` queue over the application's existing `PgPool`.
//! The queue is at-least-once; claims are fenced by a per-claim UUID token so
//! a stale worker (lease expired, sweep requeued) cannot commit its result
//! over another worker's claim.
//!
//! # Architecture
//!
//! - [`Jobs`] is the enqueue facade (one pool, no second connection).
//! - [`Worker`] claims and runs jobs. Handlers are closures registered via
//!   [`Registry::add`].
//! - [`Scheduler`] enqueues recurring jobs on a cadence; the worker runs them.
//! - [`RetryPolicy`] is exponential backoff with jitter and a cap.
//! - [`Observer`] is the observability seam (default: no-op).
//!
//! # Example
//!
//! ```ignore
//! use arcature::jobs::{JobModel, JobRequest, Jobs, Registry, Worker, JobError};
//!
//! #[derive(serde::Serialize, serde::Deserialize)]
//! struct SendWelcome { email: String }
//!
//! const SEND_WELCOME: JobModel<SendWelcome> = JobModel::new("send_welcome", 1, 3);
//!
//! let registry = Registry::new();
//! // registry.add(&SEND_WELCOME, |job| async { ... })?;
//! let worker = Worker::new(pool.clone(), registry);
//! let jobs = Jobs::new(pool.clone());
//! jobs.enqueue(&JobRequest::new(&SEND_WELCOME, &SendWelcome { email: "a@b.com".into() })?).await?;
//! ```

#![forbid(unsafe_code)]

mod claim;
mod complete;
mod config;
mod enqueue;
mod error;
mod jobs;
mod migrate;
mod observe;
mod registry;
mod scheduler;
mod validate;
mod worker;

pub mod admin {
    pub use super::claim::claim_jobs;
    pub use super::complete::{cancel, requeue_dead, sweep_expired_leases};
}
pub mod retry {
    pub use super::config::RetryPolicy;
}

pub use claim::ClaimedJob;
pub use config::{DEFAULT_MAX_PAYLOAD_BYTES, JobModel, RetryPolicy, WorkerConfig};
pub use enqueue::{EnqueuedJob, JobRequest, JobStatus};
pub use error::{
    EnqueueError, JobError, MigrateError, RegisterError, RetryPolicyError, SchedulerError,
    WorkerConfigError, WorkerError,
};
pub use jobs::Jobs;
pub use observe::{Event, FailReason, NoopObserver, Observer, SharedObserver};
pub use registry::Registry;
pub use scheduler::{ScheduleBinding, ScheduleCadence, Scheduler};
pub use worker::{Worker, WorkerBuilder};

// Re-export the certified sqlx so downstream code targets the pinned version
// through Arcature (e.g. `arcature::jobs::sqlx`).
pub use sqlx;

use serde::Serialize;

// ---------------------------------------------------------------------------
// Job trait — the marker trait for typed jobs.
// ---------------------------------------------------------------------------

/// The marker trait for typed Arcature jobs.
///
/// A job type must have a static [`NAME`](crate::DxComponent::NAME) and must
/// be `Serialize + DeserializeOwned` (the user adds
/// `#[derive(Serialize, Deserialize)]`). The `#[job]` macro generates
/// `impl DxComponent` (with `NAME = stringify!(StructName)`) and the empty
/// `impl Job`.
pub trait Job: crate::DxComponent + Serialize + serde::de::DeserializeOwned {}
