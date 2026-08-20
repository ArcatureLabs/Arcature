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
#[cfg(feature = "jobs")]
pub mod jobs_runtime;
pub mod lifecycle;
pub mod pipeline;
pub mod resources;

pub use builder::{Application, ApplicationBuilder};
pub use lifecycle::{Lifecycle, LifecycleState};
pub use resources::Resources;

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
