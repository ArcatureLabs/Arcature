//! Email message builder over lettre, with attachments.
//!
//! [`Email`] is a thin `#[derive(Clone)]` wrapper around
//! [`lettre::message::MessageBuilder`]. It chains `from`/`to`/`cc`/`bcc`/
//! `subject` and terminates with a body method (`plain`/`html`/`alternative`/
//! `mixed`/`plain_with_attachments`/`alternative_with_attachments`) that
//! produces a [`lettre::Message`].

use std::fmt;

use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, Message, MessageBuilder, MultiPart, SinglePart};

use crate::mail::error::EmailError;
#[cfg(feature = "views")]
use crate::mail::error::MailViewError;

/// A thin wrapper around [`lettre::message::MessageBuilder`] for the
/// ergonomic email construction API.
#[derive(Clone)]
pub struct Email {
    builder: MessageBuilder,
}

impl Email {
    /// Start a new email builder.
    #[must_use]
    pub fn builder() -> Self {
        Self {
            builder: Message::builder(),
        }
    }

    /// Construct an `Email` from an existing `MessageBuilder` (escape hatch).
    #[must_use]
    pub fn from_builder(builder: MessageBuilder) -> Self {
        Self { builder }
    }

    /// Set the `From` address.
    #[must_use]
    pub fn from(mut self, mailbox: Mailbox) -> Self {
        self.builder = self.builder.from(mailbox);
        self
    }

    /// Set the `Reply-To` address.
    #[must_use]
    pub fn reply_to(mut self, mailbox: Mailbox) -> Self {
        self.builder = self.builder.reply_to(mailbox);
        self
    }

    /// Add a `To` recipient. May be called repeatedly.
    #[must_use]
    pub fn to(mut self, mailbox: Mailbox) -> Self {
        self.builder = self.builder.to(mailbox);
        self
    }

    /// Add a `Cc` recipient. May be called repeatedly.
    #[must_use]
    pub fn cc(mut self, mailbox: Mailbox) -> Self {
        self.builder = self.builder.cc(mailbox);
        self
    }

    /// Add a `Bcc` recipient. May be called repeatedly.
    #[must_use]
    pub fn bcc(mut self, mailbox: Mailbox) -> Self {
        self.builder = self.builder.bcc(mailbox);
        self
    }

    /// Set the `Subject`.
    #[must_use]
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.builder = self.builder.subject(subject);
        self
    }

    /// Terminate the builder with a plain-text body.
    ///
    /// # Errors
    ///
    /// Returns [`EmailError::Build`] if lettre cannot build the message.
    pub fn plain(self, body: impl Into<String>) -> Result<Message, EmailError> {
        let body: String = body.into();
        self.builder
            .header(ContentType::TEXT_PLAIN)
            .body(body)
            .map_err(EmailError::build)
    }

    /// Terminate the builder with an HTML body.
    ///
    /// # Errors
    ///
    /// Returns [`EmailError::Build`] if lettre cannot build the message.
    pub fn html(self, body: impl Into<String>) -> Result<Message, EmailError> {
        let html_part = SinglePart::builder()
            .header(ContentType::TEXT_HTML)
            .body(body.into());
        self.builder
            .singlepart(html_part)
            .map_err(EmailError::build)
    }

    /// Terminate the builder with an alternative plain/HTML body (the client
    /// picks whichever it can render).
    ///
    /// # Errors
    ///
    /// Returns [`EmailError::Build`] if lettre cannot build the message.
    pub fn alternative(
        self,
        plain: impl Into<String>,
        html: impl Into<String>,
    ) -> Result<Message, EmailError> {
        let multipart = MultiPart::alternative_plain_html(plain.into(), html.into());
        self.builder.multipart(multipart).map_err(EmailError::build)
    }

    /// Terminate the builder with a `multipart/mixed` body (a multipart body
    /// plus attachments).
    ///
    /// # Errors
    ///
    /// Returns [`EmailError::Build`] if lettre cannot build the message.
    pub fn mixed(
        self,
        body: MultiPart,
        attachments: Vec<EmailAttachment>,
    ) -> Result<Message, EmailError> {
        let mut mixed = MultiPart::mixed().multipart(body);
        for attachment in attachments {
            mixed = mixed.singlepart(attachment.into_lettre());
        }
        self.builder.multipart(mixed).map_err(EmailError::build)
    }

    /// Terminate the builder with a plain body plus attachments.
    ///
    /// # Errors
    ///
    /// Returns [`EmailError::Build`] if lettre cannot build the message, or
    /// [`EmailError::ContentType`] if an attachment content type is invalid.
    pub fn plain_with_attachments(
        self,
        body: impl Into<String>,
        attachments: Vec<EmailAttachment>,
    ) -> Result<Message, EmailError> {
        let body = MultiPart::alternative_plain_html(body.into(), String::new());
        self.mixed(body, attachments)
    }

    /// Terminate the builder with an alternative plain/HTML body plus
    /// attachments.
    ///
    /// # Errors
    ///
    /// Returns [`EmailError::Build`] if lettre cannot build the message.
    pub fn alternative_with_attachments(
        self,
        plain: impl Into<String>,
        html: impl Into<String>,
        attachments: Vec<EmailAttachment>,
    ) -> Result<Message, EmailError> {
        let body = MultiPart::alternative_plain_html(plain.into(), html.into());
        self.mixed(body, attachments)
    }

    /// Consume the wrapper and return the underlying `MessageBuilder` (escape
    /// hatch).
    #[must_use]
    pub fn into_builder(self) -> MessageBuilder {
        self.builder
    }
}

