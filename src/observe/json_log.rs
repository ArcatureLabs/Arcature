//! One JSON object per log line, with the deny-list applied on the way out.
//!
//! `tracing-subscriber` is pinned here with only its `registry` feature, so
//! neither `fmt` nor `json` is available and the formatter is written by
//! hand. That turns out to be the right shape anyway: the layer owns the
//! serialisation, so [`redact`](super::redact) runs on every field with no
//! way for a caller to opt out of it.
//!
//! Output is newline-delimited JSON -- one object per event, no pretty
//! printing, no trailing state. Every mainstream log shipper reads that
//! format without configuration.
//!
//! # What is never written
//!
//! A field whose name matches [`redact::DENY_LIST`](super::redact::DENY_LIST)
//! is written as `"[redacted]"`. Concretely that covers credentials, tokens,
//! cookies, request bodies, SQL bind values, cache values, email bodies and
//! job payloads -- provided they are recorded as *fields*. A value that a
//! caller has already interpolated into the message string cannot be
//! recovered by any formatter, which is why the framework's own layers
//! record structured fields and never format a secret into a message.

use std::fmt;
use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use super::redact;

/// Where a formatted log line goes.
///
/// A trait rather than `std::io::Write` because a sink is shared across
/// threads and must serialise whole lines: a `Write` that interleaved two
/// events would produce unparseable output. Implementors are responsible for
/// writing the line and its terminator atomically enough that a reader never
/// sees half of one.
pub trait LogSink: Send + Sync + 'static {
    /// Write one complete log line. The line does not carry a terminator.
    fn write_line(&self, line: &str);
}

/// A sink that writes to standard error.
///
/// Standard error rather than standard output because output is where a CLI
/// puts its results, and a log line interleaved into those is a bug.
#[derive(Debug, Clone, Copy, Default)]
pub struct StderrSink;

impl LogSink for StderrSink {
    fn write_line(&self, line: &str) {
        let mut out = std::io::stderr().lock();
        let _ = writeln!(out, "{line}");
    }
}

/// A sink that keeps lines in memory, for tests.
///
/// Cloning shares the buffer, so a test can hand one clone to the layer and
/// keep the other to assert on.
#[derive(Debug, Clone, Default)]
pub struct CaptureSink {
    lines: Arc<Mutex<Vec<String>>>,
}

impl CaptureSink {
    /// A new, empty capture buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every line written so far.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        match self.lines.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Every line written so far, joined -- convenient for a `contains`
    /// assertion over the whole transcript.
    #[must_use]
    pub fn transcript(&self) -> String {
        self.lines().join("\n")
    }
}

impl LogSink for CaptureSink {
    fn write_line(&self, line: &str) {
        let mut guard = match self.lines.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.push(line.to_string());
    }
}

/// The visitor that turns `tracing` fields into JSON, redacting as it goes.
///
/// `record_debug` is the fallback the macros use for anything without a more
/// specific method, and it formats through `Debug` -- which for a secret
/// newtype is exactly the redacted form the type chose. The deny-list runs
/// first regardless, so a plain `&str` password is caught too.
#[derive(Debug, Default)]
struct JsonVisitor {
    fields: Map<String, Value>,
}

impl JsonVisitor {
    fn insert(&mut self, field: &Field, value: Value) {
        let name = field.name();
        if redact::is_sensitive(name) {
            self.fields
                .insert(name.to_string(), Value::from(redact::REDACTED));
        } else {
            self.fields.insert(name.to_string(), value);
        }
    }
}

