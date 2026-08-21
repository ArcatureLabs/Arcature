//! Error pages.
//!
//! Each is field-free on purpose: an error response must not depend on
//! loading data that may itself be what failed. The status text and any
//! detail live in the client component.

use arcature::prelude::*;

/// Props for the `errors/404` component: nothing was found.
#[page("errors/404")]
pub struct NotFoundPage {}

/// Props for the `errors/419` component: the session or CSRF token expired.
#[page("errors/419")]
pub struct PageExpiredPage {}

/// Props for the `errors/500` component: the server failed.
#[page("errors/500")]
pub struct ServerErrorPage {}
