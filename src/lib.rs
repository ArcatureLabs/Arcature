//! Arcature — an opinionated full-stack Rust web framework.
//!
//! One package, batteries included. `cargo add arcature` is enough for the
//! canonical generated application: HTTP routing, native Inertia, database,
//! auth, validation, cache, storage, mail, jobs, events, and the `arc` CLI.
//!
//! # Quick start
//!
//! ```no_run
//! use arcature::application::EngineResult;
//! use arcature::prelude::*;
//!
//! #[arcature::main]
//! async fn main() -> EngineResult<()> {
//!     Application::<()>::new()
//!         .routes(Routes::new([Route::get("/", index).name("home")]))
//!         .build()
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

/// The Arcature framework version, as `MAJOR.MINOR.PATCH`.
pub const FRAMEWORK_VERSION: &str = env!("CARGO_PKG_VERSION");

// --- Always-on kernel -------------------------------------------------------

pub mod application;
pub mod assets;
pub mod config;
mod error;
pub mod http;
pub mod proxy;
pub mod routing;

// The Arcature application DX layer: the unified DSL (`module!`,
// `application!`, `routes!`, `#[service]`, `#[resource]`, `#[page]`, ...)
// and the runtime contracts those macros generate code against
// (`ApplicationGraph`, `ModuleDescriptor`, `Resolve<S>`, `Service`,
// `Bound<T>`, `Command`, ...). Gated behind the `dx` feature, which pulls
// in the `arcature-macros` DSL macros and the `events` + `jobs` subsystems
// whose binding-metadata types `ModuleDescriptor` aggregates.
#[cfg(feature = "dx")]
pub mod dx;

// The one-port development proxy (Vite over IPC). Engine plumbing, gated
// behind the `dev-proxy` feature so production builds pay nothing. The layer
// is a zero-overhead pass-through when `ARCATURE_VITE_IPC` is unset.
#[cfg(feature = "dev-proxy")]
pub mod dev_proxy;

pub use application::{
    Application, ApplicationBuilder, EngineError, Lifecycle, LifecycleState, Resources,
};
pub use error::{Error, Result, ValidationError, bad_request, forbidden, not_found};
#[cfg(any(feature = "api", feature = "inertia"))]
pub use http::json;
#[cfg(feature = "uploads")]
pub use http::{Attachment, BoundedField, BoundedMultipart, MultipartError, MultipartLimits};
pub use http::{RedirectResponse, no_content, redirect, text};
pub use routing::{IntoRoutes, Middleware, Next, Route, RouteGroup, RouterState, Routes};

// Re-export axum method-routing constructors at the crate root so the kernel
// compiles without a direct `axum::routing` import in user code.
pub use axum::middleware::from_fn;
pub use axum::routing::{any, delete, get, head, options, patch, post, put};

/// The base trait that gives a framework component (job, event, etc.) its
/// static name, used for dispatch lookup and inspection. Always available;
/// the `#[job]` and `#[derive(Event)]` macros generate `impl DxComponent`.
pub trait DxComponent {
    /// The static component name (e.g. the type name).
    const NAME: &'static str;
}

// --- The Arcature application DX layer (feature `dx`) ----------------------
//
// Runtime contracts the DSL macros (`module!`, `application!`, `routes!`,
// `#[service]`, `#[resource]`, `#[page]`, `#[route_model]`, ...) generate
// code against. These are re-exported at the crate root so downstream code
// references them as `arcature::ApplicationGraph`, `arcature::Resolve`, etc.
#[cfg(all(feature = "dx", feature = "database", feature = "api"))]
pub use dx::Bound;
#[cfg(feature = "dx")]
pub use dx::{
    ApplicationGraph, ControllerMetadata, ControllerMethod, Empty, FieldShape, GraphError, Inject,
    Json, ModuleDescriptor, ModuleNode, Page, Provider, RequestCacheDescriptor, RequestMetadata,
    Resolve, ResourceMetadata, RouteDescriptor, RouteMethod, Service, page,
};
#[cfg(feature = "dx")]
pub use dx::{Command, CommandError, CommandFuture, CommandRegistry};
#[cfg(feature = "dx")]
pub use dx::{CommandBinding, JobBinding};
#[cfg(all(feature = "dx", feature = "database"))]
pub use dx::{DbFromState, RouteModel};
#[cfg(feature = "dx")]
pub use dx::{RequestCache, RequestCacheKey};

// --- Feature-gated subsystems ----------------------------------------------

