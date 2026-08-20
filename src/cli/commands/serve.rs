//! `arc serve [--bind <addr>] [--port <n>]` — run the current application.
//!
//! A developer convenience that runs `cargo run` in the current directory so a
//! generated app boots without remembering the full incantation. The bind
//! address and port are forwarded to the app via `ARCATURE_BACKEND_BIND` and
//! `ARCATURE_BACKEND_PORT` (the app honors `PORT` / `ARCATURE_BACKEND_PORT`
//! already; `--bind` sets the address). This command shells out to `cargo`
//! and is intentionally thin: the app owns its own server.

use std::ffi::OsString;
use std::process::Command;

use super::super::parser::{Subcommand, SubcommandError};

/// Parse `arc serve` arguments into a [`Subcommand::Serve`].
pub fn parse<'a>(
    iter: &mut std::slice::Iter<'a, OsString>,
) -> Result<Subcommand, SubcommandError> {
    let mut bind = None;
    let mut port = None;
    while let Some(arg) = iter.next() {
        let arg_str = arg.to_string_lossy();
        match arg_str.as_ref() {
            "--bind" => {
                let value = iter.next().ok_or(SubcommandError::MissingFlagValue {
                    subcommand: "serve".into(),
                    flag: "--bind".into(),
                })?;
                bind = Some(value.to_string_lossy().into_owned());
            }
            "--port" => {
                let value = iter.next().ok_or(SubcommandError::MissingFlagValue {
                    subcommand: "serve".into(),
                    flag: "--port".into(),
                })?;
                let s = value.to_string_lossy().into_owned();
                let p: u16 = s.parse().map_err(|_| SubcommandError::InvalidValue {
                    subcommand: "serve".into(),
                    value: s,
                    reason: "expected a port number 0-65535".into(),
                })?;
                port = Some(p);
            }
            _ => {}
        }
    }
    Ok(Subcommand::Serve { bind, port })
}

/// Execute the `serve` subcommand: forward the bind/port to the app via env
/// and run `cargo run`.
pub fn run(bind: Option<&str>, port: Option<u16>) -> Result<(), ServeError> {
    let mut cmd = Command::new("cargo");
    cmd.arg("run");

    // Forward overrides to the app's environment. The app honors
    // `ARCATURE_BACKEND_PORT`; we add the bind via a dedicated env so the
    // generated app's bootstrap can read it.
    if let Some(p) = port {
        cmd.env("ARCATURE_BACKEND_PORT", p.to_string());
    }
    if let Some(addr) = bind {
        cmd.env("ARCATURE_BACKEND_BIND", addr);
    }

    let status = cmd.status().map_err(|source| ServeError::Spawn { source })?;
    if !status.success() {
        return Err(ServeError::Exited {
            code: status.code(),
        });
    }
    Ok(())
}

/// An error from the `serve` command.
#[derive(Debug)]
pub enum ServeError {
    /// `cargo` could not be spawned.
    Spawn { source: std::io::Error },
    /// `cargo run` exited with a non-zero status.
    Exited { code: Option<i32> },
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { source } => write!(f, "failed to spawn cargo: {source}"),
            Self::Exited { code } => match code {
                Some(c) => write!(f, "cargo run exited with status {c}"),
                None => write!(f, "cargo run exited without a status"),
            },
        }
    }
}

impl std::error::Error for ServeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { source } => Some(source),
            _ => None,
        }
    }
}
