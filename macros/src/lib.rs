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
//!
//! That cycle is also why every example in this crate's module docs is
//! tagged ```` ```ignore ````. The examples show what a developer writes and
//! what the macro expands to, and both name `::arcature::` items that no
//! doctest compiled here can resolve. The examples that *are* compiled live
//! in `arcature` itself, on the items these macros generate impls for.

mod application;
mod command;
mod component;
mod controller;
mod diagnostic;
mod event;
mod field_shape;
mod job;
mod job_handler;
mod listener;
mod middleware;
mod model;
mod module;
mod page;
mod page_macro;
mod policy;
mod provider;
mod redirect;
mod request;
mod request_cache;
mod resource;
mod route_model;
mod routes;
mod schema;
mod service;
mod signature;
mod test_attr;
mod util;

use proc_macro::TokenStream;

use crate::diagnostic::MacroResult;

/// Converts a macro implementation's [`MacroResult`] into the token stream
/// the compiler consumes: the expansion on success, a `compile_error!`
/// invocation carrying the `ARC-M<NNN>` code on failure. No Arcature macro
/// panics on an ordinary syntax mistake.
fn finish(result: MacroResult) -> TokenStream {
    match result {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// `#[model(table = "users")]` — see `model.rs`.
#[proc_macro_attribute]
pub fn model(attr: TokenStream, item: TokenStream) -> TokenStream {
    model::model(attr, item)
}

/// `#[request]` — see `request.rs`.
#[proc_macro_attribute]
pub fn request(attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(request::request(attr.into(), item.into()))
}

/// `#[controller]` — see `controller.rs`.
#[proc_macro_attribute]
pub fn controller(attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(controller::controller(attr.into(), item.into()))
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

/// `#[derive(DxComponent)]` — see `component.rs`.
#[proc_macro_derive(DxComponent, attributes(component))]
pub fn derive_dx_component(input: TokenStream) -> TokenStream {
    finish(component::derive(input.into()))
}

/// `#[service]` — see `service.rs`.
#[proc_macro_attribute]
pub fn service(attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(service::service(attr.into(), item.into()))
}

/// `#[provider]` — see `provider.rs`.
#[proc_macro_attribute]
pub fn provider(attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(provider::provider(attr.into(), item.into()))
}

/// `#[policy(Model)]` — see `policy.rs`.
#[proc_macro_attribute]
pub fn policy(attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(policy::policy(attr.into(), item.into()))
}

/// `#[middleware]` — see `middleware.rs`.
#[proc_macro_attribute]
pub fn middleware(attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(middleware::middleware(attr.into(), item.into()))
}

/// `#[command("name")]` — see `command.rs`.
#[proc_macro_attribute]
pub fn command(attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(command::command(attr.into(), item.into()))
}

/// `#[job_handler]` — see `job_handler.rs`.
#[proc_macro_attribute]
pub fn job_handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(job_handler::job_handler(attr.into(), item.into()))
}

/// `#[route_model(entity = ..., key_type = ...)]` — see `route_model.rs`.
#[proc_macro_attribute]
pub fn route_model(attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(route_model::route_model(attr.into(), item.into()))
}

/// `#[request_cache(name = "...", key = "...")]` — see `request_cache.rs`.
#[proc_macro_attribute]
pub fn request_cache(attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(self::request_cache::request_cache(attr.into(), item.into()))
}

/// `#[resource]` — see `resource.rs`.
#[proc_macro_attribute]
pub fn resource(attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(resource::resource(attr.into(), item.into()))
}

/// `#[arcature::test(app = ...)]` — see `test_attr.rs`.
#[proc_macro_attribute]
pub fn test(attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(test_attr::test_attr(attr.into(), item.into()))
}

/// `#[page("users/show")]` — see `page.rs`.
#[proc_macro_attribute]
pub fn page(attr: TokenStream, item: TokenStream) -> TokenStream {
    finish(page::page(attr.into(), item.into()))
}

/// `page_macro!(ShowUserPage { .. })` — see `page_macro.rs`. Named
/// `page_macro` rather than `page` because attribute and function-like
/// macros share one namespace, and `#[page("name")]` already owns `page`.
#[proc_macro]
pub fn page_macro(input: TokenStream) -> TokenStream {
    finish(self::page_macro::page_macro(input.into()))
}

/// `application! { pub App { .. } }` — see `application/mod.rs`.
#[proc_macro]
pub fn application(input: TokenStream) -> TokenStream {
    finish(self::application::application(input.into()))
}

/// `module! { pub Accounts { .. } }` — see `module/mod.rs`.
#[proc_macro]
pub fn module(input: TokenStream) -> TokenStream {
    finish(self::module::module(input.into()))
}

/// `routes! { pub app { .. } }` — see `routes/mod.rs`.
#[proc_macro]
pub fn routes(input: TokenStream) -> TokenStream {
    finish(self::routes::routes(input.into()))
}

/// `redirect!(route::...)` — see `redirect.rs`.
#[proc_macro]
pub fn redirect(input: TokenStream) -> TokenStream {
    finish(redirect::redirect(input.into()))
}
