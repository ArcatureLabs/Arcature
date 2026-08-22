//! A throttled login form, and the two things it must not give away.
//!
//! `CredentialChecker` makes "no such account" and "wrong password" cost the
//! same and say the same. `LoginThrottle` is bolted onto the same handler,
//! and it is perfectly capable of undoing that: a throttle that only counts
//! failures against accounts that exist answers the enumeration question by
//! *who gets locked out*. That interaction is what this file is about, and it
//! is not visible from either type alone -- it lives in the handler, in the
//! order the two are called and in which branches call them.
//!
//! The second property is smaller and just as easy to lose: once a client is
//! throttled, the right password must be refused too. A throttle consulted
//! *after* verification would let an attacker who finally guesses correctly
//! walk straight through the lockout that was supposed to stop them.

#![cfg(feature = "auth-flows")]

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use arcature::auth::flows::{
    CREDENTIAL_REJECTION, CredentialChecker, CredentialOutcome, LoginThrottle,
};
use arcature::auth::{PasswordConfig, PasswordHashString, PasswordHasher};
use arcature::http::{ClientIp, TrustedProxies};
use axum::Router;
use axum::body::Body;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// The application
// ---------------------------------------------------------------------------

/// The one address that has an account here.
const REGISTERED: &str = "registered@example.com";

/// An address that does not, and whose indistinguishability is the point.
const UNKNOWN: &str = "unknown@example.com";

const PASSWORD: &str = "correct horse battery staple";

#[derive(Clone)]
struct AppState {
    users: Arc<HashMap<String, PasswordHashString>>,
    checker: CredentialChecker,
    throttle: LoginThrottle,
}

fn state(throttle: LoginThrottle) -> AppState {
    // The cheapest valid parameters: the property under test is which
    // branches run a verification, not how long one takes.
    let hasher = PasswordHasher::new(PasswordConfig::new(8, 1, 1)).expect("valid params");
    let stored = hasher.hash(PASSWORD.as_bytes()).expect("hash");

    let mut users = HashMap::new();
    users.insert(REGISTERED.to_owned(), stored);

    AppState {
        users: Arc::new(users),
        checker: CredentialChecker::new(hasher).expect("absent-user hash"),
        throttle,
    }
}

/// A login handler wired the way the documentation says to wire one.
///
/// The body is parsed here rather than in a layer, which is the whole reason
/// the throttle is a handle and not a [`tower::Layer`]: the address being
/// signed in to arrives in the request body, and a layer runs before anybody
/// has read it.
async fn login(
    State(state): State<AppState>,
    client: Option<Extension<ClientIp>>,
    body: String,
) -> Response {
    let fields: Vec<(String, String)> = serde_urlencoded::from_str(&body).expect("form body");
    let field = |wanted: &str| {
        fields
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    let email = field("email");
    let password = field("password");
    let client = client.map(|Extension(ip)| ip.addr());

    // Before the lookup and before the hash. Both halves matter: the point of
    // refusing early is that the CPU is not spent, and the point of refusing
    // before the lookup is that a locked-out account and a locked-out
    // nonexistent address are one code path.
    if let Some(retry_after) = state.throttle.check(&email, client).retry_after() {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_after.as_secs().to_string())],
            CREDENTIAL_REJECTION,
        )
            .into_response();
    }

    let stored = state.users.get(&email);
    match state.checker.check(stored, password.as_bytes()) {
        CredentialOutcome::Verified => {
            state.throttle.record_success(&email, client);
            (StatusCode::OK, "signed in").into_response()
        }
        // `CredentialOutcome` is `#[non_exhaustive]`, and this arm is written
        // as the wildcard rather than as `Rejected` on purpose: a variant
        // added later lands here, refused, instead of falling through to
        // something a compiler error would have had to be fixed to allow.
        //
        // Recording the failure is unconditional, and in particular does not
        // depend on whether `stored` was `Some`. Recording only for real
        // accounts would mean an unknown address never locks out -- which
        // answers, by observation, the question the single rejection message
        // refuses to answer.
        _ => {
            state.throttle.record_failure(&email, client);
            (StatusCode::UNAUTHORIZED, CREDENTIAL_REJECTION).into_response()
        }
    }
}

fn app(state: AppState) -> Router {
    Router::new().route("/login", post(login)).with_state(state)
}

// ---------------------------------------------------------------------------
// Driving it
// ---------------------------------------------------------------------------

/// What the caller can see of one attempt.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    status: StatusCode,
    body: String,
    /// Present only on a refusal, and compared for presence rather than
    /// value: the number is a clock reading and would differ between two runs
    /// that are otherwise identical.
    throttled: bool,
}

async fn attempt(app: &Router, email: &str, password: &str, from: IpAddr) -> Answer {
    let body = serde_urlencoded::to_string([("email", email), ("password", password)])
        .expect("encode form");

    // No forwarded headers and an empty trusted list, so this resolves to the
    // peer -- the same value the TCP serve path installs, arrived at by the
    // same function rather than by a hand-built extension that could drift
    // from it.
    let ip = ClientIp::resolve(from, &HeaderMap::new(), &TrustedProxies::none());

    let request = Request::builder()
        .method("POST")
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .extension(ip)
        .body(Body::from(body))
        .expect("request");

    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let throttled = response.headers().contains_key(header::RETRY_AFTER);
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");

    Answer {
        status,
        body: String::from_utf8(bytes.to_vec()).expect("utf-8"),
        throttled,
    }
}

