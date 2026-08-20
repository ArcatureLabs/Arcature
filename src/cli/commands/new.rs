//! `arc new <name> [--dest <path>]` — generate a new application.
//!
//! Parses its own arguments and runs the template generator. This file holds
//! only the `new` command: its parser, its executor, and its error.

use std::ffi::OsString;
use std::path::PathBuf;

use super::super::parser::{Subcommand, SubcommandError};

/// Parse `arc new` arguments into a [`Subcommand::New`].
pub fn parse<'a>(
    iter: &mut std::slice::Iter<'a, OsString>,
) -> Result<Subcommand, SubcommandError> {
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
    while let Some(arg) = iter.next() {
        let arg_str = arg.to_string_lossy();
        if arg_str == "--dest" {
            let value = iter.next().ok_or(SubcommandError::MissingFlagValue {
                subcommand: "new".into(),
                flag: "--dest".into(),
            })?;
            dest = Some(PathBuf::from(value.to_string_lossy().into_owned()));
        }
    }

    Ok(Subcommand::New {
        name: project_name,
        dest,
    })
}

/// Execute the `new` subcommand: generate the application and report the path.
pub fn run(name: &str, dest: Option<PathBuf>) -> Result<(), NewError> {
    let target = dest.unwrap_or_else(|| PathBuf::from(name));
    crate::templates::generate(&target).map_err(NewError::Template)?;
    println!("Created {name} at {}", target.display());
    Ok(())
}

/// An error from the `new` command.
#[derive(Debug)]
pub enum NewError {
    /// Template generation failed.
    Template(crate::templates::TemplateError),
}

impl std::fmt::Display for NewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Template(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for NewError {}

impl From<crate::templates::TemplateError> for NewError {
    fn from(e: crate::templates::TemplateError) -> Self {
        Self::Template(e)
    }
}
