//! The supervisor's request path: hold, then forward.
//!
//! The supervisor owns the only TCP listener, so it is the only thing that
//! can decide what a request gets while the application it fronts is not
//! running. Two rules cover everything:
//!
//! 1. **Vite's requests never wait for the backend.** They are separated out
//!    by [`ViteRoutes`], the same table
//!    `cargo run --features dev` uses, and forwarded straight to the Vite
//!    IPC endpoint. That is why HMR survives a backend rebuild: the
//!    WebSocket tunnel and the process on the other end of it are untouched
//!    by anything the backend does.
//! 2. **Everything else is held, not refused.** A request that arrives while
//!    the backend is down waits for it, up to a deadline; past the deadline
//!    it gets a page saying so. The browser never sees a refused connection,
//!    because the listener never closes.
//!
//! # What is reused rather than rebuilt
//!
//! The Vite/application split is [`ViteRoutes::matches_request`]; the
//! HTTP-over-IPC forwarding, including the `101 Switching Protocols` tunnel,
//! is [`crate::dev_proxy::service::forward`]; and the composition of the two
//! is [`DevProxyLayer`] itself, wrapped around [`BackendService`] instead of
//! around an application router. A second implementation of any of those
//! would be a second place for the two topologies to disagree, and the whole
//! design rests on them agreeing.

use std::convert::Infallible;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::axum::body::Body;
use crate::axum::http::Response;
use crate::dev_proxy::endpoint::IpcEndpoint;
use crate::dev_proxy::service::{DevProxyLayer, DevProxyService, forward};

use super::pages;

/// The request type the whole supervisor pipeline speaks.
type Request = crate::axum::extract::Request<Body>;

/// A boxed future resolving to the infallible response every stage produces.
type BoxFuture = Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

/// How long a request waits for a backend that is not up.
///
/// Five seconds is long enough to cover an incremental rebuild of a normal
/// application and short enough that a developer who has broken something
/// badly sees a page rather than a hung tab.
pub const DEFAULT_HOLD: Duration = Duration::from_secs(5);

/// How long to pause before retrying a connection to a backend that was
/// reported ready but refused. Readiness is a claim about a moment that has
/// already passed.
const RECONNECT_INTERVAL: Duration = Duration::from_millis(20);

/// What the backend child is doing.
#[derive(Clone, Debug)]
pub enum BackendStatus {
    /// No backend is listening: first boot, or a rebuild in progress.
    Building,
    /// A backend is listening on the application IPC endpoint.
    Ready,
    /// The build failed. The payload is the compiler's rendered output.
    Failed(Arc<str>),
}

/// The supervisor's view of its backend child, shared with the request path.
///
/// One writer -- the rebuild loop -- and as many readers as there are
/// in-flight requests. A `watch` channel rather than a mutex plus a notify:
/// a waiting request needs both the current value and a wakeup on change,
/// which is exactly what `watch` is.
#[derive(Clone, Debug)]
pub struct BackendHandle {
    /// `Arc` because `watch::Sender` is not itself cloneable, and the
    /// rebuild loop and the service must hold the same one.
    status: Arc<tokio::sync::watch::Sender<BackendStatus>>,
}

impl Default for BackendHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendHandle {
    /// A handle whose backend has not started yet.
    ///
    /// [`BackendStatus::Building`] is the correct initial state: the
    /// supervisor binds its TCP listener before it builds the application,
    /// so requests can arrive before any backend has ever existed, and those
    /// should be held exactly like the ones during a later rebuild.
    #[must_use]
    pub fn new() -> Self {
        Self {
            status: Arc::new(tokio::sync::watch::Sender::new(BackendStatus::Building)),
        }
    }

    /// The current status.
    #[must_use]
    pub fn status(&self) -> BackendStatus {
        self.status.borrow().clone()
    }

    /// Record that a rebuild has started.
    pub fn mark_building(&self) {
        self.set(BackendStatus::Building);
    }

