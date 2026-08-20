//! SMTP configuration with credential redaction.
//!
//! The connection URL may contain a password (`smtp://user:pass@host:port`).
//! It must never appear in `Debug`, `Display`, error output, or logs.

use std::fmt;
use std::time::Duration;

use lettre::transport::smtp::PoolConfig;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::Tls;
use lettre::transport::smtp::client::TlsParameters;
use lettre::transport::smtp::extension::ClientId;

use crate::mail::error::{MailConfigError, SmtpError};

/// The TLS mode for an SMTP connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TlsMode {
    /// Implicit TLS (port 465): the connection is TLS from the start.
    /// The default: the strictest mode, so an unconfigured transport does not
    /// silently fall back to plaintext.
    #[default]
    Implicit,
    /// STARTTLS required (port 587): upgrade to TLS after the greeting.
    Starttls,
    /// STARTTLS opportunistic: upgrade if the server advertises it.
    Opportunistic,
    /// Plain, no TLS (port 25): insecure, do not use for credentials.
    Plain,
}

impl TlsMode {
    /// The default port for this TLS mode.
    #[must_use]
    pub fn default_port(self) -> u16 {
        match self {
            Self::Implicit => 465,
            Self::Starttls | Self::Opportunistic => 587,
            Self::Plain => 25,
        }
    }

    /// Whether this TLS mode is encrypted.
    #[must_use]
    pub fn is_encrypted(self) -> bool {
        matches!(self, Self::Implicit | Self::Starttls)
    }
}

/// SMTP credentials (username + password). Secrets are never exposed in
/// `Debug`; there is deliberately **no `Display`** impl (a compile error
/// instead of a leak).
#[derive(Clone)]
pub struct SmtpCredentials {
    username: String,
    password: String,
}

impl SmtpCredentials {
    /// Create SMTP credentials.
    #[must_use]
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    pub(crate) fn to_lettre(&self) -> Credentials {
        Credentials::new(self.username.clone(), self.password.clone())
    }
}

impl fmt::Debug for SmtpCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SmtpCredentials")
            .finish_non_exhaustive()
    }
}

/// Resolved SMTP configuration.
///
/// Construct with [`SmtpConfig::new`] (host only) or
/// [`SmtpConfig::from_url`] (connection URL), then override settings with the
/// builder methods.
///
/// # Credential redaction
///
/// `SmtpConfig` implements `Debug` manually. It never exposes the password
/// or the full connection URL. Only the host, port, TLS mode, timeout, and a
/// boolean indicator of whether credentials are set appear in `Debug`.
#[derive(Clone)]
pub struct SmtpConfig {
    host: String,
    port: Option<u16>,
    tls_mode: TlsMode,
    credentials: Option<SmtpCredentials>,
    timeout: Option<Duration>,
    pool_config: PoolConfig,
    hello_name: ClientId,
}

impl SmtpConfig {
    /// Create SMTP configuration with the given host and secure defaults
    /// (implicit TLS, 60s timeout, default pool config).
    ///
    /// # Errors
    ///
    /// Returns [`MailConfigError::EmptyHost`] if `host` is empty or
    /// whitespace-only.
    pub fn new(host: impl Into<String>) -> Result<Self, MailConfigError> {
        let host = host.into();
        if host.trim().is_empty() {
            return Err(MailConfigError::empty_host());
        }
        Ok(Self {
            host,
            port: None,
            tls_mode: TlsMode::Implicit,
            credentials: None,
            timeout: Some(Duration::from_secs(60)),
            pool_config: PoolConfig::default(),
            hello_name: ClientId::default(),
        })
    }

