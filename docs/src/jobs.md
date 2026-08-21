# Jobs

Durable background jobs on PostgreSQL. A `FOR UPDATE SKIP LOCKED` queue over
the application's existing `PgPool` — no second connection pool, no separate
broker to operate.

The queue is at-least-once. A handler must tolerate running twice.

## Declaring a job

```rust,ignore
use arcature::Job;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, arcature::Job)]
struct SendVerificationEmail {
    user_id: u64,
}
```

The derive generates `impl DxComponent` (so `NAME` is
`"SendVerificationEmail"`) and a `JOB` const describing the queue identity.
The defaults are the struct name in snake_case, version 1, three attempts:

```rust,ignore
assert_eq!(SendVerificationEmail::JOB.kind(), "send_verification_email");
assert_eq!(SendVerificationEmail::JOB.version(), 1);
assert_eq!(SendVerificationEmail::JOB.max_attempts(), 3);
```

Override them with the helper attribute:

```rust,ignore
#[derive(Debug, Clone, Serialize, Deserialize, arcature::Job)]
#[job(kind = "custom_kind", version = 2, attempts = 5)]
struct CleanupSessions {
    user_id: u64,
}
```

`version` is part of the queue identity, not decoration. A handler registers
for a `(kind, version)` pair, so bumping the version lets a new payload shape
coexist with jobs already in the table rather than deserializing into the
wrong struct.

`JobModel::new(kind, version, max_attempts)` builds the identity by hand if
you would rather not derive it.

## Enqueueing

```rust,ignore
use arcature::jobs::{JobRequest, Jobs};

let jobs = Jobs::new(pool.clone());
let request = JobRequest::new(
    &SendVerificationEmail::JOB,
    &SendVerificationEmail { user_id: 42 },
)?;
jobs.enqueue(&request).await?;
```

`JobRequest` builders: `.delay(Duration)`, `.run_at(DateTime<Utc>)`,
`.max_attempts(n)` to override the model's default for this one job.

`jobs.enqueue_tx(..)` enqueues inside a transaction you already hold, and
`enqueue_with(executor, ..)` takes any SQLx executor. Enqueueing in the same
transaction as the state change is the only way to avoid the job that runs
before its row exists.

`jobs.migrate()` creates the queue tables; `migrate_tx` does it inside a
transaction you own.

Payloads are size-capped (`DEFAULT_MAX_PAYLOAD_BYTES`, overridable per model
with `with_max_payload_bytes`). A queue row is not a blob store; put the bytes
in [Storage](storage.md) and the key in the payload.

## Handlers

A handler is a closure registered against the job model. `Registry::add`
takes `&mut self`, so the registry is mutable while you build it:

```rust,ignore
use arcature::jobs::{JobError, Registry};

pub fn registry() -> Registry {
    let mut registry = Registry::new();
    registry
        .add(&SendVerificationEmail::JOB, |job: SendVerificationEmail| async move {
            send_email(job.user_id).await.map_err(JobError::retryable)
        })
        .expect("job kind is valid and registered once");
    registry
}
```

Registering the same `(kind, version)` twice is an error, not a silent
overwrite.

The handler's error type decides what happens next. `JobError::Retryable`
means retry per the backoff policy until `max_attempts` is exhausted;
`JobError::Permanent` means dead immediately. `retryable(e)` and
`permanent(e)` wrap any `std::error::Error`; `retryable_msg(s)` and
`permanent_msg(s)` take a string. Choosing between them is the handler's job,
because the framework cannot tell a bad payload from a flaky network.

`#[job_handler]` validates that a handler function is `pub async fn` with a
return type and emits it unchanged. It generates no binding const and does
not register anything: a handler's proc-macro cannot see the job's kind and
version, since those come from `#[derive(Job)]` on the payload struct.
Registration stays explicit in application code.

## Running a worker

```rust,ignore
use std::time::Duration;
use arcature::jobs::{RetryPolicy, Worker, WorkerConfig};
use tokio_util::sync::CancellationToken;

let worker = Worker::builder(pool.clone(), registry())
    .worker_id("worker-1")
    .config(WorkerConfig::default().concurrency(16))
    .retry_policy(
        RetryPolicy::exponential(Duration::from_secs(5), 2.0, Duration::from_secs(600))
            .jitter(true),
    )
    .build();

worker.run(CancellationToken::new()).await?;
```

`Worker::new(pool, registry)` skips the builder when the defaults suffice.
`run` takes a `CancellationToken` and returns when it fires, so shutdown is
the caller's to sequence.

`WorkerConfig` defaults: concurrency 8, poll interval 200ms, lease 300s, poll
batch 8, sweep every 30s in batches of 64, per-job timeout 60s, heartbeat
derived as lease / 3.

Or from the CLI: `arc queue work`, `arc queue drain`, `arc queue stats`.

## Why claims are fenced

Every claim carries a per-claim UUID `claim_token`, and every completion
mutation fences on `(id, status = 'running', claim_token)`.

The reason is the lease. A worker claims a job for a bounded time; if it dies
or stalls past the lease, the sweep requeues the job and another worker picks
it up. Without the token, the first worker waking up late would write its
result over the second worker's claim — two runs, one of them clobbering the
other's outcome. With it, the stale worker's UPDATE matches zero rows and
does nothing.

At-least-once still means at-least-once. The fence stops a stale worker
committing a result, not a job body running twice.

## Retries

`RetryPolicy::exponential(base, multiplier, cap)` computes
`base * multiplier^(attempts - 1)`, capped. `RetryPolicy::fixed(delay)` for a
flat wait. `.jitter(true)` enables full jitter, which is what stops a batch of
simultaneous failures retrying in lockstep forever.

A job that exhausts `max_attempts` is dead. `arcature::jobs::admin` exposes
`requeue_dead`, `cancel`, and `sweep_expired_leases` for the operator paths.

## Scheduling

```rust,ignore
use arcature::jobs::{ScheduleBinding, ScheduleCadence, Scheduler};

const NIGHTLY: ScheduleBinding = ScheduleBinding {
    job: "cleanup_sessions",
    version: 1,
    cadence: ScheduleCadence::Daily { hour: 3, minute: 0 },
};

let scheduler = Scheduler::new().schedule(&NIGHTLY, move || {
    let jobs = jobs.clone();
    async move { /* enqueue */ Ok(()) }
});

scheduler.run(CancellationToken::new()).await?;
```

`ScheduleCadence` is an interval or a daily wall-clock time. The scheduler
enqueues; the worker runs. They are separate processes if you want them to
be. From the CLI: `arc schedule`.

## Observability

`Observer` is the seam, defaulting to `NoopObserver`. Implement it and pass it
to `WorkerBuilder::observer` to see claims, completions and failures.

## PostgreSQL only

The queue requires PostgreSQL. `FOR UPDATE SKIP LOCKED` is the whole design,
and SQLite and MySQL do not have a usable equivalent. An application on
`db-sqlite` gets the rest of the framework and no job queue.
