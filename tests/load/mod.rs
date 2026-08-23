//! A load generator, and the sampling that makes a load run mean something.
//!
//! Four criterion benches already measure functions. This measures the thing
//! a framework is actually asked to do: hold a lot of connections open at
//! once and keep answering. The two are not substitutes. A microbenchmark
//! that got 10% faster tells you nothing about a pool of ten under five
//! hundred callers, and the failures worth finding -- a map that grows and
//! never shrinks, a queue nobody drains, a socket nobody closes -- are all
//! invisible until the run lasts longer than a moment.
//!
//! ## What is measured, and what a number here can conclude
//!
//! Latency percentiles and throughput are **recorded, not asserted**. The
//! runs happen on a shared hosted runner whose neighbours are invisible; the
//! same commit measured twice can differ by more than any regression worth
//! catching, so a p99 threshold would be a gate on the runner's mood. There
//! is a real example of this in the repository already: the same `cargo
//! clippy` invocation took 49m54s with four agents compiling beside it and
//! 3m34s on an idle machine. A threshold set against the first number is
//! meaningless and a threshold set against the second fails constantly.
//!
//! What *is* asserted is the set of claims that hold regardless of how fast
//! the machine is on the day:
//!
//!   - no request fails while the offered load is inside capacity;
//!   - resident memory is no higher at the end of a sustained run than at the
//!     start, beyond a stated margin;
//!   - the process holds no more file descriptors at the end than at the
//!     start, beyond a stated margin -- a socket that is not closed is a leak
//!     whether or not it costs latency;
//!   - deliberate overload is refused rather than absorbed, and the server is
//!     still answering afterwards.
//!
//! Those are properties of the code. The percentiles are properties of the
//! code *and* the machine, so they go in the record and not in an `assert!`.
//!
//! ## Why the samplers read `/proc`
//!
//! Resident set size and open descriptor count are the two numbers that
//! expose a leak, and neither is available from `std` on any platform. The
//! portable ways to get them are a C library call or a crate that makes one.
//! Both were rejected: the crate is a dependency added for a test, and the
//! call needs `unsafe` in a repository whose central claim is that it has
//! none. Linux publishes both as text files, so on Linux they are read and on
//! everything else they are reported absent -- [`Sample::rss_bytes`] is an
//! `Option`, and the flatness checks say plainly when they had nothing to
//! check. CI runs Linux, which is where the assertion needs to hold.
//!
//! Sampling the *test process* is correct here rather than a compromise:
//! [`TestApp::serve`](arcature::test_kit::TestApp::serve) runs the server on a
//! task in this process, so the server's memory and the server's sockets are
//! this process's memory and sockets.
//!
//! It does mean the generator is inside its own measurement, and the first
//! Linux run proved that is not something a comparison can be reasoned around.
//! The check is a last quarter against a first quarter rather than a bound
//! against a constant, which handles a generator with a *fixed* overhead --
//! and handles a generator that grows not at all, because linear growth is
//! precisely what a quartile comparison is built to detect. Keeping every
//! latency in a vector was therefore a leak the leak check reported as a leak:
//! 38.9 MiB to 47.5 MiB across sixty seconds, against a server holding no
//! per-request state. See [`Histogram`], which is the fix and the argument
//! for it. Anything added to this harness later has to keep that property:
//! per-request state in the generator is indistinguishable from per-request
//! state in the framework.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Turns the load suite on. Absent, every load test returns immediately.
///
/// Off by default because a sustained run is minutes, and a suite that costs
/// minutes on every `cargo test` is a suite people stop running.
pub const LOAD_VAR: &str = "ARCATURE_LOAD";

/// Turns a skipped load run from a pass into a failure.
///
/// The same two-variable shape as
/// [`REQUIRE_TEST_DB_VAR`](arcature::test_kit::REQUIRE_TEST_DB_VAR), for the
/// same reason and it is worth repeating: without the skip nobody can run the
/// rest of the suite on a laptop, and without the switch nobody would notice
/// the day CI stopped setting the first variable and the load job started
/// passing in four seconds while measuring nothing.
pub const REQUIRE_LOAD_VAR: &str = "ARCATURE_REQUIRE_LOAD";

