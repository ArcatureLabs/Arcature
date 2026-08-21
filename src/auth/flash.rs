//! One-time session messages -- the PRG (Post-Redirect-Get) flash pattern.
//!
//! [`Flash`] is a genuine Axum [`axum::extract::FromRequestParts`] extractor.
//! It carries two payloads that ride in the same session but are written by
//! different code: levelled [`FlashMessage`]s written by the handler, and the
//! key/value data written above the handler by
//! [`redirect().with(..)`](crate::http::response::RedirectResponse::with).
//! Both are read and cleared on extraction, so exactly one request sees them.

use std::convert::Infallible;

use axum::extract::FromRequestParts;
use serde::{Deserialize, Serialize};
use tower_sessions::Session as TowerSession;

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
