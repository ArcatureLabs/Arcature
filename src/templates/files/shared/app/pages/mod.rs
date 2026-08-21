//! Page prop types: one struct per client component.
//!
//! A `#[page("component/name")]` struct is the contract between a controller
//! and the JavaScript component of that name. Every field crosses the wire,
//! so nothing lands here that a browser must not see. Add one with
//! `arc make:page`, and list it in the `module!` block in `app/mod.rs`.

pub mod errors;
pub mod home;

pub use errors::{NotFoundPage, PageExpiredPage, ServerErrorPage};
pub use home::HomePage;
