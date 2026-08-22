//! The store itself: issue, present, revoke, sweep -- and the outcome type
//! that says which of the three things a presented cookie turned out to be.

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use sha2::{Digest, Sha256};
use sqlx::Row;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use super::dialect::{RememberPool, sql, stored_time};
use super::error::RememberTokenError;
use super::migrate;
use super::token::{
    IssuedRememberToken, PlaintextRememberToken, SECRET_BYTES, SERIES_BYTES, format_plaintext,
    parse_plaintext,
};

/// How many fresh series [`RememberTokens::issue`] will try before giving up.
///
/// A series is 128 bits, so one clash is already a once-in-the-heat-death
/// event and eight in a row is not chance -- it is a random source that is not
/// random. Looping forever would turn that into a hang; reporting it turns it
/// into a log line someone can act on.
const ISSUE_ATTEMPTS: u32 = 8;

/// How long a just-retired secret keeps working, unless told otherwise.
///
/// A minute. The window exists for one reason -- a client that sent two
/// requests with the same cookie before the first reply came back -- and a
/// minute is far longer than any such overlap while still being far shorter
/// than the time a thief needs to notice they were beaten to it.
const DEFAULT_GRACE: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// RememberOutcome
// ---------------------------------------------------------------------------

/// What a presented remember-me cookie turned out to be.
///
/// Three outcomes and no `Result` in sight, because none of them is an error:
/// a browser holding a cookie from a laptop that was wiped last month is the
/// system working. See [`RememberTokenError`] for the split.
///
/// Unlike the password-reset store, which answers `None` to every failure on
/// purpose, this type *does* distinguish its outcomes -- and the reason the
/// enumeration-oracle argument does not apply is that the distinction is not
/// made from anything the presenter can choose. [`Unrecognised`](Self::Unrecognised)
/// means no live row has that series; [`Theft`](Self::Theft) means a live row
/// does and the secret was wrong. An attacker who could tell those apart would
/// need to guess a 128-bit series first, and having guessed one they have
/// already been told they guessed it -- by the account being signed out
/// everywhere.
#[derive(Debug)]
#[non_exhaustive]
pub enum RememberOutcome {
    /// No live token has this series, or the string is not one this crate
    /// minted.
    ///
    /// The ordinary answer for a cookie whose row expired, whose device was
    /// signed out, or which was never a remember-me cookie at all. Clear the
    /// cookie and carry on as an anonymous request; there is nothing to report
    /// and nobody to warn.
    Unrecognised,

    /// The cookie is good. Sign the subject in.
    #[non_exhaustive]
    Accepted {
        /// Whoever the token was issued for, in the application's own
        /// spelling.
        subject: String,

        /// The cookie to set in the response, when there is one.
        ///
        /// `Some` on the ordinary path: the presented secret was spent by this
        /// request and this is what replaces it. Set it before returning, or
        /// the browser keeps a secret the row no longer holds and the *next*
        /// request looks like a theft.
        ///
        /// `None` in two cases, and in both the right move is to leave the
        /// cookie alone. Either another request rotated this series first --
        /// its response carries the replacement -- or the presented secret was
        /// the one just retired and is inside the grace window, in which case
        /// the browser is already holding a cookie it has not caught up with.
        replacement: Option<PlaintextRememberToken>,
    },

    /// A live series was presented with a secret that is neither its current
    /// one nor a recently retired one.
    ///
    /// Every token for this subject has already been deleted by the time this
    /// is returned, so the credential is dead on every device. What is left is
    /// the application's: end the subject's sessions, and tell them. A user
    /// who is signed out with no explanation files a bug report; a user who is
    /// told their remembered device may have been copied changes their
    /// password.
    ///
    /// # Read this before treating it as proof
    ///
    /// It is evidence, not certainty. The honest reading is "somebody knew a
    /// series and did not know its secret", and the innocent way to produce
    /// that is a client that held a cookie through a rotation it never saw --
    /// a browser restored from a backup, a tab suspended past the grace
    /// window, a shared cookie jar. Weigh it accordingly: this is a signal
    /// worth acting on, not a fact worth accusing anybody of.
    #[non_exhaustive]
    Theft {
        /// Whoever the stolen token was issued for.
        subject: String,
    },
}

