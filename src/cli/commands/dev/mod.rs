//! `arc dev` -- the development supervisor.
//!
//! One TCP port for the whole development session. The supervisor binds it,
//! and nothing else does: Vite runs in middleware mode on an IPC endpoint,
//! the application runs as a child process on a second IPC endpoint, and
//! every browser request arrives here first and is forwarded to whichever of
//! the two owns it.
//!
//! ```text
//! browser --TCP:1183--> arc dev
//!                         |-- Vite's? --> vite IPC   (ARCATURE_VITE_IPC)
//!                         `-- otherwise -> app IPC   (ARCATURE_APP_IPC)
//! ```
//!
//! # Why this is not the same as `cargo run --features dev`
//!
//! It is the inverse. Under `cargo run --features dev` the application owns
//! the TCP port and forwards Vite's requests to Vite; here the supervisor
//! owns it and forwards to both. Both topologies answer the same request the
//! same way, because both run the same forwarding code -- see
//! [`service::Supervisor`], which is literally
//! `DevProxyLayer::layer(BackendService)`.
//!
//! The reason to have the second topology at all is that the process holding
//! the port is then not the process being rebuilt. A rebuild replaces only
//! the application child: the listener stays bound, so a request that arrives
//! mid-rebuild is queued rather than refused, and Vite's HMR socket -- which
//! is a connection through that listener -- is never dropped.
//!
//! # What the supervisor deliberately does not do
//!
//! It does not know what a Vite request is, it does not parse HTTP beyond
//! what forwarding requires, and it holds no route table. Those live in
//! [`crate::dev_proxy`] and are reused rather than restated, so that the two
//! topologies cannot drift apart.

mod backend;
mod child;
mod codegen;
mod endpoints;
mod pages;
pub(crate) mod project;
mod vite;
mod watch;

// Public because the request path is the part worth testing from outside:
// `tests/dev_supervisor.rs` drives a real TCP listener through
// [`service::Supervisor`] with real IPC children behind it. The rest of the
// supervisor -- process spawning, the watcher, the scratch directory -- is
// its own business.
pub mod service;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use child::{ChildGuard, inherited};
use endpoints::Endpoints;
use project::Project;
use service::{BackendHandle, Supervisor};

/// How long to wait for Vite to start listening on its endpoint.
///
/// Shorter than the application's boot timeout: Vite has no database to
/// reach, so a slow start here is a broken install rather than a busy one.
const VITE_TIMEOUT: Duration = Duration::from_secs(30);

/// A cause that is real but whose type is an implementation detail.
///
/// The supervisor's internals are private modules; naming one of their error
/// types in a public enum would publish it. Boxing keeps the cause -- and
/// therefore [`std::error::Error::source`] -- without widening the crate's
/// surface.
type Cause = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Everything that can stop `arc dev` before or during a session.
#[derive(Debug)]
pub enum DevError {
    /// The working directory is not an Arcature project.
    Project(Cause),
    /// `node` is not on the path, so Vite cannot be started.
    NodeMissing,
    /// The frontend's npm dependencies are not installed.
    Frontend(super::install::InstallError),
    /// The Vite entry script could not be written to the scratch directory.
    Script(std::io::Error),
    /// A child process could not be started.
    Spawn {
        /// What was being started, for the message.
        program: String,
        /// Why it could not be.
        source: std::io::Error,
    },
    /// A child started but never listened on its endpoint.
    Wait {
        /// Which endpoint went unanswered, and why.
        source: Cause,
    },
    /// The one TCP port could not be bound.
    Bind {
        /// The address that was asked for.
        address: SocketAddr,
        /// Why it was refused.
        source: std::io::Error,
    },
    /// The file watcher could not be started, so rebuilds would never fire.
    Watch(notify::Error),
    /// `cargo` could not be run, or its output could not be read.
    Cargo {
        /// The underlying failure.
        source: std::io::Error,
    },
    /// The freshly built binary could not be copied aside to be run from.
    Stage {
        /// Why the copy failed.
        source: std::io::Error,
    },
    /// The supervisor's own listener failed while serving.
    Serve(std::io::Error),
}

