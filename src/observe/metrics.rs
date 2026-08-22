//! A Prometheus text endpoint, and the handle that feeds it.
//!
//! [`Metrics`] is a value the application holds and clones -- there is no
//! global recorder, no `lazy_static` registry, and no macro that reaches for
//! one. Two `Metrics` values are two independent registries, which is what
//! makes a test able to assert on exactly its own counters.
//!
//! The exposition format is the Prometheus text format, version 0.0.4: the
//! one every scraper reads, including the OpenMetrics parsers, which accept
//! it as a subset.
//!
//! Nothing here records a label whose value comes from user input. Path
//! templates and method names are bounded sets; a raw URI path is not, and
//! using one as a label is how a metrics endpoint turns into a memory leak.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};

/// The content type a Prometheus scraper expects.
pub const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// The default histogram bucket bounds, in seconds.
///
/// The set Prometheus client libraries ship by default: it brackets the
/// latency range a web request actually lives in, from a cached 5 ms answer
/// to a 10 s timeout.
pub const DEFAULT_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// A metric name plus its label set, in canonical order.
///
/// `BTreeMap` for the labels rather than a `Vec` so two recordings that name
/// the same labels in a different order land on the same series.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Series {
    name: String,
    labels: BTreeMap<String, String>,
}

impl Series {
    fn new(name: &str, labels: &[(&str, &str)]) -> Self {
        Self {
            name: name.to_string(),
            labels: labels
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    /// Render `name{label="value",...}` with the label values escaped.
    fn render(&self, suffix: &str, extra: Option<(&str, &str)>, out: &mut String) {
        out.push_str(&self.name);
        out.push_str(suffix);
        if self.labels.is_empty() && extra.is_none() {
            return;
        }
        out.push('{');
        let mut first = true;
        for (key, value) in self
            .labels
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .chain(extra)
        {
            if !first {
                out.push(',');
            }
            first = false;
            let _ = write!(out, "{key}=\"{}\"", escape_label(value));
        }
        out.push('}');
    }
}

/// Escape a label value for the text format: backslash, quote, newline.
fn escape_label(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// What kind of metric a name refers to, for the `# TYPE` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Counter,
    Gauge,
    Histogram,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
            Self::Histogram => "histogram",
        }
    }
}

/// The accumulated observations for one histogram series.
#[derive(Debug, Clone)]
struct Histogram {
    bounds: Vec<f64>,
    counts: Vec<u64>,
    sum: f64,
    count: u64,
}

impl Histogram {
    fn new(bounds: Vec<f64>) -> Self {
        let counts = vec![0; bounds.len()];
        Self {
            bounds,
            counts,
            sum: 0.0,
            count: 0,
        }
    }

    fn record(&mut self, value: f64) {
        for (index, bound) in self.bounds.iter().enumerate() {
            if value <= *bound {
                self.counts[index] += 1;
            }
        }
        self.sum += value;
        self.count += 1;
    }
}

/// The shared state behind a [`Metrics`] handle.
#[derive(Debug, Default)]
struct Registry {
    counters: BTreeMap<Series, u64>,
    gauges: BTreeMap<Series, f64>,
    histograms: BTreeMap<Series, Histogram>,
    /// Name to (kind, help). Metadata is per name, not per series, because
    /// that is how the text format defines it.
    described: BTreeMap<String, (Kind, String)>,
    buckets: Vec<f64>,
}

/// A metrics registry the application holds.
///
/// Cloning shares the registry, so the copy handed to a middleware and the
/// copy handed to the `/metrics` route are the same set of counters.
///
/// Label values are escaped for the exposition format and are **not**
/// redacted: the deny-list in [`redact`](super::redact) is consulted by the
/// JSON log layer and by nothing here. Nothing [`MetricsLayer`] writes can
/// carry a secret, since its label values are a method, a status and a
/// `&'static str` route, but a caller choosing its own labels is choosing
/// what a scrape endpoint publishes. A label value is also a series
/// dimension, so a secret used as one is usually an unbounded-cardinality
/// bug as well.
///
/// ```
/// use arcature::observe::Metrics;
///
/// let metrics = Metrics::new();
/// metrics.describe_counter("jobs_processed_total", "Jobs run to completion.");
/// metrics.increment("jobs_processed_total", &[("queue", "default")], 1);
/// assert!(metrics.render().contains("jobs_processed_total{queue=\"default\"} 1"));
/// ```
#[derive(Debug, Clone)]
pub struct Metrics {
    registry: Arc<Mutex<Registry>>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    /// An empty registry with the default histogram buckets.
    #[must_use]
    pub fn new() -> Self {
        Self::with_buckets(DEFAULT_BUCKETS)
    }