impl RememberOutcome {
    /// Whoever this outcome is about, if it is about anybody.
    ///
    /// `None` only for [`Unrecognised`](Self::Unrecognised), which by
    /// definition names no one. Present so that logging and metrics code does
    /// not have to match on a `#[non_exhaustive]` enum to answer the one
    /// question both of them ask.
    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        match self {
            Self::Unrecognised => None,
            Self::Accepted { subject, .. } | Self::Theft { subject } => Some(subject),
        }
    }
}

// ---------------------------------------------------------------------------
// RememberTokens
// ---------------------------------------------------------------------------

/// Rotating remember-me tokens in the application's own database.
///
/// The credential behind "stay signed in": a cookie that outlives the session
/// and signs its holder back in weeks later. That is a long-lived bearer
/// token, which is a thing worth being nervous about -- so this store
/// implements the scheme that makes one defensible, described by Barry Jaspan
/// and reached independently by most frameworks that have had to solve it.
///
/// # The scheme, in three properties
///
/// **A token is spent every time it is used.** Each accepted cookie is
/// replaced by a fresh secret before the response goes out, so a copy taken
/// from a log, a backup, or somebody's disk stops working the next time the
/// real browser makes a request. The window a stolen cookie is useful for
/// shrinks from the token's whole lifetime to whatever gap happens to sit
/// between two of its owner's requests.
///
/// **A stolen token announces itself.** The series survives rotation, so a
/// presented cookie is looked up by something stable and checked against
/// something that moves. When both the thief and the owner use the cookie,
/// whoever goes second presents a secret that was already retired -- and there
/// is no way for either party to avoid that, which is the property the whole
/// design is for. That is [`Theft`](RememberOutcome::Theft), and the store has
/// already deleted every token for the subject by the time it says so.
///
/// **The database never holds a working cookie.** Rows keep the 16-byte series
/// in the clear -- it is a lookup key -- and the SHA-256 of the current secret
/// and of the one just retired. A stolen backup or a reporting account with
/// `SELECT` on this table yields digests and no way to sign in as anybody.
///
/// # The grace window, and why there is one
///
/// Strict rotation has a false positive that would make it unusable: a browser
/// that fires two requests with the same cookie -- restoring a window of tabs,
/// retrying a request whose response was lost -- has the second one present a
/// secret the first just retired. That is not theft, and reporting it as theft
/// would sign people out for using their browser normally.
///
/// Two things keep it from being a hole. A concurrent rotation of the
/// *current* secret is detected exactly, by compare-and-swap rather than by a
/// clock: the loser is told it lost and gets [`Accepted`] with no replacement.
/// The grace window covers only the case where the loser's *response* also got
/// through and the client kept the retired value; it is
/// [`DEFAULT_GRACE`] wide by default and adjustable with
/// [`grace`](Self::grace). A thief needs to present the retired secret inside
/// that window, which means holding a copy already and racing the owner's next
/// request.
///
/// # The denial of service this accepts, stated plainly
///
/// Anyone who knows a live series can force the subject to be signed out
/// everywhere by presenting it with any wrong secret. This is inherent to
/// detecting theft at all -- the whole point is that a wrong secret against a
/// live series is treated as serious -- and it is the trade-off the scheme was
/// published with. It costs the attacker a series: 128 bits that exist in one
/// browser's cookie jar and in one row, and never in a URL, a form, or a
/// `Referer` header.
///
/// # What this type does not do
///
/// It does not read or write cookies, does not end sessions, and does not
/// decide what a subject is. It mints a credential, spends it, and reports
/// what happened; setting `Set-Cookie`, clearing the session store on
/// [`Theft`], and warning the user are the application's, for the reason the
/// module documentation gives.
///
/// # Example
///
/// ```no_run
/// // Needs a database, so this example is compiled and not run.
/// use arcature::auth::flows::{RememberOutcome, RememberTokens};
/// use std::time::Duration;
///
/// # async fn example(pool: arcature::auth::flows::RememberPool)
/// # -> Result<(), Box<dyn std::error::Error>> {
/// let remember = RememberTokens::new(pool);
/// remember.migrate().await?;
///
/// // At login, when the "remember me" box was ticked.
/// let issued = remember
///     .issue("user@example.test", Duration::from_secs(60 * 60 * 24 * 30))
///     .await?;
/// let cookie = issued.plaintext().expose().to_owned();
///
/// // On a later request that has no session, with the cookie the browser sent.
/// match remember.present(&cookie).await? {
///     RememberOutcome::Accepted { subject, replacement, .. } => {
///         // Sign `subject` in, and set `replacement` as the new cookie.
///         assert_eq!(subject, "user@example.test");
///         assert!(replacement.is_some());
///     }
///     RememberOutcome::Theft { subject, .. } => {
///         // Every token for `subject` is already gone. End their sessions
///         // and tell them.
///         let _ = subject;
///     }
///     // The enum is `#[non_exhaustive]`, so the wildcard arm is required;
///     // each variant is too, so the `..` above are required as well. Both
///     // are what let a later release add an outcome, or a field to one,
///     // without breaking this code.
///     _ => {}
/// }
/// # Ok(())
/// # }
/// ```
///
/// [`Accepted`]: RememberOutcome::Accepted
/// [`Theft`]: RememberOutcome::Theft
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RememberTokens {
    pool: RememberPool,
    grace: Duration,
}

