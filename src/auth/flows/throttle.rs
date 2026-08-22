//! Making a sign-in form expensive to guess against.
//!
//! A login form that answers in constant time and says nothing about which
//! half was wrong -- everything [`CredentialChecker`](super::CredentialChecker)
//! is for -- is still a free oracle if it will answer ten thousand times. The
//! guard for that is a counter, and the whole question is *what it counts*.

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Failures allowed against one address-and-client pair per window.
///
/// Five, matching the number a person who has genuinely forgotten which of
/// their passwords they used here will burn through and no more.
const DEFAULT_PER_IDENTITY: u32 = 5;

/// Failures allowed from one client per window, across every address it tries.
///
/// Ten times the per-identity figure rather than equal to it: this bucket is
/// aimed at a spray across many accounts, not at somebody mistyping their own
/// password, and setting it too close to the first would lock out a shared
/// office or campus NAT on an ordinary afternoon. See
/// [`per_address`](LoginThrottle::per_address) for the trade being made.
const DEFAULT_PER_ADDRESS: u32 = 50;

/// How long a full bucket takes to empty.
const DEFAULT_WINDOW: Duration = Duration::from_secs(15 * 60);

/// The floor on a reported wait.
///
/// A `Retry-After` of zero reads as "try again immediately", which is the
/// opposite of what a refusal means, and rounding can produce one.
const MINIMUM_RETRY_AFTER: Duration = Duration::from_secs(1);

/// How large the bucket table may grow before idle entries are dropped.
///
/// The same figure as [`routing::rate_limit`](crate::routing::rate_limit),
/// for the same reason: sweeping on a size trigger rather than on a timer
/// keeps the memory bound without a background task.
const SWEEP_AT: usize = 8192;

/// Domain separator for the address-and-client bucket.
const IDENTITY_DOMAIN: u8 = 1;

/// Domain separator for the client-only bucket.
const ADDRESS_DOMAIN: u8 = 2;

/// Stand-in for a client whose address is not known.
const UNATTRIBUTED: &[u8] = b"arcature/login-throttle/unattributed";

/// Whether another sign-in attempt may be made.
///
/// ```
/// use arcature::auth::flows::ThrottleDecision;
///
/// assert!(ThrottleDecision::Allowed.is_allowed());
/// assert_eq!(ThrottleDecision::Allowed.retry_after(), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ThrottleDecision {
    /// Go ahead and verify the credentials.
    Allowed,
    /// Too many failures have been recorded. Refuse without verifying
    /// anything.
    TooManyAttempts {
        /// How long until one more attempt would be allowed. Suitable for a
        /// `Retry-After` header, and never zero.
        retry_after: Duration,
    },
}

impl ThrottleDecision {
    /// Whether the attempt may proceed.
    ///
    /// ```
    /// use arcature::auth::flows::ThrottleDecision;
    ///
    /// assert!(ThrottleDecision::Allowed.is_allowed());
    /// ```
    #[must_use]
    pub fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// The wait, if the attempt was refused.
    ///
    /// ```
    /// use arcature::auth::flows::ThrottleDecision;
    /// use std::time::Duration;
    ///
    /// let refused = ThrottleDecision::TooManyAttempts {
    ///     retry_after: Duration::from_secs(60),
    /// };
    /// assert_eq!(refused.retry_after(), Some(Duration::from_secs(60)));
    /// ```
    #[must_use]
    pub fn retry_after(self) -> Option<Duration> {
        match self {
            Self::Allowed => None,
            Self::TooManyAttempts { retry_after } => Some(retry_after),
        }
    }
}

/// One bucket's worth of allowance.
#[derive(Debug, Clone, Copy)]
struct Quota {
    /// Failures tolerated before the bucket is empty.
    capacity: f64,
    /// Failures forgiven per second.
    refill_per_sec: f64,
}

impl Quota {
    fn new(limit: u32, window: Duration) -> Self {
        // A limit of zero would mean "nobody may ever attempt a sign-in",
        // which is not a throttle setting, and it divides by zero on the way
        // to saying so. A window of zero divides by zero outright.
        let limit = f64::from(limit.max(1));
        let window = window.max(Duration::from_millis(1));
        Self {
            capacity: limit,
            refill_per_sec: limit / window.as_secs_f64(),
        }
    }
}

