//! Routing scope contracts.
//!
//! Middleware attached to a route must reach that route and nothing else.
//! This file exists because it once did not: `Route` used to carry a closure
//! folding the whole `axum::Router`, and `Routes::new` folds routes into one
//! accumulating router — so a route's middleware wrapped every route
//! registered before it. A public route silently inherited the guard of a
//! protected route declared later in the same array.
//!
//! These tests pin the scope of each attachment point. They are cheap and
//! they guard something that fails silently and dangerously.

use arcature::Result;
use arcature::routing::{IntoRoutes, Middleware, Next, Request, Response, Route, RouteGroup, Routes};
use axum::body::Body;
use axum::http::Request as HttpRequest;
use axum::http::StatusCode;
use std::future::Future;
use std::pin::Pin;
use tower::ServiceExt;

/// Stamps a header on the way out, so a response tells us which middleware
/// ran. Named so several instances can be distinguished in one router.
#[derive(Clone)]
struct Stamp(&'static str);

impl Middleware for Stamp {
    fn handle(
        &self,
        request: Request,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send>> {
        let name = self.0;
        Box::pin(async move {
            let mut response = next.run(request).await;
            response
                .headers_mut()
                .append("x-stamp", name.parse().expect("header value"));
            Ok(response)
        })
    }
}

/// Refuses the request outright. The security-relevant shape: a guard that
/// must not protect routes it was never attached to, and must not *fail* to
/// protect the one it was.
#[derive(Clone)]
struct Deny;

impl Middleware for Deny {
    fn handle(
        &self,
        _request: Request,
        _next: Next,
    ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send>> {
        Box::pin(async move {
            Ok(axum::response::IntoResponse::into_response((
                StatusCode::FORBIDDEN,
                "denied",
            )))
        })
    }
}

async fn ok() -> &'static str {
    "ok"
}

/// Send one request through the router and return `(status, stamps)`.
async fn call(router: axum::Router, uri: &str) -> (StatusCode, Vec<String>) {
    let response = router
        .oneshot(
            HttpRequest::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("infallible");
    let status = response.status();
    let stamps = response
        .headers()
        .get_all("x-stamp")
        .iter()
        .map(|v| v.to_str().expect("utf-8").to_string())
        .collect();
    (status, stamps)
}

#[tokio::test]
async fn route_middleware_does_not_leak_onto_sibling_routes() {
    let router = Routes::new([
        Route::get("/plain", ok),
        Route::get("/stamped", ok).middleware(Stamp("route")),
    ])
    .into_router();

    let (_, plain) = call(router.clone(), "/plain").await;
    assert!(
        plain.is_empty(),
        "a route with no middleware must not inherit a sibling's: {plain:?}"
    );

    let (_, stamped) = call(router, "/stamped").await;
    assert_eq!(stamped, ["route"]);
}

#[tokio::test]
async fn route_middleware_does_not_leak_regardless_of_declaration_order() {
    // The old bug leaked backwards, onto routes declared *before*. Assert the
    // reverse order too, so a "fix" that merely reverses the fold fails here.
    let router = Routes::new([
        Route::get("/stamped", ok).middleware(Stamp("route")),
        Route::get("/plain", ok),
    ])
    .into_router();

    let (_, plain) = call(router, "/plain").await;
    assert!(plain.is_empty(), "leaked forwards: {plain:?}");
}

#[tokio::test]
async fn a_guard_on_one_route_leaves_the_others_reachable() {
    let router = Routes::new([
        Route::get("/public", ok),
        Route::get("/private", ok).middleware(Deny),
    ])
    .into_router();

    let (public, _) = call(router.clone(), "/public").await;
    assert_eq!(public, StatusCode::OK, "public route was denied");

    let (private, _) = call(router, "/private").await;
    assert_eq!(private, StatusCode::FORBIDDEN, "guard did not run");
}

#[tokio::test]
async fn group_middleware_reaches_every_route_in_the_group_and_no_others() {
    let mut routes = vec![Route::get("/open", ok)];
    routes.extend(
        RouteGroup::new("/admin", [Route::get("/a", ok), Route::get("/b", ok)])
            .middleware(Stamp("group"))
            .into_routes(),
    );
    let router = Routes::new(routes).into_router();

    let (_, a) = call(router.clone(), "/admin/a").await;
    assert_eq!(a, ["group"]);

    let (_, b) = call(router.clone(), "/admin/b").await;
    assert_eq!(b, ["group"]);

    let (_, open) = call(router, "/open").await;
    assert!(
        open.is_empty(),
        "group middleware escaped the group: {open:?}"
    );
}

#[tokio::test]
async fn group_and_route_middleware_both_run() {
    let router = Routes::new(
        RouteGroup::new("/admin", [Route::get("/a", ok).middleware(Stamp("route"))])
            .middleware(Stamp("group"))
            .into_routes(),
    )
    .into_router();

    let (_, stamps) = call(router, "/admin/a").await;
    assert_eq!(stamps.len(), 2, "expected both stamps, got {stamps:?}");
    assert!(stamps.contains(&"route".to_string()));
    assert!(stamps.contains(&"group".to_string()));
}

#[tokio::test]
async fn collection_middleware_wraps_every_route_in_the_collection() {
    let router = Routes::new([Route::get("/a", ok), Route::get("/b", ok)])
        .middleware(Stamp("all"))
        .into_router();

    for path in ["/a", "/b"] {
        let (_, stamps) = call(router.clone(), path).await;
        assert_eq!(stamps, ["all"], "{path}");
    }
}

#[tokio::test]
async fn merged_collections_keep_their_own_middleware() {
    let public = Routes::new([Route::get("/public", ok)]);
    let admin = Routes::new([Route::get("/admin", ok)]).middleware(Stamp("admin"));

    let router = public.merge(admin).into_router();

    let (_, open) = call(router.clone(), "/public").await;
    assert!(
        open.is_empty(),
        "a merged collection's middleware leaked: {open:?}"
    );

    let (_, guarded) = call(router, "/admin").await;
    assert_eq!(guarded, ["admin"]);
}

#[tokio::test]
async fn two_methods_on_one_path_still_share_the_path() {
    // `Routes::new` registers each route's own `MethodRouter` at its path.
    // Axum merges method routers for a shared path; this asserts the refactor
    // did not turn that into a panic or an overwrite.
    let router = Routes::new([Route::get("/thing", ok), Route::post("/thing", ok)]).into_router();

    let get = router
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/thing")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("infallible");
    assert_eq!(get.status(), StatusCode::OK);

    let post = router
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/thing")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("infallible");
    assert_eq!(post.status(), StatusCode::OK);
}

#[tokio::test]
async fn per_method_middleware_stays_on_its_method() {
    let router = Routes::new([
        Route::get("/thing", ok),
        Route::post("/thing", ok).middleware(Deny),
    ])
    .into_router();

    let get = router
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/thing")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("infallible");
    assert_eq!(get.status(), StatusCode::OK, "GET was denied by POST's guard");

    let post = router
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/thing")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("infallible");
    assert_eq!(post.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn named_routes_survive_grouping() {
    let routes: Routes = Routes::new(
        RouteGroup::new("/admin", [Route::get("/users/{id}", ok).name("admin.users.show")])
            .middleware(Stamp("group"))
            .into_routes(),
    );

    assert_eq!(
        routes.url_for("admin.users.show", &["7"]).expect("url"),
        "/admin/users/7"
    );
}
