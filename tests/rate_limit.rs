//! What a rate-limited route answers, and to whom.
//!
//! The unit tests in `src/routing/rate_limit.rs` pin the token-bucket
//! arithmetic. These pin the HTTP contract on top of it: the status, the
//! problem body, the `Retry-After` and `RateLimit-*` headers, and the scope
//! of an attachment -- a limit on one route must not throttle its
//! neighbours, which is the failure mode that only shows up in production.

#![cfg(feature = "api")]

use arcature::routing::{KeySource, RateLimit, Route, Routes};
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt as _;

async fn ok() -> &'static str {
    "ok"
}

/// One request through `router`, with optional headers.
async fn call(
    router: &axum::Router,
    uri: &str,
    headers: &[(&str, &str)],
) -> axum::http::Response<Body> {
    let mut builder = Request::builder().uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    router
        .clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("infallible")
}

/// The `RateLimit-*` triple, as strings, for whatever the response carries.
fn limit_headers(response: &axum::http::Response<Body>) -> (String, String, String) {
    let read = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    (
        read("ratelimit-limit"),
        read("ratelimit-remaining"),
        read("ratelimit-reset"),
    )
}

#[tokio::test]
async fn a_route_answers_until_the_bucket_empties_and_then_refuses() {
    let router = Routes::new([
        Route::get("/limited", ok).layer(RateLimit::per_minute(2).by(KeySource::Global))
    ])
    .into_router();

    for expected_remaining in ["1", "0"] {
        let response = call(&router, "/limited", &[]).await;
        assert_eq!(response.status(), StatusCode::OK);
        let (limit, remaining, _) = limit_headers(&response);
        assert_eq!(limit, "2");
        assert_eq!(remaining, expected_remaining);
    }

    let refused = call(&router, "/limited", &[]).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_refusal_is_an_rfc_9457_problem_with_retry_after() {
    let router = Routes::new([
        Route::get("/limited", ok).layer(RateLimit::per_hour(1).by(KeySource::Global))
    ])
    .into_router();

    assert_eq!(
        call(&router, "/limited", &[]).await.status(),
        StatusCode::OK
    );
    let refused = call(&router, "/limited", &[]).await;

    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    let content_type = refused
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("application/problem+json"),
        "{content_type}"
    );

    let retry_after: u64 = refused
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .expect("a Retry-After in seconds");
    assert!(retry_after >= 1, "a refusal must say when to come back");

    let (limit, remaining, reset) = limit_headers(&refused);
    assert_eq!(limit, "1");
    assert_eq!(remaining, "0");
    assert!(
        reset.parse::<u64>().is_ok(),
        "reset should be seconds: {reset}"
    );

    let body = axum::body::to_bytes(refused.into_body(), 64 * 1024)
        .await
        .expect("a body");
    let problem: serde_json::Value = serde_json::from_slice(&body).expect("problem JSON");
    assert_eq!(problem["status"], 429);
    assert!(problem["title"].is_string(), "{problem}");
    assert!(problem["type"].is_string(), "{problem}");
}

#[tokio::test]
async fn a_limit_on_one_route_does_not_throttle_its_neighbour() {
    let router = Routes::new([
        Route::get("/open", ok),
        Route::get("/limited", ok).layer(RateLimit::per_hour(1).by(KeySource::Global)),
    ])
    .into_router();

    assert_eq!(
        call(&router, "/limited", &[]).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        call(&router, "/limited", &[]).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );

    for _ in 0..5 {
        assert_eq!(
            call(&router, "/open", &[]).await.status(),
            StatusCode::OK,
            "an unlimited route must not inherit a sibling's quota"
        );
    }
}

