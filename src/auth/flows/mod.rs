//! The pieces a sign-in screen is made of.
//!
//! `arcature::auth` owns the seams -- hash a password, bind a user to a
//! session, authorize an action. This module owns the handful of small,
//! security-critical decisions that sit between those seams and a login
//! form, and that every application otherwise rewrites: the ones where the
//! obvious implementation is wrong in a way nothing tells you about.
//!
//! Behind the `auth-flows` feature, which is off by default.
//!
//! # What lives here, and why it is not left to the application
//!
//! Each item here exists because the naive version leaks something:
//!
//! * [`CredentialChecker`] -- "no such user" and "wrong password" must be one
//!   answer, and must cost the same. An `if let Some(user)` that skips the
//!   Argon2 verification when the address is unknown turns the *response
//!   time* into a working list of who has an account.
//! * [`EmailVerification`] -- a verification link has to be bound to the
//!   address it was mailed to, not just to the account. A link that only names
//!   the account verifies whichever address the account holds when it is
//!   clicked, so registering with your own address, changing it to somebody
//!   else's, and then clicking gets you a verified address you cannot read.
//! * [`LoginThrottle`] -- a form that answers in constant time and gives one
//!   message away is still a free oracle if it will answer ten thousand
//!   times. Counting failures per account is the obvious guard and it misses
//!   the actual attack, which is one guess each against ten thousand
//!   different accounts; counting them per account *only* also hands anybody
//!   a way to lock anybody else out.
//! * [`PasswordConfirmation`] -- asking for the password again before
//!   something irreversible only means anything if it can go stale, and the
//!   obvious implementation cannot. A boolean in the session says a password
//!   was proved at some point and never says when, so one confirmation covers
//!   every irreversible action for as long as the session lives; and a
//!   confirmation that does not name who made it is inherited by whoever holds
//!   the session next.
//! * `PasswordResets` -- a reset link has to be spendable exactly once, and
//!   the obvious implementation spends it twice. Checking "is this token
//!   valid?" and then deleting it is two statements with a gap, and two
//!   requests carrying the same link both pass the check before either
//!   deletes. Behind the `auth-reset` feature, which is off by default because
//!   it needs a database.
//! * `RememberTokens` -- a "stay signed in" cookie is a bearer credential
//!   that lives for weeks, and the obvious implementation cannot tell that one
//!   was copied. Rotating the secret on every use turns a stolen cookie into
//!   something that both stops working and *announces itself*, because
//!   whichever party goes second presents a secret that is already retired.
//!   Behind the `auth-remember` feature, which is off by default because it
//!   needs a database.
//!
//! # What does not live here
//!
//! The user table, the handlers, the routes, and the HTML. Those are the
//! application's. `arc new` writes none of them -- a fresh scaffold has no
//! user table and no sign-in route -- and `arc make:auth <name>` writes all
//! but the last: an account model, three controllers, a route collection and
//! a migration, headless, with the screens left to `arc make:page` and
//! `arc make:view`. This module is the part that has to be right rather than
//! the part that has to be yours.

mod confirm;
mod credentials;
#[cfg(feature = "auth-remember")]
mod remember;
#[cfg(feature = "auth-reset")]
mod reset;
mod throttle;
mod verification;

pub use confirm::{CONFIRMATION_SESSION_KEY, ConfirmationState, PasswordConfirmation};
pub use credentials::{CREDENTIAL_REJECTION, CredentialChecker, CredentialOutcome};
#[cfg(feature = "auth-remember")]
pub use remember::{
    IssuedRememberToken, PlaintextRememberToken, REMEMBER_TOKEN_PREFIX, RememberOutcome,
    RememberPool, RememberTokenError, RememberTokens,
};
#[cfg(feature = "auth-reset")]
pub use reset::{
    IssuedPasswordReset, PasswordResetError, PasswordResets, PlaintextReset, RESET_TOKEN_PREFIX,
    ResetPool,
};
pub use throttle::{LoginThrottle, ThrottleDecision};
pub use verification::{EmailVerification, EmailVerificationError};
