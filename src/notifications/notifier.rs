//! The thing that actually delivers.

use std::fmt;

use crate::mail::{Email, EmailError, Mail, Mailable, lettre::Message};

#[cfg(feature = "notifications-broadcast")]
use super::broadcast::BroadcastNotifications;
use super::channel::{Channel, NotificationError};
use super::notification::{BroadcastContent, DatabaseContent, MailContent, Notification};
use super::recipient::Notifiable;
#[cfg(feature = "notifications-db")]
use super::store::DatabaseNotifications;

/// Which channels a notification actually reached.
///
/// Returned rather than discarded because "delivered to nothing" is a real
/// outcome and an invisible one: a notification whose `to_mail` returns
/// `None` for everybody is indistinguishable from a working one unless the
/// caller can ask.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct Delivery {
    channels: Vec<Channel>,
}

impl Delivery {
    /// The channels that delivered, in the order they were tried.
    #[must_use]
    pub fn channels(&self) -> &[Channel] {
        &self.channels
    }

    /// Whether a particular channel delivered.
    #[must_use]
    pub fn reached(&self, channel: Channel) -> bool {
        self.channels.contains(&channel)
    }

    /// Whether the notification reached nobody on any channel.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }
}

/// Delivers notifications over the channels it has been given.
///
/// A notifier holds one backing per channel and knows nothing about any
/// particular notification. It is cheap to clone and is meant to live in
/// application state.
///
/// # Failure is loud
///
/// If a notification renders content for a channel this notifier was not
/// built with, [`Notifier::send`] returns
/// [`NotificationError::NotConfigured`] rather than skipping it. Skipping
/// would turn a missing `.with_mail(..)` at startup into mail that silently
/// never arrives -- discovered, if ever, by a user who did not get their
/// password reset.
///
/// # Example
///
/// ```
/// use arcature::mail::{Mail, Mailer};
/// use arcature::notifications::{Channel, MailContent, Notification, Notifier, Recipient};
///
/// struct PasswordChanged;
///
/// impl Notification for PasswordChanged {
///     fn to_mail(&self, recipient: &Recipient) -> Option<MailContent> {
///         recipient.email_address()?;
///         Some(MailContent::new(
///             "Your password was changed",
///             "If this was not you, reset it immediately.",
///         ))
///     }
/// }
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// // `capture_ok` accepts every message and sends nothing: what a test wants.
/// let mailer = Mailer::capture_ok();
/// let notifier = Notifier::new()
///     .with_mail(Mail::new(mailer.clone(), "noreply@example.com".parse()?));
///
/// let ada = Recipient::new("user:42").email("ada@example.com");
/// let delivery = notifier.send(&ada, &PasswordChanged).await?;
///
/// assert!(delivery.reached(Channel::Mail));
/// assert_eq!(mailer.captured().await.unwrap().len(), 1);
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct Notifier {
    mail: Option<Mail>,
    #[cfg(feature = "notifications-db")]
    database: Option<DatabaseNotifications>,
    #[cfg(feature = "notifications-broadcast")]
    broadcast: Option<BroadcastNotifications>,
}

impl fmt::Debug for Notifier {
    /// Reports which channels are wired, not what is behind them: a
    /// `Mailer` holds SMTP credentials and a pool holds a database URL, and a
    /// `Debug` that printed either would put it in the first log line that
    /// formats application state.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = f.debug_struct("Notifier");
        out.field("mail", &self.mail.is_some());
        #[cfg(feature = "notifications-db")]
        out.field("database", &self.database.is_some());
        #[cfg(feature = "notifications-broadcast")]
        out.field("broadcast", &self.broadcast.is_some());
        out.finish()
    }
}

impl Notifier {
    /// A notifier with no channels. Every notification it is given will fail
    /// with [`NotificationError::NotConfigured`] until one is added.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable the mail channel.
    #[must_use]
    pub fn with_mail(mut self, mail: Mail) -> Self {
        self.mail = Some(mail);
        self
    }

    /// Whether the mail channel is wired.
    #[must_use]
    pub fn has_mail(&self) -> bool {
        self.mail.is_some()
    }

    /// Enable the in-app inbox channel.
    ///
    /// The store does not create its own table. Call
    /// [`DatabaseNotifications::migrate`] once at startup, or run the
    /// migration alongside the application's own.
    #[cfg(feature = "notifications-db")]
    #[must_use]
    pub fn with_database(mut self, database: DatabaseNotifications) -> Self {
        self.database = Some(database);
        self
    }

    /// Whether the in-app inbox channel is wired.
    #[cfg(feature = "notifications-db")]
    #[must_use]
    pub fn has_database(&self) -> bool {
        self.database.is_some()
    }

