//! The broadcast channel: a live push to whoever is connected right now.
//!
//! This sits on top of [`crate::realtime`], which offers one thing: a
//! [`Broadcast`], a bounded fanout that every subscriber to it receives every
//! message from. That is the right primitive for a channel like "the build
//! status changed", and the wrong one for a notification, which is addressed
//! to a person. Publishing notifications onto a single shared `Broadcast`
//! would hand every connected user every other user's notifications.
//!
//! So the channel is not a `Broadcast`. It is a [`BroadcastChannels`]
//! resolver: given a recipient's key, hand back the `Broadcast` that
//! recipient's own connections are subscribed to, or `None`. Targeting is
//! then a property of which channel the bytes go into, not a filter applied
//! after the fact -- there is no code path that puts one recipient's payload
//! into another's channel, so there is no rule for a handler to remember.
//!
//! [`PerRecipientChannels`] is the built-in resolver and does the obvious
//! thing: one channel per recipient key, created when the first connection
//! subscribes. An application that already routes connections some other way
//! -- per team, per document, per tenant -- writes its own resolver instead.
//!
//! # Nobody connected is not a failure
//!
//! A recipient who is not looking at the application has no subscription, and
//! that is the ordinary case rather than an error. A push to a recipient with
//! no connections succeeds and reports that it reached nobody: the channel is
//! simply absent from the resulting
//! [`Delivery`](crate::notifications::Delivery). This is why the live push and
//! the in-app inbox are complements and not alternatives -- the push is what a
//! recipient sees without reloading, the inbox is what they see when they
//! arrive.
//!
//! # The fanout is per process
//!
//! [`Broadcast`] is a `tokio::sync::broadcast`, which reaches the connections
//! held by *this* process and no others. Two instances behind a load balancer
//! each push to their own connected subscribers, so a recipient connected to
//! instance A does not receive a notification sent from instance B.
//!
//! This is the same limit the rest of [`crate::realtime`] carries, disclosed
//! in `README.md` and `docs/src/deployment.md`, and it is stated again here
//! because a notification is exactly the kind of thing an application sends
//! from a background worker -- a different process from the one holding the
//! socket. Until a cross-process bridge exists, an application running more
//! than one instance should treat the live push as an optimisation over the
//! inbox rather than a delivery guarantee, and should enable
//! `notifications-db` alongside it.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use crate::realtime::{Broadcast, ChannelPayload, Subscription};

use super::channel::NotificationError;
use super::notification::BroadcastContent;

/// Resolves a recipient key to the channel that recipient's live connections
/// are subscribed to.
///
/// Implemented by [`PerRecipientChannels`]; implement it yourself when
/// connections are already grouped by something other than the recipient --
/// a tenant, a document, a team.
///
/// Returning `None` means "this recipient has nowhere to push right now",
/// which is not an error. Returning a channel nobody is subscribed to means
/// the same thing and is equally fine.
///
/// # Do not resolve two recipients to one channel
///
/// Whatever the grouping, the contract is that everything subscribed to the
/// returned channel is entitled to see this recipient's notifications. A
/// resolver that maps two people onto one channel to save an allocation has
/// turned a targeted notification into a leak, and nothing downstream can
/// detect it.
pub trait BroadcastChannels: Send + Sync + fmt::Debug {
    /// The channel this recipient's connections are subscribed to, if any.
    fn channel_for(&self, notifiable_key: &str) -> Option<Broadcast>;
}

impl<T: BroadcastChannels + ?Sized> BroadcastChannels for Arc<T> {
    fn channel_for(&self, notifiable_key: &str) -> Option<Broadcast> {
        (**self).channel_for(notifiable_key)
    }
}

