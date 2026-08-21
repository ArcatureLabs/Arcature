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

#![cfg(all(feature = "inertia", feature = "auth", feature = "observe"))]

use arcature::assets::{Assets, AssetsConfig};
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
        MarkService {
            inner,
            name: self.0,
        }
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

// --- the `Page<T>` golden path ----------------------------------------------
//
// A handler returning `Page<T>` never touches the `Inertia` extractor. It
// cannot render on its own -- `IntoResponse` has no request -- so it records
// the render and the Inertia layer performs it. These tests pin both halves:
// the render really happens when the layer is there, and its absence is loud.

#[derive(serde::Serialize)]
struct HomePage {
    greeting: String,
}

impl arcature::inertia::ClientData for HomePage {
    fn exposure_schema() -> arcature::inertia::PropsSchema {
        arcature::inertia::PropsSchema::new()
            .required("greeting", arcature::inertia::ContractType::string())
    }
}

impl arcature::inertia::PageType for HomePage {
    const CONTRACT: arcature::inertia::PageContract<Self> =
        arcature::inertia::PageContract::new("home");
}

async fn home_page() -> arcature::Page<HomePage> {
    arcature::dx::page(HomePage {
        greeting: "hello".to_string(),
    })
}

#[tokio::test]
async fn a_page_return_type_renders_through_the_inertia_layer() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", home_page)]))
        .inertia(inertia_config())
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    let body = String::from_utf8(body.to_vec()).expect("utf-8");
    assert!(body.contains("<!doctype html>"), "{body}");
    // The component name came from `HomePage::CONTRACT`, not from anything
    // the route or the handler repeated.
    assert!(body.contains("home"), "component missing: {body}");
    assert!(body.contains("hello"), "props missing: {body}");
}

#[tokio::test]
async fn a_page_on_an_inertia_visit_is_the_page_object() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", home_page)]))
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

    let page = json_body(response).await;
    assert_eq!(page["component"], "home");
    assert_eq!(page["props"]["greeting"], "hello");
}

#[tokio::test]
async fn a_page_without_the_inertia_layer_says_so() {
    // The placeholder must never look like a working route: no blank 200,
    // and a detail that names the missing builder call.
    let app = Application::new()
        .routes(Routes::new([Route::get("/", home_page)]))
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = json_body(response).await;
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("home"), "the page is not named: {detail}");
    assert!(
        detail.contains(".inertia("),
        "the fix is not named: {detail}"
    );
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
    // Inertia is stage 16, user layers stage 18: the context is already in the
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
    // CSRF is stage 15, outside user layers, so a rejected request is refused
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
    // Timeout is stage 12, outside everything it bounds. When it fires, the
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

// --- the document root -----------------------------------------------------
//
// The production half of the one-port story. In development the dev proxy
// hands Vite requests to Vite; in production this process is the only thing
// running, so the built assets have to come from the router's fallback. Until
// this landed nothing in the crate served a file at all -- the default root
// document referenced `/css/app.css` and `/js/app.js` with nothing behind
// either.

/// A scaffold-shaped document root: one hashed bundle, one plain file, and
/// the manifest that names the bundle.
fn document_root() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    let public = dir.path().join("public");
    std::fs::create_dir_all(public.join("build").join("assets")).expect("assets dir");
    std::fs::create_dir_all(public.join("build").join(".vite")).expect("manifest dir");
    std::fs::write(
        public.join("build").join("assets").join("app-C7xk91Qa.js"),
        "export default 1;\n",
    )
    .expect("bundle");
    std::fs::write(public.join("robots.txt"), "User-agent: *\n").expect("robots.txt");
    std::fs::write(
        public.join("build").join(".vite").join("manifest.json"),
        r#"{"resources/js/app.tsx":{"file":"assets/app-C7xk91Qa.js","isEntry":true}}"#,
    )
    .expect("manifest");
    dir
}

fn assets_config(root: &tempfile::TempDir) -> AssetsConfig {
    AssetsConfig::new().public_dir(root.path().join("public"))
}

fn cache_control(response: &Response) -> &str {
    response
        .headers()
        .get(axum::http::header::CACHE_CONTROL)
        .map_or("", |v| v.to_str().expect("utf-8"))
}

#[tokio::test]
async fn a_hashed_bundle_is_served_and_cached_forever() {
    let root = document_root();
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .static_files(&assets_config(&root))
        .build();

    let response = send(app.into_router(), get("/build/assets/app-C7xk91Qa.js")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        cache_control(&response),
        "public, max-age=31536000, immutable"
    );
}

