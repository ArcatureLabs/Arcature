//! The worker: claims and runs jobs from the shared pool.
//!
//! The run loop claims a batch of pending jobs, spawns each as a dispatch
//! task bounded by a concurrency semaphore, and reaps finished tasks. A
//! sweep of expired leases runs on its own cadence regardless of queue
//! busy-ness (crash recovery must not starve on a busy queue). Graceful
//! shutdown releases unclaimed leases and waits for in-flight tasks to
//! commit their outcome (no abort).

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::claim::{self, ClaimedJob};
use super::complete::{self, ClaimTransition, DispatchOutcome, ErrorKind, JobState, Outcome};
use super::config::{RetryPolicy, WorkerConfig};
use super::dialect::{JobPool, sql};
use super::error::WorkerError;
use super::observe::{Event, FailReason, Observer};
use super::registry::{self, Registry};

/// The fixed message stored when a handler panics. The panic payload is never
/// stored or emitted (a handler may panic with a secret in the message).
const PANIC_MESSAGE: &str = "handler panicked";

/// A worker that claims and runs jobs from the shared pool.
#[derive(Clone)]
pub struct Worker {
    pool: JobPool,
    registry: Registry,
    config: WorkerConfig,
    retry: RetryPolicy,
    worker_id: String,
    observer: Option<Arc<dyn Observer>>,
}

impl Worker {
    /// Create a worker with default config and retry policy.
    pub fn new(pool: JobPool, registry: Registry) -> Self {
        Self::builder(pool, registry).build()
    }

    /// Create a worker builder.
    pub fn builder(pool: JobPool, registry: Registry) -> WorkerBuilder {
        WorkerBuilder {
            inner: Worker {
                pool,
                registry,
                config: WorkerConfig::default(),
                retry: RetryPolicy::default(),
                worker_id: format!("worker-{}", Uuid::new_v4().simple()),
                observer: None,
            },
        }
    }

    /// Run the worker until `shutdown` is cancelled.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), WorkerError> {
        run_loop(
            self.pool,
            self.registry,
            self.config,
            self.retry,
            self.worker_id,
            self.observer,
            shutdown,
        )
        .await
    }
}

/// A builder for [`Worker`].
pub struct WorkerBuilder {
    inner: Worker,
}

impl WorkerBuilder {
    /// Set the worker id. Must be non-empty, at most 128 bytes, ASCII
    /// graphic only; otherwise the default `worker-<uuid>` is kept.
    pub fn worker_id(mut self, id: impl Into<String>) -> Self {
        let id = id.into();
        if !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_graphic()) {
            self.inner.worker_id = id;
        }
        self
    }

    /// Set the worker configuration.
    pub fn config(mut self, config: WorkerConfig) -> Self {
        self.inner.config = config;
        self
    }

    /// Set the retry policy.
    pub fn retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.inner.retry = policy;
        self
    }

    /// Set the observer.
    pub fn observer(mut self, observer: impl Observer) -> Self {
        self.inner.observer = Some(Arc::new(observer));
        self
    }

    /// Build the worker.
    pub fn build(self) -> Worker {
        self.inner
    }
}

