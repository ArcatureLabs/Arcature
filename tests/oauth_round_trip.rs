//! The Authorization Code flow, driven end to end against a provider.
//!
//! `tests/oauth.rs` pins the properties: the constant-time `state`
//! comparison, the types that refuse to render their own secrets, the
//! transport check. Every one of them holds a single piece of the module
//! still. None of them ever completes a flow, and the bugs that survive a
//! green unit suite are the ones that live in the joins -- a verifier sent
//! under the wrong parameter name, a `state` checked on the leg after the
//! one that matters, an access token parsed out of the `refresh_token`
//! member. Each of those is invisible to a test that never puts an
//! authorization server on the other end of the socket.
//!
//! So there is one here. It is an `axum::Router` bound to `127.0.0.1:0` in
//! the test process, and it is a mock rather than a real provider for the
//! reason that matters to CI: a pull request from a fork gets no secrets and
//! no network egress, and a test that cannot run there is a test that stops
//! guarding the code the moment an outside contributor touches it. It is a
//! hand-written mock rather than `wiremock` for the reason that governs
//! every dependency in this repository -- a crate not pulled in is a crate
//! nobody has to watch for advisories.
//!
//! `127.0.0.1` is inside the loopback exception in
//! `oauth::provider::require_transport_security`, so `http://127.0.0.1:PORT`
//! is accepted by design and no certificate has to be minted for a test.
//! `OauthClient::for_urls` rather than `OauthClient::new`, because a port
//! chosen by the kernel cannot be a `&'static str`.
//!
//! # What the provider checks, and why it checks it itself
//!
//! The mock recomputes the PKCE challenge from the verifier it is handed and
//! refuses the exchange when the two disagree. That is what a real
//! authorization server does, and doing it here is what turns
//! "`code_challenge` appears in the URL" into "the challenge the provider
//! saw is the SHA-256 of the verifier the exchange later sent". SHA-256 and
//! base64url are written out at the bottom of this file rather than pulled
//! in: `sha2` belongs to the `uploads` feature and is not compiled by an
//! `oauth` build, and the implementation is pinned against the published
//! FIPS 180-4 and RFC 7636 vectors, so a mistake in the test's own
//! arithmetic fails loudly instead of quietly agreeing with a mistake in the
//! code under test.

#![cfg(feature = "oauth")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arcature::oauth::{OauthClient, OauthError, OauthState, PkceVerifier, TokenSet};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

// ---------------------------------------------------------------------------
// The values the flow carries, chosen so an assertion cannot pass by accident
// ---------------------------------------------------------------------------

const CLIENT_ID: &str = "arcature-round-trip";
const CLIENT_SECRET: &str = "the-client-secret-nobody-should-see";
const ACCESS_TOKEN: &str = "access-token-issued-by-the-mock-provider";
const REFRESH_TOKEN: &str = "refresh-token-issued-by-the-mock-provider";
const REFRESHED_ACCESS_TOKEN: &str = "second-access-token-from-the-refresh-grant";
const AUTHORIZATION_CODE: &str = "the-one-time-authorization-code";
const SUBJECT: &str = "user-42";

// ---------------------------------------------------------------------------
// The mock authorization server
// ---------------------------------------------------------------------------

/// What the authorize leg put aside for the token leg to check.
#[derive(Debug, Clone)]
struct PendingFlow {
    challenge: String,
    challenge_method: String,
    redirect_uri: String,
    state: String,
    scope: String,
}

/// Everything the provider saw, so a test can assert on the wire rather than
/// on the client's own account of it.
#[derive(Debug, Default)]
struct Ledger {
    authorize_calls: usize,
    token_calls: usize,
    userinfo_calls: usize,
    /// Keyed by the authorization code the authorize leg minted.
    pending: HashMap<String, PendingFlow>,
    /// Every `code_verifier` the token endpoint was handed, in order.
    verifiers: Vec<String>,
    /// Every `Authorization` header the token endpoint was handed.
    token_authorizations: Vec<String>,
    /// Every bearer credential the userinfo endpoint was handed.
    userinfo_bearers: Vec<String>,
    /// Refresh tokens the provider has issued and will still honour.
    live_refresh_tokens: Vec<String>,
}

