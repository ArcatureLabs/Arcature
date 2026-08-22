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
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Mail => "mail",
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
}
