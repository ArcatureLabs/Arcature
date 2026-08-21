//! The dev-only Unified Application Graph endpoint.
//!
//! `GET /_arcature/uag.json` answers with the same bytes
//! [`UagArtifact::to_json`](crate::uag::UagArtifact::to_json) writes: every
//! module, every route with its parameters, pages and policies, every job,
//! command and schedule, and the prop schema of every registered page. It
//! exists so `arc dev` can regenerate `resources/js/generated/` after a
//! restart by fetching one URL from the process it just started, instead of
//! the alternatives -- a `build.rs`, or a second binary target linked on
//! every loop -- both of which pay for the graph with build time on the edit
//! path that has to stay fast.
//!
//! # Why it is merged, not layered
//!
//! Like [`Health`](crate::application::health::Health), this router is
//! `merge`d beside the application router rather than layered over it. The
//! graph is a static description of the process; answering it must not depend
//! on a session store, must not be turned into a maintenance `503`, and must
//! not be refused by a rate limit while `arc dev` is hammering the reload
//! loop. See [`crate::application::pipeline`] for where it sits.
//!
//! # The gate
//!
//! This endpoint serves the application's entire internal structure. Shipping
//! it by accident is the failure to design against, so reaching it takes
//! three independent conditions that are all satisfied at build time, and
//! none of which a request can influence:
//!
//! 1. **The `uag` cargo feature.** This module does not exist without it, so
//!    a binary built without the feature has no route, no handler and no
//!    artifact in it. `uag` is not in `arcature`'s default features; the
//!    generated application turns it on only through its own `dev` feature.
//! 2. **An explicit builder call.** [`ApplicationBuilder::uag_endpoint`] must
//!    be called with the application graph. Absent, the pipeline slot stays
//!    `None` and nothing is merged. The scaffold's `bootstrap/app.rs` puts
//!    that call behind `#[cfg(feature = "dev")]`.
//! 3. **A debug profile.** [`UagEndpoint::allowed`] is false when
//!    `debug_assertions` is off, and the builder then refuses the
//!    registration and says so once on stderr. This is the backstop for the
//!    case the other two gates cannot catch: someone builds a release binary
//!    with the dev feature accidentally left on. Overriding it takes
//!    `debug-assertions = true` in the release profile, which is a deliberate
//!    line in a manifest rather than an oversight.
//!
//! There is deliberately no environment variable, header or query parameter
//! that enables the endpoint. Anything a request or a process environment can
//! flip is something an attacker who has reached either can flip, and the
//! whole point of the gate is that a running production binary has no state
//! in which it serves this.
//!
//! [`ApplicationBuilder::uag_endpoint`]: super::ApplicationBuilder::uag_endpoint

use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::routing::RouterState;
use crate::uag::UagArtifact;

/// The path the artifact is served from.
///
/// Under `/_arcature/` with the rest of the framework's own endpoints, so an
/// application can reserve that one prefix and know no route of its own will
/// ever collide with a future one.
pub const PATH: &str = "/_arcature/uag.json";

/// A serialized [`UagArtifact`], ready to serve.
///
/// Cheap to clone (`Arc`-backed). The bytes are produced once, when the
/// builder composes the router, because the graph is `&'static` metadata that
/// cannot change while the process runs -- serializing per request would burn
/// time on every `arc dev` reload for an answer that is identical every time.
#[derive(Clone)]
pub struct UagEndpoint(Arc<[u8]>);

impl UagEndpoint {
    /// Serialize `artifact` and hold the result.
    ///
    /// Serialization cannot fail here: [`UagArtifact`] is plain data with
    /// `String`-keyed maps, and `serde_json` only errors on a non-string map
    /// key or a `Serialize` impl that returns one, neither of which this type
    /// can produce. The `expect` documents that rather than propagating a
    /// `Result` every caller would have to pretend to handle.
    #[must_use]
    pub fn new(artifact: &UagArtifact) -> Self {
        let json = artifact
            .to_json()
            .expect("UagArtifact is plain String-keyed data, so serialization cannot fail");
        UagEndpoint(json.into())
    }

    /// Whether this build is allowed to serve the graph at all.
    ///
    /// The third gate described in the module documentation: false in any
    /// build with `debug_assertions` off, which is every default release
    /// profile.
    #[must_use]
    pub fn allowed() -> bool {
        cfg!(debug_assertions)
    }

    /// The serialized artifact.
    #[must_use]
    pub fn json(&self) -> &[u8] {
        &self.0
    }

    /// The single route, ready to merge beside the application router.
    pub fn router<S: RouterState>(&self) -> Router<S> {
        let endpoint = self.clone();
        Router::new().route(
            PATH,
            axum::routing::get(move || {
                let endpoint = endpoint.clone();
                async move { endpoint.response() }
            }),
        )
    }

    /// The response: the artifact, uncached.
    ///
    /// `no-store` because the whole point is to read the graph of the process
    /// that is answering right now; a cached copy would be the previous
    /// build's graph, which is exactly the bug this endpoint exists to avoid.
    fn response(&self) -> Response {
        let mut response = (StatusCode::OK, Vec::from(&*self.0)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, max-age=0"),
        );
        response
    }
}

impl std::fmt::Debug for UagEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UagEndpoint")
            .field("path", &PATH)
            .field("bytes", &self.0.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dx::application_graph::ApplicationGraph;
    use crate::inertia::contracts::PageContracts;

    fn endpoint() -> UagEndpoint {
        let graph = ApplicationGraph::new_unchecked(Vec::new());
        let contracts = PageContracts::new().artifact();
        UagEndpoint::new(&crate::uag::build(&graph, &contracts))
    }

    #[test]
    fn the_endpoint_serves_the_same_bytes_the_artifact_writes() {
        let graph = ApplicationGraph::new_unchecked(Vec::new());
        let contracts = PageContracts::new().artifact();
        let artifact = crate::uag::build(&graph, &contracts);
        let expected = artifact.to_json().expect("plain data serializes");
        assert_eq!(UagEndpoint::new(&artifact).json(), expected.as_slice());
    }

    #[test]
    fn the_response_is_json_and_uncached() {
        let response = endpoint().response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/json"))
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&HeaderValue::from_static("no-store, max-age=0"))
        );
    }

    #[test]
    fn a_build_with_debug_assertions_off_is_never_allowed_to_serve_the_graph() {
        // The test suite runs with debug assertions on, so this asserts the
        // mapping rather than the release answer -- the point being that
        // `allowed` reads the profile and nothing else, in particular nothing
        // a request or an environment variable can reach.
        assert_eq!(UagEndpoint::allowed(), cfg!(debug_assertions));
    }

    #[tokio::test]
    async fn an_application_that_did_not_ask_for_it_does_not_serve_the_graph() {
        // The gate that matters most is the one nobody types. An application
        // built without `.uag_endpoint(..)` must have no route here at all --
        // not a `403`, which would still confirm the endpoint exists and
        // still be one misconfiguration away from answering.
        use tower::ServiceExt as _;

        let router = crate::application::Application::new()
            .routes(crate::routing::Routes::new([crate::routing::Route::get(
                "/",
                || async { "ok" },
            )]))
            .build()
            .into_router();

        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(PATH)
                    .body(axum::body::Body::empty())
                    .expect("a GET with an empty body is a valid request"),
            )
            .await
            .expect("the router is infallible");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