/// What is known about one key.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    /// Failures still tolerated, as of `updated`.
    tokens: f64,
    /// When `tokens` was last correct.
    updated: Instant,
}

impl Bucket {
    /// Tokens available now, without writing anything back.
    fn tokens_at(self, quota: Quota, now: Instant) -> f64 {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        (self.tokens + elapsed * quota.refill_per_sec).min(quota.capacity)
    }
}

/// Refuses sign-in attempts once too many have failed, keyed on the address
/// being tried and the client trying it.
///
/// # Why this is not a [`RateLimit`](crate::routing::RateLimit) layer
///
/// Not taste -- it cannot be one. A rate-limit layer keys requests through
/// `Fn(&Request<Body>) -> Option<String>`, and it runs *before* the body is
/// read, because reading the body is the handler's job and a layer that
/// consumed it would leave the handler nothing to parse. The address being
/// signed in to is in that body. So a layer can throttle the login *route*,
/// by client, and that is worth having -- but it cannot tell one account's
/// attempts from another's, which is the distinction the whole mechanism
/// turns on.
///
/// This is therefore a handle the handler calls, after parsing the form and
/// before verifying anything. Keep it in application state; it is [`Clone`]
/// and clones share the counters.
///
/// # What it counts, and why that matters
///
/// **Failures, not attempts.** A person who signs in successfully forty times
/// in an afternoon is not attacking anything, and a throttle that counted
/// attempts would eventually stop them. So the handler reports the outcome:
/// [`check`](Self::check) before verifying,
/// [`record_failure`](Self::record_failure) after a rejection,
/// [`record_success`](Self::record_success) after an acceptance.
///
/// Two buckets are consulted, and either can refuse:
///
/// * **The address and the client together.** Five failures against
///   `you@example.com` from one client. Keyed on both, so that an attacker
///   cannot lock a victim out of their own account by failing five times
///   against it from somewhere else -- which is what a bucket keyed on the
///   address alone would hand them.
/// * **The client alone.** That first bucket does nothing against the actual
///   shape of a credential-stuffing run, which is one failure each against
///   ten thousand *different* addresses: every one of those buckets is fresh.
///   The second bucket is what notices.
///
/// # An address nobody has registered is throttled identically
///
/// There is no lookup here and no way to do one. An unknown address gets a
/// bucket exactly as a real one does, fills it at the same rate, and is
/// refused with the same wait. Anything else would rebuild, in the throttle,
/// precisely the enumeration oracle that
/// [`CredentialChecker`](super::CredentialChecker) exists to close: *submit an
/// address, see whether it can be locked out, learn whether it has an
/// account.*
///
/// # What it does not do
///
/// It does not find the user, verify the password, write a response, or
/// remember anything across a restart. The counters are in this process's
/// memory, which means a deploy forgives everybody and a second instance
/// counts separately. For a login form that is a reasonable place to land --
/// an attacker cannot make you deploy -- but it is worth knowing before
/// scaling out.
///
/// ```
/// use arcature::auth::flows::LoginThrottle;
/// use std::net::{IpAddr, Ipv4Addr};
///
/// let throttle = LoginThrottle::new().per_identity(2);
/// let client = Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)));
///
/// // Two failures are tolerated.
/// assert!(throttle.check("you@example.com", client).is_allowed());
/// throttle.record_failure("you@example.com", client);
/// assert!(throttle.check("you@example.com", client).is_allowed());
/// throttle.record_failure("you@example.com", client);
///
/// // The third attempt never reaches the password check.
/// let refused = throttle.check("you@example.com", client);
/// assert!(!refused.is_allowed());
/// assert!(refused.retry_after().is_some());
///
/// // A different address from the same client still has its own allowance...
/// assert!(throttle.check("someone-else@example.com", client).is_allowed());
///
/// // ...and signing in successfully clears the account that was blocked.
/// throttle.record_success("you@example.com", client);
/// assert!(throttle.check("you@example.com", client).is_allowed());
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub struct LoginThrottle {
    /// Allowance for one address from one client.
    identity: Quota,
    /// Allowance for one client across all addresses.
    address: Quota,
    /// How long a full bucket takes to empty, kept so the builders can
    /// recompute both quotas when it changes.
    window: Duration,
    /// Failures tolerated per identity, kept for the same reason.
    identity_limit: u32,
    /// Failures tolerated per client, kept for the same reason.
    address_limit: u32,
    /// The counters. Shared across clones, because a clone is the same
    /// throttle.
    buckets: Arc<Mutex<HashMap<[u8; 32], Bucket>>>,
}

