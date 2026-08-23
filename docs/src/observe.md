# Observability

Request ids, newline-delimited JSON logs, an access log, a Prometheus text
endpoint, W3C trace context, and OTLP export. All of it sits on `tracing`,
which the framework re-exports as `arcature::observe::tracing` so downstream
code targets the pinned version through Arcature.

Nothing here installs itself. There is no global recorder, no global tracer
provider, and no subscriber the crate registers on import — a `JsonLog` sink,
a `Metrics` registry and a `Telemetry` pipeline are values the application
holds and clones, and the subscriber is installed by a call the application
makes from `main`. The rejected alternative is the usual one: a library that
grabs the global subscriber when it is linked. It costs the binary the ability
to choose, and it costs a test the ability to capture only its own output.

## Turning it on

```toml
# Logs, request ids, the access log, the Prometheus registry, trace context.
arcature = { version = "0.1", features = ["observe"] }

# The same, plus OTLP span export. `otel` implies `observe`.
arcature = { version = "0.1", features = ["otel"] }
```

| Feature | Gives you | Pulls |
| --- | --- | --- |
| `observe` | `install_logging`, `JsonLog`, `RequestId` + `RequestIdLayer`, `AccessLogLayer`, `Metrics` + `MetricsLayer`, `TraceContext` + `TraceContextLayer`, `redact` | `tracing`, `tracing-subscriber`, `uuid` |
| `otel` | `Telemetry`, `TelemetryBuilder`, the `observe::otel` module | `observe`, `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`, `tracing-opentelemetry` |

| Where | `observe` | `otel` |
| --- | --- | --- |
| framework `default` | on | off |
| framework `fullstack` | on | off |
| generated application | on | off, and `arc new` scaffolds nothing for it |

`otel = ["observe", ...]`, so it is an addition and never an alternative. It
is an operator opt-in: four crates and the gRPC stack under them enter the
graph, and most applications never enable it.

`otel` adds the OTLP **span** exporter and nothing else. The Prometheus
endpoint belongs to `observe` — `observe::metrics` is not gated on `otel`,
whatever the comment above the feature in `Cargo.toml` suggests — so a
default build already has it.

