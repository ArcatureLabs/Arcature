//! Arcature — an opinionated full-stack Rust web framework.
//!
//! One package, batteries included. `cargo add arcature` is enough for the
//! canonical generated application: HTTP routing, native Inertia, database,
//! auth, validation, cache, storage, mail, jobs, events, and the `arc` CLI.
//!
//! # Quick start
//!
//! ```ignore
//! use arcature::prelude::*;
//!
//! #[arcature::main]
//! async fn main() -> Result<()> {
//!     Application::new()
//!         .routes(Routes::new([Route::get("/", index).name("home")]))
//!         .run()
//!         .await
//! }
//!
//! async fn index() -> Result<Response> {
//!     Ok(text(StatusCode::OK, "hello"))
//! }
//! ```
//!
//! # Philosophy
//!
//! Arcature integrates proven wheels (Axum, Tower, Tokio, SeaORM, SQLx,
//! Inertia, OpenDAL, lettre, tracing) and owns the developer experience: the
//! application lifecycle, conventions, integration, and a coherent vocabulary.
//! The raw Axum/Tower/SeaORM escape hatches stay available for when the
//! framework's opinions run out.

#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

// Re-export the certified `axum` so downstream code targets the pinned version
// through Arcature (e.g. `arcature::axum::routing`).
pub use axum;

/// The Arcature framework version (YBF: `YEAR.BREAK.FIX`).
pub const FRAMEWORK_VERSION: &str = env!("CARGO_PKG_VERSION");

// --- Always-on kernel -------------------------------------------------------

mod error;
pub mod http;
pub mod routing;
pub mod config;
pub mod application;

pub use error::{not_found, bad_request, forbidden, Error, Result, ValidationError};
pub use application::{Application, ApplicationBuilder, EngineError, Lifecycle, LifecycleState, Resources};
pub use routing::{Middleware, Next, Route, RouteGroup, Routes, RouterState};
pub use http::{redirect, RedirectResponse, text, no_content};
#[cfg(any(feature = "api", feature = "inertia"))]
pub use http::json;

// Re-export axum method-routing constructors at the crate root so the kernel
// compiles without a direct `axum::routing` import in user code.
pub use axum::routing::{any, delete, get, head, options, patch, post, put};
pub use axum::middleware::from_fn;

// --- Feature-gated subsystems ----------------------------------------------

#[cfg(feature = "inertia")]
pub mod inertia;
#[cfg(feature = "inertia")]
pub use inertia::{
    default_root_document, AssetVersion, Inertia, InertiaConfig, InertiaError,
    InertiaLayer, RootDocument, ScriptBody,
};
// The `inertia!()` macro is `#[macro_export]` inside `inertia::mod`, so it is
// reachable as `arcature::inertia!(...)` at the crate root.

#[cfg(feature = "database")]
pub mod database;
#[cfg(feature = "database")]
pub use database::{Db, DatabaseConfig};

#[cfg(feature = "auth")]
pub mod auth;
#[cfg(feature = "auth")]
pub use auth::{
    verify_password, Auth, AuthError, AuthManager, AuthUser, AuthzError, Current, CsrfConfig,
    CsrfConfigError, CsrfError, CsrfLayer, CsrfToken, Flash, FlashError, FlashLevel, FlashMessage,
    LoginBuilder, OptionalAuth, OptionalCurrent, PasswordHashError, PasswordHasher,
    PasswordHashString, PasswordSecret, PasswordVerifyError, Policy, RehashOutcome,
    SameSite, Session, SessionBuildError, SessionConfig, SessionConfigError, SessionError,
    SessionKey, SessionLayer, SigningKeyReason, UserLoader,
};

#[cfg(feature = "validation")]
pub mod validation;
#[cfg(feature = "validation")]
pub use validation::{
    validate_or_problem, validation_problem, Request, Validated, ValidatedForm, ValidatedJson,
    ValidatedPath, ValidatedQuery,
};
#[cfg(feature = "validation")]
pub use validation::rejection::{
    from_form_rejection, from_json_rejection, from_path_rejection, from_query_rejection,
};

#[cfg(feature = "cache")]
pub mod cache;
#[cfg(feature = "cache")]
pub use cache::{
    Cache, CacheConfig, CacheConfigError, CacheConnectError, CacheError, CacheHealthError,
    Namespace,
};

#[cfg(feature = "storage-fs")]
pub mod storage;
#[cfg(feature = "storage-fs")]
pub use storage::{
    Disk, FsConfig, S3Config, Storage, StorageBuilder, StorageConfig, StorageConfigError,
    StorageConnectError, StorageError, StoragePath, StoragePathError,
};

#[cfg(feature = "mail")]
pub mod mail;
#[cfg(feature = "mail")]
pub use mail::{
    Email, EmailAttachment, EmailError, Mail, MailBuilder, MailConfigError, MailSendError,
    Mailer, Mailable, SmtpConfig, SmtpCredentials, TlsMode,
};

#[cfg(feature = "jobs")]
pub mod jobs;
#[cfg(feature = "jobs")]
pub use jobs::{Job, JobHandler, Jobs, JobError};

#[cfg(feature = "events")]
pub mod events;
#[cfg(feature = "events")]
pub use events::{Event, Dispatcher, ListenerBinding};

#[cfg(feature = "realtime")]
pub mod realtime;
#[cfg(feature = "realtime")]
pub use realtime::{Broadcast, WebSocketEndpoint, SseEndpoint, Registry};

// The `api` module is always available: `Problem` (RFC 9457) needs only
// always-on deps and the validation subsystem depends on it. The `api`
// feature gates additional conveniences layered on top.
pub mod api;
pub use api::{Problem, ProblemBuilder, ProblemKind, PROBLEM_JSON};

#[cfg(feature = "observe")]
pub mod observe;
#[cfg(feature = "observe")]
pub use observe::RequestId;

#[cfg(feature = "pages")]
pub mod pages;

#[cfg(feature = "templates")]
pub mod templates;

#[cfg(feature = "cli")]
pub mod cli;

#[cfg(feature = "macros")]
mod macros;
#[cfg(feature = "macros")]
pub use macros::main;

// --- The curated prelude ----------------------------------------------------

pub mod prelude;

// --- Serialization re-export (shared by inertia/api/auth/etc.) -------------

#[cfg(any(feature = "inertia", feature = "api", feature = "auth", feature = "database", feature = "events", feature = "jobs", feature = "validation", feature = "pages"))]
pub use serde::{Deserialize, Serialize};
