//! What the test kit promises.
//!
//! Two things are pinned here.
//!
//! **That the harness drives a real application.** Every request below goes
//! through an `axum::Router` as a `tower::Service`, with no socket bound, so
//! what these tests exercise is the same dispatch a served request takes.
//!
//! **That no assertion can pass vacuously.** For each assertion there is a
//! test that the assertion *fails* when the thing it names is absent. An
//! assertion that quietly passes on a missing prop or an empty error bag is
//! worse than having no assertion, because the suite then reports success
//! for work that was never checked. Those tests catch the panic and read the
//! message, which also pins that the message names the value actually seen.

#![cfg(feature = "test-kit")]

use arcature::test_kit::TestApp;
use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde_json::json;

/// Catch a panicking assertion and return its message.
///
/// The panic hook is silenced for the duration: these tests expect the panic,
/// and a suite that prints a backtrace for every expected failure trains the
/// reader to ignore backtraces.
fn failure_message(assertion: impl FnOnce()) -> String {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(assertion));
    std::panic::set_hook(previous);
    let payload = outcome.expect_err("the assertion should have failed");
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "(panic payload was not a string)".to_owned())
}

/// A router covering the shapes the assertions read.
fn router() -> Router {
    Router::new()
        .route("/", get(|| async { "home" }))
        .route(
            "/users",
            get(|| async {
                axum::Json(json!({
                    "users": [{ "email": "ada@example.com", "roles": ["admin"] }],
                    "total": 1
                }))
            }),
        )
        .route("/missing", get(|| async { StatusCode::NOT_FOUND }))
        .route(
            "/echo",
            post(|headers: axum::http::HeaderMap, body: String| async move {
                let content_type = headers
                    .get(axum::http::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_owned();
                axum::Json(json!({ "content_type": content_type, "body": body }))
            }),
        )
        .route(
            "/old",
            get(|| async { (StatusCode::FOUND, [("location", "/new")]).into_response() }),
        )
        .route("/new", get(|| async { "arrived" }))
        .route(
            "/loop",
            get(|| async { (StatusCode::FOUND, [("location", "/loop")]).into_response() }),
        )
}

#[tokio::test]
async fn a_request_is_dispatched_in_process_without_binding_a_socket() {
    let app = TestApp::from_router(router());
    let response = app.get("/").send().await;
    response.assert_ok();
    assert_eq!(response.text(), "home");
}

#[tokio::test]
async fn every_verb_reaches_its_route() {
    let app = TestApp::from_router(router());
    app.get("/").send().await.assert_ok();
    app.head("/").send().await.assert_ok();
    app.post("/echo")
        .body("text/plain", "hi")
        .send()
        .await
        .assert_ok();
}

#[tokio::test]
async fn a_status_mismatch_reports_the_status_it_saw_and_the_body() {
    let app = TestApp::from_router(router());
    let response = app.get("/missing").send().await;
    let message = failure_message(|| {
        response.assert_status(StatusCode::OK);
    });
    assert!(message.contains("404"), "message: {message}");
    assert!(
        message.contains("expected status 200"),
        "message: {message}"
    );
}

#[tokio::test]
async fn a_json_path_reads_through_objects_and_arrays() {
    let app = TestApp::from_router(router());
    let response = app.get("/users").send().await;
    response.assert_json_path("total", 1);
    response.assert_json_path("users.0.email", "ada@example.com");
    response.assert_json_path("users.0.roles.0", "admin");
}

#[tokio::test]
async fn a_missing_json_path_fails_and_names_where_it_stopped() {
    let app = TestApp::from_router(router());
    let response = app.get("/users").send().await;
    let message = failure_message(|| {
        response.assert_json_path("users.0.name", "Ada");
    });
    assert!(message.contains("users.0.name"), "message: {message}");
    assert!(
        message.contains("`name` is missing at `users.0`"),
        "message: {message}"
    );
    assert!(message.contains("email"), "message: {message}");
}

#[tokio::test]
async fn a_wrong_json_value_reports_both_values() {
    let app = TestApp::from_router(router());
    let response = app.get("/users").send().await;
    let message = failure_message(|| {
        response.assert_json_path("total", 2);
    });
    assert!(message.contains("expected 2"), "message: {message}");
    assert!(message.contains("got 1"), "message: {message}");
}

#[tokio::test]
async fn json_path_returns_none_rather_than_panicking_when_asked_directly() {
    let app = TestApp::from_router(router());
    let response = app.get("/users").send().await;
    assert!(response.json_path("users.0.name").is_none());
    assert_eq!(response.json_path("total"), Some(json!(1)));
}

#[tokio::test]
async fn a_json_body_is_sent_with_its_content_type() {
    let app = TestApp::from_router(router());
    let response = app
        .post("/echo")
        .json(&json!({ "email": "ada@example.com" }))
        .send()
        .await;
    response.assert_json_path("content_type", "application/json");
    response.assert_json_path("body", "{\"email\":\"ada@example.com\"}");
}

#[tokio::test]
async fn a_form_body_is_percent_encoded_the_way_a_browser_sends_it() {
    let app = TestApp::from_router(router());
    let response = app
        .post("/echo")
        .form(&json!({ "email": "ada@example.com", "note": "a b&c" }))
        .send()
        .await;
    response.assert_json_path("content_type", "application/x-www-form-urlencoded");
    let body = response
        .json_path("body")
        .expect("the echo route returns the body");
    let body = body.as_str().expect("the body is a string");
    let decoded: std::collections::BTreeMap<String, String> =
        serde_urlencoded::from_str(body).expect("the harness must emit decodable form data");
    assert_eq!(decoded["email"], "ada@example.com");
    assert_eq!(decoded["note"], "a b&c");
}

#[tokio::test]
async fn a_form_array_becomes_a_repeated_key() {
    let app = TestApp::from_router(router());
    let response = app
        .post("/echo")
        .form(&json!({ "role": ["admin", "editor"] }))
        .send()
        .await;
    let body = response.json_path("body").expect("body");
    assert_eq!(body.as_str().expect("string"), "role=admin&role=editor");
}

#[tokio::test]
async fn a_header_assertion_lists_what_was_sent_when_the_header_is_absent() {
    let app = TestApp::from_router(router());
    let response = app.get("/").send().await;
    let message = failure_message(|| {
        response.assert_header("x-request-id", "abc");
    });
    assert!(message.contains("x-request-id"), "message: {message}");
    assert!(message.contains("content-type"), "message: {message}");
}

#[tokio::test]
async fn a_redirect_is_asserted_by_its_destination() {
    let app = TestApp::from_router(router());
    app.get("/old").send().await.assert_redirect("/new");
}

#[tokio::test]
async fn a_response_that_does_not_redirect_fails_the_redirect_assertion() {
    let app = TestApp::from_router(router());
    let response = app.get("/").send().await;
    let message = failure_message(|| {
        response.assert_redirect("/new");
    });
    assert!(message.contains("no redirect header"), "message: {message}");
    assert!(message.contains("200"), "message: {message}");
}

#[tokio::test]
async fn a_redirect_elsewhere_reports_where_it_actually_went() {
    let app = TestApp::from_router(router());
    let response = app.get("/old").send().await;
    let message = failure_message(|| {
        response.assert_redirect("/elsewhere");
    });
    assert!(message.contains("got `/new`"), "message: {message}");
}

#[tokio::test]
async fn following_redirects_returns_the_final_response() {
    let app = TestApp::from_router(router());
    let response = app.get("/old").follow_redirects().send().await;
    response.assert_ok();
    assert_eq!(response.text(), "arrived");
}

// Not a `#[tokio::test]`: this one drives its own runtime through
// `block_on`, which cannot be nested inside another.
#[test]
fn a_redirect_loop_fails_rather_than_hanging() {
    let message = failure_message(|| {
        arcature::test_kit::block_on(async {
            TestApp::from_router(router())
                .get("/loop")
                .follow_redirects()
                .send()
                .await
        });
    });
    assert!(message.contains("redirect loop"), "message: {message}");
}

#[cfg(feature = "inertia")]
mod inertia {
    use super::{TestApp, failure_message};
    use axum::Router;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use serde_json::json;

    /// The page object both transports carry.
    fn page() -> serde_json::Value {
        json!({
            "component": "users/index",
            "props": { "users": [{ "email": "ada@example.com" }], "errors": {} },
            "url": "/users",
            "version": "1"
        })
    }

    /// The same page inside a root document, escaped the way the framework
    /// escapes it (`<`, `>`, `&`, and `/` as JSON escapes).
    fn root_document() -> String {
        let json = page().to_string().replace('/', r"\/");
        format!(
            "<!doctype html><html><body><script data-page=\"app\" type=\"application/json\">{json}</script><div id=\"app\"></div></body></html>"
        )
    }

    fn router() -> Router {
        Router::new()
            .route(
                "/users",
                get(|| async {
                    (
                        [("x-inertia", "true"), ("content-type", "application/json")],
                        page().to_string(),
                    )
                        .into_response()
                }),
            )
            .route(
                "/users-html",
                get(|| async {
                    (
                        [("content-type", "text/html; charset=utf-8")],
                        root_document(),
                    )
                        .into_response()
                }),
            )
            .route("/plain", get(|| async { "not a page" }))
    }

    #[tokio::test]
    async fn a_page_object_is_read_from_an_inertia_visit() {
        let app = TestApp::from_router(router());
        let response = app.get("/users").inertia().send().await;
        response.assert_inertia_component("users/index");
        response.assert_inertia_prop("users.0.email", "ada@example.com");
    }

    #[tokio::test]
    async fn a_page_object_is_read_out_of_the_root_document_on_a_first_load() {
        let app = TestApp::from_router(router());
        let response = app.get("/users-html").send().await;
        response.assert_inertia_component("users/index");
        response.assert_inertia_prop("users.0.email", "ada@example.com");
    }

    #[tokio::test]
    async fn a_wrong_component_reports_the_one_that_was_rendered() {
        let app = TestApp::from_router(router());
        let response = app.get("/users").inertia().send().await;
        let message = failure_message(|| {
            response.assert_inertia_component("users/show");
        });
        assert!(message.contains("got `users/index`"), "message: {message}");
    }

    #[tokio::test]
    async fn a_missing_prop_fails_rather_than_passing_quietly() {
        let app = TestApp::from_router(router());
        let response = app.get("/users").inertia().send().await;
        let message = failure_message(|| {
            response.assert_inertia_prop("users.0.name", "Ada");
        });
        assert!(message.contains("does not exist"), "message: {message}");
        assert!(message.contains("email"), "message: {message}");
    }

    #[tokio::test]
    async fn a_response_that_is_not_a_page_fails_the_page_assertions() {
        let app = TestApp::from_router(router());
        let response = app.get("/plain").send().await;
        let message = failure_message(|| {
            response.assert_inertia_component("users/index");
        });
        assert!(
            message.contains("no Inertia page object"),
            "message: {message}"
        );
    }
}

#[cfg(feature = "api")]
mod problems {
    use super::{TestApp, failure_message};
    use arcature::api::{Problem, ProblemKind};
    use axum::Router;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use serde_json::json;

    fn router() -> Router {
        Router::new()
            .route(
                "/missing",
                get(|| async { Problem::of(ProblemKind::NotFound).into_response() }),
            )
            .route(
                "/invalid",
                get(|| async {
                    Problem::of(ProblemKind::Validation)
                        .with_extension("errors", json!({ "email": [{ "code": "email" }] }))
                        .into_response()
                }),
            )
            .route(
                "/plain-404",
                get(|| async { (StatusCode::NOT_FOUND, "nope").into_response() }),
            )
    }

    #[tokio::test]
    async fn a_problem_response_matches_its_kind() {
        let app = TestApp::from_router(router());
        app.get("/missing")
            .send()
            .await
            .assert_problem(ProblemKind::NotFound);
    }

    #[tokio::test]
    async fn a_plain_error_page_with_the_same_status_is_not_a_problem_document() {
        let app = TestApp::from_router(router());
        let response = app.get("/plain-404").send().await;
        let message = failure_message(|| {
            response.assert_problem(ProblemKind::NotFound);
        });
        assert!(
            message.contains("application/problem+json"),
            "message: {message}"
        );
    }

    #[tokio::test]
    async fn a_validation_problem_names_the_field_that_failed() {
        let app = TestApp::from_router(router());
        app.get("/invalid")
            .send()
            .await
            .assert_validation_error("email");
    }

    #[tokio::test]
    async fn a_field_that_did_not_fail_is_not_reported_as_failing() {
        let app = TestApp::from_router(router());
        let response = app.get("/invalid").send().await;
        let message = failure_message(|| {
            response.assert_validation_error("name");
        });
        assert!(message.contains("the bag holds"), "message: {message}");
        assert!(message.contains("email"), "message: {message}");
    }

    #[tokio::test]
    async fn a_response_with_no_error_bag_fails_the_validation_assertion() {
        let app = TestApp::from_router(router());
        let response = app.get("/missing").send().await;
        let message = failure_message(|| {
            response.assert_validation_error("email");
        });
        assert!(message.contains("no error bag"), "message: {message}");
    }
}

#[cfg(feature = "auth")]
mod sessions {
    use super::{TestApp, failure_message};
    use arcature::auth::AuthUser;
    use arcature::auth::tower_sessions::cookie::Key;
    use arcature::auth::tower_sessions::{Session, SessionManagerLayer};
    use arcature::test_kit::TestSessions;
    use axum::Router;
    use axum::routing::get;

    /// A 64-byte key: `cookie::Key` needs exactly that much material.
    const SIGNING_KEY: &[u8] = &[7u8; 64];
    const COOKIE_NAME: &str = "arcature_session";

    struct User {
        id: i64,
    }

    impl AuthUser for User {
        type Id = i64;
        fn id(&self) -> &i64 {
            &self.id
        }
    }

    /// The application under test, plus the sessions it was built with.
    fn app() -> (TestApp, TestSessions) {
        let sessions = TestSessions::new(COOKIE_NAME, SIGNING_KEY).expect("a 64-byte key is valid");
        let layer = SessionManagerLayer::new(sessions.store())
            .with_name(COOKIE_NAME)
            .with_signed(Key::from(SIGNING_KEY));
        let router = Router::new()
            .route(
                "/whoami",
                get(|session: Session| async move {
                    let id: Option<i64> = session.get("user_id").await.unwrap_or(None);
                    id.map_or_else(|| "anonymous".to_owned(), |id| id.to_string())
                }),
            )
            .route(
                "/flash",
                get(|session: Session| async move {
                    let value: Option<String> = session.get("flash").await.unwrap_or(None);
                    value.unwrap_or_else(|| "(none)".to_owned())
                }),
            )
            .layer(layer);
        (
            TestApp::from_router(router).with_sessions(sessions.clone()),
            sessions,
        )
    }

    #[tokio::test]
    async fn acting_as_arrives_as_that_user() {
        let (app, _sessions) = app();
        let response = app.get("/whoami").acting_as(&User { id: 42 }).send().await;
        response.assert_ok();
        assert_eq!(response.text(), "42");
    }

    #[tokio::test]
    async fn a_request_without_acting_as_arrives_anonymous() {
        let (app, _sessions) = app();
        let response = app.get("/whoami").send().await;
        assert_eq!(response.text(), "anonymous");
    }

    #[tokio::test]
    async fn a_seeded_session_value_is_visible_to_the_handler() {
        let (app, _sessions) = app();
        let response = app
            .get("/flash")
            .with_session("flash", "saved")
            .send()
            .await;
        assert_eq!(response.text(), "saved");
    }

    // Drives its own runtime, so not a `#[tokio::test]`.
    #[test]
    fn seeding_a_session_without_a_store_fails_loudly_rather_than_sending_it_anonymous() {
        let bare = TestApp::from_router(Router::new().route("/", get(|| async { "hi" })));
        let message = failure_message(|| {
            arcature::test_kit::block_on(async {
                bare.get("/").acting_as(&User { id: 1 }).send().await
            });
        });
        assert!(message.contains("session store"), "message: {message}");
    }

    #[tokio::test]
    async fn a_signing_key_of_the_wrong_length_is_refused() {
        let error = TestSessions::new(COOKIE_NAME, &[1u8; 16])
            .expect_err("a 16-byte key cannot sign a cookie");
        assert!(error.to_string().contains("64 bytes"), "{error}");
    }
}

#[tokio::test]
async fn the_real_socket_mode_answers_over_tcp() {
    // The in-process path cannot exercise a protocol upgrade, so the harness
    // can also bind a port. This pins that the served router is the same one.
    let app = TestApp::from_router(router());
    let server = app.serve().await.expect("an ephemeral port should bind");
    let body = reqwest::get(format!("{}/", server.base_url()))
        .await
        .expect("the served app should answer")
        .text()
        .await
        .expect("a body");
    assert_eq!(body, "home");
    assert!(server.ws_url("/socket").starts_with("ws://"));
}

#[cfg(feature = "events")]
#[tokio::test]
async fn an_event_recorder_reports_what_was_dispatched() {
    use arcature::events::{DispatchError, Dispatcher, Event};
    use arcature::test_kit::Events;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct UserRegistered {
        email: String,
    }
    impl arcature::DxComponent for UserRegistered {
        const NAME: &'static str = "UserRegistered";
    }
    impl Event for UserRegistered {}

    let events = Events::fake().register(|_event: UserRegistered| async { Ok(()) });
    let dispatcher: Dispatcher = events.dispatcher();
    dispatcher
        .dispatch(&UserRegistered {
            email: "ada@example.com".to_owned(),
        })
        .await
        .map_err(|error: DispatchError| error.to_string())
        .expect("dispatch should succeed");

    events.assert_dispatched("UserRegistered");
    let message = failure_message(|| {
        events.assert_dispatched("UserDeleted");
    });
    assert!(message.contains("UserRegistered"), "message: {message}");
}

#[cfg(feature = "mail")]
mod mail {
    use super::failure_message;
    use arcature::mail::{Email, EmailError, Mailable, lettre};
    use arcature::test_kit::Mail;

    struct Welcome;

    impl Mailable for Welcome {
        fn build(&self, email: Email) -> Result<lettre::Message, EmailError> {
            email.subject("Welcome").plain("hello")
        }
    }

    /// Send one message through a fake mailer.
    async fn send_one(mail: &Mail) {
        mail.sender("noreply@example.com")
            .expect("a valid from address")
            .to("ada@example.com")
            .send(&Welcome)
            .await
            .expect("the capture transport always succeeds");
    }

    #[tokio::test]
    async fn a_mail_recorder_reports_what_was_sent() {
        let mail = Mail::fake();
        mail.assert_no_mail_sent().await;
        send_one(&mail).await;
        mail.assert_mail_sent("ada@example.com").await;
        mail.assert_mail_contains("ada@example.com", "Welcome")
            .await;
    }

    // Drives its own runtime, so not a `#[tokio::test]`.
    #[test]
    fn mail_to_another_address_does_not_satisfy_the_assertion() {
        let message = failure_message(|| {
            arcature::test_kit::block_on(async {
                let mail = Mail::fake();
                send_one(&mail).await;
                mail.assert_mail_sent("grace@example.com").await;
            });
        });
        assert!(message.contains("ada@example.com"), "message: {message}");
    }

    // Drives its own runtime, so not a `#[tokio::test]`.
    #[test]
    fn an_empty_mailbox_fails_the_sent_assertion_rather_than_passing() {
        let message = failure_message(|| {
            arcature::test_kit::block_on(async {
                Mail::fake().assert_mail_sent("ada@example.com").await;
            });
        });
        assert!(message.contains("nothing was sent"), "message: {message}");
    }
}

#[cfg(feature = "database")]
mod database {
    use arcature::database::sqlx;
    use arcature::test_kit::{TestDatabase, assert_database_has};

    // These need a live PostgreSQL named by `ARCATURE_TEST_DB_URL`, so they
    // are ignored by default. They are not `skip`ped at runtime: an ignored
    // test is visibly ignored in the output, while a test that returns early
    // because a variable is unset reports a pass for work it never did.
    // Run them with:
    //   ARCATURE_TEST_DB_URL=postgres://.../arcature_test_kit \
    //     cargo test --features test-kit,db-postgres --test test_kit -- --ignored

    #[tokio::test]
    #[ignore = "needs a live postgres named by ARCATURE_TEST_DB_URL"]
    async fn a_transaction_rolls_back_when_it_is_dropped() {
        let database = TestDatabase::connect()
            .await
            .expect("ARCATURE_TEST_DB_URL must name a test database");
        {
            let mut transaction = database.begin().await.expect("begin");
            sqlx::query("CREATE TEMPORARY TABLE probe (name text)")
                .execute(transaction.connection())
                .await
                .expect("create");
            sqlx::query("INSERT INTO probe (name) VALUES ('ada')")
                .execute(transaction.connection())
                .await
                .expect("insert");
            assert_database_has(transaction.connection(), "probe", &[("name", "ada")]).await;
        }
        let mut next = database.begin().await.expect("begin");
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_tables WHERE tablename = 'probe')")
                .fetch_one(next.connection())
                .await
                .expect("query");
        assert!(!exists, "the dropped transaction must have rolled back");
    }

    // Drives its own runtime, so not a `#[tokio::test]`.
    #[test]
    #[ignore = "needs a live postgres named by ARCATURE_TEST_DB_URL"]
    fn a_row_that_does_not_match_fails_the_assertion() {
        let message = super::failure_message(|| {
            arcature::test_kit::block_on(async {
                let database = TestDatabase::connect()
                    .await
                    .expect("ARCATURE_TEST_DB_URL must name a test database");
                let mut transaction = database.begin().await.expect("begin");
                sqlx::query("CREATE TEMPORARY TABLE probe (name text, city text)")
                    .execute(transaction.connection())
                    .await
                    .expect("create");
                sqlx::query("INSERT INTO probe (name, city) VALUES ('ada', 'london')")
                    .execute(transaction.connection())
                    .await
                    .expect("insert");
                assert_database_has(
                    transaction.connection(),
                    "probe",
                    &[("name", "ada"), ("city", "paris")],
                )
                .await;
            });
        });
        assert!(message.contains("holds 1 rows"), "message: {message}");
        assert!(
            message.contains("city = `paris` matches 0 rows"),
            "message: {message}"
        );
    }
}