/// The provider's shared state. `Arc` because axum hands each handler a
/// clone, and `Mutex` because the assertions read it from the test thread
/// while the server writes it from the runtime.
#[derive(Debug, Default)]
struct Provider {
    ledger: Mutex<Ledger>,
}

impl Provider {
    fn ledger(&self) -> std::sync::MutexGuard<'_, Ledger> {
        self.ledger.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// An OAuth error object, which is what a real provider answers with and the
/// shape `OauthError::Provider` is built from.
fn oauth_error(code: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": code,
            "error_description": "the mock provider refused the request",
        })),
    )
        .into_response()
}

/// `GET /authorize` -- record what the client sent, then redirect back with a
/// code, which is the part a browser would do.
async fn authorize(
    State(provider): State<Arc<Provider>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let mut ledger = provider.ledger();
    ledger.authorize_calls += 1;

    let Some(redirect_uri) = params.get("redirect_uri").cloned() else {
        return oauth_error("invalid_request");
    };
    let state = params.get("state").cloned().unwrap_or_default();
    ledger.pending.insert(
        AUTHORIZATION_CODE.to_string(),
        PendingFlow {
            challenge: params.get("code_challenge").cloned().unwrap_or_default(),
            challenge_method: params
                .get("code_challenge_method")
                .cloned()
                .unwrap_or_default(),
            redirect_uri: redirect_uri.clone(),
            state: state.clone(),
            scope: params.get("scope").cloned().unwrap_or_default(),
        },
    );

    let location = format!(
        "{redirect_uri}?code={}&state={}",
        urlencode(AUTHORIZATION_CODE),
        urlencode(&state)
    );
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

/// `POST /token` -- both grants, with the PKCE check a real server performs.
async fn token(
    State(provider): State<Arc<Provider>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let form: HashMap<String, String> = serde_urlencoded::from_str(&body).unwrap_or_default();
    let mut ledger = provider.ledger();
    ledger.token_calls += 1;
    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        ledger.token_authorizations.push(value.to_string());
    }

    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            let Some(code) = form.get("code") else {
                return oauth_error("invalid_request");
            };
            let Some(pending) = ledger.pending.remove(code) else {
                return oauth_error("invalid_grant");
            };
            let verifier = form.get("code_verifier").cloned().unwrap_or_default();
            ledger.verifiers.push(verifier.clone());

            // The check that makes this a PKCE test rather than a
            // the-string-appears-in-a-URL test.
            if pending.challenge_method != "S256"
                || challenge_for(&verifier) != pending.challenge
                || form.get("redirect_uri") != Some(&pending.redirect_uri)
            {
                return oauth_error("invalid_grant");
            }

            ledger.live_refresh_tokens.push(REFRESH_TOKEN.to_string());
            Json(serde_json::json!({
                "access_token": ACCESS_TOKEN,
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": REFRESH_TOKEN,
                // Narrower than what was asked for, which is a provider's
                // prerogative and something `TokenSet::scopes` has to report
                // rather than echo back from the request.
                "scope": "read:user",
            }))
            .into_response()
        }
        Some("refresh_token") => {
            let presented = form.get("refresh_token").cloned().unwrap_or_default();
            if !ledger.live_refresh_tokens.contains(&presented) {
                return oauth_error("invalid_grant");
            }
            Json(serde_json::json!({
                "access_token": REFRESHED_ACCESS_TOKEN,
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": REFRESH_TOKEN,
                "scope": "read:user",
            }))
            .into_response()
        }
        _ => oauth_error("unsupported_grant_type"),
    }
}

/// `GET /userinfo` -- the resource-server leg, which is the only thing that
/// can tell an access token from a refresh token that happened to parse.
async fn userinfo(State(provider): State<Arc<Provider>>, headers: HeaderMap) -> Response {
    let mut ledger = provider.ledger();
    ledger.userinfo_calls += 1;
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    ledger.userinfo_bearers.push(presented.clone());

    if presented == format!("Bearer {ACCESS_TOKEN}")
        || presented == format!("Bearer {REFRESHED_ACCESS_TOKEN}")
    {
        Json(serde_json::json!({ "sub": SUBJECT, "login": "ada" })).into_response()
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "invalid_token" })),
        )
            .into_response()
    }
}

