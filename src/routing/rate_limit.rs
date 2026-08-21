//! Token-bucket rate limiting.
//!
//! # The shape
//!
//! A [`RateLimit`] is a handle the application holds and a [`tower::Layer`]
//! at the same time, exactly like [`Maintenance`](crate::http::Maintenance).
//! Every clone shares one set of buckets, so the same value can be installed
//! on the whole application and on a single route without the two becoming
//! two independent limits:
//!
//! ```
//! use arcature::routing::RateLimit;
//!
//! let limit = RateLimit::per_minute(60);
//! assert_eq!(limit.limit(), 60);
//! ```
//!
//! There is no registry to look a limiter up in. If nothing holds the handle,
//! nothing is limited.
//!
//! # Token bucket, not fixed window
//!
//! A fixed window lets a client spend its whole allowance in the last instant
//! of one window and again in the first instant of the next -- twice the
//! nominal rate across the boundary. A token bucket refills continuously:
//! `limit` tokens per `window`, capped at `burst` (which defaults to `limit`).
//! A request costs one token.
//!
//! # Two backends
//!
//! * **In memory** (the default). Per process. Three instances behind a load
//!   balancer enforce three times the limit between them, which is fine for
//!   shedding accidental load and is not fine as a security control.
//! * **Redis/Valkey** via [`Cache`](crate::cache::Cache), feature `cache`.
//!   One bucket per key across every instance, refilled by a Lua script so
//!   the read-modify-write is atomic.
//!
//! # What a refusal looks like
//!
//! `429` with an RFC 9457 [`Problem`](crate::api::Problem) body, a
//! `Retry-After` header, and the `RateLimit-*` headers from
//! [draft-ietf-httpapi-ratelimit-headers]. The `RateLimit-*` headers are on
//! successful responses too -- a client that can see it is running out has
//! somewhere to slow down before it is refused.
//!
//! [draft-ietf-httpapi-ratelimit-headers]: https://datatracker.ietf.org/doc/draft-ietf-httpapi-ratelimit-headers/

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::http::{HeaderName, HeaderValue, Request, Response, header};
use tower::{Layer, Service};

use crate::api::{Problem, ProblemKind};

/// `RateLimit-Limit`: the quota, in requests per window.
pub const RATELIMIT_LIMIT: HeaderName = HeaderName::from_static("ratelimit-limit");
/// `RateLimit-Remaining`: whole requests still available right now.
pub const RATELIMIT_REMAINING: HeaderName = HeaderName::from_static("ratelimit-remaining");
/// `RateLimit-Reset`: seconds until the bucket is full again.
pub const RATELIMIT_RESET: HeaderName = HeaderName::from_static("ratelimit-reset");

/// The bucket key used when [`KeySource::Ip`] cannot identify the peer.
///
/// A shared bucket, deliberately: an unidentifiable client must not be an
/// unlimited one. See [`KeySource::Ip`] for how to make sure this is never
/// reached.
pub const UNIDENTIFIED_KEY: &str = "unidentified";

/// How many keys the in-memory backend tolerates before it sweeps.
const SWEEP_AT: usize = 8192;

// ---------------------------------------------------------------------------
// KeySource
// ---------------------------------------------------------------------------

/// What a request is bucketed by.
#[derive(Clone)]
pub enum KeySource {
    /// The peer address, read from
    /// [`ConnectInfo`](axum::extract::ConnectInfo).
    ///
    /// The extension is only present when the server was started with
    /// `into_make_service_with_connect_info::<SocketAddr>()`. Without it
    /// every request falls into the shared [`UNIDENTIFIED_KEY`] bucket, which
    /// is safe but useless -- so check that first if a limiter seems to be
    /// refusing far too eagerly.
    ///
    /// Behind a reverse proxy the peer address is the proxy. Use
    /// [`KeySource::Header`] with whichever header the proxy is *trusted* to
    /// set; a forwarding header from an untrusted hop is client-controlled
    /// and turns the limiter off.
    Ip,
    /// A request header's value -- an API key, a tenant id, or a forwarding
    /// header set by a trusted proxy.
    ///
    /// A request without the header falls into [`UNIDENTIFIED_KEY`].
    Header(HeaderName),
    /// One bucket for everything: a ceiling on total throughput rather than a
    /// per-client quota.
    Global,
    /// Anything else -- an authenticated user id out of request extensions,
    /// say. `None` falls into [`UNIDENTIFIED_KEY`].
    Custom(KeyFn),
}

