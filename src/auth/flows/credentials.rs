//! Checking a login form without saying which half of it was wrong.
//!
//! The whole module is one property: **a sign-in attempt against an address
//! nobody has registered must be indistinguishable from a sign-in attempt
//! with the wrong password.** Indistinguishable in the response body, in the
//! status code, and -- the half that is usually missed -- in the time taken.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::auth::{PasswordHashError, PasswordHashString, PasswordHasher, verify_password};

/// The plaintext the absent-user hash is built from.
///
/// Its value does not matter and it is not a secret: nothing is ever verified
/// against it on purpose, and a presented password that happened to equal it
/// would still be [`Rejected`](CredentialOutcome::Rejected), because the
/// outcome is gated on the account existing and not on the comparison alone.
/// It exists so that [`CredentialChecker::new`] has something to hash.
const ABSENT_USER_PLAINTEXT: &[u8] = b"arcature/absent-user/not-a-password";

/// The one sentence a refused sign-in is allowed to say.
///
/// Not "unknown email" and not "incorrect password". Either of those is a
/// membership oracle: a form that answers the first tells anybody with a list
/// of addresses which of them have accounts here, which is the reconnaissance
/// step of a credential-stuffing run and, for some applications, a disclosure
/// all by itself.
///
/// ```
/// use arcature::auth::flows::CREDENTIAL_REJECTION;
///
/// // One message, and it names neither the address nor the password.
/// assert!(!CREDENTIAL_REJECTION.to_lowercase().contains("email"));
/// assert!(!CREDENTIAL_REJECTION.to_lowercase().contains("password"));
/// ```
pub const CREDENTIAL_REJECTION: &str = "These credentials do not match our records.";

/// Whether a sign-in attempt's credentials were accepted.
///
/// Two variants and not three. There is deliberately no `NoSuchUser`: the
/// moment a caller can branch on it, some caller will, and the branch will
/// reach a response.
///
/// ```
/// use arcature::auth::flows::CredentialOutcome;
///
/// assert!(CredentialOutcome::Verified.is_verified());
/// assert!(!CredentialOutcome::Rejected.is_verified());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CredentialOutcome {
    /// The address matched an account and the password verified against that
    /// account's stored hash.
    Verified,
    /// The credentials were not accepted. Which half was wrong is not said,
    /// here or anywhere downstream.
    Rejected,
}

impl CredentialOutcome {
    /// Whether the attempt succeeded.
    ///
    /// ```
    /// use arcature::auth::flows::CredentialOutcome;
    ///
    /// assert!(CredentialOutcome::Verified.is_verified());
    /// ```
    #[must_use]
    pub fn is_verified(self) -> bool {
        matches!(self, Self::Verified)
    }
}

/// Verifies a presented password against a stored hash **that may not
/// exist**, in constant work either way.
///
/// # The bug this type exists to prevent
///
/// The obvious login handler is (not run as a doc test: it is a
/// counter-example, and it names an application's own `users` store and form
/// type, neither of which exists in this crate):
///
/// ```ignore
/// // Wrong. Do not copy this.
/// let Some(user) = users.find_by_email(&form.email).await? else {
///     return Err(rejected());
/// };
/// verify_password(&hasher, form.password.expose(), &user.password_hash)?;
/// ```
///
/// It returns the same message on both paths, so it looks correct. It is not.
/// Argon2id at the recommended parameters takes tens of milliseconds and a
/// failed row lookup takes a fraction of one, so the early return answers an
/// order of magnitude faster than the full path. That difference is
/// measurable over the network, it is stable across repeats, and it turns the
/// form into a query interface: *submit an address, time the answer, learn
/// whether the account exists.*
///
/// # What this type does instead
///
/// [`new`](Self::new) hashes a fixed throwaway plaintext **once**, at
/// construction, and keeps the result. [`check`](Self::check) runs a full
/// Argon2id verification on every call: against the account's real hash when
/// there is one, and against that stored throwaway hash when there is not.
/// Both branches do one verification at the same parameters, so both cost the
/// same, and the result is [`Rejected`](CredentialOutcome::Rejected) either
/// way.
///
/// The dummy hash is computed once rather than per attempt on purpose:
/// hashing it inside `check` would make the absent-user branch cost a *hash
/// plus* a verification, which is a timing signal in the other direction.
///
/// # What it does not do
///
/// It does not find the user, rate-limit the attempt, or write the session.
/// It takes the stored hash the caller already looked up and answers one
/// question about it.
///
/// ```
/// use arcature::auth::flows::{CredentialChecker, CredentialOutcome};
/// use arcature::auth::{PasswordConfig, PasswordHasher};
///
/// // Cheap parameters so the doc test is quick. An application passes
/// // `PasswordConfig::recommended()`.
/// let hasher = PasswordHasher::new(PasswordConfig::new(8, 1, 1))?;
/// let stored = hasher.hash(b"correct horse battery staple")?;
/// let checker = CredentialChecker::new(hasher)?;
///
/// assert_eq!(
///     checker.check(Some(&stored), b"correct horse battery staple"),
///     CredentialOutcome::Verified
/// );
///
/// // The wrong password and an address with no account are one answer...
/// assert_eq!(
///     checker.check(Some(&stored), b"hunter2"),
///     CredentialOutcome::Rejected
/// );
/// assert_eq!(checker.check(None, b"hunter2"), CredentialOutcome::Rejected);
///
/// // ...and one amount of work: three checks, three Argon2id verifications.
/// assert_eq!(checker.verifications(), 3);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub struct CredentialChecker {
    /// The hasher every verification runs under. Shared with the application
    /// so the absent-user branch uses the same parameters as the real one --
    /// different parameters would be the timing signal all over again.
    hasher: PasswordHasher,
    /// The hash the absent-user branch verifies against, computed once.
    absent: PasswordHashString,
    /// How many Argon2id verifications [`check`](CredentialChecker::check)
    /// has run. Shared across clones, because a clone is the same checker.
    verifications: Arc<AtomicU64>,
}

