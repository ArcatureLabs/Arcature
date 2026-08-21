//! The `arc dev` supervisor, driven through a real TCP port.
//!
//! These tests exercise the topology rather than the pieces: a real listener
//! on a real port, real IPC endpoints with real servers behind them, and a
//! real HTTP client. What they are checking is the promise the one-port
//! design makes -- that a browser pointed at that port keeps getting answers
//! while the application behind it is stopped, restarted, or failing to
//! compile.
//!
//! The application child and the Vite child are stood in for by ordinary
//! axum routers listening on the same IPC endpoints the real children would
//! use. That is the honest substitution: the supervisor cannot tell the
//! difference, because everything it knows about either child arrives over
//! that socket.

#![cfg(all(feature = "cli", feature = "macros"))]

use std::path::{Path, PathBuf};
use std::time::Duration;

use arcature::application::serve_ipc::IpcListener;
use arcature::axum::Router;
use arcature::axum::routing::get;
use arcature::cli::commands::dev::service::{BackendHandle, Supervisor};

/// A distinct pair of endpoint names per test.
fn ipc_path(label: &str) -> PathBuf {
    let pid = std::process::id();
    #[cfg(windows)]
    {
        PathBuf::from(format!(r"\\.\pipe\arcature-dev-supervisor-{label}-{pid}"))
    }
    #[cfg(unix)]
    {
        std::env::temp_dir().join(format!("arcature-dev-supervisor-{label}-{pid}.sock"))
    }
}

/// Connect until something answers, so a test never races a listener.
async fn wait_until_listening(path: &Path) {
    for _ in 0..400 {
        #[cfg(unix)]
        let connected = tokio::net::UnixStream::connect(path).await.is_ok();
        #[cfg(windows)]
        let connected = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(path)
            .is_ok();
        if connected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("nothing started listening on {}", path.display());
}

/// A child stand-in, alive until stopped or dropped.
struct Child(Option<tokio::task::JoinHandle<()>>);

impl Child {
    /// Stop the child and wait until its endpoint is genuinely released.
    ///
    /// `abort` only *requests* cancellation: the task, and the listener it
    /// owns, are dropped at some later scheduler pass. Binding the same
    /// endpoint before that happens fails on Windows with
    /// `ERROR_ACCESS_DENIED`, because the pipe name still has an owner.
    ///
    /// Waiting here is not a fudge to make a test pass -- it is what the
    /// supervisor itself does. `dev::child` kills *and reaps*, for the reason
    /// written at `src/cli/commands/dev/child.rs:61`: the endpoint is released
    /// when the process is reaped, not when the kill is requested, and the
    /// next spawn binds that same endpoint. A test that only aborted would be
    /// modelling a restart the supervisor never performs.
    async fn stop(mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // The un-awaited path, for a child whose endpoint nothing rebinds.
        if let Some(handle) = &self.0 {
            handle.abort();
        }
    }
}

/// Serve `router` on `path` and return once it is accepting.
async fn serve_over_ipc(path: &Path, router: Router) -> Child {
    let listener = IpcListener::bind(path)
        .await
        .expect("the endpoint should be bindable");
    let handle = tokio::spawn(async move {
        let _ = arcature::axum::serve(listener, router).await;
    });
    wait_until_listening(path).await;
    Child(Some(handle))
}

/// Bind the supervisor's one TCP port and return its base URL.
fn serve_supervisor(
    vite: PathBuf,
    app: PathBuf,
    backend: BackendHandle,
    hold: Duration,
) -> (String, Child) {
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a loopback port should be available");
    listener
        .set_nonblocking(true)
        .expect("the listener should go non-blocking");
    let address = listener.local_addr().expect("the port should be readable");

    let supervisor = Supervisor::new(vite, app, backend, hold);
    let handle = tokio::spawn(async move {
        use arcature::axum::ServiceExt as _;

        let listener = tokio::net::TcpListener::from_std(listener)
            .expect("the listener should adopt into the runtime");
        let _ = arcature::axum::serve(listener, supervisor.into_make_service()).await;
    });

    (format!("http://{address}"), Child(Some(handle)))
}

/// The application child: an ordinary router that says who answered.
fn application() -> Router {
    Router::new()
        .route("/", get(|| async { "the application answered" }))
        .route("/api/ping", get(|| async { "pong" }))
}

/// The Vite child: the asset routes and nothing else.
fn vite() -> Router {
    Router::new().route(
        "/resources/js/app.tsx",
        get(|| async { "export default 'vite'" }),
    )
}

#[tokio::test]
async fn a_request_that_arrives_during_a_restart_is_answered_when_the_backend_returns() {
    let vite_path = ipc_path("restart-vite");
    let app_path = ipc_path("restart-app");
    let backend = BackendHandle::new();

    let _vite = serve_over_ipc(&vite_path, vite()).await;
    // No application child at all: this is the middle of a rebuild, when the
    // old process has been killed and the new one has not started.
    let (base, _supervisor) = serve_supervisor(
        vite_path,
        app_path.clone(),
        backend.clone(),
        Duration::from_secs(10),
    );

    let starting = {
        let backend = backend.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let child = serve_over_ipc(&app_path, application()).await;
            backend.mark_ready();
            child
        })
    };

    let response = reqwest::get(format!("{base}/"))
        .await
        .expect("the connection must not be refused: the listener never closed");
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.text().await.expect("a body"),
        "the application answered",
        "the request should have been held and then forwarded, not failed"
    );

    let _child = starting.await.expect("the restart task should finish");
}

