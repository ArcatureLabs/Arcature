//! The application lifecycle state machine.
//!
//! A small atomic state machine (`Starting → Ready → Draining → Stopped`)
//! with readiness checks and drain hooks. The production server uses it to
//! keep `/up/live` liveness and `/up/ready` readiness endpoints accurate
//! during graceful shutdown: readiness goes false first, then the listener
//! stops, then drain hooks run, then the process exits.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// The lifecycle states, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LifecycleState {
    Starting = 0,
    Ready = 1,
    Draining = 2,
    Stopped = 3,
}

impl LifecycleState {
    #[must_use]
    fn from_u8(v: u8) -> Self {
        match v {
            1 => LifecycleState::Ready,
            2 => LifecycleState::Draining,
            3 => LifecycleState::Stopped,
            _ => LifecycleState::Starting,
        }
    }

    /// Whether the process is alive (not stopped).
    #[must_use]
    pub fn is_live(self) -> bool {
        !matches!(self, LifecycleState::Stopped)
    }
}

/// The lifecycle handle, cheaply cloneable (Arc-backed).
#[derive(Clone)]
pub struct Lifecycle {
    state: Arc<AtomicU8>,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl Lifecycle {
    /// A new lifecycle in the `Starting` state.
    #[must_use]
    pub fn new() -> Self {
        Lifecycle {
            state: Arc::new(AtomicU8::new(LifecycleState::Starting as u8)),
        }
    }

    /// The current state.
    #[must_use]
    pub fn state(&self) -> LifecycleState {
        LifecycleState::from_u8(self.state.load(Ordering::Acquire))
    }

    /// Whether the process is live (not stopped).
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.state().is_live()
    }

    /// Whether the process is ready to serve traffic.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.state() == LifecycleState::Ready
    }

    /// Transition to `Ready`. Only valid from `Starting`; no-op otherwise.
    pub fn mark_ready(&self) {
        let _ = self.state.compare_exchange(
            LifecycleState::Starting as u8,
            LifecycleState::Ready as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Begin draining. Idempotent: transitions `Starting`/`Ready` to
    /// `Draining`; no-op if already draining or stopped.
    pub fn begin_drain(&self) {
        loop {
            let current = self.state.load(Ordering::Acquire);
            if current == LifecycleState::Draining as u8 || current == LifecycleState::Stopped as u8
            {
                return;
            }
            if self
                .state
                .compare_exchange(
                    current,
                    LifecycleState::Draining as u8,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return;
            }
        }
    }

    /// Mark the process stopped.
    pub fn mark_stopped(&self) {
        self.state.store(LifecycleState::Stopped as u8, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_transitions() {
        let lc = Lifecycle::new();
        assert_eq!(lc.state(), LifecycleState::Starting);
        assert!(!lc.is_ready());
        assert!(lc.is_live());

        lc.mark_ready();
        assert_eq!(lc.state(), LifecycleState::Ready);
        assert!(lc.is_ready());

        lc.begin_drain();
        assert_eq!(lc.state(), LifecycleState::Draining);
        // idempotent
        lc.begin_drain();
        assert_eq!(lc.state(), LifecycleState::Draining);

        lc.mark_stopped();
        assert_eq!(lc.state(), LifecycleState::Stopped);
        assert!(!lc.is_live());
    }

    #[test]
    fn mark_ready_from_draining_is_noop() {
        let lc = Lifecycle::new();
        lc.begin_drain();
        lc.mark_ready();
        assert_eq!(lc.state(), LifecycleState::Draining);
    }
}
