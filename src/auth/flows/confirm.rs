//! Asking for the password again before something that cannot be undone.
//!
//! A session is a bearer credential that outlives the moment it was created,
//! and the person holding it is not always the person who signed in. An
//! unlocked laptop, a shared browser, a cookie lifted by an XSS that has since
//! been patched -- all three end in the same place: a valid session in the
//! wrong hands. Password confirmation does not try to work out which of those
//! happened. It asks for something the session does not contain.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::auth::{Session, SessionError};
use crate::crypt::{Clock, SystemClock};

/// The session key a confirmation is stored under by default.
///
/// Published because sign-out belongs to the application, and an application
/// that clears the session selectively rather than calling
/// [`Session::flush`] needs to name this key to clear it. Prefixed, because
/// the session is a namespace shared with whatever else the application puts
/// there.
pub const CONFIRMATION_SESSION_KEY: &str = "arcature.password_confirmed";

/// How long a confirmation stands before it is asked for again.
///
/// Fifteen minutes: long enough that a settings page which changes three
/// things does not ask three times, short enough that walking away from a
/// desk does not leave the window open for the rest of the afternoon. It is
/// deliberately shorter than a session, since a confirmation that outlived
/// the tab it was made in would be measuring nothing.
const DEFAULT_WINDOW: Duration = Duration::from_secs(15 * 60);

/// What is written to the session when a password is confirmed.
///
/// A timestamp and a subject rather than a flag. See
/// [`PasswordConfirmation`] for why each field is here.
#[derive(Serialize, Deserialize)]
struct Confirmation {
    /// Who proved the password.
    subject: String,
    /// When they proved it, in seconds since the Unix epoch, read from the
    /// configured [`Clock`].
    at: u64,
}

/// Whether a sensitive action may go ahead without asking for the password.
///
/// ```
/// use arcature::auth::flows::ConfirmationState;
///
/// assert!(!ConfirmationState::Stale.is_fresh());
/// assert_eq!(ConfirmationState::Stale.remaining(), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfirmationState {
    /// The password was proved recently enough. Go ahead.
    Fresh {
        /// How much of the window is left. Suitable for telling the user when
        /// they will be asked again, and never zero -- a confirmation with
        /// nothing left is [`Stale`](Self::Stale).
        remaining: Duration,
    },
    /// Ask for the password. This is the answer for a session that has never
    /// confirmed, one whose window has run out, one that confirmed as a
    /// different subject, and one whose stored value cannot be read -- see
    /// [`PasswordConfirmation::state`] for why those are one answer.
    Stale,
}

impl ConfirmationState {
    /// Whether the action may proceed.
    ///
    /// ```
    /// use arcature::auth::flows::ConfirmationState;
    /// use std::time::Duration;
    ///
    /// let fresh = ConfirmationState::Fresh {
    ///     remaining: Duration::from_secs(60),
    /// };
    /// assert!(fresh.is_fresh());
    /// assert!(!ConfirmationState::Stale.is_fresh());
    /// ```
    #[must_use]
    pub fn is_fresh(self) -> bool {
        matches!(self, Self::Fresh { .. })
    }

    /// How much of the window is left, if any.
    ///
    /// ```
    /// use arcature::auth::flows::ConfirmationState;
    /// use std::time::Duration;
    ///
    /// let fresh = ConfirmationState::Fresh {
    ///     remaining: Duration::from_secs(60),
    /// };
    /// assert_eq!(fresh.remaining(), Some(Duration::from_secs(60)));
    /// assert_eq!(ConfirmationState::Stale.remaining(), None);
    /// ```
    #[must_use]
    pub fn remaining(self) -> Option<Duration> {
        match self {
            Self::Fresh { remaining } => Some(remaining),
            Self::Stale => None,
        }
    }
}

