//! Multi-channel notifications.
//!
//! One event, told to one person, over whichever channels apply: an email, an
//! in-app row, a live push. A [`Notification`] renders itself per channel, a
//! [`Notifier`] delivers it, and a [`Recipient`] says who to.
//!
//! Two channels ship today: mail, and -- behind the `notifications-db`
//! feature -- an in-app inbox backed by the application's own database. The
//! trait is shaped so later channels arrive without touching a line of
//! application code.
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

mod channel;
#[cfg(feature = "notifications-db")]
mod dialect;
#[cfg(feature = "notifications-db")]
mod migrate;
mod notification;
mod notifier;
mod recipient;
#[cfg(feature = "notifications-db")]
mod store;
#[cfg(feature = "notifications-db")]
mod stored;

pub use channel::{Channel, NotificationError};
#[cfg(feature = "notifications-db")]
pub use dialect::NotificationPool;
pub use notification::{DatabaseContent, MailContent, Notification};
pub use notifier::{Delivery, Notifier};
pub use recipient::{Notifiable, Recipient};
#[cfg(feature = "notifications-db")]
pub use store::DatabaseNotifications;
#[cfg(feature = "notifications-db")]
pub use stored::{NotificationId, StoredNotification};
