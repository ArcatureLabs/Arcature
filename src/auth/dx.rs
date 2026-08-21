//! Deprecated compatibility re-exports. Use the four modules named below.
//!
//! This file used to hold four unrelated concerns in ~900 lines under a name
//! that revealed none of them -- `dx` abbreviates "developer experience",
//! which names a goal rather than a thing. Each concern now lives in a module
//! named after what is in it:
//!
//! | Was `auth::dx::` | Is now |
//! |---|---|
//! | `Auth`, `OptionalAuth`, `Current`, `OptionalCurrent`, `AuthManager`, `LoginBuilder`, `AuthError`, `UserLoader` | [`crate::auth::extract`] |
//! | `Session`, `SessionError` | [`crate::auth::session_api`] |
//! | `Flash`, `FlashMessage`, `FlashLevel`, `FlashError` | [`crate::auth::flash`] |
//! | `Policy`, `AuthzError`, `Auth::authorize` | [`crate::auth::policy`] |
//!
//! Nothing here is a new type: every name below resolves to exactly the type
//! it always did, so code written against `arcature::auth::dx` keeps
//! compiling. The module is scheduled for removal in `0.2.0`. The shortest
//! fix is to delete the `dx` segment -- `arcature::auth::Auth` and the
//! crate-root `arcature::Auth` are unchanged and are not deprecated.
//!
//! # Why some of these warn and some do not
//!
//! rustc ignores `#[deprecated]` on a `pub use` (rust-lang/rust#30827), so a
//! re-export cannot carry a deprecation of its own. Every item that can be
//! spelled as a deprecated type alias instead is, and warns on use. The
//! exceptions are the two traits, which have no alias form, and the three
//! tuple structs with public fields -- `Auth`, `OptionalAuth` and
//! `SessionError` -- because a type alias cannot be used as a tuple-struct
//! constructor or pattern, so aliasing them would stop `Auth(user)` and
//! `let Auth(user) = ..` compiling. Keeping those callers working matters
//! more than warning them, and this page is their notice.
//!
//! # The compatibility contract
//!
//! Every name this module ever exported, spelled through `auth::dx`. This is
//! a compiled test rather than an illustration: if it stops compiling, a
//! `0.1.0` application stops compiling too, and the release stops being a
//! patch. It warns loudly when run -- that is the point.
//!
//! ```
//! use arcature::auth::AuthUser;
//! use arcature::auth::dx::{
//!     Auth, AuthError, AuthManager, AuthzError, Current, Flash, FlashError, FlashLevel,
//!     FlashMessage, LoginBuilder, OptionalAuth, OptionalCurrent, Policy, Session, SessionError,
//!     UserLoader,
//! };
//!
//! struct User {
//!     id: i64,
//! }
//!
//! impl AuthUser for User {
//!     type Id = i64;
//!     fn id(&self) -> &i64 {
//!         &self.id
//!     }
//! }
//!
//! // A loader written against the old path still names the trait the
//! // extractors resolve from its new home.
//! impl UserLoader<()> for User {
//!     type Error = std::convert::Infallible;
//!     async fn load_user(id: &i64, _state: &()) -> Result<Option<Self>, Self::Error> {
//!         Ok(Some(User { id: *id }))
//!     }
//! }
//!
//! struct Doc;
//! struct DocPolicy;
//!
//! // Likewise a policy.
//! impl Policy<Doc> for DocPolicy {
//!     type User = User;
//!     fn check(_user: &User, action: &str, _resource: &Doc) -> bool {
//!         action == "view"
//!     }
//! }
//!
//! // The four names that only ever appear in type position.
//! fn types(_m: AuthManager<User>, _b: LoginBuilder<'_, User>, _s: Session, _f: Flash) {}
//!
//! // Tuple structs, constructed and destructured exactly as at 0.1.0.
//! let auth = Auth(User { id: 7 });
//! assert_eq!(auth.user().id, 7);
//! let OptionalAuth(absent) = OptionalAuth::<User>(None);
//! assert!(absent.is_none());
//! assert_eq!(SessionError("boom".into()).to_string(), "session error: boom");
//!
//! // The golden-path aliases name the same extractor as before.
//! let _current: Current<User> = auth;
//! let _optional: OptionalCurrent<User> = OptionalAuth::<User>(None);
//!
//! // Enum variants, in expression and in pattern.
//! assert_eq!(FlashLevel::Success, arcature::auth::FlashLevel::Success);
//! assert!(matches!(AuthError::Session(String::new()), AuthError::Session(_)));
//! assert!(matches!(FlashError::Session(String::new()), FlashError::Session(_)));
//!
//! // A named-field struct.
//! let message = FlashMessage { level: FlashLevel::Info, message: "hi".into() };
//! assert_eq!(message.message, "hi");
//!
//! // The authorization seam, reached from the old path.
//! let auth = Auth(User { id: 1 });
//! assert!(auth.authorize::<Doc, DocPolicy>("view", &Doc).is_ok());
//! assert_eq!(
//!     auth.authorize::<Doc, DocPolicy>("delete", &Doc),
//!     Err(AuthzError::Forbidden),
//! );
//! ```

