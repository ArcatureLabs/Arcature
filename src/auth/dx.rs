//! Authorization policies, and the compatibility re-exports for the three
//! concerns that used to share this file.
//!
//! The authentication extractors moved to [`crate::auth::extract`], the
//! session API to [`crate::auth::session_api`] and the flash messages to
//! [`crate::auth::flash`]; all three are re-exported here so the `auth::dx`
//! paths keep resolving.
//!
//! # Binding does NOT imply authorization
//!
//! [`Auth<U>`] proves the user is authenticated. It does NOT authorize access
//! to any specific resource. Authorization is a separate, explicit step via
//! [`Auth::authorize`] and the [`Policy`] trait.

use axum::response::{IntoResponse, Response};

use crate::auth::AuthUser;

pub use crate::auth::extract::{
    Auth, AuthError, AuthManager, Current, LoginBuilder, OptionalAuth, OptionalCurrent, UserLoader,
};
pub use crate::auth::flash::{Flash, FlashError, FlashLevel, FlashMessage};
pub use crate::auth::session_api::{Session, SessionError};

impl<U: AuthUser> Auth<U> {
    /// Authorize an action on a resource via a `Policy<M>` impl.
    ///
    /// Returns `Ok(())` if the policy allows, `Err(AuthzError::Forbidden)` if
    /// denied. This is the explicit authorization step -- it is never
    /// automatic.
    ///
    /// # Example
    ///
    /// Both type parameters have to be named: `M` is the resource type and
    /// `P` the policy, and Rust allows no partial turbofish.
    ///
    /// ```ignore
    /// auth.authorize::<Link, LinkPolicy>("update", &link)?;
    /// ```
    pub fn authorize<M, P: Policy<M, User = U>>(
        &self,
        action: &str,
        resource: &M,
    ) -> Result<(), AuthzError> {
        if P::check(&self.0, action, resource) {
            Ok(())
        } else {
            Err(AuthzError::Forbidden)
        }
    }
}

/// A typed authorization error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthzError {
    /// The policy denied the action.
    Forbidden,
}

impl std::fmt::Display for AuthzError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Forbidden => write!(f, "forbidden: policy denied the action"),
        }
    }
}

impl std::error::Error for AuthzError {}

impl IntoResponse for AuthzError {
    fn into_response(self) -> Response {
        (axum::http::StatusCode::FORBIDDEN, "Forbidden").into_response()
    }
}

/// A policy for resource type `M`.
///
/// The application implements this for its policy type. The `check` method
/// receives the authenticated user, an action name (e.g. `"view"`,
/// `"update"`), and the resource, and returns whether the action is allowed.
///
/// Authorization stays explicit: a policy is a type that decides whether a
/// user may perform an action on a resource. The application writes the
/// policy methods; the framework provides the [`Auth::authorize`] seam.
///
/// # Example
///
/// ```ignore
/// pub struct LinkPolicy;
///
/// impl arcature::Policy<Link> for LinkPolicy {
///     type User = User;
///     fn check(user: &User, action: &str, link: &Link) -> bool {
///         match action {
///             "view" => true,
///             "update" => user.id == link.user_id,
///             _ => false,
///         }
///     }
/// }
///
/// async fn show(auth: Auth<User>, link: Bound<Link>) -> Result<Page> {
///     auth.authorize::<LinkPolicy>("view", &link)?;
///     // ...
/// }
/// ```
///
/// # Binding does NOT imply authorization
///
/// `Bound<T>` loads the model; `Auth::authorize` checks the policy. These are
/// separate steps. Authorization is never automatic.
pub trait Policy<M>: Send + Sync + 'static {
    /// The user type this policy authorizes for.
    type User: AuthUser;

    /// Check whether `user` may perform `action` on `resource`.
    ///
    /// Returns `true` if allowed, `false` if denied. The caller
    /// ([`Auth::authorize`]) maps `false` to [`AuthzError::Forbidden`].
    fn check(user: &Self::User, action: &str, resource: &M) -> bool;
}