/// The `#[arcature::test]` attribute, exercised as a user writes it.
///
/// Reachable here as `#[arcature::test_kit::test]` because the crate root
/// does not yet re-export the macro; see the note in `test_kit`.
#[arcature::test_kit::test(app = router())]
async fn the_attribute_boots_the_app_and_hands_it_to_the_body(app: TestApp) {
    let response = app.get("/").send().await;
    response.assert_ok();
    assert_eq!(response.text(), "home");
}

/// The same, with a fallible body: the return type reaches the generated
/// `#[test]`, so `?` works.
#[arcature::test_kit::test(app = router())]
async fn a_fallible_test_body_can_use_the_question_mark(
    app: TestApp,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = app.get("/users").send().await;
    let total = response.json_path("total").ok_or("total is missing")?;
    assert_eq!(total, serde_json::json!(1));
    Ok(())
}

// --- stateful applications -------------------------------------------------

#[tokio::test]
async fn a_stateful_application_is_booted_with_the_state_it_runs_on() {
    // `with_state` is the seam a generated app needs. Its router carries an
    // `AppState`, so the harness cannot drive it until the test hands over a
    // state -- the harness will not invent one.
    #[derive(Clone)]
    struct AppState {
        greeting: &'static str,
    }

    async fn greet(axum::extract::State(state): axum::extract::State<AppState>) -> &'static str {
        state.greeting
    }

    let application = arcature::Application::<AppState>::new()
        .routes(arcature::Routes::new([arcature::Route::get("/", greet)]))
        .build();
    let app = TestApp::with_state(
        application,
        AppState {
            greeting: "stateful",
        },
    );

    let response = app.get("/").send().await;
    response.assert_ok();
    assert_eq!(response.text(), "stateful");
}
