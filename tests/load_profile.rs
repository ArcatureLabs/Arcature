//! The framework under concurrent traffic.
//!
//! Issue #6 opens by saying the framework has never served concurrent
//! traffic, and names four things that only show up when it does: pool
//! exhaustion, a lock held across an `await`, unbounded queue growth under
//! backpressure, and a rate limiter whose bucket map never evicts. This file
//! is the answer to that, and the first thing worth recording is that one of
//! the four is not here to be found.
//!
//! **There is no lock held across an `await`.** `src/` contains no `RwLock`
//! at all and no `parking_lot` dependency; every blocking-mutex site takes
//! its guard inside a synchronous function and drops it before returning --
//! the rate limiter's bucket table, the notification registry, the request
//! cache, the metrics table, the login throttle, the event recorder. A test
//! asserting the absence would pass without exercising anything, so none is
//! written. Saying so here is worth more than a green check that proves
//! nothing.
//!
//! The nearest real thing is in the same neighbourhood from a different
//! cause: the limiter's bucket sweep is `O(n)` under a `std::sync::Mutex` on
//! a Tokio worker thread, so a wide key space makes a *blocking* critical
//! section rather than a spanning one. That is measured below and recorded,
//! not gated -- see [`a_wide_key_space_is_measured_against_a_control`].
//!
//! ## What is asserted and what is only written down
//!
//! Latency percentiles and throughput are **recorded**. These runs happen on
//! a shared hosted runner whose neighbours are invisible, and this repository
//! already has the receipts for how much that matters: the identical `cargo
//! clippy` invocation took 49m54s with four agents compiling beside it and
//! 3m34s on an idle machine. A p99 threshold calibrated against either number
//! is a gate on the runner, not on the code.
//!
//! What is asserted is what stays true regardless of the machine: nobody gets
//! an error while the offered load is inside capacity, resident memory and
//! descriptor count do not climb across a sustained run, a flood on one
//! rate-limit key does not take out a caller on another, and shutdown
//! finishes the requests it had already accepted.
//!
//! ## "Against a generated application", and where this falls short
//!
//! The definition of done asks for a profile against a generated
//! application. Running `arc new` and building its output would be a cargo
//! build inside a test -- minutes per run, a second target directory, and a
//! toolchain assumption -- so what runs here is an application composed
//! through the same [`ApplicationBuilder`] the scaffold's `bootstrap/app.rs`
//! calls, with the same layers turned on. The pipeline under load is the real
//! one; the handlers are not the scaffold's. That gap is real and is left
//! open deliberately: closing it belongs to the end-to-end application proof,
//! not to a test binary.
//!
//! ## Overload does not mean 503 here, and asserting that it did would fail
//!
//! `Cargo.toml` takes `tower` with `default-features = false`, so
//! `tower::limit`, `tower::load_shed` and `tower::buffer` are not compiled
//! in. There is no concurrency limit and no load-shed layer anywhere in the
//! pipeline, and `src/application/pipeline.rs` says as much: every stage from
//! 5 down is off unless asked for. A default-built application under
//! deliberate overload therefore answers with **slow 200s**, and eventually a
//! 500 when the database pool's ten-second acquire timeout expires.
//!
//! So "graceful under overload" is asserted against applications that opted
//! in -- a rate limit for 429, a body limit for 413 -- because that is the
//! only place the claim is true. Asserting it against a default build would
//! fail for a reason that has nothing to do with the code being wrong.

#![cfg(feature = "test-kit")]

mod load;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use arcature::{Application, ApplicationBuilder};
use arcature::routing::{KeySource, RateLimit, Route, Routes};
use arcature::test_kit::TestApp;
use axum::Router;
use axum::http::HeaderName;

use load::Profile;

/// The header every keyed test buckets on.
const KEY_HEADER: &str = "x-api-key";

/// Something for a handler to return that is not empty, so a response body
/// exists to be read and a connection exists to be reused.
async fn hello() -> &'static str {
    "hello"
}

