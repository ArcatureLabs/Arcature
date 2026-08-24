//! Terminal styling for the long-running commands.
//!
//! `arc dev`, `arc serve` and `arc build` hold a terminal for minutes at a
//! time, and what they print is the only view of the framework anybody has
//! while they work. Until `0.1.3` that view was a debug dump: an IPC pipe
//! name, cargo's own "Finished `dev` profile [unoptimized + debuginfo]" line,
//! and a row of six timings with no hierarchy between them.
//!
//! # No dependency, and no feature flag
//!
//! ANSI escapes written directly. A colour crate would be a dependency on the
//! request path of nothing, carried by every build that compiles the CLI, to
//! save about forty lines. `std::io::IsTerminal` has been in std since 1.70
//! and this crate requires 1.97.
//!
//! # When colour is off
//!
//! Three ways, checked in order, because each catches a case the others miss:
//!
//! - `NO_COLOR` set to anything non-empty. The [informal standard]; honouring
//!   it is one `env::var` and not honouring it is a bug report.
//! - `TERM=dumb`, which is what an editor's build pane usually claims to be.
//! - stdout is not a terminal, which covers `arc dev > log`, a CI runner, and
//!   the supervisor's output being read by another process.
//!
//! Every helper returns a plain `String` when colour is off, so a caller never
//! branches and no format string carries escapes that might not be wanted.
//!
//! [informal standard]: https://no-color.org/

use std::io::IsTerminal;
use std::sync::OnceLock;

/// Whether escapes should be emitted at all.
///
/// Resolved once. The answer cannot change while the process runs -- stdout
/// is not re-opened and the environment is read at startup -- and a
/// long-running command would otherwise ask three questions per line printed.
fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) {
            return false;
        }
        if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
            return false;
        }
        std::io::stdout().is_terminal()
    })
}

/// Wrap `text` in `code`, or return it unchanged when colour is off.
fn paint(code: &str, text: &str) -> String {
    if enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// Secondary text: labels, units, anything the eye should skip.
#[must_use]
pub fn dim(text: &str) -> String {
    paint("2", text)
}

/// The one thing on the screen worth looking at.
#[must_use]
pub fn bold(text: &str) -> String {
    paint("1", text)
}

/// Success, and the URL the developer is about to click.
#[must_use]
pub fn green(text: &str) -> String {
    paint("32", text)
}

/// A warning that is not a failure.
#[must_use]
pub fn yellow(text: &str) -> String {
    paint("33", text)
}

/// A failure.
#[must_use]
pub fn red(text: &str) -> String {
    paint("31", text)
}

/// The framework's own mark, used once per run.
#[must_use]
pub fn brand(text: &str) -> String {
    paint("1;36", text)
}

/// A duration, rendered the way a person reads one.
///
/// Milliseconds below a second, because "0.03s" is three characters spent
/// saying "fast" and `30ms` says it better. Two decimals above, because the
/// difference between a 2.3s and a 2.9s rebuild is the thing being watched.
#[must_use]
pub fn duration(value: std::time::Duration) -> String {
    let seconds = value.as_secs_f32();
    if seconds < 1.0 {
        format!("{}ms", value.as_millis())
    } else {
        format!("{seconds:.2}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The test process's stdout is not a terminal, so the helpers must be
    /// transparent here. That is also the property that keeps escapes out of
    /// a redirected log.
    #[test]
    fn styling_is_transparent_when_it_is_not_a_terminal() {
        assert_eq!(dim("x"), "x");
        assert_eq!(bold("x"), "x");
        assert_eq!(green("x"), "x");
        assert_eq!(brand("x"), "x");
        assert!(!red("x").contains('\x1b'));
    }

    #[test]
    fn a_sub_second_duration_is_milliseconds() {
        assert_eq!(duration(Duration::from_millis(30)), "30ms");
        assert_eq!(duration(Duration::from_millis(999)), "999ms");
    }

    #[test]
    fn a_duration_of_a_second_or_more_is_seconds_with_two_decimals() {
        assert_eq!(duration(Duration::from_millis(1000)), "1.00s");
        assert_eq!(duration(Duration::from_millis(2310)), "2.31s");
    }
}
