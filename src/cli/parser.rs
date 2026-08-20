//! Hand-rolled subcommand parser (no clap dependency on the runtime path).
//!
//! Each subcommand owns its own argument parsing in its `commands/<name>.rs`
//! module; this module only resolves *which* subcommand was invoked and the
//! shared error vocabulary. Keeping dispatch and per-command parsing apart
//! means each file holds exactly one responsibility.

use std::ffi::OsString;

/// A parsed CLI subcommand.
///
/// The variants mirror the `commands/<name>.rs` modules: each carries the
/// arguments that its command needs to execute. The `queue` and `doctor`
/// variants are gated on the features their commands need.
#[derive(Debug, Clone)]
pub enum Subcommand {
    /// `arc new <name> [--dest <path>]`. Parsed and executed in
    /// [`commands::new`](super::commands::new).
    New {
        name: String,
        dest: Option<std::path::PathBuf>,
    },
    /// `arc version` (also `--version`, `-V`). Executed in
    /// [`commands::version`](super::commands::version).
    Version,
    /// `arc serve [--bind <addr>] [--port <n>]`. Executed in
    /// [`commands::serve`](super::commands::serve).
    Serve {
        bind: Option<String>,
        port: Option<u16>,
    },
    /// `arc migrate [--dsn <url>]`. Executed in
    /// [`commands::migrate`](super::commands::migrate).
    Migrate { dsn: Option<String> },
    /// `arc schedule [--dsn <url>]`. Executed in
    /// [`commands::schedule`](super::commands::schedule).
    Schedule { dsn: Option<String> },
    /// `arc queue [--dsn <url>] <work|drain|stats>`. Executed in
    /// [`commands::queue`](super::commands::queue). Only available with the
    /// `database` + `jobs` features.
    #[cfg(all(feature = "database", feature = "jobs"))]
    Queue {
        action: QueueAction,
        dsn: Option<String>,
    },
    /// `arc doctor`. Executed in [`commands::doctor`](super::commands::doctor).
    /// Only available with the `database` feature.
    #[cfg(feature = "database")]
    Doctor,
}

/// The queue action selected on the command line for `arc queue`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueAction {
    /// Claim and run jobs until Ctrl-C.
    Work,
    /// Requeue dead jobs back to pending.
    Drain,
    /// Print pending / running / dead / cancelled counts.
    Stats,
}

/// An error from parsing a subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubcommandError {
    /// No subcommand was given.
    Missing,
    /// The subcommand is unknown.
    Unknown { name: String },
    /// The subcommand is missing a required argument.
    MissingArg { subcommand: String, arg: String },
    /// A flag was given without its value (e.g. `--port` with nothing after).
    MissingFlagValue { subcommand: String, flag: String },
    /// An argument value was invalid (e.g. a non-numeric port).
    InvalidValue { subcommand: String, value: String, reason: String },
}

impl std::fmt::Display for SubcommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("no subcommand given"),
            Self::Unknown { name } => write!(f, "unknown subcommand: {name}"),
            Self::MissingArg { subcommand, arg } => {
                write!(f, "{subcommand} requires a {arg} argument")
            }
            Self::MissingFlagValue { subcommand, flag } => {
                write!(f, "{subcommand}: flag {flag} requires a value")
            }
            Self::InvalidValue {
                subcommand,
                value,
                reason,
            } => write!(f, "{subcommand}: invalid value {value:?}: {reason}"),
        }
    }
}

impl std::error::Error for SubcommandError {}

/// Parse the CLI arguments into a [`Subcommand`].
///
/// Reads the first positional token to dispatch to the matching command's
/// parser. Each command's parser lives next to its executor so parsing and
/// execution for a command share one file.
pub fn parse(args: &[OsString]) -> Result<Subcommand, SubcommandError> {
    let mut iter = args.iter();
    iter.next(); // skip program name
    let first = iter.next().ok_or(SubcommandError::Missing)?;
    let name = first.to_string_lossy().into_owned();

    match name.as_str() {
        "new" => super::commands::new::parse(&mut iter),
        "version" | "--version" | "-V" => Ok(Subcommand::Version),
        "serve" => super::commands::serve::parse(&mut iter),
        "migrate" => super::commands::migrate::parse(&mut iter),
        #[cfg(all(feature = "database", feature = "jobs"))]
        "queue" => super::commands::queue::parse(&mut iter),
        "schedule" => super::commands::schedule::parse(&mut iter),
        #[cfg(feature = "database")]
        "doctor" => Ok(Subcommand::Doctor),
        other => Err(SubcommandError::Unknown {
            name: other.to_owned(),
        }),
    }
}
