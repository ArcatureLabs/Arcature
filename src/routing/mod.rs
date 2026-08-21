//! HTTP routing.
//!
//! Routes are ordinary Rust values, built with the [`Routes`] collection and
//! the [`Route`] constructors. Nothing here is reachable only through a macro:
//! the `routes!` macro is a shorthand that expands to exactly these calls, so
//! a route table stays debuggable whichever spelling it was written in.
//!
//! ```ignore
//! pub fn routes() -> Routes {
//!     Routes::new([
//!         Route::get("/", HomeController::index).name("home"),
//!         Route::post("/users", UserController::store).name("users.store"),
//!     ])
//! }
//! ```
//!
//! Named routes support URL generation (for redirects and links) via
//! [`Routes::url_for`]. Route groups attach shared middleware and a path
//! prefix. Middleware composes on Tower/Axum via the [`Middleware`] trait, not
//! a separate engine.

pub mod rate_limit;
pub mod redirect_mapper;
pub mod table;
pub use rate_limit::{Decision, KeyFn, KeySource, OnBackendError, RateLimit, RateLimitService};
pub use redirect_mapper::{RedirectMapper, RedirectMapperService};
pub use table::RouteTable;

use crate::error::{Error, Result};
use axum::Router;
use axum::handler::Handler as AxumHandler;
use axum::middleware::from_fn;
use axum::response::IntoResponse;
use axum::routing::MethodRouter;

// The request and response types a handler or middleware is written against.
// Re-exported (not merely imported) so a `#[middleware]` expansion can name
// them as `::arcature::routing::Request` without the user's crate depending
// on axum directly. They stay on the `routing` path rather than the crate
// root because `arcature::Request` is already the validated-request contract.
pub use axum::extract::Request;
pub use axum::response::Response;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// The application state bound to a [`Router`]. The canonical Arcature app uses
/// [`crate::application::AppState`]; expert users may bring their own.
pub trait RouterState: Clone + Send + Sync + 'static {}
impl<T: Clone + Send + Sync + 'static> RouterState for T {}

/// A single route: the path template, an optional name for URL generation,
/// and the `axum::routing::MethodRouter` that dispatches it.
///
/// The method router — not the whole `axum::Router` — is what a route owns,
/// and that is what makes per-route middleware *per route*. An earlier design
/// held a closure folding `Router<S>`; because `Routes::new` folds routes into
/// one accumulating router, layering inside that closure wrapped every route
/// registered so far. A route's middleware silently applied to its siblings.
/// Layering a `MethodRouter` cannot reach past the one path it serves.
pub struct Route<S: RouterState = ()> {
    path: String,
    name: Option<String>,
    method_router: MethodRouter<S>,
}

impl<S: RouterState> Route<S> {
    /// Register a method handler at `path`.
    fn method<H, T>(method: axum::http::Method, path: impl Into<String>, handler: H) -> Self
    where
        H: AxumHandler<T, S> + Send + 'static,
        T: 'static,
    {
        Route {
            path: path.into(),
            name: None,
            method_router: method_router_for(method, handler),
        }
    }

    /// `GET` route.
    pub fn get<H, T>(path: impl Into<String>, handler: H) -> Self
    where
        H: AxumHandler<T, S> + Send + 'static,
        T: 'static,
    {
        Self::method(axum::http::Method::GET, path, handler)
    }
    /// `POST` route.
    pub fn post<H, T>(path: impl Into<String>, handler: H) -> Self
    where
        H: AxumHandler<T, S> + Send + 'static,
        T: 'static,
    {
        Self::method(axum::http::Method::POST, path, handler)
    }
    /// `PUT` route.
    pub fn put<H, T>(path: impl Into<String>, handler: H) -> Self
    where
        H: AxumHandler<T, S> + Send + 'static,
        T: 'static,
    {
        Self::method(axum::http::Method::PUT, path, handler)
    }
    /// `PATCH` route.
    pub fn patch<H, T>(path: impl Into<String>, handler: H) -> Self
    where
        H: AxumHandler<T, S> + Send + 'static,
        T: 'static,
    {
        Self::method(axum::http::Method::PATCH, path, handler)
    }
    /// `DELETE` route.
    pub fn delete<H, T>(path: impl Into<String>, handler: H) -> Self
    where
        H: AxumHandler<T, S> + Send + 'static,
        T: 'static,
    {
        Self::method(axum::http::Method::DELETE, path, handler)
    }
    /// `HEAD` route.
    pub fn head<H, T>(path: impl Into<String>, handler: H) -> Self
    where
        H: AxumHandler<T, S> + Send + 'static,
        T: 'static,
    {
        Self::method(axum::http::Method::HEAD, path, handler)
    }
    /// `OPTIONS` route.
    pub fn options<H, T>(path: impl Into<String>, handler: H) -> Self
    where
        H: AxumHandler<T, S> + Send + 'static,
        T: 'static,
    {
        Self::method(axum::http::Method::OPTIONS, path, handler)
    }

