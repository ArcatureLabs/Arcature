//! Changing a password signs the other sessions out.
//!
//! The mechanism under test is a stamp: login writes a digest of the user's
//! stored credential into the session, and every authenticated request
//! compares it against the credential the row holds now. What makes that
//! worth an integration test rather than a unit test is that the claim is
//! about a *sequence of requests* sharing a session store -- "log in, change
//! the password elsewhere, come back" is not a shape a unit test of any one
//! function can express.
//!
//! There is no per-user index in a session store, which is the constraint the
//! whole design is bent around: `DELETE FROM sessions WHERE user = ?` is not a
//! query `tower-sessions` can answer, on any backend. So the check runs on the
//! way in, against the session the browser already carries, and works the same
//! on `MemoryStore` as on a database store.

#![cfg(feature = "auth")]

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use arcature::auth::SessionConfig;
use arcature::{Auth, AuthManager, AuthUser, UserLoader};
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::routing::{get, post};
use axum::{Router, response::IntoResponse};
use tower::ServiceExt;
use tower_sessions_memory_store::MemoryStore;

// ---------------------------------------------------------------------------
// A user whose password lives in application state
// ---------------------------------------------------------------------------

/// One row of the fake user table.
#[derive(Clone)]
struct Row {
    password_hash: String,
    /// Whether this application has adopted `stored_credential` yet.
    ///
    /// A flag rather than two user types, because the interesting case is the
    /// *transition*: a session created while the application returned `None`
    /// and read back after it started returning `Some`. That is what an
    /// upgrade looks like from the session's point of view, and it is the one
    /// case where the mechanism deliberately does not sign anybody out.
    stamped: bool,
}

#[derive(Clone)]
struct AppState {
    users: Arc<Mutex<HashMap<i64, Row>>>,
}

impl AppState {
    fn with_user(id: i64, password_hash: &str, stamped: bool) -> Self {
        let mut users = HashMap::new();
        users.insert(
            id,
            Row {
                password_hash: password_hash.to_owned(),
                stamped,
            },
        );
        Self {
            users: Arc::new(Mutex::new(users)),
        }
    }

    fn set_password(&self, id: i64, password_hash: &str) {
        let mut users = self.users.lock().expect("state lock");
        users.get_mut(&id).expect("user exists").password_hash = password_hash.to_owned();
    }

    fn adopt_stamping(&self, id: i64) {
        let mut users = self.users.lock().expect("state lock");
        users.get_mut(&id).expect("user exists").stamped = true;
    }

    fn read(&self, id: i64) -> Option<User> {
        let users = self.users.lock().expect("state lock");
        users.get(&id).map(|row| User {
            id,
            password_hash: row.password_hash.clone(),
            stamped: row.stamped,
        })
    }
}

struct User {
    id: i64,
    password_hash: String,
    stamped: bool,
}

impl AuthUser for User {
    type Id = i64;

    fn id(&self) -> &i64 {
        &self.id
    }

    fn stored_credential(&self) -> Option<&[u8]> {
        self.stamped.then_some(self.password_hash.as_bytes())
    }
}

impl UserLoader<AppState> for User {
    type Error = Infallible;

    async fn load_user(id: &i64, state: &AppState) -> Result<Option<Self>, Infallible> {
        Ok(state.read(*id))
    }
}

// ---------------------------------------------------------------------------
// The application
// ---------------------------------------------------------------------------

const USER_ID: i64 = 7;

async fn login(State(state): State<AppState>, auth: AuthManager<User>) -> impl IntoResponse {
    let user = state.read(USER_ID).expect("user exists");
    auth.login(&user).await.expect("login");
    StatusCode::OK
}

async fn me(Auth(user): Auth<User>) -> impl IntoResponse {
    user.id.to_string()
}

/// Change the password and say nothing to the session, which is the whole
/// point: the other sessions have to fall over on their own.
async fn change_password(State(state): State<AppState>) -> impl IntoResponse {
    state.set_password(USER_ID, "$argon2id$v=19$second");
    StatusCode::OK
}

/// Change the password and keep *this* session, the way a settings form has
/// to if it is not going to log the user out of the page they are on.
async fn change_password_keeping_me(
    State(state): State<AppState>,
    auth: AuthManager<User>,
) -> impl IntoResponse {
    state.set_password(USER_ID, "$argon2id$v=19$second");
    // Re-read: the point of the call is to stamp the *new* hash, and the user
    // object from before the update carries the old one.
    let reloaded = state.read(USER_ID).expect("user exists");
    auth.rebind_credential(&reloaded).await.expect("rebind");
    StatusCode::OK
}

async fn adopt_stamping(State(state): State<AppState>) -> impl IntoResponse {
    state.adopt_stamping(USER_ID);
    StatusCode::OK
}

fn app(state: AppState) -> Router {
    let layer = SessionConfig::dev(&[7u8; 64])
        .expect("session config")
        .into_layer(MemoryStore::default())
        .expect("session layer");

    Router::new()
        .route("/login", post(login))
        .route("/me", get(me))
        .route("/change-password", post(change_password))
        .route(
            "/change-password-keeping-me",
            post(change_password_keeping_me),
        )
        .route("/adopt-stamping", post(adopt_stamping))
        .with_state(state)
        .layer(layer)
}

