//! Double-submit CSRF protection for cookie-authenticated browser requests.
//!
//! [`CsrfLayer`] is a Tower layer that enforces a **naive double-submit** CSRF
//! defense. It:
//!
//! - Exempts safe methods (`GET`, `HEAD`, `OPTIONS`, `TRACE`).
//! - Exempts **bearer-token API** requests: an unsafe request carrying an
//!   `Authorization: Bearer ...` header is forwarded to the inner service
//!   without the double-submit check and without a CSRF cookie.
//! - On safe, non-bearer methods: injects a `Set-Cookie` with a fresh CSRF
//!   token if the request did not carry one.
//! - On unsafe, non-bearer methods (`POST`, `PUT`, `PATCH`, `DELETE`): reads
//!   the CSRF cookie and the matching header, and rejects the request (`403`)
//!   if they are missing or do not match.
//!
//! # Mechanism
//!
//! This is the **naive** double-submit pattern (not a signed or session-bound
//! token): the server issues a random nonce in a `__Host-csrf` cookie, the
//! client echoes it back in a header, and the server compares the two. The
//! strength comes from the cookie attributes, not from a signature:
//!
//! - `__Host-csrf` prefix -> mandates `Secure`, no `Domain`, path `/`
//!   (RFC 6265bis): a sibling subdomain cannot overwrite the cookie.
//! - `SameSite=Strict` -> not sent on cross-site requests.
//! - `HttpOnly=false` -> JavaScript must read the cookie to send it in the
//!   header (the header is the proof the page is same-origin).
//!
//! It defends against **forged cross-site unsafe requests from an
//! authenticated browser** (classic CSRF). It does **not** defend against XSS
//! (same-origin script can read and send the token), and it is **not** a
//! substitute for the reverse-proxy front door, which owns TLS termination,
//! rate limiting, and request-size limits. No Arcature-written cryptography.

use std::convert::Infallible;
use std::fmt;

use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use tower::Layer;
use tower::Service;
use tower_sessions::cookie::{Cookie, SameSite};

use crate::auth::{CsrfConfigError, CsrfError};

/// Resolved CSRF protection configuration.
///
/// Construct with [`CsrfConfig::new`] for the production double-submit token
/// (`__Host-csrf`, `Secure = true`) or [`CsrfConfig::dev`] for development over
/// plain HTTP (`arcature-csrf`, `Secure = false`). Override the cookie/header
/// field names with the `with_*` builders.
#[derive(Clone)]
pub struct CsrfConfig {
    cookie_name: String,
    header_name: String,
    secure: bool,
}

impl CsrfConfig {
    /// Build CSRF configuration with the **production** defaults: cookie
    /// `__Host-csrf`, header `x-csrf-token`, `Secure = true`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cookie_name: "__Host-csrf".to_string(),
            header_name: "x-csrf-token".to_string(),
            secure: true,
        }
    }

    /// Build CSRF configuration with the **development** defaults: cookie
    /// `arcature-csrf` (no `__Host-` prefix), header `x-csrf-token`,
    /// `Secure = false`.
    #[must_use]
    pub fn dev() -> Self {
        Self {
            cookie_name: "arcature-csrf".to_string(),
            header_name: "x-csrf-token".to_string(),
            secure: false,
        }
    }

    /// Override the CSRF cookie name. A `__Host-` prefix is recommended; it
    /// **mandates** `Secure = true` (RFC 6265bis), so setting a `__Host-` name
    /// auto-enables `Secure` -- the invalid `__Host-` + `Secure = false`
    /// combination is impossible after this call.
    #[must_use]
    pub fn with_cookie_name(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        if name.starts_with("__Host-") {
            self.secure = true;
        }
        self.cookie_name = name;
        self
    }

    /// Override the CSRF header field name. The default is `x-csrf-token`.
    #[must_use]
    pub fn with_header_name(mut self, name: impl Into<String>) -> Self {
        self.header_name = name.into();
        self
    }

    /// Override the `Secure` attribute on the CSRF cookie.
    ///
    /// # Errors
    ///
    /// Returns `Err` when `secure = false` and the current cookie name carries
    /// the `__Host-` prefix -- a `__Host-` cookie is `Secure` by mandate.
    pub fn with_secure(self, secure: bool) -> Result<Self, CsrfConfigError> {
        if !secure && self.cookie_name.starts_with("__Host-") {
            return Err(CsrfConfigError::InsecureHostPrefixedCookie {
                cookie_name: self.cookie_name,
            });
        }
        Ok(Self { secure, ..self })
    }

    /// The CSRF cookie name.
    #[must_use]
    pub fn cookie_name(&self) -> &str {
        &self.cookie_name
    }

    /// The CSRF header field name.
    #[must_use]
    pub fn header_name(&self) -> &str {
        &self.header_name
    }

    /// The `Secure` attribute the injected CSRF cookie carries.
    #[must_use]
    pub fn secure(&self) -> bool {
        self.secure
    }
}