/// The closure behind [`KeySource::Custom`]: it reads a bucket key out of a
/// request, or returns `None` when the request carries nothing to key on.
///
/// `Arc` rather than `Box` because a [`RateLimit`] is cloned into every
/// service the layer builds, and a boxed closure cannot be cloned.
pub type KeyFn = Arc<dyn Fn(&Request<axum::body::Body>) -> Option<String> + Send + Sync>;

impl fmt::Debug for KeySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ip => f.write_str("Ip"),
            Self::Header(name) => write!(f, "Header({name})"),
            Self::Global => f.write_str("Global"),
            Self::Custom(_) => f.write_str("Custom(..)"),
        }
    }
}

impl KeySource {
    /// The bucket key for one request.
    fn key_for(&self, request: &Request<axum::body::Body>) -> String {
        let resolved = match self {
            Self::Ip => request
                .extensions()
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|info| info.0.ip().to_string()),
            Self::Header(name) => request
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            Self::Global => Some(String::from("global")),
            Self::Custom(f) => f(request).filter(|key| !key.is_empty()),
        };
        resolved.unwrap_or_else(|| UNIDENTIFIED_KEY.to_string())
    }
}

/// What to do when the shared backend cannot be reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnBackendError {
    /// Answer `503` with an RFC 9457 problem. The default: a limiter that
    /// stops limiting the moment its backend blinks is not a limit.
    ///
    /// The status is `503`, not `429`: the client exceeded nothing, the
    /// server lost the ability to tell.
    Refuse,
    /// Let the request through. For a limiter that is shedding accidental
    /// load rather than enforcing a security boundary, an outage of the
    /// limiter should not become an outage of the site.
    Allow,
}

// ---------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------

/// The outcome of one bucket check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    /// Whether the request may proceed.
    pub allowed: bool,
    /// Whole tokens left after this request.
    pub remaining: u32,
    /// Seconds until the bucket is full again (`RateLimit-Reset`).
    pub reset_after: u64,
    /// Seconds until one token is available (`Retry-After`), when refused.
    pub retry_after: u64,
}

/// The refill parameters, shared by both backends.
#[derive(Debug, Clone, Copy)]
struct Quota {
    /// Requests per window -- the number reported as `RateLimit-Limit`.
    limit: u32,
    /// Bucket capacity. Equal to `limit` unless `burst` was set.
    capacity: f64,
    /// Tokens added per second.
    refill_per_sec: f64,
}

impl Quota {
    /// Apply `elapsed` of refill to `tokens`, spend one if there is one, and
    /// describe the result.
    ///
    /// Shared by both backends so the arithmetic cannot drift between them:
    /// the Redis script does the same thing in Lua, and the tests here pin
    /// the numbers.
    fn settle(self, tokens: f64) -> (f64, Decision) {
        let (left, allowed) = if tokens >= 1.0 {
            (tokens - 1.0, true)
        } else {
            (tokens, false)
        };
        (left, self.describe(left, allowed))
    }

    /// Turn a post-spend token count into the headers a client sees.
    ///
    /// Split out from [`Quota::settle`] because the Redis backend spends its
    /// token inside a Lua script and only gets the count back; both backends
    /// still report the same numbers for the same state.
    fn describe(self, tokens_left: f64, allowed: bool) -> Decision {
        let deficit = (self.capacity - tokens_left).max(0.0);
        Decision {
            allowed,
            remaining: tokens_left.max(0.0) as u32,
            reset_after: seconds_to_accrue(deficit, self.refill_per_sec),
            retry_after: if allowed {
                0
            } else {
                seconds_to_accrue(1.0 - tokens_left, self.refill_per_sec).max(1)
            },
        }
    }
}

/// Seconds, rounded up, for `tokens` to accrue at `rate` per second.
fn seconds_to_accrue(tokens: f64, rate: f64) -> u64 {
    if tokens <= 0.0 || rate <= 0.0 {
        return 0;
    }
    (tokens / rate).ceil() as u64
}

// ---------------------------------------------------------------------------
// In-memory backend
// ---------------------------------------------------------------------------

/// One key's bucket.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    updated: Instant,
}

