//! Typed errors for the auth subsystem (password, session, CSRF).
//!
//! No secret material is ever embedded in any variant. Passwords, signing
//! keys, and tokens are held by the caller or by redacting wrappers; the error
//! types report only the reason for the failure.

use std::fmt;

// --- Password errors --------------------------------------------------------

/// Failure from [`crate::auth::PasswordHasher::hash`].
#[derive(Debug)]
pub enum PasswordHashError {
    /// The Argon2id parameters are out of range.
    InvalidParams {
        /// Which parameter and why.
        detail: String,
    },
    /// The underlying Argon2id computation failed.
    Hash {
        /// The upstream `argon2::password_hash::Error`.
        source: argon2::password_hash::Error,
    },
}

impl fmt::Display for PasswordHashError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidParams { detail } => {
                write!(formatter, "invalid Argon2id parameters: {detail}")
            }
            Self::Hash { source } => write!(formatter, "Argon2id hashing failed: {source}"),
        }
    }
}

impl std::error::Error for PasswordHashError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Hash { source } => Some(source),
            Self::InvalidParams { .. } => None,
        }
    }
}

/// Failure from [`crate::auth::verify_password`].
#[derive(Debug)]
pub enum PasswordVerifyError {
    /// The stored string is not a valid PHC-format Argon2id hash.
    MalformedHash {
        /// The parse detail from `password_hash`.
        detail: String,
    },
    /// The password does not match the stored hash.
    PasswordMismatch,
    /// The underlying Argon2id verification computation failed.
    Verify {
        /// The upstream `argon2::password_hash::Error`.
        source: argon2::password_hash::Error,
    },
}

impl fmt::Display for PasswordVerifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedHash { detail } => {
                write!(formatter, "malformed stored password hash: {detail}")
            }
            Self::PasswordMismatch => write!(formatter, "password does not match stored hash"),
            Self::Verify { source } => write!(formatter, "Argon2id verification failed: {source}"),
        }
    }
}

impl std::error::Error for PasswordVerifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Verify { source } => Some(source),
            Self::MalformedHash { .. } | Self::PasswordMismatch => None,
        }
    }
}

// --- Session errors ---------------------------------------------------------

/// Validation failure for [`crate::auth::SessionConfig`].
#[derive(Debug)]
pub enum SessionConfigError {
    /// A required duration is zero, which would make sessions unusable.
    ZeroDuration {
        /// Which setting was zero.
        field: &'static str,
    },
    /// The session-cookie signing key is not exactly 64 bytes.
    InvalidSigningKey {
        /// Why the key was rejected.
        reason: SigningKeyReason,
    },
    /// The cookie name or path is empty.
    EmptyCookieAttribute {
        /// Which attribute was empty.
        attribute: &'static str,
    },
    /// A `__Host-`-prefixed session cookie was combined with `Secure = false`.
    InsecureHostPrefixedCookie {
        /// The offending cookie name.
        cookie_name: String,
    },
}

/// Why a session signing key was rejected.
#[derive(Debug, Clone, Copy)]
pub enum SigningKeyReason {
    /// The key was not exactly 64 bytes.
    WrongLength,
}

impl fmt::Display for SigningKeyReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongLength => write!(formatter, "must be exactly 64 bytes"),
        }
    }
}

impl fmt::Display for SessionConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDuration { field } => write!(formatter, "{field} must not be zero"),
            Self::InvalidSigningKey { reason } => {
                write!(formatter, "invalid session signing key: {reason}")
            }
            Self::EmptyCookieAttribute { attribute } => {
                write!(formatter, "session cookie {attribute} must not be empty")
            }
            Self::InsecureHostPrefixedCookie { cookie_name } => write!(
                formatter,
                "session cookie `{cookie_name}` uses the __Host- prefix which mandates \
                 Secure = true; use SessionConfig::dev() for a development policy"
            ),
        }
    }
}

impl std::error::Error for SessionConfigError {}

/// Failure from [`crate::auth::SessionConfig::into_layer`] construction: the
/// resolved configuration was internally inconsistent.
#[derive(Debug)]
pub struct SessionBuildError {
    source: SessionConfigError,
}

impl SessionBuildError {
    pub(crate) fn new(source: SessionConfigError) -> Self {
        Self { source }
    }
}

impl fmt::Display for SessionBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "session layer construction failed: {}",
            self.source
        )
    }
}

impl std::error::Error for SessionBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

// --- CSRF errors ------------------------------------------------------------

/// Failure from [`crate::auth::CsrfLayer`] rejecting an unsafe request.
#[derive(Debug)]
pub enum CsrfError {
    /// A state-changing request was missing the CSRF header token.
    MissingHeader,
    /// The header token did not match the cookie token.
    TokenMismatch,
    /// The CSRF cookie was missing or unreadable on a state-changing request.
    MissingCookie,
    /// The CSRF cookie value was malformed (wrong length or encoding).
    MalformedCookie,
}

impl fmt::Display for CsrfError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHeader => write!(formatter, "missing CSRF header token"),
            Self::TokenMismatch => write!(formatter, "CSRF token mismatch"),
            Self::MissingCookie => write!(formatter, "missing CSRF cookie"),
            Self::MalformedCookie => write!(formatter, "malformed CSRF cookie"),
        }
    }
}

impl std::error::Error for CsrfError {}

/// Validation failure for [`crate::auth::CsrfConfig`].
#[derive(Debug)]
pub enum CsrfConfigError {
    /// The `Secure` attribute was set to `false` while the cookie name carries
    /// the `__Host-` prefix.
    InsecureHostPrefixedCookie {
        /// The cookie name that conflicts with `Secure = false`.
        cookie_name: String,
    },
}

impl fmt::Display for CsrfConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsecureHostPrefixedCookie { cookie_name } => write!(
                formatter,
                "CSRF cookie \"{cookie_name}\" carries the __Host- prefix, which requires \
                 Secure = true (RFC 6265bis). Use CsrfConfig::dev() for HTTP development, \
                 or set a non-__Host- cookie name before with_secure(false)."
            ),
        }
    }
}

impl std::error::Error for CsrfConfigError {}