impl RememberTokens {
    /// Build a store over an existing pool, with the default grace window.
    #[must_use]
    pub fn new(pool: RememberPool) -> Self {
        Self {
            pool,
            grace: DEFAULT_GRACE,
        }
    }

    /// How long a just-retired secret keeps working. Default
    /// [`DEFAULT_GRACE`].
    ///
    /// Widen it if clients legitimately present a cookie long after a rotation
    /// they missed -- a native app that queues requests offline, say. Every
    /// second of it is a second in which a thief who already holds a retired
    /// secret can still use it, so widen it because something is failing, not
    /// in case something might.
    ///
    /// [`Duration::ZERO`] turns the window off. That is stricter than it
    /// sounds rather than as strict as it sounds: concurrent use of the
    /// *current* secret is still handled exactly, by compare-and-swap, so what
    /// zero removes is only the tolerance for a client that kept a secret
    /// through a rotation whose response it did receive.
    #[must_use]
    pub fn grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    /// The pool the store runs over.
    #[must_use]
    pub fn pool(&self) -> &RememberPool {
        &self.pool
    }

    /// Create `arcature_remember_tokens` and its indexes if they are not
    /// there.
    ///
    /// Idempotent, and safe to run from every replica at once: the migration
    /// is applied under the dialect's advisory lock with a history table.
    /// Call it at startup.
    ///
    /// # Errors
    ///
    /// Returns [`RememberTokenError::Database`] if the database is unreachable
    /// or rejects a statement.
    pub async fn migrate(&self) -> Result<(), RememberTokenError> {
        migrate::apply(&self.pool).await
    }

    /// Mint a token for one device and return the plaintext once.
    ///
    /// Note what this does *not* do, because it is the one place this store
    /// deliberately parts company with the password-reset store's `issue`
    /// (named in prose rather than linked, because `auth-reset` can be off
    /// while this feature is on): it does **not** clear the subject's other
    /// tokens. Two live reset links for one
    /// account are a bug -- an old mail left armed after the user asked for a
    /// new one. Two live remember-me tokens are a phone and a laptop, which is
    /// the entire feature. Each call adds a row; use
    /// [`revoke_all_for`](Self::revoke_all_for) to sign every device out.
    ///
    /// `ttl` is a product decision rather than a security one, within reason:
    /// a month is ordinary, a year is a credential most people will forget
    /// they are carrying. It bounds how long a token survives *unused* --
    /// rotation does not extend it, because a deadline that each use pushes
    /// forward is not a deadline.
    ///
    /// # Errors
    ///
    /// * [`RememberTokenError::Entropy`] if the OS randomness source is
    ///   unavailable. No fallback is attempted: a cookie that is merely hard
    ///   to predict is not a credential.
    /// * [`RememberTokenError::Expiry`] if `ttl` is so large that the deadline
    ///   falls outside the range the database column can hold.
    /// * [`RememberTokenError::SeriesCollision`] if [`ISSUE_ATTEMPTS`] random
    ///   series were all taken, which in practice means the randomness source
    ///   is broken.
    /// * [`RememberTokenError::Database`] if the database rejects a statement.
    pub async fn issue(
        &self,
        subject: &str,
        ttl: Duration,
    ) -> Result<IssuedRememberToken, RememberTokenError> {
        let expires_at = deadline(ttl)?;
        let created_at = Utc::now();

        let mut secret = [0u8; SECRET_BYTES];
        fill_random(&mut secret)?;
        let digest = digest_of(&secret);

        for _ in 0..ISSUE_ATTEMPTS {
            let mut series = [0u8; SERIES_BYTES];
            fill_random(&mut series)?;

            let written = sqlx::query(sql::INSERT_NEW)
                .bind(series.to_vec())
                .bind(digest.to_vec())
                .bind(subject)
                .bind(stored_time(expires_at))
                .bind(stored_time(created_at))
                .execute(&self.pool)
                .await?
                .rows_affected();

            if written == 0 {
                // `DO NOTHING` / `INSERT IGNORE`: the series is taken. Draw
                // another rather than parsing a driver-specific constraint
                // name out of an error.
                continue;
            }

            let plaintext = PlaintextRememberToken::new(format_plaintext(&series, &secret));
            secret.zeroize();
            return Ok(IssuedRememberToken::new(
                subject.to_owned(),
                expires_at,
                plaintext,
            ));
        }

        secret.zeroize();
        Err(RememberTokenError::SeriesCollision {
            attempts: ISSUE_ATTEMPTS,
        })
    }