impl std::fmt::Display for DevError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project(source) => write!(formatter, "{source}"),
            Self::NodeMissing => write!(
                formatter,
                "`node` was not found on PATH. `arc dev` runs Vite as a child \
                 process, so Node.js has to be installed."
            ),
            Self::Frontend(source) => write!(formatter, "{source}"),
            Self::Script(source) => write!(
                formatter,
                "could not write the Vite entry script into .arcature: {source}"
            ),
            Self::Spawn { program, source } => {
                write!(formatter, "could not start {program}: {source}")
            }
            Self::Wait { source } => write!(formatter, "{source}"),
            Self::Bind { address, source } => write!(
                formatter,
                "could not bind {address}: {source}. This is the only TCP port \
                 `arc dev` uses; pass --port to choose another one."
            ),
            Self::Watch(source) => write!(
                formatter,
                "could not watch the project for changes: {source}"
            ),
            Self::Cargo { source } => write!(formatter, "could not run cargo: {source}"),
            Self::Stage { source } => write!(
                formatter,
                "could not copy the built binary aside to run it: {source}. \
                 The copy is what lets the next rebuild replace cargo's own \
                 output while the current process is still running."
            ),
            Self::Serve(source) => write!(formatter, "the development server stopped: {source}"),
        }
    }
}

impl std::error::Error for DevError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Project(source) | Self::Wait { source } => Some(source.as_ref()),
            Self::Script(source) | Self::Serve(source) => Some(source),
            Self::Spawn { source, .. } | Self::Cargo { source } | Self::Stage { source } => {
                Some(source)
            }
            Self::Bind { source, .. } => Some(source),
            Self::Watch(source) => Some(source),
            Self::Frontend(source) => Some(source),
            Self::NodeMissing => None,
        }
    }
}

/// What `arc dev` was asked to do.
#[derive(Debug, Clone)]
pub struct Options {
    /// The one TCP port to bind.
    pub port: u16,
    /// The address to bind it on. Loopback by default: a development server
    /// with an unbuilt frontend and debug assertions on is not something to
    /// publish to a network by accident.
    pub host: IpAddr,
    /// Open the browser once the first build is serving.
    pub open: bool,
    /// How long a request waits for a rebuilding backend before it is given
    /// the holding page instead.
    pub hold: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            port: crate::application::builder::DEFAULT_PORT,
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            open: false,
            hold: service::DEFAULT_HOLD,
        }
    }
}

/// Resolve command-line arguments into [`Options`].
///
/// Separate from [`run`] so the dispatcher can report a bad `--host` before
/// building a runtime, and so the resolution is testable without starting
/// anything.
///
/// # Errors
///
/// [`DevError::Bind`] if `host` is not an address. It is reported as a bind
/// failure because that is what it is: an address that cannot be parsed is
/// an address that cannot be bound.
pub fn options(port: Option<u16>, host: Option<&str>, open: bool) -> Result<Options, DevError> {
    let defaults = Options::default();
    let host = match host {
        Some(host) => host.parse::<IpAddr>().map_err(|error| DevError::Bind {
            address: SocketAddr::new(defaults.host, port.unwrap_or(defaults.port)),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("`{host}` is not an IP address: {error}"),
            ),
        })?,
        None => defaults.host,
    };

    Ok(Options {
        port: port.unwrap_or(defaults.port),
        host,
        open,
        hold: defaults.hold,
    })
}

/// Run the development supervisor until Ctrl-C.
///
/// The order below is the whole design, and none of it is interchangeable:
///
/// 1. find the project, and refuse early if it has no frontend;
/// 2. mint two process-private IPC endpoints;
/// 3. write Vite's entry script into the scratch directory;
/// 4. start Vite and wait for it to listen;
/// 5. bind the one TCP port -- after Vite, so a port clash is reported
///    before anything has been built;
/// 6. build and start the application, and wait for it to listen;
/// 7. print one URL.
///
/// From then on the loop only ever replaces the child from step 6.
///
/// # Errors
///
/// Any [`DevError`]. Every one of them is fatal on purpose: there is no
/// fallback that opens a second TCP port, because a second port would make
/// the URL printed in step 7 a lie.
pub async fn run(options: Options) -> Result<(), DevError> {
    let working_directory = std::env::current_dir().map_err(|source| DevError::Spawn {
        program: String::from("`arc dev` in this directory"),
        source,
    })?;
    let project = Project::discover(&working_directory)
        .map_err(|error| DevError::Project(Box::new(error)))?;
    let scratch = project
        .scratch()
        .map_err(|error| DevError::Project(Box::new(error)))?;

    let node = project::node_version().ok_or(DevError::NodeMissing)?;
    // Before anything is written or spawned. Without this the first sign of
    // an uninstalled frontend is Node throwing `ERR_MODULE_NOT_FOUND` for
    // `vite` out of a generated file the reader never wrote, through four
    // frames of `node:internal/modules/*`, followed by the supervisor
    // reporting that the process which should have listened exited first.
    // Three messages, none of them naming the one thing that is wrong.
    super::install::ensure_installed(project.root()).map_err(DevError::Frontend)?;
    let endpoints = Endpoints::mint(&scratch);
    let sentinel = project.sentinel();
    // Written before Vite starts, because Vite reads it on the first change
    // and a missing file would make the watcher's first event a removal.
    project::touch_sentinel(&sentinel).map_err(DevError::Script)?;

    let script = vite::write_script(&scratch).map_err(DevError::Script)?;
    let mut vite_child = start_vite(&project, &script, &endpoints, &sentinel)?;
    let waited = endpoints::wait_until_listening(&endpoints.vite, VITE_TIMEOUT, || {
        vite_child
            .exited()
            .map(|status| format!("vite exited with {status}"))
    })
    .await
    .map_err(|error| DevError::Wait {
        source: Box::new(error),
    })?;
    // `node` is the Node version, not Vite's -- the previous wording put it
    // directly after the word "vite", where it read as one. The supervisor
    // never learns Vite's version: it spawns the project's own copy through
    // `node_modules`, deliberately, so that there is no version for this
    // process to keep in step with.
    println!(
        "  vite    ready in {:.2}s (node {node})",
        waited.as_secs_f32()
    );

    let address = SocketAddr::new(options.host, options.port);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|source| DevError::Bind { address, source })?;

    serve(project, endpoints, sentinel, listener, options, vite_child).await
}

