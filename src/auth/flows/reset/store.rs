//! The store itself: issue, consume, revoke, sweep.

use std::time::Duration;

use chrono::{TimeDelta, Utc};
use sha2::{Digest, Sha256};
use sqlx::Row;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use super::dialect::{ResetPool, sql, stored_time};
use super::error::PasswordResetError;
use super::migrate;
use super::token::{
    ID_BYTES, IssuedPasswordReset, PlaintextReset, SECRET_BYTES, format_plaintext, parse_plaintext,
};

/// How many fresh ids [`PasswordResets::issue`] will try before giving up.
///
/// An id is 128 bits, so one clash is already a once-in-the-heat-death event
/// and eight in a row is not chance -- it is a random source that is not
/// random. Looping forever would turn that into a hang; reporting it turns it
/// into a log line someone can act on.
const ISSUE_ATTEMPTS: u32 = 8;

/// One-time password-reset links in the application's own database.
///
/// # The three properties, and where each one lives
///
/// **The link is single-use.** [`consume`](Self::consume) deletes the row
/// before it returns the subject, and the delete *is* the check: two requests
/// carrying the same link both pass the digest comparison, then race on the
/// same statement, and exactly one of them sees a row affected. The loser gets
/// `Ok(None)`, the same answer a forged link gets. There is no "used" flag,
/// because a spent row left in the table is a row some future query can forget
/// to filter.
///
/// **The link expires, and the database says so.** Every read carries
/// `expires_at > now()` evaluated server-side, so a link stops working the
/// instant it lapses whether or not [`sweep_expired`](Self::sweep_expired) has
/// ever run. A deployment that never sweeps wastes disk, not security. There
/// is no "never expires": the column is `NOT NULL` and there is no sentinel.
///
/// **The database never holds the link.** The row keeps the 16-byte public id
/// in the clear -- it is a lookup key -- and the SHA-256 of the 32-byte
/// secret. A stolen backup, a read replica, or a reporting account with
/// `SELECT` on this table yields digests and no way to reset anybody's
/// password. This matters more here than it does for an API token: a token
/// grants what it was scoped to, a reset link grants the account.
///
/// # Every failure is the same failure
///
/// [`consume`](Self::consume) returns `Ok(None)` for a malformed string, an
/// unknown id, a wrong secret, an expired link, and a link already spent. The
/// caller cannot tell those apart, and the reason is the same reason
/// [`CredentialChecker`](super::super::CredentialChecker) gives one rejection
/// for both halves of a login: a caller that learns *which* one has learned
/// something about links it does not hold. An error variant per reason would
/// be an enumeration oracle with a type signature.
///
/// # What this type does not do
///
/// It does not send mail, and it does not change a password. It mints a
/// credential and spends it; what the subject string means, how the link
/// reaches its owner, and what happens after `consume` returns a subject are
/// the application's. That boundary is why `subject` is a `&str` and not a
/// user type -- see the module documentation.
///
/// # Example
///
/// ```no_run
/// // Needs a database, so this example is compiled and not run.
/// use arcature::auth::flows::PasswordResets;
/// use std::time::Duration;
///
/// # async fn example(pool: arcature::auth::flows::ResetPool)
/// # -> Result<(), Box<dyn std::error::Error>> {
/// let resets = PasswordResets::new(pool);
/// resets.migrate().await?;
///
/// // Issuing invalidates any link the subject already had.
/// let issued = resets
///     .issue("user@example.test", Duration::from_secs(60 * 60))
///     .await?;
///
/// // The one and only time the link exists. Mail it; do not log it.
/// let link = format!(
///     "https://example.test/reset/{}",
///     issued.plaintext().expose()
/// );
///
/// // Later, when the link comes back.
/// let presented = issued.plaintext().expose().to_owned();
/// let subject = resets.consume(&presented).await?.expect("just issued");
/// assert_eq!(subject, "user@example.test");
///
/// // And it works exactly once.
/// assert!(resets.consume(&presented).await?.is_none());
/// # let _ = link;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PasswordResets {
    pool: ResetPool,
}