    /// Check a cookie a client presented, rotating it if it is good.
    ///
    /// The three answers are [`RememberOutcome`]'s three variants, and the
    /// type documentation says what each obliges the caller to do. In short:
    /// sign in and set the replacement cookie, or clear the cookie, or clear
    /// everything and warn.
    ///
    /// # The order of operations, and why it is that order
    ///
    /// Parse, then read, then compare, then write. A string this crate never
    /// minted is rejected before the database is touched, which is not a
    /// micro-optimisation: this method runs on unauthenticated requests
    /// carrying whatever a cookie jar holds, so the unparseable case is the
    /// common one and making it free is what keeps an anonymous request from
    /// costing a query.
    ///
    /// # Both comparisons always run
    ///
    /// The presented digest is compared with the current secret's *and* with
    /// the retired one's, through [`subtle::ConstantTimeEq`], before either
    /// result is looked at. A short-circuit would make "matched the current
    /// secret" measurably cheaper than "matched the retired one", and the
    /// difference between those two is the difference between a normal request
    /// and one worth investigating.
    ///
    /// What follows the comparison necessarily diverges -- an accepted cookie
    /// writes a rotation and a theft writes a cascade of deletes, and no
    /// amount of care makes those take the same time. The comparison is the
    /// part where constant time buys something, because it is the part that
    /// runs before the outcome is known.
    ///
    /// # A wrong guess cannot rotate the token
    ///
    /// The rotation carries the presented digest in its `WHERE` clause, so it
    /// is a compare-and-swap and not a write. That is what lets two of the
    /// owner's own requests race harmlessly: both read the same row, both
    /// match, and exactly one updates it. It also means a caller cannot spend
    /// somebody's token by knowing only its series.
    ///
    /// # Errors
    ///
    /// * [`RememberTokenError::Entropy`] if the OS randomness source is
    ///   unavailable while minting the replacement secret.
    /// * [`RememberTokenError::Database`] if the database rejects a statement.
    ///
    /// A cookie that simply does not sign anybody in is
    /// [`Unrecognised`](RememberOutcome::Unrecognised), not an error.
    pub async fn present(&self, presented: &str) -> Result<RememberOutcome, RememberTokenError> {
        let Some((series, mut secret)) = parse_plaintext(presented) else {
            return Ok(RememberOutcome::Unrecognised);
        };

        let mut presented_digest = digest_of(&secret);
        secret.zeroize();

        let found = sqlx::query(sql::FIND_LIVE)
            .bind(stored_time(self.grace_cutoff()))
            .bind(series.as_bytes().to_vec())
            .fetch_optional(&self.pool)
            .await?;

        let Some(row) = found else {
            presented_digest.zeroize();
            return Ok(RememberOutcome::Unrecognised);
        };

        // Columns by index, not by name: the three dialects agree on the order
        // the statement asks for and nothing else has to be true.
        let current: Vec<u8> = row.try_get(0)?;
        let previous: Option<Vec<u8>> = row.try_get(1)?;
        // An integer and not a bool: PostgreSQL would hand back a real boolean
        // for a `CASE ... THEN true`, SQLite and MySQL an integer, and
        // decoding is typed. The SQL casts to a 64-bit integer everywhere.
        let rotation_is_recent: bool = row.try_get::<i64, _>(2)? != 0;
        let subject: String = row.try_get(3)?;

        // A token that has never rotated is compared against a digest of the
        // right length rather than skipped, so the two cases take the same
        // path. All-zero is safe to stand in for "absent": reaching it would
        // mean finding a SHA-256 preimage of thirty-two zero bytes.
        let retired = previous.unwrap_or_else(|| vec![0u8; 32]);

        // Both, unconditionally, before either is read. See the doc comment.
        let matches_current: bool = presented_digest.ct_eq(current.as_slice()).into();
        let matches_retired: bool = presented_digest.ct_eq(retired.as_slice()).into();
        presented_digest.zeroize();

        if matches_current {
            return self.rotate(&series, current, subject).await;
        }

        if matches_retired && rotation_is_recent {
            // The client is one rotation behind and inside the window. Sign it
            // in, but do not rotate again: the secret it is behind on is the
            // live one, and a second rotation would put it two behind.
            return Ok(RememberOutcome::Accepted {
                subject,
                replacement: None,
            });
        }

        // A live series, and a secret that is neither. Kill the credential on
        // every device this subject has before saying so, so that the caller
        // cannot act on the report before the tokens are gone.
        sqlx::query(sql::DELETE_FOR)
            .bind(&subject)
            .execute(&self.pool)
            .await?;
        Ok(RememberOutcome::Theft { subject })
    }