impl Visit for JsonVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, Value::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, Value::from(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, Value::from(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert(field, Value::from(value));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.insert(field, Value::from(value.to_string()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.insert(field, Value::from(format!("{value:?}")));
    }
}

/// The fields recorded on a span, kept in the registry's extensions so an
/// event nested inside the span can inherit them.
#[derive(Debug, Default)]
struct SpanFields(Map<String, Value>);

/// A `tracing` layer that writes newline-delimited JSON to a [`LogSink`].
///
/// Install it on a `tracing_subscriber::Registry`. The layer holds its sink;
/// nothing is registered globally, so a test can build a subscriber with a
/// [`CaptureSink`] and set it for the duration of one closure.
///
/// ```
/// use arcature::observe::{CaptureSink, JsonLog};
/// use tracing_subscriber::layer::SubscriberExt as _;
///
/// let sink = CaptureSink::new();
/// let subscriber = tracing_subscriber::registry().with(JsonLog::new(sink.clone()));
/// tracing::subscriber::with_default(subscriber, || {
///     tracing::info!(user = "ada", password = "hunter2", "signed in");
/// });
/// let line = sink.transcript();
/// assert!(line.contains("\"user\":\"ada\""));
/// assert!(!line.contains("hunter2"));
/// ```
#[derive(Debug, Clone)]
pub struct JsonLog<W> {
    sink: W,
    include_spans: bool,
}

impl<W: LogSink> JsonLog<W> {
    /// A layer writing to `sink`, with span fields folded into each event.
    pub fn new(sink: W) -> Self {
        Self {
            sink,
            include_spans: true,
        }
    }

    /// Drop the inherited span fields and log only what the event itself
    /// recorded. Smaller lines, at the cost of losing the request id that
    /// the enclosing span carries.
    #[must_use]
    pub fn without_span_fields(mut self) -> Self {
        self.include_spans = false;
        self
    }
}

impl Default for JsonLog<StderrSink> {
    fn default() -> Self {
        Self::new(StderrSink)
    }
}

impl<S, W> tracing_subscriber::Layer<S> for JsonLog<W>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: LogSink,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        let Some(span) = ctx.span(id) else { return };
        let mut visitor = JsonVisitor::default();
        attrs.record(&mut visitor);
        span.extensions_mut().insert(SpanFields(visitor.fields));
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: Context<'_, S>,
    ) {
        let Some(span) = ctx.span(id) else { return };
        let mut visitor = JsonVisitor::default();
        values.record(&mut visitor);
        let mut extensions = span.extensions_mut();
        if let Some(existing) = extensions.get_mut::<SpanFields>() {
            existing.0.extend(visitor.fields);
        } else {
            extensions.insert(SpanFields(visitor.fields));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut visitor = JsonVisitor::default();
        event.record(&mut visitor);

        let metadata = event.metadata();
        let mut line = Map::new();
        line.insert(
            "timestamp".into(),
            Value::from(rfc3339_utc(SystemTime::now())),
        );
        line.insert("level".into(), Value::from(metadata.level().as_str()));
        line.insert("target".into(), Value::from(metadata.target()));

        // `message` is just another field to `tracing`; lifting it to a
        // top-level key is what makes the line readable to a human and
        // indexable by everything else.
        if let Some(message) = visitor.fields.remove("message") {
            line.insert("message".into(), message);
        }

        if self.include_spans
            && let Some(scope) = ctx.event_scope(event)
        {
            let mut names = Vec::new();
            let mut inherited = Map::new();
            // Outermost first, so a nested span's field wins over its
            // parent's when both record the same name.
            for span in scope.from_root() {
                names.push(Value::from(span.name()));
                if let Some(fields) = span.extensions().get::<SpanFields>() {
                    inherited.extend(fields.0.clone());
                }
            }
            if !names.is_empty() {
                line.insert("spans".into(), Value::Array(names));
            }
            for (key, value) in inherited {
                visitor.fields.entry(key).or_insert(value);
            }
        }

        if !visitor.fields.is_empty() {
            line.insert("fields".into(), Value::Object(visitor.fields));
        }

        self.sink.write_line(&Value::Object(line).to_string());
    }
}

/// An RFC 3339 UTC timestamp with millisecond precision.
///
/// Written by hand because `chrono` belongs to the `database` feature and a
/// logging layer must not drag a database dependency behind it.
fn rfc3339_utc(now: SystemTime) -> String {
    let since_epoch = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since_epoch.as_secs();
    let millis = since_epoch.subsec_millis();
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let time_of_day = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60,
    )
}

/// Days since the Unix epoch to a proleptic Gregorian date.
///
/// Howard Hinnant's `civil_from_days`, which is the standard branch-free
/// formulation and the same one `chrono` and `time` are built on.
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    // Shift the epoch to 0000-03-01 so a leap day lands at the end of the
    // 400-year era and the month arithmetic below stays linear.
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt as _;

    /// Run `body` with a capturing JSON subscriber and return what it wrote.
    fn capture(body: impl FnOnce()) -> Vec<String> {
        let sink = CaptureSink::new();
        let subscriber = tracing_subscriber::registry().with(JsonLog::new(sink.clone()));
        tracing::subscriber::with_default(subscriber, body);
        sink.lines()
    }

    #[test]
    fn a_password_field_never_appears_in_a_formatted_line() {
        let lines = capture(|| {
            tracing::info!(user = "ada", password = "hunter2", "signed in");
        });
        let line = lines.join("\n");
        assert!(!line.contains("hunter2"), "leaked the password: {line}");
        assert!(line.contains("[redacted]"));
        assert!(line.contains("\"user\":\"ada\""));
    }

    #[test]
    fn a_password_carried_on_a_span_is_redacted_too() {
        let lines = capture(|| {
            let span = tracing::info_span!("login", password = "hunter2", user = "ada");
            let _entered = span.enter();
            tracing::info!("inside");
        });
        let line = lines.join("\n");
        assert!(!line.contains("hunter2"), "leaked through the span: {line}");
        assert!(line.contains("\"user\":\"ada\""));
    }

    #[test]
    fn every_deny_listed_category_is_withheld() {
        let lines = capture(|| {
            tracing::info!(
                request_body = "{\"card\":\"4111111111111111\"}",
                sql_args = "['ada@example.test']",
                cache_value = "session-blob",
                authorization = "Bearer abc123",
                email_body = "Dear Ada,",
                job_payload = "{\"user\":1}",
                "handled"
            );
        });
        let line = lines.join("\n");
        for secret in [
            "4111111111111111",
            "ada@example.test",
            "session-blob",
            "abc123",
            "Dear Ada",
            "{\\\"user\\\":1}",
        ] {
            assert!(!line.contains(secret), "leaked {secret} in {line}");
        }
    }

    #[test]
    fn a_line_is_one_json_object_with_the_expected_shape() {
        let lines = capture(|| tracing::warn!(status = 503, "upstream down"));
        assert_eq!(lines.len(), 1);
        let value: Value = serde_json::from_str(&lines[0]).expect("valid JSON");
        assert_eq!(value["level"], "WARN");
        assert_eq!(value["message"], "upstream down");
        assert_eq!(value["fields"]["status"], 503);
        assert!(
            value["timestamp"]
                .as_str()
                .is_some_and(|t| t.ends_with('Z'))
        );
    }

    #[test]
    fn the_timestamp_matches_known_epoch_instants() {
        let at = |secs| rfc3339_utc(UNIX_EPOCH + std::time::Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00.000Z");
        assert_eq!(at(1_700_000_000), "2023-11-14T22:13:20.000Z");
    }
}
