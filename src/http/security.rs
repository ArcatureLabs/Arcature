//! Response security headers.
//!
//! # What this is for
//!
//! Four of the five headers here close a browser behaviour that is unsafe by
//! default and cannot be closed from application code: MIME sniffing, framing,
//! referrer leakage, and downgrade to plain HTTP. They are cheap, they apply to
//! every response, and leaving them off is a decision nobody makes on purpose.
//!
//! The fifth, `Content-Security-Policy`, is different in kind: a policy tight
//! enough to be worth having breaks pages that were not written for it, so it
//! is **off unless asked for** ([`SecurityHeaders::with_csp`]). A CSP that gets
//! disabled the first time a page breaks is worse than no CSP, because it reads
//! as protection that is not there.
//!
//! # What it does not do
//!
//! Headers are not a substitute for correct output escaping, correct cookie
//! attributes, or TLS. `Strict-Transport-Security` in particular is only
//! meaningful once the site is actually served over HTTPS -- which is why it is
//! opt-in through [`SecurityHeaders::with_hsts`] rather than on by default: a
//! development server that sends it pins `localhost` to HTTPS in the developer's
//! browser, and the browser will keep honouring that long after the header
//! stops being sent.
//!
//! # Existing headers win
//!
//! Every header is inserted only if the response does not already carry it. A
//! handler that deliberately sets `X-Frame-Options: ALLOWALL` on one embeddable
//! route keeps it; the layer supplies a floor, not a ceiling.

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::http::header::{HeaderName, HeaderValue};
use axum::http::{Request, Response};
use tower::{Layer, Service};

/// The `X-Content-Type-Options` value: never sniff, always trust the declared
/// `Content-Type`.
pub const NO_SNIFF: &str = "nosniff";

/// The `X-Frame-Options` value: no framing at all, by anyone.
pub const DENY_FRAMING: &str = "DENY";

/// The `Referrer-Policy` value: full URL to same-origin, bare origin
/// cross-origin, nothing at all on a downgrade to HTTP.
pub const REFERRER_POLICY: &str = "strict-origin-when-cross-origin";

/// A one-year `Strict-Transport-Security` covering subdomains.
///
/// Deliberately without `preload`: submission to the browser preload list is
/// close to irreversible, and it is not a framework's decision to make on an
/// application's domain.
pub const HSTS_ONE_YEAR: &str = "max-age=31536000; includeSubDomains";

/// Response headers applied to everything the application returns.
///
/// Built with [`SecurityHeaders::new`]; `Strict-Transport-Security` and
/// `Content-Security-Policy` are added with [`with_hsts`](Self::with_hsts) and
/// [`with_csp`](Self::with_csp).
///
/// ```
/// use arcature::http::security::SecurityHeaders;
///
/// let headers = SecurityHeaders::new()
///     .with_hsts()
///     .with_csp("default-src 'self'");
/// ```
#[derive(Clone, Debug)]
pub struct SecurityHeaders {
    hsts: bool,
    csp: Option<HeaderValue>,
}

impl SecurityHeaders {
    /// The three headers that are safe on any application, HTTP or HTTPS:
    /// `nosniff`, `DENY` framing, and a strict referrer policy.
    ///
    /// HSTS and CSP are not included -- see the module documentation for why
    /// each is opt-in.
    #[must_use]
    pub fn new() -> Self {
        SecurityHeaders {
            hsts: false,
            csp: None,
        }
    }

    /// Send `Strict-Transport-Security` ([`HSTS_ONE_YEAR`]).
    ///
    /// Only for a deployment actually reachable over HTTPS. On a plain-HTTP
    /// development server this pins the host to HTTPS in the developer's
    /// browser for a year, and the pin outlives the header.
    #[must_use]
    pub fn with_hsts(mut self) -> Self {
        self.hsts = true;
        self
    }

    /// Send `Content-Security-Policy` with `policy`.
    ///
    /// The policy is taken verbatim; this type does not build one, because a
    /// useful policy depends on what the application loads. A policy that
    /// cannot be encoded as a header value is dropped rather than sent
    /// mangled.
    #[must_use]
    pub fn with_csp(mut self, policy: impl AsRef<str>) -> Self {
        self.csp = HeaderValue::from_str(policy.as_ref()).ok();
        self
    }
}

