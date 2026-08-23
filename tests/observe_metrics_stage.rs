//! A refused request is counted, which is the whole reason `.metrics()`
//! exists rather than `.layer(MetricsLayer::new(..))`.
//!
//! A user `.layer()` lands at stage 21 of the pipeline -- inside the body
//! limit (12), the timeout (13), maintenance (14) and the rate limiter (15).
//! A counter installed there never sees a request refused with a `413`, a
//! `408`, a `503` or a `429`, so the request total silently means "requests
//! that got through admission". The access log at stage 9 does see them, so
//! the two sources disagree, and the gap is widest under exactly the load an
//! incident is about.
//!
//! `.metrics(..)` installs the same layer at stage 9. Both tests below drive
//! the identical request through the identical rate limit; the only
//! difference is where the layer sits.

#![cfg(all(feature = "observe", feature = "macros"))]

use std::time::Duration;

use arcature::Application;
use arcature::observe::{Metrics, MetricsLayer};
use arcature::routing::{RateLimit, Route, Routes};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;

async fn ok() -> &'static str {
    "ok"
}

/// A quota of zero refuses from the first request, so no timing is involved.
fn refusing_limit() -> RateLimit {
    RateLimit::new(0, Duration::from_secs(60))
}

fn request() -> Request<Body> {
    Request::builder()
        .uri("/")
        .body(Body::empty())
        .expect("a well-formed request")
}

/// The counter reading for `http_requests_total`, or 0 when the series is
/// absent entirely.
fn total(metrics: &Metrics) -> u64 {
    metrics
        .render()
        .lines()
        .filter(|line| line.starts_with("http_requests_total"))
        .filter_map(|line| line.rsplit(' ').next())
        .filter_map(|value| value.trim().parse::<f64>().ok())
        .map(|value| value as u64)
        .sum()
}

#[tokio::test]
async fn a_rate_limited_request_is_counted_when_metrics_sits_at_stage_nine() {
    let metrics = Metrics::new();
    let app = Application::<()>::new()
        .routes(Routes::new(vec![Route::get("/", ok)]))
        .rate_limit(refusing_limit())
        .metrics(metrics.clone())
        .build();

    let response = app
        .into_router()
        .oneshot(request())
        .await
        .expect("the router answered");
    assert_eq!(
        response.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the quota of zero should have refused this",
    );

    assert_eq!(
        total(&metrics),
        1,
        "a refused request was not counted. `.metrics()` puts the layer at \
         stage 9, outside the rate limiter, so it must see this.",
    );
}

/// The same request, the same limit, the layer installed the obvious way.
///
/// This asserts the *gap* rather than a desirable behaviour: it is here so
/// that the difference between the two placements is pinned, and so that
/// anyone who later moves user layers outside the admission stages finds out
/// from a failing test rather than from a graph.
#[tokio::test]
async fn a_rate_limited_request_is_missed_when_the_layer_sits_among_user_layers() {
    let metrics = Metrics::new();
    let app = Application::<()>::new()
        .routes(Routes::new(vec![Route::get("/", ok)]))
        .rate_limit(refusing_limit())
        .layer(MetricsLayer::new(metrics.clone()))
        .build();

    let response = app
        .into_router()
        .oneshot(request())
        .await
        .expect("the router answered");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

    assert_eq!(
        total(&metrics),
        0,
        "a user layer is at stage 21, inside the rate limiter at 15, so it \
         cannot have seen a request the limiter refused. If this now counts \
         the request, the pipeline order changed and `.metrics()` may no \
         longer be needed.",
    );
}

/// The two placements agree on a request that is actually served, so the
/// difference above is about refusals and not about counting at all.
#[tokio::test]
async fn both_placements_count_a_request_that_gets_through() {
    for (label, app, metrics) in [
        {
            let metrics = Metrics::new();
            (
                "stage 9",
                Application::<()>::new()
                    .routes(Routes::new(vec![Route::get("/", ok)]))
                    .metrics(metrics.clone())
                    .build(),
                metrics,
            )
        },
        {
            let metrics = Metrics::new();
            (
                "stage 21",
                Application::<()>::new()
                    .routes(Routes::new(vec![Route::get("/", ok)]))
                    .layer(MetricsLayer::new(metrics.clone()))
                    .build(),
                metrics,
            )
        },
    ] {
        let response = app
            .into_router()
            .oneshot(request())
            .await
            .expect("the router answered");
        assert_eq!(response.status(), StatusCode::OK, "{label}");
        assert_eq!(total(&metrics), 1, "{label} did not count a served request");
    }
}
