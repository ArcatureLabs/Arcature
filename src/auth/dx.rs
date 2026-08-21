//! Compatibility re-exports for the four concerns that used to share this
//! file.
//!
//! The authentication extractors moved to [`crate::auth::extract`], the
//! session API to [`crate::auth::session_api`], the flash messages to
//! [`crate::auth::flash`] and the authorization seam to
//! [`crate::auth::policy`]. Everything is re-exported here so the `auth::dx`
//! paths keep resolving.

pub use crate::auth::extract::{
    Auth, AuthError, AuthManager, Current, LoginBuilder, OptionalAuth, OptionalCurrent, UserLoader,
};
pub use crate::auth::flash::{Flash, FlashError, FlashLevel, FlashMessage};
pub use crate::auth::policy::{AuthzError, Policy};
pub use crate::auth::session_api::{Session, SessionError};