/// Records that a signed-in user has just re-entered their password, and
/// answers whether that is still recent enough to act on.
///
/// Keep it in application state; it is [`Clone`] and holds no per-request
/// data. Like [`LoginThrottle`](super::LoginThrottle) it is a handle the
/// handler calls rather than a `tower::Layer`, and for a related reason: the
/// guard needs the signed-in subject, which the application resolves, and a
/// layer would have to guess where that lives.
///
/// # The threat this addresses, and the one it does not
///
/// The attacker here holds a valid session and does not know the password.
/// That is the whole model, and two things follow from it that are easy to get
/// backwards:
///
/// * **Account enumeration is not a concern.** Unlike a sign-in form, this
///   endpoint is only reachable by somebody already signed in, so the account
///   is not in question -- only the password. The constant-work dance that
///   [`CredentialChecker`](super::CredentialChecker) performs is not needed;
///   verifying against the signed-in user's stored hash directly is correct.
/// * **Volume very much is.** A confirm endpoint is a password-guessing
///   oracle against a *known* account, which is a better position than a
///   sign-in form offers. Guard it with
///   [`LoginThrottle`](super::LoginThrottle), keyed on the same subject, and
///   count a wrong password here exactly as you would count one at sign-in.
///
/// What it does not address: an attacker who has the password, an attacker who
/// can read the session store, or anything at all about how long the session
/// itself lives. It narrows the window on a *borrowed* session and nothing
/// else.
///
/// # Three mistakes this type is shaped to prevent
///
/// * **A flag instead of a timestamp.** `session.put("confirmed", true)` never
///   expires, so one confirmation on Monday covers every irreversible action
///   until the session does. What is stored here is when, not whether.
/// * **A confirmation not bound to who made it.** A session that is reused
///   across a sign-out and a sign-in -- because sign-out regenerated the id
///   but did not clear the data -- would otherwise carry the previous user's
///   confirmation to the next one. Every read names the subject it expects,
///   and a mismatch is [`Stale`](ConfirmationState::Stale).
/// * **A window extended by activity.** Reading the state never rewrites the
///   timestamp, so the deadline is measured from the confirmation and cannot
///   be slid forward by using the account. A "sensitive window" that renews
///   itself on every sensitive action is one that never closes.
///
/// # Using it
///
/// ```
/// use arcature::auth::Session;
/// use arcature::auth::flows::PasswordConfirmation;
/// use arcature::axum::extract::State;
/// use arcature::axum::http::StatusCode;
///
/// // The guard, on every route that must not run on a session alone.
/// async fn delete_account(
///     State(confirmation): State<PasswordConfirmation>,
///     session: Session,
///     // However the application names its signed-in user. This must come
///     // from the session, not from the request body: a subject the caller
///     // supplies is a subject the caller can set to whatever it last saw
///     // confirmed.
///     subject: String,
/// ) -> StatusCode {
///     if !confirmation.is_fresh(&session, &subject).await {
///         // Send them to the confirm form, and back here afterwards.
///         return StatusCode::FORBIDDEN;
///     }
///     StatusCode::NO_CONTENT
/// }
///
/// // The confirm form's own handler, after it has verified the submitted
/// // password against the signed-in user's stored hash.
/// async fn confirm(
///     State(confirmation): State<PasswordConfirmation>,
///     session: Session,
///     subject: String,
/// ) -> StatusCode {
///     match confirmation.record_verified(&session, &subject).await {
///         Ok(()) => StatusCode::NO_CONTENT,
///         Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
///     }
/// }
///
/// let _ = (delete_account, confirm);
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub struct PasswordConfirmation {
    /// How long a confirmation stands.
    window: Duration,
    /// The session key it is stored under.
    key: Arc<str>,
    /// Where "now" comes from. Shared across clones, because a clone is the
    /// same policy.
    clock: Arc<dyn Clock>,
}

impl Default for PasswordConfirmation {
    fn default() -> Self {
        Self::new()
    }
}

