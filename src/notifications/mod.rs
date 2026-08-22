//! Multi-channel notifications.
//!
//! One event, told to one person, over whichever channels apply: an email, an
//! in-app row, a live push. A [`Notification`] renders itself per channel, a
//! [`Notifier`] delivers it, and a [`Recipient`] says who to.
//!
//! Three channels ship today: mail; behind the `notifications-db` feature, an
//! in-app inbox backed by the application's own database; and behind
//! `notifications-broadcast`, a live push to whoever is connected right now.
//! The trait is shaped so later channels arrive without touching a line of
//! application code.
//!
//! The inbox and the live push are complements rather than alternatives. The
//! push is what a recipient sees without reloading; the inbox is what they see
//! when they arrive. A recipient who was offline missed the push and lost
//! nothing, provided the inbox was written too -- which is why an application
//! enabling `notifications-broadcast` alone should know it is choosing
//! best-effort delivery.
//!
//! # The live push is per process, and targeted by construction
//!
//! [`crate::realtime`] offers a single flat fanout, which is the wrong shape
//! for something addressed to a person: everything subscribed to a channel
//! receives everything published to it. So the broadcast channel is not a
//! channel but a [`BroadcastChannels`] resolver, recipient key to channel.
//! Targeting is then which channel the bytes go into rather than a filter
//! applied afterwards, and one recipient's payload has no path into another's
//! connection. [`PerRecipientChannels`] is the built-in resolver.
//!
//! The fanout underneath is a `tokio::sync::broadcast`, so it reaches the
//! connections held by *this* process and no others. An application running
//! more than one instance -- or sending notifications from a background
//! worker, which is a different process from the one holding the socket --
//! should treat the push as an optimisation over the inbox rather than a
//! delivery guarantee. The same limit is disclosed for the rest of
//! [`crate::realtime`] in `README.md` and `docs/src/deployment.md`; it is
//! repeated here because a notification is exactly the case where it bites.
//!
//! # Mail can be deferred; the other two cannot
//!
//! Behind `notifications-queue`, [`Notifier::queue`] writes the email to
//! [`crate::jobs`] instead of waiting for the SMTP server, and the request
//! stops paying for a TLS handshake to a machine it does not control. The
//! inbox row and the live push still run inline in the same call.
//!
//! That asymmetry is not an omission. Deferring the inbox would mean the
//! recipient who opens the application right after the event finds nothing
//! there, which is the exact failure writing the inbox first was meant to
//! prevent. And the push reaches the connections held by *this* process; a
//! worker holds none of them, so a queued push is a dropped one.
//!
//! The queue is at-least-once, so a worker that dies after handing a message
//! to the SMTP server but before marking the job complete leaves a job that
//! runs again -- and the email arrives twice. That is the cost of the
//! deferral, and it is disclosed rather than designed away, because writing
//! to a remote server and recording that you did cannot be made one
//! operation.
//!
//! # The inbox cannot be read across recipients
//!
//! [`DatabaseNotifications`] takes the recipient key on *every* method,
//! including the ones that already have an id: marking a notification read and
//! deleting one both carry `notifiable_key` in their `WHERE` clause. That is
//! not belt and braces. There is no statement in the store a handler can reach
//! with an id alone, so reading or dismissing someone else's notification is
//! not a rule that a handler has to remember to apply -- it is a query that
//! does not exist.
//!
//! # What is different from Laravel
//!
//! Laravel's notifications name their channels in `via()` and render them in
//! `toMail`/`toDatabase`/`toBroadcast`. Two places, and nothing keeps them
//! agreeing: a channel in `via()` with no method behind it throws at runtime,
//! and a method `via()` forgot is simply never called.
//!
//! Here there is no `via`. A notification reaches a channel exactly when the
//! method for that channel returns `Some`, so the list *is* the methods. The
//! per-recipient decision `via($notifiable)` exists to make is still there --
//! every method takes the [`Recipient`] -- but it is made in the one place
//! that also produces the content.
//!
//! # Nothing is delivered quietly
//!
//! Two outcomes that a notification system can easily hide are made visible
//! here. Asking for a channel the [`Notifier`] was never given returns
//! [`NotificationError::NotConfigured`] instead of skipping it, so a
//! forgotten `.with_mail(..)` at startup fails on the first send rather than
//! becoming password-reset emails that never arrive. And a successful send
//! returns a [`Delivery`] naming the channels that ran, so "reached nobody"
//! is a thing the caller can ask about rather than a silence identical to
//! success.
//!
//! # Example
//!
//! ```
//! use arcature::mail::{Mail, Mailer};
//! use arcature::notifications::{
//!     Channel, MailContent, Notifiable, Notification, Notifier, Recipient,
//! };
//!
//! struct User {
//!     id: i64,
//!     email: String,
//! }
//!
//! impl Notifiable for User {
//!     fn recipient(&self) -> Recipient {
//!         Recipient::new(format!("user:{}", self.id)).email(&self.email)
//!     }
//! }
//!
//! struct InvoicePaid {
//!     amount_cents: i64,
//! }
//!
//! impl Notification for InvoicePaid {
//!     fn to_mail(&self, recipient: &Recipient) -> Option<MailContent> {
//!         recipient.email_address()?;
//!         Some(
//!             MailContent::new(
//!                 "Your invoice is paid",
//!                 format!("We received {}.{:02}.", self.amount_cents / 100, self.amount_cents % 100),
//!             )
//!             .html("<p>We received your payment.</p>"),
//!         )
//!     }
//! }
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let notifier = Notifier::new()
//!     .with_mail(Mail::new(Mailer::capture_ok(), "billing@acme.test".parse()?));
//!
//! let ada = User { id: 42, email: "ada@example.com".into() };
//! let delivery = notifier.send(&ada, &InvoicePaid { amount_cents: 1250 }).await?;
//!
//! assert!(delivery.reached(Channel::Mail));
//! # Ok(())
//! # }
//! ```

#[cfg(feature = "notifications-broadcast")]
mod broadcast;
mod channel;
#[cfg(feature = "notifications-db")]
mod dialect;
#[cfg(feature = "notifications-db")]
mod migrate;
mod notification;
mod notifier;
#[cfg(feature = "notifications-queue")]
mod queue;
mod recipient;
#[cfg(feature = "notifications-db")]
mod store;
#[cfg(feature = "notifications-db")]
mod stored;

#[cfg(feature = "notifications-broadcast")]
pub use broadcast::{BroadcastChannels, BroadcastNotifications, PerRecipientChannels};
pub use channel::{Channel, NotificationError};
#[cfg(feature = "notifications-db")]
pub use dialect::NotificationPool;
pub use notification::{BroadcastContent, DatabaseContent, MailContent, Notification};
pub use notifier::{Delivery, Notifier};
#[cfg(feature = "notifications-queue")]
pub use queue::{MAIL_JOB, NotificationQueue, QueuedMail, register_mail_handler};
pub use recipient::{Notifiable, Recipient};
#[cfg(feature = "notifications-db")]
pub use store::DatabaseNotifications;
#[cfg(feature = "notifications-db")]
pub use stored::{NotificationId, StoredNotification};
