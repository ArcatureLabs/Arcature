//! Traces reach a collector, and arrive as a trace rather than as debris.
//!
//! `src/observe/otel.rs` is 159 lines of pipeline construction with two unit
//! tests, both of which read fields back off a builder. Its own test module
//! says why: building the pipeline "would need a live collector, which is an
//! integration concern". Everything the module exists to do therefore had no
//! test at all -- not that a span is exported, not that the batch processor
//! flushes on shutdown, and not that a parent and a child arrive joined.
//!
//! That last one is the point. A span that reaches a collector unlinked is
//! worse than a span that never arrives: a missing trace is an outage
//! somebody investigates, while a trace whose spans are all roots looks like
//! working telemetry right up until the first time somebody needs it to
//! explain a slow request. The W3C model is two claims -- a child carries its
//! parent's trace id, and names its parent's span id as its parent -- and
//! both are asserted here against the bytes that came off the socket.
//!
//! The collector is real (see [`otlp_collector`]): a `TraceService` gRPC
//! server on a loopback port, decoding the protobuf the exporter wrote. The
//! test and the exporter share nothing but the socket, so an assertion here
//! cannot pass by sharing a bug with the code it is testing.
//!
//! Nothing here needs the network or a secret, so it runs on a pull request
//! from a fork exactly as it runs on a laptop.

#![cfg(feature = "otel")]

use std::time::Duration;

use arcature::observe::Telemetry;
use tracing_subscriber::layer::SubscriberExt as _;

mod otlp_collector;

use otlp_collector::{RunningCollector, hex, named};