impl PasswordConfirmation {
    /// A fifteen-minute window on the wall clock, stored under
    /// [`CONFIRMATION_SESSION_KEY`].
    ///
    /// ```
    /// use arcature::auth::flows::PasswordConfirmation;
    ///
    /// let confirmation = PasswordConfirmation::new();
    /// assert!(format!("{confirmation:?}").contains("900s"));
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            window: DEFAULT_WINDOW,
            key: Arc::from(CONFIRMATION_SESSION_KEY),
            clock: Arc::new(SystemClock::new()),
        }
    }

    /// How long a confirmation stands before it is asked for again.
    ///
    /// [`Duration::ZERO`] means every sensitive action asks, which is a
    /// coherent setting rather than an accident: the window is compared
    /// exclusively, so a confirmation with nothing left is already stale.
    ///
    /// ```
    /// use arcature::auth::flows::PasswordConfirmation;
    /// use std::time::Duration;
    ///
    /// let confirmation = PasswordConfirmation::new().window(Duration::from_secs(300));
    /// assert!(format!("{confirmation:?}").contains("300s"));
    /// ```
    #[must_use]
    pub fn window(mut self, window: Duration) -> Self {
        self.window = window;
        self
    }

    /// The session key to store the confirmation under.
    ///
    /// Worth changing only to avoid a collision with a key the application
    /// already uses, or to run two independent confirmations -- one for
    /// billing and one for account deletion, say, so that confirming for the
    /// first does not silently authorise the second.
    ///
    /// ```
    /// use arcature::auth::flows::PasswordConfirmation;
    ///
    /// let confirmation = PasswordConfirmation::new().session_key("billing.confirmed");
    /// assert!(format!("{confirmation:?}").contains("billing.confirmed"));
    /// ```
    #[must_use]
    pub fn session_key(mut self, key: impl Into<Arc<str>>) -> Self {
        self.key = key.into();
        self
    }

    /// Where "now" comes from.
    ///
    /// The same [`Clock`] the signed-URL machinery uses, and injected for the
    /// same reason: a test for "the confirmation expires after fifteen
    /// minutes" written against the wall clock either sleeps for fifteen
    /// minutes or proves nothing.
    ///
    /// ```
    /// use arcature::auth::flows::PasswordConfirmation;
    /// use arcature::crypt::Clock;
    ///
    /// struct Frozen(u64);
    /// impl Clock for Frozen {
    ///     fn now_unix(&self) -> u64 {
    ///         self.0
    ///     }
    /// }
    ///
    /// let confirmation: PasswordConfirmation =
    ///     PasswordConfirmation::new().clock(Frozen(1_700_000_000));
    /// assert!(format!("{confirmation:?}").contains("900s"));
    /// ```
    #[must_use]
    pub fn clock(mut self, clock: impl Clock) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// Record that `subject` has just proved their password.
    ///
    /// The name carries the precondition: call this **after** verifying the
    /// submitted password against the signed-in user's stored hash, never
    /// before. Nothing here checks a password, and nothing here can.
    ///
    /// `subject` must be the identity the application already holds for this
    /// session -- the same string later passed to [`state`](Self::state).
    ///
    /// Calling it again overwrites, which restarts the window. That is what
    /// makes a re-prompt after expiry work.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the session write fails. Treat that as a
    /// failed confirmation and do not perform the sensitive action: the
    /// alternative is an action that goes ahead on a confirmation nobody will
    /// be able to read back.
    pub async fn record_verified(
        &self,
        session: &Session,
        subject: &str,
    ) -> Result<(), SessionError> {
        session
            .put(
                &self.key,
                Confirmation {
                    subject: subject.to_owned(),
                    at: self.clock.now_unix(),
                },
            )
            .await
    }

    /// Whether `subject`'s confirmation still stands, and for how much longer.
    ///
    /// `subject` must come from the session, not from the request. A subject
    /// the caller supplies is a subject the caller can set to whatever it last
    /// saw confirmed, which turns the binding into decoration.
    ///
    /// Reads only. Calling it ten times is the same as calling it once, and in
    /// particular does not push the deadline out.
    ///
    /// # Why this returns no error
    ///
    /// Four situations reach this method and all four have the same correct
    /// response, which is to ask for the password: never confirmed, expired,
    /// confirmed by somebody else, and a stored value that will not
    /// deserialize -- which is what an older shape of this record looks like
    /// after a deploy. A `Result` would offer a fifth branch, and every
    /// plausible thing to do in it is worse than re-prompting. Failing closed
    /// here also means a session store that has gone away denies sensitive
    /// actions rather than waving them through.
    pub async fn state(&self, session: &Session, subject: &str) -> ConfirmationState {
        let stored = session.get::<Confirmation>(&self.key).await.ok().flatten();
        self.state_of(stored.as_ref(), subject, self.clock.now_unix())
    }

    /// [`state`](Self::state) reduced to the question a route guard asks.
    pub async fn is_fresh(&self, session: &Session, subject: &str) -> bool {
        self.state(session, subject).await.is_fresh()
    }

    /// Discard any confirmation held in this session.
    ///
    /// Call it after anything that should cost a fresh password: a password
    /// change, an elevated action completing, a sign-out that clears the
    /// session selectively rather than with [`Session::flush`].
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the session write fails. A confirmation
    /// that could not be discarded is one that still stands, so a caller that
    /// is revoking access should treat this as a failure rather than ignore
    /// it.
    pub async fn forget(&self, session: &Session) -> Result<(), SessionError> {
        // Removed as an untyped value rather than as a `Confirmation`: a
        // record left by an older shape of this type must still be removable,
        // and asking for it back as `Confirmation` would fail to deserialize
        // it and leave it in place.
        session.forget::<serde_json::Value>(&self.key).await?;
        Ok(())
    }

    /// The whole decision, with the session read and the clock already done.
    ///
    /// Split out so the tests can state the four ways a confirmation fails as
    /// four values rather than as four session fixtures.
    fn state_of(
        &self,
        stored: Option<&Confirmation>,
        subject: &str,
        now: u64,
    ) -> ConfirmationState {
        let Some(stored) = stored else {
            return ConfirmationState::Stale;
        };

        // A plain comparison, not a constant-time one. The subject is not a
        // secret: it is this session's own signed-in identity, which anybody
        // in a position to reach this code already holds.
        if stored.subject != subject {
            return ConfirmationState::Stale;
        }

        let Some(elapsed) = now.checked_sub(stored.at) else {
            // The confirmation is stamped in the future, so either the clock
            // stepped backwards or the record was written by something else.
            // Neither is a confirmation this method can measure, and asking
            // for the password again is the cost of saying so.
            return ConfirmationState::Stale;
        };

        match self.window.checked_sub(Duration::from_secs(elapsed)) {
            // Exclusive at the far end: a confirmation with exactly nothing
            // left has expired. That is also what makes a zero window mean
            // "ask every time" rather than "never expire".
            Some(remaining) if !remaining.is_zero() => ConfirmationState::Fresh { remaining },
            _ => ConfirmationState::Stale,
        }
    }
}

