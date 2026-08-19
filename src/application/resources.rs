//! The typed bundle of started subsystem handles passed to the application
//! state closure. Each field is `Some` only when the corresponding feature is
//! enabled and the subsystem connected successfully during startup.

use std::sync::Arc;

/// The resources available to the application state closure after startup.
///
/// A stateless app ignores this; a fullstack app reads `resources.db()` etc.
/// Each accessor returns `Option` so the closure degrades gracefully when a
/// subsystem is disabled.
#[derive(Clone, Default)]
pub struct Resources {
    #[cfg(feature = "database")]
    db: Option<Arc<crate::database::Db>>,
    #[cfg(feature = "cache")]
    cache: Option<Arc<crate::cache::Cache>>,
    #[cfg(feature = "storage-fs")]
    storage: Option<Arc<crate::storage::Storage>>,
    #[cfg(feature = "mail")]
    mail: Option<Arc<crate::mail::Mailer>>,
}

impl Resources {
    /// An empty resource bundle (no subsystems connected).
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// The database handle, when the `database` feature is enabled and the DB
    /// connected.
    #[cfg(feature = "database")]
    #[must_use]
    pub fn db(&self) -> Option<&crate::database::Db> {
        self.db.as_deref()
    }

    #[cfg(feature = "database")]
    pub(crate) fn set_db(&mut self, db: crate::database::Db) {
        self.db = Some(Arc::new(db));
    }

    /// The cache handle, when the `cache` feature is enabled and connected.
    #[cfg(feature = "cache")]
    #[must_use]
    pub fn cache(&self) -> Option<&crate::cache::Cache> {
        self.cache.as_deref()
    }

    #[cfg(feature = "cache")]
    pub(crate) fn set_cache(&mut self, cache: crate::cache::Cache) {
        self.cache = Some(Arc::new(cache));
    }

    /// The storage handle, when the `storage-fs` feature is enabled.
    #[cfg(feature = "storage-fs")]
    #[must_use]
    pub fn storage(&self) -> Option<&crate::storage::Storage> {
        self.storage.as_deref()
    }

    #[cfg(feature = "storage-fs")]
    pub(crate) fn set_storage(&mut self, storage: crate::storage::Storage) {
        self.storage = Some(Arc::new(storage));
    }

    /// The mailer handle, when the `mail` feature is enabled.
    #[cfg(feature = "mail")]
    #[must_use]
    pub fn mail(&self) -> Option<&crate::mail::Mailer> {
        self.mail.as_deref()
    }

    #[cfg(feature = "mail")]
    pub(crate) fn set_mail(&mut self, mail: crate::mail::Mailer) {
        self.mail = Some(Arc::new(mail));
    }
}
