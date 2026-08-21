//! OAuth error type.
//!
//! # Why the variants carry so little
//!
//! An OAuth failure happens next to the most sensitive material an
//! application handles. The upstream `oauth2::RequestTokenError::Parse`
//! variant carries the raw response body, and that body is a token response
//! often enough that embedding its `Display` would put access tokens in log
//! files. Every variant here is therefore built from a fixed `&'static str`
//! or from a provider-supplied error *code*, never from a response body and
//! never from a secret.

use std::fmt;

/// A failure in the OAuth Authorization Code flow.
#[derive(Debug)]
pub enum OauthError {
    /// An endpoint or redirect URL could not be parsed.
    InvalidUrl {
        /// Which URL: `"authorization endpoint"`, `"token endpoint"`, or
        /// `"redirect URI"`.
        role: &'static str,
    },
    /// An endpoint or redirect URL was plaintext `http` on a non-loopback
    /// host. See [`crate::oauth`] for why this is refused rather than warned
    /// about.
    InsecureTransport {
        /// Which URL was refused.
        role: &'static str,
    },
    /// The operating system refused to produce randomness, so no
    /// unguessable `state` could be generated.
    Entropy,
    /// The `state` returned by the provider did not match the one this
    /// application generated. Either the response is a forgery or it belongs
    /// to a different sign-in attempt; both are refused the same way.
    StateMismatch,
    /// The token endpoint could not be reached.
    Transport,
    /// The token endpoint answered with an OAuth error. `code` is the
    /// provider's `error` member (`invalid_grant`, `invalid_client`, ...),
    /// which is a fixed vocabulary, not free-form text.
    Provider {
        /// The provider's `error` code.
        code: String,
    },
    /// The token endpoint's answer was not a token response this
    /// implementation understands. The body is deliberately not carried: a
    /// malformed *success* response still contains a token.
    MalformedResponse,
}

impl fmt::Display for OauthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl { role } => write!(f, "{role} is not a valid URL"),
            Self::InsecureTransport { role } => {
                write!(
                    f,
                    "{role} must use https (http is allowed only on loopback)"
                )
            }
            Self::Entropy => f.write_str("could not generate an unguessable OAuth state"),
            Self::StateMismatch => f.write_str("OAuth state did not match"),
            Self::Transport => f.write_str("the OAuth token endpoint could not be reached"),
            Self::Provider { code } => write!(f, "the OAuth provider returned `{code}`"),
            Self::MalformedResponse => {
                f.write_str("the OAuth token endpoint returned a response that could not be parsed")
            }
        }
    }
}

impl std::error::Error for OauthError {}