fn from(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(203, 0, 113, last))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The floor. Without this, a 401 anywhere below proves only that the handler
/// is broken.
#[tokio::test]
async fn the_right_password_signs_in() {
    let app = app(state(LoginThrottle::new()));
    let answer = attempt(&app, REGISTERED, PASSWORD, from(1)).await;
    assert_eq!(answer.status, StatusCode::OK, "{answer:?}");
}

/// The property the whole file exists for. Five failures against an address
/// with an account and five against one without must produce the same five
/// answers, and the sixth must be the same refusal.
#[tokio::test]
async fn a_nonexistent_address_is_throttled_exactly_like_a_real_one() {
    let app = app(state(LoginThrottle::new().per_identity(3)));

    let mut real = Vec::new();
    let mut absent = Vec::new();
    for _ in 0..4 {
        real.push(attempt(&app, REGISTERED, "wrong", from(1)).await);
        absent.push(attempt(&app, UNKNOWN, "wrong", from(2)).await);
    }

    assert_eq!(
        real, absent,
        "the two addresses were answered differently somewhere in the sequence"
    );
    // And the sequence actually reached a lockout -- otherwise the assertion
    // above would hold for two sequences of identical 401s and prove nothing.
    assert_eq!(real[3].status, StatusCode::TOO_MANY_REQUESTS, "{real:?}");
    assert!(real[3].throttled);
}

/// A lockout that the right password walks through is not a lockout. This is
/// what fixes the throttle's position in the handler: before the check, not
/// after it.
#[tokio::test]
async fn the_right_password_does_not_escape_a_lockout() {
    let app = app(state(LoginThrottle::new().per_identity(2)));

    for _ in 0..2 {
        let answer = attempt(&app, REGISTERED, "wrong", from(1)).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{answer:?}");
    }

    let answer = attempt(&app, REGISTERED, PASSWORD, from(1)).await;
    assert_eq!(
        answer.status,
        StatusCode::TOO_MANY_REQUESTS,
        "guessing correctly got past the throttle"
    );
}

/// The refusal must not become the oracle the message avoids being. A 429
/// says "you have been trying too much", which is true of the client and
/// says nothing about the address.
#[tokio::test]
async fn the_refusal_says_no_more_than_the_rejection_does() {
    let app = app(state(LoginThrottle::new().per_identity(1)));
    let _ = attempt(&app, UNKNOWN, "wrong", from(1)).await;
    let refused = attempt(&app, UNKNOWN, "wrong", from(1)).await;

    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    let message = refused.body.to_lowercase();
    assert!(!message.contains("email"), "{message}");
    assert!(!message.contains("password"), "{message}");
    assert!(!message.contains("account"), "{message}");
    assert!(!message.contains(UNKNOWN), "{message}");
}

/// Two mistakes and then the right password must not leave somebody one
/// mistake from a lockout tomorrow.
#[tokio::test]
async fn signing_in_clears_the_failures_that_came_before_it() {
    let app = app(state(LoginThrottle::new().per_identity(3)));

    for _ in 0..2 {
        let _ = attempt(&app, REGISTERED, "wrong", from(1)).await;
    }
    let signed_in = attempt(&app, REGISTERED, PASSWORD, from(1)).await;
    assert_eq!(signed_in.status, StatusCode::OK);

    // The full allowance again, not the one attempt that was left.
    for _ in 0..3 {
        let answer = attempt(&app, REGISTERED, "wrong", from(1)).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{answer:?}");
    }
}

/// One failure each against many addresses never fills a per-address bucket.
/// The client bucket is the one that sees a spray for what it is.
#[tokio::test]
async fn a_spray_across_addresses_is_stopped() {
    let app = app(state(LoginThrottle::new().per_identity(5).per_address(4)));

    for account in 0..4 {
        let email = format!("user-{account}@example.com");
        let answer = attempt(&app, &email, "wrong", from(1)).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{answer:?}");
    }

    let answer = attempt(&app, "user-99@example.com", "wrong", from(1)).await;
    assert_eq!(
        answer.status,
        StatusCode::TOO_MANY_REQUESTS,
        "the client kept guessing after spending its whole allowance"
    );
    // Somebody else is unaffected, which is what keeps the client bucket from
    // being a way to take the login form down for everyone.
    let elsewhere = attempt(&app, REGISTERED, PASSWORD, from(9)).await;
    assert_eq!(elsewhere.status, StatusCode::OK);
}

/// The other half of "refuse early": a refused attempt must not pay for an
/// Argon2id verification. Otherwise the throttle bounds the guessing and not
/// the CPU, and the login form stays a way to spend the server's time.
#[tokio::test]
async fn a_throttled_attempt_costs_no_hash() {
    let state = state(LoginThrottle::new().per_identity(2));
    let checker = state.checker.clone();
    let app = app(state);

    for _ in 0..2 {
        let _ = attempt(&app, UNKNOWN, "wrong", from(1)).await;
    }
    assert_eq!(
        checker.verifications(),
        2,
        "an unknown address must still pay for a verification while it is allowed"
    );

    for _ in 0..10 {
        let answer = attempt(&app, UNKNOWN, "wrong", from(1)).await;
        assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
    }
    assert_eq!(
        checker.verifications(),
        2,
        "the throttle was consulted after the hash rather than before it"
    );
}