/// An email attachment: filename, body bytes, and MIME content type.
pub struct EmailAttachment {
    filename: String,
    body: Vec<u8>,
    content_type: ContentType,
}

impl EmailAttachment {
    /// Create an attachment.
    ///
    /// # Errors
    ///
    /// Returns [`EmailError::ContentType`] if `content_type` is not a valid
    /// MIME type string.
    pub fn new(
        filename: impl Into<String>,
        body: Vec<u8>,
        content_type: &str,
    ) -> Result<Self, EmailError> {
        let content_type = ContentType::parse(content_type).map_err(EmailError::content_type)?;
        Ok(Self {
            filename: filename.into(),
            body,
            content_type,
        })
    }

    pub(crate) fn into_lettre(self) -> SinglePart {
        Attachment::new(self.filename).body(self.body, self.content_type)
    }
}

impl fmt::Debug for EmailAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EmailAttachment")
            .field("filename", &self.filename)
            .field("content_type", &self.content_type)
            .field("body_len", &self.body.len())
            .finish_non_exhaustive()
    }
}

/// Terminators that render the message body from compiled templates.
///
/// # One pair of templates, both halves of one mail
///
/// A `multipart/alternative` mail carries the same message twice, and the two
/// copies drifting apart is the ordinary way mail templating goes wrong: the
/// HTML half gets the new wording and the plain half keeps the old, and only
/// the readers on the text client ever see it. These terminators take the two
/// templates together and render them in one call, so a change to a message
/// is a change to a pair.
///
/// The two halves are separate templates rather than one, because escaping is
/// chosen by the extension: the `.html` template escapes its values and the
/// `.txt` one does not. Rendering a text body through an HTML template would
/// send `&#38;` to someone reading plain text.
#[cfg(feature = "views")]
impl Email {
    /// Terminate the builder with an alternative plain/HTML body rendered
    /// from a pair of templates.
    ///
    /// The argument order matches [`Email::alternative`]: plain first.
    ///
    /// # Errors
    ///
    /// Returns [`MailViewError::Render`] if either template fails to render,
    /// or [`MailViewError::Build`] if lettre cannot build the message.
    ///
    /// ```
    /// use arcature::mail::Email;
    /// use arcature::view::Template;
    ///
    /// #[derive(Template)]
    /// #[template(
    ///     source = "Hello {{ name }}, your invoice is ready.",
    ///     ext = "txt",
    ///     askama = arcature::askama
    /// )]
    /// struct InvoiceText {
    ///     name: String,
    /// }
    ///
    /// #[derive(Template)]
    /// #[template(
    ///     source = "<p>Hello {{ name }}, your invoice is ready.</p>",
    ///     ext = "html",
    ///     askama = arcature::askama
    /// )]
    /// struct InvoiceHtml {
    ///     name: String,
    /// }
    ///
    /// let message = Email::builder()
    ///     .from("Billing <billing@example.com>".parse().unwrap())
    ///     .to("ada@example.com".parse().unwrap())
    ///     .subject("Your invoice")
    ///     .templated(
    ///         &InvoiceText { name: "Ada".into() },
    ///         &InvoiceHtml { name: "Ada".into() },
    ///     )
    ///     .unwrap();
    ///
    /// let raw = String::from_utf8(message.formatted()).unwrap();
    /// assert!(raw.contains("multipart/alternative"));
    /// ```
    pub fn templated<P, H>(self, plain: &P, html: &H) -> Result<Message, MailViewError>
    where
        P: crate::view::Template,
        H: crate::view::Template,
    {
        let (plain, html) = render_pair(plain, html)?;
        Ok(self.alternative(plain, html)?)
    }

