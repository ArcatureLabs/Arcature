//! The bootstrap layer: assemble the [`Application`] from configuration.
//!
//! [`app`] reads `.env`, loads the typed [`Config`](crate::config::Config), and
//! wires every enabled subsystem (database, cache, storage, mail, jobs) into
//! the `Application` builder. [`state_fn`] is the closure that builds the
//! per-request [`AppState`] from the started [`Resources`] at run time.

pub mod app;
pub mod state;

pub use app::app;
pub use state::{AppState, state_fn};
