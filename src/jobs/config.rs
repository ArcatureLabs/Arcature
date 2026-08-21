//! Job model descriptors, retry policy, and worker configuration.

use std::marker::PhantomData;
use std::time::Duration;

use serde::Serialize;

use super::error::{RetryPolicyError, WorkerConfigError};
use super::validate::{validate_kind, validate_version};

/// The default maximum payload size, in bytes (64 KiB).
pub const DEFAULT_MAX_PAYLOAD_BYTES: usize = 65_536;

// ---------------------------------------------------------------------------
// JobModel<J> — the compile-time descriptor for a job type.
// ---------------------------------------------------------------------------

/// A compile-time descriptor for a job type `J`.
///
/// Carries the kind string, version, default max attempts, and the payload
/// size limit. The `#[job]` macro generates a `JobModel<J>` const from an
/// annotated struct; you can also construct one by hand:
///
/// ```
/// use arcature::jobs::JobModel;
///
/// #[derive(serde::Serialize, serde::Deserialize)]
/// struct SendWelcome {
///     email: String,
/// }
///
/// const SEND_WELCOME: JobModel<SendWelcome> = JobModel::new("send_welcome", 1, 3);
///
/// assert_eq!(SEND_WELCOME.kind(), "send_welcome");
/// assert_eq!(SEND_WELCOME.version(), 1);
/// assert_eq!(SEND_WELCOME.max_attempts(), 3);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct JobModel<J> {
    kind: &'static str,
    version: i16,
    max_attempts: u32,
    payload_limit: usize,
    _job: PhantomData<J>,
}

// We need `const fn new` to allow `const` declarations (e.g. from the macro).
// `PhantomData::default()` is not const-stable on all MSRVs, so we use the
// struct initialization syntax with the unit-like `PhantomData`.
impl<J> JobModel<J> {
    /// Create a job model. `max_attempts` is floored to 1.
    pub const fn new(kind: &'static str, version: i16, max_attempts: u32) -> Self {
        Self {
            kind,
            version,
            max_attempts: if max_attempts < 1 { 1 } else { max_attempts },
            payload_limit: DEFAULT_MAX_PAYLOAD_BYTES,
            _job: PhantomData,
        }
    }

    /// The job kind string.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// The payload version.
    pub fn version(&self) -> i16 {
        self.version
    }

    /// The default maximum number of attempts.
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// The maximum payload size in bytes.
    pub fn max_payload_bytes(&self) -> usize {
        self.payload_limit
    }
}

impl<J: Serialize + serde::de::DeserializeOwned> JobModel<J> {
    /// Set a custom maximum payload size.
    pub fn with_max_payload_bytes(mut self, bytes: usize) -> Self {
        self.payload_limit = bytes;
        self
    }

    /// Validate the model's kind and version. Called by enqueue and the
    /// registry so both agree on what a valid identity is.
    pub(crate) fn validate_identity(&self) -> Result<(), String> {
        validate_kind(self.kind)?;
        validate_version(self.version)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// RetryPolicy — exponential backoff with jitter and a cap.
// ---------------------------------------------------------------------------

/// The retry backoff policy.
///
/// `delay_for(attempts)` computes `base * multiplier^(attempts - 1)` (with
/// `attempts == 0` yielding `base`), capped at `cap`, with optional full
/// jitter. The computation is panic-free: NaN, negative, and infinite values
/// are clamped defensively.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    base: Duration,
    multiplier: f64,
    cap: Duration,
    jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::exponential(Duration::from_secs(1), 2.0, Duration::from_secs(3600))
    }
}

impl RetryPolicy {
    /// Exponential backoff: `base * multiplier^(attempts - 1)` capped at `cap`.
    pub const fn exponential(base: Duration, multiplier: f64, cap: Duration) -> Self {
        Self {
            base,
            multiplier,
            cap,
            jitter: false,
        }
    }

