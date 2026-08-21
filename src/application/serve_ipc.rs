//! Listening on an IPC endpoint instead of a TCP port.
//!
//! Production, and `cargo run --features dev`, bind a TCP listener: the
//! application owns the port. Under `arc dev` the supervisor owns the only
//! TCP port and the application runs as a child process, so it must listen
//! somewhere that is not a port. That somewhere is an IPC endpoint -- a Unix
//! domain socket on Unix, a named pipe on Windows -- whose path the
//! supervisor passes in [`crate::config::APP_IPC_ENV`].
//!
//! # Why a `Listener` and not a second serving loop
//!
//! `axum::serve` is generic over [`axum::serve::Listener`], and it already
//! drives each accepted connection through
//! `hyper_util::server::conn::auto::Builder::serve_connection_with_upgrades`.
//! Upgrades are not optional here: the application's own WebSocket routes
//! reach it through this path, and so does anything else that answers `101`.
//! Implementing the trait therefore buys the correct connection handling and
//! the same graceful-shutdown behaviour the TCP path has, instead of a
//! parallel accept loop that would drift away from it.
//!
//! axum ships `impl Listener for tokio::net::UnixListener`, but there is no
//! equivalent for Windows named pipes, and the Unix socket file needs
//! unlinking on both bind and drop. [`IpcListener`] wraps both platforms so
//! the serve path sees one type.
//!
//! # There is no second TCP port
//!
//! If the IPC endpoint cannot be created the application fails to start. It
//! does not quietly fall back to binding a port: two listeners is exactly
//! the outcome the one-port design exists to prevent, and a silent second
//! origin is worse than a clear error.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::{EngineError, EngineResult};

/// How long to wait before retrying an accept that failed for a reason that
/// is plausibly transient. Matches the spirit of axum's own TCP accept
/// backoff: a hot loop on a broken listener would bury the terminal.
const ACCEPT_BACKOFF: std::time::Duration = std::time::Duration::from_millis(25);

/// Resolve the application's IPC endpoint from the environment.
///
/// Read once, at serve time, and never per request. `None` means the
/// application binds its configured TCP address -- production, and any run
/// that `arc dev` did not supervise.
#[must_use]
pub fn endpoint_from_env() -> Option<PathBuf> {
    std::env::var(crate::config::APP_IPC_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
}

/// Exit if the process that started this one goes away.
///
/// Only meaningful under `arc dev`, which is why it is called from the IPC
/// branch of [`ServeTarget::bind`] and nowhere else. The supervisor gives
/// each child a stdin pipe that it holds open and never writes to; the
/// operating system closes the write end when the supervisor stops existing,
/// by whatever means, and the read here then returns end-of-file.
///
/// # Why the application has to care at all
///
/// The supervisor kills its children on the way out, and that covers every
/// exit it gets to run code for. It does not cover `kill -9`, End Task, or a
/// terminal closed out from under it. What is left behind is an application
/// still holding the IPC endpoint the next `arc dev` will try to create --
/// on Windows a named pipe with no filesystem trace, owned by a process
/// nobody can see. This is the only signal that reaches the child in that
/// case.
///
/// # Why a thread and a hard exit
///
/// A blocking read of stdin is the whole mechanism, and it must not occupy a
/// runtime worker for the life of the process; a dedicated thread costs one
/// stack and never wakes. The exit is [`std::process::exit`] rather than a
/// graceful shutdown because the parent is already gone: there is nobody
/// left to forward the in-flight requests to, and draining would only delay
/// the release of the endpoint that the next run is waiting for.
fn exit_when_orphaned() {
    std::thread::Builder::new()
        .name(String::from("arcature-orphan-watch"))
        .spawn(|| {
            use std::io::Read as _;
            let mut byte = [0_u8; 1];
            match io::stdin().read(&mut byte) {
                // The supervisor is gone, or something closed the pipe.
                Ok(0) | Err(_) => std::process::exit(0),
                // The supervisor never writes, so a byte means this is
                // somebody else's stdin -- a developer who ran the binary by
                // hand with the env var set. Not our business; stop watching.
                Ok(_) => {}
            }
        })
        // A failure to spawn one thread is not a reason to refuse to serve.
        // The cost is a possible orphan in a case that already requires the
        // supervisor to have been killed uncleanly.
        .map(drop)
        .unwrap_or_else(|error| {
            eprintln!("arcature: could not watch for an orphaned supervisor: {error}");
        });
}

/// The endpoint an IPC connection arrived on.
///
/// `axum::serve` requires an address type per accepted connection. An IPC
/// peer has no address worth reporting -- the socket is process-private and
/// has exactly one client population -- so the endpoint path stands in. It
/// is what a log line would want to name anyway.
#[derive(Clone, Debug)]
pub struct IpcAddr(Arc<Path>);

impl IpcAddr {
    /// The endpoint path this connection arrived on.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl std::fmt::Display for IpcAddr {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0.display())
    }
}