impl Default for CsrfConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for CsrfConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CsrfConfig")
            .field("cookie_name", &self.cookie_name)
            .field("header_name", &self.header_name)
            .field("secure", &self.secure)
            .finish()
    }
}

/// A 32-byte double-submit CSRF token, hex-encoded for transport.
///
/// The token is not secret -- it is a random nonce shared between the cookie
/// and the header. The protection comes from the browser's SameSite cookie
/// policy plus the attacker's inability to read the cookie value cross-origin.
#[derive(Clone, PartialEq, Eq)]
pub struct CsrfToken(String);

/// The number of random bytes in a CSRF token.
const TOKEN_BYTES: usize = 32;

impl CsrfToken {
    /// Generate a fresh random CSRF token from the certified `getrandom` OS
    /// RNG.
    ///
    /// # Errors
    ///
    /// Returns [`CsrfError::MalformedCookie`] only if the OS RNG fails.
    pub fn generate() -> Result<Self, CsrfError> {
        let mut bytes = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| CsrfError::MalformedCookie)?;
        Ok(Self(hex_encode(&bytes)))
    }

    /// Parse a token from a cookie or header string.
    ///
    /// # Errors
    ///
    /// Returns [`CsrfError::MalformedCookie`] if the value is not 64 hex chars.
    pub fn parse(value: &str) -> Result<Self, CsrfError> {
        if value.len() != TOKEN_BYTES * 2 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CsrfError::MalformedCookie);
        }
        Ok(Self(value.to_string()))
    }

    /// The hex-encoded token string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CsrfToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The token is not secret (it is a public nonce), but redact it in
        // Debug to avoid leaking it into logs where it could be correlated.
        write!(formatter, "CsrfToken(<{} hex chars>)", self.0.len())
    }
}

impl fmt::Display for CsrfToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Display exposes the token -- it is needed to set the cookie/header
        // value. This is intentional, not a leak.
        write!(formatter, "{}", self.0)
    }
}

/// Hex-encode a byte slice to a lowercase hex string.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// A Tower layer that installs double-submit CSRF protection on a router.
///
/// Construct with [`CsrfLayer::new`] (defaults) or
/// [`CsrfLayer::with_config`], then apply on a router with `.layer(...)`.
#[derive(Clone)]
pub struct CsrfLayer {
    config: CsrfConfig,
}

impl CsrfLayer {
    /// Build a CSRF layer with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: CsrfConfig::new(),
        }
    }

    /// Build a CSRF layer with custom configuration.
    #[must_use]
    pub fn with_config(config: CsrfConfig) -> Self {
        Self { config }
    }
}

impl Default for CsrfLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for CsrfLayer {
    type Service = CsrfMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CsrfMiddleware {
            inner,
            config: self.config.clone(),
        }
    }
}

/// The service produced by [`CsrfLayer`].
#[derive(Clone)]
pub struct CsrfMiddleware<S> {
    inner: S,
    config: CsrfConfig,
}

