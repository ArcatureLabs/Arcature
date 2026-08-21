//! `arc typegen` -- the generated TypeScript, written to disk.
//!
//! Four files land in `resources/js/generated/`: `routes.ts` (the typed
//! route table and `route()`), `pages.d.ts` (per-page prop types),
//! `forms.ts` (field names and validation rules) and `index.ts`, the barrel
//! the scaffold's `@/generated` alias resolves to. The directory is
//! gitignored by the scaffold, because it is derived: every byte of it comes
//! out of the Rust source, and a checked-in copy is a second source of truth
//! that will be wrong the first time someone forgets to re-run this.
//!
//! There is no npm package, no Vite plugin and no virtual module. The output
//! imports nothing, so an editor, `tsc` and Vite all understand it with no
//! setup at all -- see [`crate::uag::codegen`].
//!
//! # Validation comes first, and it is a refusal
//!
//! [`crate::uag::validate`] runs before anything is written, and a single
//! diagnostic stops the run with nothing written at all. The reason is what
//! the generated files are *for*: they are the compiler's picture of the
//! backend. Emitting them from a graph with a duplicate route or a page
//! nobody registered would compile on the frontend and fail in the browser,
//! which is the exact failure the whole artifact exists to prevent. Half a
//! generation is worse still, so the emission is all-or-nothing.
//!
//! The page-component check is included when `resources/js/pages` exists and
//! skipped when it does not: a backend-only checkout has no components to
//! find, and reporting all of them as missing would make the command useless
//! there.

use std::path::{Path, PathBuf};

use crate::uag::codegen::{forms_ts, index_ts, pages_dts, routes_ts};
use crate::uag::{UagArtifact, UagDiagnostic, ValidateOptions};

use super::Cause;
use super::uag_source;

/// The directory the generated TypeScript is written to, relative to the
/// project root.
pub const OUTPUT_DIR: &str = "resources/js/generated";

/// Where page identities resolve to component files, relative to the project
/// root.
const PAGES_DIR: &str = "resources/js/pages";

/// Run `arc typegen` in the current directory.
///
/// # Errors
///
/// [`TypegenError`] when the graph cannot be obtained, when validation found
/// anything, or when a file cannot be written.
pub fn run() -> Result<(), TypegenError> {
    let cwd = std::env::current_dir().map_err(|source| TypegenError::Cwd { source })?;
    let loaded = uag_source::load(&cwd).map_err(|source| TypegenError::Source {
        source: Box::new(source),
    })?;

    let written = emit(&loaded.artifact, &loaded.root)?;
    println!(
        "Wrote {} file{} to {OUTPUT_DIR}/ from {}.",
        written.len(),
        if written.len() == 1 { "" } else { "s" },
        loaded.source
    );
    Ok(())
}

/// Validate `artifact` and write the generated files under `root`.
///
/// Returns the paths written, in emission order. Separated from [`run`] so
/// `arc build` can reuse it, and so the refusal is testable without a
/// project on disk.
///
/// # Errors
///
/// [`TypegenError::Invalid`] with every diagnostic found, before anything is
/// written; [`TypegenError::Write`] if a file or the directory cannot be
/// created.
pub fn emit(artifact: &UagArtifact, root: &Path) -> Result<Vec<PathBuf>, TypegenError> {
    let mut options = ValidateOptions::new();
    let pages = root.join(PAGES_DIR);
    if pages.is_dir() {
        options = options.with_pages_dir(pages);
    }
    if let Err(diagnostics) = crate::uag::validate(artifact, &options) {
        return Err(TypegenError::Invalid { diagnostics });
    }

    let directory = root.join(OUTPUT_DIR);
    std::fs::create_dir_all(&directory).map_err(|source| TypegenError::Write {
        path: directory.clone(),
        source,
    })?;

    let files = [
        ("routes.ts", routes_ts::generate(artifact)),
        ("pages.d.ts", pages_dts::generate(artifact)),
        ("forms.ts", forms_ts::generate(artifact)),
        ("index.ts", index_ts::generate()),
    ];

    let mut written = Vec::with_capacity(files.len());
    for (name, contents) in files {
        let path = directory.join(name);
        // Only when the bytes differ. Vite watches this directory, and
        // rewriting an identical file on every restart would trigger a
        // reload for a file that did not change.
        if std::fs::read_to_string(&path).is_ok_and(|existing| existing == contents) {
            written.push(path);
            continue;
        }
        std::fs::write(&path, contents).map_err(|source| TypegenError::Write {
            path: path.clone(),
            source,
        })?;
        written.push(path);
    }
    Ok(written)
}