/// The multi-threaded flavour is load-bearing, twice over.
///
/// The SDK's batch processor runs on an OS thread of its own and exports
/// with a blocking `block_on`, while the tonic channel it exports through is
/// a task on this runtime. On a current-thread runtime the blocking
/// `shutdown` below would park the only thread that could drive the export
/// it is waiting for -- the deadlock `BatchSpanProcessor`'s own
/// documentation warns about. `spawn_blocking` for the shutdown is the other
/// half of the same precaution.
macro_rules! collector_test {
    ($(#[$attribute:meta])* async fn $name:ident() $body:block) => {
        $(#[$attribute])*
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $name() $body
    };
}

/// Build a pipeline pointed at `collector`, run `emit` under it, and flush.
///
/// The flush is [`Telemetry::shutdown`], not a sleep: shutdown is the call
/// an application makes on the way out of `main`, and "the last batch is
/// exported when the process ends" is exactly the claim worth pinning.
async fn export_under_telemetry(
    collector: &RunningCollector,
    service_name: &str,
    emit: impl FnOnce(),
) {
    let telemetry = Telemetry::builder(service_name)
        .endpoint(collector.endpoint())
        .timeout(Duration::from_secs(5))
        .build()
        .expect("the OTLP pipeline builds");
    let subscriber = tracing_subscriber::registry().with(telemetry.tracing_layer());
    tracing::subscriber::with_default(subscriber, emit);
    tokio::task::spawn_blocking(move || telemetry.shutdown())
        .await
        .expect("the shutdown ran to completion")
        .expect("the exporter flushed and stopped cleanly");
}

collector_test! {
    /// The end-to-end claim: a span entered in application code is a span a
    /// collector holds. Everything else in this file refines it.
    async fn a_span_entered_under_the_layer_arrives_at_the_collector() {
        let collector = RunningCollector::start().await;
        export_under_telemetry(&collector, "arcature-otlp-arrival", || {
            let span = tracing::info_span!("checkout");
            let _entered = span.enter();
        })
        .await;

        let spans = collector.wait_for_spans(1).await;
        let checkout = named(&spans, "checkout");
        assert_eq!(
            checkout.trace_id.len(),
            16,
            "a trace id is sixteen bytes on the wire"
        );
        assert_eq!(
            checkout.span_id.len(),
            8,
            "a span id is eight bytes on the wire"
        );
        assert_ne!(
            checkout.trace_id, [0_u8; 16],
            "an all-zero trace id is invalid and joins nothing"
        );
        assert_ne!(
            checkout.span_id, [0_u8; 8],
            "an all-zero span id is invalid and is nobody's parent"
        );
        assert!(
            checkout.end_time_unix_nano >= checkout.start_time_unix_nano,
            "a span that ended before it started: {} then {}",
            checkout.start_time_unix_nano,
            checkout.end_time_unix_nano
        );
    }
}

collector_test! {
    /// One trace id across the nesting. Two roots would render as two
    /// unrelated requests in every backend that reads them.
    async fn a_parent_and_its_child_arrive_under_one_trace_id() {
        let collector = RunningCollector::start().await;
        export_under_telemetry(&collector, "arcature-otlp-trace-id", || {
            let outer = tracing::info_span!("handle_request");
            let _outer = outer.enter();
            let inner = tracing::info_span!("charge_card");
            let _inner = inner.enter();
        })
        .await;

        let spans = collector.wait_for_spans(2).await;
        let parent = named(&spans, "handle_request");
        let child = named(&spans, "charge_card");
        assert_eq!(
            hex(&parent.trace_id),
            hex(&child.trace_id),
            "the child started a second trace instead of joining its parent's"
        );
        assert_ne!(
            hex(&parent.span_id),
            hex(&child.span_id),
            "two spans in one trace shared a span id"
        );
    }
}

collector_test! {
    /// The other half of the W3C claim: the child names the parent. A trace
    /// whose spans share an id but name no parents is a flat list, not a
    /// tree, and no waterfall view can be drawn from it.
    async fn the_child_names_its_parents_span_id_as_its_parent() {
        let collector = RunningCollector::start().await;
        export_under_telemetry(&collector, "arcature-otlp-parentage", || {
            let outer = tracing::info_span!("handle_request");
            let _outer = outer.enter();
            let inner = tracing::info_span!("charge_card");
            let _inner = inner.enter();
        })
        .await;

        let spans = collector.wait_for_spans(2).await;
        let parent = named(&spans, "handle_request");
        let child = named(&spans, "charge_card");
        assert_eq!(
            hex(&child.parent_span_id),
            hex(&parent.span_id),
            "the child's parent id is not the parent's span id"
        );
        assert!(
            parent.parent_span_id.is_empty() || parent.parent_span_id == [0_u8; 8],
            "the outermost span claims a parent that was never exported: {}",
            hex(&parent.parent_span_id)
        );
    }
}

collector_test! {
    /// Three levels, because a two-level test passes for an implementation
    /// that hangs every span off the trace's first one.
    async fn a_three_level_nesting_arrives_as_a_chain_rather_than_a_fan() {
        let collector = RunningCollector::start().await;
        export_under_telemetry(&collector, "arcature-otlp-chain", || {
            let outer = tracing::info_span!("handle_request");
            let _outer = outer.enter();
            let middle = tracing::info_span!("charge_card");
            let _middle = middle.enter();
            let inner = tracing::info_span!("call_gateway");
            let _inner = inner.enter();
        })
        .await;

        let spans = collector.wait_for_spans(3).await;
        let outer = named(&spans, "handle_request");
        let middle = named(&spans, "charge_card");
        let inner = named(&spans, "call_gateway");
        assert_eq!(hex(&middle.parent_span_id), hex(&outer.span_id));
        assert_eq!(
            hex(&inner.parent_span_id),
            hex(&middle.span_id),
            "the innermost span was hung off the root instead of its own parent"
        );
        let trace = hex(&outer.trace_id);
        for span in [outer, middle, inner] {
            assert_eq!(hex(&span.trace_id), trace, "{} left the trace", span.name);
        }
    }
}

collector_test! {
    /// `Telemetry::builder` requires a service name rather than defaulting
    /// one, on the grounds that a deployment reporting as `unknown_service`
    /// is a deployment nobody can find. That is only true if the name it
    /// requires actually travels.
    async fn the_service_name_travels_as_a_resource_attribute() {
        let collector = RunningCollector::start().await;
        export_under_telemetry(&collector, "arcature-otlp-service-name", || {
            let span = tracing::info_span!("checkout");
            let _entered = span.enter();
        })
        .await;

        collector.wait_for_spans(1).await;
        let attributes = collector.resource_attributes();
        assert!(
            attributes.iter().any(|(key, value)| key == "service.name"
                && value == "arcature-otlp-service-name"),
            "no service.name resource attribute in {attributes:?}"
        );
    }
}

collector_test! {
    /// A field recorded on a `tracing` span has to arrive as an attribute,
    /// or every structured field the framework's own layers record is
    /// invisible to the collector -- and the redaction suite next door,
    /// which asserts on what those attributes contain, would be asserting
    /// on an empty list and passing.
    async fn a_field_recorded_on_a_span_arrives_as_an_attribute() {
        let collector = RunningCollector::start().await;
        export_under_telemetry(&collector, "arcature-otlp-attributes", || {
            let span = tracing::info_span!("checkout", order_id = "order-42");
            let _entered = span.enter();
        })
        .await;

        collector.wait_for_spans(1).await;
        let attributes = collector.span_attributes();
        assert!(
            attributes
                .iter()
                .any(|(key, value)| key == "order_id" && value == "order-42"),
            "no order_id attribute in {attributes:?}"
        );
    }
}

collector_test! {
    /// A collector that is down is an operational event, not an application
    /// one. The export fails, the process carries on, and the failure
    /// surfaces where a caller can see it rather than as a panic on a
    /// background thread.
    async fn a_collector_that_never_answers_does_not_panic_the_application() {
        // Bind a port and drop the listener: the address is then almost
        // certainly free, which is what makes the connection fail rather
        // than hang. A hard-coded port might be in use by something that
        // does answer, and the test would prove nothing.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let address = listener.local_addr().expect("the assigned port");
        drop(listener);

        let telemetry = Telemetry::builder("arcature-otlp-absent-collector")
            .endpoint(format!("http://{address}"))
            .timeout(Duration::from_millis(250))
            .build()
            .expect("the pipeline builds even when nothing is listening");
        let subscriber = tracing_subscriber::registry().with(telemetry.tracing_layer());
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("checkout");
            let _entered = span.enter();
        });

        // Whether shutdown reports the failed export is the SDK's business.
        // What this pins is that the application reaches the line after it.
        let _ = tokio::task::spawn_blocking(move || telemetry.shutdown())
            .await
            .expect("the shutdown ran to completion rather than panicking");
    }
}

