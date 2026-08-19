//! Observability: request IDs, HTTP access logging, and tracing integration.
//!
//! Built on the certified `tracing` ecosystem. Arcature never installs a
//! global default subscriber on the production path; operators wire their own.
//! The framework re-exports `tracing` so downstream code targets the pinned
//! version through Arcature.
//!
//! The [`RequestId`] type is a validated, low-cardinality identifier echoed
//! on every application response via the `x-request-id` header (wire-
//! compatible, no `X-Arcature-*` prefix). The [`RequestIdLayer`] Tower
//! layer resolves the id from the upstream header or generates one.

use std::fmt;
use std::str::FromStr;

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};

pub use tracing;

// ---------------------------------------------------------------------------
// RequestId
// ---------------------------------------------------------------------------

/// The `x-request-id` header name (wire-compatible, no `X-Arcature-*`).
pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// The maximum length of a request id, in bytes.
pub const MAX_REQUEST_ID_BYTES: usize = 128;

/// A validated request identifier.
///
/// Generated as a UUID v4, or parsed from the upstream `x-request-id` header.
/// The charset is an allow-list (alphanumerics + `-_.:@+/=`); hostile input is
/// rejected and a fresh id is generated instead (never errors on the request
/// path).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(String);

impl RequestId {
    /// Generate a new request id (UUID v4).
    #[must_use]
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// Parse a request id from a string. Returns an error if the string is
    /// empty, too large, or contains disallowed characters.
    pub fn from_str(s: &str) -> Result<Self, RequestIdError> {
        s.parse()
    }

    /// Resolve a request id from the upstream `x-request-id` header. Never
    /// errors: on hostile or missing input, a fresh id is generated.
    #[must_use]
    pub fn from_header(headers: &HeaderMap) -> Self {
        if let Some(value) = headers.get(&REQUEST_ID_HEADER)
            && let Ok(s) = value.to_str()
        {
            if let Ok(id) = Self::from_str(s) {
                return id;
            }
        }
        Self::generate()
    }

    /// The id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Build the response header value.
    pub fn to_response_header(&self) -> Result<HeaderValue, ObserveError> {
        HeaderValue::from_str(&self.0).map_err(|_| ObserveError::RequestHeader {
            reason: "request id is not valid header bytes",
        })
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for RequestId {
    type Err = RequestIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(RequestIdError::Empty);
        }
        if s.len() > MAX_REQUEST_ID_BYTES {
            return Err(RequestIdError::TooLarge {
                size: s.len(),
                limit: MAX_REQUEST_ID_BYTES,
            });
        }
        if !s.bytes().all(is_allowed_request_id_byte) {
            return Err(RequestIdError::InvalidChar);
        }
        Ok(Self(s.to_string()))
    }
}

/// Whether a byte is allowed in a request id.
fn is_allowed_request_id_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'@' | b'+' | b'/' | b'=')
}

/// An error from parsing a request id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestIdError {
    /// The id was empty.
    Empty,
    /// The id exceeded the maximum length.
    TooLarge { size: usize, limit: usize },
    /// The id contained a disallowed character.
    InvalidChar,
}

impl fmt::Display for RequestIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("request id must not be empty"),
            Self::TooLarge { size, limit } => {
                write!(f, "request id is {size} bytes, exceeds {limit}-byte limit")
            }
            Self::InvalidChar => f.write_str("request id contains a disallowed character"),
        }
    }
}

impl std::error::Error for RequestIdError {}

// ---------------------------------------------------------------------------
// ObserveError
// ---------------------------------------------------------------------------

/// A small contextual error for the observe layer's own construction failures.
/// The `reason` is a fixed `&'static str` (never the offending header value).
#[derive(Debug)]
pub enum ObserveError {
    /// A response header could not be constructed.
    RequestHeader {
        reason: &'static str,
    },
}

impl fmt::Display for ObserveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestHeader { reason } => write!(f, "observe header error: {reason}"),
        }
    }
}

impl std::error::Error for ObserveError {}

// ---------------------------------------------------------------------------
// Stable span names (observability contract)
// ---------------------------------------------------------------------------

/// The stable span name for HTTP request handling.
pub const REQUEST: &str = "arcature.request";
/// The stable span name for database queries.
pub const DB_QUERY: &str = "arcature.db.query";
/// The stable span name for cache get operations.
pub const CACHE_GET: &str = "arcature.cache.get";
/// The stable span name for job handler execution.
pub const JOB_HANDLE: &str = "arcature.job.handle";
/// The stable span name for page rendering.
pub const PAGE_RENDER: &str = "arcature.page.render";
/// The stable span name for event listener execution.
pub const EVENT_LISTENER: &str = "arcature.event.listener";
/// The stable span name for schedule ticks.
pub const SCHEDULE_TICK: &str = "arcature.schedule.tick";

/// All stable span names, in canonical order.
pub const ALL: &[&str] = &[
    REQUEST,
    DB_QUERY,
    CACHE_GET,
    JOB_HANDLE,
    PAGE_RENDER,
    EVENT_LISTENER,
    SCHEDULE_TICK,
];

/// Whether a span name is a stable Arcature span name.
#[must_use]
pub fn is_stable(name: &str) -> bool {
    ALL.contains(&name)
}

// ---------------------------------------------------------------------------
// RequestIdLayer — Tower layer that assigns the request id
// ---------------------------------------------------------------------------

use std::convert::Infallible;
use axum::extract::Request;
use axum::response::Response;
use tower::{Layer, Service};

/// A Tower layer that resolves the request id from the upstream header or
/// generates one, inserts it into request extensions, and echoes it on the
/// response.
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestIdLayer;

impl<S> Layer<S> for RequestIdLayer {
    type Service = RequestIdService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        RequestIdService { inner }
    }
}

/// The service produced by [`RequestIdLayer`].
#[derive(Debug, Clone)]
pub struct RequestIdService<S> {
    inner: S,
}

impl<S> Service<Request> for RequestIdService<S>
where
    S: Service<Request, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request) -> Self::Future {
        let id = RequestId::from_header(req.headers());
        req.extensions_mut().insert(id.clone());

        let inner = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, inner);
        Box::pin(async move {
            let mut response = inner.call(req).await?;
            if let Ok(value) = id.to_response_header() {
                response.headers_mut().insert(REQUEST_ID_HEADER, value);
            }
            Ok(response)
        })
    }
}

/// The HTTP status returned for an observe error (for admission paths).
#[must_use]
pub fn admission_status(_e: &ObserveError) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}