// ---------------------------------------------------------------------------
// Driving it
// ---------------------------------------------------------------------------

/// A browser: one cookie jar, reduced to the single cookie this application
/// sets.
struct Client {
    cookie: Option<String>,
}

impl Client {
    fn new() -> Self {
        Self { cookie: None }
    }

    async fn send(&mut self, app: &Router, method: &str, path: &str) -> (StatusCode, String) {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(cookie) = &self.cookie {
            request = request.header(header::COOKIE, cookie);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).expect("request"))
            .await
            .expect("infallible");

        if let Some(set) = response.headers().get(header::SET_COOKIE) {
            let set = set.to_str().expect("ascii cookie");
            let pair = set.split(';').next().expect("cookie pair").to_owned();
            self.cookie = Some(pair);
        }

        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        (status, String::from_utf8(body.to_vec()).expect("utf-8"))
    }

    async fn log_in(&mut self, app: &Router) {
        let (status, _) = self.send(app, "POST", "/login").await;
        assert_eq!(status, StatusCode::OK, "login failed");
        assert!(self.cookie.is_some(), "login set no session cookie");
    }

    async fn me(&mut self, app: &Router) -> StatusCode {
        self.send(app, "GET", "/me").await.0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_signed_in_session_works_before_anything_changes() {
    // The floor under every other test here: if this one is red, a 401
    // elsewhere proves nothing about credential changes.
    let state = AppState::with_user(USER_ID, "$argon2id$v=19$first", true);
    let app = app(state);

    let mut client = Client::new();
    client.log_in(&app).await;

    let (status, body) = client.send(&app, "GET", "/me").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "7");
}

#[tokio::test]
async fn a_password_change_signs_the_session_out() {
    let state = AppState::with_user(USER_ID, "$argon2id$v=19$first", true);
    let app = app(state);

    let mut client = Client::new();
    client.log_in(&app).await;
    assert_eq!(client.me(&app).await, StatusCode::OK);

    client.send(&app, "POST", "/change-password").await;

    assert_eq!(
        client.me(&app).await,
        StatusCode::UNAUTHORIZED,
        "the session outlived the credential it was issued against"
    );
}

#[tokio::test]
async fn the_session_stays_out_after_it_is_signed_out() {
    // The invalidated session is flushed, not merely rejected once. A session
    // that answered 401 and then recovered on the next request would be worse
    // than no mechanism at all, because the 401 would read as proof.
    let state = AppState::with_user(USER_ID, "$argon2id$v=19$first", true);
    let app = app(state);

    let mut client = Client::new();
    client.log_in(&app).await;
    client.send(&app, "POST", "/change-password").await;

    assert_eq!(client.me(&app).await, StatusCode::UNAUTHORIZED);
    assert_eq!(client.me(&app).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rebinding_keeps_this_session_and_drops_the_others() {
    // The asymmetry is the feature. "Change my password and sign my other
    // devices out" is one call, and the device in front of the user is the
    // one that must not be signed out by it.
    let state = AppState::with_user(USER_ID, "$argon2id$v=19$first", true);
    let app = app(state);

    let mut here = Client::new();
    let mut elsewhere = Client::new();
    here.log_in(&app).await;
    elsewhere.log_in(&app).await;
    assert_eq!(here.me(&app).await, StatusCode::OK);
    assert_eq!(elsewhere.me(&app).await, StatusCode::OK);

    here.send(&app, "POST", "/change-password-keeping-me").await;

    assert_eq!(
        here.me(&app).await,
        StatusCode::OK,
        "the session that changed the password was signed out by its own change"
    );
    assert_eq!(
        elsewhere.me(&app).await,
        StatusCode::UNAUTHORIZED,
        "the other device kept working after the password changed"
    );
}

#[tokio::test]
async fn an_application_that_stores_no_credential_is_unaffected() {
    // The trait method defaults to `None`, and a build that never overrides it
    // must behave exactly as it did before the method existed -- including
    // through a password change, which such an application has not asked
    // anybody to notice.
    let state = AppState::with_user(USER_ID, "$argon2id$v=19$first", false);
    let app = app(state);

    let mut client = Client::new();
    client.log_in(&app).await;
    client.send(&app, "POST", "/change-password").await;

    assert_eq!(client.me(&app).await, StatusCode::OK);
}

#[tokio::test]
async fn adopting_the_stamp_does_not_sign_existing_users_out() {
    // The upgrade trade, stated as a test so that changing it is a deliberate
    // act. A session created before the application returned a credential has
    // nothing to compare against; signing it out would log out every user of
    // a deployment on the day it upgrades. So the first authenticated request
    // stamps it instead -- and a change *after* that point is caught normally,
    // which is the second half of the assertion and the part that makes the
    // first half acceptable.
    let state = AppState::with_user(USER_ID, "$argon2id$v=19$first", false);
    let app = app(state.clone());

    let mut client = Client::new();
    client.log_in(&app).await;

    client.send(&app, "POST", "/adopt-stamping").await;
    assert_eq!(
        client.me(&app).await,
        StatusCode::OK,
        "adopting the stamp logged out a session that was already valid"
    );

    client.send(&app, "POST", "/change-password").await;
    assert_eq!(
        client.me(&app).await,
        StatusCode::UNAUTHORIZED,
        "the session picked up no stamp on the way through, so nothing was armed"
    );
}