collector_test! {
    /// An incoming `traceparent` becomes the *exported* trace id, not just a
    /// string on a log line.
    ///
    /// This is the property distributed tracing is for and it was missing.
    /// `TraceContextLayer` parsed the header, opened a span carrying
    /// `trace_id` and `parent_span_id` as fields, and never called
    /// `set_parent` -- so `tracing-opentelemetry` minted a fresh trace id for
    /// the span it exported. A request arriving with a `traceparent` opened a
    /// *new* trace at this service, and the caller's half and this half never
    /// met in the backend. The log line looked right, which is what made it
    /// survive: the ids were there, on a record nothing joined on.
    ///
    /// The assertion therefore reads the id off the span the collector holds,
    /// not off a log field. Reading the field would pass against the bug.
    async fn an_incoming_traceparent_becomes_the_exported_trace_id() {
        use arcature::observe::{TraceContext, TraceContextLayer};
        use axum::Router;
        use axum::body::Body;
        use axum::http::Request;
        use axum::routing::get;
        use tower::ServiceExt as _;

        // A caller's trace, chosen so it is unmistakable in the output.
        const TRACEPARENT: &str =
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
        const PARENT_SPAN: &str = "00f067aa0ba902b7";

        let collector = RunningCollector::start().await;
        let telemetry = Telemetry::builder("arcature-otlp-continued")
            .endpoint(collector.endpoint())
            .timeout(Duration::from_secs(5))
            .build()
            .expect("the OTLP pipeline builds");
        let subscriber = tracing_subscriber::registry().with(telemetry.tracing_layer());
        let dispatch = tracing::Dispatch::new(subscriber);

        let router = Router::new()
            .route(
                "/checkout",
                get(|| async {
                    // The context must reach the handler as well as the span.
                    tracing::info!("inside the handler");
                    "ok"
                }),
            )
            .layer(TraceContextLayer);

        let request = Request::builder()
            .uri("/checkout")
            .header("traceparent", TRACEPARENT)
            .body(Body::empty())
            .expect("a well-formed request");

        // `with_subscriber` on the future rather than `with_default`: the
        // router is polled across await points and may resume on another
        // thread, where a thread-local subscriber would not be installed.
        {
            use tracing::instrument::WithSubscriber as _;
            let status = async move {
                router.oneshot(request).await.expect("the router answered").status()
            }
            .with_subscriber(dispatch)
            .await;
            assert_eq!(status, axum::http::StatusCode::OK);
        }

        tokio::task::spawn_blocking(move || telemetry.shutdown())
            .await
            .expect("the shutdown ran to completion")
            .expect("the exporter flushed and stopped cleanly");

        let spans = collector.wait_for_spans(1).await;
        let request_span = named(&spans, "arcature.request");

        assert_eq!(
            hex(&request_span.trace_id),
            TRACE_ID,
            "the exported span opened a new trace instead of joining the \
             caller's. `set_parent` is what links them; the `trace_id` field \
             on the log line is not.",
        );
        assert_eq!(
            hex(&request_span.parent_span_id),
            PARENT_SPAN,
            "the exported span has no remote parent, so a backend cannot draw \
             the edge from the caller's span to this one",
        );

        // And the extension still reaches application code, which is the
        // other half of what the layer is for.
        let _ = TraceContext::from_headers(&axum::http::HeaderMap::new());
    }
}