    /// Fixed delay (multiplier 1.0, cap = delay).
    pub const fn fixed(delay: Duration) -> Self {
        Self {
            base: delay,
            multiplier: 1.0,
            cap: delay,
            jitter: false,
        }
    }

    /// Enable or disable full jitter.
    pub fn jitter(self, enabled: bool) -> Self {
        Self {
            jitter: enabled,
            ..self
        }
    }

    /// Validate the policy. The multiplier must be finite and non-negative.
    pub fn validate(&self) -> Result<(), RetryPolicyError> {
        if self.multiplier.is_nan() || self.multiplier.is_infinite() {
            return Err(RetryPolicyError::MultiplierNotFinite {
                multiplier: self.multiplier,
            });
        }
        if self.multiplier < 0.0 {
            return Err(RetryPolicyError::MultiplierNegative {
                multiplier: self.multiplier,
            });
        }
        Ok(())
    }

    /// Validate and return self (convenience for builder chains).
    pub fn validated(self) -> Result<Self, RetryPolicyError> {
        self.validate()?;
        Ok(self)
    }

    /// Compute the delay before the next attempt.
    ///
    /// `attempts` is the post-increment count (1 for the first retry). The
    /// computation is panic-free: NaN, negative, and infinite values are
    /// clamped. With jitter enabled, a random value in `[0, delay]` is
    /// returned (full jitter).
    pub fn delay_for(&self, attempts: u32) -> Duration {
        if attempts == 0 {
            return self.base;
        }

        let cap_secs = self.cap.as_secs_f64();
        let exp = if attempts >= 40 {
            // Saturate: multiplier^40 for multiplier > 1 is astronomically
            // large anyway; for multiplier < 1 it is effectively 0. Either
            // way, 40 iterations is the loop cap.
            40
        } else {
            attempts - 1
        };

        let raw = if self.multiplier == 0.0 {
            0.0
        } else if self.multiplier == 1.0 {
            self.base.as_secs_f64()
        } else {
            let factor = self.multiplier.powi(i32::try_from(exp).unwrap_or(i32::MAX));
            self.base.as_secs_f64() * factor
        };

        // Defense-in-depth clamp (the validate() gate catches these earlier,
        // but delay_for is also called directly in tests).
        let clamped = if raw.is_nan() || raw < 0.0 {
            0.0
        } else if raw > cap_secs {
            cap_secs
        } else {
            raw
        };

        let mut delay = Duration::from_secs_f64(clamped);

        if self.jitter && !delay.is_zero() {
            // Full jitter: random in [0, delay]. Uses uuid::Uuid for the
            // random source (no new dependency; uuid is already a dep).
            let cap_nanos = delay.as_nanos();
            if cap_nanos > 0 {
                let rand = uuid::Uuid::new_v4().as_u128() % (cap_nanos + 1);
                delay = Duration::from_nanos(u64::try_from(rand).unwrap_or(0));
            }
        }

        delay
    }

    /// The base delay.
    pub fn base(&self) -> Duration {
        self.base
    }

    /// The cap.
    pub fn cap(&self) -> Duration {
        self.cap
    }

    /// Whether jitter is enabled.
    pub fn jitter_enabled(&self) -> bool {
        self.jitter
    }
}

// ---------------------------------------------------------------------------
// WorkerConfig — the worker's tuning knobs.
// ---------------------------------------------------------------------------

/// The worker configuration.
///
/// All durations are floored to 1 ms to avoid zero-duration busy loops. The
/// [`validate`](Self::validate) method enforces two invariants:
/// `job_timeout <= lease` (a timeout longer than the lease would guarantee
/// duplicate delivery) and `heartbeat_interval < lease` (a heartbeat at or
/// above the lease would never refresh in time).
#[derive(Debug, Clone, Copy)]
pub struct WorkerConfig {
    concurrency: usize,
    poll_interval: Duration,
    lease: Duration,
    poll_batch: i64,
    sweep_interval: Duration,
    sweep_batch: i64,
    job_timeout: Duration,
    heartbeat_interval: Option<Duration>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            concurrency: 8,
            poll_interval: Duration::from_millis(200),
            lease: Duration::from_secs(300),
            poll_batch: 8,
            sweep_interval: Duration::from_secs(30),
            sweep_batch: 64,
            job_timeout: Duration::from_secs(60),
            heartbeat_interval: None, // derived: lease / 3
        }
    }
}

