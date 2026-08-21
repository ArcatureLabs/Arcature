//! [`TestApp`] -- the application under test, held in a value.
//!
//! The default mode drives the composed router directly as a
//! `tower::Service`: no listener, no port allocation, no "wait for the server
//! to be up" sleep, and no teardown race when the test ends. The router is
//! cloned per request, which is what axum itself does per connection.
//!
//! [`TestApp::serve`] is the escape hatch for the cases where the transport
//! is the thing under test -- a WebSocket upgrade, a client that only speaks
//! to a URL. It binds `127.0.0.1:0`, so the OS picks a free port and two
//! tests running at once cannot collide.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request};
use axum::response::Response;
use tower::Service;

use super::request::TestRequest;

/// An application booted in-process and driven as a `tower::Service`.
///
/// Cheap to clone: every clone shares one router and one session store, so a
/// request built from a clone sees the same application.
#[derive(Clone)]
pub struct TestApp {
    inner: Arc<Inner>,
}

struct Inner {
    router: Router,
    #[cfg(feature = "auth")]
    sessions: Option<super::session::TestSessions>,
}

impl TestApp {
    /// Boot a stateless [`Application`](crate::Application).
    ///
    /// The router-level pipeline (body limit, timeout, session, CSRF,
    /// Inertia, user layers) is already composed by
    /// [`ApplicationBuilder::build`](crate::ApplicationBuilder::build), so
    /// what the test drives is the pipeline the application will run in
    /// production -- not a bare route table.
    #[must_use]
    pub fn new(app: crate::Application<()>) -> Self {
        Self::from_router(app.into_router())
    }

    /// Boot a stateful [`Application`](crate::Application) with the state it
    /// runs on.
    ///
    /// The state is a value the test supplies, because a state is built from
    /// live resources -- a pool, a cache, a mailer -- and the harness has no
    /// business inventing any of them. A test that cannot build the real
    /// state should say so by not running, rather than by running against a
    /// substitute that answers differently.
    #[must_use]
    pub fn with_state<S>(app: crate::Application<S>, state: S) -> Self
    where
        S: crate::RouterState,
    {
        Self::from_router(app.into_router().with_state(state))
    }

    /// Boot from a router directly.
    ///
    /// The escape hatch for a router assembled by hand, and what the two
    /// constructors above go through.
    #[must_use]
    pub fn from_router(router: Router) -> Self {
        Self {
            inner: Arc::new(Inner {
                router,
                #[cfg(feature = "auth")]
                sessions: None,
            }),
        }
    }
}

impl TestApp {
    /// Attach the session store the application was built with.
    ///
    /// [`acting_as`](TestRequest::acting_as) and
    /// [`with_session`](TestRequest::with_session) write a record into this
    /// store and send the matching signed cookie. Without it there is nowhere
    /// to write, and both methods panic at send time rather than quietly
    /// producing an anonymous request -- a test that believes it is logged in
    /// and is not passes for the wrong reason.
    #[cfg(feature = "auth")]
    #[must_use]
    pub fn with_sessions(self, sessions: super::session::TestSessions) -> Self {
        Self {
            inner: Arc::new(Inner {
                router: self.inner.router.clone(),
                sessions: Some(sessions),
            }),
        }
    }

    /// The session store this app was given, if any.
    #[cfg(feature = "auth")]
    pub(crate) fn sessions(&self) -> Option<&super::session::TestSessions> {
        self.inner.sessions.as_ref()
    }

    /// Begin a `GET` request.
    #[must_use]
    pub fn get(&self, path: impl Into<String>) -> TestRequest {
        self.request(Method::GET, path)
    }

    /// Begin a `POST` request.
    #[must_use]
    pub fn post(&self, path: impl Into<String>) -> TestRequest {
        self.request(Method::POST, path)
    }

    /// Begin a `PUT` request.
    #[must_use]
    pub fn put(&self, path: impl Into<String>) -> TestRequest {
        self.request(Method::PUT, path)
    }

