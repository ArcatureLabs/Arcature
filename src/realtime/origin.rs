//! Origin policy for realtime endpoints (WebSocket + SSE).

use std::fmt;

use axum::http::{HeaderMap, HeaderValue};

/// A verified, non-ASCII-safe origin string. Built from a trusted source or
/// parsed from a header (non-ASCII is rejected).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VerifiedOrigin(String);

impl VerifiedOrigin {
    /// Build from a trusted string (no validation).
    #[must_use]
    pub fn from_trusted(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Build from a header value. Returns `None` if the value is not valid
    /// ASCII.
    #[must_use]
    pub fn from_header(value: &HeaderValue) -> Option<Self> {
        let s = value.to_str().ok()?;
        if !s.bytes().all(|b| b.is_ascii()) {
            return None;
        }
        Some(Self(s.to_string()))
    }

    /// The origin as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VerifiedOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The decision from an origin policy check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginDecision {
    /// The origin is allowed.
    Allowed,
    /// The origin is denied.
    Denied,
}

/// The origin policy: deny all, allow an exact origin, or allow a set.
#[derive(Debug, Clone)]
pub enum OriginPolicy {
    /// Deny all origins (the default).
    DenyAll,
    /// Allow a single exact origin.
    AllowExact {
        origin: VerifiedOrigin,
    },
    /// Allow a set of origins.
    AllowSet {
        origins: Vec<VerifiedOrigin>,
    },
}

impl OriginPolicy {
    /// Deny all origins.
    #[must_use]
    pub fn deny_all() -> Self {
        Self::DenyAll
    }

    /// Allow a single exact origin.
    #[must_use]
    pub fn allow_exact(origin: VerifiedOrigin) -> Self {
        Self::AllowExact { origin }
    }

    /// Allow a set of origins.
    #[must_use]
    pub fn allow_set(origins: Vec<VerifiedOrigin>) -> Self {
        Self::AllowSet { origins }
    }

    /// Check a request's origin header against the policy.
    #[must_use]
    pub fn authorize(&self, header: Option<&HeaderValue>) -> OriginDecision {
        match self {
            Self::DenyAll => OriginDecision::Denied,
            Self::AllowExact { origin } => {
                if let Some(h) = header
                    && let Some(v) = VerifiedOrigin::from_header(h)
                    && &v == origin
                {
                    OriginDecision::Allowed
                } else {
                    OriginDecision::Denied
                }
            }
            Self::AllowSet { origins } => {
                if let Some(h) = header
                    && let Some(v) = VerifiedOrigin::from_header(h)
                    && origins.contains(&v)
                {
                    OriginDecision::Allowed
                } else {
                    OriginDecision::Denied
                }
            }
        }
    }
}

impl Default for OriginPolicy {
    fn default() -> Self {
        Self::DenyAll
    }
}
