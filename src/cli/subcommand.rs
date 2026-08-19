//! Hand-rolled subcommand parser (no clap dependency on the runtime path).

use std::ffi::OsString;

/// A parsed CLI subcommand.
#[derive(Debug, Clone)]
pub enum Subcommand {
    /// `arc new <name> [--dest <path>]` — generate a new application.
    New {
        name: String,
        dest: Option<std::path::PathBuf>,
    },
    /// `arc version` — print the framework version.
    Version,
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
}

impl std::fmt::Display for SubcommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("no subcommand given"),
            Self::Unknown { name } => write!(f, "unknown subcommand: {name}"),
            Self::MissingArg { subcommand, arg } => {
                write!(f, "{subcommand} requires a {arg} argument")
            }
        }
    }
}

/// Parse the CLI arguments into a [`Subcommand`].
pub fn parse(args: &[OsString]) -> Result<Subcommand, SubcommandError> {
    let mut iter = args.iter().skip(1); // skip program name
    let first = iter.next().ok_or(SubcommandError::Missing)?;
    let name = first.to_string_lossy().into_owned();

    match name.as_str() {
        "new" => {
            let project_name = iter
                .next()
                .ok_or(SubcommandError::MissingArg {
                    subcommand: "new".into(),
                    arg: "<name>".into(),
                })?
                .to_string_lossy()
                .into_owned();

            // Parse optional --dest <path>.
            let mut dest = None;
            let mut rest = iter.clone();
            while let Some(arg) = rest.next() {
                let arg_str = arg.to_string_lossy();
                if arg_str == "--dest" {
                    let value = rest.next().ok_or(SubcommandError::MissingArg {
                        subcommand: "new".into(),
                        arg: "--dest <path>".into(),
                    })?;
                    dest = Some(std::path::PathBuf::from(
                        value.to_string_lossy().into_owned(),
                    ));
                }
            }

            Ok(Subcommand::New {
                name: project_name,
                dest,
            })
        }
        "version" | "--version" | "-V" => Ok(Subcommand::Version),
        other => Err(SubcommandError::Unknown {
            name: other.to_owned(),
        }),
    }
}