    /// Name the route for URL generation and redirects.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Attach middleware to this route, and only this route.
    #[must_use]
    pub fn middleware<M>(mut self, middleware: M) -> Self
    where
        M: Middleware,
    {
        self.method_router = middleware_layer(middleware, self.method_router);
        self
    }

    /// Attach a raw [`tower::Layer`] to this route, and only this route.
    ///
    /// The escape hatch for middleware that is not a [`Middleware`] — a
    /// `tower_http` layer, say. Prefer [`middleware`](Self::middleware) for
    /// application middleware.
    #[must_use]
    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: tower::Layer<axum::routing::Route> + Clone + Send + Sync + 'static,
        L::Service: tower::Service<Request, Error = std::convert::Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        <L::Service as tower::Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as tower::Service<Request>>::Future: Send + 'static,
    {
        self.method_router = self.method_router.layer(layer);
        self
    }
}

fn method_router_for<S, H, T>(
    method: axum::http::Method,
    handler: H,
) -> axum::routing::MethodRouter<S>
where
    S: RouterState,
    H: AxumHandler<T, S> + Send + 'static,
    T: 'static,
{
    use axum::routing::*;
    match method {
        axum::http::Method::GET => get(handler),
        axum::http::Method::POST => post(handler),
        axum::http::Method::PUT => put(handler),
        axum::http::Method::PATCH => patch(handler),
        axum::http::Method::DELETE => delete(handler),
        axum::http::Method::HEAD => head(handler),
        axum::http::Method::OPTIONS => options(handler),
        _ => any(handler),
    }
}

/// A group of routes sharing a path prefix and/or middleware.
pub struct RouteGroup<S: RouterState = ()> {
    prefix: String,
    apply_mw: Vec<RouteLayer<S>>,
    routes: Vec<Route<S>>,
}

/// A cloneable layer-builder over a single route's `MethodRouter<S>`.
///
/// Cloneable (Arc-backed) so one group's middleware applies to every route in
/// the group *individually* -- a group layer must not reach routes outside the
/// group, which is exactly what folding the whole `Router` used to do.
#[derive(Clone)]
pub struct RouteLayer<S: RouterState> {
    #[allow(clippy::type_complexity)]
    inner: Arc<dyn Fn(MethodRouter<S>) -> MethodRouter<S> + Send + Sync + 'static>,
}

impl<S: RouterState> RouteLayer<S> {
    /// Wrap a fold over a route's method router.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(MethodRouter<S>) -> MethodRouter<S> + Send + Sync + 'static,
    {
        RouteLayer { inner: Arc::new(f) }
    }

    /// Wrap a [`tower::Layer`] so it can be stored alongside other route
    /// layers regardless of its concrete type.
    pub fn from_layer<L>(layer: L) -> Self
    where
        L: tower::Layer<axum::routing::Route> + Clone + Send + Sync + 'static,
        L::Service: tower::Service<Request, Error = std::convert::Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        <L::Service as tower::Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as tower::Service<Request>>::Future: Send + 'static,
    {
        RouteLayer::new(move |method_router: MethodRouter<S>| method_router.layer(layer.clone()))
    }

    /// Apply the fold.
    pub fn apply(&self, method_router: MethodRouter<S>) -> MethodRouter<S> {
        (self.inner)(method_router)
    }
}

