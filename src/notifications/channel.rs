//! The set of delivery channels, and what can go wrong on the way to one.

use std::fmt;

/// A way a notification can reach someone.
///
/// Marked `#[non_exhaustive]`: channels are added over time, and a `match`
/// that would silently stop compiling when one appears is a `match` that
/// would have silently dropped a notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum Channel {
    /// Email, through the [`crate::mail`] transport.
    Mail,

    /// An in-app inbox, one row per notification in the application's own
    /// database.
    ///
    /// The variant is here whatever features are on, and that is on purpose.
    /// Rendering a notification for this channel costs nothing but
    /// `serde_json`; it is *delivering* one that needs the `notifications-db`
    /// feature. Gating the variant too would mean an application that writes
    /// `to_database` without the feature fails to compile in a way that says
    /// nothing about the feature -- or worse, if the method were gated as
    /// well, compiles and silently sends nothing. As it stands the mistake
    /// surfaces as [`NotConfigured`](NotificationError::NotConfigured) on the
    /// first send, naming the channel that has no backing.
    Database,

    /// A live push to whoever is connected right now, over the
    /// [`crate::realtime`] machinery.
    ///
    /// Ungated for the same reason [`Database`](Self::Database) is.
    ///
    /// This channel appears in a [`Delivery`](crate::notifications::Delivery)
    /// only when at least one connection actually received the push. Nobody
    /// connected is not a failure -- it is the ordinary state of a recipient
    /// who is not looking at the application -- so it is reported as the
    /// channel simply not being among the ones that ran.
    Broadcast,
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Mail => "mail",
            Self::Database => "database",
            Self::Broadcast => "broadcast",
        };
        f.write_str(name)
    }
}

/// An error raised while delivering a notification.
///
/// Every variant is a mismatch between what a notification asked for and what
/// the application can do, and every one is reported rather than skipped. A
/// notification that quietly reaches nobody is the failure mode this type
/// exists to prevent: the sender believes it was told, the recipient never
/// hears, and nothing anywhere says so.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NotificationError {
    /// The notification produced content for a channel this
    /// [`crate::notifications::Notifier`] was never given the means to use.
    ///
    /// A wiring mistake, not a delivery failure: the notifier was built
    /// without the mailer (or, later, the pool or the broadcast) that this
    /// channel needs.
    #[error("notification: the {channel} channel is not configured on this notifier")]
    NotConfigured {
        /// The channel that has no backing.
        channel: Channel,
    },

    /// The notification asked for the mail channel, but the recipient carries
    /// no address to send to.
    ///
    /// A notification decides per recipient whether mail applies -- its
    /// `to_mail` receives the [`crate::notifications::Recipient`] and can
    /// return `None`. Returning content anyway for a recipient with no
    /// address is a contradiction, so it is an error rather than a skip.
    #[error("notification: the mail channel was requested for {key}, which has no email address")]
    NoAddress {
        /// The recipient's stable key, for finding it in the application.
        key: String,
    },

    /// The mail transport refused the message or could not deliver it.
    #[error("notification: {source}")]
    Mail {
        /// The transport error underneath.
        #[from]
        source: crate::mail::MailSendError,
    },

    /// The database rejected a statement, or was unreachable.
    #[cfg(feature = "notifications-db")]
    #[error("notification: {source}")]
    Database {
        /// The driver error underneath.
        #[from]
        source: sqlx::Error,
    },

    /// A stored notification did not hold what the schema promises.
    ///
    /// Reaching this means a row was written by something other than this
    /// store -- a hand-run `INSERT`, an older schema, another application
    /// sharing the table name.
    #[cfg(feature = "notifications-db")]
    #[error("notification: a stored notification could not be decoded: {0}")]
    Decode(String),

    /// A stored timestamp was not a time that can be represented.
    ///
    /// Only reachable on SQLite, which stores epoch milliseconds as a plain
    /// integer and so cannot reject a nonsensical one at write time.
    #[cfg(feature = "notifications-db")]
    #[error("notification: a stored timestamp is out of range: {0}")]
    Timestamp(String),

    /// [`DatabaseNotifications::store`](crate::notifications::DatabaseNotifications::store)
    /// drew this many random ids and found every one of them already taken.
    ///
    /// An id is 128 bits, so this does not happen by chance. It means the
    /// randomness source is returning something other than randomness, which
    /// is worth an error rather than a ninth attempt.
    #[cfg(feature = "notifications-db")]
    #[error("notification: {attempts} random ids were all taken")]
    IdCollision {
        /// How many ids were drawn.
        attempts: u32,
    },

    /// The OS randomness source was unavailable.
    ///
    /// No fallback is attempted. An id drawn from a clock is guessable, and
    /// the inbox's second line of defence rests on ids that are not.
    #[cfg(feature = "notifications-db")]
    #[error("notification: the OS randomness source is unavailable")]
    Entropy,

    /// A broadcast payload could not be serialised to the bytes a channel
    /// carries.
    ///
    /// Holds the message rather than the `serde_json::Error` so that the
    /// variant does not put `serde_json` in the public signature of a type
    /// that is otherwise reachable without it.
    #[cfg(feature = "notifications-broadcast")]
    #[error("notification: a broadcast payload could not be serialised: {0}")]
    Encode(String),
}