    /// Enable the live push channel.
    ///
    /// Wiring it says the application *can* push, not that anyone is
    /// listening. A recipient with no open connection is the ordinary case,
    /// and a push to them succeeds having reached nobody.
    #[cfg(feature = "notifications-broadcast")]
    #[must_use]
    pub fn with_broadcast(mut self, broadcast: BroadcastNotifications) -> Self {
        self.broadcast = Some(broadcast);
        self
    }

    /// Whether the live push channel is wired.
    #[cfg(feature = "notifications-broadcast")]
    #[must_use]
    pub fn has_broadcast(&self) -> bool {
        self.broadcast.is_some()
    }

    /// Render `notification` for `to` and deliver it on every channel it
    /// produced content for.
    ///
    /// # Errors
    ///
    /// - [`NotificationError::NotConfigured`] if the notification wants a
    ///   channel this notifier has no backing for.
    /// - [`NotificationError::NoAddress`] if it wants the mail channel for a
    ///   recipient with no email address.
    /// - [`NotificationError::Mail`] if the transport refuses the message.
    /// - [`NotificationError::Database`] if the inbox row cannot be written.
    /// - [`NotificationError::Encode`] if a broadcast payload cannot be
    ///   serialised.
    ///
    /// Delivery stops at the first failing channel, and the order the channels
    /// run in is therefore part of the contract: **inbox, then live push, then
    /// mail** -- the durable local record first, then the local push, then the
    /// one thing that leaves this process. The inbox cannot fail for a reason
    /// outside the application, so writing it first means an SMTP server that
    /// is down leaves the notification visible in the application rather than
    /// losing it along with the email. The reverse order would trade a
    /// recoverable failure for an unrecoverable one.
    ///
    /// The [`Delivery`] a successful call returns is the record of what did go
    /// out. [`Channel::Broadcast`] appears in it only when at least one
    /// connection received the push: a recipient who is not connected is not
    /// an error, and recording the channel as delivered when nobody was
    /// listening would make the record say something it does not know.
    pub async fn send<N>(
        &self,
        to: &impl Notifiable,
        notification: &N,
    ) -> Result<Delivery, NotificationError>
    where
        N: Notification + ?Sized,
    {
        let recipient = to.recipient();
        let mut channels = Vec::new();

        if let Some(content) = notification.to_database(&recipient) {
            self.deliver_database(recipient.key(), &content).await?;
            channels.push(Channel::Database);
        }

        if let Some(content) = notification.to_broadcast(&recipient)
            && self.deliver_broadcast(recipient.key(), &content)? > 0
        {
            channels.push(Channel::Broadcast);
        }

        if let Some(content) = notification.to_mail(&recipient) {
            let mail = self.mail.as_ref().ok_or(NotificationError::NotConfigured {
                channel: Channel::Mail,
            })?;
            let address =
                recipient
                    .email_address()
                    .ok_or_else(|| NotificationError::NoAddress {
                        key: recipient.key().to_owned(),
                    })?;

            mail.to(address).send(&AsMailable(&content)).await?;
            channels.push(Channel::Mail);
        }

        Ok(Delivery { channels })
    }

    /// Write one inbox row.
    #[cfg(feature = "notifications-db")]
    async fn deliver_database(
        &self,
        key: &str,
        content: &DatabaseContent,
    ) -> Result<(), NotificationError> {
        let database = self
            .database
            .as_ref()
            .ok_or(NotificationError::NotConfigured {
                channel: Channel::Database,
            })?;
        database.store(key, content).await?;
        Ok(())
    }

    /// Without the `notifications-db` feature there is no store to write to,
    /// so every attempt is the wiring error -- the same one a notifier built
    /// without `.with_database(..)` gives. A notification that renders inbox
    /// content in a build that cannot deliver it is a mistake either way, and
    /// it says so on the first send instead of on the day somebody notices the
    /// inbox has been empty.
    #[cfg(not(feature = "notifications-db"))]
    #[expect(
        clippy::unused_async,
        reason = "matches the feature-on signature, which awaits the database"
    )]
    async fn deliver_database(
        &self,
        key: &str,
        content: &DatabaseContent,
    ) -> Result<(), NotificationError> {
        let _ = (key, content);
        Err(NotificationError::NotConfigured {
            channel: Channel::Database,
        })
    }

    /// Push once, and report how many connections received it.
    ///
    /// Not `async`: a `tokio::sync::broadcast` send neither waits nor blocks,
    /// so awaiting here would only add a suspension point that never yields.
    #[cfg(feature = "notifications-broadcast")]
    fn deliver_broadcast(
        &self,
        key: &str,
        content: &BroadcastContent,
    ) -> Result<usize, NotificationError> {
        let broadcast = self
            .broadcast
            .as_ref()
            .ok_or(NotificationError::NotConfigured {
                channel: Channel::Broadcast,
            })?;
        broadcast.push(key, content)
    }

    /// Without the `notifications-broadcast` feature there is nothing to push
    /// through, so every attempt is the wiring error -- the same reasoning as
    /// [`deliver_database`](Self::deliver_database). Note that this is *not*
    /// the same case as a recipient with no open connection, which succeeds
    /// having reached nobody: that is a fact about the recipient, this is a
    /// fact about the build.
    #[cfg(not(feature = "notifications-broadcast"))]
    fn deliver_broadcast(
        &self,
        key: &str,
        content: &BroadcastContent,
    ) -> Result<usize, NotificationError> {
        let _ = (key, content);
        Err(NotificationError::NotConfigured {
            channel: Channel::Broadcast,
        })
    }
}