// ---------------------------------------------------------------------------
// The run loop
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn run_loop(
    pool: JobPool,
    registry: Registry,
    config: WorkerConfig,
    retry: RetryPolicy,
    worker_id: String,
    observer: Option<Arc<dyn Observer>>,
    shutdown: CancellationToken,
) -> Result<(), WorkerError> {
    // Validate the relational invariants before the loop starts. A config
    // with job_timeout > lease would guarantee duplicate delivery; a
    // heartbeat not below the lease would be futile; a retry policy with a
    // non-finite or negative multiplier would panic on the first retry.
    config.validate()?;
    retry.validate()?;

    let semaphore = Arc::new(Semaphore::new(config.get_concurrency()));
    let mut join_set: JoinSet<()> = JoinSet::new();
    let mut next_sweep = tokio::time::Instant::now() + config.get_sweep_interval();

    loop {
        // Reap finished tasks so the JoinSet does not grow unbounded.
        while join_set.try_join_next().is_some() {}

        if shutdown.is_cancelled() {
            break;
        }

        // Run the lease sweep when it is due, regardless of queue busy-ness.
        let now = tokio::time::Instant::now();
        if now >= next_sweep {
            let _ = complete::sweep_expired_leases(&pool, config.get_sweep_batch()).await;
            next_sweep = now + config.get_sweep_interval();
        }

        // Claim a batch of pending jobs.
        let claimed = claim::claim_jobs(
            &pool,
            &worker_id,
            config.get_lease(),
            config.get_poll_batch(),
        )
        .await?;

        if claimed.is_empty() {
            // No jobs available: sleep before the next poll (interruptible).
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(config.get_poll_interval()) => {}
            }
            continue;
        }

        for job in claimed {
            // On shutdown, release the just-claimed row back to pending so a
            // sweep redelivers it. We do not start a job we will not finish.
            if shutdown.is_cancelled() {
                release_claim(&pool, job.id, job.claim_token).await?;
                continue;
            }
            // Acquire a concurrency permit before spawning. If the semaphore
            // is exhausted, wait here (interruptible by shutdown).
            let permit = {
                let sem = semaphore.clone();
                tokio::select! {
                    res = sem.acquire_owned() => match res {
                        Ok(permit) => permit,
                        Err(_) => {
                            release_claim(&pool, job.id, job.claim_token).await?;
                            continue;
                        }
                    },
                    _ = shutdown.cancelled() => {
                        release_claim(&pool, job.id, job.claim_token).await?;
                        continue;
                    }
                }
            };

            let pool_clone = pool.clone();
            let registry_clone = registry.clone();
            let observer_clone = observer.clone();
            let retry_clone = retry;
            let config_clone = config;
            join_set.spawn(async move {
                let _permit = permit;
                let result = dispatch_one(
                    &pool_clone,
                    &registry_clone,
                    retry_clone,
                    config_clone,
                    observer_clone.as_deref(),
                    job,
                )
                .await;
                if let Err(e) = result {
                    eprintln!("job dispatch error: {e}");
                }
            });
        }
    }

    // Graceful shutdown: wait for every in-flight task to finish. Each is
    // bounded by the per-job timeout, so this join cannot hang.
    while let Some(res) = join_set.join_next().await {
        if let Err(e) = res {
            eprintln!("job dispatch task panicked: {e}");
        }
    }

    Ok(())
}

/// Release a claim back to pending (fenced on the claim token).
async fn release_claim(pool: &JobPool, job_id: Uuid, claim_token: Uuid) -> Result<(), WorkerError> {
    let _ = sqlx::query(sql::RELEASE_CLAIM)
        .bind(job_id)
        .bind(claim_token)
        .execute(pool)
        .await?;
    Ok(())
}

fn emit(observer: Option<&dyn Observer>, event: Event) {
    if let Some(o) = observer {
        o.observe(&event);
    }
}

