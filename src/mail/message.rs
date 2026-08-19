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