/// A running provider: its base URL, and the ledger of what it saw.
struct RunningProvider {
    base: String,
    provider: Arc<Provider>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RunningProvider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Bind the provider to a kernel-chosen loopback port and start serving.
async fn start_provider() -> RunningProvider {
    let provider = Arc::new(Provider::default());
    let router = Router::new()
        .route("/authorize", get(authorize))
        .route("/token", post(token))
        .route("/userinfo", get(userinfo))
        .with_state(Arc::clone(&provider));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("a loopback port should be available");
    let address = listener.local_addr().expect("the port should be readable");
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    RunningProvider {
        base: format!("http://{address}"),
        provider,
        task,
    }
}

impl RunningProvider {
    /// A client pointed at this provider. Loopback plaintext, which is the
    /// one exception the transport check makes.
    fn client(&self) -> OauthClient {
        OauthClient::for_urls(
            &format!("{}/authorize", self.base),
            &format!("{}/token", self.base),
            CLIENT_ID,
            Some(CLIENT_SECRET.to_string()),
            &format!("{}/callback", self.base),
        )
        .expect("loopback http is inside the transport exception")
    }

    /// The browser leg: follow the authorization URL as far as the redirect
    /// and hand back the callback's query parameters.
    async fn approve(&self, url: &str) -> HashMap<String, String> {
        let agent = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("a client with redirects disabled");
        let response = agent.get(url).send().await.expect("the authorize leg");
        assert_eq!(
            response.status().as_u16(),
            302,
            "the provider should redirect back to the application"
        );
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .expect("a Location header")
            .to_string();
        let query = location.split_once('?').map(|(_, q)| q).unwrap_or_default();
        serde_urlencoded::from_str(query).expect("the callback query parses")
    }
}

// ---------------------------------------------------------------------------
// The flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_whole_authorization_code_flow_completes_against_a_provider() {
    let server = start_provider().await;
    let client = server.client();

    let start = client
        .authorize(&["read:user", "profile"])
        .expect("the authorization URL builds");
    let issued_state = start.state().as_str().to_string();
    let issued_verifier = start.verifier().secret().to_string();

    let callback = server.approve(start.url().as_str()).await;
    assert_eq!(
        callback.get("state").map(String::as_str),
        Some(issued_state.as_str()),
        "the provider must echo the state it was given"
    );

    let tokens = client
        .exchange(
            &OauthState::from_stored(issued_state),
            callback.get("state").expect("a state on the callback"),
            callback.get("code").expect("a code on the callback"),
            PkceVerifier::from_secret(issued_verifier),
        )
        .await
        .expect("the exchange should succeed");

    assert_eq!(tokens.access_token(), ACCESS_TOKEN);
    assert_eq!(tokens.refresh_token(), Some(REFRESH_TOKEN));
    assert_eq!(tokens.token_type(), "bearer");
    assert_eq!(tokens.expires_in(), Some(Duration::from_secs(3600)));
    assert_eq!(tokens.scopes(), ["read:user".to_string()]);

    // The resource-server leg. Nothing before this point can tell an access
    // token parsed out of the right member from one parsed out of the wrong
    // one: both are strings, and both survive every assertion above.
    let profile: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/userinfo", server.base))
        .bearer_auth(tokens.access_token())
        .send()
        .await
        .expect("the userinfo leg")
        .json()
        .await
        .expect("a JSON profile");
    assert_eq!(profile["sub"], SUBJECT);

    let ledger = server.provider.ledger();
    assert_eq!(ledger.authorize_calls, 1);
    assert_eq!(ledger.token_calls, 1);
    assert_eq!(ledger.userinfo_calls, 1);
    assert_eq!(
        ledger.userinfo_bearers,
        vec![format!("Bearer {ACCESS_TOKEN}")],
        "a refresh token must never be the credential sent to a resource server"
    );
    assert_eq!(
        ledger.token_authorizations,
        vec![format!(
            "Basic {}",
            base64_standard(format!("{CLIENT_ID}:{CLIENT_SECRET}").as_bytes())
        )],
        "the client must authenticate itself to the token endpoint"
    );
}