/// What the developer sees once the first build is serving.
///
/// The line this replaced was `Arcature dev server ready at <url>`, printed
/// after a row of six timings. Everything that matters was there and nothing
/// was easier to find for it. A banner earns its blank lines by making the
/// URL the only thing the eye lands on.
///
/// `--host` is named rather than a second address printed, because the
/// supervisor binds loopback by default and a "Network" row showing an
/// address nothing can reach from another machine is worse than no row.
fn ready_banner(url: &str) {
    use crate::cli::style;

    println!();
    println!(
        "  {} {}",
        style::brand("Arcature"),
        style::dim(crate::FRAMEWORK_VERSION)
    );
    println!();
    println!("  {}  {}", style::dim("Local:  "), style::green(url));
    println!(
        "  {}  {}",
        style::dim("Network:"),
        style::dim("use --host to expose")
    );
    println!();
}

/// Start Vite in middleware mode on its endpoint.
///
/// The endpoint and the sentinel path are passed as arguments rather than in
/// the environment. Both are decided by the supervisor for the lifetime of
/// one run, and neither is configuration: an environment variable would
/// suggest otherwise, would be inherited by everything Vite spawns, and would
/// let a stale value exported in some shell present itself as a fault in the
/// dev server. Arguments are visible in a process list and reach exactly one
/// process.
fn start_vite(
    project: &Project,
    script: &std::path::Path,
    endpoints: &Endpoints,
    sentinel: &std::path::Path,
) -> Result<ChildGuard, DevError> {
    let mut command = inherited("node");
    command
        .arg(script)
        .arg(&endpoints.vite)
        .arg(sentinel)
        .current_dir(project.root());
    // stdin is left as `inherited` set it: a pipe the supervisor holds
    // open, whose closing is how Vite learns it was orphaned. A null
    // stdin here is end-of-file immediately, and Vite would shut down
    // the moment it finished starting. See the header of `child.rs`.

    ChildGuard::spawn("vite", &mut command).map_err(|source| DevError::Spawn {
        program: String::from("node (for Vite)"),
        source,
    })
}