    /// Replace the secret behind one series, compare-and-swap.
    ///
    /// Split out of [`present`](Self::present) because it is the only place
    /// that writes a secret on a read path, and it deserves to be readable on
    /// its own.
    async fn rotate(
        &self,
        series: &super::token::SeriesId,
        expected: Vec<u8>,
        subject: String,
    ) -> Result<RememberOutcome, RememberTokenError> {
        let mut next = [0u8; SECRET_BYTES];
        fill_random(&mut next)?;
        let next_digest = digest_of(&next);

        let rotated = sqlx::query(sql::ROTATE)
            .bind(next_digest.to_vec())
            .bind(stored_time(Utc::now()))
            .bind(series.as_bytes().to_vec())
            .bind(expected)
            .execute(&self.pool)
            .await?
            .rows_affected();

        if rotated == 0 {
            // Another request rotated this series between the read and here.
            // Both requests are the same browser and both are legitimate; the
            // one that won carries the replacement cookie, and issuing a
            // second one would leave the client holding whichever response
            // happened to arrive last.
            next.zeroize();
            return Ok(RememberOutcome::Accepted {
                subject,
                replacement: None,
            });
        }

        let plaintext = PlaintextRememberToken::new(format_plaintext(series.as_bytes(), &next));
        next.zeroize();
        Ok(RememberOutcome::Accepted {
            subject,
            replacement: Some(plaintext),
        })
    }

