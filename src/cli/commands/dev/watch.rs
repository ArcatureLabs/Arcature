//! Watching the project for changes that need a rebuild.
//!
//! One responsibility: turn filesystem events into "rebuild now" or "restart
//! now", exactly once per burst of edits. Everything about which files matter
//! lives in [`classify`], as a pure function, so it can be tested without a
//! filesystem and read without one either.
//!
//! # Why so little is watched
//!
//! Only Rust sources, the manifest, and the environment file reach this loop.
//! A `.tsx`, `.css` or `.vue` edit is Vite's business: it is picked up by
//! Vite's own watcher, hot-replaced in the browser, and costs no rebuild at
//! all. That is the single largest reason the development loop feels fast,
//! and it only holds if the supervisor stays out of the way -- a `.rs` filter
//! that accidentally matched a template would rebuild the backend on every
//! style tweak.
//!
//! # Why the ignored directories are never subscribed to, only filtered
//!
//! Filtering an event after it arrives is not the same as not asking for it.
//! On Linux the platform watcher is inotify, which costs one kernel watch
//! *per directory* in a recursive subscription; `target/` after a few builds
//! and `node_modules/` after one install are together tens of thousands of
//! directories, against a default `max_user_watches` of 8192 on a good many
//! distributions. Subscribing to them does not merely waste memory -- it runs
//! the limit out, and the watch that then fails to register is silently some
//! *other* directory, so edits stop triggering rebuilds for a reason nothing
//! reports. So the subscription is built one top-level entry at a time and
//! the ignored ones are never entered.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use notify::{RecursiveMode, Watcher as _};

/// How long to wait for edits to stop before rebuilding.
///
/// Editors write in bursts: a save can be a temp file, a rename, and a
/// truncate, and "format on save" doubles it. Rebuilding on the first event
/// would start a build against a half-written tree and then have to do it
/// again.
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(300);

/// Directory names whose contents never justify a rebuild.
///
/// `target` is the build output -- watching it means every build triggers
/// the next one. `node_modules` is Vite's. `.arcature` is ours, and the
/// restart sentinel we write lives there.
const IGNORED_DIRECTORIES: &[&str] = &["target", "node_modules", ".arcature", ".git"];

/// Generated TypeScript, written by the supervisor after each restart.
/// Rebuilding because of it would make every rebuild trigger another.
const GENERATED_ASSETS: &str = "resources/js/generated/";

/// What a change to a file asks the supervisor to do.
///
/// Ordered by cost, and compared as such: a burst that touches both a source
/// file and `.env` is one rebuild, not a rebuild and a restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Change {
    /// Restart the application without compiling anything.
    ///
    /// Configuration is read once at boot, so a `.env` edit the running
    /// process cannot see is a change the developer made and did not get.
    /// Compiling again to deliver it would be several seconds spent proving
    /// the binary is the one already on disk.
    Restart,
    /// Compile, then restart.
    Rebuild,
}

/// What, if anything, a change to `path` asks for.
///
/// Pure, so the answer is the same in a test as in the watcher thread.
#[must_use]
pub(crate) fn classify(path: &Path) -> Option<Change> {
    if ignored(path) {
        return None;
    }
    if path.extension().is_some_and(|extension| extension == "rs") {
        return Some(Change::Rebuild);
    }
    match path.file_name().and_then(std::ffi::OsStr::to_str) {
        // The manifest and the lockfile are inputs to the compiler in the
        // same sense a source file is: adding a dependency and seeing
        // nothing happen is the same defect as editing a handler and seeing
        // nothing happen.
        Some("Cargo.toml" | "Cargo.lock") => Some(Change::Rebuild),
        Some(name) if name == ".env" || name.starts_with(".env.") => Some(Change::Restart),
        _ => None,
    }
}

/// Should a change to `path` rebuild the backend?
#[must_use]
#[cfg(test)]
pub(crate) fn triggers_rebuild(path: &Path) -> bool {
    classify(path) == Some(Change::Rebuild)
}

/// Is `path` inside somewhere the supervisor never reacts to?
fn ignored(path: &Path) -> bool {
    if path.components().any(|component| {
        matches!(component, Component::Normal(name)
            if name.to_str().is_some_and(|name| IGNORED_DIRECTORIES.contains(&name)))
    }) {
        return true;
    }
    // `Path` comparison cannot see through the separator difference between
    // platforms, and this one prefix is written the same way in both.
    path.to_string_lossy()
        .replace('\\', "/")
        .contains(GENERATED_ASSETS)
}

/// Is this a directory to subscribe to?
fn watchable(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| !IGNORED_DIRECTORIES.contains(&name))
}