#[tokio::test]
async fn a_compile_error_reaches_the_browser_as_a_page() {
    let vite_path = ipc_path("compile-vite");
    let app_path = ipc_path("compile-app");
    let backend = BackendHandle::new();
    backend.mark_failed("error[E0308]: mismatched types\n --> src/main.rs:7:5\n");

    let _vite = serve_over_ipc(&vite_path, vite()).await;
    let (base, _supervisor) =
        serve_supervisor(vite_path, app_path, backend, Duration::from_secs(10));

    let response = reqwest::get(format!("{base}/"))
        .await
        .expect("a failing build is still an answered request");
    assert_eq!(response.status(), 500);

    let body = response.text().await.expect("a body");
    assert!(
        body.contains("E0308"),
        "the compiler's own words should be on the page, got: {body}"
    );
    assert!(
        body.contains("<html") || body.contains("<pre"),
        "it should be a page a browser renders, got: {body}"
    );
}

#[tokio::test]
async fn vite_keeps_answering_while_the_backend_is_down() {
    let vite_path = ipc_path("assets-vite");
    let app_path = ipc_path("assets-app");
    // Left in `Building` for the whole test: nothing will ever answer on the
    // application endpoint.
    let backend = BackendHandle::new();

    let _vite = serve_over_ipc(&vite_path, vite()).await;
    let (base, _supervisor) =
        serve_supervisor(vite_path, app_path, backend, Duration::from_millis(200));

    let response = reqwest::get(format!("{base}/resources/js/app.tsx"))
        .await
        .expect("an asset request should not wait on the backend at all");
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.text().await.expect("a body"),
        "export default 'vite'"
    );
}

#[tokio::test]
async fn a_backend_that_never_returns_gets_a_page_rather_than_a_hung_tab() {
    let vite_path = ipc_path("hold-vite");
    let app_path = ipc_path("hold-app");
    let backend = BackendHandle::new();

    let _vite = serve_over_ipc(&vite_path, vite()).await;
    let (base, _supervisor) =
        serve_supervisor(vite_path, app_path, backend, Duration::from_millis(200));

    let response = reqwest::get(format!("{base}/"))
        .await
        .expect("the hold has to end in a response, not a dropped connection");
    assert_eq!(response.status(), 503);
    assert_eq!(
        response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok()),
        Some("1"),
        "the page reloads itself, so it has to say when"
    );
}

#[tokio::test]
async fn five_consecutive_restarts_never_refuse_a_connection() {
    let vite_path = ipc_path("loop-vite");
    let app_path = ipc_path("loop-app");
    let backend = BackendHandle::new();

    let _vite = serve_over_ipc(&vite_path, vite()).await;
    let mut child = Some(serve_over_ipc(&app_path, application()).await);
    backend.mark_ready();

    let (base, _supervisor) = serve_supervisor(
        vite_path,
        app_path.clone(),
        backend.clone(),
        Duration::from_secs(10),
    );
    let client = reqwest::Client::new();

    // The shape of the verification the design calls for: edit, rebuild,
    // request, five times over, with no failed connection anywhere in it.
    for round in 0..5 {
        backend.mark_building();
        if let Some(running) = child.take() {
            running.stop().await;
        }

        let request = client.get(format!("{base}/api/ping")).send();
        let restart = async {
            tokio::time::sleep(Duration::from_millis(120)).await;
            let started = serve_over_ipc(&app_path, application()).await;
            backend.mark_ready();
            started
        };
        let (response, started) = tokio::join!(request, restart);
        child = Some(started);

        let response = response
            .unwrap_or_else(|error| panic!("round {round} refused the connection: {error}"));
        assert_eq!(response.status(), 200, "round {round}");
        assert_eq!(
            response.text().await.expect("a body"),
            "pong",
            "round {round}"
        );
    }
}