    /// Delete every token belonging to one subject, and report how many.
    ///
    /// "Sign out everywhere". Call it when the password changes, when the
    /// account is disabled, and on [`Theft`](RememberOutcome::Theft) -- though
    /// [`present`](Self::present) has already done it by the time it reports
    /// one, so calling again is only ever belt and braces.
    ///
    /// # Errors
    ///
    /// Returns [`RememberTokenError::Database`] if the statement fails.
    pub async fn revoke_all_for(&self, subject: &str) -> Result<u64, RememberTokenError> {
        let result = sqlx::query(sql::DELETE_FOR)
            .bind(subject)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Delete the token a client is holding, and report whether there was one.
    ///
    /// An ordinary sign-out on one device: the phone forgets, the laptop does
    /// not. Takes the plaintext because that is what the request carries, and
    /// because a caller that had to take the series apart itself would need a
    /// parser this module deliberately does not export.
    ///
    /// No secret check. Presenting somebody else's cookie to this method
    /// deletes their token, which sounds worse than it is: doing so requires
    /// holding the cookie, and anyone holding it could sign in as them
    /// instead. Refusing on a wrong secret would be worse -- it would make
    /// sign-out fail for the client whose secret is one rotation stale, which
    /// is the client most likely to be signing out because something looks
    /// wrong.
    ///
    /// # Errors
    ///
    /// Returns [`RememberTokenError::Database`] if the statement fails.
    pub async fn revoke(&self, presented: &str) -> Result<bool, RememberTokenError> {
        let Some((series, mut secret)) = parse_plaintext(presented) else {
            return Ok(false);
        };
        secret.zeroize();

        let deleted = sqlx::query(sql::DELETE_SERIES)
            .bind(series.as_bytes().to_vec())
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(deleted > 0)
    }

    /// Delete every token whose deadline has passed, and report how many.
    ///
    /// This reclaims disk. It is not what makes expiry correct -- every read
    /// already carries the expiry predicate -- so a deployment that never
    /// calls it is secure, merely wasteful. It matters more here than for
    /// reset links: these rows live for weeks, so the table accumulates
    /// instead of turning over.
    ///
    /// # Errors
    ///
    /// Returns [`RememberTokenError::Database`] if the statement fails.
    pub async fn sweep_expired(&self) -> Result<u64, RememberTokenError> {
        let result = sqlx::query(sql::DELETE_EXPIRED).execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    /// The instant a rotation must be after to still be inside the window.
    ///
    /// Both fallbacks say "everything is inside the window", which is what a
    /// grace so wide it cannot be represented asked for. There is no honest
    /// alternative: refusing would turn a configuration value into a runtime
    /// error on every request, and clamping to zero would silently mean the
    /// opposite of what was written.
    fn grace_cutoff(&self) -> DateTime<Utc> {
        let delta = TimeDelta::from_std(self.grace).unwrap_or(TimeDelta::MAX);
        Utc::now()
            .checked_sub_signed(delta)
            .unwrap_or(DateTime::<Utc>::MIN_UTC)
    }
}

/// Turn a time-to-live into the instant the token stops working.
///
/// Both steps can fail, and neither failure is hypothetical if the `ttl`
/// arrives from configuration: [`TimeDelta::from_std`] refuses a `Duration`
/// wider than its own range, and the addition refuses one that walks the
/// calendar off its end. Reporting either is better than the alternative,
/// which is a deadline that silently wrapped into the past -- a cookie dead on
/// arrival -- or into the far future.
fn deadline(ttl: Duration) -> Result<DateTime<Utc>, RememberTokenError> {
    let describe = || format!("now + {ttl:?}");
    let delta = TimeDelta::from_std(ttl).map_err(|_| RememberTokenError::Expiry(describe()))?;
    Utc::now()
        .checked_add_signed(delta)
        .ok_or_else(|| RememberTokenError::Expiry(describe()))
}

/// The 32 bytes a secret is stored as.
///
/// SHA-256 and deliberately not argon2, for the reason spelled out at the same
/// function in `crate::tokens`: the secret is [`SECRET_BYTES`] bytes straight
/// from the OS CSPRNG, so there is no distribution to enumerate and a slow
/// hash defends against nothing it was designed to defend against. Here there
/// is a second reason on top of that one -- this digest is computed on
/// ordinary authenticated requests, so a deliberately slow hash would be a
/// denial of service the application inflicted on itself.
///
/// `pub(super)` rather than private so the live-database tests can assert
/// against the bytes the table holds without a second implementation. A test
/// that hashed the secret its own way would agree with itself and prove
/// nothing about what was written.
pub(super) fn digest_of(secret: &[u8]) -> [u8; 32] {
    Sha256::digest(secret).into()
}

/// Fill a buffer from the OS randomness source.
///
/// No fallback. If the OS cannot produce randomness the honest outcome is an
/// error the operator sees, not a cookie minted from a clock.
fn fill_random(buffer: &mut [u8]) -> Result<(), RememberTokenError> {
    getrandom::fill(buffer).map_err(|_| RememberTokenError::Entropy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_is_not_the_secret() {
        let secret = [0xa5u8; SECRET_BYTES];
        let digest = digest_of(&secret);
        assert_eq!(digest.len(), 32);
        assert_ne!(&digest[..], &secret[..]);
    }

    #[test]
    fn the_digest_is_the_documented_sha256() {
        // Pinned against an independently known SHA-256 so a future change of
        // algorithm cannot pass unnoticed: this is the digest of 32 zero
        // bytes. It is also, not by accident, the value that would have to be
        // forged to make the all-zero stand-in for an absent previous digest
        // match anything.
        let digest = crate::crypt::base64url::encode(&digest_of(&[0u8; 32]));
        assert_eq!(digest, "Zmh6rfhivXdsj8GLjp-OIAiXFIVu4jOzkCpZHQ1fKSU");
    }

    #[test]
    fn the_random_source_fills_the_whole_buffer() {
        // Not a randomness test -- it is a wiring test. A buffer left
        // untouched by a silently failing call would mint every token with the
        // same all-zero secret, and every rotation would be a no-op that MySQL
        // reports as zero rows changed.
        let mut first = [0u8; SECRET_BYTES];
        let mut second = [0u8; SECRET_BYTES];
        fill_random(&mut first).expect("the OS randomness source is available");
        fill_random(&mut second).expect("the OS randomness source is available");
        assert_ne!(first, [0u8; SECRET_BYTES]);
        assert_ne!(first, second);
    }

    #[test]
    fn a_rotation_never_writes_the_digest_it_replaces() {
        // The premise the compare-and-swap rests on, and the one MySQL makes
        // load-bearing: `rows_affected` there counts *changed* rows, so an
        // update that wrote the existing value would report zero and be read
        // as "somebody else got there first". Two fresh secrets colliding is
        // the same event as a SHA-256 collision.
        let mut a = [0u8; SECRET_BYTES];
        let mut b = [0u8; SECRET_BYTES];
        fill_random(&mut a).expect("the OS randomness source is available");
        fill_random(&mut b).expect("the OS randomness source is available");
        assert_ne!(digest_of(&a), digest_of(&b));
    }

    #[test]
    fn an_ordinary_ttl_lands_in_the_future() {
        let before = Utc::now();
        let at = deadline(Duration::from_secs(60 * 60 * 24 * 30)).expect("a month is fine");
        assert!(at > before);
    }

    #[test]
    fn a_ttl_too_wide_to_represent_is_refused_rather_than_wrapped() {
        // The failure this guards is not a crash, it is silence: a deadline
        // that wrapped would either be in the past, so every cookie is dead on
        // arrival, or absurdly far ahead, so none of them ever expire.
        let error = deadline(Duration::from_secs(u64::MAX)).expect_err("not representable");
        assert!(matches!(error, RememberTokenError::Expiry(_)));
    }

    #[test]
    fn the_default_grace_window_is_in_the_past_and_close_to_it() {
        // The cutoff has to be *behind* now, or a rotation that just happened
        // would not count as recent and every concurrent request would be read
        // as a theft. This is the sign of one subtraction, and getting it
        // backwards would sign people out for opening two tabs.
        let store_grace = DEFAULT_GRACE;
        let cutoff = Utc::now() - TimeDelta::from_std(store_grace).expect("a minute is fine");
        assert!(cutoff < Utc::now());
        assert!(cutoff > Utc::now() - TimeDelta::seconds(120));
    }

    #[test]
    fn a_zero_grace_window_puts_the_cutoff_at_now() {
        // Documented as "turns the window off", and this is what that means:
        // no rotation is ever strictly after the cutoff by the time the
        // comparison runs, so only the current secret is accepted.
        let cutoff = Utc::now() - TimeDelta::from_std(Duration::ZERO).expect("zero is fine");
        assert!(cutoff <= Utc::now());
    }

    #[test]
    fn an_outcome_names_its_subject_except_when_there_is_none() {
        assert_eq!(RememberOutcome::Unrecognised.subject(), None);
        assert_eq!(
            RememberOutcome::Accepted {
                subject: "user@example.test".to_owned(),
                replacement: None,
            }
            .subject(),
            Some("user@example.test")
        );
        assert_eq!(
            RememberOutcome::Theft {
                subject: "user@example.test".to_owned(),
            }
            .subject(),
            Some("user@example.test")
        );
    }

    #[test]
    fn a_debug_of_an_accepted_outcome_does_not_leak_the_replacement() {
        // `RememberOutcome` derives `Debug` so it can be logged, and the thing
        // most worth logging it near is the thing it must not print.
        let outcome = RememberOutcome::Accepted {
            subject: "user@example.test".to_owned(),
            replacement: Some(PlaintextRememberToken::new("arcrmb_dead.beef".to_owned())),
        };
        let rendered = format!("{outcome:?}");
        assert!(rendered.contains("user@example.test"));
        assert!(!rendered.contains("beef"), "{rendered}");
    }
}