/// Whether this run should generate load.
///
/// Panics rather than returning `false` when [`REQUIRE_LOAD_VAR`] is set, so
/// the job that exists to run these cannot quietly stop running them.
#[must_use]
pub fn enabled() -> bool {
    if std::env::var_os(LOAD_VAR).is_some() {
        return true;
    }
    assert!(
        std::env::var_os(REQUIRE_LOAD_VAR).is_none(),
        "{REQUIRE_LOAD_VAR} is set but {LOAD_VAR} is not: this run was \
         supposed to generate load and would instead have reported success \
         without sending a request",
    );
    false
}

/// Skip the calling test unless load generation is on.
///
/// Written as a macro so the `return` lands in the test function rather than
/// in a helper, which is the difference between skipping the test and
/// skipping one line of it.
#[macro_export]
macro_rules! skip_unless_load_enabled {
    () => {
        if !$crate::load::enabled() {
            eprintln!(
                "skipping: set {} to run the load profile",
                $crate::load::LOAD_VAR
            );
            return;
        }
    };
}

/// The shape of one run.
#[derive(Debug, Clone)]
pub struct Profile {
    /// How many requests are in flight at once.
    ///
    /// Each is a task with its own connection, so this is also the number of
    /// sockets the server is asked to hold.
    pub connections: usize,
    /// How long to keep offering load after the warm-up.
    pub duration: Duration,
    /// Load offered before measurement starts.
    ///
    /// The first requests through a cold process pay for lazily built
    /// routers, a TLS-less but still unpooled client, and the allocator's
    /// first calls to the kernel. Including them makes p99 a measurement of
    /// process start-up, and -- worse for the leak check -- makes the first
    /// memory sample artificially low, so ordinary warm-up looks like growth.
    pub warmup: Duration,
    /// How often to record memory and descriptor counts.
    pub sample_every: Duration,
    /// The paths to request, cycled through by each connection.
    ///
    /// A mix rather than one path, because a single route exercises one code
    /// path and a framework's cost is spread across the pipeline every
    /// request pays.
    pub paths: Vec<String>,
    /// Headers every request carries.
    ///
    /// Present for one reason: [`TestApp::serve`] installs no `ConnectInfo`
    /// (it calls `into_make_service`, not the `_with_connect_info` variant),
    /// so `KeySource::Ip` collapses every caller in this harness into the
    /// shared `UNIDENTIFIED_KEY` bucket. Any test that needs distinct
    /// rate-limit buckets has to mint them from a header instead.
    pub headers: Vec<(String, String)>,
    /// A header carrying a rate-limit key, and how many distinct values it
    /// takes.
    ///
    /// Off by default. Turning it on is the only way to reach a rate
    /// limiter's key-space behaviour from here: the in-memory bucket table is
    /// swept when it passes 8192 entries, but the sweep drops only buckets
    /// that have refilled to full, so residency per one-shot key is
    /// `window / limit` and a tight quota over a long window keeps entries
    /// alive for minutes.
    pub key_space: Option<KeySpace>,
}

/// How many distinct rate-limit keys a run mints.
#[derive(Debug, Clone)]
pub struct KeySpace {
    /// The header the key rides in.
    pub header: String,
    /// How many distinct values it takes, or `None` for a fresh one every
    /// request.
    ///
    /// A bounded space below the limiter's 8192-entry sweep threshold is the
    /// control: it pays for the limiter without ever paying for the sweep, so
    /// the difference between it and an unbounded space is attributable to
    /// the sweep rather than to the limiter existing.
    pub distinct: Option<usize>,
}

