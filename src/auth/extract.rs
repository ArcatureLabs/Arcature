//! The authentication extractors: who is logged in, and how they log in.
//!
//! These are genuine Axum [`axum::extract::FromRequestParts`] extractors --
//! Axum remains the handler runtime. The session is
//! [`tower_sessions::Session`], accessed via the `auth` feature.
//!
//! # Extractors
//!
//! - [`Auth<U>`] -- the authenticated user. 401 if not logged in.
//! - [`OptionalAuth<U>`] -- `Option<U>`. `None` if not logged in (no rejection).
//! - [`AuthManager<U>`] -- login/logout. Holds the session handle.
//!
//! [`Current<U>`] and [`OptionalCurrent<U>`] are the golden-path spellings of
//! the first two.
//!
//! # Binding does NOT imply authorization
//!
//! `Auth<U>` proves the user is authenticated. It does NOT authorize access
//! to any specific resource. Authorization is a separate, explicit step via
//! [`Auth::authorize`](crate::auth::Auth::authorize) and the
//! [`Policy`](crate::auth::Policy) trait.

use std::convert::Infallible;
use std::future::Future;
use std::marker::PhantomData;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::FromRequestParts;
use axum::response::{IntoResponse, Response};
use tower_sessions::Session as TowerSession;

use crate::auth::AuthUser;

/// Session key under which the authentication timestamp (Unix milliseconds)
/// is stored at login, for the absolute authenticated-lifetime enforcement in
/// [`load_user`]. The idle/inactivity timeout is a separate, sliding bound
/// owned by the session layer; this timestamp is the anchor for the hard cap
/// that activity cannot reset. Millisecond precision keeps a short absolute
/// bound deterministic.
const ABSOLUTE_AUTH_AT_KEY: &str = "__arcature_absolute_auth_at";

/// The current time as Unix milliseconds. Cannot panic: a clock set before
/// the Unix epoch yields `0` rather than aborting. Used only to stamp and
/// compare the absolute session lifetime.
fn now_unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The authenticated user. Extracts from the session, loading the user from
/// application state via `UserLoader<S>`.
///
/// Returns `401 Unauthorized` if no user is logged in or the session is stale.
/// Authorization is a separate, explicit step
/// ([`authorize`](crate::auth::Auth::authorize)).
pub struct Auth<U: AuthUser>(pub U);

impl<U: AuthUser> Auth<U> {
    /// Extract the user value.
    #[must_use]
    pub fn into_inner(self) -> U {
        self.0
    }

    /// Get a reference to the user.
    #[must_use]
    pub fn user(&self) -> &U {
        &self.0
    }
}

impl<U, S> FromRequestParts<S> for Auth<U>
where
    U: UserLoader<S>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let user = load_user::<U, S>(parts, state).await?;
        user.map(Auth).ok_or_else(|| {
            (
                axum::http::StatusCode::UNAUTHORIZED,
                "Authentication required",
            )
                .into_response()
        })
    }
}

/// The optional authenticated user. `None` if not logged in (no rejection).
///
/// Use this for routes that behave differently for authenticated vs anonymous
/// users (e.g. a landing page that shows a dashboard link if logged in).
pub struct OptionalAuth<U: AuthUser>(pub Option<U>);

impl<U: AuthUser> OptionalAuth<U> {
    /// Get the user if authenticated.
    #[must_use]
    pub fn user(&self) -> Option<&U> {
        self.0.as_ref()
    }

    /// True if a user is authenticated.
    #[must_use]
    pub fn is_authenticated(&self) -> bool {
        self.0.is_some()
    }
}

impl<U, S> FromRequestParts<S> for OptionalAuth<U>
where
    U: UserLoader<S>,
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let user = load_user::<U, S>(parts, state).await.unwrap_or(None);
        Ok(OptionalAuth(user))
    }
}

/// The current authenticated user -- the zero-plumbing golden-path name for
/// [`Auth<U>`].
///
/// `Current<User>` extracts exactly the same value as `Auth<User>` (the
/// authenticated user from the session, 401 if none) and is a type alias so
/// the two are fully interchangeable. The name `Current<User>` reads as "the
/// current user" on the golden path; `Auth<User>` remains available for
/// callers that prefer the explicit auth vocabulary.
pub type Current<U> = Auth<U>;

/// The optional current user -- the zero-plumbing golden-path name for
/// [`OptionalAuth<U>`].
pub type OptionalCurrent<U> = OptionalAuth<U>;

