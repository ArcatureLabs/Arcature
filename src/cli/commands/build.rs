//! `arc build` -- the production build, in the order the artifacts depend on
//! each other.
//!
//! Four stages, each of which can only start because the one before it
//! succeeded:
//!
//! 1. **graph** -- read the application graph, the same way `arc routes` and
//!    `arc typegen` do.
//! 2. **typegen** -- validate it and write `resources/js/generated/`. This is
//!    before the frontend build on purpose: Vite compiles what this writes,
//!    so generating afterwards would ship a bundle built against the previous
//!    graph.
//! 3. **cargo** -- `cargo build --release`.
//! 4. **vite** -- `npm run build`, which produces `public/build` and the
//!    manifest the release binary reads to resolve asset URLs.
//!
//! # Fail fast, and name the stage
//!
//! Each stage stops the run. A build that carried on past a failed typegen
//! would produce a release binary and a bundle that disagree, which is the
//! one outcome worse than no build at all. Every error carries the stage
//! name, because "the build failed" and "the *frontend* build failed" send a
//! reader to different files.
//!
//! # Why `npm run build` and not `vite build`
//!
//! The script in `package.json` is the project's own definition of what
//! building the frontend means. A project that adds a type-check or an asset
//! step to it should get that step here too, and calling Vite directly would
//! silently skip it -- besides requiring `vite` to be on `PATH`, which after
//! a plain `npm install` it is not.

use std::path::Path;
use std::process::Command;

use super::Cause;
use super::typegen::{self, TypegenError};
use super::uag_source;

/// The stages, in order, for reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Reading the application graph.
    Graph,
    /// Validating it and writing the TypeScript.
    Typegen,
    /// `cargo build --release`.
    Cargo,
    /// `npm run build`.
    Vite,
}

impl Stage {
    /// The stage name as printed.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::Typegen => "typegen",
            Self::Cargo => "cargo",
            Self::Vite => "vite",
        }
    }
}

impl std::fmt::Display for Stage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Run the production build in the current directory.
///
/// # Errors
///
/// [`BuildError`] naming the stage that failed. Nothing is retried and no
/// later stage runs.
pub fn run() -> Result<(), BuildError> {
    let cwd = std::env::current_dir().map_err(|source| BuildError::Cwd { source })?;

    println!("[1/4] graph");
    let loaded = uag_source::load(&cwd).map_err(|source| BuildError::Source {
        source: Box::new(source),
    })?;
    println!("      read from {}", loaded.source);

    println!("[2/4] typegen");
    let written = typegen::emit(&loaded.artifact, &loaded.root)
        .map_err(|source| BuildError::Typegen { source })?;
    println!(
        "      {} file{} in {}/",
        written.len(),
        if written.len() == 1 { "" } else { "s" },
        typegen::OUTPUT_DIR
    );

    println!("[3/4] cargo build --release");
    stage(Stage::Cargo, &loaded.root, "cargo", &["build", "--release"])?;

    println!("[4/4] npm run build");
    stage(Stage::Vite, &loaded.root, "npm", &["run", "build"])?;

    println!("Build finished.");
    Ok(())
}

/// A `Command` for `program`, routed through `cmd` on Windows.
///
/// `npm` is a `.cmd` shim there and [`Command::new`] does not consult
/// `PATHEXT`, so spawning it by name fails on every Windows machine with
/// "program not found". `cargo` is a real executable and needs none of this,
/// but one rule for both is easier to keep true than two.
fn spawnable(program: &str) -> Command {
    if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/C", program]);
        command
    } else {
        Command::new(program)
    }
}

/// Run one child process to completion, inheriting its output.
///
/// Output is inherited rather than captured: a release build takes minutes,
/// and a progress bar nobody sees until the end is not progress. It also
/// means a compiler error is already on the screen by the time the error
/// below names the stage.
fn stage(stage: Stage, root: &Path, program: &str, args: &[&str]) -> Result<(), BuildError> {
    let status = spawnable(program)
        .args(args)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|source| BuildError::Spawn {
            stage,
            program: program.to_owned(),
            source,
        })?;

    if status.success() {
        Ok(())
    } else {
        Err(BuildError::Failed { stage, status })
    }
}

/// A failure in the production build, always naming its stage.
#[derive(Debug)]
pub enum BuildError {
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
    /// Validation or emission failed.
    Typegen {
        /// Why.
        source: TypegenError,
    },
    /// A stage's program could not be started.
    Spawn {
        /// Which stage.
        stage: Stage,
        /// The program that could not be started.
        program: String,
        /// The spawn failure.
        source: std::io::Error,
    },
    /// A stage ran and failed.
    Failed {
        /// Which stage.
        stage: Stage,
        /// Its exit status.
        status: std::process::ExitStatus,
    },
}

impl BuildError {
    /// The stage that failed.
    #[must_use]
    pub fn stage(&self) -> Stage {
        match self {
            Self::Cwd { .. } | Self::Source { .. } => Stage::Graph,
            Self::Typegen { .. } => Stage::Typegen,
            Self::Spawn { stage, .. } | Self::Failed { stage, .. } => *stage,
        }
    }
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "build failed at the {} stage: ", self.stage())?;
        match self {
            Self::Cwd { source } => {
                write!(formatter, "could not read the working directory: {source}")
            }
            Self::Source { source } => write!(formatter, "{source}"),
            Self::Typegen { source } => write!(formatter, "{source}"),
            Self::Spawn {
                program, source, ..
            } => write!(
                formatter,
                "could not run `{program}`: {source}. It has to be on PATH for this stage."
            ),
            Self::Failed { status, .. } => write!(
                formatter,
                "the command exited with {status}. Its output is above."
            ),
        }
    }
}

impl std::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Cwd { source } | Self::Spawn { source, .. } => Some(source),
            Self::Source { source } => Some(source.as_ref()),
            Self::Typegen { source } => Some(source),
            Self::Failed { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_names_the_stage_it_failed_at() {
        let spawn = BuildError::Spawn {
            stage: Stage::Vite,
            program: String::from("npm"),
            source: std::io::Error::other("not found"),
        };
        assert_eq!(spawn.stage(), Stage::Vite);
        let message = spawn.to_string();
        assert!(message.contains("at the vite stage"), "{message}");
        assert!(message.contains("npm"), "{message}");
    }

    #[test]
    fn a_graph_failure_is_reported_as_the_first_stage_rather_than_as_the_build() {
        let error = BuildError::Cwd {
            source: std::io::Error::other("gone"),
        };
        assert_eq!(error.stage(), Stage::Graph);
        assert!(error.to_string().contains("at the graph stage"));
    }

    #[test]
    fn the_stages_are_named_in_the_order_they_run() {
        let names: Vec<&str> = [Stage::Graph, Stage::Typegen, Stage::Cargo, Stage::Vite]
            .iter()
            .map(|stage| stage.as_str())
            .collect();
        assert_eq!(names, ["graph", "typegen", "cargo", "vite"]);
    }
}
