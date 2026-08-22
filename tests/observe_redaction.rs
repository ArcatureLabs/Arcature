//! Redaction, on the wire, in all three channels at once.
//!
//! `src/observe/mod.rs` makes a specific promise about telemetry:
//!
//! > None of the following ever reaches a log line, a metric label, or a
//! > span attribute: [...] credentials of every kind -- passwords, password
//! > hashes, API keys, bearer tokens, OAuth access and refresh tokens, PKCE
//! > verifiers, CSRF state, session identifiers, cookies, and
//! > `Authorization` headers.
//!
//! Three destinations are named. The mechanism behind the promise,
//! [`redact::is_sensitive`], is consulted in exactly two files --
//! `json_log.rs` and `access_log.rs` -- and both of them write log lines.
//! `metrics.rs`, `otel.rs` and `trace_context.rs` never call it. Whether the
//! promise holds for the other two destinations is therefore not something
//! the unit tests can answer, and it is not something the module's own
//! wording leaves any room to be vague about.
//!
//! So this file drives one request carrying a password, an `Authorization:
//! Bearer` header, a session cookie and a PKCE verifier through a router
//! wearing the whole stack -- request ids, access logging, metrics, trace
//! context -- with a JSON log sink, a metrics registry and a real OTLP
//! collector all capturing at once, and then looks for each secret's *value*
//! in every byte of all three outputs.
//!
//! # What it found
//!
//! The promise holds for the framework's own layers: a request stuffed with
//! secrets produces logs, metrics and spans with none of them in it.
//!
//! It does not hold for the second mechanism the module describes -- "a
//! field an application adds is covered by the same rule without the
//! application having to remember it". That is true of the JSON log and only
//! of the JSON log. A field named `password` recorded on a `tracing` span is
//! redacted on its way to a log line and exported to the OTLP collector in
//! full, because [`Telemetry::tracing_layer`] is
//! `tracing_opentelemetry::layer()` with no redacting visitor in front of
//! it. A metric label named `password` is rendered in full for the same
//! reason. Both are pinned below, as tests that assert the leak, named so
//! that nobody mistakes them for tests of a working defence:
//!
//! - `a_secret_recorded_as_a_span_field_reaches_the_collector_in_full`
//! - `a_secret_recorded_as_a_metric_label_is_rendered_in_full`
//!
//! Those two tests assert today's behaviour. If either starts failing
//! because the value is now redacted, the fix is to delete the test and the
//! caveat -- a failure there is the defence being extended, not a
//! regression.
//!
//! # Why a `tower::Service` call rather than a socket
//!
//! The request goes through the router as a `tower::Service` rather than
//! over a TCP connection. Every layer under test runs either way, and the
//! only thing a socket adds is hyper's parsing, which cannot redact or leak
//! anything. What it would cost is a second runtime and a thread boundary
//! between the code that sets the subscriber and the code that logs, which
//! is precisely the kind of arrangement that produces a green test capturing
//! nothing.
//!
//! [`redact::is_sensitive`]: arcature::observe::is_sensitive
//! [`Telemetry::tracing_layer`]: arcature::observe::Telemetry::tracing_layer

#![cfg(feature = "otel")]

use std::collections::HashMap;
use std::time::Duration;

use arcature::observe::{
    AccessLogLayer, CaptureSink, JsonLog, Metrics, MetricsLayer, RequestIdLayer, Telemetry,
    TraceContextLayer, is_sensitive,
};
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::routing::post;
use tower::ServiceExt as _;
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::layer::SubscriberExt as _;

mod otlp_collector;

use otlp_collector::RunningCollector;

// ---------------------------------------------------------------------------
// The secrets
// ---------------------------------------------------------------------------
//
// Each is a distinctive literal that could not appear in a log line by
// accident: a substring search for one of these finding a hit is a leak and
// never a coincidence.

