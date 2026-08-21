//! The redirect vocabulary, end to end through a real session.
//!
//! `redirect().route(..)` and `redirect().with(..)` are the two halves of the
//! Post/Redirect/Get pattern, and both used to be documented as not
//! implemented: the first returned `400`, the second silently dropped its
//! data. Neither could work from `IntoResponse` alone, which sees no request,
//! no route table and no session.
//!
//! The fix is [`RedirectMapper`], a layer above the handler. The unit tests
//! next to it drive it against a stub leaf service. This file is the part
//! those cannot cover: a genuine `SessionLayer` with a real store, and three
//! requests in sequence, because "flashed for exactly one request" is a claim
//! about the *second* and *third* requests rather than the first.

#![cfg(feature = "auth")]

use arcature::auth::{Flash, SessionConfig};
use arcature::http::response::{RedirectResponse, redirect};
use arcature::routing::{RedirectMapper, Route, Routes};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use tower::ServiceExt;
use tower_sessions_memory_store::MemoryStore;

/// A 64-byte signing key. Fixed, because a test that generates one learns
/// nothing and fails differently on each run.
const KEY: &[u8] = &[7u8; 64];

/// Flashes a status message and redirects to a named route.
async fn save() -> RedirectResponse {
    redirect()
        .route("users.show", 7u64)
        .with("status", "Profile updated")
}

/// Echoes the flashed status back, or `-` if there is none.
async fn show(flash: Flash) -> String {
    flash.get("status").unwrap_or("-").to_string()
}

/// The application under test: mapper inside, session outside.
///
/// That nesting is not incidental. The mapper writes flash data through a
/// session handle it lifts out of the request extensions, so the session
/// layer has to have already put one there, and it has to still be above on
/// the way out to save the record and set the cookie.
fn app() -> Router {
    let routes: Routes = Routes::new([
        Route::post("/save", save).name("save"),
        Route::get("/users/{id}", show).name("users.show"),
    ]);
    let table = routes.table();
    let session = SessionConfig::dev(KEY)
        .expect("a 64-byte key is valid")
        .into_layer(MemoryStore::default())
        .expect("the dev session config is valid");

    routes
        .into_router()
        .layer(RedirectMapper::new(table))
        .layer(session)
}

/// The `Set-Cookie` the session layer issued, in a form that can be sent back
/// as a `Cookie` header.
fn cookie(response: &Response<Body>) -> String {
    response
        .headers()
        .get(header::SET_COOKIE)
        .expect("the session layer set a cookie")
        .to_str()
        .expect("cookies are ASCII")
        .split(';')
        .next()
        .expect("split always yields one element")
        .to_string()
}

async fn body(response: Response<Body>) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("the test bodies are tiny");
    String::from_utf8(bytes.to_vec()).expect("the handler returns UTF-8")
}

#[tokio::test]
async fn a_flashed_redirect_resolves_its_name_and_survives_exactly_one_request() {
    let app = app();

    // 1. The POST. The name resolves to a path, and the session picks up the
    //    flash data on the way out.
    let posted = app
        .clone()
        .oneshot(
            Request::post("/save")
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router is infallible");

    assert_eq!(
        posted.headers().get(header::LOCATION).unwrap(),
        "/users/7",
        "`route(\"users.show\", 7)` should resolve against the table"
    );
    let session = cookie(&posted);

    // 2. The GET the browser follows to. The flash is readable here and
    //    nowhere else.
    let followed = app
        .clone()
        .oneshot(
            Request::get("/users/7")
                .header(header::COOKIE, &session)
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router is infallible");

    assert_eq!(followed.status(), StatusCode::OK);
    assert_eq!(body(followed).await, "Profile updated");

    // 3. A reload of the same page. Reading the flash consumed it, so a
    //    refresh must not show the toast a second time.
    let reloaded = app
        .oneshot(
            Request::get("/users/7")
                .header(header::COOKIE, &session)
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router is infallible");

    assert_eq!(body(reloaded).await, "-", "flash data outlived its request");
}

#[tokio::test]
async fn a_second_session_cannot_read_the_first_session_flash() {
    let app = app();

    let posted = app
        .clone()
        .oneshot(
            Request::post("/save")
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router is infallible");
    let _ = cookie(&posted);

    // No cookie: a different visitor entirely.
    let stranger = app
        .oneshot(
            Request::get("/users/7")
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router is infallible");

    assert_eq!(
        body(stranger).await,
        "-",
        "flash data leaked across sessions"
    );
}
