//! The notification trait and the content each channel takes.

use super::recipient::Recipient;

/// Something worth telling someone about, renderable per channel.
///
/// # There is no `via`
///
/// Laravel's notifications declare their channels in a `via()` method and
/// render them in separate `toMail`/`toDatabase` methods, which means the
/// list and the methods can disagree: a channel named in `via()` with no
/// method behind it, or a method nobody calls because `via()` forgot it.
///
/// Here the channel set is not declared, it is *derived*: a notification goes
/// to the mail channel exactly when [`Notification::to_mail`] returns `Some`,
/// and to the in-app inbox exactly when [`Notification::to_database`] does.
/// There is no second place to keep in sync, so the two cannot drift.
///
/// Each `to_*` method receives the [`Recipient`], so a notification can still
/// decide per person -- returning `None` from `to_mail` for someone who has
/// asked not to be emailed is the same expression Laravel writes inside
/// `via()`.
///
/// # Adding a channel is additive
///
/// Every channel method has a default body returning `None`, so a
/// notification written today keeps compiling when a channel is added later;
/// it simply does not use it.
///
/// # Example
///
/// ```
/// use arcature::notifications::{MailContent, Notification, Recipient};
///
/// struct InvoicePaid {
///     amount_cents: i64,
/// }
///
/// impl Notification for InvoicePaid {
///     fn to_mail(&self, recipient: &Recipient) -> Option<MailContent> {
///         // No address, no mail -- and no error, because this notification
///         // is genuinely not a mail notification for this person.
///         recipient.email_address()?;
///
///         Some(MailContent::new(
///             "Your invoice is paid",
///             format!("We received {}.{:02}. Thank you!",
///                     self.amount_cents / 100, self.amount_cents % 100),
///         ))
///     }
/// }
///
/// let ada = Recipient::new("user:42").email("ada@example.com");
/// let content = InvoicePaid { amount_cents: 1250 }.to_mail(&ada).unwrap();
/// assert_eq!(content.subject(), "Your invoice is paid");
/// assert!(content.text().contains("12.50"));
///
/// // Same notification, someone with no address: not an error, just no mail.
/// assert!(InvoicePaid { amount_cents: 1250 }
///     .to_mail(&Recipient::new("user:43"))
///     .is_none());
/// ```
pub trait Notification: Send + Sync {
    /// Render this notification as an email, or `None` if it should not be
    /// emailed to this recipient.
    fn to_mail(&self, recipient: &Recipient) -> Option<MailContent> {
        let _ = recipient;
        None
    }

    /// Render this notification as a row in the recipient's in-app inbox, or
    /// `None` if it should not appear there.
    ///
    /// This method exists whatever features are on, for the reason given on
    /// [`Channel::Database`](super::Channel::Database): rendering costs
    /// nothing, and a notification whose inbox content is compiled out
    /// depending on a feature flag is a notification that silently changes
    /// what it does.
    fn to_database(&self, recipient: &Recipient) -> Option<DatabaseContent> {
        let _ = recipient;
        None
    }
}

/// The body of a notification email, independent of any mail library.
///
/// A notification describes what to say; the [`crate::mail`] transport turns
/// it into a MIME message. Keeping the two apart means a notification can be
/// rendered and asserted on in a test with no mailer, and -- once queued
/// notifications exist -- built in one place and sent in another.
///
/// # Plain text is not optional
///
/// [`MailContent::new`] takes the text body and [`MailContent::html`] adds
/// the HTML one, not the other way round. An HTML-only email is unreadable in
/// a text client, in a screen reader that falls back, and in the preview line
/// every mail app shows, and it is one of the older signals a spam filter
/// weighs. Making the readable body the mandatory argument costs a caller
/// nothing and removes the failure entirely.
///
/// # Example
///
/// ```
/// use arcature::notifications::MailContent;
///
/// let content = MailContent::new("Welcome", "Welcome to Acme, Ada.")
///     .html("<p>Welcome to Acme, <strong>Ada</strong>.</p>");
///
/// assert_eq!(content.subject(), "Welcome");
/// assert_eq!(content.text(), "Welcome to Acme, Ada.");
/// assert!(content.html_body().is_some());
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct MailContent {
    subject: String,
    text: String,
    html: Option<String>,
}

impl MailContent {
    /// An email with a subject and a plain-text body.
    #[must_use]
    pub fn new(subject: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            text: text.into(),
            html: None,
        }
    }

    /// Add an HTML body, sent alongside the text one as `multipart/
    /// alternative` so the reader's client picks whichever it can render.
    ///
    /// The HTML is used verbatim. Anything interpolated into it that came
    /// from a user must be escaped by the caller -- an email body is a
    /// perfectly good place to land a phishing link.
    #[must_use]
    pub fn html(mut self, html: impl Into<String>) -> Self {
        self.html = Some(html.into());
        self
    }

    /// The subject line.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// The plain-text body.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The HTML body, if one was set.
    #[must_use]
    pub fn html_body(&self) -> Option<&str> {
        self.html.as_deref()
    }
}

