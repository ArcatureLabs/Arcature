//! `RedirectMapper` -- the layer that finishes a [`RedirectResponse`].
//!
//! [`RedirectResponse::route`] and [`RedirectResponse::with`] need two things
//! `IntoResponse` cannot reach. `into_response(self)` takes no request and no
//! application context, so it has no route table to turn `"users.show"` into
//! `/users/7`, and no session to put flash data in. On its own it can only
//! handle a literal path.
//!
//! So it does not try. `into_response` builds the best response it can and
//! then puts the *unresolved builder* into the response extensions. This
//! layer, which runs outside the handler and therefore has both the table it
//! was constructed with and the session from the request, takes the builder
//! back out and finishes the job.
//!
//! # Why a response extension rather than a registry
//!
//! The obvious alternatives are all worse. A process-global route table
//! breaks the moment two test applications exist in one process. A
//! task-local is the hidden ambient state this crate refuses everywhere else.
//! Threading the table through every handler signature makes `redirect()`
//! cost an extractor. A response extension carries the builder exactly as far
//! as it needs to go -- from the handler to the layer directly above it --
//! and nothing else in the process can see it.
//!
//! # When the layer is absent
//!
//! Nothing breaks and nothing lies. The extension rides along unread, and the
//! caller gets the fallback response `into_response` already built: a literal
//! path redirects, a named route is a `400`, and flash data is dropped. That
//! is the behaviour of a build that never installed the mapper, and it is why
//! the mapper is installed by default.

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::http::{Request, Response};
use axum::response::IntoResponse as _;
use tower::{Layer, Service};

use super::table::RouteTable;
use crate::http::response::RedirectResponse;

/// The layer that resolves named-route redirects and persists flash data.
///
/// Constructed from a [`RouteTable`], which is a snapshot -- the layer holds
/// no `Routes<S>` and so is not generic over the application state, which is
/// what lets one live in a pipeline shared by every state type.
///
/// ```
/// use arcature::routing::{RedirectMapper, Route, Routes};
///
/// let routes: Routes = Routes::new([
///     Route::get("/users/{id}", || async { "ok" }).name("users.show"),
/// ]);
/// let mapper = RedirectMapper::new(routes.table());
///
/// assert_eq!(mapper.table().template("users.show"), Some("/users/{id}"));
/// ```
#[derive(Debug, Clone, Default)]
pub struct RedirectMapper {
    table: RouteTable,
}

impl RedirectMapper {
    /// Build a mapper that resolves names against `table`.
    #[must_use]
    pub fn new(table: RouteTable) -> Self {
        RedirectMapper { table }
    }

    /// The table this mapper resolves against.
    #[must_use]
    pub fn table(&self) -> &RouteTable {
        &self.table
    }
}

impl<S> Layer<S> for RedirectMapper {
    type Service = RedirectMapperService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RedirectMapperService {
            inner,
            table: self.table.clone(),
        }
    }
}

/// The service [`RedirectMapper`] wraps around.
#[derive(Debug, Clone)]
pub struct RedirectMapperService<S> {
    inner: S,
    table: RouteTable,
}

impl<S> Service<Request<axum::body::Body>> for RedirectMapperService<S>
where
    S: Service<
            Request<axum::body::Body>,
            Response = Response<axum::body::Body>,
            Error = Infallible,
        > + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = Response<axum::body::Body>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<axum::body::Body>) -> Self::Future {
        let table = self.table.clone();

        // `redirect().back()` needs the `Referer`, which is on the request
        // and not on the response. Captured as an owned `String` rather than
        // held by reference, because the resolution happens after the handler
        // has consumed the request.
        let referer = request
            .headers()
            .get(axum::http::header::REFERER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        // The session has to be taken from the *request*, before the handler
        // runs, because the response does not carry it. Cloning a
        // `tower_sessions::Session` shares the same record, so writing
        // through this handle after the handler returns reaches the session
        // the session layer saves on the way out.
        #[cfg(feature = "auth")]
        let session = request
            .extensions()
            .get::<tower_sessions::Session>()
            .cloned();

        // Swap in the clone and drive the original: only the original is
        // known ready, and `poll_ready` readiness does not survive cloning.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            let mut response = inner.call(request).await?;

            // Taking rather than reading: a resolved redirect must not leave
            // an unresolved builder behind for a second mapper to redo.
            let Some(pending) = response.extensions_mut().remove::<RedirectResponse>() else {
                return Ok(response);
            };

            #[cfg(feature = "auth")]
            if let Some(session) = session {
                pending.persist_flash(&session).await;
            }

            match pending.resolve(&table, referer.as_deref()) {
                Ok(resolved) => Ok(graft(response, resolved.into_response())),
                // A redirect to a route that does not exist, or with too few
                // parameters, is a bug in the application rather than
                // anything the browser did -- so it is a `500`, not the `400`
                // the unmapped fallback produces.
                Err(error) => Ok(crate::error::Error::Other(error.to_string()).into_response()),
            }
        })
    }
}