/// The form field a sign-in carries.
const PASSWORD: &str = "correct-horse-battery-staple-91";
/// The credential in the `Authorization` header, without the scheme.
const BEARER: &str = "AbCdEf0123456789-the-bearer-credential";
/// The value of the session cookie.
const SESSION: &str = "th3-session-identifier-value-xyz";
/// A PKCE verifier, in the RFC 7636 shape.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
/// The username, which is not a secret and must survive: a test that proves
/// nothing is logged would also pass against a logger that logs nothing.
const USERNAME: &str = "ada";

/// Every secret, with the name an assertion failure should call it.
const SECRETS: &[(&str, &str)] = &[
    ("the password", PASSWORD),
    ("the bearer credential", BEARER),
    ("the session cookie", SESSION),
    ("the PKCE verifier", VERIFIER),
];

// ---------------------------------------------------------------------------
// The handlers
// ---------------------------------------------------------------------------

/// An application that keeps its secrets to itself.
///
/// It logs the one field that is not a credential. Everything the request
/// carried reaches the observability stack only through the framework's own
/// layers, which is the arrangement the module's promise is about.
async fn discreet_handler(headers: HeaderMap, body: String) -> &'static str {
    // Read them, so the handler is not passing by virtue of never having
    // touched the values.
    let form = parse_form(&body);
    let _ = headers.get(header::AUTHORIZATION);
    let _ = form.get("password");
    tracing::info!(username = USERNAME, "sign-in attempt");
    "ok"
}

/// An application that hands every secret to the observability stack under
/// the most honest name available.
///
/// This is not a straw man. It is what the module documentation tells an
/// application to do -- record structured fields and let the deny-list
/// decide -- and every field name below is on `DENY_LIST`. What the stack
/// does with them afterwards is the question this file exists to answer.
async fn candid_handler(
    State(metrics): State<Metrics>,
    headers: HeaderMap,
    body: String,
) -> &'static str {
    let form = parse_form(&body);
    let password = form.get("password").cloned().unwrap_or_default();
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    let authorization = header_text(&headers, header::AUTHORIZATION);
    let cookie = header_text(&headers, header::COOKIE);

    let span = tracing::info_span!(
        "sign_in",
        username = USERNAME,
        password = %password,
        authorization = %authorization,
        cookie = %cookie,
        code_verifier = %verifier,
    );
    let _entered = span.enter();
    tracing::info!(
        username = USERNAME,
        password = %password,
        authorization = %authorization,
        cookie = %cookie,
        code_verifier = %verifier,
        "sign-in attempt",
    );
    // A label, not a field. `Metrics` has no deny-list of its own.
    metrics.increment("sign_in_attempts_total", &[("session_id", &cookie)], 1);
    "ok"
}

/// An application that records a secret under a name nobody thought of.
///
/// `note` is not on the deny-list and never could be: the list is a list of
/// names, and a name that carries a secret only sometimes cannot be on it.
async fn careless_handler() -> &'static str {
    tracing::info!(note = PASSWORD, "sign-in attempt");
    "ok"
}

fn parse_form(body: &str) -> HashMap<String, String> {
    serde_urlencoded::from_str(body).unwrap_or_default()
}

fn header_text(headers: &HeaderMap, name: header::HeaderName) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// The harness
// ---------------------------------------------------------------------------

/// Everything the stack emitted for one request.
#[derive(Debug)]
struct Captured {
    logs: String,
    exposition: String,
    span_attributes: Vec<(String, String)>,
    resource_attributes: Vec<(String, String)>,
    span_names: Vec<String>,
}

impl Captured {
    /// The three channels, named, as text an assertion can search.
    fn channels(&self) -> Vec<(&'static str, String)> {
        vec![
            ("the JSON log", self.logs.clone()),
            ("the metrics exposition", self.exposition.clone()),
            (
                "the exported span attributes",
                render(&self.span_attributes),
            ),
            (
                "the exported resource attributes",
                render(&self.resource_attributes),
            ),
        ]
    }

    /// Fail if `secret` appears anywhere, naming the channel that leaked it.
    fn assert_absent(&self, what: &str, secret: &str) {
        for (channel, text) in self.channels() {
            assert!(
                !text.contains(secret),
                "{what} leaked into {channel}:\n{text}"
            );
        }
    }

