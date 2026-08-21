//! Provider endpoints, the presets, and the Authorization Code client.
//!
//! An OAuth 2.0 authorization server is, from a client's point of view, two
//! URLs. That is what [`Endpoints`] holds, and it is why there is no provider
//! enum: GitHub is a pair of `const` strings, and so is the identity provider
//! a company runs in-house. Adding support for a new provider is not a
//! framework change.

use std::fmt;
use std::time::Duration;

use oauth2::basic::{BasicClient, BasicErrorResponse, BasicTokenResponse};
use oauth2::url::{Host, Position, Url};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet, EndpointSet,
    HttpClientError, PkceCodeChallenge, RedirectUrl, RequestTokenError, Scope, TokenResponse,
    TokenUrl,
};

use crate::oauth::error::OauthError;
use crate::oauth::pkce::{OauthState, PkceVerifier};

/// The two URLs that define an OAuth 2.0 authorization server.
///
/// A `const` so a preset costs nothing and an application can define its own
/// alongside the bundled ones without asking for a framework release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoints {
    /// Where the user agent is sent to authorize (RFC 6749 authorization
    /// endpoint).
    pub authorization: &'static str,
    /// Where the authorization code is redeemed (RFC 6749 token endpoint).
    pub token: &'static str,
}

/// GitHub's OAuth endpoints.
pub const GITHUB: Endpoints = Endpoints {
    authorization: "https://github.com/login/oauth/authorize",
    token: "https://github.com/login/oauth/access_token",
};

/// Google's OAuth 2.0 endpoints.
pub const GOOGLE: Endpoints = Endpoints {
    authorization: "https://accounts.google.com/o/oauth2/v2/auth",
    token: "https://oauth2.googleapis.com/token",
};

/// Discord's OAuth 2.0 endpoints.
pub const DISCORD: Endpoints = Endpoints {
    authorization: "https://discord.com/oauth2/authorize",
    token: "https://discord.com/api/oauth2/token",
};

/// Whether `url` may carry an OAuth exchange.
///
/// `https` always; plaintext `http` only when the host is loopback, which is
/// the one case where there is no network to intercept -- and the case every
/// local development redirect URI needs. There is no override: an
/// application that could switch this off would eventually ship with it
/// switched off.
fn require_transport_security(url: &Url, role: &'static str) -> Result<(), OauthError> {
    if url.scheme() == "https" {
        return Ok(());
    }
    let loopback = match url.host() {
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    if url.scheme() == "http" && loopback {
        return Ok(());
    }
    Err(OauthError::InsecureTransport { role })
}

/// Parse and transport-check one URL.
fn checked_url(raw: &str, role: &'static str) -> Result<Url, OauthError> {
    let url = Url::parse(raw).map_err(|_| OauthError::InvalidUrl { role })?;
    require_transport_security(&url, role)?;
    Ok(url)
}

/// The concrete `oauth2` client type once both endpoints are set.
type ConfiguredClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

/// An Authorization Code + PKCE client for one provider.
///
/// Hold it as application state: it owns an HTTP client and a set of
/// endpoints, and it is not looked up from anywhere.
pub struct OauthClient {
    inner: ConfiguredClient,
    http: oauth2::reqwest::Client,
}

impl fmt::Debug for OauthClient {
    /// Prints the type name only. The client holds a [`ClientSecret`], and
    /// `oauth2`'s own redaction is not something this crate should rely on
    /// transitively.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OauthClient").finish_non_exhaustive()
    }
}

impl OauthClient {
    /// Build a client for a bundled or application-defined [`Endpoints`].
    ///
    /// # Errors
    ///
    /// Returns [`OauthError::InvalidUrl`] if an endpoint or the redirect URI
    /// does not parse, or [`OauthError::InsecureTransport`] if any of them is
    /// plaintext `http` on a host that is not loopback.
    pub fn new(
        endpoints: Endpoints,
        client_id: impl Into<String>,
        client_secret: Option<String>,
        redirect_uri: &str,
    ) -> Result<Self, OauthError> {
        Self::for_urls(
            endpoints.authorization,
            endpoints.token,
            client_id,
            client_secret,
            redirect_uri,
        )
    }

