//! A mock OTLP collector: a real gRPC server that the real exporter talks to.
//!
//! `src/observe/otel.rs` builds an `opentelemetry-otlp` pipeline and hands
//! back a `tracing` layer. Everything between "a span was entered" and "a
//! collector has it" -- the SDK's batch processor, the OTLP transform, the
//! protobuf encoding, the gRPC call -- is code this crate configures and does
//! not own, which is exactly the code a unit test cannot reach. The module's
//! own test file says so in as many words: an exporter test "would need a
//! live collector, which is an integration concern". This is that collector.
//!
//! It is a genuine `TraceService` server bound to a kernel-assigned loopback
//! port, not a stub for the exporter type. The spans asserted on downstream
//! are the ones that came off the wire, decoded from the protobuf the
//! exporter actually wrote, so an assertion here cannot pass because the test
//! and the exporter share a bug: they share nothing but the socket.
//!
//! # Why the dependencies are free
//!
//! `tonic` and `opentelemetry-proto` are dev-dependencies, and both are
//! already in `Cargo.lock` -- `opentelemetry-otlp`'s `grpc-tonic` feature
//! puts them there. What this module adds is tonic's server half next to the
//! client half already compiled, which pulls no package the lock does not
//! carry.
//!
//! # Shared between test binaries
//!
//! Two integration tests need a collector: the trace-context round trip and
//! the redaction proof. Each `tests/*.rs` file is its own binary, so a helper
//! either lives in a module both include or gets written twice -- and two
//! copies of a mock is two chances for the copy under the assertion to drift
//! from the copy that was reviewed.

// Each including binary uses a different part of this surface, and an
// integration-test module is compiled once per binary. Without this, the
// binary that does not call `wait_for_attribute` fails the build under
// `-D warnings` for a function the other one depends on.
#![allow(dead_code)]

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use opentelemetry_proto::tonic::collector::trace::v1::trace_service_server::{
    TraceService, TraceServiceServer,
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::trace::v1::Span;
use tonic::transport::server::TcpIncoming;

/// How long a test waits for the exporter to deliver before giving up.
///
/// The batch processor's scheduled delay is measured in seconds, so a
/// shutdown-triggered flush is the fast path and this is only the ceiling on
/// a slow machine. Long enough not to flake under a loaded CI runner, short
/// enough that a genuinely broken export fails the test rather than hanging
/// the suite until the harness times it out.
pub const DELIVERY_TIMEOUT: Duration = Duration::from_secs(20);

/// What the collector accumulates.
#[derive(Debug, Default)]
struct Received {
    /// Every span, flattened out of its resource and scope grouping.
    spans: Vec<Span>,
    /// Every resource attribute, as `key=value` pairs, kept separately: the
    /// resource is where the service name travels, and a redaction test has
    /// to look at it too.
    resource_attributes: Vec<(String, String)>,
    /// How many export calls arrived. A test that asserts on span count
    /// wants to know whether it saw one batch or three.
    export_calls: usize,
}

/// The `TraceService` implementation. Holds nothing but the buffer.
#[derive(Debug, Default)]
struct MockCollector {
    received: Arc<Mutex<Received>>,
}

// The generated trait is an `#[async_trait]` trait, so the implementation
// has to be boxed the same way: a native `async fn` here is a different
// signature and does not implement it.
#[tonic::async_trait]
impl TraceService for MockCollector {
    async fn export(
        &self,
        request: tonic::Request<ExportTraceServiceRequest>,
    ) -> Result<tonic::Response<ExportTraceServiceResponse>, tonic::Status> {
        let mut received = lock(&self.received);
        received.export_calls += 1;
        for resource_spans in request.into_inner().resource_spans {
            if let Some(resource) = resource_spans.resource {
                for attribute in resource.attributes {
                    received
                        .resource_attributes
                        .push((attribute.key, render_value(attribute.value)));
                }
            }
            for scope_spans in resource_spans.scope_spans {
                received.spans.extend(scope_spans.spans);
            }
        }
        Ok(tonic::Response::new(ExportTraceServiceResponse {
            partial_success: None,
        }))
    }
}

/// Take the buffer lock, recovering from a poisoned mutex.
///
/// A panicking assertion in a test holds this lock for the length of one
/// `push`; poisoning it must not turn one failed assertion into a hang in
/// every other test sharing the collector.
fn lock(received: &Arc<Mutex<Received>>) -> MutexGuard<'_, Received> {
    received
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A collector listening on a loopback port, and the spans it has taken.
#[derive(Debug)]
pub struct RunningCollector {
    endpoint: String,
    received: Arc<Mutex<Received>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RunningCollector {
    fn drop(&mut self) {
        // The server loop never returns on its own. Without this, every test
        // that started a collector leaves one accepting connections for as
        // long as the test binary runs.
        self.task.abort();
    }
}

impl RunningCollector {
    /// Bind a collector to a kernel-assigned port and start serving.
    ///
    /// Port zero rather than a fixed number: the suite runs its test
    /// functions in parallel threads of one process, and two collectors on
    /// one port is a flake that only appears under load.
    pub async fn start() -> Self {
        let incoming = TcpIncoming::bind("127.0.0.1:0".parse().expect("a loopback address"))
            .expect("a loopback port");
        let address = incoming.local_addr().expect("the assigned port");
        let received = Arc::new(Mutex::new(Received::default()));
        let collector = MockCollector {
            received: Arc::clone(&received),
        };
        let task = tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(TraceServiceServer::new(collector))
                .serve_with_incoming(incoming)
                .await;
        });
        Self {
            // `http://`, not `https://`: this is loopback, and a TLS
            // handshake would only be testing rustls.
            endpoint: format!("http://{address}"),
            received,
            task,
        }
    }

    /// The OTLP endpoint to point a `Telemetry` builder at.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Every span received so far.
    #[must_use]
    pub fn spans(&self) -> Vec<Span> {
        lock(&self.received).spans.clone()
    }

    /// Every resource attribute received so far, as `key=value` pairs.
    #[must_use]
    pub fn resource_attributes(&self) -> Vec<(String, String)> {
        lock(&self.received).resource_attributes.clone()
    }

    /// How many `Export` calls have arrived.
    #[must_use]
    pub fn export_calls(&self) -> usize {
        lock(&self.received).export_calls
    }

    /// Wait until at least `count` spans have arrived, and return them.
    ///
    /// Polling rather than a notification channel because the thing being
    /// waited on is a batch processor on its own OS thread: there is nothing
    /// to await on this side, and a test that asserts immediately after
    /// `shutdown` is asserting on a race.
    ///
    /// # Panics
    ///
    /// If fewer than `count` spans arrive within [`DELIVERY_TIMEOUT`]. That
    /// is the failure this whole module exists to detect, so it is a panic
    /// with the count it did see rather than a returned `Option` a caller
    /// could ignore.
    pub async fn wait_for_spans(&self, count: usize) -> Vec<Span> {
        let deadline = std::time::Instant::now() + DELIVERY_TIMEOUT;
        loop {
            let spans = self.spans();
            if spans.len() >= count {
                return spans;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the collector received {} spans in {DELIVERY_TIMEOUT:?}, expected at least \
                 {count}: nothing reached the collector, so the exporter, the batch processor \
                 or the endpoint is wrong",
                spans.len()
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Every attribute of every received span, rendered as `key=value`.
    ///
    /// Rendered rather than typed because the redaction assertion is about
    /// the bytes on the wire: a secret is just as leaked in an `int_value`
    /// as in a `string_value`, and a test that only reads `string_value`
    /// would not notice.
    #[must_use]
    pub fn span_attributes(&self) -> Vec<(String, String)> {
        self.spans()
            .into_iter()
            .flat_map(|span| {
                span.attributes
                    .into_iter()
                    .map(|attribute| (attribute.key, render_value(attribute.value)))
            })
            .collect()
    }
}

/// One received span, found by name.
///
/// # Panics
///
/// If no span of that name arrived, listing the names that did -- a wrong
/// name in an assertion and a span that never left the process are the same
/// symptom otherwise.
#[must_use]
pub fn named<'a>(spans: &'a [Span], name: &str) -> &'a Span {
    spans
        .iter()
        .find(|span| span.name == name)
        .unwrap_or_else(|| {
            let seen: Vec<&str> = spans.iter().map(|span| span.name.as_str()).collect();
            panic!("no span named {name} arrived; the collector saw {seen:?}")
        })
}

/// Lowercase hex, the way a trace id is written everywhere it is read.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('?'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('?'));
    }
    out
}

