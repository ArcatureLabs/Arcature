//! Vite dev-server request detection.
//!
//! One responsibility: decide whether an incoming request should be forwarded
//! to the Vite IPC server or handed to the application router. The decision is
//! based on the request path and headers — never on runtime state (no env
//! read, no global). The [`crate::dev_proxy::service`] calls
//! [`is_vite_request`] to make the routing choice.
//!
//! # What is a "Vite request"?
//!
//! Vite's dev middleware serves three categories of requests that the
//! application router has no route for:
//!
//! 1. **Internal endpoints** — `/@vite/client`, `/@react-refresh`, `/@fs/...`,
//!    `/@id/...`. All begin with `/@`.
//! 2. **Source modules** — `/src/app.tsx`, `/src/...`. Vite transforms these
//!    on the fly.
//! 3. **Optimized dependencies** — `/node_modules/.vite/...`.
//! 4. **HMR WebSocket** — a `Connection: upgrade` request whose
//!    `Sec-WebSocket-Protocol` is `vite-hmr` (or `vite-ping`). The HMR client
//!    connects to the same origin as the page; the dev proxy tunnels the
//!    upgrade to Vite over IPC.
//!
//! Everything else (`/`, `/api/...`, application routes) goes to the
//! application router. This keeps the dev proxy transparent: the application
//! never sees Vite requests, and Vite never sees application routes.
//!
//! # Security
//!
//! The detection is a pure function of the request path and headers — both
//! attacker-controlled. A request that *looks* like a Vite request is
//! forwarded to the IPC server; Vite's middleware handles it. The IPC server
//! is Vite (trusted, dev-only, process-private); it is not an open redirect.
//! See the AP2.1-3 security review.

use crate::axum::body::Body;
use crate::axum::extract::Request;

/// Decide whether `req` should be forwarded to the Vite IPC server.
///
/// Pure and allocation-free. Called once per request by the dev proxy
/// service; the result determines whether the request is forwarded or
/// delegated to the inner application pipeline.
#[must_use]
pub(crate) fn is_vite_request(req: &Request<Body>) -> bool {
    let path = req.uri().path();

    // Vite internal endpoints and source modules.
    if path.starts_with("/@")
        || path.starts_with("/src/")
        || path.starts_with("/node_modules/.vite/")
    {
        return true;
    }

    // HMR WebSocket upgrade — the Vite client opens a WebSocket with the
    // `vite-hmr` (or `vite-ping`) subprotocol. The path is the page origin's
    // root (`/` by default), so we detect by protocol, not path — the
    // application's own WebSocket routes (different subprotocol or path)
    // are not intercepted.
    if is_vite_ws_upgrade(req) {
        return true;
    }

    false
}

/// Detect a Vite HMR WebSocket upgrade request.
///
/// Checks three headers: `Connection: upgrade`, `Upgrade: websocket`, and
/// `Sec-WebSocket-Protocol` containing `vite-hmr` or `vite-ping`. All three
/// must match; an application WebSocket with a different subprotocol is not
/// forwarded.
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
        assert!(is_vite_request(&request("/@vite/client")));
        assert!(is_vite_request(&request("/@react-refresh")));
        assert!(is_vite_request(&request("/@fs/src/app.tsx")));
        assert!(is_vite_request(&request("/@id/react")));
    }

    #[test]
    fn source_modules_are_forwarded() {
        assert!(is_vite_request(&request("/src/app.tsx")));
        assert!(is_vite_request(&request("/src/main.ts")));
        assert!(is_vite_request(&request("/src/styles/app.css")));
    }

    #[test]
    fn optimized_deps_are_forwarded() {
        assert!(is_vite_request(&request(
            "/node_modules/.vite/deps/react.js"
        )));
    }

    #[test]
    fn application_paths_are_not_forwarded() {
        assert!(!is_vite_request(&request("/")));
        assert!(!is_vite_request(&request("/api/users")));
        assert!(!is_vite_request(&request("/dashboard")));
        assert!(!is_vite_request(&request("/favicon.ico")));
    }

    #[test]
    fn vite_hmr_websocket_is_forwarded() {
        assert!(is_vite_request(&ws_request("vite-hmr")));
        assert!(is_vite_request(&ws_request("vite-ping")));
    }

    #[test]
    fn non_vite_websocket_is_not_forwarded() {
        let req = ws_request("custom-app-protocol");
        assert!(!is_vite_request(&req));
    }

    #[test]
    fn plain_get_to_root_is_not_forwarded() {
        assert!(!is_vite_request(&request("/")));
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
        assert!(!is_vite_request(&req));
    }
}
