//! Building the application and restarting it, without touching anything else.
//!
//! One responsibility: take the backend from "a source file changed" back to
//! "listening on the application IPC endpoint", and report how long each
//! part of that took. The TCP listener and the Vite process are not this
//! module's to touch, and that is the point -- a rebuild that killed either
//! one would drop connections and break HMR, which are the two things the
//! whole topology exists to prevent.
//!
//! # Why the built binary is spawned directly
//!
//! `cargo run` would repeat the freshness check the build just did, and it
//! puts a `cargo` process between the supervisor and the child it has to
//! kill. The build already reports the executable's path in its JSON
//! output, so the supervisor spawns it itself: one process, killable
//! directly, and no second look at the dependency graph on every save.
//!
//! # Why a copy of it, on Windows
//!
//! Windows will not let anything replace the file backing a running
//! process, and the whole point of the topology is that the old application
//! keeps answering while the new one compiles. Building on top of the
//! running binary therefore fails -- `failed to remove file ...\demo.exe`,
//! every rebuild, forever -- so the supervisor runs a copy kept out of
//! cargo's way and leaves cargo's own output free to be overwritten.
//!
//! Unix has no such rule: replacing an executable there unlinks the old
//! inode and the running process keeps it. The copy is skipped, because an
//! 18 MB memcpy on every save is a real cost in a loop whose whole purpose
//! is latency, and paying it to avoid one platform check would be the wrong
//! trade.

use std::hash::Hasher as _;
use std::io::{BufRead as _, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::child::{ChildGuard, inherited};
use super::endpoints::{self, Endpoints};
use super::service::BackendHandle;
use super::{DevError, project};

/// How long to wait for a freshly spawned backend to start listening.
///
/// Generous: an application that connects to a database and runs migrations
/// at startup can take a while, and timing that out would kill a process
/// that was about to work.
const BOOT_TIMEOUT: Duration = Duration::from_secs(60);

/// How many lines of cargo's stderr to keep for the failure page.
///
/// Enough for a manifest error with its `Caused by` chain, and short enough
/// that a build which fails after a thousand lines of progress does not put
/// a thousand lines in the browser.
const STDERR_TAIL: usize = 40;

/// The directory, beside cargo's output, where the running copy is kept.
///
/// Inside `target/`, so it is already gitignored and already cleaned by
/// `cargo clean`, and named so that a developer who finds it can guess what
/// put it there.
const STAGE_DIR: &str = ".arc-dev";

/// What the supervisor measured on one trip round the rebuild loop.
///
/// The stage names come from the development-loop budget: the point of
/// printing them is that a slow loop can be attributed to something. What is
/// reported is what cargo's message stream actually shows -- see
/// [`Stages::codegen_link`] for why two of the budget's four stages are one
/// number here rather than two invented ones.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Stages {
    /// Start of the build to the last non-executable artifact: everything up
    /// to and including type-checking the application crate. `None` when
    /// nothing but the binary was rebuilt, in which case there is no
    /// boundary to report.
    pub(crate) check: Option<Duration>,
    /// That boundary to the linked executable.
    ///
    /// Codegen and link are separate stages in the budget and are reported
    /// together here because cargo's message stream does not distinguish
    /// them -- only rustc's self-profiler does, and turning that on would
    /// cost more than the number is worth. Splitting the figure by guesswork
    /// would be worse than one honest label.
    pub(crate) codegen_link: Option<Duration>,
    /// The whole `cargo build`, spawn to exit.
    ///
    /// Reported next to the two stages above rather than derived from them:
    /// cargo does work after the last artifact it announces, and a line
    /// whose parts do not add up to its total is a line nobody can use to
    /// find out where a slow loop went.
    pub(crate) cargo: Duration,
    /// Stopping the old process and putting the new binary somewhere it can
    /// be run from: everything between cargo exiting and the spawn.
    pub(crate) swap: Option<Duration>,
    /// Asking the operating system to start the new binary.
    ///
    /// Its own figure rather than part of `boot`, because it is not the
    /// application's time and the application cannot shorten it. Starting an
    /// executable that was just written costs whatever the machine charges
    /// to read it: with Microsoft Defender watching `target/`, the first run
    /// of a freshly linked 18 MB binary measured 1.4-3.3 s on the machine
    /// this was written on, against 9 ms for the same file run twice. Folded
    /// into `boot` that reads as a slow application, and the developer goes
    /// looking in the wrong place; on its own line it is what it is, and
    /// `arc doctor` says what to do about it.
    pub(crate) spawn: Option<Duration>,
    /// From the new process starting to its first accepted IPC connection.
    pub(crate) boot: Option<Duration>,
    /// The build produced the binary that is already running, so nothing was
    /// restarted.
    ///
    /// Not a duration, because the point of it is the durations that are
    /// absent. It is worth a word on the line rather than silence: a save
    /// that prints only a cargo time and no restart looks like a supervisor
    /// that lost track of the child, and the developer has no way to tell
    /// that apart from the truth, which is that there was nothing to do.
    pub(crate) unchanged: bool,
    /// Reading the graph back out of the freshly booted application and
    /// rewriting `resources/js/generated/`. `None` when the graph could not
    /// be read, which is reported on its own line rather than as a zero.
    pub(crate) typegen: Option<Duration>,
    /// The whole trip, including the parts no stage covers.
    pub(crate) total: Duration,
}

