//! What can go wrong in the database session store.

/// An error from the database session store.
///
/// `tower_sessions` reduces every store failure to its own three-variant
/// [`session_store::Error`](tower_sessions::session_store::Error), whose
/// payloads are strings. This type is what the store's own inherent methods
/// return, so an application that calls [`migrate`] or [`sweep_expired`]
/// directly gets the real cause and not a sentence about it.
///
/// [`migrate`]: super::DbSessionStore::migrate
/// [`sweep_expired`]: super::DbSessionStore::sweep_expired
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SessionStoreError {
    /// The database refused or could not answer the statement.
    #[error("arcature_sessions: {source}")]
    Database {
        /// The driver error underneath.
        #[from]
        source: sqlx::Error,
    },

    /// A row was read but its `data` column is not a session.
    ///
    /// Nothing this store writes can produce it. It means something else
    /// wrote the row, or the column type was changed underneath the schema.
    #[error("arcature_sessions: stored session data could not be decoded: {0}")]
    Decode(String),

    /// A newly generated session id was already taken, repeatedly.
    ///
    /// An id is 128 bits from the session layer's random source, so a single
    /// clash is already implausible and a run of them is not chance. It is
    /// reported rather than retried forever because the realistic cause is a
    /// broken random source, and quietly looping would hide it.
    #[error("arcature_sessions: no unused session id after {attempts} attempts")]
    IdCollision {
        /// How many ids were tried before giving up.
        attempts: u32,
    },

    /// An expiry instant is outside the range the column can hold.
    ///
    /// Storing a different instant than the one asked for is how a session
    /// outlives its expiry, so the write is refused instead.
    #[error("arcature_sessions: expiry {0} is outside the range this database can store")]
    Expiry(String),
}

impl From<SessionStoreError> for tower_sessions::session_store::Error {
    fn from(error: SessionStoreError) -> Self {
        match error {
            // `Decode` is the one variant `tower_sessions` has a matching
            // shape for; everything else is the backend failing, which is
            // what its `Backend` variant means.
            SessionStoreError::Decode(_) => Self::Decode(error.to_string()),
            other => Self::Backend(other.to_string()),
        }
    }
}
