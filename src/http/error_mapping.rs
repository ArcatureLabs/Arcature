//! Give error responses a body, and keep internal detail out of the ones the
//! application did not write itself.
//!
//! # The problem this solves
//!
//! Most error responses in a Rust web stack are produced by something other
//! than the application: axum answers an unmatched path with a bare `404`,
//! `tower-http` answers an oversized upload with a bare `413` and an expired
//! deadline with a bare `408`. "Bare" is literal -- status line, no
//! `Content-Type`, no body. A browser shows a blank page and a `fetch()`
//! caller gets `""` to parse.
//!
//! [`ErrorMapping`] gives those responses an RFC 9457
//! [`Problem`](crate::api::Problem) body, so every error leaving the
//! application looks the same whether a handler produced it or a layer did.
//!
//! # And the one it prevents
//!
//! The second job is redaction. A `500` carrying `text/plain` is, in practice,
//! a stringified internal error -- a connection URL with a password in it, a
//! SQL fragment, an absolute path from the build machine. That is fine in
//! development and must never reach a client in production, so a `text/plain`
//! 5xx body is replaced with a generic problem when redaction is on (the
//! default outside `debug_assertions`).
//!
//! Redaction is deliberately narrow. A 5xx the application rendered as HTML or
//! JSON is a body someone chose, and it is left alone; only the shape nothing
//! chooses on purpose is replaced. If you want the wider behaviour, say so
//! with [`ErrorMapping::redact_errors`].
//!
//! # Precedence
//!
//! 1. A custom mapper from [`ErrorMapping::with`], if it returns `Some`.
//! 2. Redaction, if the response is a `text/plain` 5xx and redaction is on.
//! 3. A problem body, if the response has no `Content-Type` at all.
//! 4. Otherwise the response is passed through untouched.
//!
//! # Where it sits in the pipeline
//!
//! Outside the body limit, the timeout, the session, CSRF and the router --
//! everything whose error responses it exists to dress -- and inside the panic
//! catcher, compression, the security headers and the access log, so a mapped
//! response is still compressed, still carries its headers, and is still
//! logged under its real status. See [`crate::application::pipeline`].

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::http::{HeaderMap, Request, Response, StatusCode, header};
use tower::{Layer, Service};

use crate::api::{Problem, ProblemKind};

/// A caller-supplied mapper: given the status of an error response and the
/// headers of the *request* that produced it, either replace the response or
/// decline.
///
/// The request headers are what content negotiation needs -- `Accept`,
/// `X-Requested-With`, `X-Inertia` -- which is the whole point of installing
/// a mapper: an HTML error page for a browser, a problem document for
/// everything else.
pub type Mapper =
    Arc<dyn Fn(StatusCode, &HeaderMap) -> Option<Response<axum::body::Body>> + Send + Sync>;

/// Turns bodiless error responses into RFC 9457 problems, and redacts
/// `text/plain` 5xx bodies.
///
/// ```
/// use arcature::http::ErrorMapping;
///
/// // The default: problem bodies everywhere, redaction in release builds.
/// let mapping = ErrorMapping::new();
///
/// // Always redact, including in development.
/// let strict = ErrorMapping::new().redact_errors(true);
/// ```
#[derive(Clone)]
pub struct ErrorMapping {
    mapper: Option<Mapper>,
    redact: bool,
}

impl Default for ErrorMapping {
    fn default() -> Self {
        Self::new()
    }
}

impl ErrorMapping {
    /// The default mapping: problem bodies for bodiless errors, and redaction
    /// of `text/plain` 5xx bodies in builds without `debug_assertions`.
    ///
    /// Keying redaction on `debug_assertions` rather than on an environment
    /// variable is deliberate: it is decided at compile time, so a production
    /// binary cannot be talked into leaking by its environment.
    #[must_use]
    pub fn new() -> Self {
        ErrorMapping {
            mapper: None,
            redact: !cfg!(debug_assertions),
        }
    }

    /// Override the redaction default.
    ///
    /// `true` redacts even in a development build -- useful for a test that
    /// asserts nothing leaks. `false` disables it entirely, which is a choice
    /// to make consciously.
    #[must_use]
    pub fn redact_errors(mut self, redact: bool) -> Self {
        self.redact = redact;
        self
    }

