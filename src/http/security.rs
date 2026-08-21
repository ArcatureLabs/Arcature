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
//! # Per-request nonces
//!
//! [`SecurityHeaders::with_csp_nonce`] takes a policy *template* containing
//! [`CSP_NONCE_PLACEHOLDER`] and substitutes a fresh 144-bit random value into
//! it on every request, so `script-src 'nonce-{nonce}'` becomes a policy that
//! only the elements the framework stamped that same value onto can satisfy.
//!
//! The value is inserted into the request extensions on the way *down*, before
//! anything below can produce a response. That ordering is the whole feature:
//! it is what lets the Inertia renderer put the nonce on the `data-page`
//! payload script and on the tags built from the Vite manifest, and what lets
//! a handler read it back with the [`CspNonce`] extractor. A nonce in the
//! header with no matching `nonce=` attribute in the document does not harden
//! the page, it blanks it.
//!
//! # What a nonce buys, and what it does not
//!
//! A nonce constrains exactly the directive that carries it. `script-src
//! 'nonce-X'` says nothing about stylesheets, frames or `connect-src`, and a
//! policy with neither `script-src` nor `default-src` is not made stricter by
//! putting a nonce somewhere else in it.
//!
//! `'unsafe-inline'` is the detail most often stated wrong. A CSP Level 2 or
//! later browser **ignores** `'unsafe-inline'` in a directive that also carries
//! a nonce-source or a hash-source, which is why `script-src 'nonce-X'
//! 'unsafe-inline'` is the documented fallback for older browsers rather than a
//! self-cancelling policy. There are still two ways to lose with it: a browser
//! that only understands CSP Level 1 honours `'unsafe-inline'` and gets no
//! protection at all, and `'unsafe-inline'` in a directive that carries no
//! nonce -- `style-src`, usually -- is not ignored by anything, because the
//! rule is per directive.
//!
//! Without `'strict-dynamic'` a nonce does not propagate. A nonce'd script that
//! goes on to insert another `<script src="...">` produces a script that has to
//! satisfy the rest of the directive on its own, so a code-split bundle that
//! fetches further chunks at runtime needs `'self'` (or `'strict-dynamic'`) in
//! `script-src` alongside the nonce. With `'strict-dynamic'` the inserted
//! script inherits trust instead, and host and scheme allowlists in that
//! directive stop being consulted.
//!
//! Finally, a nonce is only unguessable while it is fresh. A shared cache in
//! front of the application stores the document and the header together, so the
//! two stay consistent -- but every visitor then receives a nonce that any
//! other visitor already knows, which is the one property the nonce existed
//! for. Nonce'd HTML must not be stored in a shared cache. Arcature does not
//! set `Cache-Control` on the initial document; that exclusion is the
//! deployment's to configure.
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
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::{FromRequestParts, OptionalFromRequestParts};
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::request::Parts;
use axum::http::{Request, Response, StatusCode};
use axum::response::IntoResponse;
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

/// The substring [`SecurityHeaders::with_csp_nonce`] replaces with the
/// request's nonce.
///
/// Spelled like a format placeholder because that is what it is, and because
/// no CSP token contains a brace -- a template that happens to mean something
/// on its own cannot collide with it.
pub const CSP_NONCE_PLACEHOLDER: &str = "{nonce}";

/// The number of random bytes behind a nonce.
///
/// 18 bytes is 144 bits, comfortably over the 128 the CSP specification asks
/// for, and a multiple of three, so its base64 encoding is 24 characters with
/// no padding.
const NONCE_BYTES: usize = 18;

/// A per-request Content-Security-Policy nonce.
///
/// Present in the request extensions only when the layer was built with
/// [`SecurityHeaders::with_csp_nonce`], and only for the duration of that one
/// request. Read it in a handler with the extractor (`nonce: CspNonce`, or
/// `Option<CspNonce>` when the policy is configuration-dependent), or take it
/// from a [`ScriptBody`](crate::inertia::ScriptBody) in a hand-written root
/// document.
#[derive(Clone, PartialEq, Eq)]
pub struct CspNonce(Arc<str>);