impl Default for SecurityHeaders {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for SecurityHeaders {
    type Service = SecurityHeadersService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        SecurityHeadersService {
            inner,
            config: self.clone(),
        }
    }
}

/// The service [`SecurityHeaders`] wraps around.
#[derive(Clone, Debug)]
pub struct SecurityHeadersService<S> {
    inner: S,
    config: SecurityHeaders,
}

impl<S> Service<Request<axum::body::Body>> for SecurityHeadersService<S>
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
        // Swap in the clone and drive the original: only the original is
        // known ready, and `poll_ready` readiness does not survive cloning.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let config = self.config.clone();
        Box::pin(async move {
            let mut response = inner.call(request).await?;
            apply(response.headers_mut(), &config);
            Ok(response)
        })
    }
}

/// Insert each configured header that is not already present.
fn apply(headers: &mut axum::http::HeaderMap, config: &SecurityHeaders) {
    /// Insert `value` under `name` unless the response already set it.
    fn set(headers: &mut axum::http::HeaderMap, name: HeaderName, value: HeaderValue) {
        if !headers.contains_key(&name) {
            headers.insert(name, value);
        }
    }

    set(
        headers,
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static(NO_SNIFF),
    );
    set(
        headers,
        axum::http::header::X_FRAME_OPTIONS,
        HeaderValue::from_static(DENY_FRAMING),
    );
    set(
        headers,
        axum::http::header::REFERRER_POLICY,
        HeaderValue::from_static(REFERRER_POLICY),
    );
    if config.hsts {
        set(
            headers,
            axum::http::header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(HSTS_ONE_YEAR),
        );
    }
    if let Some(policy) = &config.csp {
        set(
            headers,
            axum::http::header::CONTENT_SECURITY_POLICY,
            policy.clone(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn headers_from(config: &SecurityHeaders) -> HeaderMap {
        let mut headers = HeaderMap::new();
        apply(&mut headers, config);
        headers
    }

    fn value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
        headers.get(name).map(|v| v.to_str().expect("ascii"))
    }

    #[test]
    fn the_always_on_headers_are_present() {
        let headers = headers_from(&SecurityHeaders::new());
        assert_eq!(value(&headers, "x-content-type-options"), Some("nosniff"));
        assert_eq!(value(&headers, "x-frame-options"), Some("DENY"));
        assert_eq!(
            value(&headers, "referrer-policy"),
            Some("strict-origin-when-cross-origin")
        );
    }

    #[test]
    fn hsts_and_csp_are_absent_unless_asked_for() {
        // A development server that sends HSTS pins localhost to HTTPS in the
        // browser for a year, and the pin outlives the header.
        let headers = headers_from(&SecurityHeaders::new());
        assert_eq!(value(&headers, "strict-transport-security"), None);
        assert_eq!(value(&headers, "content-security-policy"), None);
    }

    #[test]
    fn hsts_and_csp_appear_once_asked_for() {
        let headers = headers_from(
            &SecurityHeaders::new()
                .with_hsts()
                .with_csp("default-src 'self'"),
        );
        assert_eq!(
            value(&headers, "strict-transport-security"),
            Some("max-age=31536000; includeSubDomains")
        );
        assert_eq!(
            value(&headers, "content-security-policy"),
            Some("default-src 'self'")
        );
    }

    #[test]
    fn hsts_is_not_submitted_for_preloading() {
        // `preload` is close to irreversible and is the site owner's call.
        assert!(!HSTS_ONE_YEAR.contains("preload"));
    }

    #[test]
    fn a_header_the_handler_already_set_is_left_alone() {
        // The layer is a floor, not a ceiling: one embeddable route may need
        // its own framing policy.
        let mut headers = HeaderMap::new();
        headers.insert("x-frame-options", HeaderValue::from_static("SAMEORIGIN"));
        apply(&mut headers, &SecurityHeaders::new());
        assert_eq!(value(&headers, "x-frame-options"), Some("SAMEORIGIN"));
        // ...and the rest are still applied.
        assert_eq!(value(&headers, "x-content-type-options"), Some("nosniff"));
    }

    #[test]
    fn a_policy_that_cannot_be_a_header_value_is_dropped_not_mangled() {
        let headers =
            headers_from(&SecurityHeaders::new().with_csp("default-src 'self'\n\rinjected"));
        assert_eq!(value(&headers, "content-security-policy"), None);
    }
}