impl Profile {
    /// The sustained profile: enough connections to matter, long enough for a
    /// leak to show.
    ///
    /// Sixty seconds is chosen against the issue this answers, which says a
    /// slow leak is invisible in a thirty-second test. It is not long enough
    /// to catch a leak of a few bytes an hour and does not claim to be; it is
    /// long enough that a leak proportional to request count -- the kind an
    /// unevicted map produces -- moves the number by more than the noise.
    #[must_use]
    pub fn sustained(paths: Vec<String>) -> Self {
        Self {
            connections: 128,
            duration: Duration::from_secs(60),
            warmup: Duration::from_secs(5),
            sample_every: Duration::from_millis(500),
            paths,
            headers: Vec::new(),
            key_space: None,
        }
    }

    /// A short profile for proving the harness itself works.
    ///
    /// Every assertion the sustained run makes is made here too, over a few
    /// seconds. A harness whose only exercise is the expensive run is a
    /// harness that gets debugged during the expensive run.
    #[must_use]
    pub fn smoke(paths: Vec<String>) -> Self {
        Self {
            connections: 16,
            duration: Duration::from_secs(3),
            warmup: Duration::from_millis(500),
            sample_every: Duration::from_millis(100),
            paths,
            headers: Vec::new(),
            key_space: None,
        }
    }

    /// Send `name: value` on every request.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Mint rate-limit keys in `name`, `distinct` of them.
    ///
    /// `None` means a fresh key on every request.
    #[must_use]
    pub fn key_space(mut self, name: &str, distinct: Option<usize>) -> Self {
        self.key_space = Some(KeySpace {
            header: name.to_owned(),
            distinct,
        });
        self
    }

    /// Override the connection count.
    #[must_use]
    pub fn connections(mut self, connections: usize) -> Self {
        self.connections = connections;
        self
    }

    /// Override how long the measured phase runs.
    #[must_use]
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }
}

/// One reading of the process's footprint.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// When it was taken, from the start of measurement.
    pub at: Duration,
    /// Resident set size in bytes, or `None` off Linux.
    pub rss_bytes: Option<u64>,
    /// Open file descriptors, or `None` off Linux.
    ///
    /// Sockets are descriptors, so a connection the server accepted and never
    /// closed shows up here even when it costs no measurable memory.
    pub open_files: Option<u64>,
}

impl Sample {
    fn now(at: Duration) -> Self {
        Self {
            at,
            rss_bytes: rss_bytes(),
            open_files: open_files(),
        }
    }
}

/// Resident set size of this process, in bytes.
#[cfg(target_os = "linux")]
fn rss_bytes() -> Option<u64> {
    // `VmRSS` rather than `/proc/self/statm`, whose second field is in pages
    // and would need the page size -- which `std` does not expose either, so
    // the simpler-looking file is the one that needs the C call.
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find_map(|it| it.strip_prefix("VmRSS:"))?;
    let kilobytes: u64 = line.split_whitespace().next()?.parse().ok()?;
    Some(kilobytes * 1024)
}

#[cfg(not(target_os = "linux"))]
fn rss_bytes() -> Option<u64> {
    None
}

/// How many descriptors this process holds.
#[cfg(target_os = "linux")]
fn open_files() -> Option<u64> {
    // The count includes the descriptor `read_dir` itself is holding, which
    // is a constant offset and therefore invisible to a check that compares
    // one sample against another.
    Some(std::fs::read_dir("/proc/self/fd").ok()?.count() as u64)
}

#[cfg(not(target_os = "linux"))]
fn open_files() -> Option<u64> {
    None
}

/// Sub-buckets per power of two. Eight gives at most 12.5% relative error.
const SUB_BITS: u32 = 3;
const SUB: usize = 1 << SUB_BITS;
/// Powers of two covered, in microseconds: up to roughly thirteen days.
const OCTAVES: usize = 40;