impl<S, ReqBody> Service<Request<ReqBody>> for CsrfMiddleware<S>
where
    S: Service<Request<ReqBody>, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
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

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let config = self.config.clone();
        let method = req.method().clone();
        let headers = req.headers().clone();
        let cookie_token = extract_csrf_cookie(&headers, config.cookie_name());

        // Bearer-token APIs are exempt from double-submit CSRF.
        let bearer = is_bearer_request(&headers);

        // Compute inject_cookie before the unsafe-method check borrows
        // cookie_token. On safe, non-bearer methods we inject a fresh cookie if
        // none was present; on unsafe methods we consume cookie_token in the
        // check. Bearer requests are forwarded untouched.
        let inject_cookie = cookie_token.is_none() && !bearer;
        let safe = is_safe_method(&method);

        if !safe && !bearer {
            let cookie_token = match cookie_token {
                Some(token) => token,
                None => {
                    drop(req);
                    return Box::pin(async move { Ok(csrf_rejection(CsrfError::MissingCookie)) });
                }
            };
            let header_token = match extract_csrf_header(&headers, config.header_name()) {
                Some(token) => token,
                None => {
                    drop(req);
                    return Box::pin(async move { Ok(csrf_rejection(CsrfError::MissingHeader)) });
                }
            };
            if cookie_token != header_token {
                drop(req);
                return Box::pin(async move { Ok(csrf_rejection(CsrfError::TokenMismatch)) });
            }
        }

        let mut inner = self.inner.clone();
        Box::pin(async move {
            let resp = inner.call(req).await?;
            if inject_cookie {
                Ok(inject_csrf_cookie(resp, &config))
            } else {
                Ok(resp)
            }
        })
    }
}

/// Whether the request carries a bearer-token `Authorization` header.
fn is_bearer_request(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(axum::http::header::AUTHORIZATION) else {
        return false;
    };
    let bytes = value.as_bytes();
    let scheme = match bytes.iter().position(u8::is_ascii_whitespace) {
        Some(end) => &bytes[..end],
        None => bytes,
    };
    scheme.eq_ignore_ascii_case(b"bearer")
}

/// Extract the CSRF cookie value from the `Cookie` header, if present.
fn extract_csrf_cookie(headers: &HeaderMap, cookie_name: &str) -> Option<CsrfToken> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for cookie_str in raw.split(';') {
        let trimmed = cookie_str.trim();
        if trimmed.is_empty() {
            continue;
        }
        let cookie = Cookie::parse_encoded(trimmed.to_string()).ok()?;
        if cookie.name() == cookie_name {
            return CsrfToken::parse(cookie.value()).ok();
        }
    }
    None
}

/// Extract the CSRF header value, if present and valid.
fn extract_csrf_header(headers: &HeaderMap, header_name: &str) -> Option<CsrfToken> {
    let raw = headers.get(header_name)?.to_str().ok()?;
    CsrfToken::parse(raw).ok()
}

/// Inject a fresh CSRF cookie into the response.
fn inject_csrf_cookie(mut response: Response, config: &CsrfConfig) -> Response {
    let token = match CsrfToken::generate() {
        Ok(token) => token,
        Err(_) => return response,
    };
    let cookie = Cookie::build((config.cookie_name().to_string(), token.as_str().to_string()))
        .same_site(SameSite::Strict)
        .secure(config.secure())
        .http_only(false)
        .path("/")
        .build();
    match HeaderValue::from_str(&cookie.encoded().to_string()) {
        Ok(value) => {
            response
                .headers_mut()
                .append(axum::http::header::SET_COOKIE, value);
        }
        Err(_) => { /* cookie encoding should not fail for hex values */ }
    }
    response
}

/// Return a 403 Forbidden response for a CSRF rejection.
fn csrf_rejection(error: CsrfError) -> Response {
    (StatusCode::FORBIDDEN, error.to_string()).into_response()
}