impl fmt::Debug for PasswordConfirmation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The clock is left out: it is a trait object with no useful rendering
        // and nothing a reader of a log line would act on.
        formatter
            .debug_struct("PasswordConfirmation")
            .field("window", &self.window)
            .field("session_key", &self.key)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{CONFIRMATION_SESSION_KEY, ConfirmationState, PasswordConfirmation};
    use crate::auth::Session;
    use crate::crypt::Clock;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;
    use tower_sessions::Session as TowerSession;
    use tower_sessions_memory_store::MemoryStore;

    /// A clock the test drives by hand, so "fifteen minutes later" costs
    /// nothing.
    #[derive(Clone)]
    struct Frozen(Arc<AtomicU64>);

    impl Frozen {
        fn at(seconds: u64) -> Self {
            Self(Arc::new(AtomicU64::new(seconds)))
        }

        fn advance(&self, seconds: u64) {
            self.0.fetch_add(seconds, Ordering::SeqCst);
        }

        fn set(&self, seconds: u64) {
            self.0.store(seconds, Ordering::SeqCst);
        }
    }

    impl Clock for Frozen {
        fn now_unix(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// A session backed by a store of its own, so tests cannot see each
    /// other's writes.
    fn session() -> Session {
        Session(TowerSession::new(
            None,
            Arc::new(MemoryStore::default()),
            None,
        ))
    }

    /// The handle under test, plus the clock driving it.
    fn confirmation(window_secs: u64) -> (PasswordConfirmation, Frozen) {
        let clock = Frozen::at(1_700_000_000);
        let handle = PasswordConfirmation::new()
            .window(Duration::from_secs(window_secs))
            .clock(clock.clone());
        (handle, clock)
    }

    #[tokio::test]
    async fn a_session_that_has_never_confirmed_is_stale() {
        let (handle, _clock) = confirmation(900);
        let session = session();
        assert_eq!(
            handle.state(&session, "user-1").await,
            ConfirmationState::Stale
        );
        assert!(!handle.is_fresh(&session, "user-1").await);
    }

    #[tokio::test]
    async fn a_recorded_confirmation_is_fresh_for_the_whole_window() {
        let (handle, clock) = confirmation(900);
        let session = session();
        handle
            .record_verified(&session, "user-1")
            .await
            .expect("record");

        assert_eq!(
            handle.state(&session, "user-1").await,
            ConfirmationState::Fresh {
                remaining: Duration::from_secs(900)
            }
        );

        clock.advance(600);
        assert_eq!(
            handle.state(&session, "user-1").await,
            ConfirmationState::Fresh {
                remaining: Duration::from_secs(300)
            },
            "the remaining time did not count down"
        );
    }

    /// The boundary is exclusive, and which side it falls on is the difference
    /// between a window that is honoured and one that is a second long.
    #[tokio::test]
    async fn a_confirmation_expires_exactly_at_the_window() {
        let (handle, clock) = confirmation(900);
        let session = session();
        handle
            .record_verified(&session, "user-1")
            .await
            .expect("record");

        clock.advance(899);
        assert!(
            handle.is_fresh(&session, "user-1").await,
            "expired a second early"
        );

        clock.advance(1);
        assert!(
            !handle.is_fresh(&session, "user-1").await,
            "still standing at the end of the window"
        );
    }

    /// The reason the subject is stored at all. A sign-out that regenerates
    /// the session id without clearing the data would otherwise hand the next
    /// user the previous one's confirmation.
    #[tokio::test]
    async fn a_confirmation_does_not_carry_over_to_another_subject() {
        let (handle, _clock) = confirmation(900);
        let session = session();
        handle
            .record_verified(&session, "user-1")
            .await
            .expect("record");

        assert!(handle.is_fresh(&session, "user-1").await);
        assert!(
            !handle.is_fresh(&session, "user-2").await,
            "one user's confirmation authorised another user's action"
        );
    }

    /// A sensitive window that renews itself every time it is consulted is one
    /// that never closes.
    #[tokio::test]
    async fn reading_the_state_does_not_extend_the_window() {
        let (handle, clock) = confirmation(900);
        let session = session();
        handle
            .record_verified(&session, "user-1")
            .await
            .expect("record");

        // Busy account: consulted every hundred seconds for the whole window.
        for _ in 0..9 {
            assert!(handle.is_fresh(&session, "user-1").await);
            clock.advance(100);
        }

        assert!(
            !handle.is_fresh(&session, "user-1").await,
            "the deadline was pushed out by reading it"
        );
    }

    #[tokio::test]
    async fn confirming_again_restarts_the_window() {
        let (handle, clock) = confirmation(900);
        let session = session();
        handle
            .record_verified(&session, "user-1")
            .await
            .expect("record");

        clock.advance(1000);
        assert!(!handle.is_fresh(&session, "user-1").await);

        handle
            .record_verified(&session, "user-1")
            .await
            .expect("re-record");
        assert_eq!(
            handle.state(&session, "user-1").await,
            ConfirmationState::Fresh {
                remaining: Duration::from_secs(900)
            }
        );
    }

    #[tokio::test]
    async fn a_zero_window_asks_every_time() {
        let (handle, _clock) = confirmation(0);
        let session = session();
        handle
            .record_verified(&session, "user-1")
            .await
            .expect("record");

        assert!(
            !handle.is_fresh(&session, "user-1").await,
            "a zero window read as `never expires`"
        );
    }

    /// A record stamped in the future cannot be measured, and guessing which
    /// direction to guess in is how a clock skew becomes an open window.
    #[tokio::test]
    async fn a_clock_that_has_gone_backwards_is_stale() {
        let (handle, clock) = confirmation(900);
        let session = session();
        handle
            .record_verified(&session, "user-1")
            .await
            .expect("record");
        assert!(handle.is_fresh(&session, "user-1").await);

        clock.set(1_600_000_000);
        assert!(
            !handle.is_fresh(&session, "user-1").await,
            "a backwards clock left the confirmation standing"
        );
    }

    #[tokio::test]
    async fn forgetting_a_confirmation_makes_it_stale() {
        let (handle, _clock) = confirmation(900);
        let session = session();
        handle
            .record_verified(&session, "user-1")
            .await
            .expect("record");
        assert!(handle.is_fresh(&session, "user-1").await);

        handle.forget(&session).await.expect("forget");
        assert!(!handle.is_fresh(&session, "user-1").await);
    }

    /// What a deploy that changed the stored shape looks like from here. It
    /// must read as "ask again", not as an error and not as a confirmation.
    #[tokio::test]
    async fn a_stored_value_that_is_not_a_confirmation_is_stale() {
        let (handle, _clock) = confirmation(900);
        let session = session();
        session
            .put(CONFIRMATION_SESSION_KEY, "true")
            .await
            .expect("seed");

        assert_eq!(
            handle.state(&session, "user-1").await,
            ConfirmationState::Stale
        );

        // And it can still be cleared, which is why `forget` does not ask for
        // the value back as a `Confirmation`.
        handle.forget(&session).await.expect("forget");
        handle
            .record_verified(&session, "user-1")
            .await
            .expect("record");
        assert!(handle.is_fresh(&session, "user-1").await);
    }

    /// The published constant has to be the key actually used, or an
    /// application that clears the session selectively clears the wrong thing.
    #[tokio::test]
    async fn the_default_key_is_the_published_constant() {
        let (handle, _clock) = confirmation(900);
        let session = session();
        handle
            .record_verified(&session, "user-1")
            .await
            .expect("record");

        let raw: Option<serde_json::Value> =
            session.get(CONFIRMATION_SESSION_KEY).await.expect("read");
        assert!(raw.is_some(), "nothing was stored under the published key");
    }

    #[tokio::test]
    async fn a_custom_key_is_independent_of_the_default_one() {
        let (default, clock) = confirmation(900);
        let billing = PasswordConfirmation::new()
            .window(Duration::from_secs(900))
            .session_key("billing.confirmed")
            .clock(clock);
        let session = session();

        default
            .record_verified(&session, "user-1")
            .await
            .expect("record");

        assert!(default.is_fresh(&session, "user-1").await);
        assert!(
            !billing.is_fresh(&session, "user-1").await,
            "confirming for one purpose authorised another"
        );
    }

    #[test]
    fn debug_names_the_window_and_the_key() {
        let rendered = format!("{:?}", PasswordConfirmation::new());
        assert!(rendered.contains("900s"), "{rendered}");
        assert!(rendered.contains(CONFIRMATION_SESSION_KEY), "{rendered}");
    }
}
