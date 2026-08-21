//! Locating the project `arc dev` is supervising.
//!
//! One responsibility: turn "the directory the developer typed `arc dev` in"
//! into the three facts the supervisor needs -- the crate root, the
//! `.arcature` scratch directory, and confirmation that Node is available.
//!
//! Node is not optional. Vite runs in a Node process; there is no Rust
//! implementation of it to fall back to. Saying so on the first line, with
//! the reason, is better than a spawn failure five steps into startup.

use std::path::{Path, PathBuf};

/// The scratch directory `arc dev` owns inside a project.
///
/// Everything the supervisor generates lives here: the Vite entry script,
/// the restart sentinel, and (on Unix) the socket files. It is a single
/// directory so `.gitignore` needs one line and a stale run is one
/// `rm -rf` away.
pub(crate) const SCRATCH_DIR: &str = ".arcature";

/// The file whose modification time tells the browser to reload.
///
/// The Vite process watches it; the supervisor touches it once the rebuilt
/// backend answers. Going through a file rather than a socket is what keeps
/// the two processes independent -- Vite does not need to know the
/// supervisor exists, and a supervisor restart does not require Vite to
/// reconnect to anything.
pub(crate) const RESTART_SENTINEL: &str = "restart";

/// The file `arc dev` writes its listening address into.
///
/// A file rather than a fixed port: `arc dev --port` exists, two projects can
/// be running at once, and guessing `127.0.0.1:3000` would sooner or later
/// hand one project's graph to another project's `arc typegen`. Written once
/// the port is bound and removed when the supervisor stops, so its presence
/// is the claim that something is listening.
pub(crate) const DEV_ADDRESS_FILE: &str = "dev.addr";

/// A resolved project.
#[derive(Debug, Clone)]
pub(crate) struct Project {
    root: PathBuf,
}

impl Project {
    /// Find the project containing `start`.
    ///
    /// Walks up for the nearest ancestor holding a `Cargo.toml`, then insists
    /// on a `package.json` beside it.
    ///
    /// # Errors
    ///
    /// [`ProjectError::NoCargoToml`] when no ancestor is a crate;
    /// [`ProjectError::NoPackageJson`] when the crate has no Node project.
    pub(crate) fn discover(start: &Path) -> Result<Self, ProjectError> {
        let root = start
            .ancestors()
            .find(|dir| dir.join("Cargo.toml").is_file())
            .ok_or_else(|| ProjectError::NoCargoToml {
                from: start.to_path_buf(),
            })?
            .to_path_buf();

        if !root.join("package.json").is_file() {
            return Err(ProjectError::NoPackageJson { root });
        }

        Ok(Self { root })
    }

    /// The crate root -- the directory holding `Cargo.toml`.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The `.arcature` scratch directory, created if it does not exist.
    ///
    /// # Errors
    ///
    /// [`ProjectError::Scratch`] if the directory cannot be created.
    pub(crate) fn scratch(&self) -> Result<PathBuf, ProjectError> {
        let dir = self.root.join(SCRATCH_DIR);
        std::fs::create_dir_all(&dir).map_err(|source| ProjectError::Scratch {
            path: dir.clone(),
            source,
        })?;
        Ok(dir)
    }

    /// The restart sentinel path. Vite watches it; the supervisor touches it.
    pub(crate) fn sentinel(&self) -> PathBuf {
        self.root.join(SCRATCH_DIR).join(RESTART_SENTINEL)
    }
}

/// Touch the restart sentinel so Vite tells the browser to reload.
///
/// Writes the current time rather than an empty byte: chokidar compares
/// size and mtime, and a zero-length rewrite of a zero-length file is a
/// change some filesystems do not report.
///
/// # Errors
///
/// `io::Error` if the sentinel cannot be written. The caller logs and
/// continues -- a missed reload is a nuisance, not a reason to stop serving.
pub(crate) fn touch_sentinel(path: &Path) -> std::io::Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    std::fs::write(path, format!("{stamp}\n"))
}

/// A published dev-server address, tied to the file that holds it.
///
/// A guard rather than two free functions because the file is a claim that
/// something is listening: leaving it behind after the supervisor stops
/// would send the next `arc typegen` to a closed port and make it wait for a
/// connection timeout before falling back.
#[derive(Debug)]
pub(crate) struct PublishedAddress {
    path: PathBuf,
}