#[cfg(feature = "inertia")]
pub mod inertia;
#[cfg(feature = "inertia")]
pub use inertia::{
    AssetVersion, Head, Inertia, InertiaConfig, InertiaError, InertiaLayer, InertiaRequest,
    RootDocument, ScriptBody, default_root_document, vite_root_document,
};
// The `inertia!()` macro is `#[macro_export]` inside `inertia::mod`, so it is
// reachable as `arcature::inertia!(...)` at the crate root.

#[cfg(feature = "database")]
pub mod database;
#[cfg(feature = "database")]
pub use database::{DatabaseConfig, Db};
// Re-export SeaORM so the `#[model]` macro's `::arcature::sea_orm::DeriveEntityModel`
// path resolves. The database feature pulls in sea_orm.
#[cfg(feature = "database")]
pub use sea_orm;

#[cfg(feature = "auth")]
pub mod auth;
#[cfg(feature = "auth")]
pub use auth::{
    Auth, AuthError, AuthManager, AuthUser, AuthzError, CsrfConfig, CsrfConfigError, CsrfError,
    CsrfLayer, CsrfToken, Current, Flash, FlashError, FlashLevel, FlashMessage, LoginBuilder,
    OptionalAuth, OptionalCurrent, PasswordHashError, PasswordHashString, PasswordHasher,
    PasswordSecret, PasswordVerifyError, Policy, RehashOutcome, SameSite, Session,
    SessionBuildError, SessionConfig, SessionConfigError, SessionError, SessionKey, SessionLayer,
    SigningKeyReason, UserLoader, verify_password,
};
// The database-backed session store, so `arcature::DbSessionStore` reads the
// same way as the rest of the auth surface. Behind `session-store-db`.
#[cfg(feature = "session-store-db")]
pub use auth::{DbSessionStore, SessionStoreError};

// Keyed cryptography derived from `APP_KEY`. Independent of `auth`: an API
// with no cookies and no sessions may still need to hand out an opaque token,
// and making it compile a password hasher and a session layer to get one
// would be a packaging decision pretending to be a security one.
#[cfg(any(feature = "crypt", feature = "signed-urls"))]
pub mod crypt;
#[cfg(any(feature = "crypt", feature = "signed-urls"))]
pub use crypt::{AppKey, AppKeyError};
#[cfg(feature = "signed-urls")]
pub use crypt::{Clock, SignedUrlError, SystemClock, UrlSigner};
#[cfg(feature = "crypt")]
pub use crypt::{DecryptError, EncryptError, Encrypter};

#[cfg(feature = "validation")]
pub mod validation;
#[cfg(feature = "validation")]
pub use validation::{
    Request, Validated, ValidatedForm, ValidatedJson, ValidatedPath, ValidatedQuery,
    validate_or_problem, validation_problem,
};
// Re-export `validator` so the `#[request]` macro's
// `#[derive(::arcature::Validate)]` resolves. The validation feature pulls in
// validator with the derive feature.
#[cfg(feature = "validation")]
pub use validation::rejection::{
    from_form_rejection, from_json_rejection, from_path_rejection, from_query_rejection,
};
#[cfg(feature = "uploads")]
pub use validation::{UPLOAD_FIELD, UploadPolicy, UploadedFile, from_multipart_rejection};
#[cfg(feature = "validation")]
pub use validator;

#[cfg(feature = "cache")]
pub mod cache;
#[cfg(feature = "cache")]
pub use cache::{
    Cache, CacheConfig, CacheConfigError, CacheConnectError, CacheError, CacheHealthError,
    Namespace,
};

#[cfg(feature = "storage-fs")]
pub mod storage;
#[cfg(feature = "uploads")]
pub use storage::{
    AllowedExtensions, ContentAddress, ContentHasher, Extension, FilenameError, SafeFilename,
    SniffError, SniffedType, UploadError, UploadWriter,
};
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
    Mailable, Mailer, SmtpConfig, SmtpCredentials, TlsMode,
};

#[cfg(feature = "notifications")]
pub mod notifications;
#[cfg(feature = "notifications-broadcast")]
pub use notifications::{BroadcastChannels, BroadcastNotifications, PerRecipientChannels};
#[cfg(feature = "notifications")]
pub use notifications::{
    BroadcastContent, Channel, DatabaseContent, Delivery, MailContent, Notifiable, Notification,
    NotificationError, Notifier, Recipient,
};
#[cfg(feature = "notifications-db")]
pub use notifications::{DatabaseNotifications, NotificationId, StoredNotification};