/// Put `resolved` in place of `original`, keeping what the handler added.
///
/// A handler can return more than the builder -- `(headers, redirect())` is
/// ordinary axum, and a `Set-Cookie` attached that way is not optional
/// decoration. So the resolved redirect supplies the status, the `Location`
/// and the (empty) body, and every other header the original carried comes
/// across.
///
/// Entity headers are the exception: `content-type` and `content-length`
/// describe the body being discarded, and copying them onto an empty one
/// would announce a payload that is not there. `Location` is skipped for the
/// same reason in reverse -- the resolved one is the whole point.
fn graft(
    original: Response<axum::body::Body>,
    resolved: Response<axum::body::Body>,
) -> Response<axum::body::Body> {
    use axum::http::header;

    let (original, _) = original.into_parts();
    let (mut parts, body) = resolved.into_parts();
    for (name, value) in &original.headers {
        if matches!(
            *name,
            header::CONTENT_TYPE | header::CONTENT_LENGTH | header::LOCATION
        ) {
            continue;
        }
        parts.headers.append(name, value.clone());
    }
    parts.extensions.extend(original.extensions);
    Response::from_parts(parts, body)
}

#[cfg(test)]
mod tests {
    use super::{RedirectMapper, RedirectMapperService};
    use crate::http::response::{RedirectResponse, redirect};
    use crate::routing::table::RouteTable;
    use axum::body::Body;
    use axum::http::{Request, Response, StatusCode, header};
    use axum::response::IntoResponse;
    use std::convert::Infallible;
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use tower::{Layer, Service};

    fn table() -> RouteTable {
        [("users.show", "/users/{id}"), ("home", "/")]
            .into_iter()
            .collect()
    }

    /// A leaf service that answers every request with a freshly built response.
    #[derive(Clone)]
    struct Fixed(Arc<dyn Fn() -> Response<Body> + Send + Sync>);

    impl Service<Request<Body>> for Fixed {
        type Response = Response<Body>;
        type Error = Infallible;
        type Future = std::future::Ready<Result<Response<Body>, Infallible>>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _: Request<Body>) -> Self::Future {
            std::future::ready(Ok((self.0)()))
        }
    }

    async fn through(make: impl Fn() -> Response<Body> + Send + Sync + 'static) -> Response<Body> {
        let mut service: RedirectMapperService<Fixed> =
            RedirectMapper::new(table()).layer(Fixed(Arc::new(make)));
        service
            .call(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn location(response: &Response<Body>) -> &str {
        response
            .headers()
            .get(header::LOCATION)
            .expect("a redirect carries a Location")
            .to_str()
            .unwrap()
    }

    #[tokio::test]
    async fn a_named_route_is_resolved_to_its_path() {
        let response = through(|| redirect().route("users.show", 7u64).into_response()).await;
        assert_eq!(location(&response), "/users/7");
    }

    #[test]
    fn a_named_route_without_the_mapper_is_still_the_documented_failure() {
        // The same builder with no layer above it: the fallback path.
        let response = redirect().route("users.show", 7u64).into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_unknown_route_name_is_a_server_error_not_a_client_error() {
        let response = through(|| redirect().route("users.edit", 7u64).into_response()).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn a_literal_path_passes_through_unchanged() {
        let response = through(|| redirect().to("/dashboard").into_response()).await;
        assert_eq!(location(&response), "/dashboard");
    }

    #[tokio::test]
    async fn permanent_survives_the_round_trip_through_the_extension() {
        let response = through(|| redirect().route("home", ()).permanent().into_response()).await;
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(location(&response), "/");
    }

    #[tokio::test]
    async fn back_follows_the_referer_when_the_mapper_can_see_it() {
        let mut service = RedirectMapper::new(table())
            .layer(Fixed(Arc::new(|| redirect().back().into_response())));
        let response = service
            .call(
                Request::builder()
                    .uri("/users/7/edit")
                    .header(header::REFERER, "/users/7")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(location(&response), "/users/7");
    }

    #[tokio::test]
    async fn back_refuses_an_offsite_referer_rather_than_following_it() {
        let mut service = RedirectMapper::new(table())
            .layer(Fixed(Arc::new(|| redirect().back().into_response())));
        let response = service
            .call(
                Request::builder()
                    .uri("/pay")
                    .header(header::REFERER, "https://evil.example/phish")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(location(&response), "/");
    }

    #[tokio::test]
    async fn back_falls_back_to_the_root_with_no_referer() {
        let response = through(|| redirect().back().into_response()).await;
        assert_eq!(location(&response), "/");
    }

    #[tokio::test]
    async fn a_header_the_handler_attached_survives_resolution() {
        let response = through(|| {
            (
                [("set-cookie", "session=abc")],
                redirect().route("home", ()),
            )
                .into_response()
        })
        .await;
        assert_eq!(location(&response), "/");
        assert_eq!(response.headers().get("set-cookie").unwrap(), "session=abc");
    }

    #[tokio::test]
    async fn the_discarded_body_does_not_leave_its_content_type_behind() {
        // The unmapped fallback for a named route is a JSON error body. Once
        // the redirect resolves, that body is gone and its headers must go
        // with it.
        let response = through(|| redirect().route("home", ()).into_response()).await;
        assert!(response.headers().get(header::CONTENT_TYPE).is_none());
        assert!(response.headers().get(header::CONTENT_LENGTH).is_none());
    }

    #[tokio::test]
    async fn a_response_that_is_not_a_redirect_is_left_alone() {
        let response = through(|| (StatusCode::OK, "hello").into_response()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::LOCATION).is_none());
    }

    #[tokio::test]
    async fn the_builder_is_removed_so_a_second_mapper_has_nothing_to_redo() {
        let response = through(|| redirect().route("home", ()).into_response()).await;
        assert!(response.extensions().get::<RedirectResponse>().is_none());
    }
}
