//! What a token bucket costs the request it lets through.
//!
//! `RateLimit` sits on the path of every request it guards, so its own cost
//! is charged to the traffic it is protecting. `src/routing/rate_limit.rs`
//! justifies a `std::sync::Mutex` on the grounds that "the critical section
//! is a hash lookup and three arithmetic operations"; these rows are that
//! sentence as a measurement.
//!
//! One iteration is one request driven through `tower::Service`, including
//! building the request, so the reported time is seconds per request. Every
//! row builds its request the same way and the `no-limiter` row runs the
//! same shape against the unwrapped service, which is what makes the
//! difference between two rows attributable to the limiter rather than to
//! the harness.
//!
//! The in-memory backend only. The Redis backend's cost is a network round
//! trip, which is not something a microbenchmark has anything to say about,
//! and measuring it here would need a server -- so it is left out rather
//! than faked.
//!
//! # The quotas are chosen so the outcome never changes mid-run
//!
//! An "allowed" row uses a quota large enough that the bucket cannot drain
//! during the measurement; a run that started allowing and ended refusing
//! would report the average of two different code paths. The refused row
//! uses a quota of zero, which refuses from the first request.

use std::convert::Infallible;
use std::hint::black_box;
use std::time::Duration;

use arcature::routing::{KeySource, RateLimit};
use axum::body::Body;
use axum::http::{Request, Response};
use criterion::{Criterion, criterion_group, criterion_main};
use tower::{Layer as _, Service, ServiceExt as _};

/// The number of distinct buckets the `many-keys` row spreads across.
///
/// Below the 8192-entry threshold at which the bucket table sweeps, so this
/// row measures a steady-state hash lookup rather than the sweep.
const KEY_SPACE: usize = 1024;

/// The header the limiter buckets by.
const KEY_HEADER: &str = "x-api-key";

/// The service under the limiter: a handler that does nothing, so what is
/// left in the measurement is the limiter and the request.
fn inner()
-> impl Service<Request<Body>, Response = Response<Body>, Error = Infallible, Future: Send>
+ Clone
+ Send
+ 'static {
    tower::service_fn(|_request: Request<Body>| async move {
        Ok::<_, Infallible>(Response::new(Body::empty()))
    })
}

/// A quota no bench run can exhaust: one token per nanosecond, and a bucket
/// deep enough that the first request already finds it full.
fn always_allows() -> RateLimit {
    RateLimit::new(u32::MAX, Duration::from_secs(1))
}

/// A quota with no tokens in it at all, so every request takes the refusal
/// path.
fn always_refuses() -> RateLimit {
    RateLimit::new(0, Duration::from_secs(60))
}

/// `KEY_SPACE` distinct bucket keys, leaked so the async blocks below can
/// take them by value without an allocation per iteration.
fn keys() -> Vec<&'static str> {
    (0..KEY_SPACE)
        .map(|n| &*Box::leak(format!("tenant-{n}").into_boxed_str()))
        .collect()
}

/// A `GET /` carrying `key` in the bucket header.
fn get(key: &str) -> Request<Body> {
    Request::builder()
        .uri("/")
        .header(KEY_HEADER, key)
        .body(Body::empty())
        .expect("the request is well-formed")
}

fn decision(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime is available");
    let keys = keys();
    let one_key = &keys[..1];

    let by_header = always_allows().by(KeySource::Header(
        KEY_HEADER.parse().expect("a valid header name"),
    ));

    let mut group = c.benchmark_group("rate-limit");

    // The floor: the same request, the same inner service, no limiter.
    group.bench_function("no-limiter", |b| {
        let service = inner();
        b.to_async(&runtime).iter(|| {
            let service = service.clone();
            async move { black_box(service.oneshot(get("tenant-0")).await) }
        });
    });

    // One bucket for the whole application. No key extraction beyond a
    // constant, so this is the arithmetic and the mutex on their own.
    group.bench_function("global-key/allowed", |b| {
        let service = always_allows().by(KeySource::Global).layer(inner());
        b.to_async(&runtime).iter(|| {
            let service = service.clone();
            async move { black_box(service.oneshot(get("tenant-0")).await) }
        });
    });

    // One bucket, reached through a header lookup: the realistic shape of a
    // per-tenant limit whose traffic happens to be one tenant.
    group.bench_function("header-key/one-bucket", |b| {
        let service = by_header.clone().layer(inner());
        let mut next = 0usize;
        b.to_async(&runtime).iter(|| {
            let key = one_key[next % one_key.len()];
            next += 1;
            let service = service.clone();
            async move { black_box(service.oneshot(get(key)).await) }
        });
    });

    // The same limiter spread across `KEY_SPACE` buckets, which is what a
    // per-tenant or per-IP limit looks like in production.
    group.bench_function("header-key/many-buckets", |b| {
        let service = by_header.clone().layer(inner());
        let mut next = 0usize;
        b.to_async(&runtime).iter(|| {
            let key = keys[next % keys.len()];
            next += 1;
            let service = service.clone();
            async move { black_box(service.oneshot(get(key)).await) }
        });
    });

    // A closure key source, the escape hatch for an authenticated user id.
    group.bench_function("custom-key/allowed", |b| {
        let service = always_allows()
            .by_fn(|request| {
                request
                    .headers()
                    .get(KEY_HEADER)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned)
            })
            .layer(inner());
        b.to_async(&runtime).iter(|| {
            let service = service.clone();
            async move { black_box(service.oneshot(get("tenant-0")).await) }
        });
    });

    // The refusal path: the inner service is never called, and a `429` with
    // its `RateLimit-*` headers is built instead.
    group.bench_function("global-key/refused", |b| {
        let service = always_refuses().by(KeySource::Global).layer(inner());
        b.to_async(&runtime).iter(|| {
            let service = service.clone();
            async move { black_box(service.oneshot(get("tenant-0")).await) }
        });
    });

    group.finish();
}

criterion_group!(benches, decision);
criterion_main!(benches);