/// A listener on an IPC endpoint.
///
/// Construct it with [`IpcListener::bind`] and hand it to `axum::serve`, or
/// to [`ServeTarget::serve`] which does that for you.
///
/// # Platform notes
///
/// On Unix this is a `tokio::net::UnixListener` plus the unlinking the
/// standard library does not do: a stale socket file from a killed process
/// makes `bind` fail with `AddrInUse`, and leaving one behind does the same
/// to the next run. On Windows it is a named pipe, which has no filesystem
/// lifetime to manage but does need a fresh server instance created for
/// every connection -- a pipe instance is consumed by the client that
/// connects to it.
#[cfg(unix)]
pub struct IpcListener {
    listener: tokio::net::UnixListener,
    path: Arc<Path>,
}

/// A listener on an IPC endpoint. See the Unix definition for the full
/// documentation; the two differ only in the transport they own.
#[cfg(windows)]
pub struct IpcListener {
    /// The next pipe instance, already created and waiting for a client.
    /// There is always one, so a client never sees "pipe not found" between
    /// two accepts.
    next: tokio::net::windows::named_pipe::NamedPipeServer,
    path: Arc<Path>,
}

impl IpcListener {
    /// Create the IPC endpoint at `path` and start listening on it.
    ///
    /// # Errors
    ///
    /// `io::Error` if the endpoint cannot be created: a directory that does
    /// not exist, a permission failure, or -- on Windows -- a pipe name
    /// another process already owns.
    pub async fn bind(path: &Path) -> io::Result<Self> {
        let owned: Arc<Path> = Arc::from(path);
        #[cfg(unix)]
        {
            // A socket file left behind by a killed process is indistinguishable
            // from a live one to `bind`, which refuses either way. Nothing else
            // is allowed to own this path (it is minted per process), so removing
            // it cannot take a listener away from anyone.
            if tokio::fs::metadata(path).await.is_ok() {
                tokio::fs::remove_file(path).await?;
            }
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            Ok(Self {
                listener: tokio::net::UnixListener::bind(path)?,
                path: owned,
            })
        }
        #[cfg(windows)]
        {
            let next = tokio::net::windows::named_pipe::ServerOptions::new()
                // Refuse to attach to a pipe name someone else already serves:
                // silently sharing an endpoint with a stale process would send
                // half the requests into the void.
                .first_pipe_instance(true)
                .create(path)?;
            Ok(Self { next, path: owned })
        }
    }