// ---------------------------------------------------------------------------
// dispatch_one — the per-job execute path
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn dispatch_one(
    pool: &JobPool,
    registry: &Registry,
    retry: RetryPolicy,
    config: WorkerConfig,
    observer: Option<&dyn Observer>,
    job: ClaimedJob,
) -> Result<(), WorkerError> {
    let job_timeout = config.get_job_timeout();
    let lease = config.get_lease();
    let heartbeat_interval = config.get_heartbeat_interval();

    let started = Instant::now();
    let job_id = job.id;
    let claim_token = job.claim_token;
    let kind = job.kind.clone();
    let version = job.version;
    let attempt = job.attempts;

    // Look up the handler. An unknown job is a poison job: mark it dead so it
    // is not retried forever.
    let handler = match registry.get(&job.kind, job.version) {
        Some(h) => h,
        None => {
            let transition = complete::mark_dead(
                pool,
                job_id,
                claim_token,
                ErrorKind::Unknown,
                format!("no handler registered for kind {kind:?} version {version}"),
            )
            .await?;
            if transition == ClaimTransition::Updated {
                emit(
                    observer,
                    Event::Failed {
                        job_id,
                        attempt,
                        duration: started.elapsed(),
                        message: format!(
                            "no handler registered for kind {kind:?} version {version}"
                        ),
                        reason: FailReason::Unknown,
                    },
                );
            }
            return Ok(());
        }
    };

    emit(
        observer,
        Event::Started {
            job_id,
            kind: kind.clone(),
            version,
            attempt,
        },
    );

    // The claim already incremented attempts, so the completion decision has
    // the exact attempt count without a second round-trip.
    let state = JobState {
        attempts: job.attempts,
        max_attempts: job.max_attempts,
    };

    // Run the handler in a spawned task so a panic is caught (it does not
    // crash the worker) and a timeout can cancel it.
    let payload = job.payload.clone();
    let mut handler_task = {
        let handler = handler.clone();
        tokio::spawn(async move { handler.handle(&payload, job_id).await })
    };

    let handler_start = tokio::time::Instant::now();
    let timeout_deadline = handler_start + job_timeout;

    let dispatch = loop {
        tokio::select! {
            res = &mut handler_task => {
                break match res {
                    Ok(handler_result) => match handler_result {
                        Ok(()) => DispatchOutcome::Succeeded,
                        Err(registry::HandlerError::Malformed) => DispatchOutcome::Malformed,
                        Err(registry::HandlerError::Job(e)) => DispatchOutcome::HandlerError(e),
                    },
                    Err(join_err) => {
                        // A panic is a permanent failure. The panic payload
                        // is NOT persisted or emitted (secret safety).
                        let transition = complete::mark_dead(
                            pool,
                            job_id,
                            claim_token,
                            ErrorKind::Panic,
                            PANIC_MESSAGE.to_string(),
                        )
                        .await?;
                        if transition == ClaimTransition::Updated {
                            emit(
                                observer,
                                Event::Failed {
                                    job_id,
                                    attempt,
                                    duration: started.elapsed(),
                                    message: PANIC_MESSAGE.to_string(),
                                    reason: FailReason::Panic,
                                },
                            );
                        }
                        let _ = join_err;
                        return Ok(());
                    }
                };
            }
            _ = tokio::time::sleep_until(timeout_deadline) => {
                handler_task.abort();
                break DispatchOutcome::Timeout;
            }
            _ = tokio::time::sleep(heartbeat_interval) => {
                // Refresh the lease. On false (claim lost) or error, continue;
                // the completion will fence out, or the lease may still hold.
                let _ = complete::heartbeat(pool, job_id, claim_token, lease).await;
            }
        }
    };

    let duration = started.elapsed();

    // Persist the outcome. Every transition fences on the claim token; a
    // lost claim affects zero rows and the stale event is suppressed.
    let now = chrono::Utc::now();
    let outcome = complete::decide(&state, &dispatch, &retry, now);
    match outcome {
        Outcome::Succeeded => {
            let transition = complete::mark_succeeded(pool, job_id, claim_token).await?;
            if transition == ClaimTransition::Updated {
                emit(
                    observer,
                    Event::Succeeded {
                        job_id,
                        attempt,
                        duration,
                    },
                );
            }
        }
        Outcome::Retry { available_at } => {
            let message = match &dispatch {
                DispatchOutcome::HandlerError(err) => err.stored_message(),
                DispatchOutcome::Timeout => {
                    format!(
                        "job {job_id} exceeded its {}s timeout",
                        job_timeout.as_secs()
                    )
                }
                _ => String::new(),
            };
            let transition =
                complete::mark_retry(pool, job_id, claim_token, available_at, message.clone())
                    .await?;
            if transition == ClaimTransition::Updated {
                emit(
                    observer,
                    Event::Retried {
                        job_id,
                        attempt,
                        duration,
                        message,
                        next_run_at: available_at,
                    },
                );
            }
        }
        Outcome::Dead {
            error_kind,
            message,
        } => {
            let reason = match error_kind {
                ErrorKind::Permanent => FailReason::Permanent,
                ErrorKind::Exhausted => FailReason::Exhausted,
                ErrorKind::Malformed => FailReason::Malformed,
                ErrorKind::Unknown => FailReason::Unknown,
                ErrorKind::Panic => FailReason::Panic,
                ErrorKind::Timeout => FailReason::Timeout,
            };
            let transition =
                complete::mark_dead(pool, job_id, claim_token, error_kind, message.clone()).await?;
            if transition == ClaimTransition::Updated {
                emit(
                    observer,
                    Event::Failed {
                        job_id,
                        attempt,
                        duration,
                        message,
                        reason,
                    },
                );
            }
        }
    }
    Ok(())
}
