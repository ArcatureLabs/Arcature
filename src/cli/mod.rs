//! The `arc` CLI entry point.
//!
//! The `arc` binary ships from the same `arcature` package. A normal
//! application never compiles the CLI; it is built by
//! `cargo install arcature` or `cargo build --bin arc`.
//!
//! Dispatch is split in two. [`parser`] describes the whole command surface
//! with clap and resolves an argument list to a [`Subcommand`]; each
//! `commands/<name>.rs` module holds that command's executor. This module owns
//! the top-level [`run`] loop, the exit-code policy, and the shared error type.
//!
//! # The command surface
//!
//! - `arc new <name> [--dest <path>] [--stack <s>] [--db <d>]` — generate an application.
//! - `arc version` (also `--version`, `-V`) — print the framework version.
//! - `arc serve [--bind <addr>] [--port <n>]` — run the current application.
//! - `arc migrate [--dsn <url>]` — run pending migrations.
//! - `arc schedule [--dsn <url>]` — run the job scheduler.
//! - `arc make:<kind> <name>` — generate a source file (seventeen kinds;
//!   `make:module` writes a directory of four).
//! - `arc key:generate [--show]` — mint the application key (`auth`).
//! - `arc storage:link` — link `storage/app/public` into `public/storage`.
//! - `arc db:seed | db:fresh | db:reset [--dsn <url>] [--force]` — database lifecycle.
//! - `arc queue <work|drain|stats> [--dsn <url>]` — operate on the job queue
//!   (`database` + `jobs`).
//! - `arc doctor` — diagnose the local environment (`database`).
//! - `arc dev [--port <n>] [--host <addr>] [--open]` — the one-port
//!   development supervisor.
//! - `arc routes [--json]` — every declared route, as a table or as JSON
//!   (`uag`).
//! - `arc typegen` — emit `resources/js/generated/` from the application
//!   graph (`uag`).
//! - `arc build` — the production build: graph, typegen, cargo, Vite
//!   (`uag`).
//!
//! The three application-graph commands read the UAG artifact, so they only
//! exist in a build with the `uag` feature -- the same rule `queue`, `doctor`
//! and `key:generate` follow. A build without it does not show them in
//! `--help` and does not accept them.

pub mod commands;
pub(crate) mod parser;

pub use parser::{Database, DbAction, MakeKind, Stack, Subcommand, parse};

#[cfg(all(feature = "database", feature = "jobs"))]
pub use parser::QueueAction;

use std::ffi::OsString;
use std::process::ExitCode;

/// Run the CLI with the given arguments. Returns an exit code.
///
/// Parse failures are clap's to report: it already knows how to print a usage
/// line, suggest a near-miss subcommand, and — for `--help` and `--version` —
/// render to stdout and succeed. [`clap::Error::use_stderr`] is the signal
/// that distinguishes the two, so this function asks rather than guessing.
#[must_use]
pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let args: Vec<OsString> = args.into_iter().collect();
    match parse(&args) {
        Ok(cmd) => match execute(cmd) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("arc: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            let failed = error.use_stderr();
            // `print` writes to whichever stream clap chose, so a help render
            // does not end up interleaved on stderr.
            let _ = error.print();
            if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
    }
}