/// Whether an HTTP method is safe (does not require CSRF protection).
fn is_safe_method(method: &Method) -> bool {
    method == Method::GET
        || method == Method::HEAD
        || method == Method::OPTIONS
        || method == Method::TRACE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_methods_recognized() {
        assert!(is_safe_method(&Method::GET));
        assert!(is_safe_method(&Method::HEAD));
        assert!(is_safe_method(&Method::OPTIONS));
        assert!(is_safe_method(&Method::TRACE));
        assert!(!is_safe_method(&Method::POST));
        assert!(!is_safe_method(&Method::PUT));
        assert!(!is_safe_method(&Method::DELETE));
        assert!(!is_safe_method(&Method::PATCH));
    }

    #[test]
    fn bearer_request_is_recognized() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer mF_9.B5f-4.1JqM"),
        );
        assert!(is_bearer_request(&headers), "Bearer <token> is exempt");

        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("bearer lowercase"),
        );
        assert!(is_bearer_request(&headers), "lowercase scheme is exempt");
    }

    #[test]
    fn non_bearer_request_is_not_exempt() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic dXNlcjpwYXNz"),
        );
        assert!(!is_bearer_request(&headers), "Basic is not exempt");
    }

    #[test]
    fn absent_authorization_is_not_exempt() {
        let headers = HeaderMap::new();
        assert!(!is_bearer_request(&headers));
    }

    #[test]
    fn production_defaults_are_expected() {
        let config = CsrfConfig::new();
        assert_eq!(config.cookie_name(), "__Host-csrf");
        assert_eq!(config.header_name(), "x-csrf-token");
        assert!(config.secure());
    }

    #[test]
    fn dev_defaults_are_expected() {
        let config = CsrfConfig::dev();
        assert_eq!(config.cookie_name(), "arcature-csrf");
        assert_eq!(config.header_name(), "x-csrf-token");
        assert!(!config.secure());
    }

    #[test]
    fn with_secure_false_rejects_host_prefixed_cookie() {
        let result = CsrfConfig::new().with_secure(false);
        assert!(matches!(
            result,
            Err(CsrfConfigError::InsecureHostPrefixedCookie { .. })
        ));
    }

    #[test]
    fn with_cookie_name_host_prefix_auto_enables_secure() {
        let config = CsrfConfig::new()
            .with_cookie_name("arcature-csrf")
            .with_secure(false)
            .expect("dev config")
            .with_cookie_name("__Host-csrf");
        assert_eq!(config.cookie_name(), "__Host-csrf");
        assert!(config.secure(), "setting a __Host- name must auto-enable Secure");
    }

    #[test]
    fn token_generate_produces_64_hex_chars() {
        let token = CsrfToken::generate().expect("rng");
        assert_eq!(token.as_str().len(), 64);
        assert!(token.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn two_tokens_differ() {
        let t1 = CsrfToken::generate().expect("rng");
        let t2 = CsrfToken::generate().expect("rng");
        assert_ne!(t1.as_str(), t2.as_str());
    }

    #[test]
    fn token_parse_validates_length_and_hex() {
        let token = CsrfToken::generate().expect("rng");
        let parsed = CsrfToken::parse(token.as_str()).expect("valid");
        assert_eq!(parsed.as_str(), token.as_str());
    }

    #[test]
    fn token_parse_rejects_wrong_length() {
        assert!(CsrfToken::parse("abc").is_err());
        assert!(CsrfToken::parse(&"a".repeat(63)).is_err());
        assert!(CsrfToken::parse(&"z".repeat(64)).is_err());
    }

    #[test]
    fn token_debug_redacts() {
        let token = CsrfToken::generate().expect("rng");
        let debug = format!("{token:?}");
        assert!(!debug.contains(token.as_str()));
    }

    #[test]
    fn extract_header_finds_valid_token() {
        let token = CsrfToken::generate().expect("rng");
        let mut headers = HeaderMap::new();
        headers.insert("x-csrf-token", HeaderValue::from_str(token.as_str()).unwrap());
        let extracted = extract_csrf_header(&headers, "x-csrf-token");
        assert_eq!(extracted.as_ref().map(|t| t.as_str()), Some(token.as_str()));
    }

    #[test]
    fn extract_cookie_finds_token() {
        let token = CsrfToken::generate().expect("rng");
        let mut headers = HeaderMap::new();
        let cookie_value = format!("__Host-csrf={}", token.as_str());
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&cookie_value).unwrap(),
        );
        let extracted = extract_csrf_cookie(&headers, "__Host-csrf");
        assert_eq!(extracted.as_ref().map(|t| t.as_str()), Some(token.as_str()));
    }
}
