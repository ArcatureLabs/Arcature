//! Application pipeline contracts.
//!
//! Two things are pinned here.
//!
//! **That the pipeline can be installed at all.** Before this, `InertiaLayer`,
//! `SessionLayer` and `CsrfLayer` were all written and none of them could be
//! attached: `ApplicationBuilder` had no `.layer()`, and the only path to one
//! (`Routes::into_router().layer(..)`) produced an `axum::Router` the builder
//! would not take back. A scaffolded app answered `500 inertia adapter error`
//! on its own home page. `inertia_extractor_works_once_the_layer_is_installed`
//! is that bug, as a test.
//!
//! **That the order is the documented order.** `crate::application::pipeline`
//! states an order and gives a reason for each position. Order that is only a
//! comment drifts, so the observable order is asserted here — including that
//! it does *not* depend on the order the builder methods were called in.

use arcature::routing::{Route, Routes};
use arcature::{Application, InertiaConfig, default_root_document};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use std::task::{Context, Poll};
use tower::{Layer, Service, ServiceExt};

// --- a layer that records itself ------------------------------------------
//
// Each instance appends its name to `x-order` on the way out. A response
// travels outward through the stack, so the innermost layer appends first:
// reading `x-order` front to back reads the pipeline inside to outside.

#[derive(Clone)]
struct Mark(&'static str);

impl<S> Layer<S> for Mark {
    type Service = MarkService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        MarkService { inner, name: self.0 }
    }
}

#[derive(Clone)]
struct MarkService<S> {
    inner: S,
    name: &'static str,
}

impl<S> Service<Request<Body>> for MarkService<S>
where
    S: Service<Request<Body>, Response = Response, Error = std::convert::Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = std::convert::Infallible;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let name = self.name;
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let mut response = inner.call(request).await?;
            response
                .headers_mut()
                .append("x-order", name.parse().expect("header value"));
            Ok(response)
        })
    }
}

async fn ok() -> &'static str {
    "ok"
}

async fn send(router: axum::Router, request: Request<Body>) -> Response {
    router.oneshot(request).await.expect("infallible")
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("request")
}

fn order(response: &Response) -> Vec<String> {
    response
        .headers()
        .get_all("x-order")
        .iter()
        .map(|v| v.to_str().expect("utf-8").to_string())
        .collect()
}

// --- the builder can install layers at all ---------------------------------

#[tokio::test]
async fn a_user_layer_reaches_the_router() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .layer(Mark("user"))
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(order(&response), ["user"]);
}

#[tokio::test]
async fn user_layers_nest_in_call_order_first_outermost() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .layer(Mark("first"))
        .layer(Mark("second"))
        .build();

    let response = send(app.into_router(), get("/")).await;
    // Innermost appends first. `first` was declared first, so it is outermost,
    // so it appends last.
    assert_eq!(order(&response), ["second", "first"]);
}

// --- Blocker 1: the Inertia extractor was unreachable ----------------------

fn inertia_config() -> InertiaConfig {
    InertiaConfig::new("test-version", default_root_document("Test")).expect("config")
}

async fn render(inertia: arcature::Inertia) -> Response {
    inertia
        .render("home", serde_json::json!({ "greeting": "hello" }))
        .await
        .expect("render")
}

#[tokio::test]
async fn the_inertia_extractor_fails_without_the_layer() {
    // Not a wish for this behaviour — a record of what an application that
    // forgets `.inertia(..)` actually gets, so the failure stays legible
    // rather than turning into a panic or a blank 200.
    let app = Application::new()
        .routes(Routes::new([Route::get("/", render)]))
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn inertia_extractor_works_once_the_layer_is_installed() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", render)]))
        .inertia(inertia_config())
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the scaffolded-app 500 is back"
    );

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("text/html"),
        "a first visit must get the root document, got {content_type}"
    );

    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    let body = String::from_utf8(body.to_vec()).expect("utf-8");
    assert!(body.contains("<!doctype html>"), "{body}");
    assert!(body.contains("data-page"), "no Inertia page object: {body}");
    assert!(body.contains("hello"), "props missing: {body}");
}

#[tokio::test]
async fn an_inertia_visit_gets_the_page_object_as_json() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", render)]))
        .inertia(inertia_config())
        .build();

    let request = Request::builder()
        .uri("/")
        .header("x-inertia", "true")
        .header("x-inertia-version", "test-version")
        .body(Body::empty())
        .expect("request");

    let response = send(app.into_router(), request).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-inertia")
            .and_then(|v| v.to_str().ok()),
        Some("true"),
        "an Inertia response must be marked as one"
    );

    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    let page: serde_json::Value = serde_json::from_slice(&body).expect("json page object");
    assert_eq!(page["component"], "home");
    assert_eq!(page["props"]["greeting"], "hello");
}

// --- the order is the documented order, not the call order -----------------
//
// An ordering test only means something if it fails under the other order.
// A layer that merely *runs* proves nothing: both orders run both layers. So
// each test below turns on something order-sensitive — a short circuit that
// one side never sees past, or a request extension only the outer layer can
// have put there.

/// Reports whether the request already carries the Inertia context by the
/// time it reaches this layer. Only the *outer* of two layers can have put it
/// there, so the stamp reads the nesting directly.
#[derive(Clone)]
struct SawInertia;

