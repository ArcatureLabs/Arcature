//! The application composition root and lifecycle.
//!
//! [`Application`] is the high-level entry point. It owns routing, the
//! request pipeline, and the ordered startup/shutdown of subsystems
//! (database, jobs, cache, storage, mail). A normal generated app builds an
//! `Application` in `bootstrap/app.rs` and runs it from `main.rs`.
//!
//! Startup order is fixed: database first, then jobs (which reuse the
//! database pool), then cache, storage, mail. Shutdown is the reverse. A
//! startup failure tears down whatever was already started.

pub mod builder;
pub mod health;
// Spawning and stopping the worker is part of the serve path, so this module
// follows `macros`: without the certified runtime nothing ever starts a job
// runtime, and the module would compile only to sit unused.
#[cfg(all(feature = "jobs", feature = "macros"))]
pub mod jobs_runtime;
pub mod lifecycle;
pub mod pipeline;
pub mod resources;
// Where the process listens is part of the serve path, which the certified
// runtime owns: without `macros` nothing in this crate ever binds anything.
#[cfg(feature = "macros")]
pub mod serve_ipc;
// The dev-only application-graph endpoint. Gated on `uag` so that a binary
// built without that feature contains neither the route nor the artifact --
// the first of the three gates described in the module documentation.
#[cfg(feature = "uag")]
pub mod uag_endpoint;

pub use builder::{Application, ApplicationBuilder};
pub use health::{Health, HealthReport};
pub use lifecycle::{Lifecycle, LifecycleState};
pub use resources::Resources;
#[cfg(feature = "uag")]
pub use uag_endpoint::UagEndpoint;

// The framework error type for engine-level failures (binding a listener,
// startup/shutdown). Distinct from [`crate::Error`] which is for
// request-path failures.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("failed to bind listener on {address}: {source}")]
    BindListener {
        address: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid bind port: {0}")]
    InvalidPort(u16),
    #[error("failed to serve: {source}")]
    Serve {
        #[source]
        source: std::io::Error,
    },
    #[error("startup failed in {subsystem} during {stage}: {source}")]
    Startup {
        subsystem: &'static str,
        stage: &'static str,
        #[source]
        source: crate::Error,
    },
    #[error("shutdown failed in {subsystem}: {source}")]
    Shutdown {
        subsystem: &'static str,
        #[source]
        source: crate::Error,
    },
}

pub type EngineResult<T> = std::result::Result<T, EngineError>;
