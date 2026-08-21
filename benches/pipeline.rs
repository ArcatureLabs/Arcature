//! What the composed pipeline costs per request.
//!
//! `src/application/pipeline.rs` documents twenty-one ordered stages and
//! claims that "every stage from 4 down is off unless asked for". That is a
//! cost claim, and a cost claim with no number behind it is a hope. This
//! bench puts three routers side by side on the same request:
//!
//! * a bare `axum::Router`, the floor;
//! * an Arcature application that called nothing but `.routes()`;
//! * an Arcature application wired the way the `arc new` scaffold wires one
//!   -- compression, security headers, request id, access log, panic
//!   catcher, error mapping, body limit, timeout, session, CSRF, Inertia,
//!   and a static-file fallback.
//!
//! One iteration is one request driven through `tower::Service`, so the
//! reported time is seconds per request. Nothing here touches the network,
//! the filesystem, or the clock beyond what the layers themselves do: the
//! static-file fallback is configured but never reached, because every
//! request in this file matches a route.
//!
//! # Reading the numbers
//!
//! `oneshot` consumes the service, so each iteration clones one first --
//! which is what axum does per connection, not per request. The
//! `service/clone` group measures that clone on its own so it can be
//! subtracted; without it the layered rows would look more expensive per
//! request than they are.

use std::hint::black_box;
use std::time::Duration;

use arcature::assets::AssetsConfig;
use arcature::auth::{CsrfConfig, SessionConfig};
use arcature::http::{ErrorMapping, SecurityHeaders};
use arcature::routing::{Route, Routes};
use arcature::{Application, InertiaConfig, default_root_document};
use axum::Router;
use axum::body::Body;
use axum::http::Request;
use criterion::{Criterion, criterion_group, criterion_main};
use tower::ServiceExt as _;
use tower_sessions_memory_store::MemoryStore;

/// The handler under every router in this file, so the only difference
/// between the rows is the pipeline.
async fn ok() -> &'static str {
    "ok"
}

/// A handler with a path parameter, to keep the matching cost in the picture.
async fn show(axum::extract::Path(id): axum::extract::Path<u32>) -> String {
    id.to_string()
}

/// A 64-byte signing key. Fixed rather than random: a bench that generates
/// entropy measures the generator.
fn signing_key() -> Vec<u8> {
    (0u8..64).collect()
}

/// A bare `axum::Router` with the same two routes. The floor.
fn bare() -> Router {
    Router::new()
        .route("/", axum::routing::get(ok))
        .route("/items/{id}", axum::routing::get(show))
}

/// The two routes, as Arcature declares them.
fn routes() -> Routes<()> {
    Routes::new([Route::get("/", ok), Route::get("/items/{id}", show)])
}

/// An application that asked for nothing. The health endpoints are still
/// merged, because they are on by default -- that merge is part of what an
/// unconfigured Arcature app costs, so it stays in.
fn unconfigured() -> Router {
    Application::new().routes(routes()).build().into_router()
}

/// The same application with the health merge switched off, which isolates
/// what merging a second router costs a request that does not go to it.
fn unconfigured_without_health() -> Router {
    Application::new()
        .routes(routes())
        .health(false)
        .build()
        .into_router()
}

/// Everything the generated scaffold turns on.
fn configured() -> Router {
    let inertia = InertiaConfig::new("bench", default_root_document("Bench"))
        .expect("the asset version is non-empty");
    let session = SessionConfig::new(&signing_key()).expect("the signing key is 64 bytes");

    Application::new()
        .routes(routes())
        .compression()
        .security_headers(SecurityHeaders::new())
        .request_id()
        .access_log()
        .catch_panic()
        .error_mapping(ErrorMapping::new())
        .body_limit(2 * 1024 * 1024)
        .timeout(Duration::from_secs(30))
        .inertia(inertia)
        .session(session, MemoryStore::default())
        .expect("the session layer accepts a memory store")
        .csrf(CsrfConfig::inertia())
        .static_files(&AssetsConfig::new())
        .build()
        .into_router()
}

/// A `GET` for `uri`, with no headers beyond the ones the builder adds.
fn get(uri: &'static str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("the request is well-formed")
}

fn dispatch(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime is available");

    let mut group = c.benchmark_group("dispatch");
    for (name, router) in [
        ("bare-axum", bare()),
        ("arcature-unconfigured", unconfigured()),
        (
            "arcature-unconfigured-no-health",
            unconfigured_without_health(),
        ),
        ("arcature-configured", configured()),
    ] {
        group.bench_function(format!("{name}/static-path"), |b| {
            b.to_async(&runtime).iter(|| {
                let router = router.clone();
                async move { black_box(router.oneshot(get("/")).await) }
            });
        });
        group.bench_function(format!("{name}/one-path-param"), |b| {
            b.to_async(&runtime).iter(|| {
                let router = router.clone();
                async move { black_box(router.oneshot(get("/items/42")).await) }
            });
        });
    }
    group.finish();
}

fn service_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("service/clone");
    for (name, router) in [
        ("bare-axum", bare()),
        ("arcature-unconfigured", unconfigured()),
        ("arcature-configured", configured()),
    ] {
        group.bench_function(name, |b| b.iter(|| black_box(router.clone())));
    }
    group.finish();
}

criterion_group!(benches, dispatch, service_clone);
criterion_main!(benches);