    /// Begin a `PATCH` request.
    #[must_use]
    pub fn patch(&self, path: impl Into<String>) -> TestRequest {
        self.request(Method::PATCH, path)
    }

    /// Begin a `DELETE` request.
    #[must_use]
    pub fn delete(&self, path: impl Into<String>) -> TestRequest {
        self.request(Method::DELETE, path)
    }

    /// Begin a `HEAD` request.
    #[must_use]
    pub fn head(&self, path: impl Into<String>) -> TestRequest {
        self.request(Method::HEAD, path)
    }

    /// Begin a request with an arbitrary method.
    #[must_use]
    pub fn request(&self, method: Method, path: impl Into<String>) -> TestRequest {
        TestRequest::new(self.clone(), method, path.into())
    }
}

impl TestApp {
    /// Drive one request through the composed router.
    ///
    /// `poll_ready` before `call` is the `tower::Service` contract even
    /// though an axum `Router` is always ready; honouring it here means the
    /// harness exercises the same protocol a real server does.
    pub(crate) async fn dispatch(&self, request: Request<Body>) -> Response {
        let mut router = self.inner.router.clone();
        std::future::poll_fn(|cx| <Router as Service<Request<Body>>>::poll_ready(&mut router, cx))
            .await
            .expect("axum Router::poll_ready is infallible");
        router
            .call(request)
            .await
            .expect("axum Router::call is infallible")
    }

    /// Bind `127.0.0.1:0` and serve this application over real HTTP.
    ///
    /// For the tests the in-process mode cannot express: a WebSocket
    /// upgrade, a client library that only takes a URL. The OS picks the
    /// port, so concurrent tests cannot collide on one.
    ///
    /// The returned [`TestServer`] stops the server when it is dropped.
    ///
    /// # Errors
    ///
    /// Returns the `std::io::Error` from binding the loopback listener.
    pub async fn serve(&self) -> std::io::Result<TestServer> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let router = self.inner.router.clone();
        let task = tokio::spawn(async move {
            // A serve error here is the listener going away, which is what
            // dropping the `TestServer` does on purpose.
            let _ = axum::serve(listener, router.into_make_service()).await;
        });
        Ok(TestServer { address, task })
    }
}

impl std::fmt::Debug for TestApp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = formatter.debug_struct("TestApp");
        #[cfg(feature = "auth")]
        out.field("has_sessions", &self.inner.sessions.is_some());
        out.finish_non_exhaustive()
    }
}

/// A running loopback HTTP server for one test.
///
/// Holds the bound address and the task serving on it. Dropping it aborts
/// the task and closes the listener, so a test that returns early does not
/// leave a server behind for the next one to find.
#[derive(Debug)]
pub struct TestServer {
    address: std::net::SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    /// The bound loopback address, port included.
    #[must_use]
    pub fn address(&self) -> std::net::SocketAddr {
        self.address
    }

    /// The base URL, without a trailing slash (e.g. `http://127.0.0.1:54321`).
    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// A `ws://` URL for `path`, for WebSocket clients.
    #[must_use]
    pub fn ws_url(&self, path: &str) -> String {
        format!("ws://{}{path}", self.address)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// What `#[arcature::test(app = ...)]` accepts as the application under test.
///
/// The attribute evaluates its `app` expression and passes the value through
/// this trait, so the same attribute works whether a test's helper hands back
/// a built [`Application`](crate::Application), a bare
/// [`Router`](axum::Router), or a [`TestApp`] it has already configured with
/// a session store.
pub trait IntoTestApp {
    /// Convert into the application the test will drive.
    fn into_test_app(self) -> TestApp;
}

impl IntoTestApp for TestApp {
    fn into_test_app(self) -> TestApp {
        self
    }
}

impl IntoTestApp for Router {
    fn into_test_app(self) -> TestApp {
        TestApp::from_router(self)
    }
}

impl IntoTestApp for crate::Application<()> {
    fn into_test_app(self) -> TestApp {
        TestApp::new(self)
    }
}