impl Default for LoginThrottle {
    fn default() -> Self {
        Self::new()
    }
}

impl LoginThrottle {
    /// A throttle at the default limits: five failures per address per client
    /// and fifty per client, both forgiven over fifteen minutes.
    ///
    /// ```
    /// use arcature::auth::flows::LoginThrottle;
    ///
    /// let throttle = LoginThrottle::new();
    /// assert!(throttle.check("you@example.com", None).is_allowed());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            identity: Quota::new(DEFAULT_PER_IDENTITY, DEFAULT_WINDOW),
            address: Quota::new(DEFAULT_PER_ADDRESS, DEFAULT_WINDOW),
            window: DEFAULT_WINDOW,
            identity_limit: DEFAULT_PER_IDENTITY,
            address_limit: DEFAULT_PER_ADDRESS,
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Failures tolerated against one address from one client.
    ///
    /// Set the limits before cloning. Clones share the counters but keep
    /// their own limits, and two handles disagreeing about the capacity of
    /// one bucket table is a configuration with no clear reading.
    ///
    /// ```
    /// use arcature::auth::flows::LoginThrottle;
    ///
    /// let throttle = LoginThrottle::new().per_identity(3);
    /// for _ in 0..3 {
    ///     throttle.record_failure("you@example.com", None);
    /// }
    /// assert!(!throttle.check("you@example.com", None).is_allowed());
    /// ```
    #[must_use]
    pub fn per_identity(mut self, limit: u32) -> Self {
        self.identity_limit = limit;
        self.identity = Quota::new(limit, self.window);
        self
    }

    /// Failures tolerated from one client across every address it tries.
    ///
    /// The trade this number makes is against shared egress. Everybody behind
    /// one office, campus, or mobile-carrier NAT arrives as a single address,
    /// so a figure tight enough to bite a spray quickly is also tight enough
    /// to lock out a building. The default leaves it ten times looser than
    /// the per-identity limit; raise it if you serve a population that shares
    /// addresses, and lower it if every client is its own.
    ///
    /// ```
    /// use arcature::auth::flows::LoginThrottle;
    /// use std::net::{IpAddr, Ipv4Addr};
    ///
    /// let throttle = LoginThrottle::new().per_address(2);
    /// let client = Some(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 9)));
    ///
    /// // Two failures, against two different accounts.
    /// throttle.record_failure("one@example.com", client);
    /// throttle.record_failure("two@example.com", client);
    ///
    /// // A third account gets nowhere: the client is out of allowance even
    /// // though this address has not been tried once.
    /// assert!(!throttle.check("three@example.com", client).is_allowed());
    /// ```
    #[must_use]
    pub fn per_address(mut self, limit: u32) -> Self {
        self.address_limit = limit;
        self.address = Quota::new(limit, self.window);
        self
    }

    /// How long a bucket that is completely empty takes to refill.
    ///
    /// Failures are forgiven continuously rather than all at once at the end
    /// of a fixed window: a client that has spent every token gets one back
    /// after a fifth of this, not nothing until the clock rolls over. Fixed
    /// windows also let an attacker take two full allowances back to back by
    /// straddling the boundary.
    ///
    /// ```
    /// use arcature::auth::flows::LoginThrottle;
    /// use std::time::Duration;
    ///
    /// let throttle = LoginThrottle::new()
    ///     .per_identity(1)
    ///     .window(Duration::from_secs(60));
    ///
    /// throttle.record_failure("you@example.com", None);
    /// let refused = throttle.check("you@example.com", None);
    ///
    /// // One failure per minute means the next one is about a minute out.
    /// assert!(refused.retry_after().expect("refused") <= Duration::from_secs(60));
    /// ```
    #[must_use]
    pub fn window(mut self, window: Duration) -> Self {
        self.window = window;
        self.identity = Quota::new(self.identity_limit, window);
        self.address = Quota::new(self.address_limit, window);
        self
    }