impl std::fmt::Display for Stages {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "cargo {:.2}s", self.cargo.as_secs_f32())?;

        // In brackets, because these two are a breakdown of the number
        // before them rather than two more stages beside it.
        let inner: Vec<String> = [("check", self.check), ("codegen+link", self.codegen_link)]
            .into_iter()
            .filter_map(|(label, value)| {
                value.map(|value| format!("{label} {:.2}s", value.as_secs_f32()))
            })
            .collect();
        if !inner.is_empty() {
            write!(formatter, " ({})", inner.join(", "))?;
        }

        if self.unchanged {
            write!(formatter, "  unchanged")?;
        }
        for (label, value) in [
            ("swap", self.swap),
            ("spawn", self.spawn),
            ("boot", self.boot),
            ("typegen", self.typegen),
        ] {
            let Some(value) = value else { continue };
            write!(formatter, "  {label} {:.2}s", value.as_secs_f32())?;
        }
        write!(formatter, "  total {:.2}s", self.total.as_secs_f32())
    }
}

/// The result of one `cargo build`.
pub(crate) enum Build {
    /// The build succeeded and produced this binary.
    Succeeded {
        executable: PathBuf,
        check: Option<Duration>,
        codegen_link: Option<Duration>,
    },
    /// The build failed. `diagnostics` is what the browser is shown.
    Failed { diagnostics: String },
    /// The build was killed part-way through, because something asked for a
    /// newer one or for the session to end. Not a failure: nothing is wrong,
    /// and there is nothing to show the browser.
    Cancelled,
}

/// The ability to kill the `cargo` process a reload is waiting on.
///
/// A build is the long pole of the loop -- seconds, sometimes tens of them --
/// and two things can happen while it runs that make finishing it pointless:
/// the developer saves again, and the developer presses Ctrl-C. Without a way
/// to interrupt it, the first costs a full wasted build *plus* a restart to a
/// binary already known to be stale, and the second does not work at all
/// until the compiler happens to finish.
///
/// It has to be a handle on the child rather than a flag the build loop
/// polls, because the loop is blocked reading cargo's output: the only thing
/// that unblocks it is cargo exiting.
#[derive(Clone, Default)]
pub(crate) struct Cancel {
    inner: Arc<Mutex<CancelState>>,
}

/// What [`Cancel`] shares between the loop and the blocking build thread.
#[derive(Default)]
struct CancelState {
    /// Set once and never cleared. Read after the build to tell a killed
    /// build from one that failed on its own.
    requested: bool,
    /// The running compiler, while there is one. Held here rather than owned
    /// solely by the build thread so that it can be killed from outside;
    /// `Child::kill` needs only `&mut`, so the build thread can still take it
    /// back to reap it.
    child: Option<std::process::Child>,
}

impl Cancel {
    /// Kill the build in progress, if any, and remember that this happened.
    pub(crate) fn request(&self) {
        let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        state.requested = true;
        if let Some(child) = state.child.as_mut() {
            // A compiler that has already exited is not an error to kill;
            // there is simply nothing left to signal.
            drop(child.kill());
        }
    }

    /// Has a kill been asked for?
    pub(crate) fn requested(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .requested
    }

    /// Hand the compiler over, so it can be killed while it runs.
    ///
    /// Returns `false` if a kill was already requested before the process
    /// existed, in which case the caller must not wait for it.
    fn adopt(&self, child: std::process::Child) -> bool {
        let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        state.child = Some(child);
        if state.requested {
            if let Some(child) = state.child.as_mut() {
                drop(child.kill());
            }
            return false;
        }
        true
    }

    /// Take the compiler back, to reap it.
    fn reclaim(&self) -> Option<std::process::Child> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .child
            .take()
    }
}

/// Run `cargo build --features dev` in `root` and read its message stream.
///
/// Blocking on purpose: it is called from `spawn_blocking`, where waiting on
/// a compiler is what the thread is for.
///
/// # Errors
///
/// [`DevError::Cargo`] if `cargo` cannot be spawned or its output cannot be
/// read. A build that *fails* is not an error here -- it is
/// [`Build::Failed`], because the supervisor keeps running and shows it.
fn run_cargo_build(root: &Path, cancel: &Cancel) -> Result<Build, DevError> {
    let started = Instant::now();
    let mut child = Command::new("cargo")
        .args(["build", "--features", "dev", "--message-format", "json"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // Piped rather than inherited, and then echoed line by line. The
        // terminal still gets cargo's progress as it happens, and the tail
        // is kept so that a failure cargo reports here -- a broken manifest,
        // an unwritable artifact -- can be shown in the browser instead of a
        // page that says to go and look somewhere else.
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| DevError::Cargo { source })?;

    let stdout = child.stdout.take().ok_or_else(|| DevError::Cargo {
        source: std::io::Error::other("cargo produced no message stream"),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| DevError::Cargo {
        source: std::io::Error::other("cargo produced no error stream"),
    })?;
    // From here on the process belongs to the cancel handle, which is the
    // only thing that can end this function early -- everything below blocks
    // until cargo closes its streams.
    let live = cancel.adopt(child);

    let tail = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&tail);
    let pump = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            eprintln!("{line}");
            let mut lines = sink.lock().unwrap_or_else(PoisonError::into_inner);
            if lines.len() == STDERR_TAIL {
                lines.remove(0);
            }
            lines.push(line);
        }
    });

    let mut stream = MessageStream::default();
    for line in BufReader::new(stdout).lines() {
        // A killed compiler closes its pipes mid-line; that is an expected
        // end to this loop, not a failure to report.
        let Ok(line) = line else { break };
        stream.absorb(&line);
    }

    let status = match cancel.reclaim() {
        Some(mut child) => child.wait().map_err(|source| DevError::Cargo { source })?,
        // `adopt` refused it and killed it, or something else reclaimed it.
        // Either way there is no status to read and no build to report.
        None => {
            drop(pump.join());
            return Ok(Build::Cancelled);
        }
    };
    // Joining after the wait, not before: the pump ends when cargo closes
    // its stderr, which it does when it exits.
    drop(pump.join());
    if !live || cancel.requested() {
        return Ok(Build::Cancelled);
    }
    let tail = tail
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .join("\n");
    Ok(stream.finish(started, status, &tail))
}

