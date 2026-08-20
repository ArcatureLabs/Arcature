//! Proc-macro crate for Arcature.
//!
//! Provides the attribute/derive macros that power the Arcature developer
//! experience:
//! - `#[model(table = "users")]` — a SeaORM entity model with the query facade.
//! - `#[request]` — a validated request struct (with `#[validate(...)]` rules).
//! - `#[controller]` — an Axum controller with route metadata.
//! - `#[derive(Job)]` — a typed background job with a `JobModel` const.
//! - `#[derive(Event)]` — a typed in-process event for the `Dispatcher`.
//! - `#[listener(Event)]` — an event listener with dispatch metadata.
//!
//! Each macro lives in its own file (one file, one macro). This `lib.rs` is
//! only the dispatch surface: it declares each macro's `#[proc_macro_*]`
//! entry point and forwards to its implementation module. All expansions
//! reference Arcature APIs via absolute `::arcature::` paths that resolve in
//! the downstream app crate. This crate must NOT depend on `arcature` (would
//! create a cycle); it depends only on `syn`, `quote`, and `proc-macro2`.

mod controller;
mod event;
mod job;
mod listener;
mod model;
mod request;
mod util;

use proc_macro::TokenStream;

/// `#[model(table = "users")]` — see `model.rs`.
#[proc_macro_attribute]
pub fn model(attr: TokenStream, item: TokenStream) -> TokenStream {
    model::model(attr, item)
}

/// `#[request]` — see `request.rs`.
#[proc_macro_attribute]
pub fn request(attr: TokenStream, item: TokenStream) -> TokenStream {
    request::request(attr, item)
}

/// `#[controller]` — see `controller.rs`.
#[proc_macro_attribute]
pub fn controller(attr: TokenStream, item: TokenStream) -> TokenStream {
    controller::controller(attr, item)
}

/// `#[derive(Job)]` — see `job.rs`.
#[proc_macro_derive(Job, attributes(job))]
pub fn derive_job(input: TokenStream) -> TokenStream {
    job::derive_job(input)
}

/// `#[derive(Event)]` — see `event.rs`.
#[proc_macro_derive(Event)]
pub fn derive_event(input: TokenStream) -> TokenStream {
    event::derive_event(input)
}

/// `#[listener(Event)]` — see `listener.rs`.
#[proc_macro_attribute]
pub fn listener(attr: TokenStream, item: TokenStream) -> TokenStream {
    listener::listener(attr, item)
}