/// The in-memory bucket table.
///
/// A `std::sync::Mutex` rather than an async one: the critical section is a
/// hash lookup and three arithmetic operations, so a task never waits on it
/// long enough for an async mutex to earn its cost.
#[derive(Debug, Default)]
struct MemoryBuckets {
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl MemoryBuckets {
    fn check(&self, key: &str, quota: Quota, now: Instant) -> Decision {
        let mut buckets = match self.buckets.lock() {
            Ok(guard) => guard,
            // A panic inside the critical section leaves the map structurally
            // fine -- it only ever holds plain numbers -- so the honest
            // recovery is to carry on rather than to poison every subsequent
            // request.
            Err(poisoned) => poisoned.into_inner(),
        };

        if buckets.len() >= SWEEP_AT {
            // A bucket that has refilled to capacity carries no information:
            // recreating it lazily gives exactly the same answer. Dropping
            // those is what keeps an unbounded key space (one per IP) from
            // being an unbounded map.
            buckets.retain(|_, bucket| {
                refilled(
                    bucket.tokens,
                    quota,
                    now.saturating_duration_since(bucket.updated),
                ) < quota.capacity
            });
        }

        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: quota.capacity,
            updated: now,
        });
        let tokens = refilled(
            bucket.tokens,
            quota,
            now.saturating_duration_since(bucket.updated),
        );
        let (left, decision) = quota.settle(tokens);
        bucket.tokens = left;
        bucket.updated = now;
        decision
    }
}

/// `tokens` after `elapsed` of refill, capped at capacity.
fn refilled(tokens: f64, quota: Quota, elapsed: Duration) -> f64 {
    (tokens + elapsed.as_secs_f64() * quota.refill_per_sec).min(quota.capacity)
}

// ---------------------------------------------------------------------------
// Redis backend
// ---------------------------------------------------------------------------

/// The refill-and-spend step, as one Redis round trip.
///
/// Read-modify-write on a shared bucket has to be atomic or two instances
/// racing on the same key both see the same token count and both spend it.
/// `EVAL` is how that is done without a lock: the whole step runs inside the
/// server. The script is sent with every call rather than cached under its
/// digest because `redis`'s `Script` helper lives behind the crate's `script`
/// feature, which this build does not enable; the body is a few hundred bytes
/// and travels on a connection that is already open.
///
/// `now` is supplied by the caller rather than read from `TIME` so the script
/// stays deterministic, which is what makes it replicable and safe to run on
/// a replica-backed deployment.
#[cfg(feature = "cache")]
const BUCKET_SCRIPT: &str = r"
local capacity      = tonumber(ARGV[1])
local refill_per_ms = tonumber(ARGV[2])
local now_ms        = tonumber(ARGV[3])
local ttl_ms        = tonumber(ARGV[4])
local state   = redis.call('HMGET', KEYS[1], 't', 'u')
local tokens  = tonumber(state[1])
local updated = tonumber(state[2])
if tokens == nil or updated == nil then
  tokens  = capacity
  updated = now_ms
end
local elapsed = now_ms - updated
if elapsed < 0 then elapsed = 0 end
tokens = math.min(capacity, tokens + elapsed * refill_per_ms)
local allowed = 0
if tokens >= 1 then
  tokens  = tokens - 1
  allowed = 1
end
redis.call('HSET', KEYS[1], 't', tokens, 'u', now_ms)
redis.call('PEXPIRE', KEYS[1], ttl_ms)
return {allowed, math.floor(tokens * 1000)}
";

/// The Redis-backed bucket table.
#[cfg(feature = "cache")]
struct RedisBuckets {
    cache: crate::cache::Cache,
}

#[cfg(feature = "cache")]
impl fmt::Debug for RedisBuckets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisBuckets").finish_non_exhaustive()
    }
}

#[cfg(feature = "cache")]
impl RedisBuckets {
    fn new(cache: crate::cache::Cache) -> Self {
        Self { cache }
    }

    /// Run the script. `Err` means the backend could not be reached; the
    /// caller decides what that means via [`OnBackendError`].
    async fn check(&self, key: &str, quota: Quota) -> Result<Decision, ()> {
        let full_key = self.cache.resolve_key(&format!("ratelimit:{key}"));
        let now_ms = unix_millis();
        // Twice the time it takes to refill from empty: long enough that a
        // bucket cannot expire while it still owes a client tokens, short
        // enough that idle keys leave.
        let ttl_ms = (((quota.capacity / quota.refill_per_sec) * 2000.0) as u64).max(1000);
        let mut connection = self.cache.connection_for_op();
        let outcome: Result<(i64, i64), _> = redis::cmd("EVAL")
            .arg(BUCKET_SCRIPT)
            .arg(1_i64)
            .arg(full_key)
            .arg(quota.capacity)
            .arg(quota.refill_per_sec / 1000.0)
            .arg(now_ms)
            .arg(ttl_ms)
            .query_async(&mut connection)
            .await;
        match outcome {
            Ok((allowed, milli_tokens)) => {
                Ok(quota.describe(milli_tokens as f64 / 1000.0, allowed == 1))
            }
            Err(_) => Err(()),
        }
    }
}

