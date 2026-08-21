//! Render 404, 419 and 500 as Inertia pages instead of RFC 9457 documents.
//!
//! # Why this is a layer and not a route
//!
//! A catch-all route would shadow the static-file fallback, so every asset
//! would 404. A response-inspecting layer leaves routing alone and only acts
//! on a status that has already been decided.
//!
//! # Why it renders instead of attaching a `PendingPage`
//!
//! Returning `Page<T>` from here would work, but the Inertia layer treats the
//! render's status as authoritative and discards the one the response
//! arrived with -- a 404 would reach the browser as a 200. Rendering through
//! the [`Inertia`] extractor directly leaves the status in this layer's
//! hands, so the page and the status code agree.
//!
//! # Where this sits
//!
//! Installed with `.layer(..)`, which the pipeline places *inside* the
//! Inertia layer. That is what makes the [`Inertia`] extractor resolvable
//! here: the layer above has already inserted the config and the parsed
//! request into extensions.

use arcature::axum::extract::Request;
use arcature::axum::http::header::ACCEPT;
use arcature::axum::middleware::Next;
use arcature::prelude::*;

/// The component rendered for a `404`.
const NOT_FOUND: &str = "errors/404";
/// The component rendered for a `419`.
const PAGE_EXPIRED: &str = "errors/419";
/// The component rendered for any `5xx`.
const SERVER_ERROR: &str = "errors/500";

/// The `419 Page Expired` status. Not in `StatusCode`'s constant set because
/// it is not an IANA-registered code; it is the convention the Inertia
/// ecosystem uses for a stale CSRF token.
const PAGE_EXPIRED_STATUS: u16 = 419;

/// Replace an error response with the matching Inertia page.
///
/// Requests that did not ask for HTML -- an API client, a fetch for a missing
/// asset -- keep the RFC 9457 body the error mapping produced. A render
/// failure also falls back to the original response: an error page that
/// itself errors must not replace a correct status with a confusing one.
pub async fn error_pages(inertia: Inertia, request: Request, next: Next) -> Response {
    let wants_page = wants_html(&request);
    let response = next.run(request).await;

    let status = response.status();
    let component = match status.as_u16() {
        404 => NOT_FOUND,
        PAGE_EXPIRED_STATUS => PAGE_EXPIRED,
        _ if status.is_server_error() => SERVER_ERROR,
        _ => return response,
    };
    if !wants_page {
        return response;
    }

    match inertia
        .render(component, arcature::serde_json::json!({}))
        .await
    {
        Ok(mut rendered) => {
            *rendered.status_mut() = status;
            rendered
        }
        Err(_) => response,
    }
}

/// True when the caller is a browser navigation or an Inertia visit.
///
/// An Inertia visit is checked separately because its `Accept` is
/// `application/json`: the client asks for the page object, not a document,
/// and still expects the error page.
fn wants_html(request: &Request) -> bool {
    if request.headers().contains_key("x-inertia") {
        return true;
    }
    request
        .headers()
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}
