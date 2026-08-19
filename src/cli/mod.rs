//! The `arc` CLI entry point.
//!
//! The `arc` binary ships from the same `arcature` package. A normal
//! application never compiles the CLI; it is built by
//! `cargo install arcature` or `cargo build --bin arc`.
//!
//! Subcommands:
//! - `arc new <name>` — generate a new application.
//! - `arc version` — print the framework version.

mod subcommand;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

pub use subcommand::{parse, Subcommand, SubcommandError};

use crate::templates;

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
            eprintln!("usage: arc <new <name> | version>");
            ExitCode::FAILURE
        }
    }
}

fn execute(cmd: Subcommand) -> Result<(), CliError> {
    match cmd {
        Subcommand::New { name, dest } => {
            let target = dest.unwrap_or_else(|| PathBuf::from(&name));
            templates::generate(&target).map_err(CliError::from)?;
            println!("Created {} at {}", name, target.display());
            Ok(())
        }
        Subcommand::Version => {
            println!("arcature {}", crate::FRAMEWORK_VERSION);
            Ok(())
        }
    }
}

/// An error from the CLI.
#[derive(Debug)]
enum CliError {
    Template(templates::TemplateError),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Template(e) => write!(f, "{e}"),
        }
    }
}

impl From<templates::TemplateError> for CliError {
    fn from(e: templates::TemplateError) -> Self {
        Self::Template(e)
    }
}
