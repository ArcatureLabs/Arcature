//! Rotating remember-me tokens, with theft detection.
//!
//! Behind the `auth-remember` feature, which is off by default because it
//! needs a database and because "stay signed in" is a product decision rather
//! than a default anybody should inherit.
//!
//! # The shape of the flow this supports
//!
//! 1. Someone signs in and ticks the box. The application calls
//!    [`RememberTokens::issue`] and sets the plaintext as a cookie -- its own
//!    cookie, separate from the session, because it has to outlive one.
//! 2. Later, a request arrives with no session but with that cookie. The
//!    application calls [`RememberTokens::present`] and acts on the
//!    [`RememberOutcome`]: sign in and set the replacement cookie, or clear
//!    the cookie, or clear everything and warn.
//! 3. On sign-out, the application calls [`RememberTokens::revoke`] for this
//!    device, or [`RememberTokens::revoke_all_for`] for all of them.
//!
//! # Why the token rotates
//!
//! A remember-me cookie is a bearer credential that lives for weeks, which is
//! exactly the thing worth stealing. Rotating it on every use does two things
//! that a static one cannot: a copy stops working as soon as the real browser
//! makes a request, and a copy that *is* used announces itself, because
//! whichever party goes second presents a secret that has already been
//! retired. [`RememberTokens`] documents the scheme in full, including the
//! denial of service it accepts in exchange and the grace window that keeps a
//! browser opening twenty tabs from looking like a thief.
//!
//! # Where the boundary is
//!
//! This module mints a credential, spends it, and says what happened. It does
//! not read or write cookies, does not end sessions, does not send mail, and
//! does not know what a user is. `subject` is an opaque `&str` -- an email
//! address, a user id, whatever the application's own lookup takes -- and it
//! comes back out of [`present`](RememberTokens::present) unchanged.
//!
//! The boundary matters most at [`RememberOutcome::Theft`]. The tokens are
//! already gone by the time it is returned, because that is this module's
//! half; ending the subject's sessions and telling them are the application's,
//! for the same reason the password-reset module gives -- sessions are keyed
//! by session id and are not indexed by user, so no portable statement here
//! could delete them.
//!
//! # Storage
//!
//! One table, `arcature_remember_tokens`, created by
//! [`RememberTokens::migrate`], holding the 16-byte series in the clear -- it
//! is a lookup key -- and a SHA-256 of the current secret and of the one just
//! retired. A stolen backup of this table signs nobody in.

mod dialect;
mod error;
mod migrate;
mod store;
mod token;

// Needs `test-kit` for the "is a test database configured, and is it safe to
// write to" decision, which lives there and must have exactly one spelling.
#[cfg(all(test, feature = "test-kit"))]
mod tests;

pub use dialect::RememberPool;
pub use error::RememberTokenError;
pub use store::{RememberOutcome, RememberTokens};
pub use token::{IssuedRememberToken, PlaintextRememberToken, REMEMBER_TOKEN_PREFIX};