#[tokio::test]
async fn a_plain_public_file_revalidates_instead() {
    // Same server, same fallback -- the difference is only the name. A
    // `robots.txt` frozen for a year would be uneditable.
    let root = document_root();
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .static_files(&assets_config(&root))
        .build();

    let response = send(app.into_router(), get("/robots.txt")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(cache_control(&response), "no-cache");
}

#[tokio::test]
async fn a_missing_file_is_still_a_404() {
    let root = document_root();
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .static_files(&assets_config(&root))
        .build();

    let response = send(app.into_router(), get("/build/assets/gone-A1b2C3d4.js")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    // Not cached: a mistyped asset name must not become permanent.
    assert_eq!(cache_control(&response), "");
}

#[tokio::test]
async fn a_route_wins_over_a_file_of_the_same_name() {
    // The document root is the *fallback*, so it only sees what routing
    // missed. Were it a layer or a merged route, this would be ambiguous.
    let root = document_root();
    std::fs::write(root.path().join("public").join("greet"), "from disk\n").expect("file");
    let app = Application::new()
        .routes(Routes::new([Route::get("/greet", ok)]))
        .static_files(&assets_config(&root))
        .build();

    let response = send(app.into_router(), get("/greet")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    assert_eq!(&body[..], b"ok");
}

#[tokio::test]
async fn the_pipeline_wraps_the_document_root_too() {
    // The fallback is installed before any layer is applied, so a served file
    // gets the same headers, compression and logging as a handler response.
    // If it were installed after, this header would be missing.
    let root = document_root();
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .static_files(&assets_config(&root))
        .layer(Mark("user"))
        .build();

    let response = send(app.into_router(), get("/robots.txt")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(order(&response), ["user"]);
}

#[tokio::test]
async fn the_root_document_references_the_hashed_name_from_the_manifest() {
    // The whole point of reading the manifest: `resources/js/app.tsx` is what
    // the app author writes, `assets/app-C7xk91Qa.js` is what exists on disk,
    // and only the manifest connects them.
    let root = document_root();
    let config = assets_config(&root);
    let assets = Assets::from_manifest(&config).expect("manifest");
    assert!(!assets.is_dev());

    let app = Application::new()
        .routes(Routes::new([Route::get("/", render)]))
        .inertia(
            InertiaConfig::new(
                "test-version",
                arcature::vite_root_document("Acme", &assets, "resources/js/app.tsx"),
            )
            .expect("config"),
        )
        .static_files(&config)
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    let html = String::from_utf8(body.to_vec()).expect("utf-8");
    assert!(
        html.contains("/build/assets/app-C7xk91Qa.js"),
        "the page must reference the built file, not the source path: {html}"
    );
    assert!(
        !html.contains("/@vite/client"),
        "no HMR client in a production build: {html}"
    );

    // And that reference is actually servable by the same application.
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .static_files(&config)
        .build();
    let response = send(app.into_router(), get("/build/assets/app-C7xk91Qa.js")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

// --- the production stages -------------------------------------------------
//
// Security headers, the panic catcher, the access log and compression are all
// off unless asked for. What these lock down is not that they work in
// isolation -- each has unit tests next to it -- but *where* they sit, because
// the whole value of a fixed pipeline order is that the answer does not depend
// on how the builder was called.

async fn boom() -> &'static str {
    panic!("a handler panicked");
}

fn header<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response
        .headers()
        .get(name)
        .map(|v| v.to_str().expect("ascii"))
}

#[tokio::test]
async fn nothing_is_installed_unless_it_is_asked_for() {
    // The counterpart to every test below: a bare application has none of
    // this, so each header that appears later appeared because of a call.
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(header(&response, "x-content-type-options"), None);
    assert_eq!(header(&response, "x-frame-options"), None);
    assert_eq!(header(&response, "x-request-id"), None);
}

#[tokio::test]
async fn security_headers_reach_a_handler_response() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .security_headers(arcature::http::SecurityHeaders::new())
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(header(&response, "x-content-type-options"), Some("nosniff"));
    assert_eq!(header(&response, "x-frame-options"), Some("DENY"));
}

#[tokio::test]
async fn security_headers_reach_a_response_no_handler_produced() {
    // This is the reason the stage sits outside the body limit rather than
    // inside it, and the assertion that would fail if it were moved back: a
    // 413 is a page a browser renders, and it needs the headers too.
    async fn echo(body: String) -> String {
        body
    }

    let app = Application::new()
        .routes(Routes::new([Route::post("/", echo)]))
        .body_limit(8)
        .security_headers(arcature::http::SecurityHeaders::new())
        .build();

    let request = Request::builder()
        .method("POST")
        .uri("/")
        .body(Body::from("far more than eight bytes"))
        .expect("request");
    let response = send(app.into_router(), request).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(header(&response, "x-content-type-options"), Some("nosniff"));
}

#[tokio::test]
async fn security_headers_reach_a_served_file() {
    let root = document_root();
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .static_files(&assets_config(&root))
        .security_headers(arcature::http::SecurityHeaders::new())
        .build();

    let response = send(app.into_router(), get("/robots.txt")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, "x-content-type-options"), Some("nosniff"));
}

#[tokio::test]
async fn a_request_id_is_generated_and_echoed() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .request_id()
        .build();

    let response = send(app.into_router(), get("/")).await;
    let id = header(&response, "x-request-id").expect("an id is echoed");
    assert!(!id.is_empty());
}

#[tokio::test]
async fn an_inbound_request_id_survives_the_hop() {
    // A reverse proxy in front of the app has already assigned one; minting a
    // second breaks the trace at exactly the boundary it is meant to cross.
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .request_id()
        .build();

    let request = Request::builder()
        .uri("/")
        .header("x-request-id", "from-the-edge")
        .body(Body::empty())
        .expect("request");
    let response = send(app.into_router(), request).await;
    assert_eq!(header(&response, "x-request-id"), Some("from-the-edge"));
}

#[tokio::test]
async fn a_panic_becomes_a_problem_response() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/boom", boom)]))
        .catch_panic()
        .build();

    let response = send(app.into_router(), get("/boom")).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        header(&response, "content-type"),
        Some(arcature::PROBLEM_JSON)
    );
}

