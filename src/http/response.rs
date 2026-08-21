//! Response vocabulary: the coherent set of response helpers a controller
//! returns.
//!
//! Every controller returns [`Result<Response>`](crate::Result). The helpers
//! here build the common shapes: `redirect()`, `json()`, and (with the
//! `inertia` feature) `inertia!()`.

use crate::error::{Error, Result};
#[cfg(any(feature = "api", feature = "inertia"))]
use axum::Json;
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Redirect, Response};

/// A builder for HTTP redirect responses with named-route support.
///
/// Construct with [`redirect()`]; chain `.to()`, `.route()`, `.with()` (flash
/// data), `.permanent()`, and finish by returning it from a handler (it
/// implements [`IntoResponse`]).
#[derive(Debug, Clone)]
pub struct RedirectResponse {
    target: RedirectTarget,
    permanent: bool,
    flash: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
enum RedirectTarget {
    Path(String),
    Route {
        name: String,
        params: Vec<RouteParam>,
    },
    Back,
}

/// One filled route parameter.
///
/// An implementation detail of [`IntoRouteParams`], public only because that
/// trait's method names it. The inner field is crate-private, so no outside
/// type can produce one -- which is what seals the trait, without a separate
/// `Sealed` supertrait that has to be kept in step with the impl list.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct RouteParam(pub(crate) String);

impl RedirectResponse {
    /// Redirect to a path.
    #[must_use]
    pub fn to(mut self, path: impl Into<String>) -> Self {
        self.target = RedirectTarget::Path(path.into());
        self
    }

    /// Redirect to a named route. `params` fill the route's URI parameters in
    /// declaration order.
    ///
    /// Resolving a name to a path needs the application's route table, which
    /// no `IntoResponse` impl can reach. The
    /// [`RedirectMapper`](crate::routing::RedirectMapper) layer holds one and
    /// finishes the redirect above the handler; the builder rides up to it in
    /// the response extensions. The application builder installs that layer,
    /// so this works by default.
    ///
    /// Without the layer the name cannot be resolved and this is a **400 Bad
    /// Request** -- the same as before the mapper existed. An unknown name
    /// *with* the layer is a `500`, because a redirect to a route nobody
    /// declared is a bug in the application, not something the browser did.
    #[must_use]
    pub fn route(mut self, name: impl Into<String>, params: impl IntoRouteParams) -> Self {
        self.target = RedirectTarget::Route {
            name: name.into(),
            params: params.into_params(),
        };
        self
    }

    /// Redirect back to the referer (or `/` if none).
    #[must_use]
    pub fn back(mut self) -> Self {
        self.target = RedirectTarget::Back;
        self
    }

    /// Make the redirect permanent (308 instead of 303/307).
    #[must_use]
    pub fn permanent(mut self) -> Self {
        self.permanent = true;
        self
    }

    /// Attach flash data carried over the redirect: written to the session by
    /// the [`RedirectMapper`](crate::routing::RedirectMapper) layer and read
    /// once by the next request's [`Flash`](crate::auth::Flash) extractor.
    /// Repeated calls append, and a repeated key overwrites.
    ///
    /// This is the arbitrary key/value half of flashing -- `"status"`,
    /// `"deleted_id"`. The levelled half (`success`/`error`/`warning`/`info`)
    /// is [`Flash`](crate::auth::Flash)'s own methods; the two live in
    /// separate session keys and do not tread on each other.
    ///
    /// Needs the `auth` feature for the session to write into. Without it the
    /// data is recorded on the builder and dropped, because there is nowhere
    /// to put it.
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.flash.push((key.into(), value.into()));
        self
    }

    /// Resolve the redirect to an axum [`Redirect`], given the route table
    /// (for named routes) and the request's `Referer` (for
    /// [`back`](Self::back)).
    ///
    /// Called by [`RedirectMapper`](crate::routing::RedirectMapper), which is
    /// the only place both arguments exist at once.
    ///
    /// # Errors
    ///
    /// The target failed [`validate_redirect_target`], the name is not in the
    /// table, or the table's template wants more parameters than were given.
    pub(crate) fn resolve(
        &self,
        table: &crate::routing::RouteTable,
        referer: Option<&str>,
    ) -> Result<Redirect> {
        let path = match &self.target {
            RedirectTarget::Path(p) => {
                validate_redirect_target(p)?;
                p.clone()
            }
            RedirectTarget::Route { name, params } => {
                let params: Vec<&str> = params.iter().map(|RouteParam(s)| s.as_str()).collect();
                table.url_for(name, &params)?
            }
            // A `Referer` is whatever the other end chose to send, so it goes
            // through the same open-redirect check as a path the application
            // wrote -- and falls back to `/` rather than erroring, because a
            // hostile or absent header must not turn a working form into a
            // `500`.
            RedirectTarget::Back => referer
                .filter(|r| validate_redirect_target(r).is_ok())
                .unwrap_or("/")
                .to_string(),
        };
        Ok(if self.permanent {
            Redirect::permanent(&path)
        } else {
            Redirect::temporary(&path)
        })
    }

    /// Write the `.with(..)` data into the session for the next request.
    ///
    /// A failure is reported and swallowed. Turning it into a `500` would
    /// throw away the redirect too -- and the write the redirect is
    /// confirming has already happened, so the user would be told their
    /// successful action failed. A missing toast is the smaller loss.
    #[cfg(feature = "auth")]
    pub(crate) async fn persist_flash(&self, session: &tower_sessions::Session) {
        if self.flash.is_empty() {
            return;
        }
        // Merge rather than overwrite: two redirects can flash in one
        // session lifetime, and a `.with(..)` on the second must not erase
        // the first.
        let mut data: std::collections::BTreeMap<String, String> =
            match session.get(crate::auth::FLASH_DATA_KEY).await {
                Ok(existing) => existing.unwrap_or_default(),
                Err(error) => return flash_write_failed(&error),
            };
        data.extend(self.flash.iter().cloned());
        if let Err(error) = session.insert(crate::auth::FLASH_DATA_KEY, &data).await {
            flash_write_failed(&error);
        }
    }
}