#[tokio::test]
async fn a_limit_on_a_group_covers_every_route_in_it() {
    use arcature::routing::RouteGroup;

    let grouped =
        Routes::new([
            RouteGroup::new("/api", [Route::get("/one", ok), Route::get("/two", ok)])
                .layer(RateLimit::per_hour(2).by(KeySource::Global)),
        ])
        .into_router();
    let outside = Routes::new([Route::get("/outside", ok)]).into_router();
    let router = grouped.merge(outside);

    assert_eq!(
        call(&router, "/api/one", &[]).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        call(&router, "/api/two", &[]).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        call(&router, "/api/one", &[]).await.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the group shares one bucket"
    );
    assert_eq!(
        call(&router, "/outside", &[]).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn two_clients_keyed_by_header_get_their_own_buckets() {
    let router = Routes::new([Route::get("/limited", ok).layer(RateLimit::per_hour(1).by(
        KeySource::Header(axum::http::HeaderName::from_static("x-api-key")),
    ))])
    .into_router();

    let alice = [("x-api-key", "alice")];
    let bob = [("x-api-key", "bob")];

    assert_eq!(
        call(&router, "/limited", &alice).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        call(&router, "/limited", &alice).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        call(&router, "/limited", &bob).await.status(),
        StatusCode::OK,
        "one client's quota must not spend another's"
    );
}

#[tokio::test]
async fn requests_with_nothing_to_key_on_share_one_bucket() {
    // Not a free pass: a request that carries no identity still counts, and
    // it counts against everyone else who carries none either. The
    // alternative -- a fresh bucket per unidentifiable request -- is no
    // limit at all.
    let router = Routes::new([Route::get("/limited", ok).layer(RateLimit::per_hour(1).by(
        KeySource::Header(axum::http::HeaderName::from_static("x-api-key")),
    ))])
    .into_router();

    assert_eq!(
        call(&router, "/limited", &[]).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        call(&router, "/limited", &[]).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn a_burst_allows_more_than_the_rate_before_it_refuses() {
    let router = Routes::new([
        Route::get("/limited", ok).layer(RateLimit::per_hour(1).burst(3).by(KeySource::Global))
    ])
    .into_router();

    for _ in 0..3 {
        assert_eq!(
            call(&router, "/limited", &[]).await.status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        call(&router, "/limited", &[]).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn a_zero_limit_refuses_the_first_request() {
    let router = Routes::new([
        Route::get("/closed", ok).layer(RateLimit::per_minute(0).by(KeySource::Global))
    ])
    .into_router();

    let response = call(&router, "/closed", &[]).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let (limit, remaining, _) = limit_headers(&response);
    assert_eq!(limit, "0");
    assert_eq!(remaining, "0");
}

#[tokio::test]
async fn a_permitted_response_still_carries_the_limit_headers() {
    let router = Routes::new([
        Route::get("/limited", ok).layer(RateLimit::per_minute(10).by(KeySource::Global))
    ])
    .into_router();

    let response = call(&router, "/limited", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::RETRY_AFTER).is_none());
    let (limit, remaining, reset) = limit_headers(&response);
    assert_eq!(limit, "10");
    assert_eq!(remaining, "9");
    assert!(reset.parse::<u64>().is_ok(), "{reset}");
}

/// The Redis-backed backend, which needs a server.
///
/// Ignored: it is the one part of this module that cannot be exercised
/// without a live Valkey or Redis on `redis://127.0.0.1:6379`. Run it with
/// `cargo test --test rate_limit -- --ignored` against one.
#[cfg(feature = "cache")]
#[tokio::test]
#[ignore = "needs a live Redis on 127.0.0.1:6379"]
async fn the_redis_backend_shares_a_bucket_across_handles() {
    use arcature::cache::{Cache, CacheConfig, Namespace};

    // A per-process namespace so a rerun does not inherit the previous
    // run's buckets.
    let namespace =
        Namespace::new(&format!("ratelimit-test-{}", std::process::id())).expect("a namespace");
    let cache = Cache::connect(
        CacheConfig::new("redis://127.0.0.1:6379")
            .expect("a cache config")
            .namespace(namespace),
    )
    .await
    .expect("a live Redis");

    // Two independently built limits over one Redis: the point of a shared
    // backend is that they are still one quota, the way two instances of an
    // application behind a load balancer are.
    let first = RateLimit::per_hour(2)
        .by(KeySource::Global)
        .redis(cache.clone());
    let second = RateLimit::per_hour(2).by(KeySource::Global).redis(cache);

    let one = Routes::new([Route::get("/limited", ok).layer(first)]).into_router();
    let two = Routes::new([Route::get("/limited", ok).layer(second)]).into_router();

    assert_eq!(call(&one, "/limited", &[]).await.status(), StatusCode::OK);
    assert_eq!(call(&two, "/limited", &[]).await.status(), StatusCode::OK);
    assert_eq!(
        call(&one, "/limited", &[]).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}