/// One [`Broadcast`] per recipient key, created on first subscribe.
///
/// Cheap to clone: every clone shares the same map, so a handler holding one
/// and a notifier holding another are talking about the same connections.
///
/// # Example
///
/// ```
/// use arcature::notifications::PerRecipientChannels;
///
/// let channels = PerRecipientChannels::new(64).unwrap();
///
/// // A websocket handler subscribes the connection it just accepted.
/// let mut ada = channels.subscribe("user:1");
/// assert_eq!(channels.connections("user:1"), 1);
///
/// // Someone with no connection has no channel at all.
/// assert_eq!(channels.connections("user:2"), 0);
///
/// // Dropping the connection gives the entry back.
/// drop(ada);
/// assert_eq!(channels.connections("user:1"), 0);
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub struct PerRecipientChannels {
    capacity: usize,
    channels: Arc<Mutex<HashMap<String, Broadcast>>>,
}

impl PerRecipientChannels {
    /// A fresh set of channels, each created with `capacity` buffered
    /// messages.
    ///
    /// Returns `None` for a capacity of zero, matching
    /// [`Broadcast::new`] -- a channel that can hold nothing drops every
    /// message, and returning it as if it worked would be worse than saying
    /// no.
    ///
    /// The capacity is per recipient, and it bounds how far one connection
    /// may fall behind before it starts missing messages. It does not need to
    /// be large: a notification the recipient missed is still in the inbox if
    /// `notifications-db` is on, and a connection that is thousands of
    /// notifications behind has a problem that a bigger buffer postpones
    /// rather than solves.
    #[must_use]
    pub fn new(capacity: usize) -> Option<Self> {
        if capacity == 0 {
            return None;
        }
        Some(Self {
            capacity,
            channels: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// The per-recipient channel capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Subscribe one connection to a recipient's channel, creating the
    /// channel if this is the first.
    ///
    /// Dropping the returned [`Subscription`] releases the connection. The
    /// entry itself is reclaimed lazily, on a later call to this method.
    #[must_use]
    pub fn subscribe(&self, notifiable_key: &str) -> Subscription {
        let mut channels = self.lock();

        // Reclaim the entries nobody is subscribed to any more. Doing it here
        // rather than on drop of the `Subscription` keeps the drop path free
        // of a lock, and the map is keyed by application user ids: without a
        // sweep it grows once per person who has ever connected and never
        // shrinks, which for a long-running process is a slow leak rather
        // than a cache.
        channels.retain(|key, channel| channel.subscriber_count() > 0 || key == notifiable_key);

        let channel = channels
            .entry(notifiable_key.to_owned())
            .or_insert_with(|| {
                Broadcast::new(self.capacity)
                    .expect("capacity is non-zero, checked in PerRecipientChannels::new")
            })
            .clone();

        // Subscribing while the lock is held is what makes the count seen by
        // the sweep above trustworthy: a channel created here is never
        // observed at zero subscribers by another thread's sweep.
        channel.subscribe()
    }

    /// How many live connections this recipient has.
    #[must_use]
    pub fn connections(&self, notifiable_key: &str) -> usize {
        self.lock()
            .get(notifiable_key)
            .map_or(0, Broadcast::subscriber_count)
    }

    /// How many recipients currently hold an entry.
    ///
    /// Counts entries, not connections, and includes entries whose last
    /// connection has dropped but which have not been swept yet. Useful for a
    /// metric; not a count of who is online.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no recipient holds an entry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// The map, with a poisoned lock treated as merely locked.
    ///
    /// A panic while the map is borrowed cannot leave it half-updated -- the
    /// operations here are a single `retain`, `entry`, or `get` -- so the
    /// poison flag carries no information worth propagating, and honouring it
    /// would turn one unrelated panic into every later notification panicking.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Broadcast>> {
        self.channels.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for PerRecipientChannels {
    /// Reports the shape, not the keys: the keys are recipient identifiers,
    /// and a `Debug` that printed them would put a list of everyone currently
    /// online into the first log line that formats application state.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PerRecipientChannels")
            .field("capacity", &self.capacity)
            .field("recipients", &self.len())
            .finish()
    }
}

impl BroadcastChannels for PerRecipientChannels {
    fn channel_for(&self, notifiable_key: &str) -> Option<Broadcast> {
        // Deliberately does not create. A resolver that created a channel per
        // push would grow the map once per notification sent to someone who
        // is offline -- which is most of them -- and every one of those
        // channels would have no subscribers to sweep it away.
        self.lock().get(notifiable_key).cloned()
    }
}

/// The broadcast channel of a [`Notifier`](crate::notifications::Notifier).
///
/// Wraps a [`BroadcastChannels`] resolver and turns a
/// [`BroadcastContent`] into the bytes a subscriber receives.
///
/// # Example
///
/// ```
/// use arcature::notifications::{
///     BroadcastContent, BroadcastNotifications, PerRecipientChannels,
/// };
///
/// let channels = PerRecipientChannels::new(16).unwrap();
/// let broadcast = BroadcastNotifications::new(channels.clone());
///
/// // Nobody is connected, so the push reaches nobody -- and that is not an
/// // error.
/// let content = BroadcastContent::new("mention", serde_json::json!({}));
/// assert_eq!(broadcast.push("user:1", &content).unwrap(), 0);
///
/// // With a connection, it reaches it.
/// let subscription = channels.subscribe("user:1");
/// assert_eq!(broadcast.push("user:1", &content).unwrap(), 1);
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub struct BroadcastNotifications {
    channels: Arc<dyn BroadcastChannels>,
}

impl BroadcastNotifications {
    /// Push notifications through this resolver.
    #[must_use]
    pub fn new(channels: impl BroadcastChannels + 'static) -> Self {
        Self {
            channels: Arc::new(channels),
        }
    }

    /// Push one notification and report how many connections received it.
    ///
    /// `Ok(0)` means the recipient has no live connection, which is the
    /// ordinary state of someone not currently looking at the application.
    ///
    /// # Errors
    ///
    /// Returns [`NotificationError::Encode`] if the payload cannot be
    /// serialised. Nothing else is an error: a recipient with no channel, a
    /// channel with no subscribers, and a channel whose last subscriber
    /// dropped between the lookup and the send all report zero.
    pub fn push(
        &self,
        notifiable_key: &str,
        content: &BroadcastContent,
    ) -> Result<usize, NotificationError> {
        let Some(channel) = self.channels.channel_for(notifiable_key) else {
            return Ok(0);
        };

        let envelope = serde_json::json!({
            "kind": content.kind(),
            "data": content.data(),
        });
        let bytes = serde_json::to_vec(&envelope)
            .map_err(|error| NotificationError::Encode(error.to_string()))?;

        // What this method reports is a *count*, and every way a publish can
        // fail answers that count with zero. `Broadcast::publish` returns
        // `Closed` when there are no receivers left, which for a notification
        // is not a failure but the ordinary state of someone who is not
        // connected; and the channel underneath overwrites rather than
        // refuses when it is full, so there is no case where the payload was
        // rejected while somebody was listening.
        Ok(channel
            .publish(ChannelPayload::from_bytes(bytes))
            .unwrap_or(0))
    }
}

impl fmt::Debug for BroadcastNotifications {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BroadcastNotifications")
            .field("channels", &self.channels)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> BroadcastContent {
        BroadcastContent::new("mention", serde_json::json!({ "by": "ada" }))
    }

    #[test]
    fn a_capacity_of_zero_is_refused() {
        assert!(PerRecipientChannels::new(0).is_none());
        assert!(PerRecipientChannels::new(1).is_some());
    }

    #[test]
    fn a_recipient_with_no_connection_has_no_channel() {
        let channels = PerRecipientChannels::new(8).unwrap();
        assert!(channels.channel_for("user:1").is_none());
        assert_eq!(channels.connections("user:1"), 0);
        assert!(channels.is_empty());
    }

    #[test]
    fn subscribing_creates_the_channel_and_dropping_releases_it() {
        let channels = PerRecipientChannels::new(8).unwrap();

        let first = channels.subscribe("user:1");
        assert_eq!(channels.connections("user:1"), 1);
        assert_eq!(channels.len(), 1);

        let second = channels.subscribe("user:1");
        assert_eq!(channels.connections("user:1"), 2);
        assert_eq!(channels.len(), 1, "one entry, two connections");

        drop(first);
        assert_eq!(channels.connections("user:1"), 1);
        drop(second);
        assert_eq!(channels.connections("user:1"), 0);
    }

    #[test]
    fn a_disconnected_recipients_entry_is_swept() {
        let channels = PerRecipientChannels::new(8).unwrap();

        drop(channels.subscribe("user:1"));
        assert_eq!(channels.len(), 1, "the entry outlives the connection");

        // The sweep runs on the next subscribe, not on drop.
        let _ada = channels.subscribe("user:2");
        assert_eq!(channels.len(), 1);
        assert!(channels.channel_for("user:1").is_none());
    }

    #[test]
    fn the_sweep_never_takes_the_entry_being_subscribed_to() {
        let channels = PerRecipientChannels::new(8).unwrap();

        // Two rounds on the same key: the first leaves a zero-subscriber
        // entry, and the second must not remove the channel it is about to
        // hand out.
        drop(channels.subscribe("user:1"));
        let held = channels.subscribe("user:1");

        assert_eq!(channels.connections("user:1"), 1);
        drop(held);
    }

    #[test]
    fn a_push_to_nobody_reaches_nobody_and_is_not_an_error() {
        let channels = PerRecipientChannels::new(8).unwrap();
        let broadcast = BroadcastNotifications::new(channels);

        assert_eq!(broadcast.push("user:1", &content()).unwrap(), 0);
    }

    #[test]
    fn a_push_reaches_every_connection_of_that_recipient() {
        let channels = PerRecipientChannels::new(8).unwrap();
        let broadcast = BroadcastNotifications::new(channels.clone());

        let _one = channels.subscribe("user:1");
        let _two = channels.subscribe("user:1");

        assert_eq!(broadcast.push("user:1", &content()).unwrap(), 2);
    }

    #[tokio::test]
    async fn a_subscriber_receives_the_kind_and_the_data() {
        let channels = PerRecipientChannels::new(8).unwrap();
        let broadcast = BroadcastNotifications::new(channels.clone());

        let mut ada = channels.subscribe("user:1");
        broadcast.push("user:1", &content()).unwrap();

        let payload = ada.recv().await.unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(payload.as_bytes()).unwrap();
        assert_eq!(decoded["kind"], "mention");
        assert_eq!(decoded["data"]["by"], "ada");
    }

    #[tokio::test]
    async fn one_recipients_push_does_not_reach_another() {
        let channels = PerRecipientChannels::new(8).unwrap();
        let broadcast = BroadcastNotifications::new(channels.clone());

        let _ada = channels.subscribe("user:1");
        let mut grace = channels.subscribe("user:2");

        assert_eq!(broadcast.push("user:1", &content()).unwrap(), 1);

        // Not "grace received something harmless" -- grace received nothing.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), grace.recv())
                .await
                .is_err(),
            "a push addressed to user:1 reached user:2"
        );
    }

    #[test]
    fn debug_does_not_print_who_is_online() {
        let channels = PerRecipientChannels::new(8).unwrap();
        let _ada = channels.subscribe("user:secret-identifier");

        let rendered = format!("{channels:?}");
        assert!(!rendered.contains("secret-identifier"), "{rendered}");
        assert!(rendered.contains("capacity: 8"), "{rendered}");

        let broadcast = format!("{:?}", BroadcastNotifications::new(channels));
        assert!(!broadcast.contains("secret-identifier"), "{broadcast}");
    }

    #[test]
    fn a_custom_resolver_can_group_connections_however_it_likes() {
        /// Everyone in one team shares a channel, which is a legitimate
        /// grouping precisely because membership is the authorisation.
        #[derive(Debug)]
        struct TeamChannels {
            team: Broadcast,
        }

        impl BroadcastChannels for TeamChannels {
            fn channel_for(&self, notifiable_key: &str) -> Option<Broadcast> {
                notifiable_key
                    .starts_with("team:acme:")
                    .then(|| self.team.clone())
            }
        }

        let team = Broadcast::new(8).unwrap();
        let subscription = team.subscribe();
        let broadcast = BroadcastNotifications::new(TeamChannels { team });

        assert_eq!(broadcast.push("team:acme:ada", &content()).unwrap(), 1);
        assert_eq!(broadcast.push("team:other:grace", &content()).unwrap(), 0);
        drop(subscription);
    }
}
