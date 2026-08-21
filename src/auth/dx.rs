//! Session ergonomics, flash messages, and authorization policies.
//!
//! The authentication extractors moved to [`crate::auth::extract`]; they are
//! re-exported here so the `auth::dx` paths keep resolving.
//!
//! # Extractors
//!
//! - [`Auth<U>`] -- the authenticated user. 401 if not logged in.
//! - [`OptionalAuth<U>`] -- `Option<U>`. `None` if not logged in (no rejection).
//! - [`AuthManager<U>`] -- login/logout. Holds the session handle.
//! - [`Session`] -- high-level session ergonomics (`put`/`get`/`forget`).
//! - [`Flash`] -- one-time session messages.
//!
//! # Binding does NOT imply authorization
//!
//! `Auth<U>` proves the user is authenticated. It does NOT authorize access
//! to any specific resource. Authorization is a separate, explicit step via
//! [`Auth::authorize`] and the [`Policy`] trait.

use std::convert::Infallible;

use axum::extract::FromRequestParts;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tower_sessions::Session as TowerSession;

use crate::auth::AuthUser;

pub use crate::auth::extract::{
    Auth, AuthError, AuthManager, Current, LoginBuilder, OptionalAuth, OptionalCurrent, UserLoader,
};

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

/// High-level session ergonomics over `tower_sessions::Session`.
///
/// Wraps the raw session with the `put`/`get`/`forget`/`regenerate` API. The
/// underlying `tower_sessions::Session` is accessible via [`Session::raw`] for
/// escape-hatch access.
pub struct Session(pub(crate) TowerSession);

impl Session {
    /// Store a value in the session.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the session write fails.
    pub async fn put<T: Serialize>(&self, key: &str, value: T) -> Result<(), SessionError> {
        self.0
            .insert(key, value)
            .await
            .map_err(|e| SessionError(e.to_string()))
    }

    /// Get a value from the session.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the session read fails.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, SessionError> {
        self.0
            .get(key)
            .await
            .map_err(|e| SessionError(e.to_string()))
    }

    /// Remove a value from the session, returning it if present.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the session remove fails.
    pub async fn forget<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, SessionError> {
        self.0
            .remove(key)
            .await
            .map_err(|e| SessionError(e.to_string()))
    }

    /// Regenerate the session ID. Use after login to prevent session fixation.
    /// Calls `tower_sessions::Session::cycle_id`.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the cycle fails.
    pub async fn regenerate(&self) -> Result<(), SessionError> {
        self.0
            .cycle_id()
            .await
            .map_err(|e| SessionError(e.to_string()))
    }

    /// Flush all session data (equivalent to logout).
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the flush fails.
    pub async fn flush(&self) -> Result<(), SessionError> {
        self.0
            .flush()
            .await
            .map_err(|e| SessionError(e.to_string()))
    }

    /// Access the raw `tower_sessions::Session` for escape-hatch use.
    #[must_use]
    pub fn raw(&self) -> &TowerSession {
        &self.0
    }
}

/// A typed error from session operations.
#[derive(Debug)]
pub struct SessionError(pub String);

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "session error: {}", self.0)
    }
}

impl std::error::Error for SessionError {}

impl<S> FromRequestParts<S> for Session
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let session = TowerSession::from_request_parts(parts, state)
            .await
            .map_err(|_| unreachable!("Session extraction is infallible"))?;
        Ok(Session(session))
    }
}

/// One-time session messages.
///
/// Flash messages are stored in the session for one request: the handler
/// writes a message (e.g. "Profile updated"), redirects, and the next request
/// reads and clears the flash data. This is the standard PRG
/// (Post-Redirect-Get) flash pattern.
///
/// `Flash` is a genuine Axum `FromRequestParts` extractor. On extraction, it
/// reads the flash messages from the session and clears them. The handler can
/// write new messages via `flash.success()`, `flash.error()`, etc. -- these
/// persist in the session and are read by the next request's `Flash`
/// extractor.
pub struct Flash {
    session: TowerSession,
    messages: Vec<FlashMessage>,
    data: std::collections::BTreeMap<String, String>,
}

/// A single flash message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlashMessage {
    /// The severity level.
    pub level: FlashLevel,
    /// The message text.
    pub message: String,
}

/// The severity level of a flash message.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FlashLevel {
    /// A success message (green).
    Success,
    /// An error message (red).
    Error,
    /// A warning message (yellow).
    Warning,
    /// An informational message (blue).
    Info,
}