    /// An empty registry whose histograms use `bounds`.
    ///
    /// Bounds are sorted on the way in; an unsorted bucket list produces a
    /// non-monotonic cumulative count, which a scraper rejects.
    #[must_use]
    pub fn with_buckets(bounds: &[f64]) -> Self {
        let mut buckets: Vec<f64> = bounds.iter().copied().filter(|b| b.is_finite()).collect();
        buckets.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        Self {
            registry: Arc::new(Mutex::new(Registry {
                buckets,
                ..Registry::default()
            })),
        }
    }

    /// Take the registry lock, recovering from a poisoned mutex.
    ///
    /// A panic in one recording must not silence every future metric: the
    /// data behind the lock is plain counters with no invariant a partial
    /// write could break.
    fn lock(&self) -> std::sync::MutexGuard<'_, Registry> {
        match self.registry.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Attach help text to a counter name.
    pub fn describe_counter(&self, name: &str, help: &str) {
        self.describe(name, Kind::Counter, help);
    }

    /// Attach help text to a gauge name.
    pub fn describe_gauge(&self, name: &str, help: &str) {
        self.describe(name, Kind::Gauge, help);
    }

    /// Attach help text to a histogram name.
    pub fn describe_histogram(&self, name: &str, help: &str) {
        self.describe(name, Kind::Histogram, help);
    }

    fn describe(&self, name: &str, kind: Kind, help: &str) {
        self.lock()
            .described
            .insert(name.to_string(), (kind, help.to_string()));
    }

    /// Add `by` to a counter series.
    pub fn increment(&self, name: &str, labels: &[(&str, &str)], by: u64) {
        let series = Series::new(name, labels);
        let mut registry = self.lock();
        registry
            .described
            .entry(name.to_string())
            .or_insert((Kind::Counter, String::new()));
        *registry.counters.entry(series).or_insert(0) += by;
    }

    /// Set a gauge series to `value`.
    pub fn set(&self, name: &str, labels: &[(&str, &str)], value: f64) {
        let series = Series::new(name, labels);
        let mut registry = self.lock();
        registry
            .described
            .entry(name.to_string())
            .or_insert((Kind::Gauge, String::new()));
        registry.gauges.insert(series, value);
    }

    /// Record one observation into a histogram series.
    pub fn observe(&self, name: &str, labels: &[(&str, &str)], value: f64) {
        let series = Series::new(name, labels);
        let mut registry = self.lock();
        registry
            .described
            .entry(name.to_string())
            .or_insert((Kind::Histogram, String::new()));
        let bounds = registry.buckets.clone();
        registry
            .histograms
            .entry(series)
            .or_insert_with(|| Histogram::new(bounds))
            .record(value);
    }

    /// The current value of a counter series, if it has ever been touched.
    #[must_use]
    pub fn counter_value(&self, name: &str, labels: &[(&str, &str)]) -> Option<u64> {
        self.lock()
            .counters
            .get(&Series::new(name, labels))
            .copied()
    }
}

impl Metrics {
    /// Render the whole registry in the Prometheus text format.
    ///
    /// Series are emitted grouped by name, in name order, because the text
    /// format requires every sample of a name to sit under its one `# TYPE`
    /// line -- interleaving names is a parse error, not a style choice.
    #[must_use]
    pub fn render(&self) -> String {
        let registry = self.lock();
        let mut out = String::new();
        for (name, (kind, help)) in &registry.described {
            if !help.is_empty() {
                let _ = writeln!(out, "# HELP {name} {}", help.replace('\n', " "));
            }
            let _ = writeln!(out, "# TYPE {name} {}", kind.as_str());
            match kind {
                Kind::Counter => {
                    for (series, value) in registry.counters.iter().filter(|(s, _)| &s.name == name)
                    {
                        series.render("", None, &mut out);
                        let _ = writeln!(out, " {value}");
                    }
                }
                Kind::Gauge => {
                    for (series, value) in registry.gauges.iter().filter(|(s, _)| &s.name == name) {
                        series.render("", None, &mut out);
                        let _ = writeln!(out, " {value}");
                    }
                }
                Kind::Histogram => {
                    for (series, histogram) in
                        registry.histograms.iter().filter(|(s, _)| &s.name == name)
                    {
                        for (bound, count) in histogram.bounds.iter().zip(&histogram.counts) {
                            series.render("_bucket", Some(("le", &format_float(*bound))), &mut out);
                            let _ = writeln!(out, " {count}");
                        }
                        series.render("_bucket", Some(("le", "+Inf")), &mut out);
                        let _ = writeln!(out, " {}", histogram.count);
                        series.render("_sum", None, &mut out);
                        let _ = writeln!(out, " {}", format_float(histogram.sum));
                        series.render("_count", None, &mut out);
                        let _ = writeln!(out, " {}", histogram.count);
                    }
                }
            }
        }
        out
    }