    /// Install a custom mapper, consulted before anything else.
    ///
    /// Returning `Some(response)` replaces the error response; returning
    /// `None` falls through to the default behaviour. This is the hook for an
    /// application that wants an HTML error page, or its own error envelope.
    ///
    /// The mapper sees the status and the *request* headers -- not the
    /// response body. Reading that body would mean buffering every error
    /// response, and a mapper that needs it is really a handler.
    ///
    /// Headers the original response carried and the client acts on --
    /// `Allow`, `Retry-After`, `WWW-Authenticate` -- are copied onto the
    /// replacement unless it set them itself.
    #[must_use]
    pub fn with<F>(mut self, mapper: F) -> Self
    where
        F: Fn(StatusCode, &HeaderMap) -> Option<Response<axum::body::Body>> + Send + Sync + 'static,
    {
        self.mapper = Some(Arc::new(mapper));
        self
    }

    /// Whether 5xx redaction is on.
    #[must_use]
    pub fn redacts(&self) -> bool {
        self.redact
    }

    /// Apply the mapping to one response, given the headers of the request
    /// that produced it.
    fn map(
        &self,
        request_headers: &HeaderMap,
        response: Response<axum::body::Body>,
    ) -> Response<axum::body::Body> {
        let status = response.status();
        if !(status.is_client_error() || status.is_server_error()) {
            return response;
        }

        if let Some(mapper) = &self.mapper
            && let Some(replacement) = mapper(status, request_headers)
        {
            return carry_headers(response, replacement);
        }

        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok());

        match content_type {
            // A `text/plain` body on a status only a layer produces is a
            // `tower-http` string, not a message anyone wrote.
            Some(ct) if ct.starts_with("text/plain") && LAYER_AUTHORED.contains(&status) => {
                rebuild(response, problem_for(status))
            }
            // A stringified internal error. The operator still gets it -- the
            // access log and the panic hook record it -- the client does not.
            Some(ct) if self.redact && status.is_server_error() && ct.starts_with("text/plain") => {
                rebuild(response, problem_for(status))
            }
            // A body someone chose: HTML error page, JSON envelope, an
            // existing problem document. Left alone.
            Some(_) => response,
            // No content type means no body: a bare `404`, `405`, `408`.
            // This is the case the layer mostly exists for.
            None => rebuild(response, problem_for(status)),
        }
    }
}

impl std::fmt::Debug for ErrorMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ErrorMapping")
            .field("custom_mapper", &self.mapper.is_some())
            .field("redact", &self.redact)
            .finish()
    }
}

/// Statuses that, inside this pipeline, come from a layer rather than from a
/// handler -- so a `text/plain` body on one of them is a library's string.
///
/// * `405` is axum's method router.
/// * `408` is the timeout stage.
/// * `413` is the body-limit stage.
///
/// A handler *can* return one of these itself, and then its body is replaced
/// by a problem document with the same status. That is a smaller loss than
/// leaving an API client with an unparseable `length limit exceeded`.
const LAYER_AUTHORED: &[StatusCode] = &[
    StatusCode::METHOD_NOT_ALLOWED,
    StatusCode::REQUEST_TIMEOUT,
    StatusCode::PAYLOAD_TOO_LARGE,
];

/// Copy onto `replacement` the headers `original` carried and it does not
/// already define. Same reasoning as [`rebuild`]: `Allow`, `Retry-After` and
/// `WWW-Authenticate` are the parts of an error a client acts on, and a
/// mapper written to produce a nicer body should not have to remember them.
fn carry_headers(
    original: Response<axum::body::Body>,
    mut replacement: Response<axum::body::Body>,
) -> Response<axum::body::Body> {
    let (parts, _body) = original.into_parts();
    for (name, value) in &parts.headers {
        if name == header::CONTENT_TYPE
            || name == header::CONTENT_LENGTH
            || replacement.headers().contains_key(name)
        {
            continue;
        }
        replacement
            .headers_mut()
            .append(name.clone(), value.clone());
    }
    replacement
}

/// The problem document for a status the application gave no body for.
fn problem_for(status: StatusCode) -> Problem {
    match ProblemKind::for_status(status) {
        Some(kind) => Problem::of(kind),
        // An unusual status (`418`, `502`, an application's own) still gets a
        // well-formed document rather than being forced into a category it
        // does not belong to.
        None => Problem::custom("about:blank", status),
    }
}

