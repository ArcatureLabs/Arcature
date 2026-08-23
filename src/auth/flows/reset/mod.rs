//! One-time password-reset links, stored as digests.
//!
//! Behind the `auth-reset` feature, which is off by default because it needs
//! a database.
//!
//! # The shape of the flow this supports
//!
//! 1. Someone says they forgot their password. The application calls
//!    [`PasswordResets::issue`] and mails the plaintext to the address on
//!    file. **It answers the same way whether or not the address is known** --
//!    the reason [`CredentialChecker`](super::CredentialChecker) exists
//!    applies just as hard to a reset form, and a "we don't know that address"
//!    message is the same account list by another route.
//! 2. The link comes back. The application calls
//!    [`PasswordResets::consume`], and gets either the subject it was issued
//!    for or `Ok(None)`.
//! 3. On `Some`, the application writes the new password hash. The link is
//!    already spent by then; `consume` deleted it before it returned.
//!
//! # Where the boundary is
//!
//! This module mints and spends a credential. It does not send mail, does not
//! know what a user is, and does not change a password. `subject` is an opaque
//! `&str` -- an email address, a user id, whatever the application's own
//! lookup takes -- and it comes back out of `consume` unchanged.
//!
//! That is deliberate rather than unfinished. A reset store that knew about
//! the user table would have to know which column holds the address, what a
//! disabled account is, and whether an unverified address may be reset to --
//! all application decisions, and all of them wrong in some application.
//!
//! # What this does not invalidate
//!
//! Spending a link does not sign anybody out. Sessions live in the session
//! store, are keyed by session id, and are not indexed by user, so no portable
//! statement deletes "every session belonging to this subject". The mechanism
//! that does hold -- a credential stamp checked when a session is loaded --
//! is a separate piece, and this module deliberately does not pretend to
//! cover it.
//!
//! # Storage
//!
//! One table, `arcature_password_resets`, created by
//! [`PasswordResets::migrate`], holding the public id in the clear and a
//! SHA-256 of the secret. See [`PasswordResets`] for the three properties the
//! schema and the statements exist to hold up.

mod dialect;
mod error;
mod migrate;
mod store;
mod token;

// Needs `test-kit` for the "is a test database configured, and is it safe to
// write to" decision, which lives there and must have exactly one spelling.
#[cfg(all(test, feature = "test-kit"))]
mod tests;

pub use dialect::ResetPool;
pub use error::PasswordResetError;
pub use store::PasswordResets;
pub use token::{IssuedPasswordReset, PlaintextReset, RESET_TOKEN_PREFIX};
