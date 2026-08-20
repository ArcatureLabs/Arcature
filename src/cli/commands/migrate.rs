//! `arc migrate [--dsn <url>]` — run the application's pending migrations.
//!
//! Arcature does not own a second migration engine: the application defines
//! its migrations as a `MigratorTrait` impl in `database/migrations`. This
//! command runs `cargo run -- --migrate` in the current directory so the app's
//! own binary applies them (the generated `main.rs` honors `--migrate`).
//!
//! An optional `--dsn <url>` is forwarded as `DATABASE_URL`, overriding the
//! app's configured database for this run only.

use std::ffi::OsString;
use std::process::Command;

use super::super::parser::{Subcommand, SubcommandError};

/// Parse `arc migrate` arguments into a [`Subcommand::Migrate`].
pub fn parse<'a>(iter: &mut std::slice::Iter<'a, OsString>) -> Result<Subcommand, SubcommandError> {
    let mut dsn = None;
    while let Some(arg) = iter.next() {
        let arg_str = arg.to_string_lossy();
        if arg_str == "--dsn" {
            let value = iter.next().ok_or(SubcommandError::MissingFlagValue {
                subcommand: "migrate".into(),
                flag: "--dsn".into(),
            })?;
            dsn = Some(value.to_string_lossy().into_owned());
        }
    }
    Ok(Subcommand::Migrate { dsn })
}

/// Execute the `migrate` subcommand: run the app's migrations via its binary.
pub fn run(dsn: Option<&str>) -> Result<(), MigrateError> {
    let mut cmd = Command::new("cargo");
    cmd.args(["run", "--", "--migrate"]);

    if let Some(url) = dsn {
        cmd.env("DATABASE_URL", url);
    }

    let status = cmd
        .status()
        .map_err(|source| MigrateError::Spawn { source })?;
    if !status.success() {
        return Err(MigrateError::Exited {
            code: status.code(),
        });
    }
    Ok(())
}

/// An error from the `migrate` command.
#[derive(Debug)]
pub enum MigrateError {
    /// `cargo` could not be spawned.
    Spawn { source: std::io::Error },
    /// The migration run exited with a non-zero status.
    Exited { code: Option<i32> },
}

impl std::fmt::Display for MigrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { source } => write!(f, "failed to spawn cargo: {source}"),
            Self::Exited { code } => match code {
                Some(c) => write!(f, "migration exited with status {c}"),
                None => write!(f, "migration exited without a status"),
            },
        }
    }
}

impl std::error::Error for MigrateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { source } => Some(source),
            _ => None,
        }
    }
}
