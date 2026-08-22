//! Hashed personal access tokens.
//!
//! An opaque bearer credential for API clients that have no cookie and no
//! session: a CLI, a CI job, a mobile app, another service. A token is minted
//! for a subject, carries a set of abilities and a deadline, and is presented
//! on each request.
//!
//! # The one property worth remembering
//!
//! **The database never holds a token.** A token is a public 16-byte id and a
//! secret 32-byte half; the row holds the id and the SHA-256 of the secret.
//! [`ApiTokens::issue`] returns the plaintext exactly once, and nothing --
//! not this crate, not a `SELECT`, not a backup -- can produce it again.
//! Losing it means issuing another.
//!
//! # Why the digest is fast
//!
//! SHA-256, deliberately, not argon2. The secret is 256 bits of uniform
//! randomness, so a slow hash defends against a search that is already
//! impossible, while costing tens of milliseconds and tens of megabytes on
//! *every API request*. The full argument is written at the hashing site,
//! `digest_of` in `src/tokens/store.rs`; it is the reason this module exists in the shape it
//! does.
//!
//! # Expiry is not optional
//!
//! [`NewApiToken`] has no constructor without a deadline and the column has no
//! null state, because a credential that outlives the reason it was minted is
//! the ordinary way a leak stays useful. Every read carries
//! `expires_at > now()` evaluated by the database, so an expired token stops
//! working the instant it expires whether or not
//! [`ApiTokens::sweep_expired`] has run.
//!
//! # Example
//!
//! ```no_run
//! // Needs a database, so this example is compiled and not run.
//! use arcature::tokens::{Abilities, ApiTokens, NewApiToken};
//! use std::time::Duration;
//!
//! # async fn example(pool: arcature::tokens::TokenPool)
//! # -> Result<(), Box<dyn std::error::Error>> {
//! let tokens = ApiTokens::new(pool);
//! tokens.migrate().await?;
//!
//! let issued = tokens
//!     .issue(
//!         &NewApiToken::expiring_in("user:42", "CI deploy key", Duration::from_secs(3600))
//!             .abilities(Abilities::of(["deploy:write"])),
//!     )
//!     .await?;
//!
//! // Show this once and never again.
//! println!("{}", issued.plaintext().expose());
//!
//! for token in tokens.list_for("user:42").await? {
//!     println!("{} ({}) expires {}", token.name(), token.id(), token.expires_at());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Presenting one
//!
//! A client sends the plaintext as `Authorization: Bearer <token>`, and
//! [`ApiAuth`] turns it back into an [`ApiToken`] or rejects the request. The
//! store reaches the extractor through an axum `Extension`, so an application
//! installs it once on the router rather than threading it into state.

mod dialect;
mod error;
mod extract;
mod migrate;
mod store;
mod token;

pub use dialect::TokenPool;
pub use error::ApiTokenError;
pub use extract::ApiAuth;
pub use store::ApiTokens;
pub use token::{
    Abilities, ApiToken, ApiTokenId, IssuedApiToken, NewApiToken, PlaintextToken, TOKEN_PREFIX,
};