/// Everything after the port is bound: first build, then the rebuild loop.
///
/// Takes ownership of the Vite child so that returning from here -- for any
/// reason, including an error -- drops it and kills it.
async fn serve(
    project: Project,
    endpoints: Endpoints,
    sentinel: PathBuf,
    listener: tokio::net::TcpListener,
    options: Options,
    vite_child: ChildGuard,
) -> Result<(), DevError> {
    let _vite_child = vite_child;

    // Published so an `arc typegen`, `arc routes` or `arc build` run from a
    // second terminal reads the graph out of this process instead of
    // building a second binary. Dropped -- and so removed -- when `serve`
    // returns, because the file is a claim that something is listening.
    let _published = match listener.local_addr() {
        Ok(bound) => {
            match project::PublishedAddress::publish(project.root(), &connectable(bound)) {
                Ok(published) => Some(published),
                Err(error) => {
                    eprintln!("warning: could not publish the dev server address: {error}");
                    None
                }
            }
        }
        Err(error) => {
            eprintln!("warning: could not read the bound address: {error}");
            None
        }
    };

    let handle = BackendHandle::new();
    let supervisor = Supervisor::new(
        endpoints.vite.clone(),
        endpoints.app.clone(),
        handle.clone(),
        options.hold,
    );

    // Serving starts before the first build finishes on purpose: a browser
    // opened early gets the holding page rather than a refused connection,
    // which is the same promise the rebuild loop makes later.
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = tokio::spawn(async move {
        use crate::axum::ServiceExt as _;

        crate::axum::serve(listener, supervisor.into_make_service())
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });

    let mut backend =
        backend::Backend::new(project.root().to_path_buf(), endpoints, sentinel, handle);

    // The first build is inside the loop, not before it. It is the slowest
    // one of the session -- nothing is cached yet -- and a Ctrl-C during it
    // has to work for the same reason it has to work during any other.
    let outcome = rebuild_loop(project.root(), &mut backend, || {
        let url = format!("http://{}", local_url(&options));
        ready_banner(&url);
        if options.open {
            open_browser(&url);
        }
    })
    .await;

    backend.stop();
    let _ = stop.send(());
    match served.await {
        Ok(Ok(())) => outcome,
        Ok(Err(source)) => Err(DevError::Serve(source)),
        // A panic in the serving task has already been reported by the
        // default hook; there is nothing to add.
        Err(_) => outcome,
    }
}

/// Build once, then rebuild on every change, until Ctrl-C.
///
/// `ready` is called after the first build finishes, and is where the URL is
/// printed: there is nothing to open before then.
///
/// A failed rebuild does not end the loop: the diagnostics are already in
/// front of the browser, and the next save is the fix.
///
/// # Why the build is raced rather than awaited
///
/// A build is seconds long, and two things routinely happen inside that
/// window. The developer saves again -- and finishing the older build then
/// restarting into a binary already known to be stale wastes the whole of it,
/// twice over, because the newer build follows immediately. Or the developer
/// presses Ctrl-C -- and a supervisor that only checks for it between builds
/// looks hung. So the build runs as a future the loop selects over, and both
/// events reach in and kill the compiler.
///
/// The watcher must be started before the first build, or edits made while
/// that build runs are edits nobody sees.
async fn rebuild_loop(
    root: &std::path::Path,
    backend: &mut backend::Backend,
    ready: impl FnOnce(),
) -> Result<(), DevError> {
    let mut watch = watch::Watch::start(root).map_err(DevError::Watch)?;
    let mut ready = Some(ready);
    // `None` on the first pass: the first build is not asked for by a change,
    // it is the reason the session exists.
    let mut next = Some(watch::Change::Rebuild);

    loop {
        let change = match next.take() {
            Some(change) => change,
            None => tokio::select! {
                // Biased so that Ctrl-C during a burst of saves is not
                // starved by the change branch always being ready.
                biased;
                signal = tokio::signal::ctrl_c() => {
                    signal.map_err(DevError::Serve)?;
                    println!("\n  stopping");
                    return Ok(());
                }
                // The watcher ending means no further rebuild will ever
                // happen. Continuing would serve a stale binary while
                // pretending to be a development server.
                change = watch.next_change() => match change {
                    Some(change) => change,
                    None => return Ok(()),
                },
            },
        };

        match change {
            watch::Change::Rebuild if ready.is_some() => println!("  building"),
            watch::Change::Rebuild => println!("  rebuilding"),
            watch::Change::Restart => println!("  restarting (the environment changed)"),
        }

        let cancel = backend::Cancel::default();
        let reload = match run_change(backend, change, &cancel, &mut watch, &mut next).await? {
            Some(reload) => reload,
            // Ctrl-C reached in and stopped the build.
            None => {
                println!("\n  stopping");
                return Ok(());
            }
        };

        match reload {
            backend::Reload::Done(stages) => {
                println!("  app     {stages}");
                if let Some(ready) = ready.take() {
                    ready();
                }
            }
            // Superseded. The newer change is already in `next`, and printing
            // a partial timing for work that was thrown away would only
            // suggest something went wrong.
            backend::Reload::Cancelled => println!("  superseded"),
        }
    }
}