/// Wall-clock milliseconds since the Unix epoch.
///
/// Wall clock rather than a monotonic instant because the value crosses
/// process boundaries. Only differences are used, and a backwards difference
/// (two instances whose clocks disagree) is clamped to zero inside the
/// script, so skew costs a client a little refill and never grants any.
#[cfg(feature = "cache")]
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// RateLimit
// ---------------------------------------------------------------------------

/// Where the buckets live.
#[derive(Debug, Clone)]
enum Backend {
    Memory(Arc<MemoryBuckets>),
    #[cfg(feature = "cache")]
    Redis(Arc<RedisBuckets>),
}

/// A token-bucket rate limit: a handle, and a [`tower::Layer`].
///
/// Install it on the whole application through
/// `ApplicationBuilder::rate_limit`, on a group with
/// [`RouteGroup::layer`](crate::routing::RouteGroup::layer), or on one route
/// with [`Route::layer`](crate::routing::Route::layer). Cloning shares the
/// buckets, so a value used in two places is still one limit.
#[derive(Debug, Clone)]
pub struct RateLimit {
    quota: Quota,
    key: KeySource,
    backend: Backend,
    on_backend_error: OnBackendError,
}

impl RateLimit {
    /// `limit` requests per `window`, bucketed by peer address.
    ///
    /// `window` is clamped to a millisecond: `n` requests per zero time has
    /// no reading, and a division by zero is not the place to find that out.
    #[must_use]
    pub fn new(limit: u32, window: Duration) -> Self {
        let window = window.max(Duration::from_millis(1));
        Self {
            quota: Quota {
                limit,
                capacity: f64::from(limit),
                refill_per_sec: f64::from(limit) / window.as_secs_f64(),
            },
            key: KeySource::Ip,
            backend: Backend::Memory(Arc::new(MemoryBuckets::default())),
            on_backend_error: OnBackendError::Refuse,
        }
    }

    /// `limit` requests per second.
    #[must_use]
    pub fn per_second(limit: u32) -> Self {
        Self::new(limit, Duration::from_secs(1))
    }

    /// `limit` requests per minute.
    #[must_use]
    pub fn per_minute(limit: u32) -> Self {
        Self::new(limit, Duration::from_secs(60))
    }

    /// `limit` requests per hour.
    #[must_use]
    pub fn per_hour(limit: u32) -> Self {
        Self::new(limit, Duration::from_secs(3600))
    }

    /// Allow a burst of up to `burst` requests before the sustained rate
    /// applies. Defaults to the limit itself.
    #[must_use]
    pub fn burst(mut self, burst: u32) -> Self {
        self.quota.capacity = f64::from(burst);
        self
    }

    /// Bucket by something other than the peer address.
    #[must_use]
    pub fn by(mut self, key: KeySource) -> Self {
        self.key = key;
        self
    }

    /// Bucket by a function of the request.
    #[must_use]
    pub fn by_fn<F>(self, f: F) -> Self
    where
        F: Fn(&Request<axum::body::Body>) -> Option<String> + Send + Sync + 'static,
    {
        self.by(KeySource::Custom(Arc::new(f)))
    }

    /// Share the buckets across every instance through Redis/Valkey.
    ///
    /// The cache handle's namespace applies, and the keys are prefixed
    /// `ratelimit:` under it.
    #[cfg(feature = "cache")]
    #[must_use]
    pub fn redis(mut self, cache: crate::cache::Cache) -> Self {
        self.backend = Backend::Redis(Arc::new(RedisBuckets::new(cache)));
        self
    }

    /// What to do when the shared backend cannot be reached. Defaults to
    /// [`OnBackendError::Refuse`].
    #[must_use]
    pub fn on_backend_error(mut self, behaviour: OnBackendError) -> Self {
        self.on_backend_error = behaviour;
        self
    }