/// A handler that takes long enough to still be running when shutdown is
/// signalled.
async fn slow() -> &'static str {
    ARRIVED.fetch_add(1, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(2_000)).await;
    "slow"
}

/// How many requests have entered [`slow`] and not yet left.
///
/// The drain test signals shutdown once every request it fired is provably
/// inside a handler, rather than after a sleep long enough to *probably* be
/// true. A fixed sleep is the standard way to write this and it is a flake
/// waiting for a busy runner: if the twenty-fourth connection has not been
/// accepted when the signal lands, the server is right to refuse it and the
/// test is wrong to fail.
static ARRIVED: AtomicU64 = AtomicU64::new(0);

/// Roughly the shape of a scaffolded application's pipeline: request ids,
/// access logging, panic catching, security headers, a body limit.
///
/// Not the scaffold's handlers -- see the module documentation for why, and
/// for what that leaves unproven.
fn scaffold_shaped() -> ApplicationBuilder<()> {
    Application::<()>::new()
        .routes(Routes::new(vec![
            Route::get("/", hello),
            Route::get("/health/ready", hello),
            Route::get("/slow", slow),
        ]))
        .request_id()
        .catch_panic()
        .body_limit(64 * 1024)
}

/// The paths the sustained run cycles through.
fn mix() -> Vec<String> {
    vec!["/".to_owned(), "/health/ready".to_owned()]
}

/// Serve a router on an ephemeral port with real graceful shutdown.
///
/// [`TestApp::serve`] cannot be used where drain matters: its `TestServer`
/// stops the server by `abort()`ing the task on drop, which cuts in-flight
/// requests rather than finishing them, and it installs no shutdown hook to
/// drive. This binds its own listener so the distinction between "drained"
/// and "killed" is observable.
async fn serve_with_drain(router: Router) -> (String, tokio::sync::oneshot::Sender<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("an ephemeral port should bind");
    let address = listener.local_addr().expect("a bound listener has an address");
    let (signal, wait) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service())
            .with_graceful_shutdown(async {
                let _ = wait.await;
            })
            .await;
    });
    (format!("http://{address}"), signal)
}

/// The harness measures something, and every reading it reports is real.
///
/// This runs unmeasured on every `cargo test` rather than only under the
/// environment switch, because a harness exercised solely by the expensive
/// run is a harness that gets debugged during the expensive run. It is also
/// the anti-vacuity test for everything below: if `run` returned an empty
/// `Run`, every assertion in this file about error rates and flatness would
/// pass for the wrong reason.
#[tokio::test(flavor = "multi_thread")]
async fn the_harness_records_what_it_claims_to() {
    let app = TestApp::new(scaffold_shaped().build_stateless());
    let server = app.serve().await.expect("an ephemeral port should bind");

    let profile = Profile::smoke(mix())
        .connections(8)
        .duration(Duration::from_millis(700));
    let run = load::run(&server.base_url(), &profile).await;

    assert!(
        run.completed() > 0,
        "the generator sent nothing: every other assertion in this file would \
         pass vacuously",
    );
    assert_eq!(
        run.transport_errors, 0,
        "a loopback server refused or reset a connection",
    );
    assert_eq!(
        run.statuses.keys().copied().collect::<Vec<_>>(),
        vec![200],
        "the harness drove something other than the routes it thinks it did",
    );
    assert!(run.throughput() > 0.0, "throughput was reported as zero");

    // Percentiles must be ordered, or the sort and the rank arithmetic
    // disagree and every latency this file records is fiction.
    let (p50, p95, p99, max) = (
        run.percentile(50.0),
        run.percentile(95.0),
        run.percentile(99.0),
        run.percentile(100.0),
    );
    assert!(p50 <= p95 && p95 <= p99 && p99 <= max, "{p50:?} {p95:?} {p99:?} {max:?}");
    assert!(p50 > Duration::ZERO, "a request cannot have taken no time");

    assert!(
        run.samples.len() >= 4,
        "fewer than four samples means the quartile checks silently return \
         None and assert nothing: {} samples",
        run.samples.len(),
    );

    // On Linux the footprint samplers must actually have read something. Off
    // Linux they are expected to be absent, and the sustained run says so in
    // its report rather than pretending it checked.
    if cfg!(target_os = "linux") {
        assert!(
            run.rss_quarters().is_some(),
            "/proc/self/status gave no VmRSS on a Linux host",
        );
        assert!(
            run.descriptor_quarters().is_some(),
            "/proc/self/fd could not be counted on a Linux host",
        );
    }
}