impl CspNonce {
    /// Draw a fresh nonce from the certified `getrandom` OS RNG.
    ///
    /// # Errors
    ///
    /// [`NonceUnavailable`] only if the OS RNG fails, which on a working
    /// system it does not.
    pub fn generate() -> Result<Self, NonceUnavailable> {
        let mut bytes = [0u8; NONCE_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| NonceUnavailable)?;
        Ok(CspNonce(Arc::from(base64_encode(&bytes))))
    }

    /// The base64 value, as it appears in `'nonce-...'` and in `nonce="..."`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The value as an HTML attribute *with a leading space*, ready to be
    /// interpolated straight into a tag: `format!("<script{}>", nonce.attribute())`.
    ///
    /// The leading space is part of it on purpose. The alternative -- returning
    /// the attribute bare and asking every call site to remember the separator
    /// -- produces `<scriptnonce="...">` the one time somebody forgets, and
    /// that failure is silent in the source and fatal in the browser.
    #[must_use]
    pub fn attribute(&self) -> String {
        // The value is base64 from our own RNG: no character in the alphabet
        // needs escaping inside a double-quoted attribute.
        format!(" nonce=\"{}\"", self.0)
    }
}

impl fmt::Display for CspNonce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for CspNonce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Shown, not redacted, unlike `CsrfToken`. The value is published in
        // the body of the very response it belongs to and dies with that
        // response, so a log line cannot leak anything the page did not
        // already hand the same client -- and "which nonce did this request
        // get" is the first question anyone debugging a blocked script asks.
        write!(formatter, "CspNonce({})", self.0)
    }
}

/// The OS random number generator refused to produce bytes.
///
/// Fatal for the request rather than degraded: a nonce'd policy with no nonce
/// is either an unenforced policy or a blank page, and neither is a thing to
/// serve quietly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonceUnavailable;

impl fmt::Display for NonceUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the OS random number generator could not produce a CSP nonce")
    }
}

impl std::error::Error for NonceUnavailable {}

/// A policy template that cannot be used for nonces.
///
/// Both variants are startup failures. Accepting either would produce a
/// running application that sends a `Content-Security-Policy` nobody wrote:
/// one with no nonce in it at all, or none at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CspTemplateError {
    /// The template does not contain [`CSP_NONCE_PLACEHOLDER`].
    MissingPlaceholder,
    /// The template cannot be encoded as a header value once substituted --
    /// it contains a control character or a non-ASCII byte.
    NotAHeaderValue,
}

impl fmt::Display for CspTemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CspTemplateError::MissingPlaceholder => write!(
                formatter,
                "the CSP template contains no `{CSP_NONCE_PLACEHOLDER}`, so it would be sent \
                 without a nonce; use `with_csp` for a fixed policy"
            ),
            CspTemplateError::NotAHeaderValue => formatter.write_str(
                "the CSP template cannot be encoded as a header value: it contains a control \
                 character or a non-ASCII byte",
            ),
        }
    }
}

impl std::error::Error for CspTemplateError {}

/// No nonce in the request extensions when a handler asked for one.
///
/// Means the layer was not built with [`SecurityHeaders::with_csp_nonce`], or
/// was not installed at all. A handler that is happy either way should extract
/// `Option<CspNonce>` instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CspNonceMissing;

impl fmt::Display for CspNonceMissing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "no CSP nonce on this request: build the security headers with \
             `SecurityHeaders::with_csp_nonce(..)`",
        )
    }
}

impl std::error::Error for CspNonceMissing {}

impl IntoResponse for CspNonceMissing {
    fn into_response(self) -> Response<axum::body::Body> {
        // A misconfiguration, not a bad request: the client did nothing wrong
        // and retrying will not help.
        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()).into_response()
    }
}

impl<S> FromRequestParts<S> for CspNonce
where
    S: Send + Sync,
{
    type Rejection = CspNonceMissing;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<CspNonce>()
            .cloned()
            .ok_or(CspNonceMissing)
    }
}

impl<S> OptionalFromRequestParts<S> for CspNonce
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(parts.extensions.get::<CspNonce>().cloned())
    }
}