    /// Record that a backend is listening.
    pub fn mark_ready(&self) {
        self.set(BackendStatus::Ready);
    }

    /// Record that the build failed, with the compiler's output.
    pub fn mark_failed(&self, diagnostics: impl Into<Arc<str>>) {
        self.set(BackendStatus::Failed(diagnostics.into()));
    }

    /// Publish a status.
    ///
    /// `send_replace` rather than `send`, and the difference is the whole
    /// point: `watch::Sender::send` refuses when the receiver count is zero
    /// and, having refused, **does not store the value**. A handle starts
    /// life with no receivers, and the rebuild loop writes every status
    /// change whether or not a request happens to be in flight -- which,
    /// during the first build, is the normal case. With `send`, a backend
    /// that came up before the developer's first request would still read as
    /// `Building`, and that request would hold for the full five seconds and
    /// then show the rebuilding page over a backend that had been listening
    /// the whole time. `send_replace` always stores; the previous value is
    /// discarded because nothing here needs it.
    fn set(&self, status: BackendStatus) {
        let _previous = self.status.send_replace(status);
    }

    /// Wait until the backend is ready, the build fails, or `deadline` passes.
    async fn settle(&self, deadline: Instant) -> Settled {
        let mut changes = self.status.subscribe();
        loop {
            // Clone out rather than hold the borrow: a `watch::Ref` across an
            // await point would make this future non-`Send`.
            let current = changes.borrow_and_update().clone();
            match current {
                BackendStatus::Ready => return Settled::Ready,
                BackendStatus::Failed(diagnostics) => return Settled::Failed(diagnostics),
                BackendStatus::Building => {}
            }
            let changed = tokio::time::timeout_at(deadline.into(), changes.changed()).await;
            match changed {
                Ok(Ok(())) => {}
                // The deadline passed, or the supervisor dropped the sender
                // on its way out. Either way this request is not going to be
                // answered by a backend.
                Ok(Err(_)) | Err(_) => return Settled::StillBuilding,
            }
        }
    }
}

/// The outcome of waiting on [`BackendHandle::settle`].
enum Settled {
    /// A backend is listening; try to connect.
    Ready,
    /// The build failed; the browser gets the diagnostics.
    Failed(Arc<str>),
    /// The deadline passed with the backend still down.
    StillBuilding,
}

/// Forwards a request to the application over IPC, holding it while the
/// application is not there.
///
/// This is the inner half of the supervisor: [`DevProxyLayer`] wraps it, so
/// by the time a request reaches here it is either not Vite's, or Vite had
/// nothing for it.
#[derive(Clone)]
pub struct BackendService {
    endpoint: Arc<IpcEndpoint>,
    backend: BackendHandle,
    hold: Duration,
}

impl tower::Service<Request> for BackendService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = BoxFuture;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        // Readiness here would mean "the backend is up", and answering `not
        // ready` would make the caller stop reading the socket -- which is
        // the connection failure this service exists to avoid. The waiting
        // belongs in `call`, where the request is already accepted.
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let endpoint = Arc::clone(&self.endpoint);
        let backend = self.backend.clone();
        let hold = self.hold;
        Box::pin(async move { Ok(hold_then_forward(endpoint, backend, hold, req).await) })
    }
}

/// Wait for the backend, then forward. Never fails -- every outcome is a
/// response, because the alternative is a dropped connection.
async fn hold_then_forward(
    endpoint: Arc<IpcEndpoint>,
    backend: BackendHandle,
    hold: Duration,
    req: Request,
) -> Response<Body> {
    let started = Instant::now();
    let deadline = started + hold;

    loop {
        match backend.settle(deadline).await {
            Settled::Ready => {}
            Settled::Failed(diagnostics) => return pages::compile_error(&diagnostics),
            Settled::StillBuilding => return pages::building(started.elapsed()),
        }

        match endpoint.connect().await {
            Ok(stream) => {
                return match forward(stream, req).await {
                    Ok(response) => response,
                    // The request was consumed, so it cannot be retried on
                    // the next backend. A crash mid-response is a real
                    // failure and reads as one.
                    Err(error) => pages::backend_gone(&error.to_string()),
                };
            }
            Err(error) => {
                // The status said ready, but the child was killed between
                // that read and this connect. Go round again while there is
                // time -- the next backend is usually seconds away.
                if Instant::now() >= deadline {
                    return pages::backend_gone(&error.to_string());
                }
                tokio::time::sleep(RECONNECT_INTERVAL).await;
            }
        }
    }
}