impl CredentialChecker {
    /// Build a checker, computing the absent-user hash now.
    ///
    /// This costs one Argon2id hash at the hasher's parameters, so build it
    /// once at startup and keep it in application state rather than
    /// per-request. It is [`Clone`], and clones share the counter.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordHashError`] if the absent-user hash cannot be
    /// computed -- which, since the parameters were already validated when
    /// the [`PasswordHasher`] was built, means the OS random source failed.
    /// Failing here is deliberate: a checker with no dummy hash could only
    /// fall back to skipping the verification, which is the bug.
    ///
    /// ```
    /// use arcature::auth::flows::CredentialChecker;
    /// use arcature::auth::{PasswordConfig, PasswordHasher};
    ///
    /// // Cheap parameters so the doc test is quick.
    /// let hasher = PasswordHasher::new(PasswordConfig::new(8, 1, 1))?;
    /// let checker = CredentialChecker::new(hasher)?;
    ///
    /// // Nothing has been checked yet.
    /// assert_eq!(checker.verifications(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(hasher: PasswordHasher) -> Result<Self, PasswordHashError> {
        let absent = hasher.hash(ABSENT_USER_PLAINTEXT)?;
        Ok(Self {
            hasher,
            absent,
            verifications: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Check `presented` against `stored`, where `None` means "no account
    /// with that address".
    ///
    /// Runs exactly one Argon2id verification whatever `stored` is. See the
    /// type's documentation for why that is the point.
    ///
    /// ```
    /// use arcature::auth::flows::{CredentialChecker, CredentialOutcome};
    /// use arcature::auth::{PasswordConfig, PasswordHasher};
    ///
    /// // Cheap parameters so the doc test is quick.
    /// let hasher = PasswordHasher::new(PasswordConfig::new(8, 1, 1))?;
    /// let checker = CredentialChecker::new(hasher)?;
    ///
    /// // No account: rejected, and hashed anyway.
    /// assert_eq!(checker.check(None, b"anything"), CredentialOutcome::Rejected);
    /// assert_eq!(checker.verifications(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn check(
        &self,
        stored: Option<&PasswordHashString>,
        presented: &[u8],
    ) -> CredentialOutcome {
        // Pick the hash to verify against before doing anything else, so the
        // two branches differ by which pointer is used and by nothing else.
        let (hash, account_exists) = match stored {
            Some(hash) => (hash, true),
            None => (&self.absent, false),
        };

        self.verifications.fetch_add(1, Ordering::Relaxed);

        // Bound to a local *before* the decision below. Written inline as
        // `account_exists && verify_password(..)`, the `&&` would short-
        // circuit and the absent-user branch would skip the hash -- which is
        // exactly the bug this type exists to prevent, reintroduced by an
        // operator.
        let password_matched = verify_password(&self.hasher, presented, hash).is_ok();

        if account_exists && password_matched {
            CredentialOutcome::Verified
        } else {
            CredentialOutcome::Rejected
        }
    }

    /// How many Argon2id verifications [`check`](Self::check) has run since
    /// this checker was built.
    ///
    /// Public because it is the only way to observe the property that makes
    /// this type worth having: the count after N attempts is N, whether or
    /// not any of those addresses existed. A test can assert that; a comment
    /// cannot. It is also a reasonable thing to export as a metric -- it is
    /// the sign-in attempt rate, and the CPU the sign-in path is spending.
    ///
    /// Counted with [`Ordering::Relaxed`]: nothing is synchronised through
    /// it, and a metric that ordered memory would be a cost with no reader.
    ///
    /// ```
    /// use arcature::auth::flows::CredentialChecker;
    /// use arcature::auth::{PasswordConfig, PasswordHasher};
    ///
    /// // Cheap parameters so the doc test is quick.
    /// let hasher = PasswordHasher::new(PasswordConfig::new(8, 1, 1))?;
    /// let checker = CredentialChecker::new(hasher)?;
    ///
    /// let _ = checker.check(None, b"one");
    /// let _ = checker.check(None, b"two");
    /// assert_eq!(checker.verifications(), 2);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn verifications(&self) -> u64 {
        self.verifications.load(Ordering::Relaxed)
    }
}