/// The auth manager -- login, logout, and session control.
///
/// Extracted from the request as a genuine Axum extractor. Holds the
/// `tower_sessions::Session` handle. The handler calls `login`, `logout`,
/// etc.
///
/// `login()` automatically rotates the session ID before binding the user
/// (session-fixation defense); applications do not need to call
/// `regenerate()` after `login()`.
pub struct AuthManager<U: AuthUser> {
    session: TowerSession,
    _marker: PhantomData<U>,
}

impl<U: AuthUser> AuthManager<U> {
    /// Begin a login. Returns a [`LoginBuilder`] that stores the user ID in
    /// the session on `.await`.
    ///
    /// # Session fixation defense
    ///
    /// Awaiting the builder **automatically rotates the session ID** before
    /// the user is bound, by calling `tower_sessions::Session::cycle_id`. The
    /// anonymous -> authenticated transition must rotate the ID so a
    /// session-fixation attack cannot persist past login. This is mandatory
    /// and not opt-in.
    ///
    /// # Absolute lifetime
    ///
    /// Awaiting the builder also stamps the authentication time (Unix
    /// milliseconds) into the session under a dedicated key. [`Auth`]`<U>` /
    /// [`OptionalAuth`]`<U>` enforce a hard cap on the authenticated lifetime
    /// measured from this stamp.
    ///
    /// ```
    /// use arcature::{AuthError, AuthManager, AuthUser};
    ///
    /// # struct User { id: i64 }
    /// # impl AuthUser for User {
    /// #     type Id = i64;
    /// #     fn id(&self) -> &i64 { &self.id }
    /// # }
    /// async fn sign_in(auth: &AuthManager<User>, user: &User) -> Result<(), AuthError> {
    ///     auth.login(user).remember(true).await
    /// }
    /// # fn main() {}
    /// ```
    #[must_use]
    pub fn login(&self, user: &U) -> LoginBuilder<'_, U> {
        LoginBuilder {
            session: &self.session,
            user_id: user.id().clone(),
            remember: false,
        }
    }

    /// Log out: flush the session, clearing all data (including the user ID).
    /// The session cookie is invalidated.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Session`] if the session flush fails.
    pub async fn logout(&self) -> Result<(), AuthError> {
        self.session
            .flush()
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        Ok(())
    }

    /// Regenerate the session ID. Calls `tower_sessions::Session::cycle_id`.
    ///
    /// This is the manual escape hatch for rotating a session ID outside
    /// login. The login path already rotates the ID automatically; applications
    /// do not need to call `regenerate()` after `login()`.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Session`] if the cycle fails.
    pub async fn regenerate(&self) -> Result<(), AuthError> {
        self.session
            .cycle_id()
            .await
            .map_err(|e| AuthError::Session(e.to_string()))?;
        Ok(())
    }
}

impl<U, S> FromRequestParts<S> for AuthManager<U>
where
    U: AuthUser,
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
        Ok(AuthManager {
            session,
            _marker: PhantomData,
        })
    }
}

/// A builder for the login operation. Stores the user ID in the session on
/// `.await`.
pub struct LoginBuilder<'a, U: AuthUser> {
    session: &'a TowerSession,
    user_id: U::Id,
    remember: bool,
}

impl<'a, U: AuthUser> LoginBuilder<'a, U> {
    /// Set the "remember me" flag. When true, the session's max-age is
    /// extended (if the session architecture permits it). When false
    /// (default), the session uses the configured inactivity-based expiry.
    #[must_use]
    pub fn remember(mut self, remember: bool) -> Self {
        self.remember = remember;
        self
    }
}

impl<'a, U: AuthUser> std::future::IntoFuture for LoginBuilder<'a, U> {
    type Output = Result<(), AuthError>;
    type IntoFuture = std::pin::Pin<
        std::boxed::Box<dyn std::future::Future<Output = Result<(), AuthError>> + Send + 'a>,
    >;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            // Auto-rotate the session ID before binding the user (session
            // fixation defense: the anonymous -> authenticated transition
            // must rotate the ID). `cycle_id` preserves existing session data
            // and issues a fresh ID, so any attacker-pre-set session ID is
            // discarded. The user ID is then stored in the *new* session.
            self.session
                .cycle_id()
                .await
                .map_err(|e| AuthError::Session(e.to_string()))?;
            self.session
                .insert(U::SESSION_KEY, &self.user_id)
                .await
                .map_err(|e| AuthError::Session(e.to_string()))?;
            // Bind the authentication timestamp for the absolute-lifetime
            // enforcement in `load_user`.
            self.session
                .insert(ABSOLUTE_AUTH_AT_KEY, now_unix_millis())
                .await
                .map_err(|e| AuthError::Session(e.to_string()))?;
            if self.remember {
                self.session
                    .insert("remember_me", true)
                    .await
                    .map_err(|e| AuthError::Session(e.to_string()))?;
            }
            Ok(())
        })
    }
}