/// The supervisor's complete request path.
///
/// Vite's requests go to Vite; everything else goes to [`BackendService`].
/// Cheap to clone, which `axum::serve` requires: everything inside is an
/// `Arc` or a `Copy`.
#[derive(Clone)]
pub struct Supervisor {
    inner: DevProxyService<BackendService>,
}

impl Supervisor {
    /// Assemble the supervisor around the two IPC endpoints.
    ///
    /// `hold` is how long a request waits for a backend that is down;
    /// [`DEFAULT_HOLD`] is the value `arc dev` uses.
    #[must_use]
    pub fn new(
        vite_endpoint: PathBuf,
        app_endpoint: PathBuf,
        backend: BackendHandle,
        hold: Duration,
    ) -> Self {
        use tower::Layer as _;

        let application = BackendService {
            endpoint: Arc::new(IpcEndpoint::new(app_endpoint)),
            backend,
            hold,
        };
        // `DevProxyLayer::new` reads the asset roots from the environment,
        // which is the same read the application makes under `cargo run
        // --features dev`. Both topologies therefore route by the same table.
        Self {
            inner: DevProxyLayer::new(Some(IpcEndpoint::new(vite_endpoint))).layer(application),
        }
    }

    /// Handle one request directly, without a listener.
    ///
    /// The seam the tests drive. The response is infallible by construction:
    /// every failure has already been turned into a page.
    pub async fn handle(&mut self, req: Request) -> Response<Body> {
        match tower::Service::call(self, req).await {
            Ok(response) => response,
            Err(never) => match never {},
        }
    }
}

impl tower::Service<Request> for Supervisor {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = BoxFuture;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        // Swap in the clone and drive the original: only the original is
        // known ready, and `poll_ready` readiness does not survive cloning.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move { inner.call(req).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axum::http::{Request as HttpRequest, StatusCode};

    /// An endpoint name no other test uses.
    pub(super) fn endpoint(label: &str) -> PathBuf {
        let pid = std::process::id();
        #[cfg(windows)]
        {
            PathBuf::from(format!(r"\\.\pipe\arcature-dev-test-{label}-{pid}"))
        }
        #[cfg(unix)]
        {
            std::env::temp_dir().join(format!("arcature-dev-test-{label}-{pid}.sock"))
        }
    }

    fn get(path: &str) -> Request {
        HttpRequest::builder()
            .uri(path)
            .body(Body::empty())
            .expect("test request should build")
    }

    pub(super) async fn body_of(response: Response<Body>) -> String {
        let bytes = crate::axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the test bodies are small and complete");
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test]
    async fn a_request_that_outlives_the_hold_is_answered_rather_than_dropped() {
        let mut supervisor = Supervisor::new(
            endpoint("hold-vite"),
            endpoint("hold-app"),
            BackendHandle::new(),
            Duration::from_millis(60),
        );
        let response = supervisor.handle(get("/dashboard")).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(body_of(response).await.contains("rebuilding"));
    }

    #[tokio::test]
    async fn a_failed_build_reaches_the_browser_instead_of_a_wait() {
        let backend = BackendHandle::new();
        backend.mark_failed("error[E0308]: expected `Vec<u8>`, found `&str`");
        let mut supervisor = Supervisor::new(
            endpoint("failed-vite"),
            endpoint("failed-app"),
            backend,
            // Long enough that a hold would time the test out rather than
            // let it pass: a compile error must not wait.
            Duration::from_secs(60),
        );
        let response = supervisor.handle(get("/")).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_of(response).await;
        assert!(body.contains("E0308"), "{body}");
        assert!(body.contains("&lt;u8&gt;"), "{body}");
    }