/// Cargo's JSON message stream, reduced to the four facts the loop needs.
///
/// Separated from the process handling so it can be tested against recorded
/// cargo output instead of against a compiler.
#[derive(Default)]
struct MessageStream {
    diagnostics: String,
    executable: Option<PathBuf>,
    /// When the last non-executable artifact appeared: the type-check
    /// boundary.
    last_metadata: Option<Instant>,
    /// When the executable appeared: link completion.
    linked: Option<Instant>,
}

impl MessageStream {
    /// Take one line of cargo's output into account. Lines that are not
    /// JSON, or are JSON this does not care about, are ignored -- cargo is
    /// free to add message kinds.
    fn absorb(&mut self, line: &str) {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            return;
        };
        match message.get("reason").and_then(serde_json::Value::as_str) {
            Some("compiler-message") => {
                let Some(rendered) = message
                    .pointer("/message/rendered")
                    .and_then(serde_json::Value::as_str)
                else {
                    return;
                };
                // The terminal gets each diagnostic as it arrives; the
                // browser gets them together if the build ends up failing.
                eprint!("{rendered}");
                self.diagnostics.push_str(rendered);
            }
            Some("compiler-artifact") => {
                // A cached crate is reported the instant cargo decides it is
                // cached, which says nothing about how long anything took --
                // so a fresh artifact contributes no timing. It does still
                // name the binary, and that part is not optional: an edit
                // that leaves the crate byte-identical to a build cargo
                // already has (undo, or a change confined to a file nothing
                // includes) makes *every* artifact fresh, and dropping the
                // path there would report a successful build as "produced no
                // executable" and leave the old process running behind an
                // error page.
                let fresh = message.get("fresh").and_then(serde_json::Value::as_bool) == Some(true);
                match message
                    .get("executable")
                    .and_then(serde_json::Value::as_str)
                {
                    Some(path) => {
                        self.executable = Some(PathBuf::from(path));
                        if !fresh {
                            self.linked = Some(Instant::now());
                        }
                    }
                    None => {
                        if !fresh {
                            self.last_metadata = Some(Instant::now());
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl MessageStream {
    /// Turn what was observed into a build outcome.
    ///
    /// `stderr` is the tail of what cargo printed, used only when the JSON
    /// stream carried no diagnostic of its own.
    ///
    /// A non-zero exit is always a failure. A zero exit with no executable is
    /// also a failure: it means cargo built something, but not a binary this
    /// can run, and pretending otherwise would hang the boot wait until it
    /// timed out with a far less useful message.
    fn finish(self, started: Instant, status: std::process::ExitStatus, stderr: &str) -> Build {
        let Self {
            mut diagnostics,
            executable,
            last_metadata,
            linked,
        } = self;

        if !status.success() {
            if diagnostics.trim().is_empty() {
                // cargo can fail before it ever reaches the compiler -- a
                // broken manifest, a missing feature, a binary it cannot
                // overwrite. None of those is a `compiler-message`, so the
                // JSON stream carries nothing and the browser would get a
                // blank page. What cargo said on stderr is exactly the
                // reason, so show that.
                diagnostics.push_str(if stderr.trim().is_empty() {
                    "cargo failed without saying why. Look at the terminal \
                     running `arc dev` for the reason."
                } else {
                    stderr
                });
            }
            return Build::Failed { diagnostics };
        }

        let Some(executable) = executable else {
            return Build::Failed {
                diagnostics: String::from(
                    "cargo succeeded but produced no executable. \
                     `arc dev` needs a binary target -- check that \
                     src/main.rs exists and that the package is not \
                     library-only.",
                ),
            };
        };

        // Both stages are optional on purpose: a fully cached build reports
        // nothing but the executable, and a timing that was never observed is
        // more honest as absent than as zero.
        let check = last_metadata.map(|at| at.duration_since(started));
        let codegen_link = match (last_metadata, linked) {
            (Some(metadata), Some(linked)) => linked.checked_duration_since(metadata),
            (None, Some(linked)) => Some(linked.duration_since(started)),
            _ => None,
        };

        Build::Succeeded {
            executable,
            check,
            codegen_link,
        }
    }
}

/// What one restart cost, split where the costs have different owners.
struct Restart {
    /// Stopping the old process and staging the new binary.
    swap: Duration,
    /// The `CreateProcess`/`fork` itself. See [`Stages::spawn`].
    spawn: Duration,
    /// The new process's own startup, spawn to listening.
    boot: Duration,
}

/// Where the copy for this generation goes.
///
/// The name carries the supervisor's process id as well as the generation,
/// for the same reason the IPC endpoints do: two `arc dev` runs on one
/// project must not pick the same file, and a run that was killed rather
/// than stopped leaves its copies behind for the next one to trip over --
/// on Windows a file another process is still holding cannot be replaced,
/// so a colliding name is not a stale file, it is a failed rebuild.
fn staged_path(executable: &Path, generation: u32) -> PathBuf {
    let directory = executable
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(STAGE_DIR);
    let stem = executable
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("app");
    let mut name = format!("{stem}-{}-{generation}", std::process::id());
    if let Some(extension) = executable.extension().and_then(std::ffi::OsStr::to_str) {
        name.push('.');
        name.push_str(extension);
    }
    directory.join(name)
}

/// Put the binary somewhere it can be run without pinning cargo's output.
///
/// Returns `None` where no copy is needed, which is every platform but
/// Windows -- the caller then runs cargo's own file. See the module
/// documentation for why the split exists rather than one uniform copy.
///
/// # Errors
///
/// Whatever creating the directory or copying the file reported.
fn stage(executable: &Path, generation: u32) -> Result<Option<PathBuf>, std::io::Error> {
    if !cfg!(windows) {
        return Ok(None);
    }
    let destination = staged_path(executable, generation);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
        sweep(parent);
    }
    std::fs::copy(executable, &destination)?;
    Ok(Some(destination))
}

/// A number that changes when the contents of `path` change.
///
/// Used for one decision only: whether the binary cargo just produced is the
/// one already running, in which case stopping a working process to start an
/// identical one buys nothing and costs the most expensive stage of the loop.
/// That happens more often than it sounds -- an editor that saves on focus
/// loss, a formatter that rewrites a file to what it already said, an undo
/// back to the last build -- and each time it did, the loop paid seconds to
/// deliver a binary the developer already had.
///
/// Not a cryptographic hash and not a stable one: it is compared only against
/// another number produced by the same process in the same run, and an
/// adversary who can write into `target/` has already won.
///
/// # Errors
///
/// Whatever reading the file reported. The caller treats that as "assume it
/// changed", which is the safe direction.
fn digest(path: &Path) -> Result<u64, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = std::hash::DefaultHasher::new();
    // A binary is tens of megabytes; reading it whole to hash it would
    // allocate that much to throw it away.
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hasher.finish());
        }
        hasher.write(&buffer[..read]);
    }
}

/// Delete copies left behind by runs that are not this one.
///
/// Best effort, and deliberately so: a file that will not delete is one
/// something still holds, which is exactly the file that must not be
/// touched. The name is the whole check -- anything belonging to this
/// process is left alone, because the running child is in here too.
fn sweep(directory: &Path) {
    let marker = format!("-{}-", std::process::id());
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.contains(&marker) {
            drop(std::fs::remove_file(entry.path()));
        }
    }
}

