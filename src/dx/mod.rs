//! The Arcature application DX layer.
//!
//! This module hosts the *runtime* contracts that the `arcature-macros`
//! proc-macro crate's DSL macros generate code against. The proc-macro
//! crate itself depends on no Arcature runtime crate; its expansions
//! reference these types through absolute `::arcature::` paths, which
//! resolve in the downstream application crate (which has `arcature` on
//! its dependency graph).
//!
//! The module is gated behind the `dx` Cargo feature. Enabling `dx` pulls
//! in the `arcature-macros` DSL macros (re-exported at the crate root:
//! `module!`, `application!`, `routes!`, `#[service]`, `#[resource]`,
//! `#[page]`, etc.) and the runtime contracts those macros generate
//! against.
//!
//! # Current surface
//!
//! - [`DxComponent`](crate::DxComponent) -- a marker trait with a static
//!   `NAME`. `#[derive(DxComponent)]` generates an `impl DxComponent` with
//!   the type name (or a custom name via `#[dx_component(name = "...")]`).
//! - [`ApplicationGraph`] -- the assembled module dependency graph, with
//!   duplicate-module, unknown-import, and circular-dependency validation.
//! - [`ModuleDescriptor`] -- a single feature module's metadata (name,
//!   imports, exports, controllers, services, policies, routes).
//! - [`GraphError`] -- typed errors from graph validation.
//! - [`RouteMethod`] / [`RouteDescriptor`] -- route metadata for the
//!   `routes!` macro. Const-constructible, `&'static`-based.
//! - [`ControllerMethod`] -- controller method metadata for `#[controller]`.
//!   Const-constructible: inspectable without running the application.
//! - [`Empty`] -- 204 No Content response (always available with `dx`).
//! - [`Json`] / [`Page`] -- high-level response types. Implement
//!   `IntoResponse` so controllers return `Result<Json<T>>`,
//!   `Result<Empty>`, `Result<Page<T>>` without manual plumbing.
//! - [`RouteModel`] -- a deliberate model-binding contract (behind `dx`
//!   and `database`). A type implementing `RouteModel` can be loaded from
//!   the database by a route parameter. Binding does NOT imply
//!   authorization.
//! - [`Bound`] -- a genuine Axum extractor that loads a model by route
//!   param (behind `dx` + `database` + `api`). Returns 404 `Problem` on
//!   miss, 400 on malformed key, 500 on DB error.
//! - [`Resolve`] -- typed application resource resolution. A type
//!   implementing `Resolve<S>` can be constructed from application state
//!   `S` -- cheaply, at compile time, with no runtime container.
//!   `#[service]` generates `impl Resolve<S>`; Arcature provides impls
//!   for built-in resources (`Db`).
//! - [`Service`] -- a marker for service types. Extends `DxComponent`
//!   with `DEPS` metadata -- the dependency type names. Service
//!   dependency cycles are impossible by construction (value composition).
//! - [`Inject`] -- an Axum extractor that constructs any `T: Resolve<S>`
//!   from application state. Axum remains the handler runtime.
//! - [`Provider`] -- a marker for startup-constructed application
//!   resources. Carries `Error` and `DEPS`. The developer writes the init
//!   logic (business behavior, not mechanical plumbing).
//! - [`Command`] / [`CommandRegistry`] -- typed application commands
//!   dispatched by name through `CommandRegistry::run`. `#[command]`
//!   generates the `Command` impl; `CommandRegistry::register_command`
//!   wires it up.
//! - [`RequestCache`] / [`RequestCacheKey`] -- the per-request memo store
//!   `#[request_cache]` resolves through. An Axum extractor backed by the
//!   request's own extensions, so a memo cannot outlive or escape the
//!   request that produced it.
//! - [`RequestCacheDescriptor`] -- the compile-time half of the same
//!   feature: what the UAG records about a memoized resolver.

pub mod application_graph;
#[cfg(all(feature = "dx", feature = "database", feature = "api"))]
pub mod bound;
#[cfg(feature = "dx")]
pub mod command;
pub mod controller_metadata;
#[cfg(all(feature = "dx", feature = "database"))]
pub mod db_from_state;
pub mod field_metadata;
pub mod from_state;
pub mod graph;
pub mod provider;
#[cfg(feature = "dx")]
pub mod request_cache;
pub mod resolve;
pub mod response;
pub mod route_metadata;
#[cfg(all(feature = "dx", feature = "database"))]
pub mod route_model;
pub mod service;

pub use application_graph::{ApplicationGraph, GraphError};
#[cfg(all(feature = "dx", feature = "database", feature = "api"))]
pub use bound::Bound;
#[cfg(feature = "dx")]
pub use command::{Command, CommandError, CommandRegistry};
pub use controller_metadata::{ControllerMetadata, ControllerMethod};
#[cfg(all(feature = "dx", feature = "database"))]
pub use db_from_state::DbFromState;
pub use field_metadata::{FieldShape, RequestMetadata, ResourceMetadata};
#[cfg(feature = "cache")]
pub use from_state::CacheFromState;
#[cfg(feature = "events")]
pub use from_state::EventsFromState;
#[cfg(feature = "jobs")]
pub use from_state::JobsFromState;
#[cfg(feature = "mail")]
pub use from_state::MailFromState;
#[cfg(feature = "storage-fs")]
pub use from_state::StorageFromState;
// The A12 binding descriptors (`JobBinding`, `CommandBinding`, and the
// re-exported `ListenerBinding` / `ScheduleBinding` / `ScheduleCadence`)
// are pure compile-time metadata defined in `graph` (behind `dx` only),
// exactly like `ListenerBinding`. They are re-exported UNGATED (under `dx`)
// so the UAG can serialize them WITHOUT pulling the
// `jobs` runtime subsystem -- which drags in `database`/`chrono`/`tokio-util`
// (small apps remain small). The runtime types (`Scheduler`, `Worker`, ...)
// stay behind `jobs` in their own submodules.
pub use graph::{
    CommandBinding, JobBinding, ModuleDescriptor, ModuleNode, ScheduleBinding, ScheduleCadence,
};
// Re-export the runtime-owned binding types so `crate::dx::*` is the single
// entry point for all module-graph binding metadata.
pub use crate::events::ListenerBinding;
pub use provider::Provider;
#[cfg(feature = "dx")]
pub use request_cache::RequestCacheDescriptor;
#[cfg(feature = "dx")]
pub use request_cache_store::{RequestCache, RequestCacheKey};
pub use resolve::Resolve;
pub use response::{Empty, Json, Page, page};
pub use route_metadata::{RouteDescriptor, RouteMethod};
#[cfg(all(feature = "dx", feature = "database"))]
pub use route_model::RouteModel;
pub use service::{Inject, Service};