impl PasswordResets {
    /// Build a store over an existing pool.
    #[must_use]
    pub fn new(pool: ResetPool) -> Self {
        Self { pool }
    }

    /// The pool the store runs over.
    #[must_use]
    pub fn pool(&self) -> &ResetPool {
        &self.pool
    }

    /// Create `arcature_password_resets` and its indexes if they are not
    /// there.
    ///
    /// Idempotent, and safe to run from every replica at once: the migration
    /// is applied under the dialect's advisory lock with a history table.
    /// Call it at startup. A store whose table is missing fails on the first
    /// reset request instead, which is the same outage discovered by a user
    /// who is already locked out.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordResetError::Database`] if the database is
    /// unreachable or rejects a statement.
    pub async fn migrate(&self) -> Result<(), PasswordResetError> {
        migrate::apply(&self.pool).await
    }

    /// Mint a link for one subject and return the plaintext once.
    ///
    /// Any link the subject already held is deleted first, so requesting a
    /// reset invalidates the previous mail. Two live links for one account
    /// would mean an old message in an old inbox stays armed after the user
    /// asked for a new one, which is the opposite of what asking again means.
    ///
    /// `ttl` should be short -- an hour is generous. The link is a password
    /// change sitting in an inbox, and the inbox is exactly the thing the
    /// reset flow already assumes might be worth attacking.
    ///
    /// # Errors
    ///
    /// * [`PasswordResetError::Entropy`] if the OS randomness source is
    ///   unavailable. No fallback is attempted: a link that is merely hard to
    ///   predict is not a secret.
    /// * [`PasswordResetError::Expiry`] if `ttl` is so large that the deadline
    ///   falls outside the range the database column can hold.
    /// * [`PasswordResetError::IdCollision`] if [`ISSUE_ATTEMPTS`] random ids
    ///   were all taken, which in practice means the randomness source is
    ///   broken.
    /// * [`PasswordResetError::Database`] if the database rejects a statement.
    pub async fn issue(
        &self,
        subject: &str,
        ttl: Duration,
    ) -> Result<IssuedPasswordReset, PasswordResetError> {
        let expires_at = deadline(ttl)?;
        let created_at = Utc::now();

        let mut secret = [0u8; SECRET_BYTES];
        fill_random(&mut secret)?;
        let digest = digest_of(&secret);

        // Clear first, insert second. The other order would leave a window in
        // which the delete removes the link this call just minted.
        sqlx::query(sql::DELETE_FOR)
            .bind(subject)
            .execute(&self.pool)
            .await?;

        for _ in 0..ISSUE_ATTEMPTS {
            let mut id_bytes = [0u8; ID_BYTES];
            fill_random(&mut id_bytes)?;

            let written = sqlx::query(sql::INSERT_NEW)
                .bind(id_bytes.to_vec())
                .bind(digest.to_vec())
                .bind(subject)
                .bind(stored_time(expires_at))
                .bind(stored_time(created_at))
                .execute(&self.pool)
                .await?
                .rows_affected();

            if written == 0 {
                // `DO NOTHING` / `INSERT IGNORE`: the id is taken. Draw
                // another rather than parsing a driver-specific constraint
                // name out of an error.
                continue;
            }

            let plaintext = PlaintextReset::new(format_plaintext(&id_bytes, &secret));
            secret.zeroize();
            return Ok(IssuedPasswordReset::new(
                subject.to_owned(),
                expires_at,
                plaintext,
            ));
        }

        secret.zeroize();
        Err(PasswordResetError::IdCollision {
            attempts: ISSUE_ATTEMPTS,
        })
    }

