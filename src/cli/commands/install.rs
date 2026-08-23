//! `arc install` -- install the frontend's npm dependencies.
//!
//! A generated project is two halves. `cargo` resolves the Rust one on the
//! first build with no prompting; the Node one needs `npm install` run once,
//! and until `0.1.3` nothing said so. `arc new` printed a single line naming
//! the directory it had written, the generated `justfile` mentioned
//! `npm install` in a recipe most people never open, and `arc dev` went
//! straight to spawning Node -- which failed with a `ERR_MODULE_NOT_FOUND`
//! stack trace naming `vite`, from inside a generated file the reader had
//! never seen.
//!
//! This command is the missing half, and [`ensure_installed`] is the check
//! that keeps anybody from meeting the stack trace again.
//!
//! # Why `npm install` and not `npm ci`
//!
//! `npm ci` is the right command in the `Dockerfile`, where the lockfile is
//! the input and a drifted one should fail the build. On a developer's
//! machine it is the wrong default: it deletes `node_modules` wholesale and
//! refuses outright when `package.json` and the lockfile disagree, which is
//! the ordinary state five seconds after adding a dependency. `--ci` asks for
//! it explicitly, for a machine that wants the lockfile enforced.
//!
//! # Why shelling out rather than a Node API
//!
//! There is no Rust npm client here and there should not be. `npm` resolves a
//! registry, a proxy, a private scope, a `.npmrc` in three locations and a
//! corporate certificate store, and every one of those is configuration the
//! developer has already set up for every other Node tool on the machine.
//! Reimplementing the tenth of that we would need is how a framework ends up
//! owning a package manager.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The manifest that marks a directory as having a frontend to install.
const MANIFEST: &str = "package.json";
/// The lockfile `--ci` requires.
const LOCKFILE: &str = "package-lock.json";
/// Where npm puts what it installed.
const MODULES: &str = "node_modules";
/// The one package `arc dev` cannot start without, and therefore the one
/// worth naming when it is missing. A `node_modules` that exists but predates
/// the dependency is still a broken install, so the check reaches for the
/// package rather than stopping at the directory.
const REQUIRED: &str = "vite";

/// What a successful install did, so the caller can report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    /// `npm install` ran.
    Install,
    /// `npm ci` ran, because `--ci` asked for it.
    Ci,
}

impl Installed {
    /// The command line that was run, for the report.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Install => "npm install",
            Self::Ci => "npm ci",
        }
    }
}

/// Execute `arc install` against the current directory.
///
/// # Errors
///
/// See [`InstallError`].
pub fn run(ci: bool) -> Result<Installed, InstallError> {
    let root = std::env::current_dir().map_err(|source| InstallError::Io {
        path: PathBuf::from("."),
        source,
    })?;
    install(&root, ci)
}

/// The testable half of [`run`]: everything, against an explicit project root.
///
/// # Errors
///
/// See [`InstallError`].
pub fn install(root: &Path, ci: bool) -> Result<Installed, InstallError> {
    if !root.join(MANIFEST).is_file() {
        return Err(InstallError::NoManifest {
            root: root.to_path_buf(),
        });
    }
    if ci && !root.join(LOCKFILE).is_file() {
        return Err(InstallError::NoLockfile {
            root: root.to_path_buf(),
        });
    }

    let chosen = if ci {
        Installed::Ci
    } else {
        Installed::Install
    };
    let argument = if ci { "ci" } else { "install" };

    let status = npm(root).arg(argument).status().map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            InstallError::NpmMissing
        } else {
            InstallError::Spawn {
                source: source.to_string(),
            }
        }
    })?;

    if status.success() {
        Ok(chosen)
    } else {
        Err(InstallError::Failed {
            command: chosen.as_str(),
            code: status.code(),
        })
    }
}

/// Whether the frontend's dependencies are present.
///
/// False for a project that has a `package.json` and no installed [`REQUIRED`]
/// package. A project with no `package.json` at all is not missing an
/// install -- it has no frontend -- so this answers true for it and lets the
/// caller proceed.
#[must_use]
pub fn is_installed(root: &Path) -> bool {
    if !root.join(MANIFEST).is_file() {
        return true;
    }
    root.join(MODULES).join(REQUIRED).is_dir()
}

/// Refuse to continue when the frontend is not installed, naming the fix.
///
/// `arc dev` calls this before it writes the Node entry point or spawns
/// anything. The alternative -- and what `0.1.2` did -- is to let Node fail
/// on `import { createServer } from 'vite'`, which surfaces as an
/// `ERR_MODULE_NOT_FOUND` trace through `node:internal/modules/*` naming a
/// generated file the reader did not write. The condition is a missing
/// install every time, and nothing in that output says so.
///
/// # Errors
///
/// [`InstallError::NotInstalled`] when there is a frontend and it has not
/// been installed.
pub fn ensure_installed(root: &Path) -> Result<(), InstallError> {
    if is_installed(root) {
        Ok(())
    } else {
        Err(InstallError::NotInstalled)
    }
}