/// What the watcher thread sends to the loop.
enum Signal {
    /// A file that matters changed.
    Changed(Change),
    /// A directory appeared directly under the root and has to be
    /// subscribed to, because the root itself is watched shallowly.
    Appeared(PathBuf),
}

/// A running filesystem watcher.
///
/// Holds the `notify` watcher alive: dropping it stops the watch, and a
/// watcher nobody holds is a rebuild loop that silently never fires.
pub(crate) struct Watch {
    watcher: notify::RecommendedWatcher,
    signals: tokio::sync::mpsc::UnboundedReceiver<Signal>,
    /// Directories already subscribed to, so a burst of events about the
    /// same new directory does not register it twice.
    subscribed: HashSet<PathBuf>,
    /// The burst being debounced, held here rather than on the stack of
    /// [`next_change`](Self::next_change) so that dropping that future does
    /// not drop the change with it.
    ///
    /// It is dropped, routinely: the rebuild loop races this against the
    /// build in progress, so that a save arriving mid-build can cancel it.
    /// When the build wins that race the future is discarded, and a save
    /// already absorbed into a local would have been discarded with it --
    /// which is exactly the save the developer is waiting to see.
    pending: Option<Change>,
}

impl Watch {
    /// Start watching the parts of `root` that can ask for a rebuild.
    ///
    /// The root itself is watched shallowly -- enough to notice a top-level
    /// file like `Cargo.toml`, and enough to notice a directory being
    /// created that then gets its own subscription -- and every top-level
    /// directory that is not ignored is watched recursively.
    ///
    /// # Errors
    ///
    /// [`notify::Error`] if the platform watcher cannot be created or the
    /// root itself cannot be watched. A single sub-directory that cannot be
    /// watched is reported and skipped: losing one directory is a partial
    /// watch, while refusing to start is no watch at all.
    pub(crate) fn start(root: &Path) -> Result<Self, notify::Error> {
        let (sender, signals) = tokio::sync::mpsc::unbounded_channel();
        let watch_root = root.to_path_buf();
        // `notify` calls this from its own thread. An unbounded tokio send
        // never blocks and never awaits, which is the only thing that makes
        // it legal here.
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else {
                    return;
                };
                if let Some(change) = event.paths.iter().filter_map(|path| classify(path)).max() {
                    let _ = sender.send(Signal::Changed(change));
                }
                // A directory created directly under the root is outside
                // every existing subscription, since the root is shallow.
                for path in &event.paths {
                    if path.parent() == Some(watch_root.as_path())
                        && path.file_name().is_some_and(watchable)
                        && path.is_dir()
                    {
                        let _ = sender.send(Signal::Appeared(path.clone()));
                    }
                }
            })?;

        watcher.watch(root, RecursiveMode::NonRecursive)?;
        let mut subscribed = HashSet::new();
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                if !entry.file_type().is_ok_and(|kind| kind.is_dir())
                    || !watchable(&entry.file_name())
                {
                    continue;
                }
                let path = entry.path();
                match watcher.watch(&path, RecursiveMode::Recursive) {
                    Ok(()) => {
                        subscribed.insert(path);
                    }
                    Err(error) => eprintln!(
                        "warning: not watching {} for changes: {error}",
                        path.display()
                    ),
                }
            }
        }

        Ok(Self {
            watcher,
            signals,
            subscribed,
            pending: None,
        })
    }

    /// Wait for a burst of edits to finish, then report the costliest thing
    /// it asked for.
    ///
    /// Returns `None` when the watcher has shut down, which ends the loop.
    ///
    /// Cancel-safe: dropping the returned future keeps whatever it had
    /// absorbed, so the caller may race it against other work.
    pub(crate) async fn next_change(&mut self) -> Option<Change> {
        loop {
            // Before the first change there is nothing to debounce, so wait
            // without a deadline; afterwards every further event extends the
            // window, and the window closing is what ends the burst.
            let signal = match self.pending {
                None => self.signals.recv().await?,
                Some(_) => match tokio::time::timeout(DEBOUNCE, self.signals.recv()).await {
                    Ok(Some(signal)) => signal,
                    Ok(None) | Err(_) => return self.pending.take(),
                },
            };
            match signal {
                Signal::Changed(change) => {
                    self.pending = Some(self.pending.map_or(change, |held| held.max(change)));
                }
                Signal::Appeared(path) => self.subscribe(path),
            }
        }
    }

    /// Subscribe to a directory that appeared after the watch started.
    fn subscribe(&mut self, path: PathBuf) {
        if !self.subscribed.insert(path.clone()) {
            return;
        }
        if let Err(error) = self.watcher.watch(&path, RecursiveMode::Recursive) {
            self.subscribed.remove(&path);
            eprintln!(
                "warning: not watching {} for changes: {error}",
                path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_rust_source_edit_rebuilds() {
        assert!(triggers_rebuild(&PathBuf::from(
            "app/controllers/home_controller.rs"
        )));
        assert!(triggers_rebuild(&PathBuf::from("src/main.rs")));
    }

    #[test]
    fn a_frontend_edit_never_rebuilds_because_vite_already_handled_it() {
        for path in [
            "resources/js/pages/home.tsx",
            "resources/css/app.css",
            "resources/js/app.vue",
            "package.json",
        ] {
            assert_eq!(
                classify(&PathBuf::from(path)),
                None,
                "{path} must not cost a rebuild"
            );
        }
    }

    #[test]
    fn adding_a_dependency_rebuilds_like_editing_a_source_file() {
        // The compiler's inputs are not only the `.rs` files. A developer who
        // adds a crate and sees nothing happen has hit the same defect as one
        // who edits a handler and sees nothing happen.
        assert!(triggers_rebuild(&PathBuf::from("Cargo.toml")));
        assert!(triggers_rebuild(&PathBuf::from("Cargo.lock")));
    }

    #[test]
    fn an_environment_edit_restarts_without_paying_for_a_compile() {
        for path in [".env", ".env.local", "config/.env.development"] {
            assert_eq!(
                classify(&PathBuf::from(path)),
                Some(Change::Restart),
                "{path} changes what the process read at boot, not what the compiler reads"
            );
        }
    }

    #[test]
    fn a_burst_touching_both_asks_for_the_costlier_one() {
        // `Ord` is the mechanism, so it is the thing to pin down.
        assert!(Change::Rebuild > Change::Restart);
        assert_eq!(Change::Restart.max(Change::Rebuild), Change::Rebuild);
    }

    #[test]
    fn build_output_never_rebuilds_or_the_loop_would_never_stop() {
        for path in [
            "target/debug/build/foo-123/out/generated.rs",
            "target/debug/incremental/app/s-abc/query-cache.rs",
        ] {
            assert!(!triggers_rebuild(&PathBuf::from(path)), "{path}");
        }
    }

    #[test]
    fn a_vendored_manifest_inside_an_ignored_directory_is_still_ignored() {
        // Matching `Cargo.toml` by name is a wider net than the extension
        // test, so the ignore list has to be applied first.
        assert_eq!(
            classify(&PathBuf::from("target/package/thing-1.0/Cargo.toml")),
            None
        );
        assert_eq!(
            classify(&PathBuf::from("node_modules/esbuild/Cargo.toml")),
            None
        );
    }

    #[test]
    fn the_supervisors_own_scratch_directory_never_rebuilds() {
        assert_eq!(classify(&PathBuf::from(".arcature/restart")), None);
        assert_eq!(classify(&PathBuf::from(".arcature/anything.rs")), None);
    }

    #[test]
    fn generated_typescript_never_rebuilds() {
        // Written after every restart; rebuilding on it would loop.
        assert!(!triggers_rebuild(&PathBuf::from(
            "resources/js/generated/routes.rs"
        )));
        assert!(!triggers_rebuild(&PathBuf::from(
            r"resources\js\generated\routes.rs"
        )));
    }

    #[test]
    fn a_dependency_source_inside_node_modules_never_rebuilds() {
        assert!(!triggers_rebuild(&PathBuf::from(
            "node_modules/some-pkg/build.rs"
        )));
    }

    #[test]
    fn an_absolute_path_is_judged_by_the_same_rules() {
        let root = std::env::temp_dir().join("project");
        assert!(triggers_rebuild(&root.join("app").join("service.rs")));
        assert!(!triggers_rebuild(&root.join("target").join("thing.rs")));
    }

    #[test]
    fn the_directories_that_would_exhaust_the_kernels_watch_budget_are_never_subscribed_to() {
        for name in ["target", "node_modules", ".git", ".arcature"] {
            assert!(
                !watchable(std::ffi::OsStr::new(name)),
                "{name} must never be subscribed to recursively"
            );
        }
        for name in ["app", "src", "config", "resources"] {
            assert!(watchable(std::ffi::OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn only_the_source_directories_of_a_project_are_subscribed_to() {
        let root = std::env::temp_dir().join(format!("arcature-watch-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&root));
        for directory in ["app", "target", "node_modules", ".git"] {
            std::fs::create_dir_all(root.join(directory)).expect("a temp tree");
        }
        std::fs::write(root.join("Cargo.toml"), b"[package]").expect("a manifest");

        let watch = Watch::start(&root).expect("the watcher starts");

        assert!(watch.subscribed.contains(&root.join("app")));
        for ignored in ["target", "node_modules", ".git"] {
            assert!(
                !watch.subscribed.contains(&root.join(ignored)),
                "{ignored} was subscribed to"
            );
        }
        drop(watch);
        drop(std::fs::remove_dir_all(&root));
    }
}
