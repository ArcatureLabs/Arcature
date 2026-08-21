//! Seeding a session so `acting_as` has something to act as.
//!
//! Logging in through the application's own login route is the honest thing
//! to do when the login route is what a test is about. It is the wrong thing
//! for the other several hundred tests, which pay two extra requests and a
//! password hash to reach the page they actually care about.
//!
//! So the harness writes the session record directly and sends the cookie the
//! session middleware would have set. That is only correct if it produces
//! exactly the cookie the middleware accepts, which means the same store, the
//! same cookie name, and the same signing key the application was built with
//! -- all three are constructor arguments here, because guessing any of them
//! would produce a request that silently arrives anonymous.

use std::sync::Arc;

use tower_sessions::cookie::{Cookie, CookieJar, Key};
use tower_sessions::{Session, SessionStore};
use tower_sessions_memory_store::MemoryStore;

/// The session store, cookie name, and signing key of the application under
/// test.
///
/// Clone freely: every clone shares the one store, which is what makes a
/// session written before a request visible to the request.
#[derive(Clone)]
pub struct TestSessions {
    store: MemoryStore,
    cookie_name: String,
    key: Arc<Key>,
}

/// Why a session could not be seeded.
#[derive(Debug)]
pub enum TestSessionError {
    /// The signing key was not 64 bytes. `cookie::Key` requires 64; a shorter
    /// key is rejected here rather than panicking inside the cookie crate.
    InvalidSigningKey {
        /// The length that was supplied.
        length: usize,
    },
    /// A session value did not serialize to JSON.
    Serialize(String),
    /// The session store rejected the write.
    Store(String),
}

impl std::fmt::Display for TestSessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSigningKey { length } => write!(
                formatter,
                "session signing key must be 64 bytes, got {length}"
            ),
            Self::Serialize(message) => {
                write!(formatter, "session value did not serialize: {message}")
            }
            Self::Store(message) => write!(formatter, "session store write failed: {message}"),
        }
    }
}

impl std::error::Error for TestSessionError {}

impl TestSessions {
    /// Build a seeder for an application whose session layer uses
    /// `cookie_name` and `signing_key`.
    ///
    /// Hand [`store`](Self::store) to
    /// [`ApplicationBuilder::session`](crate::ApplicationBuilder::session)
    /// so the application and the harness share one store.
    ///
    /// # Errors
    ///
    /// Returns [`TestSessionError::InvalidSigningKey`] unless `signing_key`
    /// is exactly 64 bytes -- the length
    /// [`SessionConfig`](crate::auth::SessionConfig) also requires.
    pub fn new(
        cookie_name: impl Into<String>,
        signing_key: &[u8],
    ) -> Result<Self, TestSessionError> {
        if signing_key.len() != 64 {
            return Err(TestSessionError::InvalidSigningKey {
                length: signing_key.len(),
            });
        }
        Ok(Self {
            store: MemoryStore::default(),
            cookie_name: cookie_name.into(),
            key: Arc::new(Key::from(signing_key)),
        })
    }

    /// The shared store. Pass this to the application builder.
    #[must_use]
    pub fn store(&self) -> MemoryStore {
        self.store.clone()
    }

    /// The cookie name the application's session layer uses.
    #[must_use]
    pub fn cookie_name(&self) -> &str {
        &self.cookie_name
    }
}

impl TestSessions {
    /// Write `entries` into a fresh session and return the `Cookie` header
    /// value that names it.
    ///
    /// The value is signed with the same key and under the same name the
    /// application's layer verifies with, so the middleware loads this
    /// session rather than starting an anonymous one.
    ///
    /// # Errors
    ///
    /// Returns [`TestSessionError::Serialize`] if a value is not
    /// JSON-serializable, or [`TestSessionError::Store`] if the store write
    /// fails.
    pub async fn cookie_for(
        &self,
        entries: &[(String, serde_json::Value)],
    ) -> Result<String, TestSessionError> {
        let session = Session::new(None, Arc::new(self.store.clone()), None);
        for (key, value) in entries {
            session
                .insert(key, value)
                .await
                .map_err(|error| TestSessionError::Serialize(error.to_string()))?;
        }
        session
            .save()
            .await
            .map_err(|error| TestSessionError::Store(error.to_string()))?;
        let id = session
            .id()
            .ok_or_else(|| TestSessionError::Store("session was saved without an id".into()))?;

        let mut jar = CookieJar::new();
        jar.signed_mut(&self.key)
            .add(Cookie::new(self.cookie_name.clone(), id.to_string()));
        let signed = jar
            .get(&self.cookie_name)
            .ok_or_else(|| TestSessionError::Store("signed cookie was not produced".into()))?;
        Ok(format!("{}={}", signed.name(), signed.value()))
    }

    /// Read a value back out of the store for a session id.
    ///
    /// Lets a test assert on what a request *wrote* to the session, which is
    /// otherwise invisible from the response.
    ///
    /// # Errors
    ///
    /// Returns [`TestSessionError::Store`] if the store read fails, or
    /// [`TestSessionError::Serialize`] if the stored value is not the
    /// requested type.
    pub async fn get<T>(&self, id: &str, key: &str) -> Result<Option<T>, TestSessionError>
    where
        T: serde::de::DeserializeOwned,
    {
        let id: tower_sessions::session::Id = id
            .parse()
            .map_err(|_| TestSessionError::Store(format!("`{id}` is not a session id")))?;
        let record = self
            .store
            .load(&id)
            .await
            .map_err(|error| TestSessionError::Store(error.to_string()))?;
        let Some(record) = record else {
            return Ok(None);
        };
        record
            .data
            .get(key)
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(|error| TestSessionError::Serialize(error.to_string()))
    }
}

impl std::fmt::Debug for TestSessions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TestSessions")
            .field("cookie_name", &self.cookie_name)
            .field("signing_key", &"<redacted 64-byte secret>")
            .finish_non_exhaustive()
    }
}
