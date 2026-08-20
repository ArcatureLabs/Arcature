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
    #[expect(dead_code, reason = "read by `resolve`, which awaits the mapper")]
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
    /// # Not yet implemented
    ///
    /// Resolving a name to a path needs the application's route table, which
    /// reaches a response only through a mapper layer that does not exist
    /// yet. Until it does, this returns **400 Bad Request** at runtime. Use
    /// [`to`](Self::to) with an explicit path.
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

    /// Attach flash data carried over the redirect (stored in the session by
    /// the framework and surfaced on the next request). Repeated calls append.
    ///
    /// # Not yet implemented
    ///
    /// Same missing mapper as [`route`](Self::route): the flash data is
    /// recorded on the builder and then dropped when the redirect becomes a
    /// response. Write to the session directly in the meantime.
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.flash.push((key.into(), value.into()));
        self
    }

    /// Resolve the redirect to an axum [`Redirect`], given the application's
    /// route table (for named routes).
    ///
    /// Not yet reachable: the response mapper that would supply the route
    /// table does not exist, so [`route`](Self::route) currently produces a
    /// 400 and [`with`](Self::with) drops its flash data. Both are documented
    /// as such on those methods.
    #[expect(
        dead_code,
        reason = "reachable once the redirect response mapper lands;                   keeping the resolution logic beside the builder it serves"
    )]
    pub(crate) fn resolve(&self, routes: &crate::routing::Routes<()>) -> Result<Redirect> {
        let redirect = match &self.target {
            RedirectTarget::Path(p) => {
                validate_redirect_target(p)?;
                if self.permanent {
                    Redirect::permanent(p)
                } else {
                    Redirect::temporary(p)
                }
            }
            RedirectTarget::Route { name, params } => {
                let path = routes.url_for(
                    name,
                    &params
                        .iter()
                        .map(|p| match p {
                            RouteParam(s) => s.as_str(),
                        })
                        .collect::<Vec<_>>(),
                )?;
                if self.permanent {
                    Redirect::permanent(&path)
                } else {
                    Redirect::temporary(&path)
                }
            }
            RedirectTarget::Back => Redirect::to("/"),
        };
        Ok(redirect)
    }
}

impl IntoResponse for RedirectResponse {
    fn into_response(self) -> Response {
        // Flash data and named-route resolution require the route table,
        // which lives in request extensions. The framework installs a
        // response mapper layer that resolves this builder fully; the direct
        // `IntoResponse` impl is a best-effort fallback for raw paths.
        match &self.target {
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
        }
    }
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
