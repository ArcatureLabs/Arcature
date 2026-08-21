//! Authorization: the [`Policy`] trait and the [`Auth::authorize`] seam.
//!
//! Authentication answers "who is this?" ([`crate::auth::extract`]);
//! authorization answers "may they do this?". The two are deliberately
//! separate steps -- loading a user, or binding a model, never authorizes
//! anything on its own.
//!
//! Laravel calls this pair Gate and Policy. Arcature has no Gate: there is no
//! global registry to look a policy up in, so [`Auth::authorize`] names the
//! policy type at the call site and the compiler checks that it matches the
//! resource and the user.

use axum::response::{IntoResponse, Response};

use crate::auth::AuthUser;
use crate::auth::extract::Auth;

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