    /// Parse a connection URL of the form
    /// `scheme://[user[:pass]@]host[:port][/ehlo-name][?tls=...]`.
    ///
    /// Scheme/tls-query -> [`TlsMode`]: `smtps`->Implicit,
    /// `smtp`+`?tls=required`->Starttls, `smtp`+`?tls=opportunistic`->
    /// Opportunistic, `smtp` (no tls) -> Plain. The userinfo is
    /// percent-decoded into [`SmtpCredentials`]. The raw URL is never stored.
    ///
    /// # Errors
    ///
    /// Returns [`MailConfigError::InvalidUrl`] if the URL cannot be parsed or
    /// has an unsupported scheme/tls combination.
    pub fn from_url(connection_url: &str) -> Result<Self, MailConfigError> {
        let url = url::Url::parse(connection_url)
            .map_err(|e| MailConfigError::invalid_url(e.to_string()))?;
        let tls_mode = match url.scheme() {
            "smtps" => TlsMode::Implicit,
            "smtp" => match url
                .query_pairs()
                .find(|(k, _)| k == "tls")
                .map(|(_, v)| v.to_string())
            {
                None => TlsMode::Plain,
                Some(v) if v == "required" => TlsMode::Starttls,
                Some(v) if v == "opportunistic" => TlsMode::Opportunistic,
                Some(v) => {
                    return Err(MailConfigError::invalid_url(format!(
                        "unsupported tls query value `{v}`"
                    )));
                }
            },
            other => {
                return Err(MailConfigError::invalid_url(format!(
                    "unsupported scheme `{other}` (use `smtp` or `smtps`)"
                )));
            }
        };
        let host = url
            .host_str()
            .ok_or_else(|| MailConfigError::invalid_url("missing host"))?
            .to_string();
        if host.trim().is_empty() {
            return Err(MailConfigError::empty_host());
        }
        let port = url.port();
        let credentials = match url.username() {
            "" => None,
            username => {
                let username = percent_encoding::percent_decode_str(username)
                    .decode_utf8_lossy()
                    .into_owned();
                let password = url.password().map(|p| {
                    percent_encoding::percent_decode_str(p)
                        .decode_utf8_lossy()
                        .into_owned()
                });
                Some(SmtpCredentials::new(username, password.unwrap_or_default()))
            }
        };
        // The URL path (after the host) is the EHLO name, if present.
        let hello_name = match url.path().trim_start_matches('/') {
            "" => ClientId::default(),
            domain => ClientId::Domain(domain.to_string()),
        };
        Ok(Self {
            host,
            port,
            tls_mode,
            credentials,
            timeout: Some(Duration::from_secs(60)),
            pool_config: PoolConfig::default(),
            hello_name,
        })
    }

    /// Override the port. Defaults to the [`TlsMode`]'s default port.
    #[must_use]
    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Override the TLS mode. Defaults to [`TlsMode::Implicit`].
    #[must_use]
    pub fn tls_mode(mut self, tls_mode: TlsMode) -> Self {
        self.tls_mode = tls_mode;
        self
    }

