//! OAuth 2.0 Authorization Code with PKCE.
//!
//! Provider-agnostic: an application names its own authorization and token
//! endpoints. The bundled providers are plain consts holding those URLs, not
//! a closed enum -- a provider the framework has never heard of is configured
//! the same way as GitHub.
//!
//! # The flow
//!
//! ```ignore
//! // 1. Starting a sign-in.
//! let client = OauthClient::new(oauth::GITHUB, id, Some(secret), "https://app.test/callback")?;
//! let start = client.authorize(&["read:user"])?;
//! session.insert("oauth.state", start.state().as_str());
//! session.insert("oauth.verifier", start.verifier().secret());
//! redirect(start.url().as_str())
//!
//! // 2. The callback.
//! let stored = OauthState::from_stored(session.take("oauth.state")?);
//! let verifier = PkceVerifier::from_secret(session.take("oauth.verifier")?);
//! let tokens = client.exchange(&stored, &query.state, &query.code, verifier).await?;
//! ```
//!
//! # What this module refuses to do
//!
//! * **Plaintext transport.** Every endpoint and the redirect URI must be
//!   `https`, with one exception: `http` on a loopback host, because a local
//!   development redirect has no network to be intercepted on. There is no
//!   flag to turn this off.
//! * **Redirects on the token endpoint.** The HTTP client is built with
//!   `redirect::Policy::none()`; following a redirect from a token endpoint
//!   is a server-side request forgery primitive.
//! * **Carrying a response body into an error.** See [`error`].
//!
//! # What is never logged
//!
//! The PKCE verifier, the `state`, and every token. [`PkceVerifier`],
//! [`OauthState`] and [`TokenSet`] all redact under `Debug` and none of them
//! implements `Display`, so none can reach a log line without a call that
//! names the secret out loud.

pub mod error;
pub mod pkce;
pub mod provider;

pub use error::OauthError;
pub use pkce::{OauthState, PkceVerifier, constant_time_eq};
pub use provider::{Authorization, DISCORD, Endpoints, GITHUB, GOOGLE, OauthClient, TokenSet};

// Re-export the certified `oauth2` crate so downstream code targets the
// Arcature-pinned version.
pub use oauth2;
