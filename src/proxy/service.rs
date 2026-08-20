//! The pre-routing proxy Tower service.
//!
//! [`ProxyLayer`] wraps the entire router service (produced by
//! [`axum::Router::into_service`]) so the application's proxy function runs
//! *before* route selection — a genuine pre-routing contract (engine spec
//! §3/§4). This fixes the architecture mismatch where the proxy was
//! previously wired via `Router::layer` (which applies *after* routing).
//!
//! The service is `Service<Request<Body>, Response = Response<Body>, Error = Infallible> + Clone + Send + 'static`
//! — the exact bound `axum::serve` requires (verified against axum 0.8.9).
//! Every internal failure is converted to a redacted 500 response so the
//! service is infallible from the caller's perspective.
//!
//! # Future type
//!
//! The proxy service returns a `Pin<Box<dyn Future + Send>>` — a boxed future
//! — because the proxy function can produce two structurally different
//! futures (delegate-to-inner vs. immediate-response). This is the only
//! place in the engine that uses heterogeneous erasure (engine spec §19:
//! contain `Box<dyn Future>` where genuinely required). The boxing is one
//! allocation per request when a proxy is installed; the no-proxy path also
//! boxes for a uniform `Service::Future` type, but `axum::serve` already
//! allocates per connection. The request path remains: pre-routing layers ->
//! Axum Router -> handler.
//!
//! # Security
//!
//! - Rewrite targets are validated: an invalid or scheme-injected URI is
//!   rejected with 400 (not silently applied).
//! - Redirect `location` and `SetHeaders` values are scanned for CRLF
//!   injection and rejected with 400.
//! - Internal failures never leak internal details — a redacted 500 is
//!   returned (engine spec security review).

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;

use super::ProxyFn;
use crate::axum::body::Body;
use crate::axum::http::{HeaderMap, HeaderName, HeaderValue, Response, StatusCode, Uri};
use crate::proxy::action::Action;
use crate::proxy::request::Request as ProxyRequest;

/// A boxed future that resolves to `Result<Response<Body>, Infallible>`.
type BoxFuture = Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

/// A Tower layer that wraps an inner service with the pre-routing proxy.
///
/// Constructed by the pipeline assembler; the application never touches this
/// type directly. The layer is `Clone` (the `ProxyFn` is `Arc`-shared) so the
/// produced service is `Clone` — a requirement of `axum::serve`.
#[derive(Clone)]
pub struct ProxyLayer {
    proxy: Option<ProxyFn>,
}

impl ProxyLayer {
    /// Build a proxy layer. When `proxy` is `None`, the layer is a pass-through
    /// (no proxy function installed).
    #[must_use]
    pub fn new(proxy: Option<ProxyFn>) -> Self {
        Self { proxy }
    }
}

impl<S> tower::Layer<S> for ProxyLayer {
    type Service = ProxyService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ProxyService {
            inner,
            proxy: self.proxy.clone(),
        }
    }
}

/// The pre-routing proxy service. Wraps the inner router service and runs the
/// application's proxy function *before* delegating to it.
///
/// `Inner` is typically `RouterIntoService<Body, ()>` (produced by
/// `Router::into_service()`), but any `Service<Request<Body>, Response =
/// Response<Body>, Error = Infallible> + Clone + Send + 'static` works.
#[derive(Clone)]
pub struct ProxyService<Inner> {
    inner: Inner,
    proxy: Option<ProxyFn>,
}

impl<Inner> tower::Service<crate::axum::extract::Request<Body>> for ProxyService<Inner>
where
    Inner: tower::Service<
            crate::axum::extract::Request<Body>,
            Response = Response<Body>,
            Error = Infallible,
        > + Clone
        + Send
        + 'static,
    Inner::Future: Send + 'static,
{
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = BoxFuture;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: crate::axum::extract::Request<Body>) -> Self::Future {
        // If no proxy function is installed, delegate directly — no proxy
        // evaluation overhead. The box is a single allocation, already paid by
        // axum::serve's per-connection setup.
        let Some(proxy) = &self.proxy else {
            let fut = self.inner.call(req);
            // `Inner::Future: Send + 'static` (the `where` clause), so the
            // future coerces to `BoxFuture` (a `Pin<Box<dyn Future + Send>>`).
            return Box::pin(fut);
        };

        // Take the request apart to inspect method/uri/headers for the proxy
        // function, then reassemble for the inner service.
        let (mut parts, body) = req.into_parts();

        let proxy_request = ProxyRequest::new(&parts.method, &parts.uri, &parts.headers);
        let action = proxy(proxy_request);

        match action {
            Action::Continue { set_headers } => {
                // Merge the provided headers into the request. The engine
                // validates against CRLF injection.
                if let Err(rejection) = merge_headers(&mut parts.headers, set_headers) {
                    return Box::pin(async move { Ok(rejection) });
                }
                let req = crate::axum::extract::Request::from_parts(parts, body);
                let fut = self.inner.call(req);
                Box::pin(fut)
            }
            Action::Redirect {
                location,
                permanent,
            } => {
                // Validate the location against CRLF/header injection.
                match validate_header_value(&location) {
                    Ok(value) => {
                        let status = if permanent {
                            StatusCode::MOVED_PERMANENTLY
                        } else {
                            StatusCode::FOUND
                        };
                        let mut headers = HeaderMap::new();
                        headers.insert(HeaderName::from_static("location"), value);
                        Box::pin(async move { Ok(build_response(status, Some(headers), None)) })
                    }
                    Err(rejection) => Box::pin(async move { Ok(rejection) }),
                }
            }
            Action::Rewrite { uri } => {
                // Validate and parse the new URI. An invalid target is
                // rejected with 400 — it must not be silently applied.
                match validate_rewrite_uri(&uri) {
                    Ok(new_uri) => {
                        parts.uri = new_uri;
                        let req = crate::axum::extract::Request::from_parts(parts, body);
                        let fut = self.inner.call(req);
                        Box::pin(fut)
                    }
                    Err(rejection) => Box::pin(async move { Ok(rejection) }),
                }
            }
            Action::ShortCircuit { status, response } => {
                if let Some(response) = response {
                    Box::pin(async move { Ok(response) })
                } else {
                    Box::pin(async move { Ok(build_response(status, None, None)) })
                }
            }
        }
    }
}

