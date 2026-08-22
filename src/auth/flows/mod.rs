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
//! * `PasswordResets` -- a reset link has to be spendable exactly once, and
//!   the obvious implementation spends it twice. Checking "is this token
//!   valid?" and then deleting it is two statements with a gap, and two
//!   requests carrying the same link both pass the check before either
//!   deletes. Behind the `auth-reset` feature, which is off by default because
//!   it needs a database.
//!
//! # What does not live here
//!
//! The user table, the form, the HTML, the routes, and the redirect targets.
//! Those are the application's, and `arc new` scaffolds them. This module is
//! the part that has to be right rather than the part that has to be yours.

mod credentials;
#[cfg(feature = "auth-reset")]
mod reset;
mod throttle;
mod verification;

pub use credentials::{CREDENTIAL_REJECTION, CredentialChecker, CredentialOutcome};
#[cfg(feature = "auth-reset")]
pub use reset::{
    IssuedPasswordReset, PasswordResetError, PasswordResets, PlaintextReset, RESET_TOKEN_PREFIX,
    ResetPool,
};
pub use throttle::{LoginThrottle, ThrottleDecision};
pub use verification::{EmailVerification, EmailVerificationError};