/// A latency distribution in constant memory.
///
/// # Why not a vector of every latency
///
/// The obvious implementation keeps every measurement and sorts at the end,
/// which gives exact percentiles. It also grows by sixteen bytes per request
/// for the length of the run, inside the very process whose resident memory
/// this harness asserts is flat -- so at a few thousand requests a second the
/// generator allocates several megabytes over a minute, monotonically, and
/// the leak check reports it as a leak.
///
/// That is not a hypothetical. The first Linux run of the sustained profile
/// failed on exactly this: 38.9 MiB to 47.5 MiB, 22.1% growth, against a
/// server holding no per-request state at all. An instrument whose own cost
/// grows with what it measures cannot be used to decide whether the thing it
/// measures grows.
///
/// So: log-spaced buckets over microseconds, `OCTAVES * SUB` of them, eight
/// bytes each. Two and a half kilobytes, allocated once, never resized.
///
/// The trade is exactness. A percentile read out of a bucket is that bucket's
/// lower bound, so a reported figure is a slight under-estimate, by at most
/// one part in eight. That is affordable precisely because these numbers are
/// recorded and not asserted -- a 12.5% band on a figure nobody gates on
/// costs nothing, while 8 MiB of drift on a figure everybody gates on costs
/// the whole check.
pub struct Histogram {
    buckets: Box<[u64]>,
    count: u64,
    max: Duration,
}

impl std::fmt::Debug for Histogram {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Histogram")
            .field("count", &self.count)
            .field("max", &self.max)
            .finish_non_exhaustive()
    }
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

impl Histogram {
    /// An empty histogram. This is the only allocation it ever makes.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buckets: vec![0; OCTAVES * SUB].into_boxed_slice(),
            count: 0,
            max: Duration::ZERO,
        }
    }

    /// Which bucket `micros` falls in.
    ///
    /// Below `SUB` every value is its own bucket, so short latencies are
    /// exact. Above it, the top `SUB_BITS` significant bits pick the
    /// sub-bucket within the value's octave, which is the standard
    /// constant-relative-error layout.
    fn index(micros: u64) -> usize {
        if micros < SUB as u64 {
            return micros as usize;
        }
        let octave = 63 - micros.leading_zeros() as usize;
        let shift = octave - SUB_BITS as usize;
        let sub = ((micros >> shift) as usize) & (SUB - 1);
        let slot = (octave - SUB_BITS as usize + 1) * SUB + sub;
        slot.min(OCTAVES * SUB - 1)
    }

    /// The lowest latency a bucket can hold, which is what it reports.
    fn lower_bound(index: usize) -> u64 {
        if index < SUB {
            return index as u64;
        }
        let octave = index / SUB + SUB_BITS as usize - 1;
        let sub = (index % SUB) as u64;
        let shift = octave - SUB_BITS as usize;
        (SUB as u64 + sub) << shift
    }

    /// Record one measurement.
    pub fn record(&mut self, latency: Duration) {
        let micros = u64::try_from(latency.as_micros()).unwrap_or(u64::MAX);
        self.buckets[Self::index(micros)] += 1;
        self.count += 1;
        self.max = self.max.max(latency);
    }

    /// Fold another histogram into this one.
    pub fn merge(&mut self, other: &Self) {
        for (mine, theirs) in self.buckets.iter_mut().zip(other.buckets.iter()) {
            *mine += *theirs;
        }
        self.count += other.count;
        self.max = self.max.max(other.max);
    }

    /// How many measurements went in.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    /// The largest measurement, kept exactly.
    ///
    /// Exact rather than bucketed because the maximum is the one figure a
    /// reader treats as a specific event rather than as a summary.
    #[must_use]
    pub fn max(&self) -> Duration {
        self.max
    }

    /// The latency at percentile `p`, by nearest rank over the buckets.
    #[must_use]
    pub fn percentile(&self, p: f64) -> Duration {
        if self.count == 0 {
            return Duration::ZERO;
        }
        if p >= 100.0 {
            return self.max;
        }
        let rank = ((p / 100.0) * self.count as f64).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for (index, hits) in self.buckets.iter().enumerate() {
            seen += *hits;
            if seen >= rank {
                return Duration::from_micros(Self::lower_bound(index));
            }
        }
        self.max
    }
}

