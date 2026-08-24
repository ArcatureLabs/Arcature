//! `arc new <name> [--dest <path>] [--stack <s>] [--db <d>]` -- generate a new
//! application.
//!
//! Parsing lives in [`super::super::parser`]; this file holds the executor and
//! the command's error.
//!
//! # Why the flags are translated rather than passed through
//!
//! [`crate::templates`] declares its own `Stack` and `Database` so the
//! `templates` feature stands on its own without `cli`. The two enums are
//! deliberately separate types, and this file is the single place that maps
//! one onto the other -- an exhaustive `match` here means adding a stack to
//! the CLI without adding it to the catalog fails to compile.

use std::path::PathBuf;

use crate::cli::parser::{Database, Stack};
use crate::templates::{Database as TemplateDatabase, Stack as TemplateStack};

/// Execute the `new` subcommand: generate the application and report the path.
///
/// # Errors
///
/// See [`NewError`].
pub fn run(
    name: &str,
    dest: Option<PathBuf>,
    stack: Stack,
    database: Database,
    install: bool,
) -> Result<(), NewError> {
    let target = dest.unwrap_or_else(|| PathBuf::from(name));
    crate::templates::generate(&target, template_stack(stack), template_database(database))
        .map_err(NewError::Template)?;
    println!("Created {name} at {}", target.display());

    let keyed = generate_key(&target);
    let installed = install && install_frontend(&target);
    next_steps(name, installed, keyed);
    Ok(())
}

/// Mint `APP_KEY` into the generated `.env`.
///
/// Returns whether the project has a key.
///
/// The scaffold ships `APP_KEY=` empty and its own comment says the
/// application refuses to boot without it, which made "run `arc key:generate`"
/// a step between `arc new` and anything working at all. There is nothing for
/// a developer to decide here -- the key is 64 bytes from the OS RNG and any
/// value they chose themselves would be worse -- so the generator mints it.
///
/// [`super::key_generate::generate`] rather than a second RNG: that module
/// owns the certified source and the `.env` upsert, and two ways to produce
/// the same secret would mean only one of them was reviewed.
#[cfg(feature = "auth")]
fn generate_key(target: &std::path::Path) -> bool {
    match super::key_generate::generate(false, target) {
        Ok(_) => true,
        Err(error) => {
            println!("note: APP_KEY was not written: {error}");
            false
        }
    }
}

/// Without `auth` there is no certified RNG to mint a key with, so the
/// developer is told to run the command from a CLI that has one.
#[cfg(not(feature = "auth"))]
fn generate_key(_target: &std::path::Path) -> bool {
    false
}

/// Install the frontend, reporting rather than failing.
///
/// Returns whether the project is ready to `arc dev`.
///
/// A failure here is deliberately not a [`NewError`]. The application is
/// already written and correct; npm being absent, offline, or behind a proxy
/// that rejects the request says nothing about the files on disk. Turning
/// that into a failed `arc new` would leave a complete project behind an
/// error message, and the developer's next move -- delete it and try again --
/// is exactly the wrong one.
fn install_frontend(target: &std::path::Path) -> bool {
    println!("  installing frontend dependencies (npm install)...");
    match super::install::install(target, false) {
        Ok(_) => true,
        Err(error) => {
            println!();
            println!("note: the frontend was not installed: {error}");
            println!("      the application is written and complete; run `arc install`");
            println!("      inside it once npm can reach the registry.");
            false
        }
    }
}

/// What to do next, in the order it has to be done.
///
/// `0.1.2` printed the created path and stopped. Everything below was true
/// then too and was written down only in the generated `justfile`, which is a
/// file most people never open -- so the first thing a new project did was
/// fail, and it failed inside Node with a module-resolution trace.
fn next_steps(name: &str, installed: bool, keyed: bool) {
    println!();
    println!("Next:");
    println!("  cd {name}");
    if !installed {
        println!("  arc install          # the frontend's npm dependencies");
    }
    if !keyed {
        println!("  arc key:generate     # writes APP_KEY into .env");
    }
    println!("  arc dev              # one port, backend and Vite together");
    if installed && keyed {
        println!();
        println!("Nothing else is needed: the default database is SQLite, created on");
        println!("first connect, and `arc dev` applies migrations before it serves.");
    }
}

/// Map the CLI's stack onto the catalog's.
const fn template_stack(stack: Stack) -> TemplateStack {
    match stack {
        Stack::React => TemplateStack::React,
        Stack::Vue => TemplateStack::Vue,
        Stack::Svelte => TemplateStack::Svelte,
    }
}

/// Map the CLI's driver onto the catalog's.
const fn template_database(database: Database) -> TemplateDatabase {
    match database {
        Database::Postgres => TemplateDatabase::Postgres,
        Database::Sqlite => TemplateDatabase::Sqlite,
        Database::Mysql => TemplateDatabase::Mysql,
    }
}

/// An error from the `new` command.
#[derive(Debug)]
pub enum NewError {
    /// Template generation failed.
    Template(crate::templates::TemplateError),
}

impl std::fmt::Display for NewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Template(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for NewError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Template(error) => Some(error),
        }
    }
}

impl From<crate::templates::TemplateError> for NewError {
    fn from(error: crate::templates::TemplateError) -> Self {
        Self::Template(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stack_flag_reaches_a_catalog_stack() {
        let pairs = [
            (Stack::React, TemplateStack::React),
            (Stack::Vue, TemplateStack::Vue),
            (Stack::Svelte, TemplateStack::Svelte),
        ];
        for (cli, catalog) in pairs {
            assert_eq!(template_stack(cli), catalog);
            assert_eq!(cli.as_str(), catalog.as_str());
        }
    }

    #[test]
    fn every_driver_flag_reaches_a_catalog_driver() {
        let pairs = [
            (Database::Postgres, TemplateDatabase::Postgres),
            (Database::Sqlite, TemplateDatabase::Sqlite),
            (Database::Mysql, TemplateDatabase::Mysql),
        ];
        for (cli, catalog) in pairs {
            assert_eq!(template_database(cli), catalog);
            assert_eq!(cli.as_str(), catalog.as_str());
            assert_eq!(cli.feature(), catalog.feature());
        }
    }

    #[test]
    fn a_non_default_stack_and_driver_now_generate_rather_than_refuse() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("demo");
        run(
            "demo",
            Some(target.clone()),
            Stack::Svelte,
            Database::Sqlite,
            // No npm from a unit test: it needs a network and takes seconds.
            false,
        )
        .expect("generated");
        assert!(target.join("resources/js/app.ts").exists());
        let manifest = std::fs::read_to_string(target.join("Cargo.toml")).expect("Cargo.toml");
        assert!(manifest.contains("db-sqlite"), "{manifest}");
    }

    #[test]
    fn generating_over_an_existing_directory_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("demo");
        std::fs::create_dir_all(&target).expect("mkdir");
        let error = run(
            "demo",
            Some(target),
            Stack::default(),
            Database::default(),
            false,
        )
        .expect_err("refused");
        assert!(matches!(
            error,
            NewError::Template(crate::templates::TemplateError::ExistingTarget { .. })
        ));
    }
}