    /// Build a client from endpoint URLs known only at run time -- read from
    /// configuration, or discovered.
    ///
    /// # Errors
    ///
    /// As [`OauthClient::new`].
    pub fn for_urls(
        authorization_endpoint: &str,
        token_endpoint: &str,
        client_id: impl Into<String>,
        client_secret: Option<String>,
        redirect_uri: &str,
    ) -> Result<Self, OauthError> {
        let auth = checked_url(authorization_endpoint, "authorization endpoint")?;
        let token = checked_url(token_endpoint, "token endpoint")?;
        let redirect = checked_url(redirect_uri, "redirect URI")?;

        // `None` is a public client: a native app or a single-page app,
        // which has no secret to keep and relies on PKCE alone. Sending an
        // empty `client_secret` instead of omitting it makes some providers
        // reject the exchange outright.
        let mut inner = BasicClient::new(ClientId::new(client_id.into()));
        if let Some(secret) = client_secret {
            inner = inner.set_client_secret(ClientSecret::new(secret));
        }
        let inner = inner
            .set_auth_uri(AuthUrl::from_url(auth))
            .set_token_uri(TokenUrl::from_url(token))
            .set_redirect_uri(RedirectUrl::from_url(redirect));

        // Redirects are refused: a token endpoint that 302s somewhere is an
        // SSRF primitive, not a provider quirk to accommodate.
        let http = oauth2::reqwest::ClientBuilder::new()
            .redirect(oauth2::reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| OauthError::Transport)?;

        Ok(Self { inner, http })
    }
}

/// What [`OauthClient::authorize`] produces: where to send the user, and the
/// two values that must survive until the callback.
///
/// `Debug` shows where the user is being sent but withholds the query
/// string: the state and the code challenge live in there, and a `Debug`
/// output is exactly the thing that ends up in a log.
pub struct Authorization {
    url: Url,
    state: OauthState,
    verifier: PkceVerifier,
}

impl fmt::Debug for Authorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Authorization")
            .field(
                "url",
                &format_args!("{}?[redacted]", &self.url[..Position::AfterPath]),
            )
            .field("state", &self.state)
            .field("verifier", &self.verifier)
            .finish()
    }
}

impl Authorization {
    /// The URL to redirect the user agent to.
    #[must_use]
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The CSRF state to store until the callback arrives.
    #[must_use]
    pub fn state(&self) -> &OauthState {
        &self.state
    }

    /// The PKCE verifier to store until the callback arrives.
    #[must_use]
    pub fn verifier(&self) -> &PkceVerifier {
        &self.verifier
    }

    /// Take the three parts apart.
    #[must_use]
    pub fn into_parts(self) -> (Url, OauthState, PkceVerifier) {
        (self.url, self.state, self.verifier)
    }
}

impl OauthClient {
    /// Start a flow: generate the state and the PKCE pair, and build the
    /// authorization URL.
    ///
    /// # Errors
    ///
    /// Returns [`OauthError::Entropy`] if the OS randomness source is
    /// unavailable.
    pub fn authorize(&self, scopes: &[&str]) -> Result<Authorization, OauthError> {
        let state = OauthState::generate()?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let carried = state.as_str().to_string();
        let (url, _) = self
            .inner
            .authorize_url(move || CsrfToken::new(carried))
            .add_scopes(scopes.iter().map(|s| Scope::new((*s).to_string())))
            .set_pkce_challenge(challenge)
            .url();
        Ok(Authorization {
            url,
            state,
            verifier: PkceVerifier::from_secret(verifier.into_secret()),
        })
    }

    /// Finish a flow: check the returned state, then redeem the code.
    ///
    /// The state check comes first and is constant time. `stored` is the
    /// state this application put aside when it built the authorization URL;
    /// `returned` is the raw `state` query parameter from the callback.
    ///
    /// # Errors
    ///
    /// * [`OauthError::StateMismatch`] -- the callback does not belong to a
    ///   flow this application started.
    /// * [`OauthError::Transport`] -- the token endpoint was unreachable.
    /// * [`OauthError::Provider`] -- the token endpoint returned an OAuth
    ///   error.
    /// * [`OauthError::MalformedResponse`] -- the answer was not a token
    ///   response.
    pub async fn exchange(
        &self,
        stored: &OauthState,
        returned: &str,
        code: &str,
        verifier: PkceVerifier,
    ) -> Result<TokenSet, OauthError> {
        if !stored.verify(returned) {
            return Err(OauthError::StateMismatch);
        }
        let response = self
            .inner
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .set_pkce_verifier(verifier.into_inner())
            .request_async(&self.http)
            .await
            .map_err(map_token_error)?;
        Ok(TokenSet::from_response(&response))
    }
}