/// The application child process and everything needed to replace it.
///
/// Owns the child exclusively. Nothing else in the supervisor may spawn or
/// kill it, which is what makes "the TCP listener and Vite never die" a
/// property of the code rather than a convention.
pub(crate) struct Backend {
    root: PathBuf,
    endpoints: Endpoints,
    sentinel: PathBuf,
    handle: BackendHandle,
    /// `None` before the first successful build, and between stopping the
    /// old child and the new one listening.
    child: Option<ChildGuard>,
    /// The copy the running child was started from, where one was made.
    /// Kept so it can be deleted once that child is gone -- see [`stage`].
    staged: Option<PathBuf>,
    /// Bumped on every restart, so each staged copy gets a name that no
    /// process is holding open.
    generation: u32,
    /// Cargo's own output path from the last successful build.
    ///
    /// Kept so that a change which needs a restart but not a compile -- an
    /// edited `.env` -- has something to start without asking cargo whether
    /// the world moved.
    built: Option<PathBuf>,
    /// [`digest`] of the binary the running child was started from.
    built_digest: Option<u64>,
    /// Whether the schema has been applied in this supervisor's lifetime.
    ///
    /// One run, not one per restart: migrations are idempotent but not free,
    /// and paying for them on every rebuild would put seconds into the loop
    /// this command exists to keep short.
    migrated: bool,
}

impl Backend {
    /// Prepare a backend for `root`. Nothing is built or spawned yet.
    pub(crate) fn new(
        root: PathBuf,
        endpoints: Endpoints,
        sentinel: PathBuf,
        handle: BackendHandle,
    ) -> Self {
        Self {
            root,
            endpoints,
            sentinel,
            handle,
            child: None,
            staged: None,
            generation: 0,
            built: None,
            built_digest: None,
            migrated: false,
        }
    }

