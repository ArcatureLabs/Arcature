//! Authentication, sessions, authorization policies, password hashing, and
//! CSRF protection.
//!
//! This module owns the **integration seams** an Arcature application needs to
//! authenticate users safely: Argon2id password hashing, tower-sessions Axum
//! session middleware, double-submit CSRF protection, the `Auth<U>` /
//! `OptionalAuth<U>` / `AuthManager<U>` extractors, the `Session` and `Flash`
//! ergonomics, and the `Policy` authorization seam.
//!
//! # What this module owns
//!
//! * **Argon2id password hashing** with audited salt generation, PHC-formatted
//!   stored hashes, parameter configuration, verification, and rehash-on-
//!   parameter-change detection ([`PasswordHasher`], [`verify_password`]).
//! * **Secure sessions** over tower-sessions: cookie attributes (name,
//!   `SameSite`, `Secure`, `HttpOnly`, path, domain, `Max-Age`, expiry) and a
//!   signed cookie jar, built into a [`tower_sessions::SessionManagerLayer`]
//!   from a resolved [`SessionConfig`].
//! * **CSRF protection** for cookie-authenticated browser requests via a
//!   double-submit token ([`CsrfLayer`], [`CsrfToken`]). Bearer-token APIs and
//!   safe-method requests are exempt by design.
//! * **Auth extractors** ([`Auth`], [`OptionalAuth`], [`AuthManager`]) that
//!   load the authenticated user from the session + application state.
//! * **Session/Flash ergonomics** ([`Session`], [`Flash`]).
//! * **Authorization** via the [`Policy`] trait and [`Auth::authorize`].
//!
//! # Where each of those lives
//!
//! Every name above is re-exported from `arcature::auth`, so `use
//! arcature::auth::Auth` is the path to write and the submodule is an
//! implementation detail. When you do need the submodule: the extractors are
//! in [`extract`], the handler-facing session API in [`session_api`], the
//! one-time messages in [`flash`], the authorization seam in [`policy`], the
//! cookie/middleware configuration in [`session`], and password hashing in
//! [`password`]. The [`dx`] module is the pre-`0.1.1` spelling of the first
//! four and is deprecated.
//!
//! # What this module does not own
//!
//! It does not own the User model, role/permission/account tables, or any
//! application-specific identity schema -- applications own domain identity.
//! It does not reimplement cryptography (Argon2id, HMAC, SHA-2, and TLS come
//! from RustCrypto, `cookie`, and the certified rustls + aws-lc-rs path). It
//! does not persist sessions to a specific store by default; the application
//! wires any [`tower_sessions::SessionStore`].
//!
//! # Security note -- secrets are never logged
//!
//! Passwords, session signing keys, and tokens are wrapped in
//! [`secrecy`]-backed types whose `Debug`/`Display` never expose the secret
//! and which zeroize on drop. No plaintext password, signing key, or token
//! appears in `Debug`, `Display`, error output, or logs.

pub mod csrf;
/// Deprecated compatibility re-exports; see the module docs for the new homes.
#[deprecated(
    since = "0.1.1",
    note = "split into arcature::auth::{extract, session_api, flash, policy}"
)]
pub mod dx;
pub mod error;
pub mod extract;
pub mod flash;
pub mod password;
pub mod password_config;
pub mod policy;
pub mod session;
pub mod session_api;

// Re-export the certified tower-sessions crate so downstream code targets the
// Arcature-pinned version and reaches the certified `cookie` crate through
// `tower_sessions::cookie`.
pub use tower_sessions;

// Re-export the certified argon2 crate.
pub use argon2;

pub use csrf::{CsrfConfig, CsrfLayer, CsrfMiddleware, CsrfToken};
pub use error::{
    CsrfConfigError, CsrfError, PasswordHashError, PasswordVerifyError, SessionBuildError,
    SessionConfigError, SigningKeyReason,
};
pub use extract::{
    Auth, AuthError, AuthManager, Current, LoginBuilder, OptionalAuth, OptionalCurrent, UserLoader,
};
pub use flash::{Flash, FlashError, FlashLevel, FlashMessage};
pub use password::{
    PasswordHashString, PasswordHasher, PasswordSecret, RehashOutcome, verify_password,
};
pub use password_config::PasswordConfig;
pub use policy::{AuthzError, Policy};
pub use session::{SameSite, SessionConfig, SessionKey, SessionLayer};
pub use session_api::{Session, SessionError};

// The redirect mapper writes the same session key the `Flash` extractor
// reads, and one spelling of it has to be authoritative.
pub(crate) use flash::FLASH_DATA_KEY;

// Re-export the redacting secret wrapper for credential/token holders.
pub use secrecy;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The application identity contract.
///
/// Implemented by the application's user type. The framework uses this to
/// store/retrieve the user ID in the session and to type the auth extractors
/// ([`Auth<U>`], [`OptionalAuth<U>`], [`AuthManager<U>`]).
///
/// The application owns identity schema -- this trait does NOT mandate a fixed
/// `User` table, role model, or permission system.
///
/// # Example
///
/// ```
/// use arcature::AuthUser;
///
/// # #[allow(dead_code)]
/// pub struct User {
///     pub id: uuid::Uuid,
///     pub email: String,
/// }
///
/// impl AuthUser for User {
///     type Id = uuid::Uuid;
///     const SESSION_KEY: &'static str = "user_id";
///
///     fn id(&self) -> &uuid::Uuid {
///         &self.id
///     }
/// }
/// # fn main() {}
/// ```
pub trait AuthUser: Send + Sync + 'static {
    /// The type stored in the session to identify the user. Must be
    /// serializable/deserializable (e.g. `Uuid`, `i64`, `String`).
    type Id: Serialize + DeserializeOwned + Clone + Send + Sync + 'static;

    /// The session key under which the user ID is stored. Defaults to
    /// `"user_id"`.
    const SESSION_KEY: &'static str = "user_id";

    /// Get the ID to store in the session on login.
    fn id(&self) -> &Self::Id;
}