/// The standard base64 alphabet (RFC 4648), which is the one the CSP
/// `base64-value` grammar accepts.
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode `bytes` as padded standard base64.
///
/// Local rather than a dependency: this is the only base64 in the crate and it
/// is twenty lines. Correct for every input length, not just the multiple of
/// three [`CspNonce::generate`] happens to hand it -- an encoder that is only
/// right for its current caller is a trap for the next one.
fn base64_encode(bytes: &[u8]) -> String {
    /// Index the alphabet with the low six bits of `value`.
    fn digit(value: u32) -> char {
        char::from(BASE64_ALPHABET[(value & 0b0011_1111) as usize])
    }

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut triple = u32::from(chunk[0]) << 16;
        triple |= u32::from(chunk.get(1).copied().unwrap_or(0)) << 8;
        triple |= u32::from(chunk.get(2).copied().unwrap_or(0));
        out.push(digit(triple >> 18));
        out.push(digit(triple >> 12));
        out.push(if chunk.len() > 1 {
            digit(triple >> 6)
        } else {
            '='
        });
        out.push(if chunk.len() > 2 { digit(triple) } else { '=' });
    }
    out
}

/// Response headers applied to everything the application returns.
///
/// Built with [`SecurityHeaders::new`]; `Strict-Transport-Security` and
/// `Content-Security-Policy` are added with [`with_hsts`](Self::with_hsts) and
/// either [`with_csp`](Self::with_csp) or
/// [`with_csp_nonce`](Self::with_csp_nonce).
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
    csp_template: Option<Arc<str>>,
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
            csp_template: None,
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
    ///
    /// Mutually exclusive with [`with_csp_nonce`](Self::with_csp_nonce): the
    /// last of the two called wins, because sending two policies would mean
    /// enforcing the intersection of a fixed one and a generated one, which is
    /// never what somebody who called both meant.
    #[must_use]
    pub fn with_csp(mut self, policy: impl AsRef<str>) -> Self {
        self.csp = HeaderValue::from_str(policy.as_ref()).ok();
        self.csp_template = None;
        self
    }

    /// Send `Content-Security-Policy` built per request from `template`, with
    /// every [`CSP_NONCE_PLACEHOLDER`] replaced by that request's
    /// [`CspNonce`].
    ///
    /// ```
    /// use arcature::http::security::SecurityHeaders;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let headers = SecurityHeaders::new()
    ///     .with_csp_nonce("default-src 'self'; script-src 'self' 'nonce-{nonce}'")?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The framework stamps the same value onto every script and stylesheet
    /// element it emits itself -- the Inertia `data-page` payload and the tags
    /// resolved from the Vite manifest. Anything the application writes into a
    /// hand-written root document is its own to stamp; see
    /// [`ScriptBody::nonce`](crate::inertia::ScriptBody::nonce).
    ///
    /// Mutually exclusive with [`with_csp`](Self::with_csp); the last of the
    /// two called wins.
    ///
    /// # Errors
    ///
    /// [`CspTemplateError::MissingPlaceholder`] if `template` contains no
    /// placeholder -- silently sending a nonce-less policy from a method whose
    /// name promises a nonce is the one outcome worth refusing to compile
    /// around. [`CspTemplateError::NotAHeaderValue`] if the substituted policy
    /// could not be a header value; unlike [`with_csp`](Self::with_csp) this is
    /// reported rather than dropped, because there is no per-request moment
    /// left at which dropping it could be noticed.
    pub fn with_csp_nonce(mut self, template: impl AsRef<str>) -> Result<Self, CspTemplateError> {
        let template = template.as_ref();
        if !template.contains(CSP_NONCE_PLACEHOLDER) {
            return Err(CspTemplateError::MissingPlaceholder);
        }
        // Validate against a substituted sample rather than the raw template:
        // it is the substituted string that becomes a header, and the
        // placeholder's own braces are not header-legal characters to check.
        let sample = template.replace(
            CSP_NONCE_PLACEHOLDER,
            &"A".repeat(NONCE_BYTES.div_ceil(3) * 4),
        );
        HeaderValue::from_str(&sample).map_err(|_| CspTemplateError::NotAHeaderValue)?;
        self.csp = None;
        self.csp_template = Some(Arc::from(template));
        Ok(self)
    }

    /// Whether this configuration needs a fresh nonce on every request.
    #[must_use]
    pub fn generates_nonce(&self) -> bool {
        self.csp_template.is_some()
    }

    /// The `Content-Security-Policy` value for one request.
    ///
    /// `None` when no policy is configured, and also when a substituted
    /// template somehow fails to encode -- which construction already ruled
    /// out, the nonce alphabet being header-safe by definition.
    fn csp_value(&self, nonce: Option<&CspNonce>) -> Option<HeaderValue> {
        match (&self.csp_template, nonce) {
            (Some(template), Some(nonce)) => {
                HeaderValue::from_str(&template.replace(CSP_NONCE_PLACEHOLDER, nonce.as_str())).ok()
            }
            _ => self.csp.clone(),
        }
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

    fn call(&mut self, mut request: Request<axum::body::Body>) -> Self::Future {
        // Swap in the clone and drive the original: only the original is
        // known ready, and `poll_ready` readiness does not survive cloning.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let config = self.config.clone();

        // Generated here, before the inner call, because everything that needs
        // it -- the Inertia renderer, the asset tags, a handler -- runs below.
        // A nonce minted on the way back up would arrive after the document
        // that had to carry it.
        let nonce = if config.generates_nonce() {
            match CspNonce::generate() {
                Ok(nonce) => Some(nonce),
                Err(unavailable) => {
                    // No usable outcome remains: without a nonce the policy is
                    // either unenforced or blanks the page. Refuse the request
                    // instead of picking one silently.
                    return Box::pin(async move {
                        Ok((StatusCode::INTERNAL_SERVER_ERROR, unavailable.to_string())
                            .into_response())
                    });
                }
            }
        } else {
            None
        };
        if let Some(nonce) = &nonce {
            request.extensions_mut().insert(nonce.clone());
        }

        Box::pin(async move {
            let mut response = inner.call(request).await?;
            apply(response.headers_mut(), &config, nonce.as_ref());
            Ok(response)
        })
    }
}