    /// Build the application and put the result in front of the supervisor.
    ///
    /// The status is moved to building first, so a request that arrives
    /// during the build waits instead of reaching a process that is about to
    /// be killed. A failed build leaves the old child running: it can still
    /// answer, and the browser gets the diagnostics either way.
    ///
    /// # Errors
    ///
    /// [`DevError::Cargo`] if cargo could not be run at all, [`DevError::Spawn`]
    /// if the built binary could not be started, [`DevError::Wait`] if it
    /// never listened.
    pub(crate) async fn reload(&mut self, cancel: &Cancel) -> Result<Reload, DevError> {
        let started = Instant::now();
        self.handle.mark_building();

        let root = self.root.clone();
        let handle = cancel.clone();
        let building = Instant::now();
        let build = tokio::task::spawn_blocking(move || run_cargo_build(&root, &handle))
            .await
            .map_err(|error| DevError::Cargo {
                source: std::io::Error::other(error.to_string()),
            })??;
        let cargo = building.elapsed();

        let (executable, check, codegen_link) = match build {
            Build::Succeeded {
                executable,
                check,
                codegen_link,
            } => (executable, check, codegen_link),
            Build::Failed { diagnostics } => {
                self.handle.mark_failed(diagnostics);
                return Ok(Reload::Done(Stages {
                    cargo,
                    total: started.elapsed(),
                    ..Stages::default()
                }));
            }
            // The status is left as it was found -- building, with the old
            // child still answering. The caller is either about to start a
            // newer build, which will set it, or about to exit.
            Build::Cancelled => return Ok(Reload::Cancelled),
        };

        // Cheaper than the restart it can avoid by three orders of magnitude,
        // so it is worth paying on every reload rather than guessing when it
        // might pay off. A read that fails means "assume it changed".
        let fingerprint = digest(&executable).ok();
        self.built = Some(executable.clone());
        if self.child.is_some() && fingerprint.is_some() && fingerprint == self.built_digest {
            self.handle.mark_ready();
            return Ok(Reload::Done(Stages {
                check,
                codegen_link,
                cargo,
                unchanged: true,
                total: started.elapsed(),
                ..Stages::default()
            }));
        }

        let Restart { swap, spawn, boot } = self.restart(&executable).await?;
        self.built_digest = fingerprint;
        self.handle.mark_ready();

        // Before the sentinel, not after: the reload the sentinel triggers is
        // what loads the generated TypeScript, so writing it afterwards would
        // put the browser one restart behind. The application is already
        // answering by this point, so the wait costs the reload, not the
        // rebuild.
        let typegen = self.regenerate().await;

        // Only now: the sentinel tells Vite to reload the page, and a reload
        // before the backend answers would just show the holding page.
        if let Err(error) = project::touch_sentinel(&self.sentinel) {
            eprintln!("warning: could not signal the browser to reload: {error}");
        }

        Ok(Reload::Done(Stages {
            check,
            codegen_link,
            cargo,
            swap: Some(swap),
            spawn: Some(spawn),
            boot: Some(boot),
            typegen,
            total: started.elapsed(),
            unchanged: false,
        }))
    }

    /// Start the last binary that was built again, without compiling.
    ///
    /// For a change that alters what the process reads at boot rather than
    /// what the compiler reads -- `.env`, today. Compiling would take seconds
    /// to arrive at the file already sitting in `target/`.
    ///
    /// Falls back to a full [`reload`](Self::reload) when there is no
    /// previous build to start, which is the case if the very first one
    /// failed.
    ///
    /// # Errors
    ///
    /// As [`reload`](Self::reload).
    pub(crate) async fn restart_only(&mut self, cancel: &Cancel) -> Result<Reload, DevError> {
        let Some(executable) = self.built.clone() else {
            return self.reload(cancel).await;
        };
        let started = Instant::now();
        self.handle.mark_building();

        let Restart { swap, spawn, boot } = self.restart(&executable).await?;
        self.handle.mark_ready();

        let typegen = self.regenerate().await;
        if let Err(error) = project::touch_sentinel(&self.sentinel) {
            eprintln!("warning: could not signal the browser to reload: {error}");
        }

        Ok(Reload::Done(Stages {
            swap: Some(swap),
            spawn: Some(spawn),
            boot: Some(boot),
            typegen,
            total: started.elapsed(),
            ..Stages::default()
        }))
    }
}

/// What one trip round the loop came to.
pub(crate) enum Reload {
    /// It ran to the end; here is what each stage cost.
    Done(Stages),
    /// Something asked for a newer build, or for the session to end, before
    /// this one finished. Nothing was restarted and nothing is broken.
    Cancelled,
}

impl Backend {
    /// Rewrite `resources/js/generated/` from the application that just
    /// started, and report how long it took.
    ///
    /// Returns `None` on any failure, having printed it. Nothing here is
    /// worth stopping a working development server for: the backend is up,
    /// the browser is about to reload, and a stale `routes.ts` is a smaller
    /// problem than no server at all. The one failure worth the developer's
    /// attention -- a graph that does not validate -- prints its diagnostics
    /// in full, which is the same text `arc typegen` would have shown.
    async fn regenerate(&self) -> Option<Duration> {
        let started = Instant::now();
        match super::codegen::regenerate(&self.root, &self.endpoints.app).await {
            Ok(written) if written.is_empty() => Some(started.elapsed()),
            Ok(written) => {
                for path in written {
                    println!("  typegen {}", path.display());
                }
                Some(started.elapsed())
            }
            Err(error) => {
                eprintln!("  typegen failed: {error}");
                None
            }
        }
    }

    /// Stop the current child, start `executable`, and wait for it to listen.
    ///
    /// Stopping first is deliberate: the endpoint is a single name, and two
    /// processes racing for it would produce a backend nobody can predict.
    /// The gap is covered by the queue in
    /// [`crate::cli::commands::dev::service`], which is why this is allowed
    /// to be a gap at all.
    ///
    /// The copy is taken *before* the old child is stopped, so a failure to
    /// make it leaves a working server running rather than none.
    /// Apply the project's schema by running its own binary in `--migrate`
    /// mode, once.
    ///
    /// Reported and not propagated. A project may legitimately have no
    /// database configured, and a migration that fails still leaves a
    /// supervisor that can serve every route which does not touch the schema
    /// -- refusing to start would take that away and tell the developer less
    /// than the migrator's own output already did.
    async fn migrate(&self, program: &Path) {
        let mut command = inherited(&program.display().to_string());
        command.arg("--migrate").current_dir(&self.root);
        // `spawn_blocking` with `std::process`, not `tokio::process`: the
        // latter is behind a tokio feature this crate does not turn on, and
        // stopping the previous child a few lines below already takes this
        // shape.
        let outcome = tokio::task::spawn_blocking(move || command.status()).await;
        match outcome {
            Ok(Ok(status)) if status.success() => {}
            Ok(Ok(status)) => {
                eprintln!("warning: `--migrate` exited with {status}; the schema may be behind");
            }
            Ok(Err(error)) => {
                eprintln!("warning: could not run migrations: {error}");
            }
            Err(error) => {
                eprintln!("warning: the migration task failed: {error}");
            }
        }
    }