/// A cloneable layer-builder over a whole `axum::Router<S>`.
///
/// This is the collection-level counterpart of [`RouteLayer`]: it wraps every
/// route in the router it is applied to. The application pipeline
/// ([`crate::application::pipeline`]) stores its layers as these, which is how
/// `InertiaLayer`, `SessionLayer`, `CsrfLayer` and a user's own
/// [`tower::Layer`] sit in one ordered list despite having unrelated types.
#[derive(Clone)]
pub struct RouterLayer<S: RouterState> {
    #[allow(clippy::type_complexity)]
    inner: Arc<dyn Fn(Router<S>) -> Router<S> + Send + Sync + 'static>,
}

impl<S: RouterState> RouterLayer<S> {
    /// Wrap a fold over the router.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(Router<S>) -> Router<S> + Send + Sync + 'static,
    {
        RouterLayer { inner: Arc::new(f) }
    }

    /// Wrap a [`tower::Layer`] applicable to an `axum::Router`.
    pub fn from_layer<L>(layer: L) -> Self
    where
        L: tower::Layer<axum::routing::Route> + Clone + Send + Sync + 'static,
        L::Service: tower::Service<Request, Error = std::convert::Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        <L::Service as tower::Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as tower::Service<Request>>::Future: Send + 'static,
    {
        RouterLayer::new(move |router: Router<S>| router.layer(layer.clone()))
    }

    /// Apply the fold.
    pub fn apply(&self, router: Router<S>) -> Router<S> {
        (self.inner)(router)
    }
}

/// Retained name for [`RouterLayer`].
pub type MiddlewareLayer<S> = RouterLayer<S>;

impl<S: RouterState> RouteGroup<S> {
    /// Begin a group under `prefix`.
    pub fn new<I: IntoRoutes<S>>(prefix: impl Into<String>, routes: I) -> Self {
        RouteGroup {
            prefix: prefix.into(),
            apply_mw: Vec::new(),
            routes: routes.into_routes(),
        }
    }

    /// Attach middleware to every route in the group.
    #[must_use]
    pub fn middleware<M>(mut self, middleware: M) -> Self
    where
        M: Middleware,
    {
        let m = middleware.clone();
        self.apply_mw
            .push(RouteLayer::new(move |method_router: MethodRouter<S>| {
                middleware_layer(m.clone(), method_router)
            }));
        self
    }

    /// Attach a raw [`tower::Layer`] to every route in the group.
    #[must_use]
    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: tower::Layer<axum::routing::Route> + Clone + Send + Sync + 'static,
        L::Service: tower::Service<Request, Error = std::convert::Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        <L::Service as tower::Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as tower::Service<Request>>::Future: Send + 'static,
    {
        self.apply_mw.push(RouteLayer::from_layer(layer));
        self
    }
}

impl<S: RouterState> IntoRoutes<S> for RouteGroup<S> {
    fn into_routes(self) -> Vec<Route<S>> {
        // Prepend the prefix to each route path. Group middleware is cloneable
        // (Arc-backed), so the same layer-builders apply to every route in
        // the group. Group middleware runs outermost (folded before the
        // route's own middleware closure).
        let prefix = self.prefix;
        let group_mw = self.apply_mw;
        self.routes
            .into_iter()
            .map(move |mut r| {
                r.path = join_path(&prefix, &r.path);
                r.method_router = group_mw
                    .iter()
                    .fold(r.method_router, |method_router, mw| mw.apply(method_router));
                r
            })
            .collect()
    }
}

/// Anything that can be turned into a `Vec<Route<S>>`: a single [`Route`], a
/// [`RouteGroup`], or a `Vec`/array of routes.
pub trait IntoRoutes<S: RouterState> {
    fn into_routes(self) -> Vec<Route<S>>;
}

impl<S: RouterState> IntoRoutes<S> for Route<S> {
    fn into_routes(self) -> Vec<Route<S>> {
        vec![self]
    }
}

impl<S: RouterState, const N: usize> IntoRoutes<S> for [Route<S>; N] {
    fn into_routes(self) -> Vec<Route<S>> {
        self.into_iter().collect()
    }
}