#[tokio::test]
async fn the_challenge_the_provider_saw_is_the_sha256_of_the_verifier_the_exchange_sent() {
    let server = start_provider().await;
    let client = server.client();

    let start = client.authorize(&["read:user"]).expect("authorize");
    let issued_state = start.state().as_str().to_string();
    let issued_verifier = start.verifier().secret().to_string();

    // Read the challenge back off the wire the provider saw it on, not out
    // of the `Authorization` value: the question is what the client sent.
    let callback = server.approve(start.url().as_str()).await;
    let seen = server
        .provider
        .ledger()
        .pending
        .get(AUTHORIZATION_CODE)
        .cloned()
        .expect("the authorize leg recorded a pending flow");
    assert_eq!(seen.challenge_method, "S256");
    assert_ne!(
        seen.challenge, issued_verifier,
        "the plain verifier must never travel on the authorize leg"
    );
    assert_eq!(seen.challenge, challenge_for(&issued_verifier));
    assert_eq!(seen.scope, "read:user");
    assert_eq!(seen.state, issued_state);

    client
        .exchange(
            &OauthState::from_stored(issued_state),
            callback.get("state").expect("a state"),
            callback.get("code").expect("a code"),
            PkceVerifier::from_secret(issued_verifier.clone()),
        )
        .await
        .expect("the exchange should succeed");

    assert_eq!(
        server.provider.ledger().verifiers,
        vec![issued_verifier],
        "the verifier the token endpoint received must be the one the challenge was derived from"
    );
}

#[tokio::test]
async fn a_provider_that_recomputes_the_challenge_refuses_a_verifier_that_does_not_match() {
    let server = start_provider().await;
    let client = server.client();

    let start = client.authorize(&["read:user"]).expect("authorize");
    let issued_state = start.state().as_str().to_string();
    let callback = server.approve(start.url().as_str()).await;

    // A verifier that is well-formed and simply is not the one the challenge
    // was built from -- the shape of an intercepted authorization code being
    // redeemed by somebody else, which is the whole reason PKCE exists.
    let outcome = client
        .exchange(
            &OauthState::from_stored(issued_state),
            callback.get("state").expect("a state"),
            callback.get("code").expect("a code"),
            PkceVerifier::from_secret("a-verifier-from-somebody-elses-flow".into()),
        )
        .await;

    match outcome {
        Err(OauthError::Provider { code }) => assert_eq!(code, "invalid_grant"),
        other => panic!("a mismatched verifier must be refused: {other:?}"),
    }
    assert_eq!(server.provider.ledger().token_calls, 1);
}

#[tokio::test]
async fn a_callback_carrying_a_state_from_another_flow_never_reaches_the_token_endpoint() {
    let server = start_provider().await;
    let client = server.client();

    let start = client.authorize(&["read:user"]).expect("authorize");
    let issued_state = start.state().as_str().to_string();
    let issued_verifier = start.verifier().secret().to_string();
    let callback = server.approve(start.url().as_str()).await;

    // A second flow's state, generated the same way, so this is a real CSRF
    // shape rather than a malformed string any parser would reject anyway.
    let other = client.authorize(&["read:user"]).expect("a second flow");
    let outcome = client
        .exchange(
            &OauthState::from_stored(issued_state),
            other.state().as_str(),
            callback.get("code").expect("a code"),
            PkceVerifier::from_secret(issued_verifier),
        )
        .await;

    assert!(
        matches!(outcome, Err(OauthError::StateMismatch)),
        "{outcome:?}"
    );
    assert_eq!(
        server.provider.ledger().token_calls,
        0,
        "a forged callback must be refused before the code is redeemed, not after"
    );
}

