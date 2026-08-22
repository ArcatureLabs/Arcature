//! Mail subsystem error types.
//!
//! No secret material is ever embedded in any variant. The connection URL
//! may contain a password but it is never stored in the error.

use std::fmt;

/// The lettre SMTP error type, re-exported for convenience.
pub type SmtpError = lettre::transport::smtp::Error;
/// The lettre stub transport error type, re-exported for convenience.
pub type StubError = lettre::transport::stub::Error;

/// Configuration validation failure for [`crate::mail::SmtpConfig`].
#[derive(Debug)]
pub enum MailConfigError {
    /// The connection URL could not be parsed.
    InvalidUrl { detail: String },
    /// The SMTP host is empty or whitespace-only.
    EmptyHost,
    /// The TLS parameters could not be constructed.
    TlsSetup { source: SmtpError },
}

impl MailConfigError {
    pub(crate) fn invalid_url(detail: impl Into<String>) -> Self {
        Self::InvalidUrl {
            detail: detail.into(),
        }
    }

    pub(crate) fn empty_host() -> Self {
        Self::EmptyHost
    }

    pub(crate) fn tls_setup(source: SmtpError) -> Self {
        Self::TlsSetup { source }
    }
}

impl fmt::Display for MailConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl { detail } => write!(formatter, "invalid SMTP URL: {detail}"),
            Self::EmptyHost => write!(formatter, "SMTP host must not be empty"),
            Self::TlsSetup { source } => write!(formatter, "TLS setup failed: {source}"),
        }
    }
}

impl std::error::Error for MailConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TlsSetup { source } => Some(source),
            Self::InvalidUrl { .. } | Self::EmptyHost => None,
        }
    }
}

/// Failure from sending a message via [`crate::mail::Mailer::send`].
#[derive(Debug)]
pub enum MailSendError {
    /// The SMTP transport returned an error.
    Smtp { source: SmtpError },
    /// The capture (stub) transport returned an error.
    Capture { source: StubError },
    /// The message could not be built (address parse or body build failure).
    Build { source: EmailError },
}

impl MailSendError {
    pub(crate) fn smtp(source: SmtpError) -> Self {
        Self::Smtp { source }
    }

    pub(crate) fn capture(source: StubError) -> Self {
        Self::Capture { source }
    }

    pub(crate) fn build(source: EmailError) -> Self {
        Self::Build { source }
    }

    /// Whether this error came from the SMTP transport.
    #[must_use]
    pub fn is_smtp(&self) -> bool {
        matches!(self, Self::Smtp { .. })
    }

    /// Whether this error came from the capture (stub) transport.
    #[must_use]
    pub fn is_capture(&self) -> bool {
        matches!(self, Self::Capture { .. })
    }
}

impl fmt::Display for MailSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Smtp { source } => write!(formatter, "SMTP send failed: {source}"),
            Self::Capture { source } => write!(formatter, "capture send failed: {source}"),
            Self::Build { source } => write!(formatter, "message build failed: {source}"),
        }
    }
}

impl std::error::Error for MailSendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Smtp { source } => Some(source),
            Self::Capture { source } => Some(source),
            Self::Build { source } => Some(source),
        }
    }
}

/// Failure from building an email message.
#[derive(Debug)]
pub enum EmailError {
    /// The message could not be built.
    Build { source: lettre::error::Error },
    /// A content type string could not be parsed.
    ContentType {
        source: lettre::message::header::ContentTypeErr,
    },
    /// A recipient or sender address could not be parsed.
    Address {
        source: lettre::address::AddressError,
    },
}

impl EmailError {
    pub(crate) fn build(source: lettre::error::Error) -> Self {
        Self::Build { source }
    }

    pub(crate) fn content_type(source: lettre::message::header::ContentTypeErr) -> Self {
        Self::ContentType { source }
    }

    pub(crate) fn address(source: lettre::address::AddressError) -> Self {
        Self::Address { source }
    }
}

impl fmt::Display for EmailError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Build { source } => write!(formatter, "email build failed: {source}"),
            Self::ContentType { source } => write!(formatter, "invalid content type: {source}"),
            Self::Address { source } => write!(formatter, "invalid email address: {source}"),
        }
    }
}

impl std::error::Error for EmailError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Build { source } => Some(source),
            Self::ContentType { source } => Some(source),
            Self::Address { source } => Some(source),
        }
    }
}

/// Failure from building an email whose bodies come from templates.
///
/// A separate type rather than a new [`EmailError`] variant: `EmailError` is
/// not `#[non_exhaustive]`, so growing it would break every downstream match.
#[cfg(feature = "views")]
#[derive(Debug)]
#[non_exhaustive]
pub enum MailViewError {
    /// A template failed to render.
    Render { source: crate::view::ViewError },
    /// The rendered halves could not be assembled into a message.
    Build { source: EmailError },
}

#[cfg(feature = "views")]
impl From<crate::view::ViewError> for MailViewError {
    fn from(source: crate::view::ViewError) -> Self {
        Self::Render { source }
    }
}

#[cfg(feature = "views")]
impl From<EmailError> for MailViewError {
    fn from(source: EmailError) -> Self {
        Self::Build { source }
    }
}

#[cfg(feature = "views")]
impl fmt::Display for MailViewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Render { source } => write!(formatter, "email template failed: {source}"),
            Self::Build { source } => write!(formatter, "email build failed: {source}"),
        }
    }
}

#[cfg(feature = "views")]
impl std::error::Error for MailViewError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Render { source } => Some(source),
            Self::Build { source } => Some(source),
        }
    }
}

#[cfg(feature = "views")]
impl From<MailViewError> for crate::Error {
    /// A render failure goes through `From<ViewError>`, which logs the askama
    /// message and returns a constant string; the template's text must not
    /// reach a response body by way of the mail path either. A build failure
    /// is lettre's and carries no template detail, so it is reported as the
    /// mail error it is.
    fn from(error: MailViewError) -> Self {
        match error {
            MailViewError::Render { source } => source.into(),
            MailViewError::Build { source } => crate::Error::Mail(source.to_string()),
        }
    }
}
