//! The proxy request view — a read-only borrow of the incoming request
//! presented to the application's proxy function.
//!
//! The engine builds this from the raw incoming request and hands it to the
//! application's `proxy` function *before* route selection (engine spec §5).
//! The application reads the method, URI, and headers and returns a
//! [`crate::ProxyAction`]; it never touches Axum types directly.

use crate::axum::http::{HeaderMap, Method, Uri};

/// A borrowed view of the incoming request for the proxy function.
///
/// Lives only as long as the underlying request parts; the application's
/// proxy function is synchronous and pure (no async, no I/O) so the borrow
/// is released before the engine performs any HTTP work.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    method: &'a Method,
    uri: &'a Uri,
    headers: &'a HeaderMap,
}

impl<'a> Request<'a> {
    /// Build a proxy request view from borrowed request parts.
    ///
    /// Called by the engine; application code receives a `Request` via its
    /// proxy function argument.
    #[must_use]
    pub const fn new(method: &'a Method, uri: &'a Uri, headers: &'a HeaderMap) -> Self {
        Self {
            method,
            uri,
            headers,
        }
    }

    /// The HTTP method of the incoming request.
    #[must_use]
    pub const fn method(&self) -> &Method {
        self.method
    }

    /// The request URI (path + query) as received, before any rewrite.
    #[must_use]
    pub const fn uri(&self) -> &Uri {
        self.uri
    }

    /// The request headers as received.
    #[must_use]
    pub const fn headers(&self) -> &HeaderMap {
        self.headers
    }
}
