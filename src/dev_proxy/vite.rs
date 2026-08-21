//! Vite dev-server request detection.
//!
//! One responsibility: decide whether an incoming request should be forwarded
//! to the Vite IPC server or handed to the application router. The decision is
//! a pure function of the request path, the request headers, and a
//! [`ViteRoutes`] table resolved once at startup -- never of runtime state (no
//! env read per request, no global). [`crate::dev_proxy::service`] calls
//! [`ViteRoutes::matches_request`] to make the routing choice.
//!
//! # What is a "Vite request"?
//!
//! Vite's dev middleware serves four categories of request that the
//! application router has no route for:
//!
//! 1. **Internal endpoints** -- `/@vite/client`, `/@react-refresh`,
//!    `/@fs/...`, `/@id/...`. All begin with `/@`.
//! 2. **Optimized dependencies** -- `/node_modules/.vite/...`.
//! 3. **Source modules** -- whatever lives under the application's asset
//!    root. This one is *not* fixed, which is the whole reason
//!    [`ViteRoutes`] exists (see below).
//! 4. **HMR WebSocket** -- a `Connection: upgrade` request whose
//!    `Sec-WebSocket-Protocol` is `vite-hmr` (or `vite-ping`). The HMR client
//!    connects to the same origin as the page; the dev proxy tunnels the
//!    upgrade to Vite over IPC.
//!
//! Everything else (`/`, `/api/...`, application routes) goes to the
//! application router.
//!
//! # Why the source-module prefix is configuration, not a constant
//!
//! An earlier version hard-coded `/src/`. Arcature's own templates put their
//! entry points under `resources/js/`, so every `/resources/js/app.tsx`
//! request missed the prefix and fell through to the application router,
//! which 404s -- the first page of a fresh `arc new` app was blank. A
//! constant cannot know where an application keeps its assets, so the roots
//! come from configuration, defaulting to the two conventions that cover
//! nearly everything ([`ViteRoutes::DEFAULT_ASSET_ROOTS`]).
//!
//! Configuration alone would still be a trap -- a project with an unusual
//! layout would hit exactly the same blank page, just later. So the proxy
//! also has a second chance: [`crate::dev_proxy::service`] retries a
//! bodyless request through Vite when the application answers `404`. That is
//! the AdonisJS arrangement, where Vite's middleware sits behind the router
//! rather than in front of it. With the fallthrough in place a wrong prefix
//! costs one extra round trip in development instead of breaking the app.
//!
//! # Security
//!
//! The detection is a pure function of the request path and headers -- both
//! attacker-controlled. A request that *looks* like a Vite request is
//! forwarded to the IPC server; Vite's middleware handles it. The IPC server
//! is Vite (trusted, dev-only, process-private); it is not an open redirect.
//! See the AP2.1-3 security review.

use std::sync::Arc;

use crate::axum::body::Body;
use crate::axum::extract::Request;

/// The paths this dev proxy considers Vite's.
///
/// Resolved once at pipeline-assembly time from
/// [`prefixes_from_env`](crate::dev_proxy::config::prefixes_from_env) and
/// stored in the [`DevProxyLayer`](crate::dev_proxy::DevProxyLayer). Cheap to
/// clone: the prefix list is behind an `Arc`, and the common case is two or
/// three short strings.
#[derive(Clone, Debug)]
pub(crate) struct ViteRoutes {
    /// Application asset roots, in addition to [`Self::BUILT_IN`]. Each is
    /// normalised to begin and end with `/` so a prefix test cannot match a
    /// sibling path (`/src/` must not match `/srcmap.json`).
    asset_roots: Arc<[Box<str>]>,
}

