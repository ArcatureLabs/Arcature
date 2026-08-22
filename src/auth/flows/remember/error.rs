//! What can go wrong when issuing or presenting a remember-me token.

/// An error from the remember-me store.
///
/// Note what is *not* here: "wrong token", and -- more importantly -- "stolen
/// token". A cookie that does not sign anybody in is
/// [`RememberOutcome`](super::RememberOutcome), not an error, and that is a
/// deliberate split rather than a stylistic one.
///
/// The reason differs from the password-reset store's. There, the outcomes are
/// collapsed because telling them apart would be an enumeration oracle. Here
/// they are *not* collapsed -- the caller genuinely needs to know a theft from
/// an unknown cookie, because one of them means warning a user and ending
/// their other sessions. What they are is **not errors**: a browser presenting
/// a cookie from a laptop that was wiped last month is the system working, and
/// a `Result` that is `Err` on the ordinary path teaches every call site to
/// log-and-ignore the variant that matters.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RememberTokenError {
    /// The database refused or could not answer the statement.
    #[error("arcature_remember_tokens: {source}")]
    Database {
        /// The driver error underneath.
        #[from]
        source: sqlx::Error,
    },

    /// The operating system randomness source was unavailable.
    ///
    /// Reported rather than worked around, for the same reason the API token
    /// and password-reset stores report it: this credential's only defence is
    /// that it cannot be guessed, so a fallback drawn from a clock or a
    /// counter would be a cookie this store cannot honestly hand to a
    /// browser.
    ///
    /// It is worth one extra sentence here, because this store draws
    /// randomness on a path the other two do not: every rotation mints a fresh
    /// secret, so the entropy source is touched on ordinary authenticated
    /// requests and not only when somebody signs in. A source that fails
    /// intermittently shows up as intermittent failures to *stay* signed in.
    #[error("arcature_remember_tokens: the OS randomness source is unavailable")]
    Entropy,

    /// The requested deadline is outside the range `chrono`, or the column,
    /// can hold.
    ///
    /// Storing a different instant than the one asked for is how a cookie
    /// outlives its deadline -- or arrives already dead -- so the write is
    /// refused instead. The payload describes the deadline that could not be
    /// represented.
    #[error("arcature_remember_tokens: the deadline {0} is outside the representable range")]
    Expiry(String),

    /// A freshly generated series was already taken, repeatedly.
    ///
    /// A series is 128 bits from the OS randomness source, so one clash is
    /// already implausible and a run of them is not chance. Reported rather
    /// than retried forever because the realistic cause is a broken random
    /// source, and looping would hide it.
    #[error("arcature_remember_tokens: no unused series after {attempts} attempts")]
    SeriesCollision {
        /// How many series were tried before giving up.
        attempts: u32,
    },
}