/// A typed error from auth operations.
#[derive(Debug)]
pub enum AuthError {
    /// A session operation failed (read/write/cycle).
    Session(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Session(msg) => write!(f, "session error: {msg}"),
        }
    }
}

impl std::error::Error for AuthError {}

/// How to load an [`AuthUser`] from its session ID and application state.
///
/// The app implements this for its user type. `Auth<U>` and `OptionalAuth<U>`
/// call `U::load_user(id, state)` to resolve the authenticated user from the
/// session.
pub trait UserLoader<S>: AuthUser + Sized {
    /// The typed error from the load operation.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Load the user by its session ID from application state. Return
    /// `Ok(None)` if the user does not exist (the session is stale -- the
    /// extractor maps this to 401). Return `Err` for database errors.
    fn load_user(
        id: &Self::Id,
        state: &S,
    ) -> impl Future<Output = Result<Option<Self>, Self::Error>> + Send;

    /// The **absolute** authenticated session lifetime -- the maximum age,
    /// measured from the authentication timestamp stored in the session at
    /// login, after which a session is treated as logged out *regardless of
    /// activity*.
    ///
    /// This is the auth-boundary source for the absolute-lifetime enforcement
    /// in [`Auth`]/[`OptionalAuth`] (the idle/inactivity timeout is a
    /// separate, sliding bound owned by the session layer). The default is 30
    /// days.
    #[must_use]
    fn absolute_max_age() -> Duration {
        Duration::from_secs(60 * 60 * 24 * 30)
    }
}

/// Load the user from the session + state. Shared by `Auth<U>` and
/// `OptionalAuth<U>`.
///
/// The error is a whole `Response` rather than a small error enum because
/// both callers are extractors whose `Rejection` is `Response`: an enum here
/// would be converted into exactly this value one line later. Clippy counts
/// the 128 bytes and suggests boxing, which would only move the unboxing to
/// the two call sites without removing a single copy.
#[allow(clippy::result_large_err)]
async fn load_user<U, S>(
    parts: &mut axum::http::request::Parts,
    state: &S,
) -> Result<Option<U>, Response>
where
    U: UserLoader<S>,
    S: Send + Sync,
{
    // Extract the session.
    let session = TowerSession::from_request_parts(parts, state)
        .await
        .map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "session extraction failed",
            )
                .into_response()
        })?;

    // Read the user ID from the session. On error, return a generic 500
    // without leaking session internals.
    let user_id: Option<U::Id> = session.get(U::SESSION_KEY).await.map_err(|_err| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "session read failed",
        )
            .into_response()
    })?;

    let user_id = match user_id {
        Some(id) => id,
        None => return Ok(None),
    };

    // Absolute authenticated-lifetime enforcement. The auth boundary reads
    // the authentication timestamp bound at login and compares it to the
    // absolute max age. A session older than the absolute bound is flushed and
    // treated as logged out, *regardless of activity*. Enforced before the
    // user load so an expired session never pays for a database round trip.
    let absolute_max_millis: i64 =
        i64::try_from(U::absolute_max_age().as_millis()).unwrap_or(i64::MAX);
    let auth_at: Option<i64> = session.get(ABSOLUTE_AUTH_AT_KEY).await.map_err(|_err| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "session read failed",
        )
            .into_response()
    })?;
    match auth_at {
        Some(auth_at) => {
            // `saturating_sub` clamps a negative result (clock set backward)
            // to 0, so clock skew never logs a user out -- only a genuinely
            // elapsed absolute lifetime does. Millisecond precision keeps a
            // short bound deterministic.
            if now_unix_millis().saturating_sub(auth_at) > absolute_max_millis {
                session.flush().await.map_err(|_err| {
                    (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        "session flush failed",
                    )
                        .into_response()
                })?;
                return Ok(None);
            }
        }
        None => {
            // The user is bound but no auth timestamp exists -- a session
            // created before this feature (upgrade). Begin tracking by
            // stamping now; do NOT log out existing users on upgrade.
            session
                .insert(ABSOLUTE_AUTH_AT_KEY, now_unix_millis())
                .await
                .map_err(|_err| {
                    (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        "session write failed",
                    )
                        .into_response()
                })?;
        }
    }

    // Load the user from state.
    let user = U::load_user(&user_id, state).await.map_err(|_err| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "user load failed",
        )
            .into_response()
    })?;

    Ok(user)
}