    /// The registry as an HTTP response a `/metrics` route can return.
    ///
    /// Wire it as an ordinary route so it inherits whatever the application
    /// puts in front of it -- a metrics endpoint usually wants an allow-list
    /// or basic auth, and that belongs to the application, not here.
    ///
    /// ```no_run
    /// # use arcature::observe::Metrics;
    /// # let metrics = Metrics::new();
    /// let router: axum::Router = axum::Router::new().route(
    ///     "/metrics",
    ///     axum::routing::get(move || {
    ///         let metrics = metrics.clone();
    ///         async move { metrics.response() }
    ///     }),
    /// );
    /// ```
    #[must_use]
    pub fn response(&self) -> Response {
        (
            [(
                CONTENT_TYPE,
                HeaderValue::from_static(PROMETHEUS_CONTENT_TYPE),
            )],
            self.render(),
        )
            .into_response()
    }
}

/// Format a float the way the text format wants it: no trailing `.0` noise
/// on a whole number, and the literal `+Inf` handled by the caller.
fn format_float(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

// ---------------------------------------------------------------------------
// The HTTP recording layer
// ---------------------------------------------------------------------------

/// The counter of handled requests.
pub const HTTP_REQUESTS_TOTAL: &str = "http_requests_total";
/// The request-duration histogram, in seconds.
pub const HTTP_REQUEST_DURATION: &str = "http_request_duration_seconds";

/// A Tower layer that records request counts and durations into [`Metrics`].
///
/// Series are labelled by method and status only. There is deliberately no
/// path label taken from the request URI: a URI is unbounded input, and an
/// unbounded label value is how a scrape target runs out of memory. An
/// application that wants a route dimension installs the layer per route or
/// per group with [`MetricsLayer::labelled`] and names the template itself,
/// which keeps the set of values finite and reviewable.
#[derive(Debug, Clone)]
pub struct MetricsLayer {
    metrics: Metrics,
    route: Option<&'static str>,
}

impl MetricsLayer {
    /// Record into `metrics`, with no route label.
    #[must_use]
    pub fn new(metrics: Metrics) -> Self {
        metrics.describe_counter(HTTP_REQUESTS_TOTAL, "Total HTTP requests handled.");
        metrics.describe_histogram(HTTP_REQUEST_DURATION, "HTTP request duration in seconds.");
        Self {
            metrics,
            route: None,
        }
    }

    /// Record into `metrics`, tagging every series with `route`.
    ///
    /// `route` is a `&'static str` rather than a `String` so it cannot be a
    /// value derived from a request.
    #[must_use]
    pub fn labelled(metrics: Metrics, route: &'static str) -> Self {
        Self {
            route: Some(route),
            ..Self::new(metrics)
        }
    }
}

impl<S> tower::Layer<S> for MetricsLayer {
    type Service = MetricsService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        MetricsService {
            inner,
            metrics: self.metrics.clone(),
            route: self.route,
        }
    }
}

/// The service produced by [`MetricsLayer`].
#[derive(Debug, Clone)]
pub struct MetricsService<S> {
    inner: S,
    metrics: Metrics,
    route: Option<&'static str>,
}

