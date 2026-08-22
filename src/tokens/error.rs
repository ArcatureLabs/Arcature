//! What can go wrong when minting, reading, or revoking an API token.

/// An error from the personal access token store.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ApiTokenError {
    /// The database refused or could not answer the statement.
    #[error("arcature_api_tokens: {source}")]
    Database {
        /// The driver error underneath.
        #[from]
        source: sqlx::Error,
    },

    /// The operating system randomness source was unavailable.
    ///
    /// Reported rather than worked around. A token's only defence against
    /// guessing is that it is unpredictable, so a fallback that is merely
    /// hard to predict -- a clock, a counter, a process id -- would be a
    /// token this store cannot honestly call a secret.
    #[error("arcature_api_tokens: the OS randomness source is unavailable")]
    Entropy,

    /// A row was read but a column does not hold what the schema promises.
    ///
    /// Nothing this store writes can produce it. It means something else
    /// wrote the row, or a column type was changed underneath the schema.
    #[error("arcature_api_tokens: a stored token could not be decoded: {0}")]
    Decode(String),

    /// A timestamp is outside the range the column, or `chrono`, can hold.
    ///
    /// Storing a different instant than the one asked for is how a token
    /// outlives its expiry, so the write is refused instead.
    #[error("arcature_api_tokens: timestamp {0} is outside the range this database can store")]
    Expiry(String),

    /// A freshly generated token id was already taken, repeatedly.
    ///
    /// An id is 128 bits from the OS randomness source, so a single clash is
    /// already implausible and a run of them is not chance. It is reported
    /// rather than retried forever because the realistic cause is a broken
    /// random source, and quietly looping would hide it.
    #[error("arcature_api_tokens: no unused token id after {attempts} attempts")]
    IdCollision {
        /// How many ids were tried before giving up.
        attempts: u32,
    },
}
