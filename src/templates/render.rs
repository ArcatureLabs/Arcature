//! Placeholder substitution.
//!
//! A template file is plain text with `__TOKEN__` markers. Substitution is a
//! sequence of string replacements rather than a template engine: the token
//! set is closed, every token is a single word, and a generated project has
//! to be readable as the file the developer will edit next.

use super::catalog::{Database, Stack};
use super::name::ProjectName;

/// Render a template by substituting the placeholder tokens.
///
/// `__DATABASE_URL__` is expanded before `__RUST_NAME__` because the default
/// connection string names the database after the crate.
#[must_use]
pub fn render(
    content: &str,
    name: &ProjectName,
    arcature_version: &str,
    stack: Stack,
    database: Database,
) -> String {
    let rust_name = name.rust_identifier();
    let project_name = name.raw();
    content
        .replace("__DATABASE_URL__", database.default_url())
        .replace("__RUST_NAME__", &rust_name)
        .replace("__PROJECT_NAME__", project_name)
        .replace("__ARCATURE_VERSION__", arcature_version)
        .replace("__STACK__", stack.as_str())
        .replace("__DB_DRIVER__", database.feature())
        .replace("__JS_ENTRY__", stack.entry())
}