/// Replace `response`'s body with `problem`, keeping the headers the original
/// response set.
///
/// Header preservation matters more than it looks: a `405` carries `Allow`, a
/// `429` carries `Retry-After`, and a `401` carries `WWW-Authenticate`. Those
/// are the parts of the response a client acts on, and dropping them to
/// deliver a nicer body would be a bad trade.
fn rebuild(response: Response<axum::body::Body>, problem: Problem) -> Response<axum::body::Body> {
    use axum::response::IntoResponse as _;

    let (parts, _body) = response.into_parts();
    let mut replacement = problem.into_response();
    for (name, value) in &parts.headers {
        // The problem document sets its own content type and length; every
        // other header the original carried is kept.
        if name == header::CONTENT_TYPE || name == header::CONTENT_LENGTH {
            continue;
        }
        replacement
            .headers_mut()
            .append(name.clone(), value.clone());
    }
    *replacement.extensions_mut() = parts.extensions;
    replacement
}

impl<S> Layer<S> for ErrorMapping {
    type Service = ErrorMappingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ErrorMappingService {
            inner,
            mapping: self.clone(),
        }
    }
}

/// The service [`ErrorMapping`] wraps around.
#[derive(Clone, Debug)]
pub struct ErrorMappingService<S> {
    inner: S,
    mapping: ErrorMapping,
}