    /// Set credentials.
    #[must_use]
    pub fn credentials(mut self, credentials: SmtpCredentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Override the timeout. Defaults to 60 seconds.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Disable the timeout (no timeout).
    #[must_use]
    pub fn no_timeout(mut self) -> Self {
        self.timeout = None;
        self
    }

    /// Override the connection pool config.
    #[must_use]
    pub fn pool_config(mut self, pool_config: PoolConfig) -> Self {
        self.pool_config = pool_config;
        self
    }

    /// Override the EHLO/HELO name sent to the server. Defaults to the
    /// client's hostname (lettre's `ClientId::default()`).
    #[must_use]
    pub fn hello_name(mut self, hello_name: ClientId) -> Self {
        self.hello_name = hello_name;
        self
    }

    /// The configured host.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The configured port, if set explicitly.
    #[must_use]
    pub fn port_value(&self) -> Option<u16> {
        self.port
    }

    /// The configured TLS mode.
    #[must_use]
    pub fn tls_mode_value(&self) -> TlsMode {
        self.tls_mode
    }

    /// Whether credentials are configured.
    #[must_use]
    pub fn has_credentials(&self) -> bool {
        self.credentials.is_some()
    }

    /// The configured timeout, if set.
    #[must_use]
    pub fn timeout_value(&self) -> Option<Duration> {
        self.timeout
    }

    pub(crate) fn get_credentials(&self) -> Option<&SmtpCredentials> {
        self.credentials.as_ref()
    }

    pub(crate) fn get_pool_config(&self) -> &PoolConfig {
        &self.pool_config
    }

    pub(crate) fn get_hello_name(&self) -> &ClientId {
        &self.hello_name
    }

    /// Build the TLS parameters for this config's host. Used by
    /// [`crate::mail::Mailer::smtp`].
    pub(crate) fn build_tls_parameters(&self) -> Result<TlsParameters, SmtpError> {
        TlsParameters::new(self.host.clone())
    }

    /// The TLS enum lettre expects for this config's [`TlsMode`].
    pub(crate) fn tls_enum(&self, tls_parameters: TlsParameters) -> Tls {
        match self.tls_mode {
            TlsMode::Implicit => Tls::Wrapper(tls_parameters),
            TlsMode::Starttls => Tls::Required(tls_parameters),
            TlsMode::Opportunistic => Tls::Opportunistic(tls_parameters),
            TlsMode::Plain => Tls::None,
        }
    }
}

impl fmt::Debug for SmtpConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls_mode", &self.tls_mode)
            .field("timeout", &self.timeout)
            .field("has_credentials", &self.credentials.is_some())
            .field("hello_name", &self.hello_name)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for SmtpConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let port = self.port.unwrap_or_else(|| self.tls_mode.default_port());
        let tls_label = match self.tls_mode {
            TlsMode::Implicit => "implicit-tls",
            TlsMode::Starttls => "starttls",
            TlsMode::Opportunistic => "opportunistic-tls",
            TlsMode::Plain => "plain",
        };
        write!(formatter, "{}:{port} ({tls_label})", self.host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_empty_host() {
        assert!(matches!(
            SmtpConfig::new(""),
            Err(MailConfigError::EmptyHost)
        ));
        assert!(matches!(
            SmtpConfig::new("  "),
            Err(MailConfigError::EmptyHost)
        ));
    }

    #[test]
    fn new_defaults() {
        let config = SmtpConfig::new("smtp.example.com").unwrap();
        assert_eq!(config.host(), "smtp.example.com");
        assert_eq!(config.tls_mode_value(), TlsMode::Implicit);
        assert!(!config.has_credentials());
        assert_eq!(config.timeout_value(), Some(Duration::from_secs(60)));
    }

    #[test]
    fn from_url_smtps_implicit() {
        let config = SmtpConfig::from_url("smtps://smtp.example.com:465").unwrap();
        assert_eq!(config.host(), "smtp.example.com");
        assert_eq!(config.tls_mode_value(), TlsMode::Implicit);
        assert_eq!(config.port_value(), Some(465));
    }

    #[test]
    fn from_url_smtp_plain() {
        let config = SmtpConfig::from_url("smtp://mail.local:25").unwrap();
        assert_eq!(config.tls_mode_value(), TlsMode::Plain);
        assert_eq!(config.port_value(), Some(25));
    }

    #[test]
    fn from_url_smtp_starttls() {
        let config = SmtpConfig::from_url("smtp://mail.local:587?tls=required").unwrap();
        assert_eq!(config.tls_mode_value(), TlsMode::Starttls);
    }

    #[test]
    fn from_url_with_credentials() {
        let config =
            SmtpConfig::from_url("smtps://user%40domain:pa%40ss@smtp.example.com:465").unwrap();
        assert!(config.has_credentials());
        // The credentials themselves are redacted; we cannot read them back.
        let debug = format!("{config:?}");
        assert!(debug.contains("has_credentials: true"));
    }

    #[test]
    fn from_url_rejects_bad_scheme() {
        assert!(SmtpConfig::from_url("http://example.com").is_err());
    }

    #[test]
    fn debug_does_not_expose_password() {
        let config = SmtpConfig::new("smtp.example.com")
            .unwrap()
            .credentials(SmtpCredentials::new("user", "supersecret"));
        let debug = format!("{config:?}");
        assert!(!debug.contains("supersecret"), "debug leaked password");
        assert!(debug.contains("has_credentials: true"));
    }

    #[test]
    fn display_does_not_expose_credentials() {
        let config = SmtpConfig::new("smtp.example.com")
            .unwrap()
            .credentials(SmtpCredentials::new("user", "supersecret"));
        let display = format!("{config}");
        assert!(!display.contains("supersecret"));
        assert!(display.contains("smtp.example.com"));
    }
}