// The whole module exists to name items that have moved, so every reference
// to a moved item inside it is deliberate.
#![allow(deprecated)]

// --- Extractors -> `crate::auth::extract` -----------------------------------

// Plain re-exports: `Auth` and `OptionalAuth` are tuple structs with public
// fields, and `UserLoader` is a trait. See the module docs.
pub use crate::auth::extract::{Auth, OptionalAuth, UserLoader};

/// Deprecated. Moved to [`crate::auth::extract::AuthManager`].
#[deprecated(
    since = "0.1.1",
    note = "moved to arcature::auth::extract::AuthManager"
)]
pub type AuthManager<U> = crate::auth::extract::AuthManager<U>;

/// Deprecated. Moved to [`crate::auth::extract::LoginBuilder`].
#[deprecated(
    since = "0.1.1",
    note = "moved to arcature::auth::extract::LoginBuilder"
)]
pub type LoginBuilder<'a, U> = crate::auth::extract::LoginBuilder<'a, U>;

/// Deprecated. Moved to [`crate::auth::extract::Current`].
#[deprecated(since = "0.1.1", note = "moved to arcature::auth::extract::Current")]
pub type Current<U> = crate::auth::extract::Current<U>;

/// Deprecated. Moved to [`crate::auth::extract::OptionalCurrent`].
#[deprecated(
    since = "0.1.1",
    note = "moved to arcature::auth::extract::OptionalCurrent"
)]
pub type OptionalCurrent<U> = crate::auth::extract::OptionalCurrent<U>;

/// Deprecated. Moved to [`crate::auth::extract::AuthError`].
#[deprecated(since = "0.1.1", note = "moved to arcature::auth::extract::AuthError")]
pub type AuthError = crate::auth::extract::AuthError;

// --- Session API -> `crate::auth::session_api` ------------------------------

// A plain re-export: `SessionError` is a tuple struct with a public field.
pub use crate::auth::session_api::SessionError;

/// Deprecated. Moved to [`crate::auth::session_api::Session`].
#[deprecated(
    since = "0.1.1",
    note = "moved to arcature::auth::session_api::Session"
)]
pub type Session = crate::auth::session_api::Session;

// --- Flash -> `crate::auth::flash` ------------------------------------------

/// Deprecated. Moved to [`crate::auth::flash::Flash`].
#[deprecated(since = "0.1.1", note = "moved to arcature::auth::flash::Flash")]
pub type Flash = crate::auth::flash::Flash;

/// Deprecated. Moved to [`crate::auth::flash::FlashMessage`].
#[deprecated(since = "0.1.1", note = "moved to arcature::auth::flash::FlashMessage")]
pub type FlashMessage = crate::auth::flash::FlashMessage;

/// Deprecated. Moved to [`crate::auth::flash::FlashLevel`].
#[deprecated(since = "0.1.1", note = "moved to arcature::auth::flash::FlashLevel")]
pub type FlashLevel = crate::auth::flash::FlashLevel;

/// Deprecated. Moved to [`crate::auth::flash::FlashError`].
#[deprecated(since = "0.1.1", note = "moved to arcature::auth::flash::FlashError")]
pub type FlashError = crate::auth::flash::FlashError;

// --- Authorization -> `crate::auth::policy` ---------------------------------

// A plain re-export: `Policy` is a trait and has no alias form.
pub use crate::auth::policy::Policy;

/// Deprecated. Moved to [`crate::auth::policy::AuthzError`].
#[deprecated(since = "0.1.1", note = "moved to arcature::auth::policy::AuthzError")]
pub type AuthzError = crate::auth::policy::AuthzError;