/// Run one change to completion, unless something interrupts it.
///
/// Returns `None` if the developer pressed Ctrl-C, in which case the compiler
/// has been killed and reaped and the caller should stop. A newer change
/// interrupts too, but is not a reason to stop: it is left in `next` and the
/// reload reports itself cancelled.
async fn run_change(
    backend: &mut backend::Backend,
    change: watch::Change,
    cancel: &backend::Cancel,
    watch: &mut watch::Watch,
    next: &mut Option<watch::Change>,
) -> Result<Option<backend::Reload>, DevError> {
    let work = async {
        match change {
            watch::Change::Rebuild => backend.reload(cancel).await,
            watch::Change::Restart => backend.restart_only(cancel).await,
        }
    };
    let mut work = std::pin::pin!(work);

    loop {
        tokio::select! {
            biased;
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(DevError::Serve)?;
                cancel.request();
                // Awaited, not abandoned: the compiler is a child of this
                // process and the blocking thread is still inside `wait`.
                // Returning here would leave the runtime shutting down around
                // both of them.
                drop(work.await);
                return Ok(None);
            }
            // Only the first one matters. Once a newer change is held, the
            // build is already being killed and there is nothing a second
            // one would change.
            change = watch.next_change(), if next.is_none() => {
                // `None` is the watcher going away; let the build finish
                // and let the caller discover it on the next pass.
                if let Some(change) = change {
                    *next = Some(change);
                    cancel.request();
                }
            }
            done = &mut work => return done.map(Some),
        }
    }
}

/// The host:port to print, with the wildcard address shown as something a
/// browser can actually open.
fn local_url(options: &Options) -> String {
    let host = if options.host.is_unspecified() {
        String::from("localhost")
    } else if options.host.is_ipv6() {
        format!("[{}]", options.host)
    } else {
        options.host.to_string()
    };
    format!("{host}:{}", options.port)
}

/// The bound address as something another process on this machine can
/// connect to.
///
/// A wildcard bind is reported by the operating system as `0.0.0.0` or
/// `[::]`, neither of which is a destination. The loopback address is,
/// and it is the one the supervisor is certainly reachable on.
fn connectable(bound: SocketAddr) -> String {
    if bound.ip().is_unspecified() {
        let loopback = if bound.is_ipv6() {
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        } else {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        };
        SocketAddr::new(loopback, bound.port()).to_string()
    } else {
        bound.to_string()
    }
}

/// Ask the desktop to open `url`.
///
/// Best effort by design: failing to open a browser is not a reason to stop
/// a working server, so the failure is reported and ignored.
fn open_browser(url: &str) {
    let spawned = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };

    if let Err(error) = spawned {
        eprintln!("warning: could not open a browser: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_port_is_the_one_the_printed_url_uses() {
        let options = Options::default();

        assert_eq!(local_url(&options), "127.0.0.1:1183");
    }

    #[test]
    fn a_wildcard_bind_is_printed_as_something_a_browser_can_open() {
        let options = Options {
            host: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            ..Options::default()
        };

        assert_eq!(local_url(&options), "localhost:1183");
    }

    #[test]
    fn an_ipv6_host_is_bracketed_so_the_port_is_not_read_as_part_of_it() {
        let options = Options {
            host: IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            port: 8080,
            ..Options::default()
        };

        assert_eq!(local_url(&options), "[::1]:8080");
    }

    #[test]
    fn the_default_bind_is_loopback_so_a_dev_server_is_not_published_by_accident() {
        assert!(
            Options::default().host.is_loopback(),
            "a development build with debug assertions on should not be reachable from the network unless asked"
        );
    }

    #[test]
    fn a_missing_node_says_what_to_install() {
        let message = DevError::NodeMissing.to_string();

        assert!(message.contains("Node.js"), "got: {message}");
    }

    #[test]
    fn a_port_clash_names_the_flag_that_fixes_it() {
        let message = DevError::Bind {
            address: SocketAddr::from(([127, 0, 0, 1], 3000)),
            source: std::io::Error::from(std::io::ErrorKind::AddrInUse),
        }
        .to_string();

        assert!(message.contains("--port"), "got: {message}");
        assert!(
            message.contains("only TCP port"),
            "the message should say there is only one, got: {message}"
        );
    }
}

#[cfg(test)]
mod option_tests {
    use super::*;

    #[test]
    fn an_unparseable_host_is_refused_before_anything_is_started() {
        let error = options(None, Some("not-an-address"), false)
            .expect_err("a hostname is not an address this can bind");

        assert!(error.to_string().contains("not an IP address"));
    }

    #[test]
    fn the_named_port_and_host_survive_resolution() {
        let resolved = options(Some(5173), Some("0.0.0.0"), true).expect("resolves");

        assert_eq!(resolved.port, 5173);
        assert_eq!(resolved.host, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert!(resolved.open);
    }
}