/// Speak just enough of RFC 6455 to prove the tunnel carries frames.
///
/// A real WebSocket client is not available here -- `tokio-tungstenite` is a
/// dev-dependency without its handshake features -- and the point being
/// tested is the tunnel, not the protocol. One masked text frame out and one
/// unmasked text frame back is the whole exchange.
mod hmr_frames {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpStream;

    /// Send a masked text frame, the way a browser client must.
    pub(super) async fn send_text(stream: &mut TcpStream, text: &str) {
        let mask = [0x37, 0xfa, 0x21, 0x3d];
        let mut frame = vec![
            0x81,
            0x80 | u8::try_from(text.len()).expect("a short frame"),
        ];
        frame.extend_from_slice(&mask);
        for (index, byte) in text.as_bytes().iter().enumerate() {
            frame.push(byte ^ mask[index % 4]);
        }
        stream
            .write_all(&frame)
            .await
            .expect("the tunnel should accept a frame");
    }

    /// Read one unmasked text frame, the way a server must send it.
    pub(super) async fn read_text(stream: &mut TcpStream) -> String {
        let mut header = [0u8; 2];
        stream
            .read_exact(&mut header)
            .await
            .expect("the tunnel should carry the reply");
        assert_eq!(header[0] & 0x0f, 0x01, "expected a text frame");
        let length = usize::from(header[1] & 0x7f);
        let mut payload = vec![0u8; length];
        stream
            .read_exact(&mut payload)
            .await
            .expect("the frame should be complete");
        String::from_utf8(payload).expect("text frames are UTF-8")
    }

    /// Perform the client half of the HMR upgrade and assert it succeeded.
    pub(super) async fn upgrade(stream: &mut TcpStream, host: &str) {
        let request = format!(
            "GET / HTTP/1.1\r\n\
             Host: {host}\r\n\
             Connection: Upgrade\r\n\
             Upgrade: websocket\r\n\
             Sec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Protocol: vite-hmr\r\n\
             \r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("the supervisor should accept the upgrade request");

        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            let read = stream.read(&mut byte).await.expect("the response head");
            assert_eq!(read, 1, "the connection closed mid-handshake");
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        assert!(
            head.starts_with("HTTP/1.1 101"),
            "the HMR socket must be upgraded, got: {head}"
        );
    }
}

/// The Vite child, plus the HMR socket the browser holds open.
fn vite_with_hmr() -> Router {
    use arcature::axum::extract::ws::{Message, WebSocketUpgrade};
    use arcature::axum::response::Response;

    async fn hmr(upgrade: WebSocketUpgrade) -> Response {
        upgrade
            .protocols(["vite-hmr"])
            .on_upgrade(|mut socket| async move {
                while let Some(Ok(message)) = socket.recv().await {
                    if let Message::Text(text) = message
                        && socket.send(Message::Text(text)).await.is_err()
                    {
                        return;
                    }
                }
            })
    }

    vite().route("/", get(hmr))
}

#[tokio::test]
async fn the_hmr_socket_survives_a_backend_restart() {
    let vite_path = ipc_path("hmr-vite");
    let app_path = ipc_path("hmr-app");
    let backend = BackendHandle::new();

    let _vite = serve_over_ipc(&vite_path, vite_with_hmr()).await;
    let mut child = Some(serve_over_ipc(&app_path, application()).await);
    backend.mark_ready();

    let (base, _supervisor) = serve_supervisor(
        vite_path,
        app_path.clone(),
        backend.clone(),
        Duration::from_secs(10),
    );
    let host = base.trim_start_matches("http://").to_owned();

    let mut socket = tokio::net::TcpStream::connect(&host)
        .await
        .expect("the one port should accept a socket");
    hmr_frames::upgrade(&mut socket, &host).await;
    hmr_frames::send_text(&mut socket, "hello").await;
    assert_eq!(hmr_frames::read_text(&mut socket).await, "hello");

    // The whole rebuild, with the browser's HMR socket held open across it.
    backend.mark_building();
    if let Some(running) = child.take() {
        running.stop().await;
    }
    let restarted = serve_over_ipc(&app_path, application()).await;
    backend.mark_ready();

    hmr_frames::send_text(&mut socket, "after").await;
    assert_eq!(
        hmr_frames::read_text(&mut socket).await,
        "after",
        "the HMR socket must not notice that the backend was replaced -- \
         a dropped one is what prints `[vite] server connection lost`"
    );

    drop(restarted);
}
