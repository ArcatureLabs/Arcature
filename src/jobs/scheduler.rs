//! The recurring-job scheduler.
//!
//! A managed recurring-job enqueuer: it only enqueues jobs; the worker claims
//! and runs them. It owns a `CancellationToken` for graceful shutdown, never
//! spawns unmanaged work. UTC only. Two cadences: `Every { seconds }` and
//! `Daily { hour, minute }` (no cron expressions). The caller spawns it.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use chrono::{DateTime, Datelike, TimeZone, Utc};
use tokio_util::sync::CancellationToken;

use super::error::SchedulerError;

/// A boxed future returned by a type-erased enqueue closure.
type BoxFuture = Pin<Box<dyn Future<Output = Result<(), SchedulerError>> + Send>>;

/// A type-erased enqueue function. Captures a `Jobs` clone at construction
/// time (in the `schedule!` macro's `build_scheduler`).
type EnqueueFn = Box<dyn Fn() -> BoxFuture + Send + Sync>;

/// The cadence at which a scheduled job fires.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleCadence {
    /// Fire every `seconds` seconds.
    Every { seconds: u64 },
    /// Fire daily at `hour:minute` UTC.
    Daily { hour: u8, minute: u8 },
}

/// A compile-time schedule binding.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ScheduleBinding {
    /// The job kind string.
    pub job: &'static str,
    /// The job version.
    pub version: i16,
    /// The cadence (interval or daily time).
    pub cadence: ScheduleCadence,
}

/// A single scheduled entry.
struct ScheduleEntry {
    /// The job kind (for logging / inspection).
    kind: &'static str,
    /// The job version (for logging / inspection).
    version: i16,
    /// The cadence (interval or daily time).
    cadence: ScheduleCadence,
    /// The next time this entry should fire.
    next_fire: DateTime<Utc>,
    /// The type-erased enqueue closure.
    fire: EnqueueFn,
}

/// The recurring-job scheduler.
pub struct Scheduler {
    entries: Vec<ScheduleEntry>,
}

impl Scheduler {
    /// Create a new empty scheduler.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add a scheduled entry. The `fire` closure is called on each cadence
    /// tick and should enqueue the job. The `next_fire` is computed from the
    /// cadence and the current time.
    #[must_use]
    pub fn schedule<F>(mut self, binding: &ScheduleBinding, fire: F) -> Self
    where
        F: Fn() -> BoxFuture + Send + Sync + 'static,
    {
        let next_fire = compute_next_fire(&binding.cadence, Utc::now());
        self.entries.push(ScheduleEntry {
            kind: binding.job,
            version: binding.version,
            cadence: binding.cadence.clone(),
            next_fire,
            fire: Box::new(fire),
        });
        self
    }

    /// The number of scheduled entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no entries are scheduled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Run the scheduler loop until `shutdown` is cancelled.
    pub async fn run(mut self, shutdown: CancellationToken) -> Result<(), SchedulerError> {
        if self.entries.is_empty() {
            shutdown.cancelled().await;
            return Ok(());
        }

        loop {
            // Find the earliest next fire time.
            let earliest = self
                .entries
                .iter()
                .map(|e| e.next_fire)
                .min()
                .unwrap_or_else(Utc::now);

            let now = Utc::now();
            let sleep_duration = if earliest > now {
                (earliest - now).to_std().unwrap_or(Duration::from_secs(0))
            } else {
                Duration::from_secs(0)
            };

            tokio::select! {
                _ = shutdown.cancelled() => return Ok(()),
                _ = tokio::time::sleep(sleep_duration) => {}
            }

            // Fire all due entries.
            let now = Utc::now();
            for entry in &mut self.entries {
                if entry.next_fire <= now {
                    if let Err(e) = (entry.fire)().await {
                        eprintln!(
                            "scheduler enqueue error for {} v{}: {e}",
                            entry.kind, entry.version
                        );
                    }
                    entry.next_fire = compute_next_fire(&entry.cadence, now);
                }
            }
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("entries", &self.entries.len())
            .finish_non_exhaustive()
    }
}

/// Compute the next fire time from a cadence and the current time.
fn compute_next_fire(cadence: &ScheduleCadence, now: DateTime<Utc>) -> DateTime<Utc> {
    match cadence {
        ScheduleCadence::Every { seconds } => {
            let dur = chrono::Duration::seconds(i64::try_from(*seconds).unwrap_or(i64::MAX));
            now + dur
        }
        ScheduleCadence::Daily { hour, minute } => {
            let h = u32::from(*hour);
            let m = u32::from(*minute);
            let today = Utc
                .with_ymd_and_hms(now.year(), now.month(), now.day(), h, m, 0)
                .single();
            match today {
                Some(t) if t > now => t,
                _ => {
                    let tomorrow = now + chrono::Duration::days(1);
                    Utc.with_ymd_and_hms(tomorrow.year(), tomorrow.month(), tomorrow.day(), h, m, 0)
                        .single()
                        .unwrap_or(now)
                }
            }
        }
    }
}
