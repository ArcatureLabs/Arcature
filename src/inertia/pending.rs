//! [`PendingPage`]: a page render a handler has *declared* but not yet
//! performed.
//!
//! # Why the render is deferred
//!
//! [`IntoResponse`](axum::response::IntoResponse) receives nothing but the
//! value being converted. A real Inertia render needs the request -- the
//! `X-Inertia` headers decide between an HTML document and a JSON page
//! object, the partial-reload headers decide which props are resolved at
//! all, and the page object carries the request URL. None of that is
//! reachable from `into_response`.
//!
//! So [`Page<T>`](crate::dx::Page) does the only half it can do on its own:
//! it records the component name and the serialized props in a `PendingPage`
//! attached to the response extensions. The Inertia middleware -- which does
//! hold the request -- picks it up on the way out and performs the render.
//!
//! # If the middleware is not installed
//!
//! The placeholder body is a `500` problem document saying exactly that, not
//! an empty `200`. A `Page<T>` returned from an application that never
//! called `.inertia(..)` is a wiring mistake, and a wiring mistake should be
//! loud at the first request rather than silently serving blank pages.

use axum::response::{IntoResponse, Response};

/// A declared-but-unrendered page: the component identity plus its
/// serialized props.
///
/// Attached to a response by [`Page<T>`](crate::dx::Page) and consumed by
/// [`InertiaLayer`](crate::inertia::InertiaLayer). Cloneable because
/// response extensions require it; the clone is one `&'static str` and one
/// `serde_json::Value`.
#[derive(Clone, Debug)]
pub struct PendingPage {
    component: &'static str,
    props: serde_json::Value,
}

impl PendingPage {
    /// Record a page render for the middleware to perform.
    #[must_use]
    pub fn new(component: &'static str, props: serde_json::Value) -> Self {
        Self { component, props }
    }

    /// The frontend component identity.
    #[must_use]
    pub fn component(&self) -> &'static str {
        self.component
    }

    /// The serialized props.
    #[must_use]
    pub fn props(&self) -> &serde_json::Value {
        &self.props
    }

    /// Consume the record, yielding the component and its props.
    #[must_use]
    pub fn into_parts(self) -> (&'static str, serde_json::Value) {
        (self.component, self.props)
    }

    /// The placeholder response the handler returns: the `PendingPage` in an
    /// extension, over a body that is only ever seen when nothing picked the
    /// extension up.
    #[must_use]
    pub fn into_response(self) -> Response {
        let mut response = not_installed(self.component);
        response.extensions_mut().insert(self);
        response
    }
}

/// The body a `Page<T>` falls back to when no Inertia middleware rendered
/// it.
fn not_installed(component: &str) -> Response {
    crate::api::Problem::of(crate::api::ProblemKind::Internal)
        .with_detail(format!(
            "The handler returned a page (`{component}`) but no Inertia layer \
             is installed to render it. Add `.inertia(InertiaConfig::…)` to \
             the application builder."
        ))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn the_placeholder_carries_the_record() {
        let pending = PendingPage::new("Home", serde_json::json!({"a": 1}));
        let response = pending.into_response();
        let record = response
            .extensions()
            .get::<PendingPage>()
            .expect("the extension travels with the response");
        assert_eq!(record.component(), "Home");
        assert_eq!(record.props(), &serde_json::json!({"a": 1}));
    }

    #[test]
    fn an_unrendered_page_is_a_loud_500_not_a_blank_200() {
        // A missing `.inertia(..)` must not look like a working route that
        // happens to return nothing.
        let response = PendingPage::new("Home", serde_json::Value::Null).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