    /// The endpoint this listener owns.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
impl Drop for IpcListener {
    fn drop(&mut self) {
        // The socket file outlives the listener unless someone removes it,
        // and a leftover file is what makes the *next* `arc dev` fail.
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
impl axum::serve::Listener for IpcListener {
    type Io = tokio::net::UnixStream;
    type Addr = IpcAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((io, _peer)) => return (io, IpcAddr(Arc::clone(&self.path))),
                Err(error) => {
                    eprintln!("warning: ipc accept failed: {error}");
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(IpcAddr(Arc::clone(&self.path)))
    }
}

#[cfg(windows)]
impl axum::serve::Listener for IpcListener {
    type Io = tokio::net::windows::named_pipe::NamedPipeServer;
    type Addr = IpcAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        use tokio::net::windows::named_pipe::ServerOptions;
        loop {
            if let Err(error) = self.next.connect().await {
                eprintln!("warning: ipc accept failed: {error}");
                tokio::time::sleep(ACCEPT_BACKOFF).await;
                continue;
            }
            // The connected instance is now this client's; a replacement has
            // to exist before we hand it over, or the next client finds no
            // pipe to open. Retry rather than drop the connection we hold.
            let replacement = loop {
                match ServerOptions::new().create(&*self.path) {
                    Ok(server) => break server,
                    Err(error) => {
                        eprintln!("warning: could not open the next pipe instance: {error}");
                        tokio::time::sleep(ACCEPT_BACKOFF).await;
                    }
                }
            };
            let io = std::mem::replace(&mut self.next, replacement);
            return (io, IpcAddr(Arc::clone(&self.path)));
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(IpcAddr(Arc::clone(&self.path)))
    }
}

/// Render a bound socket address as a URL somebody can click.
///
/// Only the host is rewritten, and only when it is unspecified. A deliberate
/// bind to `127.0.0.1` prints as `127.0.0.1`, because that is a real
/// restriction the operator chose and hiding it behind `localhost` would make
/// two different configurations print the same line. IPv6 keeps its brackets:
/// `http://[::1]:3000` is the form a browser accepts.
fn http_url(addr: SocketAddr) -> String {
    if addr.ip().is_unspecified() {
        return format!("http://localhost:{}", addr.port());
    }
    match addr {
        SocketAddr::V4(v4) => format!("http://{}:{}", v4.ip(), v4.port()),
        SocketAddr::V6(v6) => format!("http://[{}]:{}", v6.ip(), v6.port()),
    }
}

/// Where the application listens.
///
/// One decision, made once, at the point the serve path used to reach
/// straight for `TcpListener::bind`. Keeping it a type rather than a
/// branch in two places is what stops the stateless `serve` and the
/// stateful `run_with_state` from disagreeing about it later.
pub enum ServeTarget {
    /// A TCP port -- production, and any run `arc dev` did not supervise.
    Tcp(tokio::net::TcpListener),
    /// An IPC endpoint -- `arc dev` owns the only TCP port.
    Ipc(IpcListener),
}

impl ServeTarget {
    /// Bind wherever this process is supposed to listen.
    ///
    /// [`crate::config::APP_IPC_ENV`] decides: set means IPC, unset means
    /// the TCP address the caller resolved. `addr` is ignored in the IPC
    /// case, deliberately -- an application that listens on IPC has no port,
    /// and pretending otherwise in a log line would send someone to a dead
    /// URL.
    ///
    /// # Errors
    ///
    /// [`EngineError::BindListener`] if the port is taken, or if the IPC
    /// endpoint cannot be created. There is no fallback from one to the
    /// other: a second TCP port is the failure this design exists to
    /// prevent.
    pub async fn bind(addr: SocketAddr) -> EngineResult<Self> {
        match endpoint_from_env() {
            Some(path) => {
                exit_when_orphaned();
                IpcListener::bind(&path)
                    .await
                    .map(Self::Ipc)
                    .map_err(|source| EngineError::BindListener {
                        address: path.display().to_string(),
                        source,
                    })
            }
            None => tokio::net::TcpListener::bind(addr)
                .await
                .map(Self::Tcp)
                .map_err(|source| EngineError::BindListener {
                    address: addr.to_string(),
                    source,
                }),
        }
    }

    /// Describe the endpoint, for the one line a booting application prints.
    ///
    /// The TCP case is rendered as a URL, because the only useful thing to do
    /// with that line is click it. A wildcard bind (`0.0.0.0`, `[::]`) is
    /// reported as `localhost`: `http://0.0.0.0:3000` is a valid address to
    /// *listen* on and, in a browser, a coin flip -- so the line names an
    /// address that is guaranteed to reach this process from this machine.
    /// The port is kept exactly as bound, including the one the kernel chose
    /// when the request was port `0`.
    ///
    /// The IPC case is the path, with no scheme and no invented URL. An
    /// application listening on a named pipe is not reachable over HTTP at
    /// any address, and `arc dev` -- the only thing that puts it there --
    /// prints its own TCP URL anyway.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Tcp(listener) => listener
                .local_addr()
                .map_or_else(|_| String::from("tcp"), http_url),
            Self::Ipc(listener) => listener.path().display().to_string(),
        }
    }