    #[tokio::test]
    async fn readiness_that_turns_out_to_be_stale_is_reported_as_a_dead_backend() {
        let backend = BackendHandle::new();
        backend.mark_ready();
        let mut supervisor = Supervisor::new(
            endpoint("stale-vite"),
            endpoint("stale-app"),
            backend,
            Duration::from_millis(60),
        );
        // Nothing is listening on either endpoint, so the connect fails and
        // the retry window closes.
        let response = supervisor.handle(get("/")).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn a_fresh_handle_starts_out_building_so_first_boot_holds_too() {
        assert!(matches!(
            BackendHandle::new().status(),
            BackendStatus::Building
        ));
    }

    #[test]
    fn a_handle_reports_the_last_status_written_to_it() {
        let backend = BackendHandle::new();
        backend.mark_ready();
        assert!(matches!(backend.status(), BackendStatus::Ready));
        backend.mark_failed("boom");
        assert!(matches!(backend.status(), BackendStatus::Failed(_)));
        backend.mark_building();
        assert!(matches!(backend.status(), BackendStatus::Building));
    }

    #[test]
    fn a_clone_of_a_handle_sees_the_original_writes() {
        // The rebuild loop and every in-flight request hold clones; if they
        // did not share, a request would wait forever on a backend that came
        // up long ago.
        let backend = BackendHandle::new();
        let observer = backend.clone();
        backend.mark_ready();
        assert!(matches!(observer.status(), BackendStatus::Ready));
    }

    #[test]
    fn a_status_written_while_nothing_is_waiting_is_not_thrown_away() {
        // `watch::Sender::send` refuses when the receiver count is zero and
        // does not store the value it refused. A handle has no receivers
        // until a request subscribes, and the first build always finishes
        // before the first request in a quiet session, so `send` would drop
        // exactly the write that matters most: the one that says the backend
        // the developer is about to hit is already up.
        let backend = BackendHandle::new();
        assert_eq!(backend.status.receiver_count(), 0);

        backend.mark_ready();
        assert!(matches!(backend.status(), BackendStatus::Ready));

        backend.mark_failed("error[E0308]: mismatched types");
        match backend.status() {
            BackendStatus::Failed(diagnostics) => assert!(diagnostics.contains("E0308")),
            other => panic!("a write made with no receivers was dropped: {other:?}"),
        }
    }
}

/// The tests that need a real application on the other end of an IPC
/// endpoint. They use [`crate::application::serve_ipc`], which is part of
/// the serve path and therefore gated on the certified runtime.
#[cfg(all(test, feature = "macros"))]
mod topology_tests {
    use super::tests::{body_of, endpoint};
    use super::*;
    use crate::application::serve_ipc::IpcListener;
    use crate::axum::http::{Request as HttpRequest, StatusCode};
    use crate::axum::{Router, routing::get as route};

    /// The application both topologies front. Identical instances, so any
    /// difference in the answers is the topology's doing.
    fn application() -> Router {
        Router::new()
            .route("/", route(|| async { "home" }))
            .route("/api/ping", route(|| async { "pong" }))
    }

    /// A stand-in for Vite: it serves a module and one path the application
    /// does not have, which is what the `404` fallthrough is for.
    fn vite() -> Router {
        Router::new()
            .route("/resources/js/app.tsx", route(|| async { "export {}" }))
            .route("/only-vite-has-this", route(|| async { "from vite" }))
    }