/// What one run produced.
#[derive(Debug)]
pub struct Run {
    /// The latency distribution, in constant memory. See [`Histogram`].
    latencies: Histogram,
    /// How many responses arrived with each status.
    pub statuses: BTreeMap<u16, u64>,
    /// Requests that never produced a status: refused, reset, timed out.
    ///
    /// Separate from a 5xx on purpose. Under deliberate overload a refused
    /// connection is the kernel's accept queue doing its job, and a 503 is
    /// the application doing its job; both are graceful, and neither is a
    /// request that was silently lost.
    pub transport_errors: u64,
    /// Wall-clock time the measured phase took.
    pub elapsed: Duration,
    /// Footprint readings taken across the measured phase.
    pub samples: Vec<Sample>,
}

impl Run {
    /// How many requests completed with a status.
    #[must_use]
    pub fn completed(&self) -> u64 {
        self.statuses.values().sum()
    }

    /// Completed requests plus transport failures.
    #[must_use]
    pub fn attempted(&self) -> u64 {
        self.completed() + self.transport_errors
    }

    /// Responses whose status was 2xx or 3xx.
    #[must_use]
    pub fn successful(&self) -> u64 {
        self.statuses
            .iter()
            .filter(|(status, _)| (200..400).contains(*status))
            .map(|(_, count)| count)
            .sum()
    }

    /// Fraction of attempts that were not a 2xx or 3xx, in `0.0..=1.0`.
    #[must_use]
    pub fn error_rate(&self) -> f64 {
        let attempted = self.attempted();
        if attempted == 0 {
            return 0.0;
        }
        let bad = attempted - self.successful();
        bad as f64 / attempted as f64
    }

    /// Completed requests per second across the measured phase.
    #[must_use]
    pub fn throughput(&self) -> f64 {
        let seconds = self.elapsed.as_secs_f64();
        if seconds <= 0.0 {
            return 0.0;
        }
        self.completed() as f64 / seconds
    }

    /// The latency at percentile `p`, by nearest rank over the buckets.
    ///
    /// Nearest rank rather than interpolation because an interpolated p99 is a
    /// number no request experienced. Bucketed rather than exact because the
    /// exact version leaks into the leak check -- see [`Histogram`] -- so a
    /// figure here is the containing bucket's lower bound, under-reporting by
    /// at most one part in eight. `percentile(100.0)` is the true maximum.
    #[must_use]
    pub fn percentile(&self, p: f64) -> Duration {
        self.latencies.percentile(p)
    }

    /// Mean of the first and last quarter of a sampled series.
    ///
    /// Quarters rather than endpoints: a single first and last reading is two
    /// samples' worth of noise deciding whether a leak exists. Quarters of a
    /// sixty-second run at two hertz are thirty readings each.
    fn quarters(&self, of: impl Fn(&Sample) -> Option<u64>) -> Option<(f64, f64)> {
        let values: Vec<u64> = self.samples.iter().filter_map(of).collect();
        // Four samples is the fewest that gives each quarter one reading.
        if values.len() < 4 {
            return None;
        }
        let quarter = values.len() / 4;
        let mean = |slice: &[u64]| slice.iter().sum::<u64>() as f64 / slice.len() as f64;
        Some((
            mean(&values[..quarter]),
            mean(&values[values.len() - quarter..]),
        ))
    }

    /// First-quarter and last-quarter mean resident bytes.
    #[must_use]
    pub fn rss_quarters(&self) -> Option<(f64, f64)> {
        self.quarters(|sample| sample.rss_bytes)
    }

    /// First-quarter and last-quarter mean descriptor count.
    #[must_use]
    pub fn descriptor_quarters(&self) -> Option<(f64, f64)> {
        self.quarters(|sample| sample.open_files)
    }