impl<S> Layer<S> for SawInertia {
    type Service = SawInertiaService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        SawInertiaService { inner }
    }
}

#[derive(Clone)]
struct SawInertiaService<S> {
    inner: S,
}

impl<S> Service<Request<Body>> for SawInertiaService<S>
where
    S: Service<Request<Body>, Response = Response, Error = std::convert::Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = std::convert::Infallible;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let saw = request
            .extensions()
            .get::<arcature::InertiaRequest>()
            .is_some();
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let mut response = inner.call(request).await?;
            response.headers_mut().append(
                "x-order",
                if saw { "saw-inertia" } else { "no-inertia" }
                    .parse()
                    .expect("header value"),
            );
            Ok(response)
        })
    }
}

#[tokio::test]
async fn a_user_layer_sees_the_inertia_context() {
    // Inertia is stage 7, user layers stage 8: the context is already in the
    // extensions when a user layer runs. Flip the two and this reads
    // `no-inertia`.
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .layer(SawInertia)
        .inertia(inertia_config())
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(order(&response), ["saw-inertia"]);
}

#[tokio::test]
async fn without_inertia_a_user_layer_sees_no_context() {
    // The control for the test above: the stamp tracks the extension, not the
    // mere presence of the layer.
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .layer(SawInertia)
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(order(&response), ["no-inertia"]);
}

#[tokio::test]
async fn a_csrf_rejection_never_reaches_a_user_layer() {
    // CSRF is stage 6, outside user layers, so a rejected request is refused
    // before any application-level layer can act on it. If the two were
    // swapped the user layer would run and stamp itself.
    let app = Application::new()
        .routes(Routes::new([Route::post("/", ok)]))
        .layer(Mark("user"))
        .csrf(arcature::CsrfConfig::dev())
        .build();

    let request = Request::builder()
        .method("POST")
        .uri("/")
        .body(Body::empty())
        .expect("request");

    let response = send(app.into_router(), request).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        order(&response).is_empty(),
        "a user layer ran on a request CSRF had already refused"
    );
}

#[tokio::test]
async fn a_timeout_never_reaches_a_user_layer() {
    // Timeout is stage 4, outside everything it bounds. When it fires, the
    // inner stack — user layers included — is dropped mid-flight.
    async fn slow() -> &'static str {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        "never"
    }

    let app = Application::new()
        .routes(Routes::new([Route::get("/", slow)]))
        .layer(Mark("user"))
        .timeout(std::time::Duration::from_millis(20))
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert!(
        order(&response).is_empty(),
        "a user layer produced a response after the deadline"
    );
}

#[tokio::test]
async fn the_pipeline_order_does_not_depend_on_the_builder_call_order() {
    // Same layers, opposite call order, same resulting pipeline. This is the
    // whole reason the builder holds slots instead of one ordered list — and
    // the stamp is order-sensitive, so a call-order-dependent pipeline would
    // show up as differing stamps rather than as two equal empty vectors.
    let inertia_last = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .layer(SawInertia)
        .inertia(inertia_config())
        .build();

    let inertia_first = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .inertia(inertia_config())
        .layer(SawInertia)
        .build();

    let a = send(inertia_last.into_router(), get("/")).await;
    let b = send(inertia_first.into_router(), get("/")).await;

    assert_eq!(a.status(), StatusCode::OK);
    assert_eq!(a.status(), b.status());
    assert_eq!(order(&a), ["saw-inertia"]);
    assert_eq!(order(&a), order(&b));
}

// --- body limit and timeout ------------------------------------------------

#[tokio::test]
async fn the_body_limit_rejects_an_oversized_body() {
    async fn echo(body: String) -> String {
        body
    }

    let app = Application::new()
        .routes(Routes::new([Route::post("/", echo)]))
        .body_limit(16)
        .build();

    let request = Request::builder()
        .method("POST")
        .uri("/")
        .body(Body::from("x".repeat(1024)))
        .expect("request");

    let response = send(app.into_router(), request).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn the_body_limit_lets_a_small_body_through() {
    async fn echo(body: String) -> String {
        body
    }

    let app = Application::new()
        .routes(Routes::new([Route::post("/", echo)]))
        .body_limit(1024)
        .build();

    let request = Request::builder()
        .method("POST")
        .uri("/")
        .body(Body::from("small"))
        .expect("request");

    let response = send(app.into_router(), request).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_timeout_bounds_a_slow_handler() {
    async fn slow() -> &'static str {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        "never"
    }

    let app = Application::new()
        .routes(Routes::new([Route::get("/", slow)]))
        .timeout(std::time::Duration::from_millis(20))
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
}

#[tokio::test]
async fn no_timeout_is_applied_unless_asked_for() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

// --- CSRF ------------------------------------------------------------------

#[tokio::test]
async fn csrf_rejects_an_unsafe_request_without_a_token() {
    let app = Application::new()
        .routes(Routes::new([Route::post("/", ok)]))
        .csrf(arcature::CsrfConfig::dev())
        .build();

    let request = Request::builder()
        .method("POST")
        .uri("/")
        .body(Body::empty())
        .expect("request");

    let response = send(app.into_router(), request).await;
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "CSRF layer is installed but not enforcing"
    );
}

#[tokio::test]
async fn csrf_leaves_safe_requests_alone() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .csrf(arcature::CsrfConfig::dev())
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(response.status(), StatusCode::OK);
}