#[tokio::test]
async fn a_panic_response_does_not_carry_the_panic_message() {
    // A panic payload is written for a developer with a backtrace. It
    // routinely names a file, a query, or the value that caused the panic --
    // none of which is the client's business.
    let app = Application::new()
        .routes(Routes::new([Route::get("/boom", boom)]))
        .catch_panic()
        .build();

    let response = send(app.into_router(), get("/boom")).await;
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    let text = String::from_utf8(body.to_vec()).expect("utf-8");
    assert!(
        !text.contains("a handler panicked"),
        "the panic payload leaked: {text}"
    );
}

#[tokio::test]
async fn the_security_headers_survive_a_caught_panic() {
    // The panic catcher is inside the header stage, so its 500 is decorated
    // like any other response. If the two were swapped, the one response most
    // likely to be rendered raw in a browser would be the one without them.
    let app = Application::new()
        .routes(Routes::new([Route::get("/boom", boom)]))
        .catch_panic()
        .security_headers(arcature::http::SecurityHeaders::new())
        .request_id()
        .build();

    let response = send(app.into_router(), get("/boom")).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(header(&response, "x-content-type-options"), Some("nosniff"));
    assert!(header(&response, "x-request-id").is_some());
}

#[tokio::test]
async fn the_production_stages_wrap_a_user_layer_not_the_other_way_round() {
    // User layers are innermost. Called first or last, the answer is the same.
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .security_headers(arcature::http::SecurityHeaders::new())
        .layer(Mark("user"))
        .build();
    let response = send(app.into_router(), get("/")).await;
    assert_eq!(order(&response), ["user"]);
    assert_eq!(header(&response, "x-content-type-options"), Some("nosniff"));

    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .layer(Mark("user"))
        .security_headers(arcature::http::SecurityHeaders::new())
        .build();
    let response = send(app.into_router(), get("/")).await;
    assert_eq!(order(&response), ["user"]);
    assert_eq!(header(&response, "x-content-type-options"), Some("nosniff"));
}

#[tokio::test]
async fn compression_applies_when_the_client_asks_and_not_otherwise() {
    // Long enough to clear `tower-http`'s minimum-size threshold; a two-byte
    // body would be left alone whatever the pipeline did.
    async fn long() -> String {
        "compress me ".repeat(64)
    }

    let build = || {
        Application::new()
            .routes(Routes::new([Route::get("/", long)]))
            .compression()
            .build()
            .into_router()
    };

    let request = Request::builder()
        .uri("/")
        .header("accept-encoding", "gzip")
        .body(Body::empty())
        .expect("request");
    let response = send(build(), request).await;
    assert_eq!(header(&response, "content-encoding"), Some("gzip"));

    // No `Accept-Encoding`, no compression: the response must stay readable
    // to a client that never said it could decode one.
    let response = send(build(), get("/")).await;
    assert_eq!(header(&response, "content-encoding"), None);
}