    /// The configured quota, in requests per window.
    #[must_use]
    pub fn limit(&self) -> u32 {
        self.quota.limit
    }

    /// The bucket capacity -- the largest burst allowed from idle.
    #[must_use]
    pub fn capacity(&self) -> u32 {
        self.quota.capacity as u32
    }

    /// Tokens added per second.
    #[must_use]
    pub fn refill_per_second(&self) -> f64 {
        self.quota.refill_per_sec
    }
}

/// The outcome of a check, including the "backend is down" case that has no
/// bucket state behind it.
///
/// The unreachable case exists only for a shared backend: an in-memory
/// bucket table cannot be down, so without the `cache` feature there is no
/// code path that could produce it and the variant is not compiled.
enum Checked {
    Decided(Decision),
    #[cfg(feature = "cache")]
    BackendDown,
}

impl RateLimit {
    /// Check one key against its bucket.
    async fn check(&self, key: &str) -> Checked {
        match &self.backend {
            Backend::Memory(buckets) => {
                Checked::Decided(buckets.check(key, self.quota, Instant::now()))
            }
            #[cfg(feature = "cache")]
            Backend::Redis(buckets) => match buckets.check(key, self.quota).await {
                Ok(decision) => Checked::Decided(decision),
                Err(()) => Checked::BackendDown,
            },
        }
    }
}

impl<S> Layer<S> for RateLimit {
    type Service = RateLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService {
            inner,
            limit: self.clone(),
        }
    }
}

/// The service [`RateLimit`] wraps around.
#[derive(Debug, Clone)]
pub struct RateLimitService<S> {
    inner: S,
    limit: RateLimit,
}

impl<S> Service<Request<axum::body::Body>> for RateLimitService<S>
where
    S: Service<
            Request<axum::body::Body>,
            Response = Response<axum::body::Body>,
            Error = Infallible,
        > + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = Response<axum::body::Body>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<axum::body::Body>) -> Self::Future {
        let limit = self.limit.clone();
        let key = limit.key.key_for(&request);

        // Swap in the clone and drive the original: only the original is
        // known ready, and `poll_ready` readiness does not survive cloning.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            // Without the `cache` feature there is only the in-memory
            // backend, which cannot fail, so `Checked` has one variant and
            // clippy would rather see a `let`. The `match` is what makes the
            // two builds one piece of code.
            #[allow(
                clippy::infallible_destructuring_match,
                reason = "the second arm exists under the `cache` feature"
            )]
            let decision = match limit.check(&key).await {
                Checked::Decided(decision) => decision,
                #[cfg(feature = "cache")]
                Checked::BackendDown => match limit.on_backend_error {
                    OnBackendError::Refuse => return Ok(backend_unavailable()),
                    // Nothing is known about the bucket, so nothing is
                    // reported: no `RateLimit-*` headers rather than
                    // invented ones.
                    OnBackendError::Allow => return inner.call(request).await,
                },
            };

            if !decision.allowed {
                return Ok(refused(limit.quota.limit, decision));
            }
            let mut response = inner.call(request).await?;
            annotate(response.headers_mut(), limit.quota.limit, decision);
            Ok(response)
        })
    }
}

/// Put the `RateLimit-*` headers on a response.
fn annotate(headers: &mut axum::http::HeaderMap, limit: u32, decision: Decision) {
    for (name, value) in [
        (RATELIMIT_LIMIT, u64::from(limit)),
        (RATELIMIT_REMAINING, u64::from(decision.remaining)),
        (RATELIMIT_RESET, decision.reset_after),
    ] {
        if let Ok(value) = HeaderValue::from_str(&value.to_string()) {
            headers.insert(name, value);
        }
    }
}