    /// Fail unless every channel actually captured something.
    ///
    /// Without this, a harness that silently captured nothing would pass
    /// every absence assertion in the file. It is the single most important
    /// check here, because it is the one that makes the others mean
    /// anything.
    fn assert_nothing_is_vacuous(&self) {
        assert!(
            self.logs.contains(USERNAME),
            "the log sink captured nothing recognisable: {:?}",
            self.logs
        );
        assert!(
            self.exposition.contains("http_requests_total"),
            "the metrics layer recorded nothing: {:?}",
            self.exposition
        );
        assert!(
            !self.span_names.is_empty(),
            "no span reached the collector, so the span channel proves nothing"
        );
        assert!(
            !self.span_attributes.is_empty(),
            "spans arrived with no attributes at all, so searching them proves nothing"
        );
    }
}

fn render(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The request every test sends: one sign-in carrying four credentials.
///
/// The PKCE verifier is in the query string as well as the body. A query
/// string is the part of a URL that ends up in every proxy log there is, and
/// the access log's decision to record `uri.path()` rather than the whole
/// URI is the thing that keeps it out of this one.
fn loaded_request() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/sign-in?code_verifier={VERIFIER}&next=/dashboard"))
        .header(header::AUTHORIZATION, format!("Bearer {BEARER}"))
        .header(header::COOKIE, format!("session_id={SESSION}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-request-id", "req-redaction-suite")
        .body(Body::from(format!(
            "username={USERNAME}&password={PASSWORD}&code_verifier={VERIFIER}"
        )))
        .expect("the request builds")
}

/// Which handler sits at the bottom of the stack.
#[derive(Debug, Clone, Copy)]
enum Handler {
    Discreet,
    Candid,
    Careless,
}

/// Run one request through the whole stack and capture all three channels.
///
/// The subscriber is attached to the future with [`WithSubscriber`] rather
/// than installed with `with_default`. `with_default` is thread-local, and a
/// future on a multi-threaded runtime may resume on another thread between
/// two awaits -- at which point the log lines the assertion is about are
/// written to no subscriber at all and the test passes by capturing nothing.
///
/// [`WithSubscriber`]: tracing::instrument::WithSubscriber
async fn drive(handler: Handler, spans_expected: usize) -> Captured {
    let collector = RunningCollector::start().await;
    let telemetry = Telemetry::builder("arcature-redaction-suite")
        .endpoint(collector.endpoint())
        .timeout(Duration::from_secs(5))
        .build()
        .expect("the OTLP pipeline builds");
    let sink = CaptureSink::new();
    let metrics = Metrics::new();

    let router = match handler {
        Handler::Discreet => Router::new().route("/sign-in", post(discreet_handler)),
        Handler::Candid => Router::new().route("/sign-in", post(candid_handler)),
        Handler::Careless => Router::new().route("/sign-in", post(careless_handler)),
    };
    // Innermost first. `RequestIdLayer` has to run before `AccessLogLayer`
    // or the id is not in the extensions when the access line is written,
    // so it is applied after it and therefore wraps it.
    let router: Router = router
        .layer(MetricsLayer::new(metrics.clone()))
        .layer(AccessLogLayer)
        .layer(RequestIdLayer)
        .layer(TraceContextLayer)
        .with_state(metrics.clone());

    let subscriber = tracing_subscriber::registry()
        .with(JsonLog::new(sink.clone()))
        .with(telemetry.tracing_layer());
    let dispatch = tracing::Dispatch::new(subscriber);

    let status = async move {
        router
            .oneshot(loaded_request())
            .await
            .expect("the router answered")
            .status()
    }
    .with_subscriber(dispatch)
    .await;
    assert_eq!(status, StatusCode::OK, "the handler did not run");

    tokio::task::spawn_blocking(move || telemetry.shutdown())
        .await
        .expect("the shutdown ran to completion")
        .expect("the exporter flushed and stopped cleanly");
    let spans = collector.wait_for_spans(spans_expected).await;

    Captured {
        logs: sink.transcript(),
        exposition: metrics.render(),
        span_attributes: collector.span_attributes(),
        resource_attributes: collector.resource_attributes(),
        span_names: spans.into_iter().map(|span| span.name).collect(),
    }
}

// ---------------------------------------------------------------------------
// The framework's own layers
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_secret_in_the_request_reaches_any_channel_through_the_framework_layers() {
    // The headline claim, tested against every channel at once rather than
    // one assertion per file. A request carrying four credentials, through
    // request ids, access logging, metrics and trace context, with the
    // application logging only a username.
    let captured = drive(Handler::Discreet, 1).await;
    captured.assert_nothing_is_vacuous();
    for (what, secret) in SECRETS {
        captured.assert_absent(what, secret);
    }
    // And the scheme word on its own, since `Bearer eyJ...` split across a
    // line boundary would still be a leak of the header.
    for (channel, text) in captured.channels() {
        assert!(
            !text.contains("Bearer "),
            "an Authorization header value reached {channel}:\n{text}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_access_log_records_the_path_and_leaves_the_query_string_behind() {
    // A query string is the part of a URL that reaches every proxy log on
    // the way. `AccessLogService` records `uri.path()`, and this is what
    // stops that from being `uri` one careless refactor later.
    let captured = drive(Handler::Discreet, 1).await;
    assert!(
        captured.logs.contains("\"path\":\"/sign-in\""),
        "no path field in the access line: {}",
        captured.logs
    );
    assert!(
        !captured.logs.contains("next=/dashboard"),
        "the query string reached the log: {}",
        captured.logs
    );
    captured.assert_absent("the PKCE verifier in the query string", VERIFIER);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_metric_labels_the_framework_chooses_are_a_bounded_set() {
    // The other half of why metrics do not leak: `MetricsLayer` labels with
    // the method, the status and an optional `&'static str` route, and there
    // is no path by which a request supplies a label name or value. An
    // unbounded label value is also how a scrape target runs out of memory,
    // so this test guards a availability property as well as a secrecy one.
    let captured = drive(Handler::Discreet, 1).await;
    assert!(captured.exposition.contains("method=\"POST\""));
    assert!(captured.exposition.contains("status=\"200\""));
    assert!(
        !captured.exposition.contains("/sign-in"),
        "the request path became a metric label: {}",
        captured.exposition
    );
}

// ---------------------------------------------------------------------------
// What the deny-list covers when the application records the secret itself
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deny_listed_field_is_redacted_in_the_json_log() {
    // The mechanism working, in the one channel that consults it. Both an
    // event field and a span field, because `JsonLog` folds span fields into
    // each event and a visitor applied to only one of the two would pass a
    // test that looked at only one of them.
    let captured = drive(Handler::Candid, 2).await;
    for name in ["password", "authorization", "cookie", "code_verifier"] {
        assert!(is_sensitive(name), "{name} is not on the deny-list");
        assert!(
            captured
                .logs
                .contains(&format!("\"{name}\":\"[redacted]\"")),
            "{name} was not redacted in the log: {}",
            captured.logs
        );
    }
    for (what, secret) in SECRETS {
        assert!(
            !captured.logs.contains(secret),
            "{what} reached the JSON log: {}",
            captured.logs
        );
    }
    assert!(
        captured.logs.contains(USERNAME),
        "the ordinary field was redacted too, which would make the deny-list useless"
    );
}

// ---------------------------------------------------------------------------
// Where it stops. These assert the leak.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_recorded_as_a_span_field_reaches_the_collector_in_full() {
    // `Telemetry::tracing_layer` is `tracing_opentelemetry::layer()` with a
    // tracer attached and nothing in front of it. `tracing_opentelemetry`
    // has its own field visitor, which copies fields onto the OTel span
    // verbatim; `redact::is_sensitive` is never asked. The same field that
    // the JSON layer wrote as `[redacted]` in the test above leaves the
    // process here in plaintext, over the wire, to a collector.
    //
    // `src/observe/mod.rs` says none of these "ever reaches a log line, a
    // metric label, or a span attribute", and `src/observe/otel.rs` says "a
    // field the log layer redacts is a field that must not be recorded on a
    // span either". Both are true as intentions and neither is enforced.
    //
    // This test asserts the leak so that the gap is visible in the suite
    // rather than in an incident. If it fails because the value is now
    // redacted, the defence has been extended: delete this test.
    let captured = drive(Handler::Candid, 2).await;
    captured.assert_nothing_is_vacuous();
    let attributes = render(&captured.span_attributes);
    assert!(
        attributes.contains(PASSWORD),
        "the password no longer reaches the collector -- if redaction was added to the OTLP \
         layer, this test has done its job and should be deleted:\n{attributes}"
    );
    for (what, secret) in SECRETS {
        assert!(
            attributes.contains(secret),
            "{what} does not reach the collector, but the others do -- the gap is now partial \
             and this test is describing it wrongly:\n{attributes}"
        );
    }
    // And to be unambiguous about which side of the boundary the leak is
    // on: the very same values were redacted in the log.
    assert!(
        !captured.logs.contains(PASSWORD),
        "the log leaked too, which is a second and separate defect"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_recorded_as_a_metric_label_is_rendered_in_full() {
    // `Metrics::increment` takes label values as `&str` and renders them
    // escaped but unredacted -- `metrics.rs` never calls `redact`. The
    // framework's own layer cannot hit this, because the only label values
    // it supplies are a method, a status and a `&'static str` route; an
    // application that labels a series with a session id can.
    //
    // As above: this asserts today's behaviour, and a failure here means
    // redaction reached the metrics registry.
    let captured = drive(Handler::Candid, 2).await;
    assert!(
        captured.exposition.contains(SESSION),
        "the session id no longer reaches the exposition -- if redaction was added to \
         `Metrics`, delete this test:\n{}",
        captured.exposition
    );
    assert!(
        is_sensitive("session_id"),
        "the label name is on the deny-list, and the deny-list is not consulted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_under_a_field_name_nobody_denied_is_logged_in_full() {
    // The structural limit of a name-based deny-list, stated as a test
    // rather than left as a footnote. `note` carries a password here and
    // could carry a postcode in the next handler; no list of names can
    // catch that, and the defence is that the framework's own layers never
    // record a field whose contents they have not chosen.
    let captured = drive(Handler::Careless, 1).await;
    assert!(!is_sensitive("note"));
    assert!(
        captured.logs.contains(PASSWORD),
        "if `note` is now redacted the deny-list has grown, and this test needs a name that \
         is still outside it:\n{}",
        captured.logs
    );
}

#[test]
fn the_deny_list_is_written_in_snake_case_and_therefore_misses_camel_case_spellings() {
    // `-` and `.` are folded to `_` before the substring test, so the header
    // and attribute spellings are caught. camelCase has no separator to fold:
    // `privateKey` lowercases to `privatekey`, and the needle is
    // `private_key`.
    //
    // Narrower than it looks, and worth saying why rather than leaving a
    // reader to work it out. Only the multi-word needles are exposed --
    // `apiKey` is still caught by `apikey`, `accessToken` by `token` -- and
    // Rust field names are snake_case, so what is left is an application
    // that records a JSON body's keys under the names the client chose.
    assert!(is_sensitive("x_api_key"));
    assert!(is_sensitive("apikey"));
    assert!(is_sensitive("authorization"));
    assert!(is_sensitive("set-cookie"));
    assert!(is_sensitive("x-api-key"));
    assert!(is_sensitive("http.request.header.authorization"));
    assert!(is_sensitive("apiKey"));
    assert!(is_sensitive("accessToken"));
    assert!(
        !is_sensitive("privateKey"),
        "camelCase is now covered, which is a fix -- narrow this test to whatever spelling \
         is still missed, or delete it"
    );
    assert!(!is_sensitive("sessionId"));
}