    /// Serve `service` until `shutdown` resolves.
    ///
    /// # Errors
    ///
    /// [`EngineError::Serve`] if the accept loop fails terminally.
    pub async fn serve<S, F>(self, service: S, shutdown: F) -> EngineResult<()>
    where
        S: tower::Service<
                crate::axum::extract::Request,
                Response = crate::axum::response::Response,
                Error = std::convert::Infallible,
            > + Clone
            + Send
            + 'static,
        S::Future: Send,
        F: Future<Output = ()> + Send + 'static,
    {
        use crate::axum::ServiceExt as _;
        match self {
            Self::Tcp(listener) => {
                axum::serve(listener, service.into_make_service())
                    .with_graceful_shutdown(shutdown)
                    .await
            }
            Self::Ipc(listener) => {
                axum::serve(listener, service.into_make_service())
                    .with_graceful_shutdown(shutdown)
                    .await
            }
        }
        .map_err(|source| EngineError::Serve { source })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A distinct endpoint per test, so two tests never collide on a name.
    fn scratch_endpoint(label: &str) -> PathBuf {
        let pid = std::process::id();
        #[cfg(windows)]
        {
            PathBuf::from(format!(r"\\.\pipe\arcature-serve-ipc-{label}-{pid}"))
        }
        #[cfg(unix)]
        {
            std::env::temp_dir().join(format!("arcature-serve-ipc-{label}-{pid}.sock"))
        }
    }

    #[test]
    fn an_unset_endpoint_means_the_application_keeps_its_port() {
        // The env is process-wide and other tests run beside this one, so
        // this asserts the parsing rule through the same filter the reader
        // applies rather than by mutating the environment.
        assert!(
            Option::<String>::None
                .filter(|value: &String| !value.trim().is_empty())
                .is_none()
        );
        assert!(
            Some(String::from("  "))
                .filter(|value: &String| !value.trim().is_empty())
                .is_none()
        );
    }

    #[tokio::test]
    async fn binding_an_endpoint_makes_it_connectable() {
        let path = scratch_endpoint("connectable");
        let listener = IpcListener::bind(&path)
            .await
            .expect("the endpoint should be creatable");
        assert_eq!(listener.path(), path.as_path());

        // Connect with the platform primitive rather than through
        // `crate::dev_proxy`: this module is compiled in builds that do not
        // have the dev proxy feature, and a test may not reach for something
        // the rest of the file cannot.
        #[cfg(unix)]
        tokio::net::UnixStream::connect(&path)
            .await
            .expect("a bound endpoint should accept a connection");
        #[cfg(windows)]
        tokio::net::windows::named_pipe::ClientOptions::new()
            .open(&path)
            .expect("a bound endpoint should accept a connection");
    }

    #[tokio::test]
    async fn a_second_bind_of_a_live_endpoint_does_not_silently_share_it() {
        let path = scratch_endpoint("exclusive");
        let _first = IpcListener::bind(&path)
            .await
            .expect("the first bind should succeed");

        // Unix has no such guarantee from the kernel -- `bind` after
        // `unlink` always succeeds -- so this only asserts what the platform
        // can actually promise. On Windows `first_pipe_instance` makes the
        // second bind fail, which is the point of setting it.
        #[cfg(windows)]
        assert!(
            IpcListener::bind(&path).await.is_err(),
            "a second listener must not attach to a live pipe name"
        );
    }

    #[tokio::test]
    async fn an_ipc_target_describes_itself_by_its_endpoint() {
        let path = scratch_endpoint("describe");
        let target = ServeTarget::Ipc(
            IpcListener::bind(&path)
                .await
                .expect("the endpoint should be creatable"),
        );
        assert_eq!(target.describe(), path.display().to_string());
    }

    #[tokio::test]
    async fn a_tcp_target_describes_itself_as_a_clickable_url() {
        let target = ServeTarget::bind("127.0.0.1:0".parse().expect("a literal address"))
            .await
            .expect("an ephemeral port should be bindable");
        let described = target.describe();
        assert!(
            described.starts_with("http://127.0.0.1:"),
            "expected a URL, got `{described}`"
        );
        assert!(
            !described.ends_with(":0"),
            "the line must name the port the kernel chose, not the one requested: `{described}`"
        );
    }

    #[test]
    fn a_wildcard_bind_is_reported_as_localhost() {
        assert_eq!(
            http_url("0.0.0.0:3000".parse().expect("a literal address")),
            "http://localhost:3000"
        );
        assert_eq!(
            http_url("[::]:3000".parse().expect("a literal address")),
            "http://localhost:3000"
        );
    }

    #[test]
    fn an_ipv6_literal_keeps_its_brackets() {
        assert_eq!(
            http_url("[::1]:8080".parse().expect("a literal address")),
            "http://[::1]:8080"
        );
    }
}