/// Dispatch a parsed subcommand to its executor.
fn execute(cmd: Subcommand) -> Result<(), CliError> {
    match cmd {
        Subcommand::New {
            name,
            dest,
            stack,
            database,
        } => commands::new::run(&name, dest, stack, database).map_err(CliError::from),
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
        Subcommand::Schedule { dsn } => {
            commands::schedule::run(dsn.as_deref()).map_err(CliError::from)
        }
        Subcommand::Make { kind, name } => {
            // `run_all` rather than `run`: `make:module` writes four files, and
            // a command that names one of them is a command whose output the
            // reader has to distrust for every other kind too.
            for generated in commands::make::run_all(kind, &name).map_err(CliError::from)? {
                generated.report();
            }
            Ok(())
        }
        #[cfg(feature = "auth")]
        Subcommand::KeyGenerate { show } => {
            commands::key_generate::run(show).map_err(CliError::from)
        }
        Subcommand::StorageLink => commands::storage_link::run().map_err(CliError::from),
        Subcommand::Db { action, dsn, force } => {
            commands::db::run(action, dsn.as_deref(), force).map_err(CliError::from)
        }
        #[cfg(all(feature = "database", feature = "jobs"))]
        Subcommand::Queue { action, dsn } => {
            // The queue command is async; run it on a current-thread runtime
            // (the CLI is a short-lived tool).
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| CliError::Runtime(error.to_string()))?;
            runtime
                .block_on(commands::queue::run(&action, dsn.as_deref()))
                .map_err(CliError::from)
        }
        #[cfg(feature = "database")]
        Subcommand::Doctor => commands::doctor::run().map_err(CliError::from),
        Subcommand::Dev { port, host, open } => {
            let options = commands::dev::options(port, host.as_deref(), open)
                .map_err(|error| CliError::Command(error.to_string()))?;
            // The supervisor drives two child processes, a file watcher and a
            // listener at once, so it needs real threads: a current-thread
            // runtime would make a blocking build stall the listener, which is
            // the one thing `arc dev` exists to prevent.
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|error| CliError::Runtime(error.to_string()))?;
            runtime
                .block_on(commands::dev::run(options))
                .map_err(CliError::from)
        }
        #[cfg(feature = "uag")]
        Subcommand::Routes { json } => commands::routes::run(json).map_err(CliError::from),
        #[cfg(feature = "uag")]
        Subcommand::Typegen => commands::typegen::run().map_err(CliError::from),
        #[cfg(feature = "uag")]
        Subcommand::Build => commands::build::run().map_err(CliError::from),
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
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command(message) => formatter.write_str(message),
            Self::Runtime(message) => write!(formatter, "failed to build runtime: {message}"),
        }
    }
}

impl std::error::Error for CliError {}

/// Each command's error type converts to the dispatcher's error so `execute`
/// stays a flat match. The conversions live here, next to the dispatcher, so a
/// command module never has to know about the other commands.
///
/// The conversion flattens to a string rather than boxing: `CliError` is only
/// ever printed and turned into an exit code, and a boxed source chain nobody
/// walks is a type that promises more than it delivers.
macro_rules! command_error {
    ($($(#[$gate:meta])* $path:path),* $(,)?) => {
        $(
            $(#[$gate])*
            impl From<$path> for CliError {
                fn from(error: $path) -> Self {
                    Self::Command(error.to_string())
                }
            }
        )*
    };
}

command_error! {
    commands::dev::DevError,
    commands::new::NewError,
    commands::serve::ServeError,
    commands::migrate::MigrateError,
    commands::schedule::ScheduleError,
    commands::make::MakeError,
    commands::storage_link::StorageLinkError,
    commands::db::DbError,
    #[cfg(feature = "uag")]
    commands::routes::RoutesError,
    #[cfg(feature = "uag")]
    commands::typegen::TypegenError,
    #[cfg(feature = "uag")]
    commands::build::BuildError,
    #[cfg(feature = "auth")]
    commands::key_generate::KeyGenerateError,
    #[cfg(all(feature = "database", feature = "jobs"))]
    commands::queue::QueueError,
    #[cfg(feature = "database")]
    commands::doctor::DoctorError,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<OsString> {
        std::iter::once("arc")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn help_exits_successfully_because_it_is_not_a_failure() {
        assert_eq!(run(argv(&["--help"])), ExitCode::SUCCESS);
    }

    #[test]
    fn an_unknown_subcommand_exits_with_a_failure_code() {
        assert_eq!(run(argv(&["migrat"])), ExitCode::FAILURE);
    }

    #[test]
    fn a_bare_invocation_shows_help_and_fails() {
        // `arg_required_else_help` makes this a `DisplayHelpOnMissing*` error,
        // which clap does route to stderr: the user asked for nothing, so a
        // shell script that reaches this state should notice.
        assert_eq!(run(argv(&[])), ExitCode::FAILURE);
    }

    #[cfg(feature = "uag")]
    #[test]
    fn typegen_outside_a_project_exits_with_a_failure_code() {
        // Run from a directory with no crate above it: the command has to
        // fail with a reason rather than reaching for a graph that is not
        // there. The temp directory is not cleaned up by this test because
        // nothing is written to it.
        let previous = std::env::current_dir().expect("a working directory");
        let empty = std::env::temp_dir().join(format!("arcature-cli-{}", std::process::id()));
        std::fs::create_dir_all(&empty).expect("temp dir");
        std::env::set_current_dir(&empty).expect("chdir");
        let code = run(argv(&["typegen"]));
        std::env::set_current_dir(previous).expect("chdir back");
        assert_eq!(code, ExitCode::FAILURE);
    }

    #[test]
    fn a_destructive_db_command_without_force_exits_with_a_failure_code() {
        assert_eq!(run(argv(&["db:fresh"])), ExitCode::FAILURE);
    }
}
