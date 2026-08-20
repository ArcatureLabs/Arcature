//! The application state: the cloneable bundle every Axum extractor sees via
//! `State<AppState>`.
//!
//! Each field mirrors one started subsystem from [`Resources`]; all are
//! `Option` so the app degrades gracefully when a feature is disabled. The
//! [`state_fn`] closure is what `Application::run_with_state` calls after
//! startup to produce the state.

use std::sync::Arc;

use arcature::prelude::*;

/// The per-request application state.
///
/// Clone this from a handler via `State<AppState>`; every field is cheap to
/// clone (each subsystem handle is `Arc`-backed or connection-multiplexed).
#[derive(Clone)]
pub struct AppState {
    /// The database handle, when the `database` feature is enabled.
    pub db: Option<Db>,
    /// The job queue facade, when the `jobs` feature is enabled.
    pub jobs: Option<Jobs>,
    /// The cache handle, when the `cache` feature is enabled.
    pub cache: Option<Cache>,
    /// The storage handle, when the `storage-fs` feature is enabled.
    pub storage: Option<Storage>,
    /// The mailer handle, when the `mail` feature is enabled.
    pub mail: Option<Mailer>,
}

/// The state closure: read the started [`Resources`] (and the [`Lifecycle`])
/// and return the [`AppState`].
///
/// Pass this to `Application::run_with_state`. Each accessor on `Resources`
/// returns `Option` so a disabled subsystem contributes `None` here.
pub fn state_fn() -> Arc<dyn Fn(&Resources, &Lifecycle) -> AppState + Send + Sync> {
    Arc::new(|res, _lc| AppState {
        db: res.db().cloned(),
        jobs: res.jobs().cloned(),
        cache: res.cache().cloned(),
        storage: res.storage().cloned(),
        mail: res.mail().cloned(),
    })
}
