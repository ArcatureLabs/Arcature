//! What `/metrics` serves is text a scraper can actually read.
//!
//! `src/observe/metrics.rs` is well covered by unit tests, and every one of
//! them asserts with `contains`. That proves a substring is present; it
//! cannot prove the document around the substring parses, and the Prometheus
//! text format has rules that a `contains` assertion is structurally unable
//! to see: every sample of a name must sit under one `# TYPE` line, a name's
//! samples must be contiguous, a series may appear once, a histogram's
//! buckets must be cumulative and must end at `+Inf`. Break any of those and
//! every existing test still passes while the endpoint returns something a
//! scraper rejects outright -- and a rejected scrape is silent, because
//! nobody reads the scrape target's error page.
//!
//! So this file writes a parser. Not a lenient one: it is the format's rules
//! turned into code, and it is itself tested against hand-written documents
//! that violate each rule in turn, because a validator nothing fails is a
//! validator that proves nothing. `the_parser_rejects_...` is the test that
//! keeps the rest of this file honest.
//!
//! A parser rather than a dependency on a Prometheus client crate, for the
//! same reason the rest of the suite avoids one: a crate pulled in to check
//! forty lines of text is a dependency tree somebody has to watch for
//! advisories forever.
//!
//! Reference: the Prometheus text exposition format, version 0.0.4, which is
//! the version `PROMETHEUS_CONTENT_TYPE` announces.

#![cfg(feature = "observe")]

use std::collections::{BTreeMap, HashMap, HashSet};

use arcature::observe::Metrics;
use arcature::observe::metrics::{
    DEFAULT_BUCKETS, HTTP_REQUEST_DURATION, HTTP_REQUESTS_TOTAL, PROMETHEUS_CONTENT_TYPE,
};

// ---------------------------------------------------------------------------
// The parser
// ---------------------------------------------------------------------------

/// What a `# TYPE` line declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Counter,
    Gauge,
    Histogram,
    Summary,
    Untyped,
}

impl Kind {
    fn parse(word: &str) -> Option<Self> {
        match word {
            "counter" => Some(Self::Counter),
            "gauge" => Some(Self::Gauge),
            "histogram" => Some(Self::Histogram),
            "summary" => Some(Self::Summary),
            "untyped" => Some(Self::Untyped),
            _ => None,
        }
    }
}

/// One sample line: a series and its value.
#[derive(Debug, Clone)]
struct Sample {
    name: String,
    /// Sorted, so two orderings of one label set compare equal -- which is
    /// what makes the duplicate-series check mean what it says.
    labels: BTreeMap<String, String>,
    value: f64,
}

impl Sample {
    /// The series identity: the name plus the label set, as text.
    fn series(&self) -> String {
        let rendered: Vec<String> = self
            .labels
            .iter()
            .map(|(key, value)| format!("{key}={value:?}"))
            .collect();
        format!("{}{{{}}}", self.name, rendered.join(","))
    }
}

/// A parsed exposition document.
#[derive(Debug, Default)]
struct Exposition {
    types: HashMap<String, Kind>,
    helps: HashMap<String, String>,
    samples: Vec<Sample>,
}

impl Exposition {
    /// Every sample whose name is exactly `name`.
    fn samples_named<'a>(&'a self, name: &str) -> Vec<&'a Sample> {
        self.samples.iter().filter(|s| s.name == name).collect()
    }

    /// The value of the one sample with this name and these labels.
    fn value(&self, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
        let wanted: BTreeMap<String, String> = labels
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        self.samples
            .iter()
            .find(|s| s.name == name && s.labels == wanted)
            .map(|s| s.value)
    }
}

/// A metric name: `[a-zA-Z_:][a-zA-Z0-9_:]*`.
fn valid_metric_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_' || first == ':')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

/// A label name: `[a-zA-Z_][a-zA-Z0-9_]*`. No colon: those are reserved for
/// names produced by recording rules, and are not valid on a label.
fn valid_label_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parse the value token: a float, or one of the three special literals the
/// format spells out.
fn parse_value(token: &str) -> Result<f64, String> {
    match token {
        "+Inf" => Ok(f64::INFINITY),
        "-Inf" => Ok(f64::NEG_INFINITY),
        "NaN" => Ok(f64::NAN),
        other => other
            .parse::<f64>()
            .map_err(|_| format!("{other:?} is not a sample value")),
    }
}

