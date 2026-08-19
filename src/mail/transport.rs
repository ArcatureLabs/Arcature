//! The [`Mailer`] transport handle and the [`Mail`] facade with the
//! `Mail::to(...).send(...)` builder.
//!
//! [`Mailer`] is the production/capture transport handle: either an
//! [`lettre::AsyncSmtpTransport`] (production) or an
//! [`lettre::AsyncStubTransport`] (tests/capture). It is `Clone + Send + Sync +
//! 'static` and works as normal Axum state.
//!
//! [`Mail`] is the high-level send facade. `Mail::to(address).send(mailable)`
//! resolves a recipient, builds the message via a [`Mailable`] impl, and
//! dispatches it through a [`Mailer`].

use std::sync::Arc;

use lettre::message::Mailbox;
use lettre::transport::stub::AsyncStubTransport;
use lettre::transport::smtp::client::Tls;
use lettre::transport::smtp::client::TlsParameters;
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::mail::config::{SmtpConfig, TlsMode};
use crate::mail::error::{EmailError, MailConfigError, MailSendError};
use crate::mail::message::Email;

/// The mail transport handle: an SMTP or capture (stub) transport.
///
/// Construct with [`Mailer::smtp`] (production) or [`Mailer::capture_ok`] /
/// [`Mailer::capture_error`] (tests). `Clone + Send + Sync + 'static`.
#[derive(Clone)]
pub struct Mailer {
    inner: Arc<MailerInner>,
}

enum MailerInner {
    Smtp(AsyncSmtpTransport<Tokio1Executor>),
    Capture(AsyncStubTransport),
}

impl Mailer {
    /// Build an SMTP mailer from resolved configuration.
    ///
    /// # Errors
    ///
    /// Returns [`MailConfigError::TlsSetup`] if the TLS parameters cannot be
    /// constructed.
    pub fn smtp(config: SmtpConfig) -> Result<Self, MailConfigError> {
        let tls_parameters = config
            .build_tls_parameters()
            .map_err(MailConfigError::tls_setup)?;
        let tls = config.tls_enum(tls_parameters);
        let port = config
            .port_value()
            .unwrap_or_else(|| config.tls_mode_value().default_port());
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(config.host())
            .port(port)
            .tls(tls)
            .timeout(config.timeout_value())
            .hello_name(config.get_hello_name().clone())
            .pool_config(config.get_pool_config().clone());
        if let Some(credentials) = config.get_credentials() {
            builder = builder.credentials(credentials.to_lettre());
        }
        Ok(Self {
            inner: Arc::new(MailerInner::Smtp(builder.build())),
        })
    }

    /// Build a capture mailer that records every sent message in memory and
    /// always succeeds. For tests.
    #[must_use]
    pub fn capture_ok() -> Self {
        Self {
            inner: Arc::new(MailerInner::Capture(AsyncStubTransport::new_ok())),
        }
    }

    /// Build a capture mailer that always fails the send. For tests.
    #[must_use]
    pub fn capture_error() -> Self {
        Self {
            inner: Arc::new(MailerInner::Capture(AsyncStubTransport::new_error())),
        }
    }

    /// Send a message through the configured transport.
    ///
    /// # Errors
    ///
    /// Returns [`MailSendError`] if the transport rejects the message.
    pub async fn send(&self, message: &Message) -> Result<(), MailSendError> {
        match &*self.inner {
            MailerInner::Smtp(transport) => transport
                .send_raw(message.envelope(), &message.formatted())
                .await
                .map(|_| ())
                .map_err(MailSendError::smtp),
            MailerInner::Capture(transport) => transport
                .send_raw(message.envelope(), &message.formatted())
                .await
                .map(|_| ())
                .map_err(MailSendError::capture),
        }
    }

    /// Whether this mailer is a capture (stub) transport.
    #[must_use]
    pub fn is_capture(&self) -> bool {
        matches!(&*self.inner, MailerInner::Capture(_))
    }

    /// Whether this mailer is an SMTP transport.
    #[must_use]
    pub fn is_smtp(&self) -> bool {
        matches!(&*self.inner, MailerInner::Smtp(_))
    }

    /// The captured messages, if this is a capture mailer. Returns `None` for
    /// an SMTP mailer.
    pub async fn captured(&self) -> Option<Vec<(lettre::address::Envelope, String)>> {
        match &*self.inner {
            MailerInner::Capture(transport) => Some(transport.messages().await),
            MailerInner::Smtp(_) => None,
        }
    }

    /// Test the SMTP connection with a NOOP. Returns `Ok(false)` for a capture
    /// mailer.
    ///
    /// # Errors
    ///
    /// Returns [`MailSendError`] if the SMTP test fails.
    pub async fn test_connection(&self) -> Result<bool, MailSendError> {
        match &*self.inner {
            MailerInner::Smtp(transport) => transport
                .test_connection()
                .await
                .map_err(MailSendError::smtp),
            MailerInner::Capture(_) => Ok(false),
        }
    }

    /// Shut down the transport. No-op for a capture mailer.
    pub async fn shutdown(&self) {
        if let MailerInner::Smtp(transport) = &*self.inner {
            let _ = transport.shutdown().await;
        }
    }
}