impl fmt::Debug for CredentialChecker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The absent-user hash is a PHC string over a constant in this file,
        // so it is not a secret -- but printing it invites somebody to
        // compare a stored hash against it, and there is no use for that.
        //
        // The hasher is not printed because `PasswordHasher` is not `Debug`,
        // and giving it one here would mean printing Argon2 parameters that
        // say how expensive a guess is -- of no use to an operator and of
        // some use to somebody sizing an attack.
        formatter
            .debug_struct("CredentialChecker")
            .field("absent", &"<precomputed>")
            .field("verifications", &self.verifications())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{CREDENTIAL_REJECTION, CredentialChecker, CredentialOutcome};
    use crate::auth::{PasswordConfig, PasswordHasher};

    /// Argon2id at the recommended parameters costs tens of milliseconds in
    /// release and far more in a debug build. The property under test is
    /// "the same work on both branches", which does not depend on how much
    /// work that is, so the tests run at the cheapest valid parameters.
    fn hasher() -> PasswordHasher {
        PasswordHasher::new(PasswordConfig::new(8, 1, 1)).expect("valid params")
    }

    fn checker() -> CredentialChecker {
        CredentialChecker::new(hasher()).expect("absent-user hash")
    }

    #[test]
    fn the_right_password_for_a_real_account_verifies() {
        let stored = hasher().hash(b"correct horse").expect("hash");
        assert_eq!(
            checker().check(Some(&stored), b"correct horse"),
            CredentialOutcome::Verified
        );
    }

    #[test]
    fn a_wrong_password_and_an_absent_account_give_the_same_answer() {
        let checker = checker();
        let stored = hasher().hash(b"correct horse").expect("hash");
        assert_eq!(
            checker.check(Some(&stored), b"wrong"),
            checker.check(None, b"wrong"),
            "the two failure paths must be one outcome"
        );
        assert_eq!(checker.check(None, b"wrong"), CredentialOutcome::Rejected);
    }

    /// The point of the type. A count that lagged behind the number of
    /// attempts would mean some branch returned without hashing, which is
    /// the user-enumeration oracle whatever the message says.
    #[test]
    fn the_absent_account_branch_still_runs_the_hash() {
        let checker = checker();
        assert_eq!(checker.verifications(), 0);

        let _ = checker.check(None, b"nobody home");
        assert_eq!(
            checker.verifications(),
            1,
            "an unknown address must still pay for one Argon2id verification"
        );

        let stored = hasher().hash(b"correct horse").expect("hash");
        let _ = checker.check(Some(&stored), b"correct horse");
        assert_eq!(checker.verifications(), 2);
    }

    /// Ten attempts against addresses that do not exist must cost ten
    /// verifications, not zero -- stated as a loop because the one-shot test
    /// above would still pass if only the first absent attempt hashed.
    #[test]
    fn every_absent_account_attempt_pays_the_same_price() {
        let checker = checker();
        for attempt in 0..10 {
            let _ = checker.check(None, format!("guess-{attempt}").as_bytes());
        }
        assert_eq!(checker.verifications(), 10);
    }

    /// A presented password equal to the absent-user plaintext must not be
    /// a way in. The outcome is gated on the account existing, so even a
    /// verification that succeeds against the dummy hash is a rejection.
    #[test]
    fn guessing_the_dummy_plaintext_is_not_a_login() {
        let checker = checker();
        assert_eq!(
            checker.check(None, super::ABSENT_USER_PLAINTEXT),
            CredentialOutcome::Rejected
        );
    }

    #[test]
    fn a_clone_shares_the_counter() {
        let checker = checker();
        let clone = checker.clone();
        let _ = clone.check(None, b"x");
        assert_eq!(checker.verifications(), 1);
    }

    #[test]
    fn the_rejection_message_names_neither_half_of_the_form() {
        let message = CREDENTIAL_REJECTION.to_lowercase();
        assert!(!message.contains("email"), "{CREDENTIAL_REJECTION}");
        assert!(!message.contains("password"), "{CREDENTIAL_REJECTION}");
        assert!(!message.contains("account"), "{CREDENTIAL_REJECTION}");
    }

    #[test]
    fn debug_does_not_print_the_absent_hash() {
        let rendered = format!("{:?}", checker());
        assert!(rendered.contains("<precomputed>"), "{rendered}");
        assert!(!rendered.contains("$argon2id$"), "{rendered}");
    }
}
