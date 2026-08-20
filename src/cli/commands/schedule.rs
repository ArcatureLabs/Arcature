//! `arc schedule [--dsn <url>]` — run the application's job scheduler.
//!
//! The recurring-job scheduler is configured by the application (its
//! `Scheduler` holds boxed enqueue closures). Arcature does not own those
//! bindings, so this command runs `cargo run -- --schedule` in the current
//! directory so the app's own binary drives the scheduler loop.
//!
//! An optional `--dsn <url>` is forwarded as `DATABASE_URL`, overriding the
//! app's configured database for this run only.

use std::ffi::OsString;
use std::process::Command;

use super::super::parser::{Subcommand, SubcommandError};

/// Parse `arc schedule` arguments into a [`Subcommand::Schedule`].
pub fn parse<'a>(
    iter: &mut std::slice::Iter<'a, OsString>,
) -> Result<Subcommand, SubcommandError> {
    let mut dsn = None;
    while let Some(arg) = iter.next() {
        let arg_str = arg.to_string_lossy();
        if arg_str == "--dsn" {
            let value = iter.next().ok_or(SubcommandError::MissingFlagValue {
                subcommand: "schedule".into(),
                flag: "--dsn".into(),
            })?;
            dsn = Some(value.to_string_lossy().into_owned());
        }
    }
    Ok(Subcommand::Schedule { dsn })
}

/// Execute the `schedule` subcommand: run the app's scheduler via its binary.
pub fn run(dsn: Option<&str>) -> Result<(), ScheduleError> {
    let mut cmd = Command::new("cargo");
    cmd.args(["run", "--", "--schedule"]);

    if let Some(url) = dsn {
        cmd.env("DATABASE_URL", url);
    }

    let status = cmd.status().map_err(|source| ScheduleError::Spawn { source })?;
    if !status.success() {
        return Err(ScheduleError::Exited {
            code: status.code(),
        });
    }
    Ok(())
}

/// An error from the `schedule` command.
#[derive(Debug)]
pub enum ScheduleError {
    /// `cargo` could not be spawned.
    Spawn { source: std::io::Error },
    /// The scheduler run exited with a non-zero status.
    Exited { code: Option<i32> },
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { source } => write!(f, "failed to spawn cargo: {source}"),
            Self::Exited { code } => match code {
                Some(c) => write!(f, "scheduler exited with status {c}"),
                None => write!(f, "scheduler exited without a status"),
            },
        }
    }
}

impl std::error::Error for ScheduleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { source } => Some(source),
            _ => None,
        }
    }
}
