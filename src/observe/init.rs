//! Installing the process-wide log subscriber.
//!
//! The framework installs nothing on its own. [`install_logging`] is a
//! function the *application* calls, once, from `main` -- which is the same
//! rule as before ("operators wire their own"), with the twenty lines of
//! boilerplate that rule used to require moved somewhere they can be got
//! right once instead of copied wrongly into every scaffold.
//!
//! What it buys is the difference between an application that reports what it
//! is doing and one that is silent. `tracing` events go nowhere at all unless
//! a subscriber is installed: without this call, [`AccessLogLayer`] runs on
//! every request and emits into the void, the asset-manifest warning in
//! [`crate::assets`] is never seen, and a job that fails does so without a
//! line anywhere. Nothing errors -- it is simply quiet, which is the worst
//! way for logging to be broken because it looks like nothing is happening.
//!
//! [`AccessLogLayer`]: super::AccessLogLayer

use std::str::FromStr as _;

use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use super::json_log::{JsonLog, StderrSink};

/// The environment variable that overrides the filter.
///
/// `RUST_LOG` and not `ARCATURE_LOG`: every Rust operator already knows the
/// name, every deployment guide already mentions it, and an application that
/// invents its own is one more thing to look up at three in the morning.
pub const FILTER_ENV: &str = "RUST_LOG";

/// Install the process-wide log subscriber. Call once, from `main`, before
/// anything that might log.
///
/// Two formats, chosen by `cfg!(debug_assertions)` and not by an environment
/// variable -- the same rule the rest of the framework follows, because the
/// shape of a log line is a property of the build, not of where it happens to
/// be running:
///
/// - **Debug builds** get [`tracing_subscriber::fmt`], which is meant for a
///   person reading a terminal.
/// - **Release builds** get [`JsonLog`], one object per line, with the
///   [`redact`](super::redact) deny-list applied to every field on the way
///   out. That is the format a log shipper reads, and the redaction is the
///   reason it is not optional.
///
/// Both write to standard error, so a log line never interleaves with
/// whatever the process writes to standard output.
///
/// `default_filter` is used when `RUST_LOG` is unset -- something like
/// `"info,my_app=debug"`. If `RUST_LOG` is set but unparseable the default is
/// used anyway and a line says so: a typo in a log filter must not stop a
/// process from booting.
///
/// # Errors
///
/// Returns [`ObserveError::Logging`] if a global subscriber is already
/// installed. That is a real error and not a no-op: the second caller's
/// configuration is being silently discarded, and the honest response is to
/// say which one won.
///
/// [`ObserveError::Logging`]: super::ObserveError::Logging
pub fn install_logging(default_filter: &str) -> Result<(), super::ObserveError> {
    let filter = filter(default_filter);

    let installed = if cfg!(debug_assertions) {
        tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    // The `ansi` feature is off, so colour is unavailable
                    // rather than disabled. Saying so explicitly keeps the
                    // line from reading like an oversight.
                    .with_ansi(false)
                    .with_target(true),
            )
            .try_init()
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(JsonLog::new(StderrSink))
            .try_init()
    };

    installed.map_err(|_| super::ObserveError::Logging {
        reason: "a global tracing subscriber is already installed",
    })
}

/// Resolve the filter: `RUST_LOG` if it parses, the caller's default
/// otherwise.
///
/// [`Targets`] rather than `EnvFilter` is a deliberate trade. `EnvFilter`
/// brings a regex engine along for span-field matching that a web application
/// almost never uses; `Targets` reads the `target=level` syntax everyone
/// actually writes -- `info`, `info,sqlx=warn`, `my_app=debug` -- and costs
/// no additional dependency at all.
fn filter(default_filter: &str) -> Targets {
    let from_env = std::env::var(FILTER_ENV).ok();
    resolve(from_env.as_deref(), default_filter)
}

/// The decision [`filter`] makes, with the environment passed in rather than
/// read, so it can be tested without a process-wide mutation that the
/// `unsafe_code = "forbid"` lint would reject and that a parallel test run
/// would race on anyway.
fn resolve(from_env: Option<&str>, default_filter: &str) -> Targets {
    let fallback = || {
        Targets::from_str(default_filter).unwrap_or_else(|_| {
            // The caller's own default did not parse. That is a bug in the
            // application, not in its environment, so it must not be silent
            // -- but it must not be fatal either.
            eprintln!(
                "warning: default log filter `{default_filter}` is not valid; logging at `info`"
            );
            Targets::new().with_default(tracing::Level::INFO)
        })
    };

    match from_env {
        Some(value) if !value.trim().is_empty() => Targets::from_str(value).unwrap_or_else(|_| {
            eprintln!(
                "warning: {FILTER_ENV} is not a valid log filter; using the application default"
            );
            fallback()
        }),
        _ => fallback(),
    }
}

#[cfg(test)]
mod tests {
    use tracing::Level;

    use super::{Targets, resolve};

    #[test]
    fn an_absent_variable_uses_the_default() {
        let targets = resolve(None, "warn,arcature=debug");
        assert!(targets.would_enable("arcature", &Level::DEBUG));
        assert!(!targets.would_enable("other", &Level::INFO));
    }

    #[test]
    fn an_empty_variable_counts_as_absent() {
        let targets = resolve(Some("   "), "arcature=debug");
        assert!(targets.would_enable("arcature", &Level::DEBUG));
    }

    #[test]
    fn the_variable_wins_when_it_parses() {
        let targets = resolve(Some("error"), "debug");
        assert!(!targets.would_enable("arcature", &Level::INFO));
        assert!(targets.would_enable("arcature", &Level::ERROR));
    }

    #[test]
    fn an_unparseable_variable_falls_back_rather_than_failing() {
        let targets = resolve(Some("=="), "arcature=debug");
        assert!(targets.would_enable("arcature", &Level::DEBUG));
    }

    #[test]
    fn an_unparseable_default_lands_on_info_rather_than_panicking() {
        let targets: Targets = resolve(None, "== not a filter ==");
        assert!(targets.would_enable("anything", &Level::INFO));
        assert!(!targets.would_enable("anything", &Level::DEBUG));
    }
}