/// Render any OTLP value as the text a reader of the collector would see.
///
/// Every variant, not just the string one. A redaction assertion that only
/// looked at strings would miss a secret recorded as bytes, and the whole
/// point of looking at the wire is to look at all of it.
fn render_value(value: Option<opentelemetry_proto::tonic::common::v1::AnyValue>) -> String {
    let Some(value) = value.and_then(|value| value.value) else {
        return String::new();
    };
    match value {
        Value::StringValue(text) => text,
        Value::BoolValue(flag) => flag.to_string(),
        Value::IntValue(number) => number.to_string(),
        Value::DoubleValue(number) => number.to_string(),
        Value::BytesValue(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Value::ArrayValue(array) => array
            .values
            .into_iter()
            .map(|value| render_value(Some(value)))
            .collect::<Vec<_>>()
            .join(","),
        Value::KvlistValue(list) => list
            .values
            .into_iter()
            .map(|entry| format!("{}={}", entry.key, render_value(entry.value)))
            .collect::<Vec<_>>()
            .join(","),
        // OTLP 1.8 added an experimental string table: a value may arrive as
        // an index into a table held elsewhere in the message rather than as
        // bytes of its own. Nothing in this tree emits one today. If that
        // ever changes, every assertion in the redaction suite that searches
        // attribute text goes blind rather than red -- an absence assertion
        // would pass because the value is unreadable, not because it is
        // absent. A panic is the only honest answer a helper that cannot see
        // the table can give.
        Value::StringValueStrindex(index) => panic!(
            "the exporter sent attribute values through the OTLP string table              (index {index}); this helper cannot resolve them, and any test              searching the wire for a secret is now blind rather than green"
        ),
    }
}