impl<S> tower::Service<axum::extract::Request> for MetricsService<S>
where
    S: tower::Service<
            axum::extract::Request,
            Response = Response,
            Error = std::convert::Infallible,
        > + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = std::convert::Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: axum::extract::Request) -> Self::Future {
        let method = request.method().as_str().to_string();
        let route = self.route;
        let metrics = self.metrics.clone();
        let started = std::time::Instant::now();

        // Swap in the clone and drive the original: only the original is
        // known ready, and `poll_ready` readiness does not survive cloning.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move {
            let response = inner.call(request).await?;
            let status = response.status().as_u16().to_string();
            let mut counted: Vec<(&str, &str)> = vec![("method", &method), ("status", &status)];
            let mut timed: Vec<(&str, &str)> = vec![("method", &method)];
            if let Some(route) = route {
                counted.push(("route", route));
                timed.push(("route", route));
            }
            metrics.increment(HTTP_REQUESTS_TOTAL, &counted, 1);
            metrics.observe(
                HTTP_REQUEST_DURATION,
                &timed,
                started.elapsed().as_secs_f64(),
            );
            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_counter_accumulates_per_label_set() {
        let metrics = Metrics::new();
        metrics.increment("hits_total", &[("route", "/a")], 1);
        metrics.increment("hits_total", &[("route", "/a")], 2);
        metrics.increment("hits_total", &[("route", "/b")], 5);
        assert_eq!(
            metrics.counter_value("hits_total", &[("route", "/a")]),
            Some(3)
        );
        assert_eq!(
            metrics.counter_value("hits_total", &[("route", "/b")]),
            Some(5)
        );
        assert_eq!(
            metrics.counter_value("hits_total", &[("route", "/c")]),
            None
        );
    }

    #[test]
    fn label_order_does_not_split_a_series() {
        let metrics = Metrics::new();
        metrics.increment("hits_total", &[("a", "1"), ("b", "2")], 1);
        metrics.increment("hits_total", &[("b", "2"), ("a", "1")], 1);
        assert_eq!(
            metrics.counter_value("hits_total", &[("a", "1"), ("b", "2")]),
            Some(2)
        );
    }

    #[test]
    fn the_rendered_text_carries_type_and_help_once_per_name() {
        let metrics = Metrics::new();
        metrics.describe_counter("hits_total", "How many.");
        metrics.increment("hits_total", &[("route", "/a")], 1);
        metrics.increment("hits_total", &[("route", "/b")], 1);
        let text = metrics.render();
        assert_eq!(text.matches("# TYPE hits_total counter").count(), 1);
        assert!(text.contains("# HELP hits_total How many."));
        assert!(text.contains("hits_total{route=\"/a\"} 1"));
        assert!(text.contains("hits_total{route=\"/b\"} 1"));
    }

    #[test]
    fn a_histogram_renders_cumulative_buckets_a_sum_and_a_count() {
        let metrics = Metrics::with_buckets(&[0.1, 1.0]);
        metrics.observe("latency_seconds", &[], 0.05);
        metrics.observe("latency_seconds", &[], 0.5);
        metrics.observe("latency_seconds", &[], 5.0);
        let text = metrics.render();
        assert!(
            text.contains("latency_seconds_bucket{le=\"0.1\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("latency_seconds_bucket{le=\"1\"} 2"),
            "{text}"
        );
        assert!(
            text.contains("latency_seconds_bucket{le=\"+Inf\"} 3"),
            "{text}"
        );
        assert!(text.contains("latency_seconds_count 3"), "{text}");
        assert!(text.contains("latency_seconds_sum 5.55"), "{text}");
    }

    #[test]
    fn a_label_value_with_quotes_or_newlines_is_escaped() {
        let metrics = Metrics::new();
        metrics.increment("odd_total", &[("name", "a\"b\\c\nd")], 1);
        assert!(
            metrics
                .render()
                .contains(r#"odd_total{name="a\"b\\c\nd"} 1"#)
        );
    }

    #[test]
    fn two_registries_do_not_share_counters() {
        let one = Metrics::new();
        let two = Metrics::new();
        one.increment("hits_total", &[], 1);
        assert_eq!(one.counter_value("hits_total", &[]), Some(1));
        assert_eq!(two.counter_value("hits_total", &[]), None);
    }

    #[test]
    fn a_clone_shares_the_registry() {
        let metrics = Metrics::new();
        let clone = metrics.clone();
        clone.increment("hits_total", &[], 1);
        assert_eq!(metrics.counter_value("hits_total", &[]), Some(1));
    }

    #[test]
    fn buckets_are_sorted_on_the_way_in() {
        let metrics = Metrics::with_buckets(&[1.0, 0.1]);
        metrics.observe("latency_seconds", &[], 0.5);
        let text = metrics.render();
        let first = text.find("le=\"0.1\"").expect("0.1 bucket");
        let second = text.find("le=\"1\"").expect("1 bucket");
        assert!(first < second, "{text}");
    }
}