    /// Terminate the builder with a template-rendered alternative body plus
    /// attachments.
    ///
    /// # Errors
    ///
    /// Returns [`MailViewError::Render`] if either template fails to render,
    /// or [`MailViewError::Build`] if lettre cannot build the message.
    pub fn templated_with_attachments<P, H>(
        self,
        plain: &P,
        html: &H,
        attachments: Vec<EmailAttachment>,
    ) -> Result<Message, MailViewError>
    where
        P: crate::view::Template,
        H: crate::view::Template,
    {
        let (plain, html) = render_pair(plain, html)?;
        Ok(self.alternative_with_attachments(plain, html, attachments)?)
    }
}

/// Render both halves, or fail before a half-built message exists.
#[cfg(feature = "views")]
fn render_pair<P, H>(plain: &P, html: &H) -> Result<(String, String), crate::view::ViewError>
where
    P: crate::view::Template,
    H: crate::view::Template,
{
    let plain = plain.render().map_err(crate::view::ViewError::from)?;
    let html = html.render().map_err(crate::view::ViewError::from)?;
    Ok((plain, html))
}

#[cfg(all(test, feature = "views"))]
mod template_tests {
    use super::*;
    use crate::view::Template;

    #[derive(Template)]
    #[template(source = "Hello {{ name }}, 3 < 4.", ext = "txt")]
    struct Text {
        name: &'static str,
    }

    #[derive(Template)]
    #[template(source = "<p>Hello {{ name }}.</p>", ext = "html")]
    struct Html {
        name: &'static str,
    }

    fn envelope() -> Email {
        Email::builder()
            .from("billing@example.com".parse().unwrap())
            .to("ada@example.com".parse().unwrap())
            .subject("Your invoice")
    }

    /// One call fills both halves, and each half is escaped according to its
    /// own extension: the HTML body turns `<` into an entity, the text body
    /// leaves it alone.
    #[test]
    fn one_template_pair_fills_both_halves() {
        let message = envelope()
            .templated(&Text { name: "Ada" }, &Html { name: "A<B" })
            .unwrap();
        let raw = String::from_utf8(message.formatted()).unwrap();

        assert!(raw.contains("multipart/alternative"), "{raw}");
        assert!(raw.contains("text/plain"), "{raw}");
        assert!(raw.contains("text/html"), "{raw}");
        assert!(
            raw.contains("Hello Ada, 3 < 4."),
            "text half missing: {raw}"
        );
        assert!(
            raw.contains("&#60;") || raw.contains("&lt;"),
            "the HTML half was not escaped: {raw}"
        );
        assert!(
            !raw.contains("<p>Hello A<B"),
            "the HTML half kept a raw angle bracket from data: {raw}"
        );
    }

    #[test]
    fn attachments_ride_along_with_a_templated_body() {
        let attachment =
            EmailAttachment::new("invoice.txt", b"total: 1".to_vec(), "text/plain").unwrap();
        let message = envelope()
            .templated_with_attachments(
                &Text { name: "Ada" },
                &Html { name: "Ada" },
                vec![attachment],
            )
            .unwrap();
        let raw = String::from_utf8(message.formatted()).unwrap();

        assert!(raw.contains("multipart/mixed"), "{raw}");
        assert!(raw.contains("invoice.txt"), "{raw}");
    }

    /// A template that cannot render stops before a message exists, and the
    /// framework error it converts into carries no template text.
    #[test]
    fn a_failing_template_never_becomes_a_message() {
        let failure = envelope()
            .templated(
                &crate::view::test_support::Unformattable::default(),
                &Html { name: "Ada" },
            )
            .unwrap_err();

        assert!(matches!(failure, MailViewError::Render { .. }));

        let framework = crate::Error::from(failure);
        assert_eq!(framework.status(), 500);
        let rendered = framework.to_string();
        assert!(
            !rendered.contains("secret-template-text"),
            "the template's text survived into the framework error: {rendered}"
        );
    }
}