/// The session key under which levelled flash messages are stored.
const FLASH_KEY: &str = "_flash";

/// The session key under which `redirect().with(..)` key/value data is stored.
///
/// Separate from [`FLASH_KEY`] because the two are different shapes with
/// different writers: this one is a `BTreeMap<String, String>` written by the
/// [`RedirectMapper`](crate::routing::RedirectMapper) above the handler, that
/// one is a `Vec<FlashMessage>` written by the handler itself. Sharing a key
/// would mean one of them silently clobbering the other.
///
/// `pub(crate)` rather than private: the mapper writes it, and it must be the
/// same string in both places.
pub(crate) const FLASH_DATA_KEY: &str = "_flash_data";

impl Flash {
    /// Get the flash messages read from the session (already cleared).
    #[must_use]
    pub fn messages(&self) -> &[FlashMessage] {
        &self.messages
    }

    /// True if there are neither messages nor key/value data.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty() && self.data.is_empty()
    }

    /// Read one key of the data flashed by
    /// [`redirect().with(..)`](crate::http::response::RedirectResponse::with).
    ///
    /// Already cleared from the session by the time the handler sees it, so
    /// this is the one and only request that can read it.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.data.get(key).map(String::as_str)
    }

    /// Every key/value pair flashed by
    /// [`redirect().with(..)`](crate::http::response::RedirectResponse::with),
    /// in key order.
    #[must_use]
    pub fn data(&self) -> &std::collections::BTreeMap<String, String> {
        &self.data
    }

    /// Add a success flash message. Persists in the session for the next
    /// request.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError`] if the session write fails.
    pub async fn success(&self, message: &str) -> Result<(), FlashError> {
        self.add(FlashLevel::Success, message).await
    }

    /// Add an error flash message.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError`] if the session write fails.
    pub async fn error(&self, message: &str) -> Result<(), FlashError> {
        self.add(FlashLevel::Error, message).await
    }

    /// Add a warning flash message.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError`] if the session write fails.
    pub async fn warning(&self, message: &str) -> Result<(), FlashError> {
        self.add(FlashLevel::Warning, message).await
    }

    /// Add an info flash message.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError`] if the session write fails.
    pub async fn info(&self, message: &str) -> Result<(), FlashError> {
        self.add(FlashLevel::Info, message).await
    }

    async fn add(&self, level: FlashLevel, message: &str) -> Result<(), FlashError> {
        let mut messages: Vec<FlashMessage> = self
            .session
            .get(FLASH_KEY)
            .await
            .map_err(|e| FlashError::Session(e.to_string()))?
            .unwrap_or_default();
        messages.push(FlashMessage {
            level,
            message: message.to_string(),
        });
        self.session
            .insert(FLASH_KEY, &messages)
            .await
            .map_err(|e| FlashError::Session(e.to_string()))?;
        Ok(())
    }
}

impl<S> FromRequestParts<S> for Flash
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let session = TowerSession::from_request_parts(parts, state)
            .await
            .map_err(|_| unreachable!("Session extraction is infallible"))?;

        // Read and clear flash messages. If the session read fails, start
        // with an empty flash -- the handler can still write new messages.
        let messages: Vec<FlashMessage> = session
            .get(FLASH_KEY)
            .await
            .map_err(|e| FlashError::Session(e.to_string()))
            .unwrap_or(None)
            .unwrap_or_default();

        // Clear the flash from the session.
        let _ = session.remove::<Vec<FlashMessage>>(FLASH_KEY).await;

        // The same read-then-clear for the `redirect().with(..)` half, which
        // the mapper wrote on the *previous* request.
        let data: std::collections::BTreeMap<String, String> = session
            .get(FLASH_DATA_KEY)
            .await
            .unwrap_or(None)
            .unwrap_or_default();
        if !data.is_empty() {
            let _ = session
                .remove::<std::collections::BTreeMap<String, String>>(FLASH_DATA_KEY)
                .await;
        }

        Ok(Flash {
            session,
            messages,
            data,
        })
    }
}

/// A typed error from flash operations.
#[derive(Debug)]
pub enum FlashError {
    /// A session operation failed.
    Session(String),
}

impl std::fmt::Display for FlashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Session(msg) => write!(f, "session error: {msg}"),
        }
    }
}

impl std::error::Error for FlashError {}

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