impl PublishedAddress {
    /// Write `address` into the scratch directory of `root`.
    ///
    /// # Errors
    ///
    /// `io::Error` if the file cannot be written. The caller warns and
    /// continues: a missing address file only costs `arc typegen` the slower
    /// path, and a development server that refused to start over it would be
    /// worse than the slower path.
    pub(crate) fn publish(root: &Path, address: &str) -> std::io::Result<Self> {
        let dir = root.join(SCRATCH_DIR);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(DEV_ADDRESS_FILE);
        std::fs::write(
            &path,
            format!(
                "{address}
"
            ),
        )?;
        Ok(Self { path })
    }
}

impl Drop for PublishedAddress {
    fn drop(&mut self) {
        // Best effort. The file is advisory, and a failure to remove it is
        // not something the developer can act on at the moment it happens.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Is Node on `PATH`?
///
/// Checked by running `node --version`, because "the file exists" and "the
/// file runs" differ often enough on Windows to be worth the 30ms.
pub(crate) fn node_version() -> Option<String> {
    let output = std::process::Command::new("node")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!version.is_empty()).then_some(version)
}

/// A failure resolving the project.
#[derive(Debug)]
pub(crate) enum ProjectError {
    /// No ancestor of the working directory holds a `Cargo.toml`.
    NoCargoToml { from: PathBuf },
    /// The crate has no `package.json`, so it has no Vite to run.
    NoPackageJson { root: PathBuf },
    /// The `.arcature` scratch directory could not be created.
    Scratch {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCargoToml { from } => write!(
                formatter,
                "no Cargo.toml in {} or any parent directory, so there is no application to run",
                from.display()
            ),
            Self::NoPackageJson { root } => write!(
                formatter,
                "{} has no package.json. `arc dev` runs Vite in a Node process to serve \
                 assets and HMR over the single port, so the project needs a Node side. \
                 Run `arc new` to scaffold one, or `arc serve` to run the backend alone.",
                root.display()
            ),
            Self::Scratch { path, source } => {
                write!(formatter, "could not create {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Scratch { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "arcature-project-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir should be creatable");
        dir
    }

    #[test]
    fn a_directory_with_no_crate_above_it_is_not_a_project() {
        let dir = scratch_dir("no-crate");
        let error = Project::discover(&dir).expect_err("there is no Cargo.toml here");
        assert!(matches!(error, ProjectError::NoCargoToml { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crate_without_a_node_project_is_refused_with_a_reason() {
        let dir = scratch_dir("no-node");
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").expect("write");
        let error = Project::discover(&dir).expect_err("there is no package.json here");
        let message = error.to_string();
        assert!(message.contains("package.json"), "{message}");
        assert!(message.contains("Node"), "{message}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_project_root_is_the_nearest_ancestor_holding_a_manifest() {
        let dir = scratch_dir("nested");
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").expect("write");
        std::fs::write(dir.join("package.json"), "{}").expect("write");
        let nested = dir.join("app").join("controllers");
        std::fs::create_dir_all(&nested).expect("nested dirs");

        let project = Project::discover(&nested).expect("the crate above should be found");
        // Compare canonically: the temp directory may be reached through a
        // symlink (macOS `/var` -> `/private/var`).
        assert_eq!(
            project.root().canonicalize().expect("root exists"),
            dir.canonicalize().expect("dir exists")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_sentinel_lives_in_the_scratch_directory() {
        let dir = scratch_dir("sentinel");
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").expect("write");
        std::fs::write(dir.join("package.json"), "{}").expect("write");
        let project = Project::discover(&dir).expect("project");

        let created = project.scratch().expect("scratch should be creatable");
        assert!(created.is_dir());
        assert_eq!(project.sentinel(), created.join(RESTART_SENTINEL));

        touch_sentinel(&project.sentinel()).expect("sentinel should be writable");
        assert!(project.sentinel().is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn touching_the_sentinel_twice_changes_what_it_contains() {
        let dir = scratch_dir("sentinel-changes");
        let path = dir.join("restart");
        touch_sentinel(&path).expect("first touch");
        let first = std::fs::read_to_string(&path).expect("read");
        // A watcher that compares content, not just mtime, still sees a
        // change: the stamp has nanosecond resolution.
        std::thread::sleep(std::time::Duration::from_millis(2));
        touch_sentinel(&path).expect("second touch");
        let second = std::fs::read_to_string(&path).expect("read");
        assert_ne!(first, second);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