#[cfg(feature = "jobs")]
pub mod jobs;
#[cfg(feature = "jobs")]
pub use jobs::{
    ClaimedJob, DEFAULT_MAX_PAYLOAD_BYTES, EnqueueError, EnqueuedJob, Job, JobError, JobModel,
    JobRequest, JobStatus, Jobs, MigrateError, RegisterError, Registry, RetryPolicy,
    ScheduleBinding, ScheduleCadence, Scheduler, SchedulerError, Worker, WorkerBuilder,
    WorkerConfig, WorkerError,
};

#[cfg(feature = "events")]
pub mod events;
#[cfg(feature = "events")]
pub use events::{DispatchError, Dispatcher, Event, ListenerBinding};

#[cfg(feature = "realtime")]
pub mod realtime;
#[cfg(feature = "realtime")]
pub use realtime::{Broadcast, SseEndpoint, WebSocketEndpoint};

// The `api` module is always available: `Problem` (RFC 9457) needs only
// always-on deps and the validation subsystem depends on it. The `api`
// feature gates additional conveniences layered on top.
pub mod api;
pub use api::{PROBLEM_JSON, Problem, ProblemBuilder, ProblemKind};

// Hashed personal access tokens. Off by default: it brings a table and a
// migration, and an application that only serves a browser never needs one.
// Independent of `auth` on purpose -- see the feature's comment in
// `Cargo.toml`.
#[cfg(feature = "api-tokens")]
pub mod tokens;
#[cfg(feature = "api-tokens")]
pub use tokens::{ApiAuth, ApiToken, ApiTokenError, ApiTokenId, ApiTokens};

#[cfg(feature = "observe")]
pub mod observe;
#[cfg(feature = "observe")]
pub use observe::RequestId;

#[cfg(feature = "pages")]
pub mod pages;

#[cfg(feature = "templates")]
pub mod templates;

// Compiled HTML views. Off by default: a generated application renders
// through Inertia, and a build that never serves a server-rendered page has
// no reason to carry a template compiler.
#[cfg(feature = "views")]
pub mod view;
#[cfg(feature = "views")]
pub use view::{View, ViewError, view};
// Re-export the certified askama so `#[template(askama = arcature::askama)]`
// resolves from an application that does not depend on askama itself -- the
// same reason `sea_orm`, `validator` and `lettre` are re-exported.
#[cfg(feature = "views")]
pub use askama;

// Fluent translation catalogs. Off by default: an application that ships one
// language should not carry a message parser and a plural-rule table to say
// so. `src/i18n/mod.rs` states why a runtime parser is acceptable for
// developer-authored `.ftl` files when `views` rejected one for templates,
// and discloses the `unsafe` the dependency subtree adds.
#[cfg(feature = "i18n")]
pub mod i18n;
#[cfg(feature = "i18n")]
pub use i18n::{Catalog, Catalogs, I18nError, Locale, LocaleId, LocaleLayer};

#[cfg(feature = "cli")]
pub mod cli;

#[cfg(feature = "uag")]
pub mod uag;

#[cfg(feature = "oauth")]
pub mod oauth;

// The harness is a development tool. It is gated so it cannot be reached
// from a production build even by accident -- a test double that answers
// like the real subsystem is exactly the thing that must not ship.
#[cfg(feature = "test-kit")]
pub mod test_kit;

#[cfg(feature = "macros")]
mod macros;
#[cfg(feature = "macros")]
pub use macros::main;

// Proc-macro re-exports from the `arcature-macros` crate. The derive macros
// `Job` and `Event` share names with the traits `jobs::Job` and
// `events::Event` in different namespaces (the same trick `serde` uses).
#[cfg(feature = "macros")]
pub use arcature_macros::{Event, Job, listener, model, request};

// The unified DSL macros. A macro and a value may share a name because they
// live in different namespaces: `page` is both the `#[page("Name")]`
// attribute and the `page(props)` constructor, and `redirect` is both the
// `redirect!()` macro and the `redirect()` response builder. The one pairing
// Rust cannot express is two macros with one name, so the `page!` bang is
// exported as `page_macro!` -- `#[page]` already owns `page` in the macro
// namespace.
#[cfg(all(feature = "macros", feature = "dx"))]
pub use arcature_macros::{
    DxComponent, application, command, controller, job_handler, middleware, module, page,
    page_macro, policy, provider, redirect, request_cache, resource, route_model, routes, service,
};

// --- The curated prelude ----------------------------------------------------

pub mod prelude;

// --- Serialization re-export (shared by inertia/api/auth/etc.) -------------

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

// Re-export serde_json so the `inertia!()` macro's `$crate::serde_json::json!`
// path resolves from downstream code. Always available (serde_json is a
// non-optional dependency).
pub use serde_json;