/// One row of an in-app inbox: a name for the shape, and the payload.
///
/// # Why the payload is a `Value` and not a generic
///
/// An inbox is heterogeneous. The rows one query returns were written by
/// different notifications with different fields, and a list that could hold
/// only one shape would not be an inbox. So the payload is JSON, and
/// [`kind`](Self::kind) is the name a reader matches on before deserialising
/// into whatever struct that kind means.
///
/// # Why `new` cannot fail and `serializing` can
///
/// [`Notification::to_database`] returns an `Option`, which has nowhere to put
/// a serialization error -- so if the constructor serialised, a struct that
/// failed to serialise would have to become `None`, and a notification would
/// vanish from the inbox because of a `#[serde(...)]` mistake. [`new`](Self::new)
/// therefore takes a [`serde_json::Value`] that is already built, which
/// `serde_json::json!` produces infallibly. [`serializing`](Self::serializing)
/// is there for the typed case and hands the error back, so the caller decides
/// what a failure means rather than having it decided as silence.
///
/// # Example
///
/// ```
/// use arcature::notifications::{DatabaseContent, Notification, Recipient};
///
/// struct InvoicePaid {
///     amount_cents: i64,
/// }
///
/// impl Notification for InvoicePaid {
///     fn to_database(&self, _recipient: &Recipient) -> Option<DatabaseContent> {
///         Some(DatabaseContent::new(
///             "invoice.paid",
///             serde_json::json!({ "amount_cents": self.amount_cents }),
///         ))
///     }
/// }
///
/// let content = InvoicePaid { amount_cents: 1250 }
///     .to_database(&Recipient::new("user:42"))
///     .unwrap();
/// assert_eq!(content.kind(), "invoice.paid");
/// assert_eq!(content.data()["amount_cents"], 1250);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct DatabaseContent {
    kind: String,
    data: serde_json::Value,
}

impl DatabaseContent {
    /// An inbox row with a kind and a payload.
    ///
    /// The kind is the application's own name for what this notification is
    /// -- `"invoice.paid"`, `"mention"`, whatever vocabulary the front end
    /// already switches on. It is deliberately not a Rust type path: the
    /// string is stored, so deriving it from a type name would turn a
    /// `refactor: rename` into a silent change to rows already written.
    #[must_use]
    pub fn new(kind: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            kind: kind.into(),
            data,
        }
    }

    /// An inbox row whose payload is serialised from a value.
    ///
    /// # Errors
    ///
    /// Returns the `serde_json` error if `data` cannot be serialised -- which
    /// for a derived `Serialize` means a map with non-string keys, or a
    /// custom implementation that failed.
    pub fn serializing<T: serde::Serialize>(
        kind: impl Into<String>,
        data: &T,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            kind: kind.into(),
            data: serde_json::to_value(data)?,
        })
    }

    /// The application's own name for what this notification is.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The payload.
    #[must_use]
    pub fn data(&self) -> &serde_json::Value {
        &self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Silent;
    impl Notification for Silent {}

    #[test]
    fn a_notification_that_implements_nothing_goes_nowhere() {
        // The default bodies are what make adding a channel additive, so the
        // empty impl has to keep compiling and keep returning nothing.
        let nobody = Recipient::new("user:1");
        assert!(Silent.to_mail(&nobody).is_none());
        assert!(Silent.to_database(&nobody).is_none());
    }

    #[test]
    fn database_content_keeps_the_kind_it_was_given() {
        let content = DatabaseContent::new("invoice.paid", serde_json::json!({ "n": 1 }));
        assert_eq!(content.kind(), "invoice.paid");
        assert_eq!(content.data()["n"], 1);
    }

    #[test]
    fn serializing_reports_a_failure_instead_of_turning_it_into_silence() {
        // The reason `new` and `serializing` are separate constructors: a
        // payload that cannot be serialised has to be an error the caller
        // sees, not a notification that never appears.
        #[derive(serde::Serialize)]
        struct Ok2 {
            n: i32,
        }
        let good = DatabaseContent::serializing("k", &Ok2 { n: 7 }).expect("plain struct");
        assert_eq!(good.data()["n"], 7);

        let bad: std::collections::BTreeMap<[u8; 2], i32> = std::iter::once(([1, 2], 3)).collect();
        assert!(DatabaseContent::serializing("k", &bad).is_err());
    }

    #[test]
    fn mail_content_has_no_html_until_it_is_given_one() {
        assert_eq!(MailContent::new("s", "t").html_body(), None);
    }

    #[test]
    fn the_last_html_body_wins() {
        let content = MailContent::new("s", "t").html("<p>a</p>").html("<p>b</p>");
        assert_eq!(content.html_body(), Some("<p>b</p>"));
    }
}
