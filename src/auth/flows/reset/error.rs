//! What can go wrong when issuing or redeeming a password-reset token.

/// An error from the password-reset store.
///
/// Note what is *not* here: "wrong token". A token that does not redeem is
/// [`PasswordResets::consume`](super::PasswordResets::consume) returning
/// `Ok(None)`, not an error, because the four reasons it can fail -- malformed,
/// unknown, wrong secret, expired, already spent -- must be indistinguishable
/// to the caller. An error variant per reason is an enumeration oracle with a
/// type signature.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PasswordResetError {
    /// The database refused or could not answer the statement.
    #[error("arcature_password_resets: {source}")]
    Database {
        /// The driver error underneath.
        #[from]
        source: sqlx::Error,
    },

    /// The operating system randomness source was unavailable.
    ///
    /// Reported rather than worked around, for the same reason the API token
    /// store reports it: a reset token's only defence is that it cannot be
    /// guessed, so a fallback drawn from a clock or a counter would be a
    /// token this store cannot honestly hand to a mailer.
    #[error("arcature_password_resets: the OS randomness source is unavailable")]
    Entropy,

    /// The requested deadline is outside the range `chrono`, or the column,
    /// can hold.
    ///
    /// Storing a different instant than the one asked for is how a reset link
    /// outlives its deadline -- or arrives already dead -- so the write is
    /// refused instead. The payload describes the deadline that could not be
    /// represented.
    #[error("arcature_password_resets: the deadline {0} is outside the representable range")]
    Expiry(String),

    /// A freshly generated token id was already taken, repeatedly.
    ///
    /// An id is 128 bits from the OS randomness source, so one clash is
    /// already implausible and a run of them is not chance. Reported rather
    /// than retried forever because the realistic cause is a broken random
    /// source, and looping would hide it.
    #[error("arcature_password_resets: no unused token id after {attempts} attempts")]
    IdCollision {
        /// How many ids were tried before giving up.
        attempts: u32,
    },
}
