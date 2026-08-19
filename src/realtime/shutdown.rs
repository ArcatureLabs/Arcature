//! The shutdown config: idempotent drain signal for realtime endpoints.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// The shutdown configuration. Cheap to clone (Arc-backed).
#[derive(Clone, Debug)]
pub struct ShutdownConfig {
    inner: Arc<ShutdownInner>,
}

struct ShutdownInner {
    max_connections: usize,
    draining: AtomicBool,
    drain_notify: Notify,
}

impl std::fmt::Debug for ShutdownInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShutdownInner")
            .field("max_connections", &self.max_connections)
            .field("draining", &self.draining.load(Ordering::Relaxed))
            .finish()
    }
}

impl ShutdownConfig {
    /// Create a shutdown config with the given max connections.
    #[must_use]
    pub fn new(max_connections: usize) -> Self {
        Self {
            inner: Arc::new(ShutdownInner {
                max_connections,
                draining: AtomicBool::new(false),
                drain_notify: Notify::new(),
            }),
        }
    }

    /// Begin the drain (idempotent). Sets the draining flag and notifies all
    /// waiters.
    pub fn begin_drain(&self) {
        if !self.inner.draining.swap(true, Ordering::Relaxed) {
            self.inner.drain_notify.notify_waiters();
        }
    }

    /// Whether the config is in draining mode.
    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.inner.draining.load(Ordering::Relaxed)
    }

    /// The configured max connections.
    #[must_use]
    pub fn max_connections(&self) -> usize {
        self.inner.max_connections
    }

    /// A future that resolves when the drain begins.
    pub fn drain_notified(&self) -> impl std::future::Future<Output = ()> + '_ {
        let notify = self.inner.drain_notify.notified();
        async move {
            if self.is_draining() {
                return;
            }
            notify.await;
        }
    }
}