impl<S: RouterState, const N: usize> IntoRoutes<S> for [RouteGroup<S>; N] {
    fn into_routes(self) -> Vec<Route<S>> {
        self.into_iter().flat_map(RouteGroup::into_routes).collect()
    }
}

impl<S: RouterState> IntoRoutes<S> for Vec<Route<S>> {
    fn into_routes(self) -> Vec<Route<S>> {
        self
    }
}

/// A collection of routes, the value returned by an application's
/// `routes::routes()` function.
pub struct Routes<S: RouterState = ()> {
    router: Router<S>,
    names: HashMap<String, String>,
}

impl<S: RouterState> Routes<S> {
    /// An empty route collection.
    #[must_use]
    pub fn new<I: IntoRoutes<S>>(routes: I) -> Self {
        let mut out = Routes {
            router: Router::new(),
            names: HashMap::new(),
        };
        for r in routes.into_routes() {
            if let Some(name) = r.name {
                out.names.insert(name, r.path.clone());
            }
            out.router = out.router.route(&r.path, r.method_router);
        }
        out
    }

    /// An empty collection (no routes). Use `Routes::new([...])` for the
    /// common case.
    #[must_use]
    pub fn empty() -> Self {
        Routes {
            router: Router::new(),
            names: HashMap::new(),
        }
    }

    /// Merge another [`Routes`] collection into this one.
    #[must_use]
    pub fn merge(mut self, other: Routes<S>) -> Self {
        for (name, path) in other.names {
            self.names.insert(name, path);
        }
        self.router = self.router.merge(other.router);
        self
    }

    /// Attach middleware to every route in this collection.
    ///
    /// Collection-level, unlike [`Route::middleware`] and
    /// [`RouteGroup::middleware`]: it wraps the routes present *at the time of
    /// the call*. Routes merged in afterwards are not covered, which is what
    /// lets a public and a guarded collection be merged without the guard
    /// spreading.
    #[must_use]
    pub fn middleware<M>(mut self, middleware: M) -> Self
    where
        M: Middleware,
    {
        let m = middleware.clone();
        self.router = self.router.layer(from_fn(move |request, next| {
            let m = m.clone();
            async move {
                match m.handle(request, Next(next)).await {
                    Ok(response) => response,
                    Err(error) => error.into_response(),
                }
            }
        }));
        self
    }

    /// Attach a raw [`tower::Layer`] to every route in this collection.
    ///
    /// Same scope rule as [`middleware`](Self::middleware): routes present at
    /// the time of the call.
    #[must_use]
    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: tower::Layer<axum::routing::Route> + Clone + Send + Sync + 'static,
        L::Service: tower::Service<Request, Error = std::convert::Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        <L::Service as tower::Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as tower::Service<Request>>::Future: Send + 'static,
    {
        self.router = self.router.layer(layer);
        self
    }

    /// Add a fallback handler (404 / not matched).
    #[must_use]
    pub fn fallback<H, T>(mut self, handler: H) -> Self
    where
        H: AxumHandler<T, S> + Send + 'static,
        T: 'static,
    {
        self.router = self.router.fallback(handler);
        self
    }

    /// Generate a URL for a named route, filling parameters in declaration
    /// order.
    pub fn url_for(&self, name: &str, params: &[&str]) -> Result<String> {
        let template = self
            .names
            .get(name)
            .ok_or_else(|| Error::NotFound(format!("route `{name}` is not defined")))?;
        render_path(template, params)
    }

    /// Consume the collection and return the underlying Axum router. This is
    /// the escape hatch to raw Axum.
    pub fn into_router(self) -> Router<S> {
        self.router
    }

    /// Borrow the underlying Axum router.
    pub fn router(&self) -> &Router<S> {
        &self.router
    }

    /// Iterate over named routes.
    ///
    /// The order is a `HashMap`'s, which is to say arbitrary and different
    /// on every run. Anything that *renders* the table wants
    /// [`table`](Self::table) instead.
    pub fn named(&self) -> impl Iterator<Item = (&String, &String)> {
        self.names.iter()
    }