/// Read `{a="1",b="2"}` starting at the opening brace.
///
/// Returns the labels and the offset just past the closing brace. Escapes
/// are validated here rather than tolerated: `\` may only introduce `\`,
/// `"` or `n`, and anything else means the renderer emitted a value it did
/// not escape, which is the bug that lets a label value close its own quote.
fn parse_labels(rest: &str) -> Result<(BTreeMap<String, String>, usize), String> {
    let bytes = rest.as_bytes();
    let mut index = 1; // past '{'
    let mut labels = BTreeMap::new();
    loop {
        while index < bytes.len() && bytes[index] == b' ' {
            index += 1;
        }
        if index < bytes.len() && bytes[index] == b'}' {
            return Ok((labels, index + 1));
        }
        let start = index;
        while index < bytes.len() && bytes[index] != b'=' {
            index += 1;
        }
        if index >= bytes.len() {
            return Err("a label list that never closes".to_string());
        }
        let key = rest[start..index].trim().to_string();
        if !valid_label_name(&key) {
            return Err(format!("{key:?} is not a valid label name"));
        }
        if labels.contains_key(&key) {
            return Err(format!("the label {key:?} appears twice on one series"));
        }
        index += 1; // past '='
        if index >= bytes.len() || bytes[index] != b'"' {
            return Err(format!("the value of {key:?} is not quoted"));
        }
        index += 1; // past the opening quote
        let mut value = String::new();
        loop {
            let Some(&byte) = bytes.get(index) else {
                return Err(format!("the value of {key:?} is never closed"));
            };
            match byte {
                b'"' => {
                    index += 1;
                    break;
                }
                b'\\' => {
                    index += 1;
                    match bytes.get(index) {
                        Some(b'\\') => value.push('\\'),
                        Some(b'"') => value.push('"'),
                        Some(b'n') => value.push('\n'),
                        Some(other) => {
                            return Err(format!(
                                "\\{} is not an escape the format defines",
                                *other as char
                            ));
                        }
                        None => return Err("a trailing backslash".to_string()),
                    }
                    index += 1;
                }
                _ => {
                    // Multi-byte UTF-8 is copied through whole; label values
                    // are arbitrary UTF-8 and slicing per byte would split a
                    // codepoint.
                    let start = index;
                    while index < bytes.len() && bytes[index] != b'"' && bytes[index] != b'\\' {
                        index += 1;
                    }
                    value.push_str(&rest[start..index]);
                }
            }
        }
        labels.insert(key, value);
        while index < bytes.len() && bytes[index] == b' ' {
            index += 1;
        }
        match bytes.get(index) {
            Some(b',') => index += 1,
            Some(b'}') => return Ok((labels, index + 1)),
            _ => return Err("a label list that never closes".to_string()),
        }
    }
}

