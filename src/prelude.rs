//! The curated Arcature prelude.
//!
//! `use arcature::prelude::*;` brings the normal-user surface into scope: the
//! [`Application`] engine, the [`Routes`] router, routing constructors,
//! common extractors and response helpers, and the framework [`Result`].
//! Capability entry points (when their feature is enabled) are also included.
//!
//! The prelude is deliberately *curated*, not a glob of every dependency: the
//! full surface of each subsystem lives under its module
//! (e.g. `arcature::database`, `arcature::auth`).

// Framework-owned types (always available).
pub use crate::application::{Application, ApplicationBuilder, Lifecycle, Resources};
pub use crate::config::{AppConfig, AppEnvironment, env_or, env_required};
pub use crate::error::{Error, Result, ValidationError, bad_request, forbidden, not_found};
pub use crate::http::{RedirectResponse, no_content, redirect, text};
pub use crate::routing::{Middleware, Next, Route, RouteGroup, RouterState, Routes};

// The pre-routing proxy contract (engine spec §4/§5). The application's
// proxy function maps a `ProxyRequest` borrow to a `ProxyAction`; the engine
// performs the actual HTTP work. Installed via `ApplicationBuilder::proxy`.
pub use crate::proxy::{ProxyAction, ProxyRequest};

// Routing constructors (the certified Axum functions, re-exported).
pub use crate::{any, delete, from_fn, get, head, options, patch, post, put};

// Common Axum extractors and response types, re-exported through Arcature.
pub use crate::axum::extract::{Path, State};
pub use crate::axum::http::{HeaderMap, StatusCode, Uri};
pub use crate::axum::response::{IntoResponse, Redirect, Response};

// JSON response helper (available with api or inertia).
#[cfg(any(feature = "api", feature = "inertia"))]
pub use crate::http::json;

// Serialization (available with any serde-using feature).
#[cfg(any(
    feature = "inertia",
    feature = "api",
    feature = "auth",
    feature = "database",
    feature = "events",
    feature = "jobs",
    feature = "validation",
    feature = "pages"
))]
pub use serde::{Deserialize, Serialize};

// --- Capability entry points (one primary type per enabled subsystem) -----

#[cfg(feature = "inertia")]
pub use crate::inertia::{Inertia, InertiaConfig, RootDocument};

// The `inertia!()` macro is `#[macro_export]`, so it is at the crate root.
#[cfg(feature = "inertia")]
pub use crate::inertia;

#[cfg(feature = "database")]
pub use crate::database::{DatabaseConfig, Db};

#[cfg(feature = "auth")]
pub use crate::auth::{
    Auth, AuthManager, AuthUser, Flash, FlashLevel, OptionalAuth, Policy, Session,
};

#[cfg(feature = "validation")]
pub use crate::validation::{Request, Validated};

#[cfg(feature = "cache")]
pub use crate::cache::{Cache, CacheConfig};

#[cfg(feature = "storage-fs")]
pub use crate::storage::{Storage, StorageConfig, StoragePath};

#[cfg(feature = "mail")]
pub use crate::mail::{Mail, Mailable, Mailer, SmtpConfig};

#[cfg(feature = "jobs")]
pub use crate::jobs::{Job, JobError, JobModel, JobRequest, Jobs, Registry, Worker, WorkerConfig};

#[cfg(feature = "events")]
pub use crate::events::{Dispatcher, Event};

// --- The DX layer: the unified DSL runtime contracts (feature `dx`) --------
//
// These are the types the DSL macros (`module!`, `application!`, `routes!`,
// `#[service]`, `#[resource]`, `#[page]`, ...) generate code against, plus the
// high-level response vocabulary (`Empty`, `Json`, `Page`, `page`) and the DI
// surface (`Inject`, `Resolve`, `Service`). Bring them into the prelude so a
// generated app's `use arcature::prelude::*` makes the full DSL available.
#[cfg(all(feature = "dx", feature = "database", feature = "api"))]
pub use crate::Bound;
#[cfg(feature = "dx")]
pub use crate::{
    ApplicationGraph, ControllerMetadata, ControllerMethod, Empty, FieldShape, GraphError, Inject,
    Json, ModuleDescriptor, ModuleNode, Page, Provider, RequestCacheDescriptor, RequestMetadata,
    Resolve, ResourceMetadata, RouteDescriptor, RouteMethod, Service, page,
};
#[cfg(feature = "dx")]
pub use crate::{Command, CommandError, CommandRegistry};
#[cfg(feature = "dx")]
pub use crate::{CommandBinding, JobBinding};
#[cfg(all(feature = "dx", feature = "database"))]
pub use crate::{DbFromState, RouteModel};

#[cfg(feature = "realtime")]
pub use crate::realtime::{Broadcast, SseEndpoint, WebSocketEndpoint};

#[cfg(feature = "api")]
pub use crate::api::{Problem, ProblemKind};

#[cfg(feature = "observe")]
pub use crate::observe::RequestId;

// The `#[arcature::main]` runtime entry point (requires the `macros` feature).
#[cfg(feature = "macros")]
pub use crate::main;

// The attribute macros (`#[model]`, `#[request]`, `#[listener]`) are
// re-exported at the crate root; pull them into the prelude so
// `use arcature::prelude::*` brings them into scope. The derive macros
// `#[derive(Job)]` and `#[derive(Event)]` are NOT re-exported here: they share
// names with the `Job`/`Event` traits (already imported above) in the type
// namespace, so a glob re-export would conflict. Users who derive them add
// `use arcature::Job;` / `use arcature::Event;` explicitly.
#[cfg(feature = "macros")]
pub use crate::{model, request};

#[cfg(all(feature = "macros", feature = "events"))]
pub use crate::listener;

// The unified DSL macros. `page` and `redirect` are absent from this list on
// purpose: a `use` names every namespace at once, and the imports above
// already bring in `crate::page` and `crate::redirect` -- which carry the
// macro of each name along with the value.
// `controller` sits in this group, not the one above: its expansion names
// `::arcature::ControllerMetadata`, which exists only under `dx`.
#[cfg(all(feature = "macros", feature = "dx"))]
pub use crate::{
    DxComponent, application, command, controller, job_handler, middleware, module, page_macro,
    policy, provider, request_cache, resource, route_model, routes, service,
};