#[tokio::test]
async fn a_provider_error_object_surfaces_as_an_oauth_error_rather_than_a_success() {
    let server = start_provider().await;
    let client = server.client();

    let start = client.authorize(&["read:user"]).expect("authorize");
    let issued_state = start.state().as_str().to_string();
    let issued_verifier = start.verifier().secret().to_string();
    let callback = server.approve(start.url().as_str()).await;

    // Redeem the code once, which consumes it, then present it again. A real
    // provider answers the replay with `invalid_grant`, and that arrives as a
    // `400` carrying an OAuth error object rather than as a transport
    // failure or a panic.
    client
        .exchange(
            &OauthState::from_stored(issued_state.clone()),
            callback.get("state").expect("a state"),
            callback.get("code").expect("a code"),
            PkceVerifier::from_secret(issued_verifier.clone()),
        )
        .await
        .expect("the first exchange should succeed");

    let replay = client
        .exchange(
            &OauthState::from_stored(issued_state),
            callback.get("state").expect("a state"),
            callback.get("code").expect("a code"),
            PkceVerifier::from_secret(issued_verifier),
        )
        .await;

    match &replay {
        Err(OauthError::Provider { code }) => assert_eq!(code, "invalid_grant"),
        other => panic!("a replayed code must not succeed: {other:?}"),
    }

    // And the error still carries nothing out of the body: the description
    // the provider sent is not in it, because a body is where a token hides.
    let rendered = replay.expect_err("the replay failed").to_string();
    assert!(
        !rendered.contains("the mock provider refused"),
        "the response body reached the error: {rendered}"
    );
}

/// The refresh grant.
///
/// `OauthClient` has no `refresh` method -- `exchange` is the only thing on
/// it that talks to a token endpoint -- so the grant itself is driven through
/// `oauth2`, the crate `arcature::oauth` re-exports precisely so downstream
/// code targets the version Arcature pinned. What that leaves testable here
/// is the half Arcature does own, and it is the half that breaks: `exchange`
/// has to carry the provider's `refresh_token` member into
/// `TokenSet::refresh_token` and not into `access_token`, the string it lands
/// on has to be the one the token endpoint will honour, and the refreshed
/// credential has to redact under `Debug` like the first one.
#[tokio::test]
async fn a_refreshed_token_set_carries_the_new_access_token() {
    use arcature::oauth::oauth2::{
        ClientId, ClientSecret, RefreshToken, TokenResponse as _, TokenUrl, basic::BasicClient,
    };

    let server = start_provider().await;
    let client = server.client();

    let start = client.authorize(&["read:user"]).expect("authorize");
    let issued_state = start.state().as_str().to_string();
    let issued_verifier = start.verifier().secret().to_string();
    let callback = server.approve(start.url().as_str()).await;
    let first = client
        .exchange(
            &OauthState::from_stored(issued_state),
            callback.get("state").expect("a state"),
            callback.get("code").expect("a code"),
            PkceVerifier::from_secret(issued_verifier),
        )
        .await
        .expect("the exchange should succeed");

    let refresh = first
        .refresh_token()
        .expect("the provider issued a refresh token and `exchange` must carry it")
        .to_string();
    assert_ne!(
        refresh,
        first.access_token(),
        "the two members must not be read out of the same key"
    );

    let http = arcature::oauth::oauth2::reqwest::ClientBuilder::new()
        .redirect(arcature::oauth::oauth2::reqwest::redirect::Policy::none())
        .build()
        .expect("an HTTP client");
    let refreshing = BasicClient::new(ClientId::new(CLIENT_ID.to_string()))
        .set_client_secret(ClientSecret::new(CLIENT_SECRET.to_string()))
        .set_token_uri(TokenUrl::new(format!("{}/token", server.base)).expect("a token URL"));
    let response = refreshing
        .exchange_refresh_token(&RefreshToken::new(refresh))
        .request_async(&http)
        .await
        .expect("the refresh grant should succeed");

    let refreshed = TokenSet::new(
        response.access_token().secret().clone(),
        response.token_type().as_ref().to_string(),
    );
    assert_eq!(refreshed.access_token(), REFRESHED_ACCESS_TOKEN);
    assert_ne!(refreshed.access_token(), first.access_token());
    assert!(
        !format!("{refreshed:?}").contains(REFRESHED_ACCESS_TOKEN),
        "a refreshed credential must redact like the first one"
    );

    // And the refreshed credential is a working one at the resource server.
    let profile: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/userinfo", server.base))
        .bearer_auth(refreshed.access_token())
        .send()
        .await
        .expect("the userinfo leg")
        .json()
        .await
        .expect("a JSON profile");
    assert_eq!(profile["sub"], SUBJECT);
}

