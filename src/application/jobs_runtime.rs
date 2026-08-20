//! The managed runtime for the job worker and scheduler.
//!
//! Started after the database connects (the worker and scheduler reuse the
//! pool) and torn down before the database closes. A [`JobsRuntime`] owns the
//! [`CancellationToken`] that stops both the worker and scheduler, plus the
//! task join handles so graceful shutdown waits for in-flight jobs to commit.
//!
//! This module is the single place that knows how to spawn and stop the job
//! subsystem; [`ApplicationBuilder`](super::builder::ApplicationBuilder) calls
//! [`start_jobs`] on startup and [`JobsRuntime::shutdown`] on teardown.

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::application::resources::Resources;
use crate::application::{EngineError, EngineResult};

/// The managed handles for a running job worker (and optional scheduler).
///
/// `None` when the `jobs` feature is disabled or no registry was registered.
/// The worker and scheduler tasks are spawned on the Tokio runtime; shutdown
/// cancels the shared token and awaits both tasks.
pub struct JobsRuntime {
    shutdown: CancellationToken,
    worker: JoinHandle<Result<(), crate::jobs::WorkerError>>,
    scheduler: Option<JoinHandle<Result<(), crate::jobs::SchedulerError>>>,
}

impl JobsRuntime {
    /// Stop the worker and scheduler. Cancels the shared token, then awaits
    /// both tasks so in-flight jobs finish their commit (no abort). Errors
    /// from the tasks are surfaced as [`EngineError::Shutdown`].
    pub async fn shutdown(self) -> EngineResult<()> {
        self.shutdown.cancel();
        if let Err(join_err) = self.worker.await {
            return Err(EngineError::Shutdown {
                subsystem: "jobs",
                source: crate::Error::Job(format!("worker task panicked: {join_err}")),
            });
        }
        if let Some(scheduler) = self.scheduler {
            if let Err(join_err) = scheduler.await {
                return Err(EngineError::Shutdown {
                    subsystem: "jobs",
                    source: crate::Error::Job(format!("scheduler task panicked: {join_err}")),
                });
            }
        }
        Ok(())
    }
}

/// Start the job subsystem from the application's configured registry and
/// scheduler. Migrates the queue schema, builds the [`Jobs`] facade and stores
/// it in `resources`, then spawns the worker (and scheduler, if any) as
/// managed tasks gated on a shared [`CancellationToken`].
///
/// The registry and scheduler are taken by value (the scheduler is not
/// `Clone`; its entries hold boxed enqueue closures). The worker config is
/// `Copy`. Returns `None` when no registry was registered (the app did not
/// call [`ApplicationBuilder::jobs`](super::builder::ApplicationBuilder::jobs)),
/// in which case nothing is spawned.
///
/// [`Jobs`]: crate::jobs::Jobs
pub(super) async fn start_jobs(
    registry: Option<crate::jobs::Registry>,
    worker_config: Option<crate::jobs::WorkerConfig>,
    scheduler: Option<crate::jobs::Scheduler>,
    resources: &mut Resources,
) -> EngineResult<Option<JobsRuntime>> {
    let Some(registry) = registry else {
        return Ok(None);
    };

    // The worker reuses the database pool, so the database must be connected.
    let db = resources.db().ok_or_else(|| EngineError::Startup {
        subsystem: "jobs",
        stage: "connect",
        source: crate::Error::Config(
            "the jobs subsystem requires the database subsystem to be enabled and connected"
                .to_string(),
        ),
    })?;
    let pool = db.sqlx().clone();

    // Apply the queue schema migrations (idempotent, advisory-locked).
    let jobs = crate::jobs::Jobs::new(pool.clone());
    jobs.migrate().await.map_err(|e| EngineError::Startup {
        subsystem: "jobs",
        stage: "migrate",
        source: crate::Error::Job(e.to_string()),
    })?;
    resources.set_jobs(jobs);

    let shutdown = CancellationToken::new();

    // Build the worker from the configured (or default) config.
    let worker_config = worker_config.unwrap_or_default();
    let worker = crate::jobs::Worker::builder(pool, registry)
        .config(worker_config)
        .build();
    let worker_shutdown = shutdown.clone();
    let worker_handle = tokio::spawn(async move { worker.run(worker_shutdown).await });

    // Spawn the scheduler only when one was registered.
    let scheduler_handle = if let Some(scheduler) = scheduler {
        let scheduler_shutdown = shutdown.clone();
        Some(tokio::spawn(async move {
            scheduler.run(scheduler_shutdown).await
        }))
    } else {
        None
    };

    Ok(Some(JobsRuntime {
        shutdown,
        worker: worker_handle,
        scheduler: scheduler_handle,
    }))
}
