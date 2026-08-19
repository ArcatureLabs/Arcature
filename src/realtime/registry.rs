//! The connection registry: enforces a max-connection cap and tracks live
//! connections for graceful drain.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use super::error::RealtimeError;

/// The connection registry. Cheap to clone (Arc-backed). Holds the live
/// connection count and a drain signal.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    inner: Arc<RegistryInner>,
}

#[derive(Debug)]
struct RegistryInner {
    live: AtomicUsize,
    drain_signal: Notify,
}

impl Default for RegistryInner {
    fn default() -> Self {
        Self {
            live: AtomicUsize::new(0),
            drain_signal: Notify::new(),
        }
    }
}

impl Registry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of live connections.
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.inner.live.load(Ordering::Relaxed)
    }

    /// Acquire a connection guard. Returns `Err(ConnectionLimit)` if the live
    /// count is at or above `max`.
    pub fn acquire(&self, max: usize) -> Result<ConnectionGuard, RealtimeError> {
        // CAS loop to increment, but only if below max.
        loop {
            let current = self.inner.live.load(Ordering::Relaxed);
            if current >= max {
                return Err(RealtimeError::ConnectionLimit);
            }
            if self
                .inner
                .live
                .compare_exchange_weak(
                    current,
                    current + 1,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
        }
        Ok(ConnectionGuard {
            inner: self.inner.clone(),
        })
    }

    /// Drain all connections within `bound`. Returns `Err(Shutdown { remaining })`
    /// if the bound elapses before all connections close.
    pub async fn drain(&self, bound: Duration) -> Result<(), RealtimeError> {
        if self.live_count() == 0 {
            return Ok(());
        }
        let _ = tokio::time::timeout(bound, self.wait_for_zero()).await;
        let remaining = self.live_count();
        if remaining > 0 {
            Err(RealtimeError::Shutdown { remaining })
        } else {
            Ok(())
        }
    }

    async fn wait_for_zero(&self) {
        loop {
            if self.live_count() == 0 {
                return;
            }
            let notify = self.inner.drain_signal.notified();
            tokio::pin!(notify);
            tokio::select! {
                _ = &mut notify => {}
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
    }
}

/// A connection guard. Dropping it decrements the live count and notifies
/// the drain waiter.
#[derive(Debug)]
pub struct ConnectionGuard {
    inner: Arc<RegistryInner>,
}

impl ConnectionGuard {
    /// The current live count (including this guard).
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.inner.live.load(Ordering::Relaxed)
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        // CAS loop down to a floor of 0.
        loop {
            let current = self.inner.live.load(Ordering::Relaxed);
            if current == 0 {
                break;
            }
            if self
                .inner
                .live
                .compare_exchange_weak(current, current - 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                self.inner.drain_signal.notify_one();
                break;
            }
        }
    }
}