    /// A human-readable block for the durable record.
    #[must_use]
    pub fn report(&self, title: &str, profile: &Profile) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "## {title}");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "connections     {}\nduration        {:.1}s (after {:.1}s warm-up)",
            profile.connections,
            self.elapsed.as_secs_f64(),
            profile.warmup.as_secs_f64(),
        );
        let _ = writeln!(
            out,
            "requests        {} completed, {} transport failures",
            self.completed(),
            self.transport_errors,
        );
        let _ = writeln!(out, "throughput      {:.0} req/s", self.throughput());
        let _ = writeln!(
            out,
            "samples         {} over {:.1}s",
            self.samples.len(),
            self.samples
                .last()
                .map_or(0.0, |sample| sample.at.as_secs_f64()),
        );
        let _ = writeln!(out, "error rate      {:.4}%", self.error_rate() * 100.0);
        let _ = writeln!(
            out,
            "latency p50     {:.2}ms\nlatency p95     {:.2}ms\nlatency p99     {:.2}ms\nlatency max     {:.2}ms  (exact; the three above are\n                                bucket lower bounds, under-reporting\n                                by at most one part in eight)",
            millis(self.percentile(50.0)),
            millis(self.percentile(95.0)),
            millis(self.percentile(99.0)),
            millis(self.percentile(100.0)),
        );
        let _ = write!(out, "statuses        ");
        if self.statuses.is_empty() {
            let _ = writeln!(out, "(none)");
        } else {
            let rendered: Vec<String> = self
                .statuses
                .iter()
                .map(|(status, count)| format!("{status}x{count}"))
                .collect();
            let _ = writeln!(out, "{}", rendered.join(" "));
        }
        match self.rss_quarters() {
            Some((first, last)) => {
                let _ = writeln!(
                    out,
                    "resident        {:.1} MiB first quarter -> {:.1} MiB last quarter ({:+.1}%)",
                    first / 1_048_576.0,
                    last / 1_048_576.0,
                    percent_change(first, last),
                );
            }
            None => {
                let _ = writeln!(out, "resident        not sampled on this platform");
            }
        }
        match self.descriptor_quarters() {
            Some((first, last)) => {
                let _ = writeln!(
                    out,
                    "descriptors     {first:.1} first quarter -> {last:.1} last quarter ({:+.1}%)",
                    percent_change(first, last),
                );
            }
            None => {
                let _ = writeln!(out, "descriptors     not sampled on this platform");
            }
        }
        out
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Change from `first` to `last` as a percentage, zero-safe.
#[must_use]
pub fn percent_change(first: f64, last: f64) -> f64 {
    if first <= 0.0 {
        return 0.0;
    }
    (last - first) / first * 100.0
}