impl<S> Service<Request<axum::body::Body>> for ErrorMappingService<S>
where
    S: Service<
            Request<axum::body::Body>,
            Response = Response<axum::body::Body>,
            Error = Infallible,
        > + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = Response<axum::body::Body>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<axum::body::Body>) -> Self::Future {
        // The request is consumed below, so a mapper that negotiates on
        // `Accept` needs its headers copied out first -- and only when there
        // is a mapper to read them. Cloning a `HeaderMap` on every request to
        // serve the default path would be a cost paid for nothing.
        let request_headers = self
            .mapping
            .mapper
            .is_some()
            .then(|| request.headers().clone());

        // Swap in the clone and drive the original: only the original is
        // known ready, and `poll_ready` readiness does not survive cloning.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let mapping = self.mapping.clone();
        Box::pin(async move {
            let response = inner.call(request).await?;
            let request_headers = request_headers.unwrap_or_default();
            Ok(mapping.map(&request_headers, response))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::HeaderValue;

    fn bare(status: StatusCode) -> Response<Body> {
        Response::builder()
            .status(status)
            .body(Body::empty())
            .expect("response")
    }

    fn typed(status: StatusCode, content_type: &str, body: &'static str) -> Response<Body> {
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .expect("response")
    }

    fn content_type_of(response: &Response<Body>) -> Option<&str> {
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
    }

    #[test]
    fn a_success_is_never_touched() {
        let response = ErrorMapping::new().map(&HeaderMap::new(), bare(StatusCode::NO_CONTENT));
        assert_eq!(content_type_of(&response), None);
    }

    #[test]
    fn a_bare_404_gets_a_problem_body() {
        let response = ErrorMapping::new().map(&HeaderMap::new(), bare(StatusCode::NOT_FOUND));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(content_type_of(&response), Some("application/problem+json"));
    }

    #[test]
    fn a_status_with_no_distinguished_kind_still_gets_a_document() {
        let response = ErrorMapping::new().map(&HeaderMap::new(), bare(StatusCode::BAD_GATEWAY));
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(content_type_of(&response), Some("application/problem+json"));
    }

    #[test]
    fn a_body_the_application_chose_is_left_alone() {
        let response = ErrorMapping::new().redact_errors(true).map(
            &HeaderMap::new(),
            typed(StatusCode::NOT_FOUND, "text/html", "<h1>gone</h1>"),
        );
        assert_eq!(content_type_of(&response), Some("text/html"));
    }

    #[test]
    fn a_text_plain_5xx_is_redacted_when_redaction_is_on() {
        let response = ErrorMapping::new().redact_errors(true).map(
            &HeaderMap::new(),
            typed(
                StatusCode::INTERNAL_SERVER_ERROR,
                "text/plain; charset=utf-8",
                "postgres://user:hunter2@db/app: connection refused",
            ),
        );
        assert_eq!(content_type_of(&response), Some("application/problem+json"));
    }

    #[test]
    fn a_text_plain_5xx_survives_when_redaction_is_off() {
        // Development wants the message; that is the whole point of the knob.
        let response = ErrorMapping::new().redact_errors(false).map(
            &HeaderMap::new(),
            typed(StatusCode::INTERNAL_SERVER_ERROR, "text/plain", "boom"),
        );
        assert_eq!(content_type_of(&response), Some("text/plain"));
    }

    #[test]
    fn a_text_plain_4xx_is_not_redacted() {
        // A 4xx `text/plain` is a message written for the client -- redacting
        // it would delete the explanation the client needs.
        let response = ErrorMapping::new().redact_errors(true).map(
            &HeaderMap::new(),
            typed(StatusCode::BAD_REQUEST, "text/plain", "missing `id`"),
        );
        assert_eq!(content_type_of(&response), Some("text/plain"));
    }

    #[test]
    fn headers_the_client_acts_on_survive_the_rewrite() {
        let original = Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .header(header::ALLOW, "GET, HEAD")
            .body(Body::empty())
            .expect("response");
        let response = ErrorMapping::new().map(&HeaderMap::new(), original);
        assert_eq!(
            response.headers().get(header::ALLOW),
            Some(&HeaderValue::from_static("GET, HEAD"))
        );
        assert_eq!(content_type_of(&response), Some("application/problem+json"));
    }

    #[test]
    fn a_layer_authored_text_plain_error_is_replaced() {
        // `tower-http` answers an oversized body with `text/plain`. Nobody
        // wrote that string for this application's clients.
        let response = ErrorMapping::new().redact_errors(false).map(
            &HeaderMap::new(),
            typed(
                StatusCode::PAYLOAD_TOO_LARGE,
                "text/plain; charset=utf-8",
                "length limit exceeded",
            ),
        );
        assert_eq!(content_type_of(&response), Some("application/problem+json"));
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn a_mapper_reads_the_request_headers_not_the_response_ones() {
        let mapping = ErrorMapping::new().with(|status, request_headers| {
            request_headers.contains_key(header::ACCEPT).then(|| {
                Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, "text/html")
                    .body(Body::from("<h1>negotiated</h1>"))
                    .expect("response")
            })
        });

        let mut request_headers = HeaderMap::new();
        request_headers.insert(header::ACCEPT, HeaderValue::from_static("text/html"));
        assert_eq!(
            content_type_of(&mapping.map(&request_headers, bare(StatusCode::NOT_FOUND))),
            Some("text/html")
        );
        assert_eq!(
            content_type_of(&mapping.map(&HeaderMap::new(), bare(StatusCode::NOT_FOUND))),
            Some("application/problem+json")
        );
    }

    #[test]
    fn a_replacement_inherits_the_headers_it_did_not_set() {
        let mapping = ErrorMapping::new().with(|status, _headers| {
            Some(
                Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, "text/html")
                    .body(Body::from("<h1>gone</h1>"))
                    .expect("response"),
            )
        });
        let original = Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .header(header::ALLOW, "GET, HEAD")
            .body(Body::empty())
            .expect("response");

        let response = mapping.map(&HeaderMap::new(), original);
        assert_eq!(
            response.headers().get(header::ALLOW),
            Some(&HeaderValue::from_static("GET, HEAD"))
        );
        assert_eq!(content_type_of(&response), Some("text/html"));
    }

    #[test]
    fn a_custom_mapper_wins_and_can_also_decline() {
        let mapping = ErrorMapping::new().with(|status, _headers| {
            (status == StatusCode::NOT_FOUND).then(|| {
                Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, "text/html")
                    .body(Body::from("<h1>not here</h1>"))
                    .expect("response")
            })
        });

        assert_eq!(
            content_type_of(&mapping.map(&HeaderMap::new(), bare(StatusCode::NOT_FOUND))),
            Some("text/html")
        );
        // Declined: the default behaviour still applies.
        assert_eq!(
            content_type_of(&mapping.map(&HeaderMap::new(), bare(StatusCode::REQUEST_TIMEOUT))),
            Some("application/problem+json")
        );
    }

    #[test]
    fn redaction_defaults_to_off_in_a_debug_build_and_on_otherwise() {
        assert_eq!(ErrorMapping::new().redacts(), !cfg!(debug_assertions));
    }
}