/// A mailable: something that can be turned into an email message.
///
/// The application implements this for its mailable types (e.g. a
/// `WelcomeEmail` struct). The `build` method receives an [`Email`] builder
/// with the `From` and `To` already set, and returns the finished
/// [`Message`].
///
/// # Example
///
/// ```ignore
/// pub struct WelcomeEmail { pub name: String }
///
/// impl Mailable for WelcomeEmail {
///     fn build(&self, email: Email) -> Result<Message, EmailError> {
///         email
///             .subject(format!("Welcome, {}!", self.name))
///             .plain(format!("Welcome, {}!", self.name))
///     }
/// }
/// ```
pub trait Mailable: Send + Sync {
    /// Build the message, starting from the given [`Email`] builder (which
    /// already has `From` and `To` set by [`Mail::send`]).
    ///
    /// # Errors
    ///
    /// Returns [`crate::mail::EmailError`] if the message cannot be built.
    fn build(&self, email: Email) -> Result<Message, EmailError>;
}

/// The high-level mail facade: `Mail::to(address).send(mailable)`.
///
/// `Mail` wraps a [`Mailer`] and a `From` address. Call
/// [`Mail::to`]`(...)` to start a send, then `.send(mailable)` to build and
/// dispatch the message.
///
/// # Example
///
/// ```ignore
/// let mail = Mail::new(mailer, "noreply@example.com".parse()?);
/// mail.to(user.email).send(&WelcomeEmail { name: user.name }).await?;
/// ```
#[derive(Clone)]
pub struct Mail {
    mailer: Mailer,
    from: Mailbox,
}

impl Mail {
    /// Create a mail facade with the given mailer and `From` address.
    #[must_use]
    pub fn new(mailer: Mailer, from: Mailbox) -> Self {
        Self { mailer, from }
    }

    /// Create a mail facade with the given mailer and a `From` address parsed
    /// from a string.
    ///
    /// # Errors
    ///
    /// Returns [`lettre::address::AddressError`] if `from` is not a valid
    /// email address.
    pub fn from_str(mailer: Mailer, from: &str) -> Result<Self, lettre::address::AddressError> {
        Ok(Self::new(mailer, from.parse()?))
    }

    /// Start a send to the given recipient address. Returns a
    /// [`MailBuilder`] that completes the send on `.send(mailable)`.
    #[must_use]
    pub fn to(&self, address: impl Into<String>) -> MailBuilder<'_> {
        MailBuilder {
            mail: self,
            to: address.into(),
        }
    }

    /// The underlying mailer.
    #[must_use]
    pub fn mailer(&self) -> &Mailer {
        &self.mailer
    }
}

/// A builder for a single send operation, returned by [`Mail::to`].
pub struct MailBuilder<'a> {
    mail: &'a Mail,
    to: String,
}

impl<'a> MailBuilder<'a> {
    /// Build the message from the [`Mailable`] and send it.
    ///
    /// # Errors
    ///
    /// Returns [`MailSendError`] if the recipient address is invalid, the
    /// message cannot be built, or the transport rejects the message.
    pub async fn send<M: Mailable>(self, mailable: &M) -> Result<(), MailSendError> {
        let to_mailbox: Mailbox = self
            .to
            .parse()
            .map_err(|e: lettre::address::AddressError| {
                MailSendError::build(EmailError::address(e))
            })?;
        let email = Email::builder()
            .from(self.mail.from.clone())
            .to(to_mailbox);
        let message = mailable
            .build(email)
            .map_err(|e| MailSendError::build(e))?;
        self.mail.mailer.send(&message).await
    }
}

/// Parse a mailbox from a string. Convenience re-export of `mailbox.parse()`.
///
/// # Errors
///
/// Returns [`lettre::address::AddressError`] if the string is not a valid
/// email address or mailbox.
pub fn parse_mailbox(mailbox: &str) -> Result<Mailbox, lettre::address::AddressError> {
    mailbox.parse()
}

/// Build a lettre `ClientId` (EHLO/HELO name) from a domain string.
#[must_use]
pub fn hello_name(domain: impl Into<String>) -> ClientId {
    ClientId::Domain(domain.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::EmailError;

    #[test]
    fn capture_ok_is_capture() {
        let mailer = Mailer::capture_ok();
        assert!(mailer.is_capture());
        assert!(!mailer.is_smtp());
    }

    #[test]
    fn capture_records_messages() {
        let mailer = Mailer::capture_ok();
        let msg = Email::builder()
            .from("noreply@example.com".parse().unwrap())
            .to("to@example.com".parse().unwrap())
            .subject("test")
            .plain("hello")
            .expect("build");
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(mailer.send(&msg)).unwrap();
        let captured = rt.block_on(mailer.captured()).expect("capture");
        assert_eq!(captured.len(), 1);
    }

    struct WelcomeEmail {
        name: String,
    }

    impl Mailable for WelcomeEmail {
        fn build(&self, email: Email) -> Result<Message, EmailError> {
            email
                .subject(format!("Welcome, {}!", self.name))
                .plain(format!("Welcome, {}!", self.name))
        }
    }

    #[test]
    fn mail_facade_sends_via_capture() {
        let mailer = Mailer::capture_ok();
        let from: Mailbox = "noreply@example.com".parse().unwrap();
        let mail = Mail::new(mailer, from);
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(
            mail.to("user@example.com")
                .send(&WelcomeEmail { name: "Alice".into() }),
        )
        .expect("send");
        let captured = rt.block_on(mail.mailer().captured()).expect("capture");
        assert_eq!(captured.len(), 1);
        let body = &captured[0].1;
        assert!(body.contains("Welcome, Alice!"));
        assert!(body.contains("Subject: Welcome, Alice!"));
    }
}