impl ViteRoutes {
    /// Prefixes Vite always owns, whatever the application's layout is.
    ///
    /// `/@` covers every Vite internal endpoint; `/node_modules/.vite/` is
    /// the optimized-dependency cache. Neither is configurable, because
    /// neither is the application's to move.
    pub(crate) const BUILT_IN: &'static [&'static str] = &["/@", "/node_modules/.vite/"];

    /// The asset roots assumed when nothing is configured.
    ///
    /// `resources/` is the Arcature (and Laravel) convention the templates
    /// use; `src/` is the plain-Vite convention. Covering both by default
    /// means the overwhelming majority of projects configure nothing, and
    /// the rest are caught by the 404 fallthrough.
    pub(crate) const DEFAULT_ASSET_ROOTS: &'static [&'static str] = &["/resources/", "/src/"];

    /// Build a routing table from a list of application asset roots.
    ///
    /// Each root is normalised to a leading and trailing `/`; empty and
    /// `/`-only entries are dropped, because a root of `/` would forward
    /// every request to Vite and take the application off the air.
    pub(crate) fn new<I, S>(asset_roots: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let asset_roots: Vec<Box<str>> = asset_roots
            .into_iter()
            .filter_map(|root| normalise_root(root.as_ref()))
            .collect();
        Self {
            asset_roots: asset_roots.into(),
        }
    }

    /// The table used when the application configured nothing.
    pub(crate) fn defaults() -> Self {
        Self::new(Self::DEFAULT_ASSET_ROOTS)
    }

    /// The configured asset roots, normalised. `arc dev` prints them, so a
    /// mis-set prefix is visible without a debugger.
    pub(crate) fn asset_roots(&self) -> &[Box<str>] {
        &self.asset_roots
    }

    /// Does `path` belong to Vite?
    pub(crate) fn matches_path(&self, path: &str) -> bool {
        Self::BUILT_IN.iter().any(|p| path.starts_with(p))
            || self
                .asset_roots
                .iter()
                .any(|root| path.starts_with(root.as_ref()))
    }

    /// Decide whether `req` should be forwarded to the Vite IPC server.
    ///
    /// Pure and allocation-free. Called once per request by the dev proxy
    /// service; the result decides whether the request is forwarded or
    /// delegated to the inner application pipeline.
    pub(crate) fn matches_request(&self, req: &Request<Body>) -> bool {
        self.matches_path(req.uri().path()) || is_vite_ws_upgrade(req)
    }
}

/// Normalise one asset root to a `/`-delimited prefix.
///
/// Returns `None` for anything that would match everything (`""`, `"/"`),
/// because forwarding every request to Vite is never what a configuration
/// value meant.
fn normalise_root(raw: &str) -> Option<Box<str>> {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(format!("/{trimmed}/").into_boxed_str())
}

