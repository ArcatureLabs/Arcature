//! The canonical application templates and generator.
//!
//! Powers `arc new <name>`. Templates are embedded with `include_str!` and
//! rendered by substituting a small set of `__PLACEHOLDER__` tokens. The
//! generator is atomic: it stages the new project in a hidden directory and
//! renames it into place only when every file is written.

mod catalog;
mod error;
mod name;
mod render;

use std::path::Path;

pub use catalog::{Database, Stack, TemplateFile};
pub use error::TemplateError;
pub use name::ProjectName;

use catalog::files;
use render::render;

/// The certified Arcature version embedded in generated apps.
const ARCATURE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Generate a new application at `target`, scaffolded for `stack` and
/// `database`.
///
/// Atomic: stages in a hidden directory, then renames into place. The
/// destination must not already exist.
///
/// # Errors
///
/// See [`TemplateError`].
pub fn generate(target: &Path, stack: Stack, database: Database) -> Result<(), TemplateError> {
    if target.exists() {
        return Err(TemplateError::ExistingTarget {
            path: target.to_path_buf(),
        });
    }

    let name = ProjectName::parse(target.file_name().and_then(|n| n.to_str()).ok_or(
        TemplateError::InvalidDestination {
            reason: "destination has no final path component".into(),
        },
    )?)?;

    let parent = target.parent().ok_or(TemplateError::InvalidDestination {
        reason: "destination has no parent directory".into(),
    })?;
    if !parent.exists() {
        std::fs::create_dir_all(parent).map_err(|e| TemplateError::Io {
            path: parent.into(),
            source: e,
        })?;
    }

    // Stage in a hidden directory next to the target.
    let staging_name = format!(".{}.arcature-staging", name.rust_identifier());
    let staging = parent.join(&staging_name);
    if staging.exists() {
        return Err(TemplateError::InvalidDestination {
            reason: format!("staging directory already exists: {}", staging.display()),
        });
    }

    // Write all files to the staging directory.
    for file in files(stack, database) {
        let rendered = render(file.content, &name, ARCATURE_VERSION, stack, database);
        let dest = staging.join(file.path);
        let dir = dest.parent().ok_or(TemplateError::InvalidDestination {
            reason: "file has no parent directory".into(),
        })?;
        std::fs::create_dir_all(dir).map_err(|e| TemplateError::Io {
            path: dir.into(),
            source: e,
        })?;
        std::fs::write(&dest, &rendered).map_err(|e| TemplateError::Io {
            path: dest.clone(),
            source: e,
        })?;
    }

    // Atomic rename into place.
    std::fs::rename(&staging, target).map_err(|e| {
        // Best-effort cleanup of the staging dir on rename failure.
        let _ = std::fs::remove_dir_all(&staging);
        TemplateError::Rename {
            staging: staging.clone(),
            target: target.to_path_buf(),
            source: e,
        }
    })?;

    Ok(())
}