/// Render diagnostics the way the command prints them.
///
/// One per line, prefixed so they line up under the heading. Public to the
/// crate because `arc build` reports the same failure at its first stage and
/// must not word it differently.
pub(crate) fn render_diagnostics(diagnostics: &[UagDiagnostic]) -> String {
    let mut out = format!(
        "the application graph has {} problem{}, so nothing was generated:\n",
        diagnostics.len(),
        if diagnostics.len() == 1 { "" } else { "s" }
    );
    for diagnostic in diagnostics {
        out.push_str(&format!("  - {diagnostic}\n"));
    }
    out.push_str(
        "Generated TypeScript is the frontend's picture of the backend; emitting it \
         from a graph with these in it would move the failure to the browser.",
    );
    out
}

/// A failure generating TypeScript.
#[derive(Debug)]
pub enum TypegenError {
    /// The working directory could not be read.
    Cwd {
        /// The underlying failure.
        source: std::io::Error,
    },
    /// The application graph could not be obtained.
    Source {
        /// Why. Boxed to keep this enum small.
        source: Cause,
    },
    /// Validation found problems. Nothing was written.
    Invalid {
        /// Every diagnostic, in the validator's stable order.
        diagnostics: Vec<UagDiagnostic>,
    },
    /// A generated file could not be written.
    Write {
        /// The path being written.
        path: PathBuf,
        /// The io failure.
        source: std::io::Error,
    },
}

impl std::fmt::Display for TypegenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cwd { source } => {
                write!(formatter, "could not read the working directory: {source}")
            }
            Self::Source { source } => write!(formatter, "{source}"),
            Self::Invalid { diagnostics } => formatter.write_str(&render_diagnostics(diagnostics)),
            Self::Write { path, source } => {
                write!(formatter, "could not write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for TypegenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Cwd { source } | Self::Write { source, .. } => Some(source),
            Self::Source { source } => Some(source.as_ref()),
            Self::Invalid { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    use crate::uag::UagRoute;

    fn route(method: &str, path: &str, name: &str, handler: &str) -> UagRoute {
        UagRoute {
            module: String::from("Web"),
            method: method.to_owned(),
            path: path.to_owned(),
            name: name.to_owned(),
            handler: handler.to_owned(),
            params: Vec::new(),
            pages: BTreeSet::new(),
            action: None,
            query: None,
            query_string: None,
            policies: BTreeSet::new(),
        }
    }

    fn artifact(routes: Vec<UagRoute>) -> UagArtifact {
        UagArtifact::new(BTreeMap::new(), routes, BTreeMap::new())
    }

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "arcature-typegen-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir should be creatable");
        dir
    }

    #[test]
    fn a_valid_graph_writes_the_four_files() {
        let dir = scratch("valid");
        let written = emit(
            &artifact(vec![route("GET", "/", "home", "HomeController::index")]),
            &dir,
        )
        .expect("a graph with one route is valid");

        assert_eq!(written.len(), 4);
        for name in ["routes.ts", "pages.d.ts", "forms.ts", "index.ts"] {
            assert!(
                dir.join(OUTPUT_DIR).join(name).is_file(),
                "{name} should have been written"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_diagnostic_refuses_the_run_and_writes_nothing() {
        let dir = scratch("invalid");
        let error = emit(
            &artifact(vec![
                route("GET", "/links", "links.index", "A::index"),
                route("GET", "/links", "links.list", "B::index"),
            ]),
            &dir,
        )
        .expect_err("the same method and path twice is a duplicate route");

        let TypegenError::Invalid { diagnostics } = &error else {
            panic!("expected a validation refusal, got {error:?}");
        };
        assert_eq!(diagnostics.len(), 1);
        assert!(
            !dir.join(OUTPUT_DIR).exists(),
            "a refused run must not leave a half-written directory behind"
        );

        let message = error.to_string();
        assert!(message.contains("duplicate route"), "{message}");
        assert!(message.contains("nothing was generated"), "{message}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn re_emitting_an_unchanged_graph_does_not_rewrite_the_files() {
        let dir = scratch("unchanged");
        let art = artifact(vec![route("GET", "/", "home", "HomeController::index")]);
        emit(&art, &dir).expect("first run");
        let path = dir.join(OUTPUT_DIR).join("routes.ts");
        let first = std::fs::metadata(&path).expect("written").modified().ok();

        std::thread::sleep(std::time::Duration::from_millis(20));
        emit(&art, &dir).expect("second run");
        let second = std::fs::metadata(&path).expect("written").modified().ok();

        // Vite watches this directory: an untouched file is one reload the
        // browser does not have to do.
        assert_eq!(first, second);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