/// Detect a Vite HMR WebSocket upgrade request.
///
/// Checks three headers: `Connection: upgrade`, `Upgrade: websocket`, and
/// `Sec-WebSocket-Protocol` containing `vite-hmr` or `vite-ping`. All three
/// must match; an application WebSocket with a different subprotocol is not
/// forwarded.
///
/// The path is deliberately not consulted: the HMR client connects to the
/// page origin's root, so the subprotocol is the only thing separating it
/// from an application WebSocket on the same path.
fn is_vite_ws_upgrade(req: &Request<Body>) -> bool {
    let headers = req.headers();

    let connection_upgrade = headers
        .get("connection")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().contains("upgrade"));

    let upgrade_websocket = headers
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));

    let vite_protocol = headers
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("vite-hmr") || v.contains("vite-ping"));

    connection_upgrade && upgrade_websocket && vite_protocol
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Uri};
    use std::str::FromStr as _;

    fn request(path: &str) -> Request<Body> {
        Request::builder()
            .method(Method::GET)
            .uri(Uri::from_str(path).expect("test path should parse as URI"))
            .body(Body::empty())
            .expect("test request should build")
    }

    fn ws_request(protocol: &str) -> Request<Body> {
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("upgrade"));
        headers.insert("upgrade", HeaderValue::from_static("websocket"));
        headers.insert(
            HeaderName::from_static("sec-websocket-protocol"),
            HeaderValue::from_str(protocol).expect("valid header value"),
        );
        Request::builder()
            .method(Method::GET)
            .uri(Uri::from_static("/"))
            .body(Body::empty())
            .map(|mut req| {
                *req.headers_mut() = headers;
                req
            })
            .expect("test ws request should build")
    }

    #[test]
    fn vite_internal_paths_are_forwarded() {
        let routes = ViteRoutes::defaults();
        assert!(routes.matches_request(&request("/@vite/client")));
        assert!(routes.matches_request(&request("/@react-refresh")));
        assert!(routes.matches_request(&request("/@fs/src/app.tsx")));
        assert!(routes.matches_request(&request("/@id/react")));
    }

    #[test]
    fn optimized_deps_are_forwarded() {
        let routes = ViteRoutes::defaults();
        assert!(routes.matches_request(&request("/node_modules/.vite/deps/react.js")));
    }

    // The regression this whole type exists for: the templates put the entry
    // point at `resources/js/app.tsx`, and the old hard-coded `/src/` prefix
    // sent that straight to the application router.
    #[test]
    fn the_template_asset_root_is_forwarded_by_default() {
        let routes = ViteRoutes::defaults();
        assert!(routes.matches_request(&request("/resources/js/app.tsx")));
        assert!(routes.matches_request(&request("/resources/js/pages/home.tsx")));
        assert!(routes.matches_request(&request("/resources/css/app.css")));
    }

    #[test]
    fn the_plain_vite_asset_root_is_still_forwarded_by_default() {
        let routes = ViteRoutes::defaults();
        assert!(routes.matches_request(&request("/src/app.tsx")));
        assert!(routes.matches_request(&request("/src/main.ts")));
    }

    #[test]
    fn a_configured_root_replaces_the_defaults() {
        let routes = ViteRoutes::new(["assets"]);
        assert!(routes.matches_request(&request("/assets/app.tsx")));
        // Configuring a root is a statement about this application, so the
        // conventional roots stop applying -- otherwise a project could not
        // serve its own `/resources/...` route.
        assert!(!routes.matches_request(&request("/resources/js/app.tsx")));
        // The built-ins are not the application's to move.
        assert!(routes.matches_request(&request("/@vite/client")));
    }

    #[test]
    fn a_root_matches_only_whole_path_segments() {
        let routes = ViteRoutes::new(["src"]);
        assert!(routes.matches_request(&request("/src/app.tsx")));
        assert!(!routes.matches_request(&request("/srcmap.json")));
    }

    #[test]
    fn roots_are_normalised_however_they_are_written() {
        for spelling in ["resources", "/resources", "resources/", "/resources/"] {
            let routes = ViteRoutes::new([spelling]);
            assert!(
                routes.matches_request(&request("/resources/js/app.tsx")),
                "spelling {spelling:?} should normalise"
            );
        }
    }

    // A root of `/` would forward every request to Vite and take the
    // application off the air. Dropping it is safer than honouring it.
    #[test]
    fn a_root_that_would_swallow_everything_is_dropped() {
        let routes = ViteRoutes::new(["/", "", "   "]);
        assert!(routes.asset_roots().is_empty());
        assert!(!routes.matches_request(&request("/")));
        assert!(!routes.matches_request(&request("/api/users")));
        assert!(routes.matches_request(&request("/@vite/client")));
    }

    #[test]
    fn application_paths_are_not_forwarded() {
        let routes = ViteRoutes::defaults();
        assert!(!routes.matches_request(&request("/")));
        assert!(!routes.matches_request(&request("/api/users")));
        assert!(!routes.matches_request(&request("/dashboard")));
        assert!(!routes.matches_request(&request("/favicon.ico")));
    }

    #[test]
    fn vite_hmr_websocket_is_forwarded() {
        let routes = ViteRoutes::defaults();
        assert!(routes.matches_request(&ws_request("vite-hmr")));
        assert!(routes.matches_request(&ws_request("vite-ping")));
    }

    #[test]
    fn non_vite_websocket_is_not_forwarded() {
        let routes = ViteRoutes::defaults();
        assert!(!routes.matches_request(&ws_request("custom-app-protocol")));
    }

    #[test]
    fn websocket_without_vite_protocol_is_not_forwarded() {
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("Upgrade"));
        headers.insert("upgrade", HeaderValue::from_static("websocket"));
        let req = Request::builder()
            .method(Method::GET)
            .uri(Uri::from_static("/"))
            .body(Body::empty())
            .map(|mut req| {
                *req.headers_mut() = headers;
                req
            })
            .expect("request should build");
        assert!(!ViteRoutes::defaults().matches_request(&req));
    }
}