/// Adapts a [`MailContent`] to the [`Mailable`] the mail transport takes.
///
/// Going through `Mail::to(..).send(..)` rather than building a `Message`
/// here means address parsing, the `From` header, and the transport's own
/// error mapping stay in one place instead of being reimplemented with
/// slightly different edge cases.
struct AsMailable<'a>(&'a MailContent);

impl Mailable for AsMailable<'_> {
    fn build(&self, email: Email) -> Result<Message, EmailError> {
        let email = email.subject(self.0.subject());
        match self.0.html_body() {
            Some(html) => email.alternative(self.0.text(), html),
            None => email.plain(self.0.text()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::recipient::Recipient;
    use super::*;
    use crate::mail::Mailer;

    struct Mails;
    impl Notification for Mails {
        fn to_mail(&self, _recipient: &Recipient) -> Option<MailContent> {
            Some(MailContent::new("subject", "body"))
        }
    }

    /// Renders for both channels, so a test can tell which one ran first.
    struct Filed;
    impl Notification for Filed {
        fn to_mail(&self, _recipient: &Recipient) -> Option<MailContent> {
            Some(MailContent::new("subject", "body"))
        }

        fn to_database(&self, _recipient: &Recipient) -> Option<DatabaseContent> {
            Some(DatabaseContent::new("filed", serde_json::json!({})))
        }
    }

    /// Renders for the live push and for mail, so a test can tell whether a
    /// push that reached nobody stopped the mail.
    struct Pushed;
    impl Notification for Pushed {
        fn to_mail(&self, _recipient: &Recipient) -> Option<MailContent> {
            Some(MailContent::new("subject", "body"))
        }

        fn to_broadcast(&self, _recipient: &Recipient) -> Option<BroadcastContent> {
            Some(BroadcastContent::new("pushed", serde_json::json!({})))
        }
    }

    struct Silent;
    impl Notification for Silent {}

    fn wired() -> (Mailer, Notifier) {
        let mailer = Mailer::capture_ok();
        let mail = Mail::new(mailer.clone(), "noreply@example.com".parse().unwrap());
        (mailer, Notifier::new().with_mail(mail))
    }

    #[tokio::test]
    async fn a_mail_notification_reaches_the_transport() {
        let (mailer, notifier) = wired();
        let ada = Recipient::new("user:1").email("ada@example.com");

        let delivery = notifier.send(&ada, &Mails).await.unwrap();

        assert!(delivery.reached(Channel::Mail));
        assert_eq!(delivery.channels(), [Channel::Mail]);
        assert_eq!(mailer.captured().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_notification_with_no_content_delivers_nothing_and_says_so() {
        let (mailer, notifier) = wired();
        let ada = Recipient::new("user:1").email("ada@example.com");

        let delivery = notifier.send(&ada, &Silent).await.unwrap();

        assert!(delivery.is_empty());
        assert!(mailer.captured().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unconfigured_channel_is_an_error_and_not_a_skip() {
        // The whole point of the type: forgetting `.with_mail(..)` at startup
        // must not read as "sent successfully to zero channels".
        let ada = Recipient::new("user:1").email("ada@example.com");

        let error = Notifier::new().send(&ada, &Mails).await.unwrap_err();

        assert!(
            matches!(
                error,
                NotificationError::NotConfigured {
                    channel: Channel::Mail
                }
            ),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn wanting_mail_for_someone_with_no_address_is_an_error() {
        let (mailer, notifier) = wired();

        let error = notifier
            .send(&Recipient::new("user:7"), &Mails)
            .await
            .unwrap_err();

        match error {
            NotificationError::NoAddress { key } => assert_eq!(key, "user:7"),
            other => panic!("got {other:?}"),
        }
        assert!(mailer.captured().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_transport_failure_is_reported_rather_than_swallowed() {
        let mail = Mail::new(
            Mailer::capture_error(),
            "noreply@example.com".parse().unwrap(),
        );
        let notifier = Notifier::new().with_mail(mail);
        let ada = Recipient::new("user:1").email("ada@example.com");

        let error = notifier.send(&ada, &Mails).await.unwrap_err();

        assert!(
            matches!(error, NotificationError::Mail { .. }),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn an_invalid_recipient_address_does_not_panic() {
        // The address comes from the application's `Notifiable`, so it is not
        // attacker input -- but a typo in a column should surface as an error
        // on the send, not as a panic inside the transport.
        let (_mailer, notifier) = wired();
        let broken = Recipient::new("user:1").email("not an address");

        let error = notifier.send(&broken, &Mails).await.unwrap_err();

        assert!(
            matches!(error, NotificationError::Mail { .. }),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn an_inbox_notification_without_a_store_is_an_error_and_not_a_skip() {
        // The same guarantee the mail channel has, and it has to hold in both
        // builds: with the feature off there is no store to wire, and with it
        // on the notifier may simply have been built without one. Either way
        // the notification asked for an inbox row and did not get one, so the
        // send fails rather than reporting a delivery to nothing.
        let (mailer, notifier) = wired();
        let ada = Recipient::new("user:1").email("ada@example.com");

        let error = notifier.send(&ada, &Filed).await.unwrap_err();

        assert!(
            matches!(
                error,
                NotificationError::NotConfigured {
                    channel: Channel::Database
                }
            ),
            "got {error:?}"
        );
        // And it failed before the mail went out: the inbox is written first
        // so that a mail failure cannot cost the durable record.
        assert!(mailer.captured().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_broadcast_notification_without_a_channel_source_is_an_error_and_not_a_skip() {
        // Same guarantee again, and the same reason it has to hold in both
        // builds. Distinct from the case a wired notifier reports, where a
        // recipient who is simply not connected succeeds having reached
        // nobody -- that is a fact about the recipient, this is a mistake.
        let (mailer, notifier) = wired();
        let ada = Recipient::new("user:1").email("ada@example.com");

        let error = notifier.send(&ada, &Pushed).await.unwrap_err();

        assert!(
            matches!(
                error,
                NotificationError::NotConfigured {
                    channel: Channel::Broadcast
                }
            ),
            "got {error:?}"
        );
        assert!(mailer.captured().await.unwrap().is_empty());
    }

    #[cfg(feature = "notifications-broadcast")]
    #[tokio::test]
    async fn an_unconnected_recipient_is_not_a_delivery_and_not_a_failure() {
        use super::super::broadcast::{BroadcastNotifications, PerRecipientChannels};

        let (mailer, notifier) = wired();
        let channels = PerRecipientChannels::new(8).unwrap();
        let notifier = notifier.with_broadcast(BroadcastNotifications::new(channels.clone()));
        let ada = Recipient::new("user:1").email("ada@example.com");

        // Nobody connected: the mail still goes, the push reports nothing.
        let delivery = notifier.send(&ada, &Pushed).await.unwrap();
        assert_eq!(delivery.channels(), [Channel::Mail]);

        // Connected: the push is recorded, and before the mail.
        let _connection = channels.subscribe("user:1");
        let delivery = notifier.send(&ada, &Pushed).await.unwrap();
        assert_eq!(delivery.channels(), [Channel::Broadcast, Channel::Mail]);

        assert_eq!(mailer.captured().await.unwrap().len(), 2);
    }

    #[test]
    fn debug_does_not_print_the_mailer() {
        let (_mailer, notifier) = wired();
        let rendered = format!("{notifier:?}");

        // Built rather than written out, because the field list depends on
        // two independent features and a hand-written expectation per
        // combination is three chances to encode the wrong one.
        let mut fields = vec!["mail: true"];
        if cfg!(feature = "notifications-db") {
            fields.push("database: false");
        }
        if cfg!(feature = "notifications-broadcast") {
            fields.push("broadcast: false");
        }
        assert_eq!(rendered, format!("Notifier {{ {} }}", fields.join(", ")));

        // Whatever the feature set, what is printed is which channels are
        // wired -- never a credential from behind one.
        assert!(!rendered.contains("noreply@example.com"), "{rendered}");
    }

    #[test]
    fn a_fresh_notifier_has_no_channels() {
        assert!(!Notifier::new().has_mail());
        #[cfg(feature = "notifications-db")]
        assert!(!Notifier::new().has_database());
        #[cfg(feature = "notifications-broadcast")]
        assert!(!Notifier::new().has_broadcast());
        assert!(Delivery::default().is_empty());
    }
}