    /// Whether an attempt on `email` from `client` may proceed.
    ///
    /// Call this *before* verifying the password. It reads the counters and
    /// writes nothing, so calling it twice is the same as calling it once.
    ///
    /// `client` is the resolved [`ClientIp`](crate::http::ClientIp), or `None`
    /// where the serve path cannot supply one -- over a Unix socket, say. All
    /// attempts with no address share one client bucket, on the principle the
    /// rate limiter already applies to the same situation: a client that
    /// cannot be identified must not thereby be unlimited.
    #[must_use]
    pub fn check(&self, email: &str, client: Option<IpAddr>) -> ThrottleDecision {
        self.check_at(email, client, Instant::now())
    }

    /// Record that an attempt on `email` from `client` was rejected.
    ///
    /// Call this after the credentials come back
    /// [`Rejected`](super::CredentialOutcome::Rejected), whether or not the
    /// address exists -- see the type's documentation for why the unknown
    /// case must be counted too.
    pub fn record_failure(&self, email: &str, client: Option<IpAddr>) {
        self.record_failure_at(email, client, Instant::now());
    }

    /// Record that an attempt on `email` from `client` succeeded.
    ///
    /// Clears the bucket for that address from that client, so somebody who
    /// mistyped their password four times is not still three failures from a
    /// lockout tomorrow.
    ///
    /// It deliberately does **not** clear the client's own bucket. An
    /// attacker running a spray usually holds one real account somewhere in
    /// the target population; if a success there reset the client bucket,
    /// signing into it every fiftieth attempt would buy back the whole
    /// allowance forever.
    pub fn record_success(&self, email: &str, client: Option<IpAddr>) {
        let key = identity_key(email, client);
        if let Ok(mut buckets) = self.buckets.lock() {
            buckets.remove(&key);
        }
    }

    /// How many buckets are currently held.
    ///
    /// Public because it is the only way to observe that the table is bounded
    /// and that [`record_success`](Self::record_success) actually forgets
    /// something -- a test can assert on this, and a comment cannot. It is
    /// also a fair metric to export: it is roughly the number of distinct
    /// accounts and clients that have failed a sign-in recently.
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.buckets.lock().map_or(0, |buckets| buckets.len())
    }

    /// [`check`](Self::check) at a caller-supplied instant, so the tests can
    /// watch a bucket refill without sleeping through fifteen minutes.
    fn check_at(&self, email: &str, client: Option<IpAddr>, now: Instant) -> ThrottleDecision {
        let pairs = [
            (identity_key(email, client), self.identity),
            (address_key(client), self.address),
        ];

        let buckets = match self.buckets.lock() {
            Ok(buckets) => buckets,
            // A poisoned lock means some other thread panicked mid-update.
            // Refusing is the safe reading: the counters cannot be trusted,
            // and the alternative is an unthrottled login form.
            Err(_) => {
                return ThrottleDecision::TooManyAttempts {
                    retry_after: MINIMUM_RETRY_AFTER,
                };
            }
        };

        // Report the longer of the two waits, so a caller that honours it
        // comes back to an answer rather than to the other refusal.
        let mut wait: Option<Duration> = None;
        for (key, quota) in pairs {
            let tokens = buckets
                .get(&key)
                .map_or(quota.capacity, |bucket| bucket.tokens_at(quota, now));
            if tokens < 1.0 {
                let seconds = (1.0 - tokens) / quota.refill_per_sec;
                let this = Duration::from_secs_f64(seconds).max(MINIMUM_RETRY_AFTER);
                wait = Some(wait.map_or(this, |longest: Duration| longest.max(this)));
            }
        }

        match wait {
            Some(retry_after) => ThrottleDecision::TooManyAttempts { retry_after },
            None => ThrottleDecision::Allowed,
        }
    }

    /// [`record_failure`](Self::record_failure) at a caller-supplied instant.
    fn record_failure_at(&self, email: &str, client: Option<IpAddr>, now: Instant) {
        let pairs = [
            (identity_key(email, client), self.identity),
            (address_key(client), self.address),
        ];

        let Ok(mut buckets) = self.buckets.lock() else {
            // Nothing useful to do: `check_at` already refuses on a poisoned
            // lock, so the failure is not going unpunished.
            return;
        };

        if buckets.len() >= SWEEP_AT {
            // Drop every bucket that has refilled to the brim. Those are the
            // ones holding no information -- a missing entry and a full one
            // answer identically.
            buckets
                .retain(|_, bucket| bucket.tokens_at(self.identity, now) < self.identity.capacity);
        }

        for (key, quota) in pairs {
            let bucket = buckets.entry(key).or_insert(Bucket {
                tokens: quota.capacity,
                updated: now,
            });
            // Settle the refill owed since the last write, then spend. Not
            // saturating at zero: a bucket is never taken below empty,
            // because letting it go negative would turn a burst of failures
            // into a lockout far longer than the window.
            let tokens = bucket.tokens_at(quota, now);
            bucket.tokens = (tokens - 1.0).max(0.0);
            bucket.updated = now;
        }
    }
}

