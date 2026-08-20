#![forbid(unsafe_code)]

//! The generated application crate root.
//!
//! Modules outside `src/` (`app/`, `bootstrap/`, `config/`, `database/`,
//! `routes/`) are pulled in with `#[path]` so the on-disk layout matches the
//! Laravel-style structure. `run()` assembles the [`Application`] via the
//! bootstrap layer and serves it with the shared [`AppState`].

#[path = "../app/mod.rs"]
pub mod app;
#[path = "../bootstrap/mod.rs"]
pub mod bootstrap;
#[path = "../config/mod.rs"]
pub mod config;
#[path = "../database/mod.rs"]
pub mod database;
#[path = "../routes/mod.rs"]
pub mod routes;

use arcature::prelude::*;

/// Build the application and serve it until shutdown.
pub async fn run() -> std::io::Result<()> {
    bootstrap::app()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?
        .run_with_state(bootstrap::state_fn())
        .await
        .map_err(|e| {
            eprintln!("{e}");
            std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
        })
}
