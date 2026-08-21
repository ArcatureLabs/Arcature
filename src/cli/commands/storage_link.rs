//! `arc storage:link` — expose `storage/app/public` at `public/storage`.
//!
//! Uploads live under `storage/`, which the static file server never serves.
//! The ones that are meant to be public need a door into `public/`, and that
//! door is a link so the bytes stay in exactly one place.
//!
//! # Why this never falls back to copying
//!
//! A copy would work on the first run and then quietly rot: every upload after
//! the copy would be invisible, and the developer would be debugging a cache
//! that is really a stale duplicate. Losing the link is a setup problem with a
//! one-line fix; silently serving yesterday's files is a bug you find in
//! production. So the ladder is symlink, then (on Windows) a directory
//! junction, then a hard error that names the fix.
//!
//! On Windows a symlink needs either Developer Mode or an elevated shell,
//! which many machines do not have. A directory junction needs neither, and
//! for a same-volume directory it behaves the same way for our purposes, so it
//! is a genuine second rung rather than a consolation prize.

use std::path::{Path, PathBuf};

/// The directory that actually holds the files, one component at a time.
///
/// Joined rather than written as `"storage/app/public"` because the junction
/// fallback hands the path to `cmd`, and `cmd` reads a forward slash as the
/// start of a switch: `mklink /J public/storage ...` fails with
/// `Invalid switch - "storage"`. Building from components gives the platform
/// separator on both sides of the call.
const SOURCE: [&str; 3] = ["storage", "app", "public"];
/// Where the web server expects to find them. See [`SOURCE`] for the shape.
const LINK: [&str; 2] = ["public", "storage"];

/// Join a component list onto `root`.
fn under(root: &Path, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// A component list as a display string, for messages.
fn shown(parts: &[&str]) -> String {
    parts.join("/")
}

/// How the link was created, so the command can say what it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// A real symbolic link.
    Symlink,
    /// A Windows directory junction, used when symlinks are not permitted.
    Junction,
}

impl LinkKind {
    /// The word to use when reporting success.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Symlink => "symlink",
            Self::Junction => "directory junction",
        }
    }
}

/// Execute `arc storage:link` against the current directory.
///
/// # Errors
///
/// See [`StorageLinkError`].
pub fn run() -> Result<(), StorageLinkError> {
    let root = std::env::current_dir().map_err(|source| StorageLinkError::Io {
        path: PathBuf::from("."),
        source,
    })?;
    let kind = link(&root)?;
    println!("{} -> {} ({})", shown(&LINK), shown(&SOURCE), kind.as_str());
    Ok(())
}

/// The testable half of [`run`]: link `root/public/storage` at
/// `root/storage/app/public`.
///
/// # Errors
///
/// See [`StorageLinkError`].
pub fn link(root: &Path) -> Result<LinkKind, StorageLinkError> {
    let source = under(root, &SOURCE);
    let destination = under(root, &LINK);

    // Creating the source is part of the job: a fresh checkout keeps empty
    // directories out of git, so `storage/app/public` often does not exist yet
    // and failing here would be pedantry rather than safety.
    std::fs::create_dir_all(&source).map_err(|source_error| StorageLinkError::Io {
        path: source.clone(),
        source: source_error,
    })?;

    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|source_error| StorageLinkError::Io {
            path: parent.to_path_buf(),
            source: source_error,
        })?;
    }

    // `symlink_metadata` rather than `metadata`: a link pointing at a deleted
    // target still occupies the name, and reporting "nothing there" would send
    // the developer chasing a filesystem that disagrees with us.
    if std::fs::symlink_metadata(&destination).is_ok() {
        return Err(StorageLinkError::Occupied { path: destination });
    }

    create_link(&source, &destination)
}

/// Create the link, preferring a symlink and falling back where the platform
/// offers something equivalent.
#[cfg(windows)]
fn create_link(source: &Path, destination: &Path) -> Result<LinkKind, StorageLinkError> {
    let symlink_error = match std::os::windows::fs::symlink_dir(source, destination) {
        Ok(()) => return Ok(LinkKind::Symlink),
        Err(error) => error,
    };

    match junction(source, destination) {
        Ok(()) => Ok(LinkKind::Junction),
        Err(junction_error) => Err(StorageLinkError::NotPermitted {
            path: destination.to_path_buf(),
            symlink_error,
            junction_error,
        }),
    }
}

/// Ask `cmd` for a junction.
///
/// `mklink /J` is the only junction API reachable without `unsafe`: the
/// alternative is `DeviceIoControl` with a hand-built reparse buffer, and this
/// crate forbids `unsafe`. Shelling out is the honest trade.
#[cfg(windows)]
fn junction(source: &Path, destination: &Path) -> Result<(), String> {
    let output = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(destination)
        .arg(source)
        .output()
        .map_err(|error| format!("could not run `cmd /C mklink /J`: {error}"))?;

    if output.status.success() {
        return Ok(());
    }

    let message = String::from_utf8_lossy(&output.stderr);
    let message = message.trim();
    if message.is_empty() {
        Err(format!("`mklink /J` failed with {}", output.status))
    } else {
        Err(message.to_string())
    }
}

