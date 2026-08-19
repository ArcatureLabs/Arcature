//! The broadcast channel: a bounded `tokio::sync::broadcast` wrapper with
//! explicit subscriber counting.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::broadcast;

/// An error from a channel operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    /// The subscriber fell behind (missed messages).
    Lagged,
    /// The channel is closed (no more senders).
    Closed,
    /// The channel is full (bounded).
    Full,
}

impl std::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lagged => f.write_str("channel subscriber lagged"),
            Self::Closed => f.write_str("channel closed"),
            Self::Full => f.write_str("channel full"),
        }
    }
}

impl std::error::Error for ChannelError {}

/// An opaque payload published to a [`Broadcast`] channel. Owned bytes, so
/// no lifetime juggling across subscribers.
#[derive(Debug, Clone)]
pub struct ChannelPayload(Arc<[u8]>);

impl ChannelPayload {
    /// Wrap a `Vec<u8>` into a payload.
    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes.into())
    }

    /// Wrap a `&'static [u8]` into a payload (no allocation).
    #[must_use]
    pub fn from_static(bytes: &'static [u8]) -> Self {
        Self(bytes.into())
    }

    /// The payload bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A broadcast channel. Cheap to clone (the sender is `Arc`-backed). Dropping
/// the last `Broadcast` closes the channel (subscribers get `Closed`).
#[derive(Clone, Debug)]
pub struct Broadcast {
    tx: broadcast::Sender<ChannelPayload>,
    capacity: usize,
    subscriber_count: Arc<AtomicUsize>,
}

impl Broadcast {
    /// Create a broadcast channel with the given capacity. Returns `None` if
    /// the capacity is zero (never panics).
    #[must_use]
    pub fn new(capacity: usize) -> Option<Self> {
        if capacity == 0 {
            return None;
        }
        let (tx, _) = broadcast::channel(capacity);
        Some(Self {
            tx,
            capacity,
            subscriber_count: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// The configured capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The number of active subscribers.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.subscriber_count.load(Ordering::Relaxed)
    }

    /// Publish a payload. Returns the number of subscribers that received it
    /// (`Ok(0)` when there are no subscribers).
    pub fn publish(&self, payload: ChannelPayload) -> Result<usize, ChannelError> {
        match self.tx.send(payload) {
            Ok(n) => Ok(n),
            Err(_) => Err(ChannelError::Closed),
        }
    }

    /// Subscribe to the channel. The returned [`Subscription`] decrements the
    /// subscriber count on drop.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        self.subscriber_count.fetch_add(1, Ordering::Relaxed);
        Subscription {
            rx: self.tx.subscribe(),
            count: self.subscriber_count.clone(),
        }
    }
}

/// A subscription to a [`Broadcast`]. Dropping it decrements the subscriber
/// count.
#[derive(Debug)]
pub struct Subscription {
    /// The underlying broadcast receiver. Composable with `select!`.
    pub rx: broadcast::Receiver<ChannelPayload>,
    count: Arc<AtomicUsize>,
}

impl Subscription {
    /// Receive the next payload. Returns `Err(Closed)` when the channel is
    /// closed, `Err(Lagged)` when the subscriber fell behind.
    pub async fn recv(&mut self) -> Result<ChannelPayload, ChannelError> {
        self.rx.recv().await.map_err(|e| match e {
            broadcast::error::RecvError::Closed => ChannelError::Closed,
            broadcast::error::RecvError::Lagged(_) => ChannelError::Lagged,
        })
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // CAS loop down to a floor of 0.
        loop {
            let current = self.count.load(Ordering::Relaxed);
            if current == 0 {
                break;
            }
            if self
                .count
                .compare_exchange_weak(current, current - 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
    }
}