/// Insert each configured header that is not already present.
fn apply(headers: &mut axum::http::HeaderMap, config: &SecurityHeaders, nonce: Option<&CspNonce>) {
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
    if let Some(policy) = config.csp_value(nonce) {
        set(headers, axum::http::header::CONTENT_SECURITY_POLICY, policy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn headers_from(config: &SecurityHeaders) -> HeaderMap {
        let mut headers = HeaderMap::new();
        apply(&mut headers, config, None);
        headers
    }

    fn value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
        headers.get(name).map(|v| v.to_str().expect("ascii"))
    }

    /// An inner service that reports the nonce it was handed and, optionally,
    /// sets a policy of its own -- the two things a test needs to see from
    /// below the layer.
    #[derive(Clone)]
    struct Echo {
        own_csp: Option<&'static str>,
    }

    impl Service<Request<axum::body::Body>> for Echo {
        type Response = Response<axum::body::Body>;
        type Error = Infallible;
        type Future =
            Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, request: Request<axum::body::Body>) -> Self::Future {
            let seen = request.extensions().get::<CspNonce>().cloned();
            let own_csp = self.own_csp;
            Box::pin(async move {
                let mut response = Response::new(axum::body::Body::empty());
                if let Some(nonce) = seen {
                    response.headers_mut().insert(
                        "x-seen-nonce",
                        HeaderValue::from_str(nonce.as_str()).expect("base64 is header-safe"),
                    );
                }
                if let Some(policy) = own_csp {
                    response.headers_mut().insert(
                        axum::http::header::CONTENT_SECURITY_POLICY,
                        HeaderValue::from_static(policy),
                    );
                }
                Ok(response)
            })
        }
    }

    async fn run(config: &SecurityHeaders, own_csp: Option<&'static str>) -> HeaderMap {
        let mut service = config.layer(Echo { own_csp });
        let response = service
            .call(Request::new(axum::body::Body::empty()))
            .await
            .expect("infallible");
        response.headers().clone()
    }

    fn nonce_policy() -> SecurityHeaders {
        SecurityHeaders::new()
            .with_csp_nonce("default-src 'self'; script-src 'self' 'nonce-{nonce}'")
            .expect("the template carries the placeholder")
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
        apply(&mut headers, &SecurityHeaders::new(), None);
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

    #[test]
    fn a_template_without_the_placeholder_is_refused() {
        // Accepting it would send a nonce-less policy from a method whose name
        // promises a nonce, and nothing downstream would ever notice.
        let error = SecurityHeaders::new()
            .with_csp_nonce("script-src 'self'")
            .expect_err("no placeholder");
        assert_eq!(error, CspTemplateError::MissingPlaceholder);
    }

    #[test]
    fn a_template_that_cannot_be_a_header_value_is_refused_not_dropped() {
        let error = SecurityHeaders::new()
            .with_csp_nonce("script-src 'nonce-{nonce}'\n\rinjected")
            .expect_err("not a header value");
        assert_eq!(error, CspTemplateError::NotAHeaderValue);
    }

    #[test]
    fn a_fixed_policy_and_a_nonce_template_replace_each_other() {
        let fixed_last = nonce_policy().with_csp("default-src 'none'");
        assert!(!fixed_last.generates_nonce());
        assert_eq!(
            value(&headers_from(&fixed_last), "content-security-policy"),
            Some("default-src 'none'")
        );

        let nonce_last = SecurityHeaders::new()
            .with_csp("default-src 'none'")
            .with_csp_nonce("script-src 'nonce-{nonce}'")
            .expect("template");
        assert!(nonce_last.generates_nonce());
        // Without a nonce there is nothing to substitute, and the replaced
        // fixed policy is gone rather than lingering as a fallback.
        assert_eq!(
            value(&headers_from(&nonce_last), "content-security-policy"),
            None
        );
    }

    #[tokio::test]
    async fn two_requests_get_two_different_nonces() {
        let config = nonce_policy();
        let first = run(&config, None).await;
        let second = run(&config, None).await;
        let first = value(&first, "x-seen-nonce").expect("nonce reached the inner service");
        let second = value(&second, "x-seen-nonce").expect("nonce reached the inner service");
        assert_ne!(first, second, "a reused nonce is a guessable nonce");
    }

    #[tokio::test]
    async fn the_header_carries_the_same_nonce_the_request_extension_did() {
        // The whole feature: a header naming a nonce no element carries does
        // not harden the page, it blanks it.
        let headers = run(&nonce_policy(), None).await;
        let seen = value(&headers, "x-seen-nonce").expect("nonce in extensions");
        assert_eq!(
            value(&headers, "content-security-policy"),
            Some(format!("default-src 'self'; script-src 'self' 'nonce-{seen}'").as_str())
        );
    }

    #[tokio::test]
    async fn a_response_that_set_its_own_csp_keeps_it() {
        let headers = run(&nonce_policy(), Some("default-src 'none'")).await;
        assert_eq!(
            value(&headers, "content-security-policy"),
            Some("default-src 'none'")
        );
        // The nonce was still minted and still reached the request, so a
        // handler that opted out of the header did not lose the value.
        assert!(value(&headers, "x-seen-nonce").is_some());
    }

    #[tokio::test]
    async fn no_nonce_is_minted_when_none_was_asked_for() {
        let headers = run(&SecurityHeaders::new().with_csp("default-src 'self'"), None).await;
        assert_eq!(value(&headers, "x-seen-nonce"), None);
    }

    #[test]
    fn a_nonce_is_base64_and_long_enough_to_be_unguessable() {
        let nonce = CspNonce::generate().expect("OS RNG");
        assert_eq!(nonce.as_str().len(), 24, "18 random bytes, unpadded");
        assert!(
            nonce.as_str().bytes().all(|b| BASE64_ALPHABET.contains(&b)),
            "a character outside the base64 alphabet would not survive the CSP grammar"
        );
        assert_eq!(nonce.attribute(), format!(" nonce=\"{nonce}\""));
    }

    #[test]
    fn base64_matches_the_rfc_4648_vectors() {
        // Every input length, not just the multiple of three the nonce uses:
        // an encoder that is only right for its current caller is a trap.
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(input.as_bytes()), expected, "input {input:?}");
        }
        assert_eq!(base64_encode(&[0xff, 0xef, 0xfe]), "/+/+");
    }
}
