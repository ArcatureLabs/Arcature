//! Assemble the [`Application`] from configuration.
//!
//! [`app`] is the composition root: it loads `.env`, reads the typed
//! [`Config`](crate::config::Config), and wires every enabled subsystem into
//! the `Application` builder. The generated `src/lib.rs` calls
//! `app()?.run_with_state(state_fn()).await`.

use arcature::prelude::*;

/// Build the fully-wired [`Application`].
///
/// Calls `dotenvy::dotenv` (a no-op when `.env` is absent), loads the typed
/// config from the environment, registers job handlers, and returns the
/// assembled `Application<AppState>`. `main.rs` runs it with
/// [`state_fn`](super::state_fn).
pub fn app() -> Result<Application<crate::bootstrap::AppState>> {
    dotenvy::dotenv().ok();
    let config = crate::config::load()?;
    Ok(Application::new()
        .routes(crate::routes::routes())
        .bind(&config.bind_addr)
        .port(config.port)
        .database(config.database)
        .cache(config.cache)
        .storage(config.storage)
        .mail(config.mail)
        .jobs(jobs_registry())
        .build())
}

/// Register job handlers.
///
/// Add one `registry.add(&MyJob::JOB, |job| async move { ... })` line per job
/// kind. Empty by default; the worker still runs and the scheduler (if any)
/// still enqueues, but unknown kinds are logged and dropped.
fn jobs_registry() -> Registry {
    let mut registry = Registry::new();
    // registry.add(&MyJob::JOB, |job| async move {
    //     // ...run the job...
    //     Ok(())
    // }).expect("duplicate job registration");
    let _ = &mut registry;
    registry
}