    /// Spend a link a client presented, returning the subject it was issued
    /// for.
    ///
    /// `Ok(Some(subject))` means: this string is a link this store minted, it
    /// has not expired, it has not been used, and this call is the one that
    /// used it. Act on it. `Ok(None)` means every other outcome, and the
    /// caller is not told which -- see the type documentation.
    ///
    /// # A wrong guess does not spend the link
    ///
    /// The delete runs only after the digest comparison succeeds. The other
    /// order would let anyone who knows a victim's *public* id -- half of a
    /// link seen over someone's shoulder, or in a proxy log -- burn every
    /// reset the victim requests, which is a lockout rather than a break, but
    /// still an attack that costs nothing to run.
    ///
    /// # The comparison is constant-time, and that is the whole point
    ///
    /// The digest of the presented secret is compared with the stored digest
    /// through [`subtle::ConstantTimeEq`], which reads every byte every time.
    /// A `==` would return at the first differing byte, and a few hundred
    /// nanoseconds averaged over enough requests is enough to recover a digest
    /// one byte at a time: thirty-two rounds of two hundred and fifty-six
    /// guesses, instead of a search of 2^256.
    ///
    /// # What is still observable, said plainly
    ///
    /// The digest is computed *before* the query, so an unknown id and a known
    /// id with a wrong secret follow the same path and differ by one
    /// constant-time comparison. What remains is whatever the database leaks
    /// by finding a row versus not finding one -- and an id is 128 random bits
    /// with a 256-bit secret behind it, so learning that some id exists costs
    /// the same search either way and buys nothing.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordResetError::Database`] if the database rejects a
    /// statement. A link that simply does not redeem is `Ok(None)`, not an
    /// error.
    pub async fn consume(&self, presented: &str) -> Result<Option<String>, PasswordResetError> {
        let Some((id, mut secret)) = parse_plaintext(presented) else {
            return Ok(None);
        };

        let mut presented_digest = digest_of(&secret);
        secret.zeroize();

        let found = sqlx::query(sql::FIND_LIVE)
            .bind(id.as_bytes().to_vec())
            .fetch_optional(&self.pool)
            .await?;

        let Some(row) = found else {
            presented_digest.zeroize();
            return Ok(None);
        };

        // Columns by index, not by name: the three dialects agree on the
        // order the statement asks for and nothing else has to be true.
        let stored: Vec<u8> = row.try_get(0)?;
        let matches: bool = presented_digest.ct_eq(stored.as_slice()).into();
        presented_digest.zeroize();
        if !matches {
            return Ok(None);
        }

        let subject: String = row.try_get(1)?;

        // The spend. One statement, so there is no state in which the link is
        // half-used: either this call cleared the subject's rows or another
        // call already had, and `rows_affected` is which.
        let spent = sqlx::query(sql::DELETE_FOR)
            .bind(&subject)
            .execute(&self.pool)
            .await?
            .rows_affected();

        if spent == 0 {
            return Ok(None);
        }
        Ok(Some(subject))
    }