`tracing-subscriber` is pinned with `registry` and `fmt` and nothing else.
`ansi` is off, so colour is unavailable rather than disabled; `env-filter` is
off, so the filter is `Targets` rather than `EnvFilter`. `json` is off too,
which is why the JSON formatter in this module is written by hand. That last
one is not a saving, it is the design: the layer owns the serialisation, so
[redaction](#redaction) runs on every field with no way for a caller to opt
out of it.

## Installing the subscriber

```rust,ignore
// The first line of the generated application's `run`, before anything that
// might have something to say.
arcature::observe::install_logging(LOG_FILTER).map_err(std::io::Error::other)?;
```

`install_logging(default_filter) -> Result<(), ObserveError>`. Call it once,
from `main`, before anything that might log. `tracing` events go nowhere
at all until a subscriber exists: without this call the access log runs on
every request and emits into the void, and a job that fails does so without a
line anywhere. Nothing errors — the process is quiet, which is the worst way
for logging to be broken, because it looks like nothing is happening.

The format is chosen by build profile, not by an environment variable, on the
grounds that the shape of a log line is a property of the build:

| Build | Layer | Redaction |
| --- | --- | --- |
| debug (`cfg!(debug_assertions)`) | `tracing_subscriber::fmt`, no ANSI, target shown | **none** |
| release | `JsonLog` on `StderrSink` | every field |

Both write to standard error, so a log line never interleaves with what the
process writes to standard output.

The redaction column is not a typo, and it is the single most misread thing in
this module. `install_logging` installs `JsonLog` in release builds only; a
debug build gets the `fmt` layer, which prints every field it is given,
verbatim, including one named `password`. An application that wants redaction
while developing composes its own subscriber with `JsonLog` in it (see
[the sinks](#sinks)) rather than calling `install_logging`.

`default_filter` is used when `RUST_LOG` is unset. The variable name is in
`FILTER_ENV`, and it is `RUST_LOG` and not `ARCATURE_LOG` because every Rust
operator already knows the name.

| `RUST_LOG` | `default_filter` | Result |
| --- | --- | --- |
| unset, or blank | parses | the default |
| set, parses | — | the variable |
| set, does not parse | parses | warning on stderr, then the default |
| unset | does not parse | warning on stderr, then `info` |

A typo in a log filter must not stop a process from booting, and an
application whose own hard-coded default does not parse has a bug that must
not be silent either — hence a warning in both directions and a running
process in both directions.

`Targets` rather than `EnvFilter` is a deliberate trade. `EnvFilter` brings a
regex engine along for span-field matching that a web application almost never
uses; `Targets` reads the `target=level` syntax people actually write —
`info`, `info,sqlx=warn`, `my_app=debug` — and costs no additional
dependency.

`install_logging` returns `Err(ObserveError::Logging)` if a global subscriber
is already installed. That is a real error rather than a no-op: the second
caller's configuration is being discarded, and the honest response is to say
so.

The generated application calls it on the first line of `run`, with a filter
chosen the same way:

| Build | `LOG_FILTER` |
| --- | --- |
| debug | `info,<app>=debug,arcature=debug` |
| release | `info` |

## What a log line looks like

One JSON object per line, no pretty printing, no trailing state. Every
mainstream shipper reads that without configuration.

```json
{"fields":{"client_ip":"203.0.113.9","duration_ms":7,"method":"GET","path":"/dashboard","request_id":"6f1e6f8c-6e5e-4a2b-9a24-6f3a2f0f1c77","status":200},"level":"INFO","message":"GET /dashboard 200 7ms","target":"arcature::observe::access_log","timestamp":"2026-08-23T10:15:04.220Z"}
```

| Key | Present | Value |
| --- | --- | --- |
| `timestamp` | always | RFC 3339 UTC, millisecond precision, always `Z` |
| `level` | always | `TRACE` / `DEBUG` / `INFO` / `WARN` / `ERROR` |
| `target` | always | the emitting module path |
| `message` | when the event recorded one | the formatted message, lifted out of the fields |
| `spans` | when the event is inside an entered span | span names, outermost first |
| `fields` | when there is at least one | the event's fields, plus inherited span fields |

Keys are serialised from a `BTreeMap`, so they come out in alphabetical order
— both at the top level and inside `fields`. Nothing depends on that; it is
what a reader will see.

`message` is only another field to `tracing`. Lifting it to a top-level key is
what makes the line readable to a human and indexable by everything else.

Span fields are folded into each event inside the span, with a nested span's
field beating its parent's and the event's own field beating both. The
timestamp is written by hand — Howard Hinnant's `civil_from_days` plus a
format string — because `chrono` belongs to the `database` feature and a
logging layer must not drag a database dependency behind it.

`JsonLog::without_span_fields()` drops the inherited fields and logs only what
the event itself recorded. Smaller lines, at the cost of losing whatever the
enclosing span was carrying.

### Sinks

`LogSink` is one method, `write_line(&self, line: &str)`, and the line arrives
without its terminator. A trait rather than `std::io::Write` because a sink is
shared across threads and must serialise whole lines: a writer that
interleaved two events would produce unparseable output.

| Sink | Use |
| --- | --- |
| `StderrSink` | standard error, one `writeln!` under the lock |
| `CaptureSink` | keeps lines in memory; cloning shares the buffer |

`CaptureSink` is how a test asserts on its own output without touching a
global:

```rust,ignore
use arcature::observe::{CaptureSink, JsonLog};
use tracing_subscriber::layer::SubscriberExt as _;

let sink = CaptureSink::new();
let subscriber = tracing_subscriber::registry().with(JsonLog::new(sink.clone()));
tracing::subscriber::with_default(subscriber, || {
    tracing::info!(user = "ada", password = "hunter2", "signed in");
});
assert!(sink.transcript().contains("[redacted]"));
```

`lines()` returns the vector, `transcript()` joins it. A poisoned mutex is
recovered from rather than propagated in both sinks: a panic in one log call
must not silence every later one.

## The request id

`RequestId` is a validated, low-cardinality identifier. `RequestIdLayer`
resolves it once per request and puts it in request extensions, and every
response carries it back as `x-request-id` — the wire-compatible name, with
no `X-Arcature-*` prefix, so a reverse proxy and a client library both already
understand it.

| Question | Answer |
| --- | --- |
| Where from | the inbound `x-request-id` header, if it parses; otherwise a fresh UUID v4 |
| Charset | ASCII alphanumerics plus `-` `_` `.` `:` `@` `+` `/` `=` |
| Maximum | `MAX_REQUEST_ID_BYTES`, 128 bytes |
| Rejected input | empty, oversized, or a disallowed byte |
| On rejection | a fresh id is generated — `RequestId::from_header` never errors |
| Response | `x-request-id`, on every response the layer sees |

Reusing the upstream value is what makes a trace survive the hop from a
reverse proxy. Validating it is what stops the same header from becoming an
injection point into every log index downstream: `RequestId::parse_str`
enforces the allow-list, and hostile input is replaced rather than reported,
because failing a request over a malformed correlation id would be a denial of
service with no upside.

`RequestIdError` is `Empty`, `TooLarge { size, limit }` or `InvalidChar`, and
only the parsing entry points return it.

### How it reaches a log line

`AccessLogService` reads the id out of request extensions and records it as
the `request_id` field on the access line it writes. That is the path, and it
is one hop: the id reaches the access line because the access line writes it,
not because anything is inherited.

Two consequences worth being exact about:

- The order matters. `RequestIdLayer` must run **outside** `AccessLogLayer` or
  the id is not in extensions when the access line is written. The
  [pipeline](deployment.md#the-pipeline) does this by construction — request
  id is stage 8 and the access log is stage 9 — and switching the request id
  off while leaving the log on produces lines with an empty id rather than an
  error.
- A handler's own `tracing::info!` does **not** automatically carry the
  request id. `AccessLogService` builds its `arcature.request` span and binds
  it, but never enters it, so no other event becomes a child of it. A handler
  that wants the id on its own lines reads `RequestId` from the request and
  records it, or opens its own span.

Both layers are off unless asked for:

```rust,ignore
Application::new()
    .request_id()
    .access_log()
```

## The access log

One `tracing` event per request, at `INFO`, from
`arcature::observe::access_log`. The message is `"{method} {path} {status}
{duration}ms"` and the structured fields are:

| Field | Type | Value |
| --- | --- | --- |
| `method` | string | the request method |
| `path` | string | `uri.path()` — the path only |
| `status` | number | the response status as `u16` |
| `duration_ms` | number | whole milliseconds, truncated |
| `request_id` | string | the resolved id, or `""` if `RequestIdLayer` did not run |
| `client_ip` | string | the resolved `ClientIp`, or `""` if nothing resolved one |

That is the whole line. No request body, no response body, no headers, and no
query string.

The query string is the interesting omission. It is the part of a URL that
ends up in every proxy log on the way, and applications put credentials in it
— an OAuth `code`, a PKCE `code_verifier`, a password-reset token.
`AccessLogService` records `uri.path()` and `tests/observe_redaction.rs` sends
a request whose query string carries a PKCE verifier and asserts it appears in
none of the outputs, so `uri` cannot quietly replace `uri.path()` one refactor
later.

An empty string rather than an absent field is deliberate for both optional
values: a reader can tell "not known here" from "this field was never part of
this log".

The client address is a field and never the message. An IP address is personal
data in most of the places this will run, and redaction decides per field name
— so an address interpolated into the human-readable message would be past
the only checkpoint there is. It is written as `client_ip` and passed through
`redact::apply("client_ip", ..)` on the way, using the same string for both,
so adding an address term to the deny-list would withhold it everywhere rather
than everywhere-except-the-message. A unit test asserts the name written and
the name asked about are the same string.

The layer sits outside the panic catcher, the body limit and the timeout, so a
`500`, a `413` and a `408` are all logged.

## Metrics

`Metrics` is a registry the application holds and clones. Two `Metrics` values
are two independent registries, which is what lets a test assert on exactly
its own counters; cloning shares one, so the handle given to a middleware and
the handle given to the `/metrics` route are the same set of series. There is
no global recorder and no macro that reaches for one.

| Call | Does |
| --- | --- |
| `Metrics::new()` | empty registry, `DEFAULT_BUCKETS` |
| `Metrics::with_buckets(&[..])` | same, with your bounds — sorted on the way in, non-finite values dropped |
| `describe_counter/gauge/histogram(name, help)` | attach `# TYPE` and `# HELP` for a name |
| `increment(name, labels, by)` | add to a counter series |
| `set(name, labels, value)` | set a gauge series |
| `observe(name, labels, value)` | record one histogram observation |
| `counter_value(name, labels)` | `Option<u64>`, for assertions |
| `render()` | the whole registry as Prometheus text |
| `response()` | the same, as an HTTP response with the content type |

Recording a series that was never described registers the name with the
implied kind and empty help, so a metric is never lost for want of a
description. Labels are a `BTreeMap` internally, so `[("a","1"),("b","2")]`
and `[("b","2"),("a","1")]` are one series and not two.

### The exposition

Prometheus text format, version 0.0.4 — the one every scraper reads,
including OpenMetrics parsers, which accept it as a subset.
`PROMETHEUS_CONTENT_TYPE` is `text/plain; version=0.0.4; charset=utf-8`.

```text
# HELP http_requests_total Total HTTP requests handled.
# TYPE http_requests_total counter
http_requests_total{method="GET",status="200"} 3
# HELP http_request_duration_seconds HTTP request duration in seconds.
# TYPE http_request_duration_seconds histogram
http_request_duration_seconds_bucket{method="GET",le="0.005"} 1
...
http_request_duration_seconds_bucket{method="GET",le="+Inf"} 3
http_request_duration_seconds_sum{method="GET"} 0.11
http_request_duration_seconds_count{method="GET"} 3
```

Series are grouped by name and emitted in name order, because the format
requires every sample of a name to sit under its one `# TYPE` line;
interleaving names is a parse error, not a style choice. `# HELP` is written
only when help text exists. Whole numbers render without a trailing `.0`, so a
bucket bound of `1.0` is `le="1"`. Label values are escaped for backslash,
quote and newline.

`tests/observe_prometheus.rs` parses the rendered document with a
purpose-written validator rather than a `contains` assertion, and the
validator is itself tested against documents that break each rule, because a
`contains` assertion cannot see whether the document around the substring
parses — and a scrape a scraper rejects is silent.

### The HTTP layer

`MetricsLayer::new(metrics)` records two series per request:

| Name | Type | Labels | Help |
| --- | --- | --- | --- |
| `http_requests_total` | counter | `method`, `status` | Total HTTP requests handled. |
| `http_request_duration_seconds` | histogram | `method` | HTTP request duration in seconds. |

`MetricsLayer::labelled(metrics, "/users/{id}")` adds a `route` label to both.

`DEFAULT_BUCKETS`, in seconds: `0.005`, `0.01`, `0.025`, `0.05`, `0.1`,
`0.25`, `0.5`, `1.0`, `2.5`, `5.0`, `10.0` — the set the Prometheus client
libraries ship, bracketing the range a web request lives in from a cached 5 ms
answer to a 10 s timeout.

There is deliberately no path label taken from the request URI. A URI is
unbounded input and an unbounded label value is how a scrape target runs out
of memory. The rejected alternative — label with `uri.path()` and hope —
costs the process its memory the first time a crawler walks a parameterised
route. An application that wants a route dimension installs the layer per
route or per group and names the template itself, and `route` is a
`&'static str` precisely so it cannot be a value derived from a request.

### Wiring it

Neither the registry nor the layer is installed by the application builder.
Nothing in the [pipeline](deployment.md#the-pipeline) constructs a `Metrics`,
and there is no `/metrics` route unless the application adds one:

```rust,ignore
use arcature::observe::{Metrics, MetricsLayer};

let metrics = Metrics::new();
let metrics_for_layer = metrics.clone(); // a clone shares the registry

let router = axum::Router::new()
    .route(
        "/metrics",
        axum::routing::get(move || {
            let metrics = metrics.clone();
            async move { metrics.response() }
        }),
    )
    .layer(MetricsLayer::new(metrics_for_layer));
```

An ordinary route, so it inherits whatever the application puts in front of
it. A metrics endpoint usually wants an allow-list or basic auth, and that
belongs to the application: the framework has no way to know which of the two
is right, and shipping either one as a default would be wrong for half the
deployments and invisible to the other half.

Constants live at `arcature::observe::metrics::{DEFAULT_BUCKETS,
HTTP_REQUESTS_TOTAL, HTTP_REQUEST_DURATION, PROMETHEUS_CONTENT_TYPE}`. Only
`Metrics`, `MetricsLayer` and `MetricsService` are re-exported at
`arcature::observe`.

## Trace context

`TraceContextLayer` reads W3C Trace Context off the inbound request, puts a
`TraceContext` in extensions, and opens a span carrying the ids so log lines
can be joined to the trace.

Inbound headers are untrusted. A malformed `traceparent` is discarded and a
fresh root started rather than propagated, because half a trace id is worse
than none: it silently corrupts every trace it joins.

| Rule | `traceparent` |
| --- | --- |
| Length | exactly 55 bytes, dashes at 2, 35 and 52 |
| Version | `00` only; anything else, including the reserved `ff`, is rejected |
| Hex case | lowercase only — accepting both would make two spellings of one id |
| Trace id | 16 bytes, all-zero rejected |
| Parent id | 8 bytes, all-zero rejected |
| Flags | one octet; bit 0 is `FLAG_SAMPLED` |

| Rule | `tracestate` |
| --- | --- |
| Length | 512 bytes maximum |
| Members | `MAX_TRACESTATE_MEMBERS`, 32 |
| Bytes | printable ASCII `0x20`--`0x7e` only |
| Without a valid `traceparent` | dropped — it describes a trace this request is not part of |
| On rejection | dropped; the trace itself still propagates |

The member cap and the byte range are not pedantry: `tracestate` is
attacker-controlled text that would otherwise be copied onto every outbound
request the service makes. It is stored as the original text rather than a
parsed member list, because the only operations performed on it are "carry it
forward" and "prepend our own member", and re-serialising a parsed form risks
normalising away something a downstream vendor depends on.

The layer records three fields on an `arcature.request` span, which it does
enter:

| Field | Meaning |
| --- | --- |
| `trace_id` | 32 lowercase hex characters |
| `parent_span_id` | 16 lowercase hex characters |
| `continued_trace` | `true` if an upstream trace was joined, `false` if this is a root |

`continued_trace` earns its place: a service at the edge starts roots and one
behind a gateway should not, and the difference is otherwise invisible.

It enters that span with `span.enter()` and holds the guard across the inner
call, rather than attaching it to the future with `Instrument`. The guard is
an entry on a thread-local stack and a future that yields does not unwind it,
so which of a handler's own events end up inheriting these fields depends on
how the runtime scheduled the task. Correlate on lines that record an id
themselves; treat span-field inheritance as a convenience, not a contract.

Nothing is written back onto the response. `traceparent` is a request header,
and echoing it would tell a client the internal trace ids for no benefit.

For a downstream call, `context.outbound_headers()` returns a `HeaderMap` with
a `traceparent` whose span id is a fresh child — which is what makes the next
hop a child of this one rather than a sibling of the caller — plus the
`tracestate` if one survived validation.

Root ids come from `getrandom` when the `oauth` feature has pulled it in, and
otherwise from a SplitMix64 mix of the clock, a per-process counter and a
stack address. Trace ids need to be unique, not unpredictable — they carry no
authority and grant no access — so the fallback is adequate, and this module
refuses to require a crypto dependency for a correlation identifier.

## OTLP export

With `otel`, `Telemetry` is an OTLP-over-gRPC span pipeline the application
holds:

```rust,ignore
use std::time::Duration;
use arcature::observe::{JsonLog, StderrSink, Telemetry};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

let telemetry = Telemetry::builder("checkout")
    .endpoint("http://collector:4317")
    .timeout(Duration::from_secs(5))
    .build()?;

tracing_subscriber::registry()
    .with(JsonLog::new(StderrSink))
    .with(telemetry.tracing_layer())
    .init();

// ... run the application ...

telemetry.shutdown()?;
```

| Point | Behaviour |
| --- | --- |
| Service name | required by `Telemetry::builder`, never defaulted |
| Transport | gRPC over tonic; the `grpc-tonic` feature is the only one enabled |
| Signal | traces only — `opentelemetry` and the SDK are pinned with `trace` and nothing else |
| Resource attributes | `service.name`, and that is all this crate sets |
| Runtime | a Tokio runtime must already be running when `build()` is called |
| Shutdown | `Telemetry::shutdown` flushes; dropping the value without it loses whatever is buffered |
| Sampling | none configured here, and nothing in this crate consults the sampled flag |

A deployment reporting as `unknown_service` is a deployment nobody can find,
so the name is a parameter rather than a default, and
`tests/observe_otlp.rs` asserts it actually travels as the `service.name`
resource attribute.

`endpoint` is worth reading carefully. When it is not called, `build()` does
not call `with_endpoint` at all, so `opentelemetry-otlp`'s own default applies
— which is the layer that reads `OTEL_EXPORTER_OTLP_ENDPOINT`.
`observe::otel::DEFAULT_ENDPOINT` is published as `http://127.0.0.1:4317`,
the address a local collector listens on out of the box, but nothing in
`build()` passes it: it is documentation of the address, not a value this
builder sends.

The pipeline must be built inside the async entry point and not in a `static`
initialiser, because the batch processor spawns a background task.
`shutdown` is fallible because a collector that is already gone cannot accept
the final batch, and that is worth reporting rather than swallowing. A
collector that never answers does not panic the application — there is a test
that binds a port, drops the listener, and exports at the dead address.

`tests/observe_otlp.rs` runs a real `TraceService` gRPC server on a loopback
port and decodes the protobuf the exporter wrote, so the assertions are
against bytes that crossed a socket: a span arrives, a parent and child share
one trace id, a child names its parent's span id, a three-level nesting
arrives as a chain rather than a fan, and a span field arrives as an
attribute.

### The two id spaces do not meet

`TraceContextLayer` and the OTLP exporter both deal in trace ids, and they are
not the same ids.

Nothing in this crate calls `set_parent`, registers a propagator, or hands the
parsed `traceparent` to the OpenTelemetry context. The ids the exporter puts
on a span are the SDK's own. The inbound W3C ids reach the collector only as
the string attributes `trace_id` and `parent_span_id` on the
`arcature.request` span, because `tracing` fields become OTLP attributes.

So: `TraceContextLayer` gives you correlation *in your logs* against the ids
your upstream used. It does not join your exported spans to an upstream
trace. An application that needs a remote parent honoured in the exported
trace wires `tracing_opentelemetry`'s span extension itself.

## Redaction

Logging is the most common way a secret escapes a process, because a log line
is written once and then copied everywhere — to a file, to a shipper, to a
third-party index, into a support ticket. So the defence is a property of the
writer, not a rule the caller remembers: `JsonLog` asks
`redact::is_sensitive` about every field name it is about to serialise, and a
field that matches is written as `REDACTED`, the fixed marker `"[redacted]"`.
A marker rather than an omission, so a reader can tell "this field was
withheld" from "this field was never recorded".

Two mechanisms are at work and they are not the same size. The first is that
the framework's own layers record structured fields and never format a secret
into a message string: the access log records method, path, status and
duration, and the metric labels are a method, a status and a `&'static str`.
That mechanism holds everywhere. The second is the deny-list, which covers a
field an application adds — and it is much narrower than it first looks.

### Which sinks it reaches

The deny-list is consulted in exactly two files: `json_log.rs`, which asks
`is_sensitive` about every field of every event and every span, and
`access_log.rs`, which calls `redact::apply` on the one `client_ip` value.
`metrics.rs`, `otel.rs` and `trace_context.rs` never call either.

| Channel | Redacted | Note |
| --- | --- | --- |
| JSON log, event field | yes | the name is matched, the value is dropped |
| JSON log, span field | yes | redacted when the span is created, so the folded copy is already safe |
| JSON log, message string | **no** | the deny-list catches the field, not the sentence |
| Debug-build console (`fmt`) | **no** | `install_logging` only installs `JsonLog` in release builds |
| Metric label value | **no** | escaped for the exposition format, not redacted |
| Exported OTLP span attribute | **no** | `tracing_opentelemetry` has its own visitor |

The last two are pinned by tests that assert the leak, named so nobody
mistakes them for tests of a working defence:

- `a_secret_recorded_as_a_span_field_reaches_the_collector_in_full` — the
  same field the JSON layer wrote as `[redacted]` leaves the process in
  plaintext, over the wire, to a collector.
- `a_secret_recorded_as_a_metric_label_is_rendered_in_full` — a series
  labelled with a session id publishes it on `/metrics`.

Both assert today's behaviour so that closing either gap is a visible change
rather than a silent one. If either starts failing because the value is now
redacted, the defence has been extended and the test is to be deleted.

Until then: treat a metric label and a span attribute as unredacted channels.
Keep the value out of them, or record it as a type whose own `Debug` renders
redacted — the JSON visitor's fallback formats through `Debug`, so a secret
newtype protects itself in every channel that formats one. A label value is
also a series dimension, so a secret used as one is usually an
unbounded-cardinality bug as well.

### How a name is matched

```rust,ignore
pub fn is_sensitive(field: &str) -> bool
```

Three steps, in this order:

1. Every `-` and every `.` becomes `_`.
2. Every other character is ASCII-lowercased.
3. The result is tested for **containment** of any needle in `DENY_LIST`.

Substring rather than exact name, on purpose: `password`, `user_password` and
`db.password` are all the same mistake, and a deny-list that only catches the
spelling someone thought of is not a deny-list. False positives cost a
debugging session; false negatives cost a credential.

Separator folding, also on purpose: an HTTP header is `x-api-key`, an
OpenTelemetry attribute is `http.request.header.authorization`, a struct field
is `api_key`. One secret, three spellings, and a needle written with `_` is a
substring of only the third. Every needle uses `_` and none contains `-` or
`.`, so folding can only ever match more.

`DENY_LIST` is public, sorted and lowercase — a unit test asserts the last
two — so an application can check its own field names against it:

| | | | |
| --- | --- | --- | --- |
| `access_token` | `api_key` | `apikey` | `auth` |
| `bearer` | `bind` | `body` | `cache_value` |
| `card` | `cookie` | `credential` | `csrf` |
| `cvv` | `id_token` | `otp` | `passphrase` |
| `passwd` | `password` | `payload` | `pin_code` |
| `private_key` | `pwd` | `refresh_token` | `secret` |
| `session_id` | `signature` | `sql_args` | `token` |
| `verifier` | | | |

Because the test is containment, ordinary names collide with short needles and
are redacted:

| Field name | Redacted by |
| --- | --- |
| `author`, `authority` | `auth` |
| `wildcard`, `discard`, `cardinality` | `card` |
| `binding` | `bind` |
| `body_bytes` | `body` |
| `token_count` | `token` |

That is the trade the module chose, stated so it is not a surprise at three in
the morning when a field reads `[redacted]` and nothing is wrong.

What is **not** folded: any separator other than `-` and `.`. A space, a
slash, a colon and a camelCase word boundary all survive step 1, so a
multi-word needle cannot match across them. Non-ASCII case is not folded
either — a field name is not expected to contain non-ASCII, and folding
Unicode would widen the surface without widening the protection.

`redact::apply(field, value)` is the borrowing form: it returns `REDACTED` or
the value unchanged, so nothing is copied for a field that is allowed through.
`is_sensitive` and `REDACTED` are re-exported at `arcature::observe`;
`DENY_LIST` and `apply` live at `arcature::observe::redact`.

### The disclosed gap: camelCase multi-word names

`tests/observe_redaction.rs` pins it by name:

```text
the_deny_list_is_written_in_snake_case_and_therefore_misses_camel_case_spellings
```

The list is written in snake_case, and camelCase has no separator to fold.
`privateKey` lowercases to `privatekey`; the needle is `private_key`; there is
no match. The test walks the spellings side by side:

| Spelling | Result | Why |
| --- | --- | --- |
| `x-api-key`, `X-Api-Key` | redacted | `-` folds to `_`, matching `api_key` |
| `http.request.header.authorization` | redacted | `.` folds, and `auth` matches anyway |
| `apiKey` | redacted | `apikey` is on the list as its own needle |
| `accessToken` | redacted | the single-word needle `token` matches |
| `privateKey` | **not redacted** | `privatekey` does not contain `private_key` |
| `sessionId` | **not redacted** | `sessionid` does not contain `session_id` |

The exposure is narrower than the headline. Only multi-word needles are
reachable this way, single-word needles catch most camelCase spellings
anyway, and Rust field names are snake_case — so what is left is an
application that records a JSON body's keys under the names the client chose.
If that is your application, check the names against `DENY_LIST` in a test of
your own, or normalise them before recording.

As with the leak tests: if `privateKey` starts coming out redacted, the
matcher has been widened and that test should be narrowed to whatever spelling
is still missed, or deleted.

### A list of names cannot see values

The other limit is structural, and it also has a test:
`a_secret_under_a_field_name_nobody_denied_is_logged_in_full`. A field called
`note` carries a password in one handler and a postcode in the next. No list
of names can catch that.

Nor can any formatter undo a secret a caller has already interpolated into a
message string. `tracing::info!("signing in {password}")` is past every
checkpoint there is by the time the layer sees it. Record fields, not
sentences.

The defence for both is the same one the framework applies to itself: its own
layers never record a field whose contents they have not chosen. The list of
what that means in practice, from the module's own documentation —

- **Request and response bodies.** Method, path, status and duration; never
  the payload.
- **SQL bind values.** A query's text may be recorded; the parameters bound
  into it may not, because that is where the row data lives.
- **Cache values.** Keys are loggable and are logged; values are not.
- **Credentials of every kind** — passwords, password hashes, API keys,
  bearer tokens, OAuth access and refresh tokens, PKCE verifiers, CSRF state,
  session identifiers, cookies, and `Authorization` headers.
- **Email bodies and recipients' message content.** A send is recorded as an
  event with a message id; the letter is not.
- **Job payloads.** A job is recorded by name, queue and attempt count.

— holds for the framework's own layers in every channel.
`tests/observe_redaction.rs` drives one request carrying a password, a bearer
token, a session cookie and a PKCE verifier through the whole stack — request
ids, access logging, metrics, trace context — with a log sink, a metrics
registry and a live OTLP collector capturing at once, then searches the log
transcript, the metrics exposition, the exported span attributes and the
exported resource attributes for each secret's value. It also asserts that
each channel captured something, because a harness that silently captured
nothing would pass every absence assertion in the file.

## Stable span names

Seven `&'static str` constants at `arcature::observe`, with
`is_stable(name)` and `ALL` to iterate them:

| Constant | Value | Opened by the framework |
| --- | --- | --- |
| `REQUEST` | `arcature.request` | yes — `AccessLogService` and `TraceContextService` |
| `DB_QUERY` | `arcature.db.query` | no |
| `CACHE_GET` | `arcature.cache.get` | no |
| `JOB_HANDLE` | `arcature.job.handle` | no |
| `PAGE_RENDER` | `arcature.page.render` | no |
| `EVENT_LISTENER` | `arcature.event.listener` | no |
| `SCHEDULE_TICK` | `arcature.schedule.tick` | no |

Six of the seven are reserved names rather than spans anything currently
emits: no code in the crate opens them today. They are here so that
instrumentation added later, in the framework or in an application, agrees on
one spelling instead of inventing a second. `arcature.request` is opened
twice when both layers are installed — once by each — so a `spans` array can
carry the name more than once.

## What this module does not do

**Install anything on its own.** No global subscriber, no global recorder, no
global tracer provider. `install_logging` is a call the binary makes.

**Add a `/metrics` route, or protect one.** The registry and the layer are
values; routing and access control are the application's.

**Wire metrics or trace context into the pipeline.** Only `RequestIdLayer` and
`AccessLogLayer` have builder methods. `MetricsLayer` and `TraceContextLayer`
are installed by hand, and a user `.layer()` sits inside the request-id and
access-log stages.

**Redact in debug builds.** The `fmt` layer prints fields verbatim.

**Redact metric labels or OTLP span attributes.** See the table above.

**Sample.** `TelemetryBuilder` sets no sampler and reads no sampling
environment variable, so whatever the SDK's own default does is what happens.
`TraceParent` carries and preserves the sampled flag, but nothing in this
crate consults it to decide whether to record or export.

**Export metrics or logs over OTLP.** Traces only. `opentelemetry` and the SDK
are pinned with the `trace` feature and nothing else; metrics leave as
Prometheus text or not at all.

**Join an upstream W3C trace to the exported spans.** No propagator is
registered and `set_parent` is never called.

**Write to a file, rotate, or ship.** Both formats go to standard error. The
process manager owns the file, and every mainstream one already does this
better than a library could.

**Throttle or deduplicate log lines.** A chatty target is what the filter is
for.

**Provide error tracking.** No Sentry, no crash reporter, no aggregation. An
`ERROR` line with structured fields is what the module produces; turning that
into an incident is a shipper's job.
