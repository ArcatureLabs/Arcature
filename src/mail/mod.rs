//! Mail subsystem: SMTP mail over lettre, with the `Mail::to(...).send(...)`
//! facade.
//!
//! This module owns the integration seam between an Arcature application and
//! an SMTP server: a [`Mailer`] transport handle (production SMTP or
//! in-memory capture), an [`Email`] message builder over lettre, a
//! [`Mailable`] trait for message-building types, and the [`Mail`] facade
//! with the `Mail::to(address).send(mailable)` builder.
//!
//! # What this module owns
//!
//! * A [`Mailer`] transport handle: [`Mailer::smtp`] (production, rustls +
//!   aws-lc-rs TLS) or [`Mailer::capture_ok`] / [`Mailer::capture_error`]
//!   (in-memory, for tests). `Clone + Send + Sync + 'static`.
//! * An [`Email`] message builder wrapping [`lettre::message::MessageBuilder`]
//!   with `from`/`to`/`cc`/`bcc`/`subject` chainers and `plain`/`html`/
//!   `alternative`/`plain_with_attachments`/`alternative_with_attachments` body
//!   terminators.
//! * An [`EmailAttachment`] type with a redacting `Debug` (never body bytes).
//! * A [`Mailable`] trait implemented by application message types.
//! * The [`Mail`] facade: `Mail::new(mailer, from)` then
//!   `mail.to(address).send(&mailable).await`.
//! * Resolved configuration: [`SmtpConfig`] (with [`SmtpCredentials`] and
//!   [`TlsMode`]) -- accepted explicitly, credentials redacted.
//!
//! # What this module does not own
//!
//! It does not reimplement SMTP, TLS, or cryptography. Lettre owns the SMTP
//! transport; the certified rustls + aws-lc-rs stack owns TLS; Tokio owns the
//! runtime.
//!
//! # Security note -- credentials are never logged
//!
//! [`SmtpCredentials`] has a `Debug` impl that prints nothing but the type
//! name (via `finish_non_exhaustive`) and deliberately has **no `Display`**
//! impl. [`SmtpConfig`] implements `Debug`/`Display` manually and never
//! exposes the password or the full connection URL.

pub mod config;
pub mod error;
pub mod message;
pub mod transport;

pub use config::{SmtpConfig, SmtpCredentials, TlsMode};
pub use error::{EmailError, MailConfigError, MailSendError};
pub use message::{Email, EmailAttachment};
pub use transport::{hello_name, parse_mailbox, Mail, MailBuilder, Mailer, Mailable};

// Re-export the certified lettre crate so downstream code targets the
// Arcature-pinned version.
pub use lettre;
pub use url;