/// Offer load at `base_url` for the length of `profile`, and record it.
///
/// One task per connection, each looping over `profile.paths` until the
/// deadline. Open-loop rate control is deliberately absent: this measures
/// what the server does when callers send as fast as it answers, which is the
/// condition a saturated pool or an unbounded queue shows up under. A
/// closed-loop generator that paced itself would be measuring its own pacing.
pub async fn run(base_url: &str, profile: &Profile) -> Run {
    assert!(
        profile.connections > 0,
        "a run needs at least one connection"
    );
    assert!(!profile.paths.is_empty(), "a run needs at least one path");

    let profile_connections = profile.connections;
    let stop = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::with_capacity(profile.connections);

    // Warm-up runs through the same workers as the measured phase, so the
    // connections that carry the measurement are the ones already warm.
    let measure_from = Instant::now() + profile.warmup;
    let deadline = measure_from + profile.duration;

    for index in 0..profile.connections {
        let client = reqwest::Client::builder()
            // One connection per worker, kept for the whole run. Without this
            // the pool would reconnect between requests and the run would
            // measure TCP handshakes.
            .pool_max_idle_per_host(1)
            .timeout(Duration::from_secs(10))
            .build()
            .expect("a plain HTTP client with no TLS roots to load");
        let urls: Vec<String> = profile
            .paths
            .iter()
            .map(|path| format!("{base_url}{path}"))
            .collect();
        let stop = Arc::clone(&stop);
        let headers = profile.headers.clone();
        let key_space = profile.key_space.clone();
        workers.push(tokio::spawn(async move {
            let mut latencies = Histogram::new();
            let mut statuses: BTreeMap<u16, u64> = BTreeMap::new();
            let mut transport_errors = 0u64;
            // Workers start at different points in the path list so a mix is
            // a mix at every instant, not a series of same-path waves.
            let mut cursor = index % urls.len();
            let mut minted = 0u64;
            while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                let url = &urls[cursor];
                cursor = (cursor + 1) % urls.len();
                let mut request = client.get(url);
                for (name, value) in &headers {
                    request = request.header(name, value);
                }
                if let Some(space) = &key_space {
                    // The worker index is in every key, so two workers cannot
                    // mint the same value and quietly halve the key space.
                    // A bounded space is cut per worker for the same reason:
                    // `distinct` is the total, not the total times the
                    // connection count.
                    let value = match space.distinct {
                        Some(total) => {
                            let per_worker = total.div_ceil(profile_connections).max(1) as u64;
                            format!("w{index}-{}", minted % per_worker)
                        }
                        None => format!("w{index}-{minted}"),
                    };
                    request = request.header(&space.header, value);
                    minted += 1;
                }
                let sent = Instant::now();
                let outcome = request.send().await;
                let waited = sent.elapsed();
                // Anything before the measurement window is warm-up: it is
                // sent, and its result is discarded rather than never
                // requested, because a warm-up that does not exercise the
                // same path warms nothing.
                let measuring = sent >= measure_from;
                match outcome {
                    Ok(response) => {
                        if measuring {
                            *statuses.entry(response.status().as_u16()).or_default() += 1;
                            latencies.record(waited);
                        }
                        // The body has to be drained for the connection to be
                        // reusable; leaving it undrained would turn every
                        // request into a fresh socket and quietly make the
                        // descriptor check measure the client.
                        let _ = response.bytes().await;
                    }
                    Err(_) if measuring => transport_errors += 1,
                    Err(_) => {}
                }
            }
            (latencies, statuses, transport_errors)
        }));
    }

    // Sampling starts when measurement does, so warm-up growth is not counted
    // as leak.
    let sampler = {
        let stop = Arc::clone(&stop);
        let every = profile.sample_every;
        tokio::spawn(async move {
            tokio::time::sleep_until(measure_from.into()).await;
            let start = Instant::now();
            let mut samples = Vec::new();
            while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                samples.push(Sample::now(start.elapsed()));
                tokio::time::sleep(every).await;
            }
            samples
        })
    };

    let started = measure_from;
    let mut latencies = Histogram::new();
    let mut statuses: BTreeMap<u16, u64> = BTreeMap::new();
    let mut transport_errors = 0;
    for worker in workers {
        let (worker_latencies, worker_statuses, worker_errors) = worker
            .await
            .expect("a load worker to finish without panicking");
        latencies.merge(&worker_latencies);
        for (status, count) in worker_statuses {
            *statuses.entry(status).or_default() += count;
        }
        transport_errors += worker_errors;
    }
    stop.store(true, Ordering::Relaxed);
    let samples = sampler
        .await
        .expect("the sampler to finish without panicking");

    Run {
        latencies,
        statuses,
        transport_errors,
        elapsed: started.elapsed(),
        samples,
    }
}

#[cfg(test)]
mod histogram_tests {
    use super::{Histogram, OCTAVES, SUB};
    use std::time::Duration;

    /// Every bucket index must be reachable and ordered, or a percentile is
    /// reading a bucket that does not correspond to the latency it names.
    #[test]
    fn bucket_lower_bounds_are_strictly_increasing() {
        let mut previous = None;
        for index in 0..OCTAVES * SUB {
            let bound = Histogram::lower_bound(index);
            if let Some(previous) = previous {
                assert!(
                    bound > previous,
                    "bucket {index} starts at {bound}, not above {previous}",
                );
            }
            previous = Some(bound);
        }
    }