impl fmt::Debug for LoginThrottle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The keys are digests and the values are two floats, so there is no
        // secret in the table -- but there is no reader for eight thousand
        // buckets either, and a `Debug` that dumps them is a log line nobody
        // can use.
        formatter
            .debug_struct("LoginThrottle")
            .field("per_identity", &self.identity_limit)
            .field("per_address", &self.address_limit)
            .field("window", &self.window)
            .field("tracked", &self.tracked())
            .finish_non_exhaustive()
    }
}

/// The bucket key for one address tried from one client.
fn identity_key(email: &str, client: Option<IpAddr>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update([IDENTITY_DOMAIN]);
    // Length-prefixed rather than separator-joined: a separator byte can
    // appear in the address half, and `("a@b", 1.2.3.4)` colliding with some
    // other pair would merge two accounts' allowances.
    let normalised = normalise(email);
    hasher.update((normalised.len() as u64).to_be_bytes());
    hasher.update(normalised.as_bytes());
    write_client(&mut hasher, client);
    hasher.finalize().into()
}

/// The bucket key for one client, whatever it is trying.
fn address_key(client: Option<IpAddr>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update([ADDRESS_DOMAIN]);
    write_client(&mut hasher, client);
    hasher.finalize().into()
}

/// Feed a client address to the hasher in a canonical form.
fn write_client(hasher: &mut Sha256, client: Option<IpAddr>) {
    match client {
        // `to_canonical` folds `::ffff:203.0.113.7` onto `203.0.113.7`. One
        // client reaching the application over both spellings -- which is
        // ordinary on a dual-stack listener -- must not get two allowances.
        Some(address) => match address.to_canonical() {
            IpAddr::V4(v4) => {
                hasher.update([4]);
                hasher.update(v4.octets());
            }
            IpAddr::V6(v6) => {
                hasher.update([6]);
                hasher.update(v6.octets());
            }
        },
        None => {
            hasher.update([0]);
            hasher.update(UNATTRIBUTED);
        }
    }
}