    /// Take a [`RouteTable`] snapshot: the named routes on their own, without
    /// the router and without the state type.
    ///
    /// This is how URL generation escapes `Routes`. The redirect response
    /// mapper and `arc routes` both need name-to-path resolution and neither
    /// can hold a `Routes<S>` -- the mapper because a layer must not be
    /// generic over the application state, the CLI because it has no state
    /// value at all.
    #[must_use]
    pub fn table(&self) -> RouteTable {
        self.names
            .iter()
            .map(|(name, template)| (name.as_str(), template.as_str()))
            .collect()
    }
}

impl<S: RouterState> Default for Routes<S> {
    fn default() -> Self {
        Self::empty()
    }
}

// --- Middleware trait -------------------------------------------------------

/// The Arcature middleware contract. Compose on Tower/Axum, not a separate
/// engine: a [`Middleware`] is adapted to an Axum `from_fn` layer.
///
/// ```ignore
/// pub struct AuthMiddleware;
///
/// impl Middleware for AuthMiddleware {
///     async fn handle(&self, request: Request, next: Next) -> Result<Response> {
///         // ...
///         next.run(request).await
///     }
/// }
/// ```
pub trait Middleware: Clone + Send + Sync + 'static {
    /// Inspect the request, optionally short-circuit, or call `next.run(...)`
    /// to continue. Returning `Err` maps the framework error to an HTTP
    /// response.
    fn handle(
        &self,
        request: Request,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send>>;
}

/// The continuation passed to a [`Middleware::handle`] call. Wrap the Axum
/// `Next`; `run` forwards to it.
pub struct Next(pub axum::middleware::Next);

impl Next {
    /// Continue the middleware chain.
    pub async fn run(self, request: Request) -> Response {
        self.0.run(request).await
    }
}

fn middleware_layer<S, M>(middleware: M, method_router: MethodRouter<S>) -> MethodRouter<S>
where
    S: RouterState,
    M: Middleware,
{
    method_router.layer(from_fn(move |request, next| {
        let middleware = middleware.clone();
        async move {
            match middleware.handle(request, Next(next)).await {
                Ok(response) => response,
                Err(error) => error.into_response(),
            }
        }
    }))
}

// --- path helpers -----------------------------------------------------------

fn render_path(template: &str, params: &[&str]) -> Result<String> {
    // Templates use `{name}` capture groups (Axum 0.8 syntax). Render by
    // splitting on `/` and substituting `{name}` segments with params in
    // declaration order.
    let mut out = String::with_capacity(template.len());
    let mut param_idx = 0;
    for (i, segment) in template.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        if let Some(rest) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            let param = params
                .get(param_idx)
                .ok_or_else(|| Error::BadRequest(format!("missing parameter `{rest}`")))?;
            param_idx += 1;
            out.push_str(param);
        } else {
            out.push_str(segment);
        }
    }
    Ok(out)
}

fn join_path(prefix: &str, suffix: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    if suffix.starts_with('/') {
        format!("{prefix}{suffix}")
    } else if suffix.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}/{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_for_fills_params() {
        let routes: Routes = Routes::new([
            Route::get("/users/{id}", || async { "ok" }).name("users.show"),
            Route::get("/users/{id}/posts/{post}", || async { "ok" }).name("users.posts.show"),
        ]);
        assert_eq!(routes.url_for("users.show", &["42"]).unwrap(), "/users/42");
        assert_eq!(
            routes.url_for("users.posts.show", &["42", "7"]).unwrap(),
            "/users/42/posts/7"
        );
    }

    #[test]
    fn group_prefixes_paths() {
        let group = RouteGroup::new(
            "/admin",
            [Route::get("/users", || async { "ok" }).name("admin.users")],
        );
        let routes: Routes = Routes::new([group]);
        assert_eq!(routes.url_for("admin.users", &[]).unwrap(), "/admin/users");
    }

    #[test]
    fn missing_param_errors() {
        let routes: Routes =
            Routes::new([Route::get("/users/{id}", || async { "ok" }).name("users.show")]);
        assert!(routes.url_for("users.show", &[]).is_err());
    }

    #[test]
    fn join_path_cases() {
        assert_eq!(join_path("/admin", "/users"), "/admin/users");
        assert_eq!(join_path("/admin/", "users"), "/admin/users");
        assert_eq!(join_path("/admin", ""), "/admin");
    }
}