// --- health endpoints ------------------------------------------------------
//
// The point of every test here is that the probes answer *despite* the rest
// of the pipeline, not *through* it. An orchestrator that gets a maintenance
// `503` on `/up/ready` replaces instances that are doing exactly what they
// were told.

async fn json_body(response: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn the_health_endpoints_are_registered_by_default() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .build();
    let router = app.into_router();

    for path in ["/up", "/up/live", "/up/ready"] {
        let response = send(router.clone(), get(path)).await;
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{path} should be registered"
        );
    }
}

#[tokio::test]
async fn liveness_is_200_before_anything_has_started() {
    // Liveness must never consult a dependency. If it did, a database blip
    // would look like a dead process and the orchestrator would restart it --
    // turning an outage into a restart loop.
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .build();

    let response = send(app.into_router(), get("/up/live")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["state"], "starting");
}

#[tokio::test]
async fn readiness_is_503_until_the_application_is_ready() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .build();

    let response = send(app.into_router(), get("/up/ready")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = json_body(response).await;
    assert_eq!(body["ready"], false);
}

#[tokio::test]
async fn a_health_response_is_never_cached() {
    // A cached readiness answer is worse than none: it reports the state the
    // instance was in, not the one it is in.
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .build();

    let response = send(app.into_router(), get("/up")).await;
    assert_eq!(
        header(&response, "cache-control"),
        Some("no-store, max-age=0")
    );
}

