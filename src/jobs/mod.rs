//! Durable background jobs on PostgreSQL, SQLite, or MySQL.
//!
//! One queue over the application's existing pool -- no second pool, no
//! separate broker. The queue is at-least-once; claims are fenced by a
//! per-claim UUID token so a stale worker (lease expired, sweep requeued)
//! cannot commit its result over another worker's claim.
//!
//! # Which dialect
//!
//! The build speaks exactly one dialect, chosen by the `db-postgres`,
//! `db-sqlite`, and `db-mysql` features. Everything dialect-specific -- the
//! statement text, the placeholder style, how a timestamp is stored, and the
//! shape of the claim -- is confined to the private `dialect` module. The
//! claim comes in two shapes because the databases genuinely differ:
//!
//! - **PostgreSQL** claims in one statement: `UPDATE ... RETURNING` over a
//!   `FOR UPDATE SKIP LOCKED` subquery.
//! - **MySQL 8** has `SKIP LOCKED` but no `RETURNING`, so it picks with a
//!   locking `SELECT` and marks each picked row inside one transaction.
//! - **SQLite** has neither, and needs neither: `BEGIN IMMEDIATE` takes the
//!   database write lock, so a claim is exclusive by construction and
//!   competing claimers wait (bounded by `busy_timeout`) rather than skip.
//!   The cost is that SQLite claimers serialise, which is the right trade for
//!   the single-node use SQLite is chosen for.
//!
//! Lease arithmetic is done by the database in every dialect, so recovering a
//! crashed worker's jobs does not depend on worker clocks agreeing.
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
mod dialect;
mod enqueue;
mod error;
mod facade;
mod migrate;
mod observe;
mod registry;
mod scheduler;
// The fixture the live-database tests share. Gated on `test-kit` because it
// reuses that module's safety check rather than keeping a second copy of it;
// see the module comment.
#[cfg(all(test, feature = "test-kit"))]
mod test_support;
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
pub use dialect::JobPool;
pub use enqueue::{EnqueuedJob, JobRequest, JobStatus};
pub use error::{
    EnqueueError, JobError, MigrateError, RegisterError, RetryPolicyError, SchedulerError,
    WorkerConfigError, WorkerError,
};
pub use facade::Jobs;
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
