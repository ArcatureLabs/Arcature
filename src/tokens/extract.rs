//! The `Authorization: Bearer` extractor.
//!
//! One extractor, [`ApiAuth`], which turns a bearer credential on the request
//! into the [`ApiToken`] it names, or rejects the request. Abilities are
//! checked on the extracted token with [`ApiToken::can`]; there is no separate
//! ability extractor, because the check a route needs is a line of Rust and a
//! second extractor would only be a less readable way to write it.

use axum::Extension;
use axum::extract::FromRequestParts;
use axum::http::{StatusCode, header, request::Parts};
use axum::response::{IntoResponse, Response};

use super::store::ApiTokens;
use super::token::ApiToken;

/// The scheme name, compared case-insensitively as RFC 7235 requires.
const SCHEME: &str = "bearer";

/// An authenticated API token, extracted from `Authorization: Bearer`.
///
/// The route runs only if the header carried a live token this application
/// minted. Every failure -- no header, a different scheme, a malformed
/// credential, an unknown id, a wrong secret, an expired token -- is the same
/// `401` with the same body, because a client that can tell them apart is
/// being told about tokens it does not hold.
///
/// The store is read from a request extension, so the application installs it
/// once:
///
/// ```
/// use arcature::axum::{Extension, Router, routing::get};
/// use arcature::tokens::{ApiAuth, ApiTokens};
///
/// async fn deploy(ApiAuth(token): ApiAuth) -> Result<String, arcature::axum::http::StatusCode> {
///     // Authentication says who; the ability says what.
///     if !token.can("deploy:write") {
///         return Err(arcature::axum::http::StatusCode::FORBIDDEN);
///     }
///     Ok(format!("deploying for {}", token.tokenable_id()))
/// }
///
/// fn routes(tokens: ApiTokens) -> Router {
///     Router::new()
///         .route("/deploy", get(deploy))
///         .layer(Extension(tokens))
/// }
/// ```
///
/// # Why `401` and not `403` for a missing token
///
/// `401` means "authenticate and try again" and obliges the response to carry
/// a `WWW-Authenticate` header saying how; this one sends `Bearer`. `403`
/// means "you authenticated and it still is not allowed", which is the answer
/// a route gives after [`ApiToken::can`] returns false -- as the example
/// above does.
#[derive(Clone, Debug)]
pub struct ApiAuth(pub ApiToken);

impl ApiAuth {
    /// The authenticated token.
    #[must_use]
    pub fn token(&self) -> &ApiToken {
        &self.0
    }

    /// Whether the token carries an ability. Shorthand for
    /// `self.token().can(..)`.
    #[must_use]
    pub fn can(&self, ability: &str) -> bool {
        self.0.can(ability)
    }
}

impl<S> FromRequestParts<S> for ApiAuth
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Extension(tokens) = Extension::<ApiTokens>::from_request_parts(parts, state)
            .await
            .map_err(|_| misconfigured())?;

        let presented = bearer(parts).ok_or_else(unauthenticated)?;

        match tokens.authenticate(&presented).await {
            Ok(Some(token)) => Ok(Self(token)),
            Ok(None) => Err(unauthenticated()),
            // A database that is down is the server's problem, not the
            // client's, and answering `401` would tell an honest client to go
            // and mint another token for no reason.
            Err(_) => Err(StatusCode::SERVICE_UNAVAILABLE.into_response()),
        }
    }
}

/// The credential from an `Authorization: Bearer` header, if there is one.
///
/// Returns an owned `String` rather than a borrow because the store's future
/// outlives the borrow of `parts`.
fn bearer(parts: &Parts) -> Option<String> {
    let value = parts.headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, credential) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case(SCHEME) {
        return None;
    }
    let credential = credential.trim_start();
    if credential.is_empty() {
        return None;
    }
    Some(credential.to_owned())
}

/// The one rejection every authentication failure shares.
fn unauthenticated() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        "Authentication required",
    )
        .into_response()
}

/// The route asked for a token but the application never installed the store.
///
/// This is a wiring mistake, not a client mistake, so it must not read as
/// `401`: a `401` would send a correct client away to mint a token that would
/// fail in exactly the same way.
fn misconfigured() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "API tokens are not configured",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn parts_with(value: &str) -> Parts {
        Request::builder()
            .header(header::AUTHORIZATION, value)
            .body(())
            .expect("a header value the test wrote is valid")
            .into_parts()
            .0
    }

    #[test]
    fn a_bearer_credential_is_read() {
        assert_eq!(
            bearer(&parts_with("Bearer abc123")).as_deref(),
            Some("abc123")
        );
    }

    #[test]
    fn the_scheme_is_matched_case_insensitively() {
        // RFC 7235 says the scheme is case-insensitive, and clients differ:
        // curl writes what you tell it, and several HTTP libraries
        // title-case or lower-case it on the way out.
        for spelling in ["Bearer", "bearer", "BEARER", "BeArEr"] {
            let header = format!("{spelling} abc123");
            assert_eq!(
                bearer(&parts_with(&header)).as_deref(),
                Some("abc123"),
                "{spelling} should be accepted"
            );
        }
    }

    #[test]
    fn another_scheme_is_not_a_bearer_token() {
        // Basic auth carries a password. Reading it as a token would send it
        // to `authenticate`, where it would fail -- but it would also have
        // been hashed and compared, and a password does not belong on that
        // path at all.
        assert!(bearer(&parts_with("Basic dXNlcjpwYXNz")).is_none());
    }

    #[test]
    fn a_scheme_with_no_credential_is_rejected() {
        for value in ["Bearer", "Bearer ", "Bearer    "] {
            assert!(
                bearer(&parts_with(value)).is_none(),
                "{value:?} carries no credential"
            );
        }
    }

    #[test]
    fn a_header_that_is_not_utf8_is_rejected_rather_than_lossily_decoded() {
        // A lossy decode would turn arbitrary bytes into a string with
        // replacement characters, which then goes to the parser as if the
        // client had sent it. Refusing is the honest answer.
        let parts = Request::builder()
            .header(
                header::AUTHORIZATION,
                axum::http::HeaderValue::from_bytes(b"Bearer \xff\xfe")
                    .expect("bytes are a valid header value even when not UTF-8"),
            )
            .body(())
            .expect("request builds")
            .into_parts()
            .0;
        assert!(bearer(&parts).is_none());
    }

    #[test]
    fn the_rejection_names_the_scheme_the_client_should_use() {
        // RFC 6750 requires it, and without it a client has been told to
        // authenticate without being told how.
        let response = unauthenticated();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer")
        );
    }

    #[test]
    fn a_missing_store_is_a_server_error_and_not_an_authentication_failure() {
        assert_eq!(misconfigured().status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