    async fn restart(&mut self, executable: &Path) -> Result<Restart, DevError> {
        let swapping = Instant::now();
        self.generation = self.generation.wrapping_add(1);
        let staged =
            stage(executable, self.generation).map_err(|source| DevError::Stage { source })?;

        if let Some(mut previous) = self.child.take() {
            tokio::task::spawn_blocking(move || previous.stop())
                .await
                .map_err(|error| DevError::Spawn {
                    program: String::from("the previous application process"),
                    source: std::io::Error::other(error.to_string()),
                })?;
        }
        // Only now that nothing is holding it. Best effort on purpose: a
        // copy that outlives its process is a few megabytes inside
        // `target/`, which `cargo clean` removes and nobody has to notice.
        self.discard_staged();
        self.staged = staged;

        let program = self
            .staged
            .as_deref()
            .unwrap_or(executable)
            .display()
            .to_string();
        let mut command = inherited(&program);
        command
            .current_dir(&self.root)
            // stdin is left as `inherited` set it: a pipe held open by the
            // supervisor, whose closing tells the application it was orphaned.
            // See the header of `child.rs`.
            // Where to listen instead of binding a port. Unset in production,
            // which is how the same binary keeps its TCP address there.
            .env(crate::config::APP_IPC_ENV, &self.endpoints.app)
            // The application resolves asset entries differently under a live
            // Vite, so it needs to know one is running even though it is the
            // supervisor, not the application, that talks to it.
            .env(crate::config::VITE_IPC_ENV, &self.endpoints.vite);

        let swap = swapping.elapsed();

        // Before the first spawn of this session. A freshly generated project
        // has no tables at all, and the session store is read on the first
        // request that carries a CSRF token -- which is every request the
        // scaffold serves. Without this, `arc dev` on a new project starts,
        // reports ready, opens a browser and fails on the first page.
        //
        // `bootstrap/app.rs` says the schema is applied by `--migrate` rather
        // than at boot, and that reasoning holds: a deploy that migrates as a
        // side effect of starting has every replica racing. A single dev
        // supervisor on a developer's machine is not that, and it is the only
        // caller here.
        if !self.migrated {
            self.migrate(self.staged.as_deref().unwrap_or(executable))
                .await;
            self.migrated = true;
        }

        let spawning = Instant::now();
        let mut child = ChildGuard::spawn("application", &mut command)
            .map_err(|source| DevError::Spawn { program, source })?;
        let spawn = spawning.elapsed();

        let waited = endpoints::wait_until_listening(&self.endpoints.app, BOOT_TIMEOUT, || {
            child
                .exited()
                .map(|status| format!("the application exited with {status}"))
        })
        .await;

        match waited {
            Ok(boot) => {
                self.child = Some(child);
                Ok(Restart { swap, spawn, boot })
            }
            Err(error) => {
                // The child is dropped here, which kills it: a process that
                // never listened is not one to leave running.
                Err(DevError::Wait {
                    source: Box::new(error),
                })
            }
        }
    }

