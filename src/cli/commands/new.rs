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
) -> Result<(), NewError> {
    let target = dest.unwrap_or_else(|| PathBuf::from(name));
    crate::templates::generate(&target, template_stack(stack), template_database(database))
        .map_err(NewError::Template)?;
    println!("Created {name} at {}", target.display());
    Ok(())
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
        let error =
            run("demo", Some(target), Stack::default(), Database::default()).expect_err("refused");
        assert!(matches!(
            error,
            NewError::Template(crate::templates::TemplateError::ExistingTarget { .. })
        ));
    }
}