impl IntoResponse for RedirectResponse {
    fn into_response(self) -> Response {
        // Two halves. The response below is what a build with no
        // `RedirectMapper` gets, and it is honest: a literal path works, a
        // named route is a `400`, flash data is dropped. The builder then
        // rides up in the response extensions so the mapper -- which has the
        // route table and the session, neither of which is reachable from
        // here -- can replace all three answers with the real ones.
        let mut response = match &self.target {
            RedirectTarget::Path(p) => {
                if validate_redirect_target(p).is_err() {
                    return Error::BadRequest("invalid redirect target".into()).into_response();
                }
                if self.permanent {
                    Redirect::permanent(p).into_response()
                } else {
                    Redirect::temporary(p).into_response()
                }
            }
            RedirectTarget::Route { .. } => {
                Error::BadRequest("named-route redirect requires the route table".into())
                    .into_response()
            }
            RedirectTarget::Back => Redirect::to("/").into_response(),
        };
        response.extensions_mut().insert(self);
        response
    }
}

/// Report a flash-data session failure.
///
/// `tracing` arrives with the `observe` feature, and `auth` does not imply
/// it. Rather than gate two call sites, the whole decision lives here: with
/// `observe` the failure is a warning, without it there is nowhere to put it
/// and it is dropped. Either way the redirect still happens.
#[cfg(feature = "auth")]
fn flash_write_failed(error: &tower_sessions::session::Error) {
    #[cfg(feature = "observe")]
    tracing::warn!(%error, "flash data could not be stored; the redirect still happened");
    #[cfg(not(feature = "observe"))]
    let _ = error;
}

/// Validate a redirect target against open-redirect attacks. Absolute URLs to
/// external hosts are rejected; only same-origin paths and relative URLs are
/// allowed.
fn validate_redirect_target(path: &str) -> Result<()> {
    // Allow scheme-relative or absolute external URLs only when explicitly
    // configured. By default reject anything that looks like a URL with an
    // authority.
    if path.starts_with("//") || path.starts_with("http://") || path.starts_with("https://") {
        // Same-origin absolute URLs are fine when the host matches; without a
        // configured allowed host we conservatively reject.
        return Err(Error::Redirect("external redirect not allowed".into()));
    }
    Ok(())
}

/// Trait for values that can fill a named route's parameters.
///
/// Public because it is the bound on [`RedirectResponse::route`]. Effectively
/// sealed: implementing it requires producing a [`RouteParam`], whose field is
/// crate-private.
pub trait IntoRouteParams {
    #[doc(hidden)]
    fn into_params(self) -> Vec<RouteParam>;
}

/// A route with no parameters: `redirect().route("home", ())`.
impl IntoRouteParams for () {
    fn into_params(self) -> Vec<RouteParam> {
        Vec::new()
    }
}

impl IntoRouteParams for &str {
    fn into_params(self) -> Vec<RouteParam> {
        vec![RouteParam(self.to_string())]
    }
}

impl IntoRouteParams for String {
    fn into_params(self) -> Vec<RouteParam> {
        vec![RouteParam(self)]
    }
}

impl IntoRouteParams for i64 {
    fn into_params(self) -> Vec<RouteParam> {
        vec![RouteParam(self.to_string())]
    }
}

impl IntoRouteParams for u64 {
    fn into_params(self) -> Vec<RouteParam> {
        vec![RouteParam(self.to_string())]
    }
}

#[cfg(feature = "database")]
impl IntoRouteParams for uuid::Uuid {
    fn into_params(self) -> Vec<RouteParam> {
        vec![RouteParam(self.to_string())]
    }
}

impl<T: IntoRouteParams> IntoRouteParams for Vec<T> {
    fn into_params(self) -> Vec<RouteParam> {
        self.into_iter().flat_map(T::into_params).collect()
    }
}

macro_rules! impl_into_route_params_tuple {
    ($($T:ident),* $(,)?) => {
        impl<$($T: IntoRouteParams),*> IntoRouteParams for ($($T,)*) {
            // The destructuring below reuses each type parameter's name as a
            // value binding, which is upper-case by necessity.
            #[allow(non_snake_case, reason = "bindings are named after the type parameters")]
            fn into_params(self) -> Vec<RouteParam> {
                let ($($T,)*) = self;
                let mut out = Vec::new();
                $( out.extend($T.into_params()); )*
                out
            }
        }
    };
}

impl_into_route_params_tuple!(A);
impl_into_route_params_tuple!(A, B);
impl_into_route_params_tuple!(A, B, C);
impl_into_route_params_tuple!(A, B, C, D);

/// Begin a redirect response builder.
#[must_use]
pub fn redirect() -> RedirectResponse {
    RedirectResponse {
        target: RedirectTarget::Path("/".into()),
        permanent: false,
        flash: Vec::new(),
    }
}

/// Build a JSON response from any serializable value. Requires the `api` or
/// `inertia` feature (serde).
#[cfg(any(feature = "api", feature = "inertia"))]
pub fn json<T: serde::Serialize>(value: T) -> Response {
    Json(value).into_response()
}

/// A plain-text response.
pub fn text<S: Into<String>>(status: axum::http::StatusCode, body: S) -> Response {
    (
        status,
        [(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        )],
        body.into(),
    )
        .into_response()
}

/// A no-content (204) response.
pub fn no_content() -> Response {
    axum::http::StatusCode::NO_CONTENT.into_response()
}