    /// Stop the child, if there is one, and clean up after it. Called on
    /// shutdown.
    pub(crate) fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.stop();
        }
        self.discard_staged();
    }

    /// Delete the staged copy, if one was made and nothing holds it.
    ///
    /// Silent on failure: the only way this fails is that the process
    /// started from it has not finished dying, and the cost of that is one
    /// stale file in a build directory.
    fn discard_staged(&mut self) {
        if let Some(path) = self.staged.take() {
            drop(std::fs::remove_file(path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recorded cargo artifact line, so the parser is tested without a
    /// compiler in the loop.
    pub(super) fn artifact(executable: Option<&str>, fresh: bool) -> String {
        let executable = match executable {
            Some(path) => format!("\"{path}\""),
            None => String::from("null"),
        };
        format!(r#"{{"reason":"compiler-artifact","fresh":{fresh},"executable":{executable}}}"#)
    }

    pub(super) fn exit_status(success: bool) -> std::process::ExitStatus {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", if success { "exit 0" } else { "exit 1" }]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", if success { "exit 0" } else { "exit 1" }]);
            command
        };
        command
            .status()
            .expect("a shell should be available to produce an exit status")
    }

    #[test]
    fn a_build_that_produced_no_binary_is_a_failure_even_when_cargo_is_happy() {
        let mut stream = MessageStream::default();
        stream.absorb(&artifact(None, false));

        match stream.finish(Instant::now(), exit_status(true), "") {
            Build::Failed { diagnostics } => {
                assert!(
                    diagnostics.contains("no executable"),
                    "the page should say what is missing, got: {diagnostics}"
                );
            }
            Build::Succeeded { .. } => panic!("no binary was produced, so nothing can be run"),
            Build::Cancelled => unreachable!("a parsed build was never asked to stop"),
        }
    }

    #[test]
    fn a_failure_with_nothing_on_stdout_still_says_something_useful() {
        match MessageStream::default().finish(Instant::now(), exit_status(false), "") {
            Build::Failed { diagnostics } => assert!(
                diagnostics.contains("terminal"),
                "an empty failure should point at the terminal, got: {diagnostics}"
            ),
            Build::Succeeded { .. } => panic!("a non-zero exit is never a success"),
            Build::Cancelled => unreachable!("a parsed build was never asked to stop"),
        }
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::tests::{artifact, exit_status};
    use super::*;

    #[test]
    fn a_cached_build_is_not_counted_as_time_spent_checking() {
        let mut stream = MessageStream::default();
        // Fresh crates are reported instantly and in bulk; letting them move
        // the check boundary would report a cached build as slow.
        stream.absorb(r#"{"reason":"compiler-artifact","fresh":true,"executable":null}"#);
        stream.absorb(r#"{"reason":"compiler-artifact","fresh":false,"executable":"app"}"#);

        match stream.finish(Instant::now(), exit_status(true), "") {
            Build::Succeeded { check, .. } => assert!(
                check.is_none(),
                "nothing but the binary was rebuilt, so there is no check stage to report"
            ),
            Build::Failed { .. } => panic!("the build produced a binary"),
            Build::Cancelled => unreachable!("a parsed build was never asked to stop"),
        }
    }

    #[test]
    fn a_diagnostic_is_kept_verbatim_for_the_browser() {
        let mut stream = MessageStream::default();
        stream.absorb(
            r#"{"reason":"compiler-message","message":{"rendered":"error[E0308]: mismatched types\n"}}"#,
        );

        match stream.finish(Instant::now(), exit_status(false), "") {
            Build::Failed { diagnostics } => assert!(
                diagnostics.contains("E0308"),
                "the browser needs the compiler's own words, got: {diagnostics}"
            ),
            Build::Succeeded { .. } => panic!("a non-zero exit is never a success"),
            Build::Cancelled => unreachable!("a parsed build was never asked to stop"),
        }
    }

    #[test]
    fn a_line_that_is_not_json_is_ignored_rather_than_fatal() {
        let mut stream = MessageStream::default();
        stream.absorb("   Compiling arcature v0.1.0");
        stream.absorb(r#"{"reason":"build-finished","success":true}"#);
        stream.absorb(&artifact(Some("target/debug/app"), false));

        match stream.finish(Instant::now(), exit_status(true), "") {
            Build::Succeeded { executable, .. } => {
                assert_eq!(executable, PathBuf::from("target/debug/app"));
            }
            Build::Failed { .. } => panic!("cargo succeeded and named a binary"),
            Build::Cancelled => unreachable!("a parsed build was never asked to stop"),
        }
    }

    #[test]
    fn a_build_cargo_had_nothing_to_do_for_still_names_the_binary_to_run() {
        // Undo an edit and cargo reports every artifact `fresh`. That is a
        // successful build of the binary already on disk, not a build that
        // produced nothing -- treating it as the latter leaves the developer
        // looking at a compile-error page for a project that compiles.
        let mut stream = MessageStream::default();
        stream.absorb(&artifact(Some("target/debug/app"), true));

        match stream.finish(Instant::now(), exit_status(true), "") {
            Build::Succeeded {
                executable,
                check,
                codegen_link,
            } => {
                assert_eq!(executable, PathBuf::from("target/debug/app"));
                // Nothing was compiled, so there is no compile time to claim.
                assert_eq!(check, None);
                assert_eq!(codegen_link, None);
            }
            Build::Failed { diagnostics } => {
                panic!("a fully cached build is a success: {diagnostics}")
            }
            Build::Cancelled => unreachable!("a parsed build was never asked to stop"),
        }
    }

    #[test]
    fn stages_that_were_never_observed_are_left_out_of_the_line() {
        let printed = Stages {
            cargo: Duration::from_millis(1_400),
            swap: Some(Duration::from_millis(200)),
            total: Duration::from_millis(1_950),
            ..Stages::default()
        }
        .to_string();

        assert_eq!(printed, "cargo 1.40s  swap 0.20s  total 1.95s");
    }

    #[test]
    fn the_parts_of_the_line_add_up_to_the_total_it_reports() {
        // The line exists to say where a slow loop went. A breakdown that
        // leaves a third of the time unnamed cannot do that, which is what
        // the cargo and swap figures are here to prevent.
        let printed = Stages {
            check: Some(Duration::from_millis(2_730)),
            codegen_link: Some(Duration::from_millis(5_060)),
            cargo: Duration::from_millis(7_850),
            swap: Some(Duration::from_millis(500)),
            spawn: Some(Duration::from_millis(5_000)),
            boot: Some(Duration::from_millis(40)),
            typegen: Some(Duration::from_millis(20)),
            total: Duration::from_millis(13_430),
            unchanged: false,
        }
        .to_string();

        assert_eq!(
            printed,
            concat!(
                "cargo 7.85s (check 2.73s, codegen+link 5.06s)",
                "  swap 0.50s  spawn 5.00s  boot 0.04s  typegen 0.02s  total 13.43s",
            )
        );
    }

    #[test]
    fn a_build_that_changed_nothing_says_so_instead_of_showing_an_empty_line() {
        // Without the word, this line is a cargo time and a total and no
        // explanation of where the restart went.
        let printed = Stages {
            cargo: Duration::from_millis(6_900),
            unchanged: true,
            total: Duration::from_millis(6_920),
            ..Stages::default()
        }
        .to_string();

        assert_eq!(printed, "cargo 6.90s  unchanged  total 6.92s");
    }

    #[test]
    fn two_identical_files_have_the_same_digest_and_a_changed_one_does_not() {
        // This is the whole basis for skipping a restart, so it is the whole
        // thing worth pinning down.
        let directory = std::env::temp_dir().join(format!("arc-digest-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a temp directory");
        let (one, two, three) = (
            directory.join("one"),
            directory.join("two"),
            directory.join("three"),
        );
        // Larger than the read buffer, so the chunking is exercised too.
        let body = vec![7_u8; 200 * 1024];
        std::fs::write(&one, &body).expect("write");
        std::fs::write(&two, &body).expect("write");
        let mut changed = body.clone();
        changed[199 * 1024] = 8;
        std::fs::write(&three, &changed).expect("write");

        assert_eq!(digest(&one).expect("read"), digest(&two).expect("read"));
        assert_ne!(digest(&one).expect("read"), digest(&three).expect("read"));
        drop(std::fs::remove_dir_all(&directory));
    }

    #[test]
    fn a_digest_of_something_unreadable_is_an_error_not_a_number() {
        // The caller reads this as "assume it changed"; a fabricated number
        // would read as "assume it did not", which is the unsafe direction.
        assert!(digest(Path::new("no-such-file-anywhere")).is_err());
    }

    #[test]
    fn a_cancel_asked_for_before_the_compiler_exists_still_stops_it() {
        // The build thread spawns cargo and only then hands it over; a Ctrl-C
        // in that window must not be forgotten, or the loop waits out a build
        // nobody wants.
        let cancel = Cancel::default();
        assert!(!cancel.requested());
        cancel.request();
        assert!(cancel.requested());

        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "exit 0"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "exit 0"]);
            command
        };
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let child = command.spawn().expect("spawn");

        assert!(
            !cancel.adopt(child),
            "adopting into an already-cancelled handle must refuse"
        );
        drop(cancel.reclaim().map(|mut child| child.wait()));
    }

    #[test]
    fn cargo_s_own_words_are_what_the_browser_is_shown() {
        // The failure that motivated this: cargo cannot overwrite a running
        // binary, says so on stderr, and emits no `compiler-message` at all.
        // A page that answered "look at the terminal" would be hiding the
        // one line that explains everything.
        let stderr = "error: failed to remove file `target/debug/demo.exe`";

        match MessageStream::default().finish(Instant::now(), exit_status(false), stderr) {
            Build::Failed { diagnostics } => assert!(
                diagnostics.contains("failed to remove file"),
                "the reason cargo gave should reach the page, got: {diagnostics}"
            ),
            Build::Succeeded { .. } => panic!("a non-zero exit is never a success"),
            Build::Cancelled => unreachable!("a parsed build was never asked to stop"),
        }
    }

    #[test]
    fn a_compiler_diagnostic_is_preferred_over_the_stderr_tail() {
        let mut stream = MessageStream::default();
        stream.absorb(
            r#"{"reason":"compiler-message","message":{"rendered":"error[E0425]: cannot find value"}}"#,
        );

        match stream.finish(
            Instant::now(),
            exit_status(false),
            "    Compiling demo v0.1.0",
        ) {
            Build::Failed { diagnostics } => {
                assert!(diagnostics.contains("E0425"), "got: {diagnostics}");
                assert!(
                    !diagnostics.contains("Compiling"),
                    "progress noise does not belong under a real diagnostic, got: {diagnostics}"
                );
            }
            Build::Succeeded { .. } => panic!("a non-zero exit is never a success"),
            Build::Cancelled => unreachable!("a parsed build was never asked to stop"),
        }
    }

    #[test]
    fn each_staged_copy_gets_a_name_of_its_own() {
        let executable = Path::new("target/debug/demo.exe");

        let first = staged_path(executable, 1);
        let second = staged_path(executable, 2);

        assert_ne!(
            first, second,
            "a copy is made while the previous one may still be running"
        );
        assert_eq!(
            first.extension().and_then(std::ffi::OsStr::to_str),
            Some("exe"),
            "Windows will not execute a file that is not named like one"
        );
        assert_eq!(first.parent(), Some(Path::new("target/debug/.arc-dev")));
    }

    #[test]
    fn a_copy_is_named_after_the_supervisor_that_made_it() {
        // Two `arc dev` runs on one project, or one run after another was
        // killed rather than stopped: without the process id in the name
        // the second run tries to overwrite a file the first is executing,
        // which on Windows is not a stale file to clean up but a rebuild
        // that fails and keeps failing.
        let staged = staged_path(Path::new("target/debug/demo.exe"), 1);
        let name = staged
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("the copy is named");

        assert!(
            name.contains(&format!("-{}-", std::process::id())),
            "the name should carry this process id, got: {name}"
        );
    }

    #[test]
    fn the_sweep_keeps_this_run_s_copies_and_only_those() {
        let directory = std::env::temp_dir().join("arcature-sweep-test");
        drop(std::fs::remove_dir_all(&directory));
        std::fs::create_dir_all(&directory).expect("a temp directory");

        let mine = staged_path(&directory.join("demo.exe"), 3);
        let mine = directory.join(mine.file_name().expect("a file name"));
        let theirs = directory.join("demo-999999-1.exe");
        std::fs::write(&mine, b"mine").expect("write");
        std::fs::write(&theirs, b"theirs").expect("write");

        sweep(&directory);

        assert!(mine.exists(), "the running child's own copy must survive");
        assert!(!theirs.exists(), "another run's leftovers are litter");
        drop(std::fs::remove_dir_all(&directory));
    }

    #[test]
    fn a_binary_without_an_extension_stays_without_one() {
        let staged = staged_path(Path::new("target/debug/demo"), 7);

        assert_eq!(
            staged,
            PathBuf::from(format!(
                "target/debug/.arc-dev/demo-{}-7",
                std::process::id()
            ))
        );
    }
}