    /// A value must land in a bucket whose range contains it. This is the
    /// property everything else rests on: if `index` and `lower_bound`
    /// disagree, percentiles are wrong in a way no other test would notice.
    #[test]
    fn every_value_lands_in_a_bucket_that_contains_it() {
        let mut probes: Vec<u64> = (0..64).collect();
        for shift in 3..40 {
            for offset in [0u64, 1, 7, 100] {
                probes.push((1u64 << shift) + offset);
                probes.push((1u64 << shift) - 1 - offset.min((1 << shift) - 1));
            }
        }
        for value in probes {
            let index = Histogram::index(value);
            let low = Histogram::lower_bound(index);
            assert!(low <= value, "{value} landed in a bucket starting at {low}");
            let next = Histogram::lower_bound(index + 1);
            assert!(
                value < next,
                "{value} landed in a bucket ending at {next}, which excludes it",
            );
        }
    }

    /// Relative error is bounded, which is the whole claim the exactness
    /// trade-off rests on.
    #[test]
    fn a_bucket_under_reports_by_at_most_one_part_in_eight() {
        for value in [8u64, 9, 1_000, 12_345, 999_999, 1 << 30] {
            let low = Histogram::lower_bound(Histogram::index(value));
            let error = (value - low) as f64 / value as f64;
            assert!(error < 1.0 / SUB as f64, "{value} -> {low} is {error}");
        }
    }

    #[test]
    fn percentiles_track_a_known_distribution() {
        let mut histogram = Histogram::new();
        // 1..=1000 milliseconds, one each.
        for millis in 1..=1000u64 {
            histogram.record(Duration::from_millis(millis));
        }
        assert_eq!(histogram.count(), 1000);
        assert_eq!(histogram.max(), Duration::from_millis(1000));

        for (p, expected) in [(50.0, 500u64), (95.0, 950), (99.0, 990)] {
            let got = histogram.percentile(p).as_millis() as u64;
            let error = (expected - got) as f64 / expected as f64;
            assert!(
                got <= expected && error < 1.0 / SUB as f64,
                "p{p} reported {got}ms against {expected}ms",
            );
        }
        assert_eq!(histogram.percentile(100.0), Duration::from_millis(1000));
    }

    #[test]
    fn merging_is_the_same_as_recording_into_one() {
        let (mut left, mut right, mut both) =
            (Histogram::new(), Histogram::new(), Histogram::new());
        for millis in 1..=100u64 {
            if millis % 2 == 0 {
                left.record(Duration::from_millis(millis));
            } else {
                right.record(Duration::from_millis(millis));
            }
            both.record(Duration::from_millis(millis));
        }
        left.merge(&right);
        assert_eq!(left.count(), both.count());
        assert_eq!(left.max(), both.max());
        for p in [50.0, 90.0, 99.0, 100.0] {
            assert_eq!(left.percentile(p), both.percentile(p), "p{p}");
        }
    }

    /// The reason this type exists: its footprint does not depend on how many
    /// measurements it holds.
    #[test]
    fn memory_does_not_grow_with_the_number_of_measurements() {
        let mut histogram = Histogram::new();
        let before = histogram.buckets.len();
        for micros in 0..200_000u64 {
            histogram.record(Duration::from_micros(micros));
        }
        assert_eq!(
            histogram.buckets.len(),
            before,
            "the bucket array was resized, which is the growth this type exists to avoid",
        );
        assert_eq!(histogram.count(), 200_000);
    }

    #[test]
    fn an_empty_histogram_reports_zero_rather_than_panicking() {
        let histogram = Histogram::new();
        assert_eq!(histogram.count(), 0);
        assert_eq!(histogram.percentile(50.0), Duration::ZERO);
        assert_eq!(histogram.percentile(100.0), Duration::ZERO);
    }
}