impl WorkerConfig {
    /// Set the concurrency cap (number of in-flight jobs).
    pub fn concurrency(mut self, n: usize) -> Self {
        self.concurrency = if n < 1 { 1 } else { n };
        self
    }

    /// Set the poll interval (sleep between empty polls).
    pub fn poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = if d < Duration::from_millis(1) {
            Duration::from_millis(1)
        } else {
            d
        };
        self
    }

    /// Set the lease duration (how long a claim holds before sweep requeues).
    pub fn lease(mut self, d: Duration) -> Self {
        self.lease = if d < Duration::from_secs(1) {
            Duration::from_secs(1)
        } else {
            d
        };
        self
    }

    /// Set the poll batch size (jobs claimed per poll).
    pub fn poll_batch(mut self, n: i64) -> Self {
        self.poll_batch = if n < 1 { 1 } else { n };
        self
    }

    /// Set the sweep interval (how often expired leases are requeued).
    pub fn sweep_interval(mut self, d: Duration) -> Self {
        self.sweep_interval = if d < Duration::from_millis(1) {
            Duration::from_millis(1)
        } else {
            d
        };
        self
    }

    /// Set the sweep batch size.
    pub fn sweep_batch(mut self, n: i64) -> Self {
        self.sweep_batch = if n < 1 { 1 } else { n };
        self
    }

    /// Set the per-job timeout.
    pub fn job_timeout(mut self, d: Duration) -> Self {
        self.job_timeout = if d < Duration::from_millis(1) {
            Duration::from_millis(1)
        } else {
            d
        };
        self
    }

    /// Set an explicit heartbeat interval. If not set, the effective value is
    /// `lease / 3`.
    pub fn heartbeat_interval(mut self, d: Duration) -> Self {
        self.heartbeat_interval = Some(if d < Duration::from_millis(1) {
            Duration::from_millis(1)
        } else {
            d
        });
        self
    }

    /// Validate the configuration invariants.
    pub fn validate(&self) -> Result<(), WorkerConfigError> {
        if self.job_timeout > self.lease {
            return Err(WorkerConfigError::JobTimeoutExceedsLease {
                job_timeout: self.job_timeout,
                lease: self.lease,
            });
        }
        let hb = self.effective_heartbeat_interval();
        if hb >= self.lease {
            return Err(WorkerConfigError::HeartbeatIntervalNotBelowLease {
                heartbeat_interval: hb,
                lease: self.lease,
            });
        }
        Ok(())
    }

    /// Validate and return self (convenience for builder chains).
    pub fn validated(self) -> Result<Self, WorkerConfigError> {
        self.validate()?;
        Ok(self)
    }

    pub fn get_concurrency(&self) -> usize {
        self.concurrency
    }
    pub fn get_poll_interval(&self) -> Duration {
        self.poll_interval
    }
    pub fn get_lease(&self) -> Duration {
        self.lease
    }
    pub fn get_poll_batch(&self) -> i64 {
        self.poll_batch
    }
    pub fn get_sweep_interval(&self) -> Duration {
        self.sweep_interval
    }
    pub fn get_sweep_batch(&self) -> i64 {
        self.sweep_batch
    }
    pub fn get_job_timeout(&self) -> Duration {
        self.job_timeout
    }
    pub fn get_heartbeat_interval(&self) -> Duration {
        self.effective_heartbeat_interval()
    }

    fn effective_heartbeat_interval(&self) -> Duration {
        match self.heartbeat_interval {
            Some(v) => v,
            None => {
                let third = self.lease / 3;
                if third < Duration::from_millis(1) {
                    Duration::from_millis(1)
                } else {
                    third
                }
            }
        }
    }
}