/// The `429`: an RFC 9457 problem, `Retry-After`, and the `RateLimit-*`
/// headers.
fn refused(limit: u32, decision: Decision) -> Response<axum::body::Body> {
    use axum::response::IntoResponse as _;

    let mut response = Problem::of(ProblemKind::RateLimit)
        .with_detail("Too many requests. Slow down and try again shortly.")
        .into_response();
    annotate(response.headers_mut(), limit, decision);
    if let Ok(value) = HeaderValue::from_str(&decision.retry_after.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// The `503` for [`OnBackendError::Refuse`].
///
/// Not a `429`: the client exceeded nothing. Saying `429` here would tell a
/// well-behaved client to back off for a quota problem it does not have, and
/// would hide a backend outage behind a client-error status.
///
/// Only a shared backend can be unreachable, so without `cache` there is no
/// caller.
#[cfg(feature = "cache")]
fn backend_unavailable() -> Response<axum::body::Body> {
    use axum::response::IntoResponse as _;

    Problem::of(ProblemKind::Unavailable)
        .with_detail("The rate limiter is unavailable. Please try again shortly.")
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_per_minute_limit_refills_at_one_per_second() {
        let limit = RateLimit::per_minute(60);
        assert_eq!(limit.limit(), 60);
        assert_eq!(limit.capacity(), 60);
        assert!((limit.refill_per_second() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn burst_raises_the_capacity_without_raising_the_rate() {
        let limit = RateLimit::per_minute(60).burst(120);
        assert_eq!(limit.limit(), 60);
        assert_eq!(limit.capacity(), 120);
        assert!((limit.refill_per_second() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_zero_window_does_not_divide_by_zero() {
        let limit = RateLimit::new(10, Duration::ZERO);
        assert!(limit.refill_per_second().is_finite());
    }

    #[test]
    fn a_bucket_empties_and_then_refuses() {
        let buckets = MemoryBuckets::default();
        let quota = RateLimit::per_second(3).quota;
        let now = Instant::now();
        for expected_remaining in [2u32, 1, 0] {
            let decision = buckets.check("k", quota, now);
            assert!(decision.allowed);
            assert_eq!(decision.remaining, expected_remaining);
        }
        let decision = buckets.check("k", quota, now);
        assert!(!decision.allowed);
        assert_eq!(decision.remaining, 0);
        assert_eq!(decision.retry_after, 1);
    }

    #[test]
    fn a_bucket_refills_over_time() {
        let buckets = MemoryBuckets::default();
        let quota = RateLimit::per_second(2).quota;
        let now = Instant::now();
        assert!(buckets.check("k", quota, now).allowed);
        assert!(buckets.check("k", quota, now).allowed);
        assert!(!buckets.check("k", quota, now).allowed);
        // One second later two tokens have accrued, capped at capacity.
        let later = now + Duration::from_secs(1);
        assert!(buckets.check("k", quota, later).allowed);
        assert!(buckets.check("k", quota, later).allowed);
        assert!(!buckets.check("k", quota, later).allowed);
    }

    #[test]
    fn buckets_do_not_leak_across_keys() {
        let buckets = MemoryBuckets::default();
        let quota = RateLimit::per_second(1).quota;
        let now = Instant::now();
        assert!(buckets.check("a", quota, now).allowed);
        assert!(!buckets.check("a", quota, now).allowed);
        assert!(buckets.check("b", quota, now).allowed);
    }

    #[test]
    fn a_zero_limit_refuses_everything() {
        let buckets = MemoryBuckets::default();
        let quota = RateLimit::new(0, Duration::from_secs(1)).quota;
        let decision = buckets.check("k", quota, Instant::now());
        assert!(!decision.allowed);
        // No refill will ever produce a token, so there is no honest
        // `Retry-After`; the floor of one second is the least misleading
        // answer available.
        assert_eq!(decision.retry_after, 1);
    }

    #[test]
    fn reset_counts_down_to_a_full_bucket() {
        let quota = RateLimit::per_second(10).quota;
        let (_, decision) = quota.settle(10.0);
        assert!(decision.allowed);
        assert_eq!(decision.remaining, 9);
        assert_eq!(decision.reset_after, 1);
        assert_eq!(decision.retry_after, 0);
    }

    #[test]
    fn an_unidentified_request_gets_the_shared_bucket() {
        let request = Request::builder()
            .uri("/")
            .body(axum::body::Body::empty())
            .expect("request builds");
        assert_eq!(KeySource::Ip.key_for(&request), UNIDENTIFIED_KEY);
        assert_eq!(
            KeySource::Header(HeaderName::from_static("x-api-key")).key_for(&request),
            UNIDENTIFIED_KEY
        );
        assert_eq!(KeySource::Global.key_for(&request), "global");
    }

    #[test]
    fn a_header_key_source_reads_the_header() {
        let request = Request::builder()
            .uri("/")
            .header("x-api-key", "abc")
            .body(axum::body::Body::empty())
            .expect("request builds");
        assert_eq!(
            KeySource::Header(HeaderName::from_static("x-api-key")).key_for(&request),
            "abc"
        );
    }
}
