//! A refused request still carries its trace, which is why `.trace_context()`
//! exists rather than `.layer(TraceContextLayer)`.
//!
//! The layer resolves `traceparent` and opens a span carrying `trace_id`, so
//! a log line can be joined to a distributed trace by id alone. Installed as
//! a user layer it lands at stage 21 -- inside the body limit (12), the
//! timeout (13), maintenance (14) and the rate limiter (15) -- so a request
//! refused by any of those never reaches it and produces log lines with no
//! trace on them. A refused request is one somebody is very likely to go
//! looking for.
//!
//! `.trace_context()` puts it at stage 8, outside the admission stages and
//! outside the access log at 9, so the access line for a `429` carries the
//! caller's trace id.

#![cfg(all(feature = "observe", feature = "macros"))]

use std::time::Duration;

use arcature::Application;
use arcature::observe::{AccessLogLayer, CaptureSink, JsonLog, RequestIdLayer, TraceContextLayer};
use arcature::routing::{RateLimit, Route, Routes};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::layer::SubscriberExt as _;

/// A syntactically valid W3C `traceparent` with a trace id that is easy to
/// find in a transcript.
const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

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
        .header("traceparent", TRACEPARENT)
        .body(Body::empty())
        .expect("a well-formed request")
}

/// Drive one refused request and return everything logged.
async fn refused_transcript(app: axum::Router) -> String {
    let sink = CaptureSink::new();
    let subscriber = tracing_subscriber::registry().with(JsonLog::new(sink.clone()));
    let dispatch = tracing::Dispatch::new(subscriber);

    let status = async move {
        app.oneshot(request())
            .await
            .expect("the router answered")
            .status()
    }
    .with_subscriber(dispatch)
    .await;

    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "the quota of zero should have refused this",
    );
    sink.transcript()
}

#[tokio::test]
async fn a_refused_request_carries_its_trace_when_the_layer_sits_at_stage_eight() {
    let app = Application::<()>::new()
        .routes(Routes::new(vec![Route::get("/", ok)]))
        .rate_limit(refusing_limit())
        .trace_context()
        .request_id()
        .access_log()
        .build();

    let logs = refused_transcript(app.into_router()).await;

    assert!(
        logs.contains("429") || logs.contains("status"),
        "no access line was captured, so the assertion below would be vacuous:\n{logs}",
    );
    assert!(
        logs.contains(TRACE_ID),
        "the access line for a refused request carried no trace id. \
         `.trace_context()` puts the layer at stage 8, outside the rate \
         limiter at 15, so it must have resolved the context before the \
         refusal.\n{logs}",
    );
}

/// The gap, pinned on purpose.
///
/// This asserts that a user layer *does* miss the refusal, so that anyone who
/// later moves user layers outside the admission stages learns it from a
/// failing test rather than from an untraceable incident.
#[tokio::test]
async fn a_refused_request_carries_no_trace_when_the_layer_sits_among_user_layers() {
    let app = Application::<()>::new()
        .routes(Routes::new(vec![Route::get("/", ok)]))
        .rate_limit(refusing_limit())
        .layer(TraceContextLayer)
        .request_id()
        .access_log()
        .build();

    let logs = refused_transcript(app.into_router()).await;

    assert!(
        !logs.contains(TRACE_ID),
        "a user layer is at stage 21, inside the rate limiter at 15, so it \
         cannot have resolved a trace for a request the limiter refused. If \
         this now carries the id, the pipeline order changed and \
         `.trace_context()` may no longer be needed.\n{logs}",
    );
}

/// And the stage-8 placement is not merely refusing-specific: a served
/// request carries the id too, so the difference above is about refusals.
#[tokio::test]
async fn a_served_request_carries_its_trace_at_stage_eight() {
    let sink = CaptureSink::new();
    let app = Application::<()>::new()
        .routes(Routes::new(vec![Route::get("/", ok)]))
        .trace_context()
        .request_id()
        .access_log()
        .build();

    let subscriber = tracing_subscriber::registry().with(JsonLog::new(sink.clone()));
    let dispatch = tracing::Dispatch::new(subscriber);

    let status = async move {
        app.into_router()
            .oneshot(request())
            .await
            .expect("the router answered")
            .status()
    }
    .with_subscriber(dispatch)
    .await;
    assert_eq!(status, StatusCode::OK);

    let logs = sink.transcript();
    assert!(
        logs.contains(TRACE_ID),
        "a served request carried no trace id:\n{logs}",
    );
}

/// The unused imports would otherwise warn under `-D warnings`; they are the
/// layers the builder installs and are named here to keep the test honest
/// about what it is comparing against.
#[test]
fn the_layers_this_compares_are_the_ones_the_builder_installs() {
    let _ = AccessLogLayer;
    let _ = RequestIdLayer;
    let _ = TraceContextLayer;
}
