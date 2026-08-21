//! The handler-facing session API: `put` / `get` / `forget` / `regenerate`.
//!
//! [`Session`] is a genuine Axum [`axum::extract::FromRequestParts`]
//! extractor wrapping [`tower_sessions::Session`]. It is the ergonomic half
//! of the session story; the cookie attributes, signing key and middleware
//! layer are configured in [`crate::auth::session`], and the one-time
//! messages that ride in the same session live in [`crate::auth::flash`].

use std::convert::Infallible;

use axum::extract::FromRequestParts;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tower_sessions::Session as TowerSession;

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