#[tokio::test]
async fn a_token_endpoint_that_is_not_listening_is_a_transport_failure_not_a_success() {
    // The counterpart to the provider-error case: `exchange` has three ways
    // to fail and they have to stay distinguishable, because an application
    // may retry one of them and must not retry the others.
    let client = OauthClient::for_urls(
        "http://127.0.0.1:1/authorize",
        "http://127.0.0.1:1/token",
        CLIENT_ID,
        None,
        "http://127.0.0.1:1/callback",
    )
    .expect("loopback http is inside the transport exception");

    let outcome = client
        .exchange(
            &OauthState::from_stored("the-state-we-issued".into()),
            "the-state-we-issued",
            "a-code",
            PkceVerifier::from_secret("a-verifier".into()),
        )
        .await;
    assert!(matches!(outcome, Err(OauthError::Transport)), "{outcome:?}");
}

// ---------------------------------------------------------------------------
// The provider's own arithmetic
// ---------------------------------------------------------------------------

/// The RFC 7636 `S256` transformation: base64url(SHA-256(verifier)), no pad.
fn challenge_for(verifier: &str) -> String {
    base64_url_nopad(&sha256(verifier.as_bytes()))
}

const ROUND_CONSTANTS: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

fn sha256(message: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];

    let mut padded = message.to_vec();
    let bit_length = (message.len() as u64) * 8;
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_length.to_be_bytes());

    for block in padded.as_chunks::<64>().0 {
        let mut schedule = [0u32; 64];
        for (slot, word) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
            *slot = u32::from_be_bytes(*word);
        }
        for index in 16..64 {
            let previous = schedule[index - 15];
            let recent = schedule[index - 2];
            let s0 = previous.rotate_right(7) ^ previous.rotate_right(18) ^ (previous >> 3);
            let s1 = recent.rotate_right(17) ^ recent.rotate_right(19) ^ (recent >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for (constant, word) in ROUND_CONSTANTS.iter().zip(schedule.iter()) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(*constant)
                .wrapping_add(*word);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut digest = [0u8; 32];
    for (chunk, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *chunk = word.to_be_bytes();
    }
    digest
}

const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn base64(bytes: &[u8], alphabet: &[u8; 64], pad: bool) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let mut packed = 0u32;
        for (index, byte) in group.iter().enumerate() {
            packed |= u32::from(*byte) << (16 - 8 * index);
        }
        let digits = group.len() + 1;
        for index in 0..digits {
            let shift = 18 - 6 * index;
            out.push(alphabet[((packed >> shift) & 0x3f) as usize] as char);
        }
        if pad {
            for _ in digits..4 {
                out.push('=');
            }
        }
    }
    out
}

fn base64_url_nopad(bytes: &[u8]) -> String {
    base64(bytes, URL_SAFE, false)
}

fn base64_standard(bytes: &[u8]) -> String {
    base64(bytes, STANDARD, true)
}

/// Percent-encode a query value. `percent-encoding` belongs to the `mail`
/// feature; a state is hex and a code is an ASCII literal, so an unreserved
/// allow-list covers everything the mock emits.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[test]
fn the_tests_own_sha256_matches_the_published_vectors() {
    // FIPS 180-4, Appendix B: the empty string and "abc".
    assert_eq!(
        hex(&sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex(&sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    // Long enough to need a second block, so the padding branch is exercised
    // rather than assumed.
    assert_eq!(
        hex(&sha256(
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
        )),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

#[test]
fn the_tests_own_pkce_transformation_matches_the_rfc_7636_vector() {
    // RFC 7636, Appendix B.
    assert_eq!(
        challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn the_tests_own_base64_matches_the_rfc_4648_vectors() {
    for (input, expected) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64_standard(input.as_bytes()), expected, "{input}");
    }
}
