//! The bootstrap layer: assemble the [`Application`](arcature::Application)
//! from configuration.
//!
//! [`app`] reads `.env`, loads the typed [`Config`](crate::config::Config),
//! and wires every enabled subsystem into the builder. [`state_fn`] builds
//! the per-request [`AppState`] from the started resources at run time.
//! [`error_pages`] holds the layer that turns an error status into an Inertia
//! page.

pub mod app;
pub mod error_pages;
pub mod state;

pub use app::{BootOptions, Bootstrapped, app};
pub use state::{AppState, state_fn};
