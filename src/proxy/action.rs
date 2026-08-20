//! The proxy action contract — what an application's proxy function returns.
//!
//! The proxy is the smallest useful *global* request boundary (engine spec
//! §4): it runs before route selection and can continue, redirect, rewrite,
//! mutate request headers, or short-circuit with an early response. It is not
//! a second middleware ecosystem — Tower remains the real one; this is
//! application-owned global policy expressed as a pure decision.
//!
//! The engine interprets each variant and performs the actual HTTP work
//! (setting status/headers, rewriting the URI, delegating to the router), so
//! the application never touches Axum middleware machinery (engine spec §5).

use crate::axum::http::{HeaderMap, StatusCode};
use crate::axum::response::Response;

/// A decision made by the application's proxy function.
///
/// Constructed by application code and consumed by the engine's proxy
/// service. All HTTP mutation is performed by the engine — the application
/// only declares intent.
#[derive(Debug)]
pub enum Action {
    /// Continue to route selection with the request as-is (optionally with
    /// mutated request headers).
    Continue {
        /// Headers to set on the request before routing. When empty, the
        /// request passes through unchanged. The engine merges these into
        /// the existing request headers.
        set_headers: HeaderMap,
    },
    /// Redirect the client to `location` without touching the router.
    Redirect {
        /// Absolute or relative target URI. The engine validates this against
        /// CRLF/header-injection before emitting it (proxy security review).
        location: String,
        /// `true` -> 301 Moved Permanently; `false` -> 302 Found.
        permanent: bool,
    },
    /// Rewrite the request URI to `uri` and continue to route selection.
    ///
    /// This is the pre-routing rewrite contract: the registered route table
    /// is matched against `uri`, not the original request path. A rewrite to
    /// a target that has no registered route yields the normal 404 fallback.
    Rewrite {
        /// The new request URI (path + query). The engine validates this
        /// before applying it; an invalid target is rejected with 400.
        uri: String,
    },
    /// Short-circuit with an immediate response, skipping route selection.
    ShortCircuit {
        /// The HTTP status to return.
        status: StatusCode,
        /// Optional response body via a ready [`Response`]; when `None` the
        /// engine emits an empty body for `status`.
        response: Option<Response>,
    },
}

impl Action {
    /// Continue to route selection with no request mutation.
    ///
    /// The common case: the proxy has nothing to say and the request proceeds
    /// normally. This is the default generated-app proxy behavior.
    #[must_use]
    pub fn continue_default() -> Self {
        Self::Continue {
            set_headers: HeaderMap::new(),
        }
    }
}

impl Default for Action {
    fn default() -> Self {
        Self::continue_default()
    }
}
