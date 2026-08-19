//! Realtime error types. Hand-rolled `Display`/`Debug` (no error-derive
//! dependency). Coarse variants to avoid attacker-controlled bytes.

use std::fmt;

use axum::http::StatusCode;

use super::channel::ChannelError;

/// An error from a realtime operation.
#[derive(Debug)]
pub enum RealtimeError {
    /// The origin was not allowed.
    Origin,
    /// The channel authorization failed.
    Unauthorized,
    /// The connection limit was reached.
    ConnectionLimit,
    /// A protocol error (oversize, malformed, utf8, stream). The hint is a
    /// fixed low-cardinality string, safe to record in tracing spans.
    Protocol { hint: ProtocolHint },
    /// A channel error (lagged, closed, full).
    Channel(ChannelError),
    /// The drain timed out with `remaining` connections still live.
    Shutdown { remaining: usize },
}

/// A fixed hint for a protocol error (low-cardinality, safe to record).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolHint {
    /// A message exceeded the size limit.
    Oversize,
    /// A message was malformed.
    Malformed,
    /// A message was not valid UTF-8.
    Utf8,
    /// A stream-level error.
    Stream,
}

impl fmt::Display for ProtocolHint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oversize => f.write_str("oversize"),
            Self::Malformed => f.write_str("malformed"),
            Self::Utf8 => f.write_str("utf8"),
            Self::Stream => f.write_str("stream"),
        }
    }
}

impl fmt::Display for RealtimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Origin => f.write_str("origin not allowed"),
            Self::Unauthorized => f.write_str("unauthorized"),
            Self::ConnectionLimit => f.write_str("connection limit reached"),
            Self::Protocol { hint } => write!(f, "protocol error: {hint}"),
            Self::Channel(e) => write!(f, "channel error: {e}"),
            Self::Shutdown { remaining } => {
                write!(f, "drain timed out with {remaining} connections remaining")
            }
        }
    }
}

impl std::error::Error for RealtimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Channel(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ChannelError> for RealtimeError {
    fn from(e: ChannelError) -> Self {
        Self::Channel(e)
    }
}

/// The HTTP status returned for a realtime admission error (explicit mapping,
/// not `IntoResponse`, to avoid payload leakage).
#[must_use]
pub fn admission_status(e: &RealtimeError) -> StatusCode {
    match e {
        RealtimeError::Origin | RealtimeError::Unauthorized => StatusCode::FORBIDDEN,
        RealtimeError::ConnectionLimit => StatusCode::SERVICE_UNAVAILABLE,
        RealtimeError::Protocol { .. }
        | RealtimeError::Channel(_)
        | RealtimeError::Shutdown { .. } => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
