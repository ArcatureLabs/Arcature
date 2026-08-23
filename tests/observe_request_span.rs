//! A log line written inside a handler can be tied back to its request.
//!
//! This is the property the request span exists for, and it was not held.
//! `AccessLogService` built the span with
//!
//! ```text
//! let _span = tracing::info_span!(REQUEST, ..);
//! ```
//!
//! which binds the span to nothing and enters it never. The access line
//! itself carried `request_id` because it names the field directly, so the
//! feature looked like it worked from the outside; a `tracing::info!` inside
//! a handler inherited nothing, and there was no way to join a handler's own
//! output to the request that produced it. The span was pure cost.
//!
//! The repair is `Instrument`, not a guard. `span.enter()` returns a guard
//! that stays entered across every await point in the handler -- including
//! the ones where the task is parked and a different request is running on
//! the thread -- so in async code it attributes other people's log lines to
//! this request. `Instrument` attaches the span to the future, which is
//! entered exactly while that future is polled.
//!
//! One request, two assertions, and the second is the one that regressed.

#![cfg(feature = "observe")]

use arcature::observe::{AccessLogLayer, CaptureSink, JsonLog, RequestIdLayer};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use tower::ServiceExt as _;
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::layer::SubscriberExt as _;

/// A handler that logs on its own account, the way application code does.
async fn handler() -> &'static str {
    tracing::info!(order_id = 4417, "reserving stock");
    "ok"
}

/// Every line the sink captured, as one string.
async fn transcript() -> String {
    let sink = CaptureSink::new();

    // `RequestIdLayer` must run before `AccessLogLayer` so the id is in the
    // extensions when the span is built, which means it is applied after it
    // and therefore wraps it.
    let router = Router::new()
        .route("/reserve", get(handler))
        .layer(AccessLogLayer)
        .layer(RequestIdLayer);

    let subscriber = tracing_subscriber::registry().with(JsonLog::new(sink.clone()));
    let dispatch = tracing::Dispatch::new(subscriber);

    // Attached to the future rather than installed with `with_default`:
    // `with_default` is thread-local, and a future on a multi-threaded
    // runtime may resume on another thread between two awaits, at which
    // point the lines the assertion is about reach no subscriber and the
    // test passes by capturing nothing.
    let status = async move {
        router
            .oneshot(
                Request::builder()
                    .uri("/reserve")
                    .body(Body::empty())
                    .expect("a well-formed request"),
            )
            .await
            .expect("the router answered")
            .status()
    }
    .with_subscriber(dispatch)
    .await;
    assert_eq!(status, StatusCode::OK, "the handler did not run");

    sink.transcript()
}

/// The access line has always carried the id. This pins the half that
/// already worked, so a failure of the other assertion cannot be blamed on
/// the harness capturing nothing.
#[tokio::test]
async fn the_access_line_carries_the_request_id() {
    let logs = transcript().await;
    assert!(
        logs.contains("request_id"),
        "no access line was captured at all, so this suite proves nothing:\n{logs}",
    );
    assert!(
        logs.contains("/reserve"),
        "the access line did not name the path:\n{logs}",
    );
}

/// The regression. A handler's own event must inherit the request span's
/// fields, or a log line from inside a request cannot be tied to it.
#[tokio::test]
async fn a_handler_event_inherits_the_request_id_from_the_span() {
    let logs = transcript().await;

    let handler_line = logs
        .lines()
        .find(|line| line.contains("reserving stock"))
        .unwrap_or_else(|| {
            panic!("the handler's own event was never logged, so the assertion below would be vacuous:\n{logs}")
        });

    assert!(
        handler_line.contains("order_id"),
        "the handler's own field is missing, so this is not the line we think it is:\n{handler_line}",
    );
    assert!(
        handler_line.contains("request_id"),
        "a handler event did not inherit `request_id` from the request span. \
         The span is being built and not entered -- see the module \
         documentation on this file.\n{handler_line}",
    );
    assert!(
        handler_line.contains("/reserve"),
        "a handler event did not inherit `path` from the request span:\n{handler_line}",
    );
}