    #[tokio::test]
    async fn a_request_arriving_while_the_backend_is_down_is_answered_once_it_returns() {
        let app_endpoint = endpoint("queued-app");
        let backend = BackendHandle::new();
        let mut supervisor = Supervisor::new(
            endpoint("queued-vite"),
            app_endpoint.clone(),
            backend.clone(),
            Duration::from_secs(10),
        );

        // The backend arrives after the request does. Nothing about the
        // request path may notice: it was accepted on a listener that never
        // closed, so there is no connection error to report.
        let late = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let served = serve_over_ipc(&app_endpoint, application()).await;
            backend.mark_ready();
            served
        });

        let response = supervisor.handle(get("/")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_of(response).await, "home");
        drop(late.await.expect("the backend task should not panic"));
    }

    #[tokio::test]
    async fn a_vite_request_is_served_while_the_backend_is_still_building() {
        let vite_endpoint = endpoint("hmr-vite");
        let served = serve_over_ipc(&vite_endpoint, vite()).await;

        // The backend never becomes ready. A hold of an hour would time the
        // test out if a Vite request waited on it.
        let mut supervisor = Supervisor::new(
            vite_endpoint,
            endpoint("hmr-app"),
            BackendHandle::new(),
            Duration::from_secs(3600),
        );

        let response = supervisor.handle(get("/resources/js/app.tsx")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_of(response).await, "export {}");
        drop(served);
    }

    #[tokio::test]
    async fn both_dev_topologies_answer_the_same_request_the_same_way() {
        use tower::{Layer as _, Service as _};

        let vite_endpoint = endpoint("parity-vite");
        let app_endpoint = endpoint("parity-app");
        let vite_served = serve_over_ipc(&vite_endpoint, vite()).await;
        let app_served = serve_over_ipc(&app_endpoint, application()).await;

        // Topology one -- `cargo run --features dev`: the application owns
        // the port and forwards Vite's requests over IPC.
        let mut in_process = DevProxyLayer::new(Some(IpcEndpoint::new(vite_endpoint.clone())))
            .layer(application().into_service::<Body>());

        // Topology two -- `arc dev`: the supervisor owns the port and both
        // the application and Vite are behind IPC.
        let backend = BackendHandle::new();
        backend.mark_ready();
        let mut supervised = Supervisor::new(
            vite_endpoint,
            app_endpoint,
            backend,
            Duration::from_secs(10),
        );

        for path in [
            "/",                     // the application's own route
            "/api/ping",             // another one, to catch a path-only fluke
            "/resources/js/app.tsx", // Vite's, by prefix
            "/only-vite-has-this",   // Vite's, by the 404 fallthrough
            "/nobody-has-this",      // neither one's
        ] {
            let direct = in_process
                .call(get(path))
                .await
                .expect("the in-process pipeline is infallible");
            let through_supervisor = supervised.handle(get(path)).await;

            assert_eq!(
                direct.status(),
                through_supervisor.status(),
                "the two topologies disagree on the status of {path}"
            );
            assert_eq!(
                body_of(direct).await,
                body_of(through_supervisor).await,
                "the two topologies disagree on the body of {path}"
            );
        }
        drop((vite_served, app_served));
    }

    /// Bind `endpoint` and serve `router` on it until the returned guard is
    /// dropped. Returns once the endpoint actually accepts, so the caller
    /// does not race the listener.
    async fn serve_over_ipc(endpoint: &std::path::Path, router: Router) -> Served {
        let listener = IpcListener::bind(endpoint)
            .await
            .expect("the test endpoint should be creatable");
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router.into_make_service()).await;
        });
        super::super::endpoints::wait_until_listening(endpoint, Duration::from_secs(5), || None)
            .await
            .expect("the test server should start listening");
        Served(task)
    }

    /// Aborts its server when dropped, so one test's endpoint does not
    /// outlive it and confuse the next.
    struct Served(tokio::task::JoinHandle<()>);

    impl Drop for Served {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    fn get(path: &str) -> Request {
        HttpRequest::builder()
            .uri(path)
            .body(Body::empty())
            .expect("test request should build")
    }
}