/// Build a response from status, optional headers, and optional body. Adds
/// `X-Content-Type-Options: nosniff` for safety. Never panics.
fn build_response(
    status: StatusCode,
    headers: Option<HeaderMap>,
    body: Option<Body>,
) -> Response<Body> {
    let mut builder = Response::builder().status(status);
    builder = builder.header(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    if let Some(h) = headers {
        for (name, value) in h.iter() {
            builder = builder.header(name.clone(), value.clone());
        }
    }
    builder
        .body(body.unwrap_or_else(Body::empty))
        .unwrap_or_else(|_| {
            // Header construction can only fail on invalid header values,
            // which we have already validated. Fallback: a minimal response.
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::empty())
                .unwrap_or_else(|_| Response::new(Body::empty()))
        })
}

/// Validate a header value for CRLF injection. Returns a 400 rejection on
/// failure, a valid `HeaderValue` on success.
//
// The `Err` variant is a `Response<Body>` (>=128 bytes). It is immediately
// consumed as the HTTP response — never stored, iterated, or returned through
// a long-lived collection — so boxing it would add an allocation on an error
// path for no benefit. The `result_large_err` lint is suppressed here for that
// reason (no arbitrary allocation on the hot path; this is the cold error
// path).
#[allow(clippy::result_large_err)]
fn validate_header_value(value: &str) -> Result<HeaderValue, Response<Body>> {
    // Reject CR, LF, and any control character (CRLF injection defense).
    if value
        .bytes()
        .any(|b| b == b'\r' || b == b'\n' || (b < 0x20 && b != b'\t'))
    {
        return Err(build_response(
            StatusCode::BAD_REQUEST,
            None,
            Some(Body::from(
                "invalid header value: CRLF or control character detected",
            )),
        ));
    }
    HeaderValue::try_from(value).map_err(|_| {
        build_response(
            StatusCode::BAD_REQUEST,
            None,
            Some(Body::from("invalid header value")),
        )
    })
}

/// Validate a rewrite URI. Rejects absolute URIs with a scheme (which could
/// redirect the request to an external host) and invalid URI syntax. Returns
/// a parsed `Uri` on success, a 400 response on failure.
//
// See `validate_header_value` for the `result_large_err` justification: the
// `Err` variant is an immediately-consumed response, not a stored value.
#[allow(clippy::result_large_err)]
fn validate_rewrite_uri(uri: &str) -> Result<Uri, Response<Body>> {
    // Parse the URI first — invalid syntax is a 400.
    let parsed = uri.parse::<Uri>().map_err(|_| {
        build_response(
            StatusCode::BAD_REQUEST,
            None,
            Some(Body::from("invalid rewrite URI")),
        )
    })?;

    // Reject a scheme part — a rewrite should only change the path/query,
    // not redirect to an external host (proxy rewrite URI parsing). An
    // absolute URI with a scheme is a redirect, not a rewrite.
    if parsed.scheme_str().is_some() {
        return Err(build_response(
            StatusCode::BAD_REQUEST,
            None,
            Some(Body::from("rewrite URI must not contain a scheme")),
        ));
    }

    Ok(parsed)
}

/// Merge `source` headers into `destination`. Validates each value for CRLF
/// injection before inserting. Existing headers with the same name are
/// overwritten (not appended) — the proxy's `SetHeaders` is a set, not an add.
//
// See `validate_header_value` for the `result_large_err` justification.
#[allow(clippy::result_large_err)]
fn merge_headers(destination: &mut HeaderMap, source: HeaderMap) -> Result<(), Response<Body>> {
    for (name, value) in source.iter() {
        // Validate against CRLF injection — defense-in-depth even for
        // application-provided values.
        if value.as_bytes().contains(&b'\r') || value.as_bytes().contains(&b'\n') {
            return Err(build_response(
                StatusCode::BAD_REQUEST,
                None,
                Some(Body::from(
                    "invalid header value in SetHeaders: CRLF detected",
                )),
            ));
        }
        destination.insert(name.clone(), value.clone());
    }
    Ok(())
}
