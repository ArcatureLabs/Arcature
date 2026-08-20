//! The `arc` CLI entry point.
//!
//! The `arc` binary ships from the same `arcature` package. A normal
//! application never compiles the CLI; it is built by
//! `cargo install arcature` or `cargo build --bin arc`.
//!
//! Dispatch is split: [`parser::parse`] resolves *which* subcommand was
//! invoked, and each `commands/<name>.rs` module holds that command's parser
//! and executor. This module owns the top-level `run` loop and the shared
//! error type.
//!
//! Subcommands:
//! - `arc new <name>` — generate a new application.
//! - `arc version` — print the framework version.
//! - `arc serve` — run the current application.
//! - `arc migrate` — run pending migrations.
//! - `arc queue <work|drain|stats>` — operate on the job queue.
//! - `arc schedule` — run the job scheduler.
//! - `arc doctor` — diagnose the local environment.

pub mod commands;
mod parser;

pub use parser::{Subcommand, SubcommandError, parse};

use std::ffi::OsString;
use std::process::ExitCode;

/// Run the CLI with the given arguments. Returns an exit code.
pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let args: Vec<OsString> = args.into_iter().collect();
    match parse(&args) {
        Ok(cmd) => match execute(cmd) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("arc: {e}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("arc: {e}");
            eprintln!(
                "usage: arc <new <name> | version | serve | migrate | queue <work|drain|stats> | schedule | doctor>"
            );
            ExitCode::FAILURE
        }
    }
}

/// Dispatch a parsed subcommand to its executor.
fn execute(cmd: Subcommand) -> Result<(), CliError> {
    match cmd {
        Subcommand::New { name, dest } => {
            commands::new::run(&name, dest).map_err(CliError::from)
        }
        Subcommand::Version => {
            commands::version::run();
            Ok(())
        }
        Subcommand::Serve { bind, port } => {
            commands::serve::run(bind.as_deref(), port).map_err(CliError::from)
        }
        Subcommand::Migrate { dsn } => {
            commands::migrate::run(dsn.as_deref()).map_err(CliError::from)
        }
        #[cfg(all(feature = "database", feature = "jobs"))]
        Subcommand::Queue { action, dsn } => {
            // The queue command is async; run it on a current-thread runtime
            // (the CLI is a short-lived tool).
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| CliError::Runtime(e.to_string()))?;
            rt.block_on(commands::queue::run(&action, dsn.as_deref()))
                .map_err(CliError::from)
        }
        Subcommand::Schedule { dsn } => {
            commands::schedule::run(dsn.as_deref()).map_err(CliError::from)
        }
        #[cfg(feature = "database")]
        Subcommand::Doctor => commands::doctor::run().map_err(CliError::from),
    }
}

/// An error from the CLI dispatcher.
#[derive(Debug)]
pub enum CliError {
    /// A command failed with its own error.
    Command(String),
    /// The runtime could not be built (for async commands).
    Runtime(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command(s) => f.write_str(s),
            Self::Runtime(s) => write!(f, "failed to build runtime: {s}"),
        }
    }
}

impl std::error::Error for CliError {}

// Each command's error type converts to the dispatcher's error so `execute`
// stays a flat match. The conversions live here, next to the dispatcher, so a
// command module never knows about the other commands.
impl From<commands::new::NewError> for CliError {
    fn from(e: commands::new::NewError) -> Self {
        Self::Command(e.to_string())
    }
}
impl From<commands::serve::ServeError> for CliError {
    fn from(e: commands::serve::ServeError) -> Self {
        Self::Command(e.to_string())
    }
}
impl From<commands::migrate::MigrateError> for CliError {
    fn from(e: commands::migrate::MigrateError) -> Self {
        Self::Command(e.to_string())
    }
}
#[cfg(all(feature = "database", feature = "jobs"))]
impl From<commands::queue::QueueError> for CliError {
    fn from(e: commands::queue::QueueError) -> Self {
        Self::Command(e.to_string())
    }
}
impl From<commands::schedule::ScheduleError> for CliError {
    fn from(e: commands::schedule::ScheduleError) -> Self {
        Self::Command(e.to_string())
    }
}
#[cfg(feature = "database")]
impl From<commands::doctor::DoctorError> for CliError {
    fn from(e: commands::doctor::DoctorError) -> Self {
        Self::Command(e.to_string())
    }
}