/// Create the link, preferring a symlink and falling back where the platform
/// offers something equivalent.
#[cfg(not(windows))]
fn create_link(source: &Path, destination: &Path) -> Result<LinkKind, StorageLinkError> {
    match std::os::unix::fs::symlink(source, destination) {
        Ok(()) => Ok(LinkKind::Symlink),
        // Unix has no second rung: if the symlink is refused, the filesystem
        // or the permissions are the problem, and copying would hide it.
        Err(symlink_error) => Err(StorageLinkError::NotPermitted {
            path: destination.to_path_buf(),
            symlink_error,
            junction_error: String::from("this platform has no directory-junction equivalent"),
        }),
    }
}

/// An error from the `storage:link` command.
#[derive(Debug)]
pub enum StorageLinkError {
    /// `public/storage` already exists.
    Occupied { path: PathBuf },
    /// Neither a symlink nor a junction could be created.
    NotPermitted {
        path: PathBuf,
        symlink_error: std::io::Error,
        junction_error: String,
    },
    /// A filesystem operation failed.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for StorageLinkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Occupied { path } => write!(
                formatter,
                "{} already exists; remove it and run `arc storage:link` again \
                 (if it is already the link you want, there is nothing to do)",
                path.display()
            ),
            Self::NotPermitted {
                path,
                symlink_error,
                junction_error,
            } => write!(
                formatter,
                "could not link {}: creating a symlink failed ({symlink_error}) \
                 and the fallback failed ({junction_error}). {}",
                path.display(),
                remedy()
            ),
            Self::Io { path, source } => {
                write!(formatter, "could not prepare {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for StorageLinkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. }
            | Self::NotPermitted {
                symlink_error: source,
                ..
            } => Some(source),
            Self::Occupied { .. } => None,
        }
    }
}

/// The platform-specific fix, spelled out so the message is actionable.
#[cfg(windows)]
fn remedy() -> &'static str {
    "Turn on Developer Mode (Settings > System > For developers > Developer Mode) \
     or run this command from an elevated terminal. Arcature will not copy the \
     files instead: a copy goes stale the moment something is uploaded."
}

/// The platform-specific fix, spelled out so the message is actionable.
#[cfg(not(windows))]
fn remedy() -> &'static str {
    "Check that you own the `public/` directory and that the filesystem supports \
     symbolic links. Arcature will not copy the files instead: a copy goes stale \
     the moment something is uploaded."
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether this machine lets an unprivileged process link at all. On
    /// Windows without Developer Mode both rungs can fail, and a test that
    /// demanded success would be testing the CI image rather than the code.
    fn linking_is_permitted(root: &Path) -> bool {
        let probe = root.join("probe");
        std::fs::create_dir_all(probe.join("target")).expect("probe target");
        create_link(&probe.join("target"), &probe.join("link")).is_ok()
    }

    #[test]
    fn linking_creates_the_source_directory_it_points_at() {
        let dir = tempfile::tempdir().expect("tempdir");
        if !linking_is_permitted(dir.path()) {
            return;
        }
        let kind = link(dir.path()).expect("linked");
        assert!(matches!(kind, LinkKind::Symlink | LinkKind::Junction));
        assert!(under(dir.path(), &SOURCE).is_dir());
        assert!(std::fs::symlink_metadata(under(dir.path(), &LINK)).is_ok());
    }

    #[test]
    fn the_link_reaches_the_files_behind_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        if !linking_is_permitted(dir.path()) {
            return;
        }
        link(dir.path()).expect("linked");
        std::fs::write(under(dir.path(), &SOURCE).join("avatar.txt"), "bytes").expect("write");
        let through_link =
            std::fs::read_to_string(under(dir.path(), &LINK).join("avatar.txt")).expect("read");
        assert_eq!(through_link, "bytes");
    }

    #[test]
    fn an_existing_public_storage_is_refused_rather_than_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(under(dir.path(), &LINK)).expect("existing");
        std::fs::write(under(dir.path(), &LINK).join("keep.txt"), "mine").expect("write");

        let error = link(dir.path()).expect_err("occupied");
        assert!(matches!(error, StorageLinkError::Occupied { .. }));
        assert!(error.to_string().contains("already exists"));
        // The refusal must not have eaten the developer's files.
        assert!(under(dir.path(), &LINK).join("keep.txt").exists());
    }

    #[test]
    fn a_failure_to_link_never_leaves_a_copy_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(under(dir.path(), &SOURCE)).expect("source");
        std::fs::write(under(dir.path(), &SOURCE).join("avatar.txt"), "bytes").expect("write");
        std::fs::create_dir_all(under(dir.path(), &LINK)).expect("occupied");

        assert!(link(dir.path()).is_err());
        assert!(!under(dir.path(), &LINK).join("avatar.txt").exists());
    }

    #[test]
    fn the_remedy_tells_the_developer_what_to_change() {
        assert!(remedy().contains("not copy"));
    }
}