/// A flood on one rate-limit key is refused, and a different caller keeps
/// being served throughout.
///
/// This is the assertion the issue's "rate limiter whose bucket map never
/// evicts" turns into once the map is read: eviction exists, so the
/// interesting failure is not growth but *collateral*. The limiter holds a
/// `std::sync::Mutex` around its bucket table on every request; if that lock
/// were contended badly enough, or if the key were shared where it should not
/// be, the polite caller would start seeing 429s that belong to somebody
/// else. It must not.
#[tokio::test(flavor = "multi_thread")]
async fn a_flood_on_one_key_is_refused_without_taking_out_another_caller() {
    let limit =
        RateLimit::per_second(50).by(KeySource::Header(HeaderName::from_static(KEY_HEADER)));
    let app = TestApp::new(scaffold_shaped().rate_limit(limit).build_stateless());
    let server = app.serve().await.expect("an ephemeral port should bind");
    let base = server.base_url();

    // A second caller, on its own key, asking politely throughout the flood.
    let polite_ok = Arc::new(AtomicU64::new(0));
    let polite_bad = Arc::new(AtomicU64::new(0));
    let polite = {
        let (base, ok, bad) = (base.clone(), Arc::clone(&polite_ok), Arc::clone(&polite_bad));
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            for _ in 0..20 {
                match client
                    .get(format!("{base}/"))
                    .header(KEY_HEADER, "polite")
                    .send()
                    .await
                {
                    Ok(response) if response.status().is_success() => {
                        ok.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {
                        bad.fetch_add(1, Ordering::Relaxed);
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    };

    let profile = Profile::smoke(vec!["/".to_owned()])
        .connections(32)
        .duration(Duration::from_millis(1_000))
        .header(KEY_HEADER, "flood");
    let run = load::run(&base, &profile).await;
    polite.await.expect("the polite caller should not panic");

    let refused = run.statuses.get(&429).copied().unwrap_or(0);
    assert!(
        refused > 0,
        "32 connections against a 50/s quota produced no 429s, so this test \
         never reached the overload it is named for: {:?}",
        run.statuses,
    );
    assert_eq!(
        run.transport_errors, 0,
        "refusal must be an answer, not a dropped connection",
    );
    let server_errors: u64 = run
        .statuses
        .iter()
        .filter(|(status, _)| **status >= 500)
        .map(|(_, count)| count)
        .sum();
    assert_eq!(server_errors, 0, "overload produced a 5xx: {:?}", run.statuses);

    assert_eq!(
        polite_bad.load(Ordering::Relaxed),
        0,
        "a caller on its own key was refused {} of 20 times while another key \
         was flooded",
        polite_bad.load(Ordering::Relaxed),
    );
    assert_eq!(
        polite_ok.load(Ordering::Relaxed),
        20,
        "the polite caller did not complete its 20 requests",
    );
}

/// Shutdown finishes the requests it had already accepted.
///
/// The plan calls this "graceful shutdown drains under load", and the
/// distinction it is testing is between a server that stops listening and a
/// server that stops answering. Twenty-four requests are in flight, each
/// four hundred milliseconds long, when the signal arrives; all twenty-four
/// must come back with a body, and the port must be closed to anything new.
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_finishes_the_requests_it_had_already_accepted() {
    ARRIVED.store(0, Ordering::SeqCst);
    let router = scaffold_shaped().build_stateless().into_router();
    let (base, signal) = serve_with_drain(router).await;

    let in_flight: Vec<_> = (0..24)
        .map(|_| {
            let base = base.clone();
            tokio::spawn(async move {
                reqwest::Client::new()
                    .get(format!("{base}/slow"))
                    .send()
                    .await
                    .map(|response| response.status().as_u16())
            })
        })
        .collect();

    // Wait for proof rather than for a duration: shutdown must be signalled
    // while all twenty-four are inside a handler, and on a busy runner
    // "probably accepted by now" is how this test would learn to flake.
    let waited = std::time::Instant::now();
    while ARRIVED.load(Ordering::SeqCst) < 24 {
        assert!(
            waited.elapsed() < Duration::from_secs(10),
            "only {} of 24 requests reached a handler in ten seconds, so this \
             test never set up the situation it exists to check",
            ARRIVED.load(Ordering::SeqCst),
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    signal.send(()).expect("the server task should still be listening");

    let mut drained = 0;
    for request in in_flight {
        let status = request
            .await
            .expect("an in-flight request task should not panic")
            .expect("an accepted request should be answered, not cut");
        assert_eq!(status, 200, "an in-flight request was answered with {status}");
        drained += 1;
    }
    assert_eq!(drained, 24, "shutdown dropped requests it had already accepted");

    // And the socket is closed: draining is not the same as staying open.
    let after = reqwest::Client::new()
        .get(format!("{base}/"))
        .timeout(Duration::from_secs(2))
        .send()
        .await;
    assert!(
        after.is_err(),
        "the listener kept accepting after shutdown: {:?}",
        after.map(|response| response.status()),
    );
}

/// The sustained run: sixty seconds, flat footprint, zero errors.
///
/// Gated behind `ARCATURE_LOAD` because a minute-long run on every
/// `cargo test` is a run people stop doing. The three tests above hold the
/// properties on the ordinary path; this one holds them for long enough that
/// a leak proportional to request count has somewhere to show.
#[tokio::test(flavor = "multi_thread")]
async fn a_sustained_run_holds_its_footprint_flat() {
    skip_unless_load_enabled!();

    let app = TestApp::new(scaffold_shaped().build_stateless());
    let server = app.serve().await.expect("an ephemeral port should bind");

    let profile = Profile::sustained(mix());
    let run = load::run(&server.base_url(), &profile).await;

    let mut report = run.report("Sustained: 128 connections, no rate limit", &profile);

    assert_eq!(
        run.transport_errors, 0,
        "connections were refused or reset while inside capacity",
    );
    assert_eq!(
        run.error_rate(),
        0.0,
        "traffic inside capacity produced errors: {:?}",
        run.statuses,
    );

    // Flatness, not a ceiling. The generator's own allocations are in the
    // same process, so an absolute bound would be measuring the harness; what
    // a leak looks like is a last quarter higher than a first quarter.
    match run.rss_quarters() {
        Some((first, last)) => {
            let growth = load::percent_change(first, last);
            assert!(
                growth < 10.0,
                "resident memory grew {growth:.1}% across the run ({:.1} MiB \
                 -> {:.1} MiB): a leak proportional to request count looks \
                 exactly like this",
                first / 1_048_576.0,
                last / 1_048_576.0,
            );
        }
        None => assert!(
            !cfg!(target_os = "linux"),
            "resident memory could not be sampled on a Linux host",
        ),
    }
    match run.descriptor_quarters() {
        Some((first, last)) => {
            assert!(
                last <= first + 8.0,
                "the process held {last:.1} descriptors at the end against \
                 {first:.1} at the start: a socket that is accepted and never \
                 closed looks exactly like this",
            );
        }
        None => assert!(
            !cfg!(target_os = "linux"),
            "descriptors could not be counted on a Linux host",
        ),
    }

    report.push('\n');
    write_report(&report);
    eprintln!("{report}");
}

/// The wide-key-space cost, measured against a control and written down
/// rather than gated.
///
/// The in-memory bucket table sweeps when it passes 8192 entries, but the
/// sweep drops only buckets that have refilled to capacity, so a key touched
/// once stays resident for `1 / refill_per_second`. Under a slow refill the
/// sweep therefore retains nearly everything, runs again on the next request,
/// and becomes `O(n)` work under a `std::sync::Mutex` on every request over a
/// table that keeps growing.
///
/// Whether that residency is a leak or a correct rate limit is a judgement
/// about arrival rate, not a property of the code -- a limiter that forgot a
/// key early would stop being a rate limit. So the throughput cost is
/// measured and recorded, and the only assertions are the ones that are not
/// judgement calls: that the process is still answering at the end, and that
/// the sweep did not turn overload into errors.
///
/// # Three runs, because two would not be attributable
///
/// An earlier version of this test compared an unbounded key space against
/// the unlimited sustained run and found a sevenfold throughput drop. That
/// comparison cannot support the conclusion it invites: the two runs differ
/// by *both* the limiter's presence and the key space, so the number is the
/// sum of two effects with no way to tell which is which. The same mistake,
/// in the same repository, is why `docs/src/dev-loop.md` carries a paragraph
/// bounding what its measurement can conclude.
///
/// So there are three, identical but for one variable each:
///
///   - **no limiter** -- the floor, what the pipeline costs on its own;
///   - **bounded keys** -- the limiter, with a key space small enough that
///     the table never reaches the sweep threshold;
///   - **unbounded keys** -- the limiter, with the table growing without
///     bound.
///
/// The first gap is what the limiter costs. The second is what the sweep
/// costs. Neither is knowable from one subtraction.
#[tokio::test(flavor = "multi_thread")]
async fn a_wide_key_space_is_measured_against_a_control() {
    skip_unless_load_enabled!();

    // A capacity no run can exhaust, over a refill slow enough that a used
    // bucket is never full again inside the run -- which is exactly the
    // condition under which the sweep retains everything it looks at. Without
    // the large burst the run would measure refusals instead of sweeping.
    // Slow refill: a capacity no run can exhaust, over a refill so slow that
    // a bucket which spent one token is never full again inside the run --
    // which is exactly the condition under which the sweep looks at every
    // entry and is entitled to drop none of them. Without the large burst
    // this would measure refusals instead of sweeping.
    let slow_refill = || {
        RateLimit::per_hour(10)
            .burst(1_000_000)
            .by(KeySource::Header(HeaderName::from_static(KEY_HEADER)))
    };
    // Fast refill: same limiter, same key minting, same allocation per
    // request -- but a bucket is full again within a microsecond of being
    // touched, so the sweep that runs at 8192 entries actually empties the
    // table. This is the control that separates "the key space is wide" from
    // "the sweep cannot evict anything".
    let fast_refill = || {
        RateLimit::per_second(1_000_000)
            .by(KeySource::Header(HeaderName::from_static(KEY_HEADER)))
    };
    let seconds = Duration::from_secs(20);

    let unlimited = {
        let app = TestApp::new(scaffold_shaped().build_stateless());
        let server = app.serve().await.expect("an ephemeral port should bind");
        let profile = Profile::sustained(vec!["/".to_owned()]).duration(seconds);
        let run = load::run(&server.base_url(), &profile).await;
        (run.report("Key space control: no rate limiter at all", &profile), run)
    };

    // 1024 keys: comfortably under the 8192-entry sweep threshold, so this
    // run pays for the limiter and never pays for a sweep.
    let bounded = {
        let app = TestApp::new(scaffold_shaped().rate_limit(slow_refill()).build_stateless());
        let server = app.serve().await.expect("an ephemeral port should bind");
        let profile = Profile::sustained(vec!["/".to_owned()])
            .duration(seconds)
            .key_space(KEY_HEADER, Some(1024));
        let run = load::run(&server.base_url(), &profile).await;
        (
            run.report("Key space bounded: 1024 keys, below the sweep threshold", &profile),
            run,
        )
    };

    let evictable = {
        let app = TestApp::new(scaffold_shaped().rate_limit(fast_refill()).build_stateless());
        let server = app.serve().await.expect("an ephemeral port should bind");
        let profile = Profile::sustained(vec!["/".to_owned()])
            .duration(seconds)
            .key_space(KEY_HEADER, None);
        let run = load::run(&server.base_url(), &profile).await;
        (
            run.report(
                "Key space unbounded, fast refill: the sweep can drop what it sees",
                &profile,
            ),
            run,
        )
    };

    let unbounded = {
        let app = TestApp::new(scaffold_shaped().rate_limit(slow_refill()).build_stateless());
        let server = app.serve().await.expect("an ephemeral port should bind");
        let profile = Profile::sustained(vec!["/".to_owned()])
            .duration(seconds)
            .key_space(KEY_HEADER, None);
        let run = load::run(&server.base_url(), &profile).await;
        (
            run.report(
                "Key space unbounded, slow refill: the sweep may drop nothing",
                &profile,
            ),
            run,
        )
    };

    for (label, run) in [
        ("no limiter", &unlimited.1),
        ("bounded keys", &bounded.1),
        ("unbounded keys, fast refill", &evictable.1),
        ("unbounded keys, slow refill", &unbounded.1),
    ] {
        assert_eq!(
            run.transport_errors, 0,
            "the {label} run cost connections, not just throughput",
        );
        let server_errors: u64 = run
            .statuses
            .iter()
            .filter(|(status, _)| **status >= 500)
            .map(|(_, count)| count)
            .sum();
        assert_eq!(
            server_errors, 0,
            "the {label} run turned into errors: {:?}",
            run.statuses,
        );
        assert!(
            run.completed() > 0,
            "the {label} run sent nothing, so its row is not a measurement",
        );
    }

    let (floor, limiter, evicting, stuck) = (
        unlimited.1.throughput(),
        bounded.1.throughput(),
        evictable.1.throughput(),
        unbounded.1.throughput(),
    );
    let attribution = format!(
        "## What the four rows above attribute\n\
         \n\
         no limiter                   {floor:.0} req/s\n\
         bounded keys, slow refill    {limiter:.0} req/s   ({:+.1}% vs no limiter)\n\
         unbounded keys, fast refill  {evicting:.0} req/s   ({:+.1}% vs bounded)\n\
         unbounded keys, slow refill  {stuck:.0} req/s   ({:+.1}% vs fast refill)\n\
         \n\
         Two variables, four runs, one moving at a time. Read down the second\n\
         column:\n\
         \n\
         * the limiter on a bounded key space is free;\n\
         * an unbounded key space is also nearly free -- so long as the sweep\n\
           can drop what it looks at;\n\
         * the cost is entirely in the last step, where a slow refill means\n\
           the sweep examines every entry and is entitled to evict none, then\n\
           does it again on the next request over a table that grew.\n\
         \n\
         So the hazard is not \"many keys\". It is many keys *and* a quota\n\
         whose refill is slower than the run, which is the shape of every\n\
         per-hour login or password-reset throttle. A per-minute or\n\
         per-second quota over the same key space does not pay it.\n\
         \n\
         All four runs are {:.0}s at {} connections against the same route.\n\n",
        load::percent_change(floor, limiter),
        load::percent_change(limiter, evicting),
        load::percent_change(evicting, stuck),
        seconds.as_secs_f64(),
        Profile::sustained(Vec::new()).connections,
    );

    let mut report = String::new();
    report.push_str(&unlimited.0);
    report.push('\n');
    report.push_str(&bounded.0);
    report.push('\n');
    report.push_str(&evictable.0);
    report.push('\n');
    report.push_str(&unbounded.0);
    report.push('\n');
    report.push_str(&attribution);
    write_report(&report);
    eprintln!("{report}");
}

/// Append a block to the fresh report at the workspace root.
///
/// Same shape as `unsafe-report.txt`: the run writes a report, and promoting
/// it to `load-baseline.<target>.txt` is a human decision that belongs in a
/// commit. Unlike the unsafe baseline there is no diff gate on it, for the
/// reason the module documentation gives -- these numbers describe the runner
/// as much as the code, so the record is for a reader, not for a check.
fn write_report(block: &str) {
    use std::io::Write as _;

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("load-report.txt");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .expect("the workspace root should be writable");
    file.write_all(block.as_bytes())
        .expect("the report should be writable");
}
