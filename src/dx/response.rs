//! High-level response types for the Arcature application DX layer.
//!
//! These types let a controller return `Result<Json<T>>`, `Result<Empty>`,
//! or `Result<Page<T>>` without manually implementing Axum response plumbing.
//! Each type implements [`axum::response::IntoResponse`] using the
//! established `(status, headers, body).into_response()` pattern.
//!
//! ## Serialize does NOT imply browser-safe
//!
//! `Json<T>` and `Page<T>` require `T: Serialize` for the wire format, but
//! serialization is not a security boundary. A `SeaORM` model that
//! `Serialize`s must not automatically become `ClientData`. Page/Resource
//! declarations are explicit exposure boundaries. These response types are
//! the server-side rendering seam; browser exposure is governed by the
//! Inertia `PageContract` / `ClientData` system, not by `Serialize` alone.

use axum::http::HeaderValue;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};

/// A JSON response body. Serializes `T` to JSON and sets
/// `Content-Type: application/json`.
///
/// This is Arcature's `Json<T>` -- a replacement for `axum::Json` so normal
/// application code never names `axum::` directly. It implements
/// [`IntoResponse`] directly, so a controller can return
/// `Result<Json<UserResource>>` with no manual response plumbing.
///
/// `Serialize` does NOT imply browser-safe. The exposure boundary for
/// Inertia `ClientData` is the `PageContract` system, not `Serialize`.
pub struct Json<T>(pub T);

impl<T> IntoResponse for Json<T>
where
    T: serde::Serialize,
{
    fn into_response(self) -> Response {
        let body = serde_json::to_vec(&self.0).unwrap_or_else(|_| {
            // Fallback: a minimal error JSON. Never panics.
            br#"{"type":"urn:arcature:problem:internal","title":"Internal Server Error","status":500}"#
                .to_vec()
        });
        let len = body.len();
        let mut response = body.into_response();
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        response
            .headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from(len));
        response
    }
}

/// An empty response (204 No Content).
///
/// Use when a handler has nothing to return -- e.g. a `DELETE` that
/// succeeded.
pub struct Empty;

impl IntoResponse for Empty {
    fn into_response(self) -> Response {
        (
            axum::http::StatusCode::NO_CONTENT,
            axum::body::Body::empty(),
        )
            .into_response()
    }
}

/// A page response.
///
/// `Page<T>` is the high-level return type for an Inertia-rendered page. A
/// handler returns `Page<HomePage>` and the component identity, the props
/// schema, and the browser-safety certificate all come from `T` -- there is
/// nothing to repeat at the route.
///
/// `Page<T>` is also the **golden-path return type** for the `#[controller]`
/// macro's page-response derivation: a handler returning `Result<Page<T>, E>`
/// or `Page<T>` has its page identity (`T::PAGE_CONTRACT.name()`) inferred
/// into the controller metadata, so the route needs no `page:` / `pages:`
/// declaration. The `T` must be a `#[page]` type -- `T::PAGE_CONTRACT`
/// exists only then, so a non-page type fails to compile (the Client
/// Exposure Firewall applied to the return type).
///
/// `Serialize` does NOT imply browser-safe. With the `inertia` feature on,
/// the [`IntoResponse`] impl below requires
/// [`PageType`](crate::inertia::PageType), which requires
/// [`ClientData`](crate::inertia::ClientData) -- so the firewall applies to
/// the return type itself, not merely to the render call.
///
/// # How it renders
///
/// `into_response` cannot render an Inertia page by itself: the choice
/// between an HTML document and a JSON page object is made from the request
/// headers, and `IntoResponse` never sees the request. So `Page<T>` records
/// a [`PendingPage`](crate::inertia::PendingPage) in the response extensions
/// and the Inertia middleware -- which does hold the request -- performs the
/// render on the way out. Without an installed `InertiaLayer` the
/// placeholder body is a `500` problem document naming the missing wiring,
/// never a blank `200`.
pub struct Page<T>(pub T);

/// Without the `inertia` feature there is no page protocol to speak, so a
/// page degrades to its props as JSON. This is the API-only build; nothing
/// in it can reach a browser through Inertia.
#[cfg(not(feature = "inertia"))]
impl<T> IntoResponse for Page<T>
where
    T: serde::Serialize,
{
    fn into_response(self) -> Response {
        Json(self.0).into_response()
    }
}

#[cfg(feature = "inertia")]
impl<T> IntoResponse for Page<T>
where
    T: crate::inertia::PageType,
{
    fn into_response(self) -> Response {
        let component = <T as crate::inertia::PageType>::CONTRACT.name();
        match serde_json::to_value(&self.0) {
            Ok(props) => crate::inertia::PendingPage::new(component, props).into_response(),
            // Props that will not serialize are a bug in the page type, not
            // a client error -- and the detail names the page so the bug is
            // findable without a debugger.
            Err(error) => crate::api::Problem::of(crate::api::ProblemKind::Internal)
                .with_detail(format!(
                    "The page `{component}` could not be serialized: {error}"
                ))
                .into_response(),
        }
    }
}

/// Construct a [`Page<T>`] from its props -- the ergonomic golden-path
/// constructor for handlers returning `Result<Page<T>, E>` / `Page<T>`.
///
/// The `#[controller]` macro derives the route->page edge from the return
/// type's `Page<T>` (reading the signature, not the body), so a handler
/// returning `Page<HomePage>` needs no `page:` route declaration -- the page
/// identity is inferred from `HomePage::PAGE_CONTRACT.name()` at compile
/// time.
#[must_use]
pub fn page<T>(props: T) -> Page<T> {
    Page(props)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn empty_returns_204() {
        let response = Empty.into_response();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[test]
    fn json_serializes_body() {
        let response = Json(serde_json::json!({"hello": "world"})).into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .map(|v| v.to_str().unwrap_or("")),
            Some("application/json")
        );
    }

    #[cfg(not(feature = "inertia"))]
    #[test]
    fn page_serializes_as_json_without_the_inertia_feature() {
        #[derive(serde::Serialize)]
        struct TestData {
            value: u32,
        }

        let response = Page(TestData { value: 42 }).into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .map(|v| v.to_str().unwrap_or("")),
            Some("application/json")
        );
    }

    #[cfg(feature = "inertia")]
    #[test]
    fn a_page_defers_its_render_to_the_middleware() {
        use crate::inertia::{ClientData, ContractType, PageContract, PageType, PropsSchema};

        #[derive(serde::Serialize)]
        struct TestPage {
            value: u32,
        }

        impl ClientData for TestPage {
            fn exposure_schema() -> PropsSchema {
                PropsSchema::new().required("value", ContractType::number())
            }
        }

        impl PageType for TestPage {
            const CONTRACT: PageContract<Self> = PageContract::new("Test");
        }

        let response = Page(TestPage { value: 42 }).into_response();
        let pending = response
            .extensions()
            .get::<crate::inertia::PendingPage>()
            .expect("the render is recorded for the middleware");
        assert_eq!(pending.component(), "Test");
        assert_eq!(pending.props(), &serde_json::json!({"value": 42}));
    }
}