#[tokio::test]
async fn health_can_be_turned_off() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .health(false)
        .build();

    let response = send(app.into_router(), get("/up/live")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn health_can_be_mounted_elsewhere() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .health_prefix("/_internal/health")
        .build();
    let router = app.into_router();

    let response = send(router.clone(), get("/_internal/health/live")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = send(router, get("/up/live")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_route_of_the_same_name_is_not_clobbered_by_health() {
    // `/up` covers `/up` and what is under it, on a segment boundary. An
    // application route named `/upload` is a different path and must survive.
    let app = Application::new()
        .routes(Routes::new([Route::get("/upload", ok)]))
        .build();

    let response = send(app.into_router(), get("/upload")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_probes_answer_while_a_user_layer_short_circuits_everything() {
    // Health is merged beside the router, not layered over it. A user layer
    // that refuses every request -- an authentication gate, say -- must not
    // take the probes down with it.
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .layer(Deny)
        .build();
    let router = app.into_router();

    let response = send(router.clone(), get("/")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = send(router, get("/up/live")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// Refuses every request with a `403`, without calling inner.
#[derive(Clone)]
struct Deny;

impl<S> Layer<S> for Deny {
    type Service = DenyService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        DenyService { inner }
    }
}

#[derive(Clone)]
struct DenyService<S> {
    #[expect(
        dead_code,
        reason = "the point of this layer is that it never calls inner"
    )]
    inner: S,
}

impl<S> Service<Request<Body>> for DenyService<S>
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

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _request: Request<Body>) -> Self::Future {
        Box::pin(async move {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::FORBIDDEN;
            Ok(response)
        })
    }
}

// --- maintenance -----------------------------------------------------------

#[tokio::test]
async fn maintenance_is_a_pass_through_until_it_is_engaged() {
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .maintenance(arcature::http::Maintenance::new())
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_engaged_switch_answers_503_with_retry_after() {
    let maintenance = arcature::http::Maintenance::new();
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .maintenance(maintenance.clone())
        .build();
    let router = app.into_router();

    maintenance.engage();
    let response = send(router.clone(), get("/")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(header(&response, "retry-after"), Some("60"));
    assert_eq!(
        header(&response, "content-type"),
        Some(arcature::PROBLEM_JSON)
    );

    // And the handle turns it back off from anywhere.
    maintenance.disengage();
    let response = send(router, get("/")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn maintenance_never_reaches_the_probes() {
    let maintenance = arcature::http::Maintenance::engaged();
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .maintenance(maintenance)
        .build();
    let router = app.into_router();

    let response = send(router.clone(), get("/")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let response = send(router, get("/up/live")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_form_post_during_maintenance_gets_503_not_a_csrf_rejection() {
    // Maintenance is stage 13, CSRF stage 15. Swap them and a browser that
    // had the page open before the window opened gets a confusing `419`
    // instead of the honest "come back later".
    let maintenance = arcature::http::Maintenance::engaged();
    let app = Application::new()
        .routes(Routes::new([Route::post("/submit", ok)]))
        .maintenance(maintenance)
        .csrf(arcature::auth::CsrfConfig::default())
        .build();

    let request = Request::builder()
        .method("POST")
        .uri("/submit")
        .body(Body::empty())
        .expect("request");
    let response = send(app.into_router(), request).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// --- error mapping ---------------------------------------------------------

#[tokio::test]
async fn a_bare_404_becomes_a_problem_document() {
    // Nothing in the application produced this response: axum's own fallback
    // did, with an empty body and no content type. Without the mapping stage
    // a JSON client gets a blank 404 it cannot parse.
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .error_mapping(arcature::http::ErrorMapping::new())
        .build();

    let response = send(app.into_router(), get("/nowhere")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        header(&response, "content-type"),
        Some(arcature::PROBLEM_JSON)
    );
    let body = json_body(response).await;
    assert_eq!(body["status"], 404);
}

#[tokio::test]
async fn a_method_not_allowed_keeps_its_allow_header() {
    // The `Allow` header is the only part of a 405 a client can act on.
    // Rewriting the body must not cost it.
    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .error_mapping(arcature::http::ErrorMapping::new())
        .build();

    let request = Request::builder()
        .method("POST")
        .uri("/")
        .body(Body::empty())
        .expect("request");
    let response = send(app.into_router(), request).await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(header(&response, "allow").is_some());
    assert_eq!(
        header(&response, "content-type"),
        Some(arcature::PROBLEM_JSON)
    );
}

#[tokio::test]
async fn an_oversized_body_rejection_becomes_a_problem_too() {
    // The handler has to read the body for the limit to fire: without a
    // `Content-Length` the layer can only refuse once the bytes arrive.
    async fn echo(body: String) -> String {
        body
    }

    let app = Application::new()
        .routes(Routes::new([Route::post("/upload", echo)]))
        .body_limit(8)
        .error_mapping(arcature::http::ErrorMapping::new())
        .build();

    let request = Request::builder()
        .method("POST")
        .uri("/upload")
        .body(Body::from(vec![b'x'; 64]))
        .expect("request");
    let response = send(app.into_router(), request).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        header(&response, "content-type"),
        Some(arcature::PROBLEM_JSON)
    );
}

#[tokio::test]
async fn a_handler_authored_error_body_is_left_alone() {
    // The stage fills in for responses nothing chose. A body the application
    // deliberately wrote is a choice, and overwriting it would be a bug.
    async fn refuse() -> Response {
        let mut response = Response::new(Body::from(r#"{"error":"nope"}"#));
        *response.status_mut() = StatusCode::BAD_REQUEST;
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        );
        response
    }

    let app = Application::new()
        .routes(Routes::new([Route::get("/", refuse)]))
        .error_mapping(arcature::http::ErrorMapping::new())
        .build();

    let response = send(app.into_router(), get("/")).await;
    assert_eq!(header(&response, "content-type"), Some("application/json"));
    let body = json_body(response).await;
    assert_eq!(body["error"], "nope");
}

#[tokio::test]
async fn error_mapping_does_not_touch_a_caught_panic() {
    // The panic catcher already produces a problem document, and it sits
    // outside this stage. Two rewrites of one response would be one too many.
    let app = Application::new()
        .routes(Routes::new([Route::get("/boom", boom)]))
        .catch_panic()
        .error_mapping(arcature::http::ErrorMapping::new())
        .build();

    let response = send(app.into_router(), get("/boom")).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = json_body(response).await;
    assert_eq!(body["status"], 500);
}

#[tokio::test]
async fn a_custom_mapper_can_return_an_html_error_page() {
    // The common case: a browser gets a rendered 404 page, an API client gets
    // the problem document.
    let mapping = arcature::http::ErrorMapping::new().with(|status, headers| {
        let wants_html = headers
            .get(axum::http::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|accept| accept.contains("text/html"));
        if !wants_html {
            return None;
        }
        let mut response = Response::new(Body::from(format!("<h1>{}</h1>", status.as_u16())));
        *response.status_mut() = status;
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("text/html; charset=utf-8"),
        );
        Some(response)
    });

    let app = Application::new()
        .routes(Routes::new([Route::get("/", ok)]))
        .error_mapping(mapping)
        .build();
    let router = app.into_router();

    let request = Request::builder()
        .uri("/nowhere")
        .header("accept", "text/html")
        .body(Body::empty())
        .expect("request");
    let response = send(router.clone(), request).await;
    assert_eq!(
        header(&response, "content-type"),
        Some("text/html; charset=utf-8")
    );

    // The mapper declined for a non-HTML client, so the default applies.
    let response = send(router, get("/nowhere")).await;
    assert_eq!(
        header(&response, "content-type"),
        Some(arcature::PROBLEM_JSON)
    );
}
