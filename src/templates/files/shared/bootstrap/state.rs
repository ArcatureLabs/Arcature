//! The application state: the cloneable bundle every Axum extractor sees via
//! `State<AppState>`.
//!
//! Each subsystem field mirrors one started subsystem from [`Resources`]; all
//! are `Option` so the app degrades gracefully when a feature is disabled.
//! The remaining fields are configuration values a handler needs at request
//! time. [`state_fn`] is the closure `Application::run_with_state` calls once
//! startup has finished.

use std::sync::Arc;

use arcature::auth::PasswordHasher;
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
    /// The Argon2id hasher.
    ///
    /// Shared rather than rebuilt per request: constructing it derives the
    /// parameter set and can fail, and neither belongs on the login path.
    pub hasher: Arc<PasswordHasher>,
    /// The application name, shown in the layout.
    pub app_name: String,
    /// The externally reachable base URL, used to build links in mail.
    pub app_url: String,
    /// The `From` address on outgoing mail.
    pub mail_from: String,
}

/// Build the state closure from the resolved configuration.
///
/// The closure captures the configuration values by clone because it outlives
/// [`Config`](crate::config::Config): the builder consumes the config's
/// subsystem halves, and the closure is only called after startup.
pub fn state_fn(
    config: &crate::config::Config,
    hasher: Arc<PasswordHasher>,
) -> Arc<dyn Fn(&Resources, &Lifecycle) -> AppState + Send + Sync> {
    let app_name = config.app_name.clone();
    let app_url = config.app_url.clone();
    let mail_from = config.mail_from.clone();
    Arc::new(move |res, _lc| AppState {
        db: res.db().cloned(),
        jobs: res.jobs().cloned(),
        cache: res.cache().cloned(),
        storage: res.storage().cloned(),
        mail: res.mail().cloned(),
        hasher: hasher.clone(),
        app_name: app_name.clone(),
        app_url: app_url.clone(),
        mail_from: mail_from.clone(),
    })
}