    /// Delete every link belonging to one subject, and report how many.
    ///
    /// Call this when the password changes by any other route, when the
    /// account is disabled, and when a user reports the reset mail was not
    /// theirs. It is also what [`consume`](Self::consume) does on success, so
    /// a completed reset leaves nothing armed.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordResetError::Database`] if the statement fails.
    pub async fn revoke_all_for(&self, subject: &str) -> Result<u64, PasswordResetError> {
        let result = sqlx::query(sql::DELETE_FOR)
            .bind(subject)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Delete every link whose deadline has passed, and report how many.
    ///
    /// This reclaims disk. It is not what makes expiry correct -- every read
    /// already carries the expiry predicate -- so a deployment that never
    /// calls it is secure, merely wasteful.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordResetError::Database`] if the statement fails.
    pub async fn sweep_expired(&self) -> Result<u64, PasswordResetError> {
        let result = sqlx::query(sql::DELETE_EXPIRED).execute(&self.pool).await?;
        Ok(result.rows_affected())
    }
}

/// Turn a time-to-live into the instant the link stops working.
///
/// Both steps can fail, and neither failure is hypothetical if the `ttl`
/// arrives from configuration: [`TimeDelta::from_std`] refuses a `Duration`
/// wider than its own range, and the addition refuses one that walks the
/// calendar off its end. Reporting either is better than the alternative,
/// which is a deadline that silently wrapped into the past -- a link dead on
/// arrival -- or into the far future.
fn deadline(ttl: Duration) -> Result<chrono::DateTime<Utc>, PasswordResetError> {
    let describe = || format!("now + {ttl:?}");
    let delta = TimeDelta::from_std(ttl).map_err(|_| PasswordResetError::Expiry(describe()))?;
    Utc::now()
        .checked_add_signed(delta)
        .ok_or_else(|| PasswordResetError::Expiry(describe()))
}

/// The 32 bytes a link's secret half is stored as.
///
/// SHA-256 and deliberately not argon2, for the reason spelled out at the
/// same function in `crate::tokens`: the secret is [`SECRET_BYTES`] bytes
/// straight from the OS CSPRNG, so there is no distribution to enumerate and a
/// slow hash defends against nothing it was designed to defend against. The
/// password itself is a different matter and is hashed with argon2 by
/// `crate::auth::password`, which is the code path where the input is
/// something a human chose.
fn digest_of(secret: &[u8]) -> [u8; 32] {
    Sha256::digest(secret).into()
}

/// Fill a buffer from the OS randomness source.
///
/// No fallback. If the OS cannot produce randomness the honest outcome is an
/// error the operator sees, not a reset link minted from a clock.
fn fill_random(buffer: &mut [u8]) -> Result<(), PasswordResetError> {
    getrandom::fill(buffer).map_err(|_| PasswordResetError::Entropy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_is_not_the_secret() {
        // The property the schema rests on: what goes into the row is not
        // what the mail carries, and is a fixed 32 bytes whatever the input.
        let secret = [0xa5u8; SECRET_BYTES];
        let digest = digest_of(&secret);
        assert_eq!(digest.len(), 32);
        assert_ne!(&digest[..], &secret[..]);
    }

    #[test]
    fn the_digest_is_the_documented_sha256() {
        // Pinned against an independently known SHA-256 so a future change of
        // algorithm cannot pass unnoticed: this is the digest of 32 zero
        // bytes.
        let digest = crate::crypt::base64url::encode(&digest_of(&[0u8; 32]));
        assert_eq!(digest, "Zmh6rfhivXdsj8GLjp-OIAiXFIVu4jOzkCpZHQ1fKSU");
    }

    #[test]
    fn two_secrets_that_differ_in_one_bit_give_different_digests() {
        let a = [0u8; SECRET_BYTES];
        let mut b = [0u8; SECRET_BYTES];
        b[SECRET_BYTES - 1] = 1;
        assert_ne!(digest_of(&a), digest_of(&b));
        // And the same input always gives the same digest, which is what
        // makes a lookup possible at all.
        assert_eq!(digest_of(&a), digest_of(&[0u8; SECRET_BYTES]));
    }

    #[test]
    fn the_random_source_fills_the_whole_buffer() {
        // Not a randomness test -- it is a wiring test. A buffer left
        // untouched by a silently failing call would mint every link with the
        // same all-zero secret.
        let mut first = [0u8; SECRET_BYTES];
        let mut second = [0u8; SECRET_BYTES];
        fill_random(&mut first).expect("the OS randomness source is available");
        fill_random(&mut second).expect("the OS randomness source is available");
        assert_ne!(first, [0u8; SECRET_BYTES]);
        assert_ne!(first, second);
    }

    #[test]
    fn an_ordinary_ttl_lands_in_the_future() {
        let before = Utc::now();
        let at = deadline(Duration::from_secs(3600)).expect("an hour is representable");
        assert!(at > before);
        assert!(at <= before + TimeDelta::seconds(3601));
    }

    #[test]
    fn a_ttl_too_wide_to_represent_is_refused_rather_than_wrapped() {
        // The failure this guards is not a crash, it is silence: a deadline
        // that wrapped would either be in the past, so every link is dead on
        // arrival, or absurdly far ahead, so none of them ever expire.
        let error = deadline(Duration::from_secs(u64::MAX)).expect_err("not representable");
        assert!(matches!(error, PasswordResetError::Expiry(_)));
    }

    #[test]
    fn a_zero_ttl_is_representable_and_already_expired() {
        // Not an error: the store's job is to write the deadline it was
        // given. `expires_at > now()` in the query is what makes such a link
        // unusable, and it is worth knowing that path is reachable.
        let at = deadline(Duration::ZERO).expect("zero is representable");
        assert!(at <= Utc::now());
    }
}