/// Parse a whole exposition document, enforcing the format's rules.
///
/// Every `Err` here is a document a scraper would reject. The rules are
/// listed in the match arms below rather than in a comment, so a reader can
/// see exactly what "well formed" is being taken to mean.
fn parse(text: &str) -> Result<Exposition, String> {
    let mut out = Exposition::default();
    let mut seen_series: HashSet<String> = HashSet::new();
    // Which names have already had a block of samples, and which name the
    // current block belongs to. The format requires a name's samples to be
    // contiguous; interleaving two names is a parse error.
    let mut closed_names: HashSet<String> = HashSet::new();
    let mut current_name: Option<String> = None;

    for (number, line) in text.lines().enumerate() {
        let number = number + 1;
        let fail = |message: String| format!("line {number}: {message}");
        if line.trim().is_empty() {
            continue;
        }
        if let Some(comment) = line.strip_prefix("# ") {
            let mut words = comment.splitn(3, ' ');
            match (words.next(), words.next(), words.next()) {
                (Some("HELP"), Some(name), text) => {
                    if !valid_metric_name(name) {
                        return Err(fail(format!("{name:?} is not a valid metric name")));
                    }
                    if out
                        .helps
                        .insert(name.to_string(), text.unwrap_or_default().to_string())
                        .is_some()
                    {
                        return Err(fail(format!("a second HELP line for {name}")));
                    }
                }
                (Some("TYPE"), Some(name), kind) => {
                    if !valid_metric_name(name) {
                        return Err(fail(format!("{name:?} is not a valid metric name")));
                    }
                    let kind = kind
                        .and_then(Kind::parse)
                        .ok_or_else(|| fail(format!("{kind:?} is not a metric type")))?;
                    if out.types.insert(name.to_string(), kind).is_some() {
                        return Err(fail(format!("a second TYPE line for {name}")));
                    }
                }
                _ => {
                    // A plain comment. Legal, and nothing to check.
                }
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }

        // A sample line.
        let name_end = line
            .find(['{', ' '])
            .ok_or_else(|| fail("a sample line with no value".to_string()))?;
        let name = &line[..name_end];
        if !valid_metric_name(name) {
            return Err(fail(format!("{name:?} is not a valid metric name")));
        }
        let rest = &line[name_end..];
        let (labels, consumed) = if rest.starts_with('{') {
            parse_labels(rest).map_err(fail)?
        } else {
            (BTreeMap::new(), 0)
        };
        let tail = rest[consumed..].trim();
        let mut fields = tail.split_ascii_whitespace();
        let value = fields
            .next()
            .ok_or_else(|| fail(format!("{name} has no value")))?;
        let value = parse_value(value).map_err(fail)?;
        // An optional timestamp may follow, and nothing else may.
        if let Some(timestamp) = fields.next() {
            timestamp
                .parse::<i64>()
                .map_err(|_| fail(format!("{timestamp:?} is not a timestamp")))?;
        }
        if fields.next().is_some() {
            return Err(fail("trailing data after the sample".to_string()));
        }

        // The name a `# TYPE` line would have declared. Histogram and
        // summary samples are named after their base metric plus a suffix.
        let base = ["_bucket", "_sum", "_count"]
            .iter()
            .find_map(|suffix| name.strip_suffix(suffix))
            .filter(|base| out.types.contains_key(*base))
            .unwrap_or(name)
            .to_string();
        if !out.types.contains_key(&base) {
            return Err(fail(format!("{name} has no TYPE line")));
        }
        if current_name.as_deref() != Some(base.as_str()) {
            if !closed_names.insert(base.clone()) {
                return Err(fail(format!(
                    "the samples of {base} are not contiguous: another name came between them"
                )));
            }
            current_name = Some(base);
        }

        let sample = Sample {
            name: name.to_string(),
            labels,
            value,
        };
        if !seen_series.insert(sample.series()) {
            return Err(fail(format!("{} appears twice", sample.series())));
        }
        out.samples.push(sample);
    }

    check_histograms(&out)?;
    Ok(out)
}

/// The histogram rules, which are about a set of lines rather than one line.
///
/// Cumulative means non-decreasing as `le` grows; `+Inf` must be present and
/// must equal `_count`, because `+Inf` *is* the count by definition. A
/// scraper that reads a histogram whose buckets go down computes a negative
/// rate, which is not an error anywhere -- it is just a wrong graph.
fn check_histograms(exposition: &Exposition) -> Result<(), String> {
    for (name, kind) in &exposition.types {
        if *kind != Kind::Histogram {
            continue;
        }
        // Group the buckets by the series they belong to: the label set
        // without `le`.
        let mut by_series: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
        for sample in exposition.samples_named(&format!("{name}_bucket")) {
            let mut labels = sample.labels.clone();
            let bound = labels
                .remove("le")
                .ok_or_else(|| format!("{name}_bucket has no le label"))?;
            let bound = parse_value(&bound)
                .map_err(|_| format!("{name}_bucket has le={bound:?}, which is not a bound"))?;
            let key = format!("{labels:?}");
            by_series
                .entry(key)
                .or_default()
                .push((bound, sample.value));
        }
        for (series, buckets) in by_series {
            let mut previous_bound = f64::NEG_INFINITY;
            let mut previous_count = f64::NEG_INFINITY;
            for (bound, count) in &buckets {
                if *bound <= previous_bound {
                    return Err(format!(
                        "{name}{series}: buckets are not in ascending le order at le={bound}"
                    ));
                }
                if *count < previous_count {
                    return Err(format!(
                        "{name}{series}: bucket counts are not cumulative at le={bound} \
                         ({count} after {previous_count})"
                    ));
                }
                previous_bound = *bound;
                previous_count = *count;
            }
            let infinite = buckets
                .iter()
                .find(|(bound, _)| bound.is_infinite())
                .ok_or_else(|| format!("{name}{series} has no +Inf bucket"))?;
            let count = exposition
                .samples_named(&format!("{name}_count"))
                .into_iter()
                .find(|sample| format!("{:?}", sample.labels) == series)
                .ok_or_else(|| format!("{name}{series} has no _count"))?;
            if (infinite.1 - count.value).abs() > f64::EPSILON {
                return Err(format!(
                    "{name}{series}: the +Inf bucket is {} but _count is {}",
                    infinite.1, count.value
                ));
            }
            if !exposition
                .samples_named(&format!("{name}_sum"))
                .into_iter()
                .any(|sample| format!("{:?}", sample.labels) == series)
            {
                return Err(format!("{name}{series} has no _sum"));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// A registry with one of everything in it
// ---------------------------------------------------------------------------

/// A registry exercising every branch of the renderer: a described counter,
/// an undescribed one, two label sets on one name, a gauge, a histogram, and
/// a label value carrying all three characters the format needs escaped.
fn populated() -> Metrics {
    let metrics = Metrics::new();
    metrics.describe_counter("orders_total", "Orders accepted.");
    metrics.increment("orders_total", &[("channel", "web")], 3);
    metrics.increment("orders_total", &[("channel", "api")], 4);
    metrics.increment("orders_total", &[("channel", "api")], 1);
    // No describe: the renderer has to emit a TYPE line anyway, or these
    // samples belong to no declared metric.
    metrics.increment("retries_total", &[], 2);
    metrics.describe_gauge("queue_depth", "Jobs waiting.");
    metrics.set("queue_depth", &[("queue", "default")], 17.5);
    metrics.describe_histogram("render_seconds", "Template render time.");
    for value in [0.001, 0.03, 0.4, 7.0, 30.0] {
        metrics.observe("render_seconds", &[("template", "invoice")], value);
    }
    metrics.increment("odd_total", &[("label", "a\"b\\c\nd")], 1);
    metrics
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

#[test]
fn the_rendered_registry_is_a_document_the_format_accepts() {
    let text = populated().render();
    let exposition = parse(&text).unwrap_or_else(|error| {
        panic!("the exposition does not parse -- {error}\n---\n{text}---");
    });
    assert!(
        !exposition.samples.is_empty(),
        "a document that parses but contains nothing proves nothing"
    );
}

#[test]
fn every_name_carries_exactly_one_type_line_and_its_help_when_described() {
    let text = populated().render();
    let exposition = parse(&text).expect("the exposition parses");
    for name in [
        "orders_total",
        "retries_total",
        "queue_depth",
        "render_seconds",
        "odd_total",
    ] {
        assert!(
            exposition.types.contains_key(name),
            "{name} has no TYPE line in\n{text}"
        );
        // One `# TYPE` per name is enforced by `parse` itself: a second one
        // is an `Err`. Counting here as well would only re-test the parser.
        assert_eq!(
            text.matches(&format!("# TYPE {name} ")).count(),
            1,
            "{name} has more than one TYPE line"
        );
    }
    assert_eq!(
        exposition.helps.get("orders_total").map(String::as_str),
        Some("Orders accepted."),
    );
    assert_eq!(
        exposition.types.get("render_seconds"),
        Some(&Kind::Histogram),
    );
    assert_eq!(exposition.types.get("queue_depth"), Some(&Kind::Gauge));
}

#[test]
fn the_counter_values_survive_the_round_trip_through_the_text() {
    let exposition = parse(&populated().render()).expect("the exposition parses");
    assert_eq!(
        exposition.value("orders_total", &[("channel", "web")]),
        Some(3.0)
    );
    assert_eq!(
        exposition.value("orders_total", &[("channel", "api")]),
        Some(5.0),
        "two increments of one series did not accumulate"
    );
    assert_eq!(exposition.value("retries_total", &[]), Some(2.0));
    assert_eq!(
        exposition.value("queue_depth", &[("queue", "default")]),
        Some(17.5)
    );
}

#[test]
fn a_label_value_carrying_a_quote_a_backslash_and_a_newline_comes_back_intact() {
    // The escaping is not decoration. An unescaped quote in a label value
    // closes the value early and the rest of the line becomes syntax, which
    // is how a metric label turns into an injection into the scrape.
    let exposition = parse(&populated().render()).expect("the exposition parses");
    assert_eq!(
        exposition.value("odd_total", &[("label", "a\"b\\c\nd")]),
        Some(1.0),
        "the escaped label value did not survive being parsed back"
    );
}

#[test]
fn the_histogram_buckets_are_cumulative_and_end_at_infinity() {
    let text = populated().render();
    let exposition = parse(&text).expect("the exposition parses");
    // `parse` has already enforced monotonicity and the `+Inf`/`_count`
    // agreement for every histogram in the document. What is left to assert
    // is that the numbers are the right ones, which the parser cannot know.
    let buckets: Vec<(String, f64)> = exposition
        .samples_named("render_seconds_bucket")
        .into_iter()
        .map(|sample| {
            (
                sample.labels.get("le").cloned().unwrap_or_default(),
                sample.value,
            )
        })
        .collect();
    assert_eq!(
        buckets.len(),
        DEFAULT_BUCKETS.len() + 1,
        "one bucket per bound plus +Inf, got {buckets:?}"
    );
    assert_eq!(
        buckets.last().map(|(bound, _)| bound.as_str()),
        Some("+Inf"),
        "the last bucket is not +Inf: {buckets:?}"
    );
    // 0.001 and 0.03 fall at or below 0.05; 0.4 does not.
    assert_eq!(
        exposition.value(
            "render_seconds_bucket",
            &[("template", "invoice"), ("le", "0.05")]
        ),
        Some(2.0),
        "{text}"
    );
    assert_eq!(
        exposition.value("render_seconds_count", &[("template", "invoice")]),
        Some(5.0)
    );
    let sum = exposition
        .value("render_seconds_sum", &[("template", "invoice")])
        .expect("a histogram renders a _sum");
    // A tolerance rather than an equality: the sum is accumulated in `f64`
    // and rendered with `{}`, so pinning the exact decimal would be pinning
    // the last bit of a floating-point addition, not the exposition format.
    assert!(
        (sum - 37.431).abs() < 1e-9,
        "the _sum is {sum}, not the total of the observations: {text}"
    );
}

#[test]
fn no_series_appears_twice_in_one_document() {
    // Enforced by `parse`, which is why this test is about the interesting
    // case rather than the general one: a name with several label sets is
    // where a renderer that keys on the name alone would emit the same
    // series twice.
    let text = populated().render();
    parse(&text).expect("the exposition parses");
    let exposition = parse(&text).expect("the exposition parses");
    assert_eq!(
        exposition.samples_named("orders_total").len(),
        2,
        "two label sets on one name should be two samples"
    );
}

#[test]
fn the_layer_the_framework_installs_produces_a_document_that_parses() {
    // The registry above is hand-fed. This one is the shape a real
    // application produces: whatever `MetricsLayer` records, under the
    // constant names the framework exports.
    let metrics = Metrics::new();
    metrics.describe_counter(HTTP_REQUESTS_TOTAL, "Total HTTP requests handled.");
    metrics.describe_histogram(HTTP_REQUEST_DURATION, "HTTP request duration in seconds.");
    metrics.increment(
        HTTP_REQUESTS_TOTAL,
        &[("method", "GET"), ("status", "200")],
        1,
    );
    metrics.increment(
        HTTP_REQUESTS_TOTAL,
        &[("method", "POST"), ("status", "422")],
        1,
    );
    metrics.observe(HTTP_REQUEST_DURATION, &[("method", "GET")], 0.012);
    let text = metrics.render();
    let exposition = parse(&text).unwrap_or_else(|error| panic!("{error}\n---\n{text}---"));
    assert_eq!(
        exposition.value(HTTP_REQUESTS_TOTAL, &[("method", "GET"), ("status", "200")]),
        Some(1.0)
    );
}

#[tokio::test]
async fn the_metrics_response_carries_the_content_type_a_scraper_expects_and_parses() {
    let (parts, body) = populated().response().into_parts();
    assert_eq!(
        parts
            .headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some(PROMETHEUS_CONTENT_TYPE),
        "a scraper decides how to parse from this header; the wrong one is a failed scrape"
    );
    let bytes = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .expect("the metrics body is finite and small");
    let text = String::from_utf8(bytes.to_vec()).expect("the exposition is UTF-8");
    parse(&text).unwrap_or_else(|error| panic!("the served body does not parse -- {error}"));
}

#[test]
fn the_parser_rejects_a_document_for_each_rule_it_claims_to_enforce() {
    // Without this test the rest of the file proves only that `parse`
    // returns `Ok`, which a function body of `Ok(Default::default())` would
    // also do. Each case below is a real scraper-visible defect.
    let cases: &[(&str, &str)] = &[
        ("a sample with no TYPE line", "orders_total 1\n"),
        (
            "two TYPE lines for one name",
            "# TYPE orders_total counter\n# TYPE orders_total gauge\norders_total 1\n",
        ),
        (
            "two HELP lines for one name",
            "# HELP orders_total One.\n# HELP orders_total Two.\n# TYPE orders_total counter\norders_total 1\n",
        ),
        (
            "the same series twice",
            "# TYPE orders_total counter\norders_total{a=\"1\"} 1\norders_total{a=\"1\"} 2\n",
        ),
        (
            "the same series twice with the labels reordered",
            "# TYPE orders_total counter\norders_total{a=\"1\",b=\"2\"} 1\norders_total{b=\"2\",a=\"1\"} 2\n",
        ),
        (
            "two names interleaved",
            "# TYPE a_total counter\n# TYPE b_total counter\na_total 1\nb_total 1\na_total{x=\"1\"} 1\n",
        ),
        (
            "a label value that closes its own quote",
            "# TYPE orders_total counter\norders_total{a=\"un\"quoted\"} 1\n",
        ),
        (
            "an escape the format does not define",
            "# TYPE orders_total counter\norders_total{a=\"c:\\path\"} 1\n",
        ),
        (
            "a label name with a hyphen",
            "# TYPE orders_total counter\norders_total{a-b=\"1\"} 1\n",
        ),
        (
            "a metric name with a hyphen",
            "# TYPE orders-total counter\norders-total 1\n",
        ),
        (
            "a value that is not a number",
            "# TYPE orders_total counter\norders_total lots\n",
        ),
        (
            "a histogram whose buckets go down",
            "# TYPE d histogram\nd_bucket{le=\"1\"} 5\nd_bucket{le=\"2\"} 2\nd_bucket{le=\"+Inf\"} 5\nd_sum 1\nd_count 5\n",
        ),
        (
            "a histogram with no +Inf bucket",
            "# TYPE d histogram\nd_bucket{le=\"1\"} 5\nd_sum 1\nd_count 5\n",
        ),
        (
            "a histogram whose +Inf disagrees with its count",
            "# TYPE d histogram\nd_bucket{le=\"1\"} 5\nd_bucket{le=\"+Inf\"} 9\nd_sum 1\nd_count 5\n",
        ),
        (
            "a histogram with no sum",
            "# TYPE d histogram\nd_bucket{le=\"1\"} 5\nd_bucket{le=\"+Inf\"} 5\nd_count 5\n",
        ),
        (
            "a bucket with no le label",
            "# TYPE d histogram\nd_bucket 5\nd_sum 1\nd_count 5\n",
        ),
    ];
    for (what, document) in cases {
        assert!(
            parse(document).is_err(),
            "the parser accepted {what}, so it would accept it from the renderer too:\n{document}"
        );
    }
}

#[test]
fn the_parser_accepts_the_things_the_format_allows() {
    // The other half of the previous test. A validator that rejects
    // everything also passes a suite of rejection cases.
    let cases: &[(&str, &str)] = &[
        (
            "a bare comment",
            "# just a note\n# TYPE a_total counter\na_total 1\n",
        ),
        ("a blank line", "# TYPE a_total counter\n\na_total 1\n"),
        (
            "an explicit timestamp",
            "# TYPE a_total counter\na_total 1 1700000000000\n",
        ),
        ("a gauge holding NaN", "# TYPE a_ratio gauge\na_ratio NaN\n"),
        (
            "a label value holding UTF-8",
            "# TYPE a_total counter\na_total{name=\"caf\u{e9}\"} 1\n",
        ),
        (
            "a well-formed histogram",
            "# TYPE d histogram\nd_bucket{le=\"1\"} 2\nd_bucket{le=\"2\"} 5\nd_bucket{le=\"+Inf\"} 5\nd_sum 3.5\nd_count 5\n",
        ),
    ];
    for (what, document) in cases {
        parse(document)
            .unwrap_or_else(|error| panic!("the parser rejected {what}: {error}\n{document}"));
    }
}

#[test]
fn a_label_name_the_application_chose_badly_is_the_applications_to_get_wrong() {
    // The limit of the mechanism, written down rather than left to be
    // discovered. `Metrics` escapes label *values*, and does not validate
    // label *names* -- so a name with a hyphen renders into text no scraper
    // will accept, and nothing in the framework says so at the call site.
    //
    // The framework's own layer cannot hit this: `MetricsLayer` labels with
    // `method`, `status` and a `&'static str` route, and there is no path
    // by which a request supplies a label name. This is only reachable by
    // an application calling `increment` with a name it made up, which is
    // why it is documented here as a limit rather than reported as a leak.
    let metrics = Metrics::new();
    metrics.increment("orders_total", &[("delivery-channel", "web")], 1);
    let text = metrics.render();
    assert!(
        text.contains("delivery-channel=\"web\""),
        "the renderer passed the name through verbatim, as expected: {text}"
    );
    assert!(
        parse(&text).is_err(),
        "a hyphenated label name should not produce a parseable document"
    );
}