/// Translate an `oauth2` token error into an [`OauthError`] that carries no
/// response body.
///
/// `RequestTokenError::Parse` holds the bytes the provider sent. Those bytes
/// are a token response often enough that formatting them is a credential
/// leak, so the variant collapses to
/// [`MalformedResponse`](OauthError::MalformedResponse) with nothing attached.
fn map_token_error(
    error: RequestTokenError<HttpClientError<oauth2::reqwest::Error>, BasicErrorResponse>,
) -> OauthError {
    match error {
        RequestTokenError::ServerResponse(response) => OauthError::Provider {
            code: response.error().to_string(),
        },
        RequestTokenError::Request(_) => OauthError::Transport,
        RequestTokenError::Parse(_, _) => OauthError::MalformedResponse,
        RequestTokenError::Other(_) => OauthError::MalformedResponse,
    }
}

/// The tokens a successful exchange produced.
///
/// # Why `Debug` says nothing
///
/// A token response is the one value in an OAuth flow that is directly usable
/// by whoever reads it. `#[derive(Debug)]` on a struct holding one is a
/// standing invitation for it to appear in a `tracing` field, an `unwrap`
/// panic message, or an error chain, so this type formats as
/// `TokenSet([redacted])` and offers no `Display`. Reaching the access token
/// means calling [`TokenSet::access_token`].
pub struct TokenSet {
    access_token: String,
    refresh_token: Option<String>,
    token_type: String,
    expires_in: Option<Duration>,
    scopes: Vec<String>,
}

impl fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenSet([redacted])")
    }
}

impl TokenSet {
    fn from_response(response: &BasicTokenResponse) -> Self {
        Self {
            access_token: response.access_token().secret().clone(),
            refresh_token: response.refresh_token().map(|t| t.secret().clone()),
            token_type: response.token_type().as_ref().to_string(),
            expires_in: response.expires_in(),
            scopes: response
                .scopes()
                .map(|scopes| scopes.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default(),
        }
    }

    /// Build a token set directly. For tests and for applications that
    /// obtained tokens some other way and want the same redaction.
    #[must_use]
    pub fn new(access_token: impl Into<String>, token_type: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
            refresh_token: None,
            token_type: token_type.into(),
            expires_in: None,
            scopes: Vec::new(),
        }
    }

    /// The access token.
    ///
    /// # Security
    ///
    /// A bearer credential. Send it; do not log it, and do not put it in an
    /// error message.
    #[must_use]
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    /// The refresh token, when the provider issued one.
    ///
    /// # Security
    ///
    /// Longer-lived than the access token, and therefore worse to leak.
    #[must_use]
    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    /// The token type, lowercased (`"bearer"` for every provider that
    /// follows RFC 6750).
    #[must_use]
    pub fn token_type(&self) -> &str {
        &self.token_type
    }

    /// How long the access token is valid for, when the provider said.
    #[must_use]
    pub fn expires_in(&self) -> Option<Duration> {
        self.expires_in
    }

    /// The scopes the provider actually granted, which may be narrower than
    /// the ones requested.
    #[must_use]
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Url {
        Url::parse(raw).expect("test URL parses")
    }

    #[test]
    fn https_is_always_accepted() {
        assert!(require_transport_security(&parse("https://example.test/authorize"), "x").is_ok());
    }

    #[test]
    fn plaintext_http_is_refused_off_loopback() {
        let result = require_transport_security(&parse("http://example.test/authorize"), "x");
        assert!(matches!(result, Err(OauthError::InsecureTransport { .. })));
    }

    #[test]
    fn plaintext_http_is_allowed_on_loopback_only() {
        for raw in [
            "http://localhost:3000/callback",
            "http://LOCALHOST:3000/callback",
            "http://127.0.0.1:3000/callback",
            "http://[::1]:3000/callback",
        ] {
            assert!(
                require_transport_security(&parse(raw), "x").is_ok(),
                "{raw} should be allowed"
            );
        }
        // A host that merely mentions loopback is not loopback.
        assert!(require_transport_security(&parse("http://localhost.evil.test/"), "x").is_err());
        assert!(require_transport_security(&parse("http://127.0.0.1.evil.test/"), "x").is_err());
    }

    #[test]
    fn a_client_refuses_to_build_over_plaintext() {
        let result = OauthClient::for_urls(
            "http://sso.example.test/authorize",
            "https://sso.example.test/token",
            "id",
            Some("secret".into()),
            "https://app.example.test/callback",
        );
        assert!(matches!(
            result,
            Err(OauthError::InsecureTransport {
                role: "authorization endpoint"
            })
        ));
    }

    #[test]
    fn the_bundled_presets_all_pass_the_transport_check() {
        for endpoints in [GITHUB, GOOGLE, DISCORD] {
            assert!(checked_url(endpoints.authorization, "authorization endpoint").is_ok());
            assert!(checked_url(endpoints.token, "token endpoint").is_ok());
        }
    }
}