/// Fold an address into the form the counters are kept under.
///
/// Trim and lowercase, and nothing cleverer. This is a *bucket key*, not an
/// identity decision: if it disagrees with how the application looks the user
/// up, the cost is that two spellings of one address count separately, which
/// is a slightly looser throttle and not a wrong answer. Doing more here --
/// stripping dots, dropping `+tags` -- would be guessing at a mail provider's
/// rules and would let one person's failures throttle another's account.
///
/// Hashing afterwards is what bounds the key: the form field is attacker-
/// supplied and has no length of its own, so keying the map on the string
/// would let a megabyte of "email" become a megabyte of resident memory, an
/// entry at a time. It also keeps the table from being a list of who has been
/// trying to sign in.
fn normalise(email: &str) -> String {
    email.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_PER_IDENTITY, LoginThrottle};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::time::{Duration, Instant};

    fn client(last: u8) -> Option<IpAddr> {
        Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, last)))
    }

    #[test]
    fn a_fresh_address_is_allowed() {
        let throttle = LoginThrottle::new();
        assert!(throttle.check("you@example.com", client(1)).is_allowed());
        assert_eq!(throttle.tracked(), 0, "a check must not create a bucket");
    }

    #[test]
    fn the_default_allowance_is_spent_exactly_at_the_limit() {
        let throttle = LoginThrottle::new();
        for attempt in 0..DEFAULT_PER_IDENTITY {
            assert!(
                throttle.check("you@example.com", client(1)).is_allowed(),
                "refused after only {attempt} failures"
            );
            throttle.record_failure("you@example.com", client(1));
        }
        assert!(!throttle.check("you@example.com", client(1)).is_allowed());
    }

    /// The reason the identity bucket is keyed on both halves. A bucket keyed
    /// on the address alone would let anybody lock anybody else out.
    #[test]
    fn one_client_cannot_lock_another_client_out_of_an_account() {
        let throttle = LoginThrottle::new().per_identity(2);
        throttle.record_failure("you@example.com", client(1));
        throttle.record_failure("you@example.com", client(1));
        assert!(!throttle.check("you@example.com", client(1)).is_allowed());

        assert!(
            throttle.check("you@example.com", client(2)).is_allowed(),
            "an attacker locked the account holder out of their own account"
        );
    }

    /// The reason there is a second bucket at all. One failure each against a
    /// thousand different addresses never fills an identity bucket.
    #[test]
    fn a_spray_across_many_accounts_is_caught_by_the_client_bucket() {
        let throttle = LoginThrottle::new().per_address(4);
        for account in 0..4 {
            let email = format!("user-{account}@example.com");
            assert!(throttle.check(&email, client(1)).is_allowed());
            throttle.record_failure(&email, client(1));
        }
        assert!(
            !throttle
                .check("never-tried@example.com", client(1))
                .is_allowed(),
            "the client kept going after spending its whole allowance"
        );
        assert!(
            throttle
                .check("never-tried@example.com", client(2))
                .is_allowed(),
            "a different client was caught by the first one's failures"
        );
    }

    /// The property that keeps the throttle from becoming the oracle the
    /// credential checker closes. Two addresses, one registered and one not,
    /// are indistinguishable here because nothing here knows the difference.
    #[test]
    fn an_address_with_no_account_is_throttled_identically() {
        let throttle = LoginThrottle::new().per_identity(3);
        // One instant for the whole test, so that the two refusals can be
        // compared as *values*. Read the clock twice and the two waits differ
        // by however long the comparison took, which says nothing about
        // either address.
        let now = Instant::now();
        for _ in 0..3 {
            throttle.record_failure_at("real@example.com", client(1), now);
            throttle.record_failure_at("nobody@example.com", client(2), now);
        }
        assert_eq!(
            throttle.check_at("real@example.com", client(1), now),
            throttle.check_at("nobody@example.com", client(2), now),
            "the two addresses were refused differently"
        );
    }

    #[test]
    fn success_clears_the_account_but_not_the_client() {
        // Three tokens for the client and two for the account, so that
        // spending the account's whole allowance leaves the client one. With
        // both limits equal there would be nothing to tell apart: the client
        // bucket would refuse everything and the account bucket's state would
        // be unobservable.
        let throttle = LoginThrottle::new().per_identity(2).per_address(3);
        throttle.record_failure("you@example.com", client(1));
        throttle.record_failure("you@example.com", client(1));
        assert!(!throttle.check("you@example.com", client(1)).is_allowed());

        throttle.record_success("you@example.com", client(1));

        // The account is forgiven...
        assert!(
            throttle.check("you@example.com", client(1)).is_allowed(),
            "a successful sign-in did not clear the account's failures"
        );

        // ...but the client's own allowance is not. Two of its three tokens
        // are still spent, so one more failure -- against any address -- ends
        // it. Had the success refunded the client too, there would be two
        // left here and this would pass. That refund is what would let a
        // spray buy itself back with the one account the attacker owns.
        throttle.record_failure("someone@example.com", client(1));
        assert!(
            !throttle.check("anyone@example.com", client(1)).is_allowed(),
            "the client's allowance was refunded by a successful sign-in"
        );
    }

    #[test]
    fn a_refusal_always_reports_a_wait_of_at_least_a_second() {
        let throttle = LoginThrottle::new().per_identity(1);
        throttle.record_failure("you@example.com", client(1));
        let wait = throttle
            .check("you@example.com", client(1))
            .retry_after()
            .expect("refused");
        assert!(wait >= Duration::from_secs(1), "{wait:?}");
    }

    #[test]
    fn the_allowance_comes_back_over_the_window() {
        let throttle = LoginThrottle::new()
            .per_identity(4)
            .window(Duration::from_secs(40));
        let start = Instant::now();

        for _ in 0..4 {
            throttle.record_failure_at("you@example.com", client(1), start);
        }
        assert!(
            !throttle
                .check_at("you@example.com", client(1), start)
                .is_allowed()
        );

        // Four failures per forty seconds is one back every ten.
        assert!(
            !throttle
                .check_at("you@example.com", client(1), start + Duration::from_secs(9))
                .is_allowed()
        );
        assert!(
            throttle
                .check_at(
                    "you@example.com",
                    client(1),
                    start + Duration::from_secs(11)
                )
                .is_allowed(),
            "the bucket did not refill"
        );
    }

    /// A long silence must not bank an unlimited allowance.
    #[test]
    fn the_allowance_does_not_accumulate_past_the_limit() {
        let throttle = LoginThrottle::new()
            .per_identity(2)
            .window(Duration::from_secs(10));
        let start = Instant::now();

        throttle.record_failure_at("you@example.com", client(1), start);
        let later = start + Duration::from_secs(60 * 60);

        throttle.record_failure_at("you@example.com", client(1), later);
        throttle.record_failure_at("you@example.com", client(1), later);
        assert!(
            !throttle
                .check_at("you@example.com", client(1), later)
                .is_allowed(),
            "an hour of quiet bought more than the configured limit"
        );
    }

    #[test]
    fn a_burst_of_failures_does_not_extend_the_lockout() {
        let throttle = LoginThrottle::new()
            .per_identity(2)
            .window(Duration::from_secs(20));
        let start = Instant::now();

        // Twenty failures against a bucket that holds two. If the bucket went
        // negative, the wait would be minutes rather than the ten seconds one
        // token costs.
        for _ in 0..20 {
            throttle.record_failure_at("you@example.com", client(1), start);
        }
        assert!(
            throttle
                .check_at(
                    "you@example.com",
                    client(1),
                    start + Duration::from_secs(11)
                )
                .is_allowed(),
            "the bucket was driven below empty"
        );
    }

    #[test]
    fn the_address_is_normalised_before_it_is_counted() {
        let throttle = LoginThrottle::new().per_identity(1);
        throttle.record_failure("  You@Example.COM ", client(1));
        assert!(
            !throttle.check("you@example.com", client(1)).is_allowed(),
            "a change of case bought a fresh allowance"
        );
    }

    /// A dual-stack listener can hand the same client to the application as
    /// either spelling. Two allowances for one client would be a bypass.
    #[test]
    fn a_v4_mapped_address_shares_the_v4_bucket() {
        let throttle = LoginThrottle::new().per_identity(1);
        let mapped = Some(IpAddr::V6(Ipv4Addr::new(203, 0, 113, 1).to_ipv6_mapped()));

        throttle.record_failure("you@example.com", mapped);
        assert!(!throttle.check("you@example.com", client(1)).is_allowed());
    }

    #[test]
    fn a_v6_client_is_counted_separately_from_a_v4_one() {
        let throttle = LoginThrottle::new().per_identity(1);
        let v6 = Some(IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)));

        throttle.record_failure("you@example.com", v6);
        assert!(throttle.check("you@example.com", client(1)).is_allowed());
    }

    /// No address is not a free pass. All unattributable attempts share one
    /// client bucket, the way the rate limiter treats the same situation.
    #[test]
    fn attempts_with_no_client_address_still_count() {
        let throttle = LoginThrottle::new().per_address(2);
        throttle.record_failure("one@example.com", None);
        throttle.record_failure("two@example.com", None);
        assert!(!throttle.check("three@example.com", None).is_allowed());
        assert!(
            throttle.check("three@example.com", client(1)).is_allowed(),
            "an identified client was caught by unattributed failures"
        );
    }

    #[test]
    fn a_clone_shares_the_counters() {
        let throttle = LoginThrottle::new().per_identity(1);
        let clone = throttle.clone();
        clone.record_failure("you@example.com", client(1));
        assert!(!throttle.check("you@example.com", client(1)).is_allowed());
    }

    #[test]
    fn a_failure_creates_one_bucket_per_dimension() {
        let throttle = LoginThrottle::new();
        throttle.record_failure("you@example.com", client(1));
        assert_eq!(throttle.tracked(), 2);

        // The same pair again reuses both.
        throttle.record_failure("you@example.com", client(1));
        assert_eq!(throttle.tracked(), 2);

        // A second address from the same client adds only the identity one.
        throttle.record_failure("other@example.com", client(1));
        assert_eq!(throttle.tracked(), 3);
    }

    #[test]
    fn debug_does_not_dump_the_bucket_table() {
        let throttle = LoginThrottle::new();
        throttle.record_failure("you@example.com", client(1));
        let rendered = format!("{throttle:?}");
        assert!(rendered.contains("tracked: 2"), "{rendered}");
        assert!(!rendered.contains("Bucket"), "{rendered}");
    }
}