/// `npm`, spawned the way the platform needs.
///
/// On Windows `npm` is `npm.cmd`, a batch script, and `CreateProcess` will
/// not run one directly -- `Command::new("npm")` fails with "program not
/// found" on a machine where `npm --version` works in the same shell. Going
/// through `cmd /C` is what makes the two agree.
fn npm(root: &Path) -> Command {
    let mut command = if cfg!(windows) {
        let mut shell = Command::new("cmd");
        shell.arg("/C").arg("npm");
        shell
    } else {
        Command::new("npm")
    };
    command.current_dir(root);
    command
}

/// A typed failure from `arc install`.
#[derive(Debug)]
pub enum InstallError {
    /// The directory holds no `package.json`.
    NoManifest {
        /// The directory that was checked.
        root: PathBuf,
    },
    /// `--ci` was asked for and there is no lockfile to enforce.
    NoLockfile {
        /// The directory that was checked.
        root: PathBuf,
    },
    /// `npm` is not on `PATH`.
    NpmMissing,
    /// `npm` could not be spawned for a reason other than being absent.
    Spawn {
        /// What the operating system said.
        source: String,
    },
    /// `npm` ran and exited non-zero.
    Failed {
        /// Which command was run.
        command: &'static str,
        /// The exit code, when the process returned one.
        code: Option<i32>,
    },
    /// A command needing the frontend found it uninstalled.
    NotInstalled,
    /// The working directory could not be read.
    Io {
        /// The path involved.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoManifest { root } => write!(
                formatter,
                "{} has no {MANIFEST}, so there is no frontend to install -- \
                 run this from the root of a generated application",
                root.display()
            ),
            Self::NoLockfile { root } => write!(
                formatter,
                "--ci needs {LOCKFILE} and {} has none; run `arc install` \
                 without --ci to create one",
                root.display()
            ),
            Self::NpmMissing => formatter.write_str(
                "npm is not on PATH. Install Node.js 22 or newer -- the \
                 frontend half of a generated application is built by Vite, \
                 and `arc dev` and `arc build` both drive it through npm",
            ),
            Self::Spawn { source } => write!(formatter, "could not run npm: {source}"),
            Self::Failed { command, code } => match code {
                Some(code) => write!(
                    formatter,
                    "`{command}` failed with exit code {code}; its output is above"
                ),
                None => write!(formatter, "`{command}` was terminated by a signal"),
            },
            // `write!`, not `write_str`: the latter takes a plain literal and
            // would print the braces around these two names rather than the
            // names.
            Self::NotInstalled => write!(
                formatter,
                "the frontend is not installed: this project has no \
                 {MODULES}/{REQUIRED}. Run `arc install` first -- `arc dev` \
                 starts Vite from the project's own copy and cannot run \
                 without it."
            ),
            Self::Io { path, source } => {
                write!(formatter, "{}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(with_modules: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(MANIFEST), "{}").expect("manifest");
        if with_modules {
            std::fs::create_dir_all(dir.path().join(MODULES).join(REQUIRED)).expect("modules");
        }
        dir
    }

    #[test]
    fn a_directory_without_a_manifest_has_no_frontend_to_install() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = install(dir.path(), false).expect_err("no package.json");
        assert!(matches!(error, InstallError::NoManifest { .. }));
        assert!(error.to_string().contains(MANIFEST));
    }

    #[test]
    fn ci_without_a_lockfile_is_refused_before_npm_runs() {
        let dir = project(false);
        let error = install(dir.path(), true).expect_err("no lockfile");
        assert!(matches!(error, InstallError::NoLockfile { .. }));
        assert!(error.to_string().contains(LOCKFILE));
    }

    #[test]
    fn a_project_with_no_frontend_is_not_reported_as_uninstalled() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(is_installed(dir.path()));
        assert!(ensure_installed(dir.path()).is_ok());
    }

    #[test]
    fn a_frontend_without_node_modules_is_uninstalled() {
        let dir = project(false);
        assert!(!is_installed(dir.path()));
        let error = ensure_installed(dir.path()).expect_err("not installed");
        assert!(matches!(error, InstallError::NotInstalled));
        let message = error.to_string();
        assert!(message.contains("arc install"), "got: {message}");
        // The message names the directory it looked in. Written with
        // `write_str` this read "no {MODULES}/{REQUIRED}" -- a literal, since
        // only `write!` interpolates.
        assert!(message.contains("node_modules/vite"), "got: {message}");
        assert!(!message.contains('{'), "got: {message}");
    }

    #[test]
    fn an_installed_frontend_passes_the_check() {
        let dir = project(true);
        assert!(is_installed(dir.path()));
        assert!(ensure_installed(dir.path()).is_ok());
    }

    /// A `node_modules` that exists but does not hold the package `arc dev`
    /// imports is the state a half-finished or interrupted install leaves,
    /// and stopping at the directory would call it installed.
    #[test]
    fn node_modules_without_vite_is_still_uninstalled() {
        let dir = project(false);
        std::fs::create_dir_all(dir.path().join(MODULES).join("something-else")).expect("modules");
        assert!(!is_installed(dir.path()));
    }

    #[test]
    fn the_report_names_the_command_that_ran() {
        assert_eq!(Installed::Install.as_str(), "npm install");
        assert_eq!(Installed::Ci.as_str(), "npm ci");
    }
}
