//! Session cookie configuration and the 64-byte signing key.
//!
//! This is resolved configuration for the [tower-sessions] cookie attributes:
//! name, `SameSite`, `Secure`, `HttpOnly`, path, domain, `Max-Age`, and the
//! cookie signing key. Arcature does not own the session store; the
//! application wires any `tower_sessions::SessionStore`. The signing key is
//! held in a [`secrecy::SecretSlice`] and never appears in `Debug`.
//!
//! [tower-sessions]: https://docs.rs/tower-sessions

use std::fmt;
use std::time::Duration;

use secrecy::{ExposeSecret, SecretSlice};
use tower_sessions::cookie::Key;
use tower_sessions::{Expiry, SessionManagerLayer};

use crate::auth::{SessionBuildError, SessionConfigError, SigningKeyReason};

/// A signed-cookie session layer built from resolved [`SessionConfig`] and a
/// [`tower_sessions::SessionStore`].
///
/// This is `tower_sessions::SessionManagerLayer<Store,
/// tower_sessions::service::SignedCookie>` -- the upstream layer with the
/// `SignedCookie` controller. Apply it on an Axum router with `.layer(...)`.
pub type SessionLayer<Store> = SessionManagerLayer<Store, tower_sessions::service::SignedCookie>;

/// SameSite cookie attribute.
///
/// Defaults to [`SameSite::Strict`] for the strongest browser default. An
/// application doing third-party-initiated logins (e.g. OAuth callbacks across
/// a redirect) may need [`SameSite::Lax`]; never use
/// [`SameSite::None`](Self::None) without also enabling `Secure`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameSite {
    /// `SameSite=Strict` -- the cookie is not sent on cross-site requests.
    Strict,
    /// `SameSite=Lax` -- sent on top-level cross-site navigations (GET).
    Lax,
    /// `SameSite=None` -- sent on all cross-site requests; requires `Secure`.
    None,
}

impl SameSite {
    pub(crate) fn as_tower(&self) -> tower_sessions::cookie::SameSite {
        match self {
            Self::Strict => tower_sessions::cookie::SameSite::Strict,
            Self::Lax => tower_sessions::cookie::SameSite::Lax,
            Self::None => tower_sessions::cookie::SameSite::None,
        }
    }
}

/// Resolved session configuration.
///
/// Construct with [`SessionConfig::new`] (a signed-cookie layer), then pass
/// to [`SessionConfig::into_layer`] with a session store to build a
/// [`SessionLayer`] for Axum. Configuration is explicit and resolved; the
/// library never reads environment variables inside layer construction or
/// request handling.
///
/// # Signing key
///
/// The cookie signing key must be exactly 64 bytes, the master-key length
/// required by the certified `cookie` crate's signed jar. Use
/// [`SessionKey::generate`] to produce a cryptographically random key. The key
/// is held in a [`secrecy::SecretSlice`]; its `Debug` output never exposes the
/// bytes.
///
/// # Two lifetimes
///
/// A session has two independent expiry bounds:
///
/// - **Idle/inactivity** (`max_age`, [`Self::with_max_age`]) -- a sliding
///   window mapped to `tower_sessions::Expiry::OnInactivity`: each request
///   that saves the session resets it.
/// - **Absolute** (`absolute_max_age`, [`Self::with_absolute_max_age`]) -- the
///   maximum authenticated lifetime measured from the authentication timestamp
///   stored in the session at login, enforced at the auth boundary.
#[derive(Clone)]
pub struct SessionConfig {
    cookie_name: String,
    same_site: SameSite,
    secure: bool,
    http_only: bool,
    path: String,
    domain: Option<String>,
    max_age: Duration,
    absolute_max_age: Duration,
    signing_key: SecretSlice<u8>,
}

impl SessionConfig {
    /// Build session configuration with a signed-cookie key.
    ///
    /// `signing_key` must be exactly 64 bytes. The cookie attributes default
    /// to secure values: name `"__Host-id"`, `SameSite=Strict`, `Secure=true`,
    /// `HttpOnly=true`, path `"/"`, no domain, idle `Max-Age` 14 days, absolute
    /// lifetime 30 days. Override any with the `with_*` builder methods.
    ///
    /// # The `__Host-` prefix
    ///
    /// The default cookie name is `__Host-id`. A `__Host-` prefix mandates
    /// `Secure`, no `Domain`, and path `/` (RFC 6265bis), which the default
    /// attributes already satisfy, so the rename is strictly tighter. It
    /// defeats session-fixation/cookie-tossing from a sibling subdomain. For
    /// development over plain HTTP use [`SessionConfig::dev`] (a `__Host-`
    /// cookie is silently dropped by the browser when it is not `Secure`).
    ///
    /// # Errors
    ///
    /// Returns [`SessionConfigError::InvalidSigningKey`] if the key is not
    /// exactly 64 bytes.
    pub fn new(signing_key: &[u8]) -> Result<Self, SessionConfigError> {
        if signing_key.len() != 64 {
            return Err(SessionConfigError::InvalidSigningKey {
                reason: SigningKeyReason::WrongLength,
            });
        }
        Ok(Self {
            cookie_name: "__Host-id".to_string(),
            same_site: SameSite::Strict,
            secure: true,
            http_only: true,
            path: "/".to_string(),
            domain: None,
            max_age: Duration::from_secs(60 * 60 * 24 * 14),
            absolute_max_age: Duration::from_secs(60 * 60 * 24 * 30),
            signing_key: SecretSlice::from(signing_key.to_vec()),
        })
    }

    /// Build session configuration with the **development** defaults: cookie
    /// name `arcature-id` (no `__Host-` prefix), `SameSite=Strict`,
    /// `Secure = false`, `HttpOnly=true`, path `"/"`, no domain, idle
    /// `Max-Age` 14 days, absolute lifetime 30 days.
    ///
    /// A development server on plain HTTP cannot use the `__Host-` prefix
    /// (the browser drops a non-`Secure` `__Host-` cookie); this policy uses
    /// a plain cookie name so the session cookie reaches the browser over
    /// HTTP. Production keeps [`SessionConfig::new`] (`__Host-id`,
    /// `Secure = true`).
    ///
    /// # Errors
    ///
    /// Returns [`SessionConfigError::InvalidSigningKey`] if the key is not
    /// exactly 64 bytes.
    pub fn dev(signing_key: &[u8]) -> Result<Self, SessionConfigError> {
        if signing_key.len() != 64 {
            return Err(SessionConfigError::InvalidSigningKey {
                reason: SigningKeyReason::WrongLength,
            });
        }
        Ok(Self {
            cookie_name: "arcature-id".to_string(),
            same_site: SameSite::Strict,
            secure: false,
            http_only: true,
            path: "/".to_string(),
            domain: None,
            max_age: Duration::from_secs(60 * 60 * 24 * 14),
            absolute_max_age: Duration::from_secs(60 * 60 * 24 * 30),
            signing_key: SecretSlice::from(signing_key.to_vec()),
        })
    }

    /// Override the session cookie name.
    #[must_use]
    pub fn with_cookie_name(mut self, name: impl Into<String>) -> Self {
        self.cookie_name = name.into();
        self
    }

    /// Override the `SameSite` attribute. Default [`SameSite::Strict`].
    #[must_use]
    pub fn with_same_site(mut self, same_site: SameSite) -> Self {
        self.same_site = same_site;
        self
    }

    /// Override the `Secure` attribute (default `true`).
    #[must_use]
    pub fn with_secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    /// Override the `HttpOnly` attribute (default `true`).
    #[must_use]
    pub fn with_http_only(mut self, http_only: bool) -> Self {
        self.http_only = http_only;
        self
    }

    /// Override the cookie `Path` attribute (default `"/"`).
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// Override the cookie `Domain` attribute (default: none).
    #[must_use]
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Override the session **idle/inactivity** timeout. Default 14 days.
    ///
    /// This is a *sliding* window: it is mapped to
    /// `tower_sessions::Expiry::OnInactivity`, so each request that saves the
    /// session resets the clock. The maximum authenticated lifetime is a
    /// separate bound -- see [`Self::with_absolute_max_age`].
    #[must_use]
    pub fn with_max_age(mut self, max_age: Duration) -> Self {
        self.max_age = max_age;
        self
    }

    /// Override the **absolute** authenticated session lifetime. Default 30
    /// days.
    #[must_use]
    pub fn with_absolute_max_age(mut self, absolute_max_age: Duration) -> Self {
        self.absolute_max_age = absolute_max_age;
        self
    }

    /// The configured **absolute** authenticated session lifetime.
    #[must_use]
    pub fn absolute_max_age(&self) -> Duration {
        self.absolute_max_age
    }

    pub(crate) fn cookie_name(&self) -> &str {
        &self.cookie_name
    }

    pub(crate) fn same_site(&self) -> SameSite {
        self.same_site
    }

    pub(crate) fn secure(&self) -> bool {
        self.secure
    }

    pub(crate) fn http_only(&self) -> bool {
        self.http_only
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn domain(&self) -> Option<&str> {
        self.domain.as_deref()
    }

    pub(crate) fn max_age(&self) -> Duration {
        self.max_age
    }

    pub(crate) fn signing_key(&self) -> &[u8] {
        self.signing_key.expose_secret()
    }

    pub(crate) fn validate(&self) -> Result<(), SessionConfigError> {
        if self.cookie_name.is_empty() {
            return Err(SessionConfigError::EmptyCookieAttribute { attribute: "name" });
        }
        if self.path.is_empty() {
            return Err(SessionConfigError::EmptyCookieAttribute { attribute: "path" });
        }
        if self.max_age.is_zero() {
            return Err(SessionConfigError::ZeroDuration { field: "max_age" });
        }
        if self.absolute_max_age.is_zero() {
            return Err(SessionConfigError::ZeroDuration {
                field: "absolute_max_age",
            });
        }
        if self.signing_key().len() != 64 {
            return Err(SessionConfigError::InvalidSigningKey {
                reason: SigningKeyReason::WrongLength,
            });
        }
        // A __Host- prefixed cookie mandates Secure = true (RFC 6265bis); a
        // non-Secure __Host- cookie is silently dropped by the browser, so
        // the combination is invalid.
        if !self.secure && self.cookie_name.starts_with("__Host-") {
            return Err(SessionConfigError::InsecureHostPrefixedCookie {
                cookie_name: self.cookie_name.clone(),
            });
        }
        Ok(())
    }

    /// Build a [`SessionLayer`] over `store`. Validates the configuration
    /// before constructing the tower-sessions layer.
    ///
    /// # Errors
    ///
    /// Returns [`SessionBuildError`] if the configuration is internally
    /// inconsistent (empty name/path, zero max-age, wrong key length, a
    /// `__Host-` cookie combined with `Secure = false`).
    pub fn into_layer<Store>(self, store: Store) -> Result<SessionLayer<Store>, SessionBuildError>
    where
        Store: tower_sessions::SessionStore,
    {
        self.validate().map_err(SessionBuildError::new)?;
        Ok(assemble_layer(self, store))
    }
}

/// Manual `Debug` -- the signing key is never exposed.
impl fmt::Debug for SessionConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionConfig")
            .field("cookie_name", &self.cookie_name)
            .field("same_site", &self.same_site)
            .field("secure", &self.secure)
            .field("http_only", &self.http_only)
            .field("path", &self.path)
            .field("domain", &self.domain)
            .field("max_age_secs", &self.max_age.as_secs())
            .field("absolute_max_secs", &self.absolute_max_age.as_secs())
            .field("signing_key", &"<redacted 64-byte secret>")
            .finish()
    }
}

fn assemble_layer<Store: tower_sessions::SessionStore>(
    config: SessionConfig,
    store: Store,
) -> SessionLayer<Store> {
    let key = Key::from(config.signing_key());
    let max_age_secs: i64 = config.max_age().as_secs().try_into().unwrap_or(i64::MAX);
    let expiry = Expiry::OnInactivity(time::Duration::seconds(max_age_secs));
    let layer = SessionManagerLayer::new(store)
        .with_name(config.cookie_name().to_string())
        .with_same_site(config.same_site().as_tower())
        .with_secure(config.secure())
        .with_http_only(config.http_only())
        .with_path(config.path().to_string())
        .with_expiry(expiry)
        .with_signed(key);
    match config.domain() {
        Some(domain) => layer.with_domain(domain.to_string()),
        None => layer,
    }
}

/// A 64-byte session cookie signing key, zeroize-on-drop and redacted in
/// `Debug`.
///
/// This is the master key for the tower-cookies signed jar; it must be kept
/// secret and stable across requests for sessions to persist. Generate one
/// per deployment (or derive from a deployment secret) and pass it to
/// [`SessionConfig::new`].
#[derive(Clone)]
pub struct SessionKey {
    inner: SecretSlice<u8>,
}

impl SessionKey {
    /// Generate a 64-byte key from the certified `getrandom` OS RNG.
    ///
    /// # Errors
    ///
    /// Returns [`SessionConfigError::InvalidSigningKey`] only if the OS RNG
    /// fails.
    pub fn generate() -> Result<Self, SessionConfigError> {
        let mut bytes = vec![0u8; 64];
        getrandom::fill(&mut bytes).map_err(|_| SessionConfigError::InvalidSigningKey {
            reason: SigningKeyReason::WrongLength,
        })?;
        Ok(Self {
            inner: SecretSlice::from(bytes),
        })
    }

    /// Restore a key from 64 raw bytes.
    ///
    /// # Errors
    ///
    /// Returns [`SessionConfigError::InvalidSigningKey`] if the slice is not
    /// exactly 64 bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SessionConfigError> {
        if bytes.len() != 64 {
            return Err(SessionConfigError::InvalidSigningKey {
                reason: SigningKeyReason::WrongLength,
            });
        }
        Ok(Self {
            inner: SecretSlice::from(bytes.to_vec()),
        })
    }

    /// Expose the raw 64 bytes for constructing a `cookie::Key` or a
    /// [`SessionConfig`].
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.inner.expose_secret()
    }
}

impl fmt::Debug for SessionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SessionKey(<redacted 64-byte key>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_sessions_memory_store::MemoryStore;

    fn fresh_config() -> SessionConfig {
        SessionConfig::new(&[0u8; 64]).expect("valid key")
    }

    #[test]
    fn defaults_are_secure() {
        let config = fresh_config();
        assert_eq!(config.cookie_name(), "__Host-id");
        assert_eq!(config.same_site(), SameSite::Strict);
        assert!(config.secure());
        assert!(config.http_only());
        assert_eq!(config.path(), "/");
        assert!(config.domain().is_none());
        assert_eq!(config.max_age(), Duration::from_secs(60 * 60 * 24 * 14));
        assert_eq!(
            config.absolute_max_age(),
            Duration::from_secs(60 * 60 * 24 * 30)
        );
        assert!(config.absolute_max_age() > config.max_age());
    }

    #[test]
    fn with_absolute_max_age_overrides_default() {
        let config = fresh_config().with_absolute_max_age(Duration::from_secs(60));
        assert_eq!(config.absolute_max_age(), Duration::from_secs(60));
    }

    #[test]
    fn dev_defaults_are_for_plain_http() {
        let key = SessionKey::generate().expect("rng");
        let config = SessionConfig::dev(key.as_bytes()).expect("valid key");
        assert_eq!(config.cookie_name(), "arcature-id");
        assert!(!config.secure(), "dev Secure defaults to false");
        assert!(config.http_only());
        assert_eq!(config.path(), "/");
        assert!(config.domain().is_none());
    }

    #[test]
    fn debug_redacts_signing_key() {
        let config = SessionConfig::new(&[0xAB; 64]).expect("valid key");
        let debug = format!("{config:?}");
        assert!(debug.contains("<redacted"));
        assert!(!debug.contains("abab"), "hex key bytes must not leak: {debug}");
        assert!(!debug.contains("171"), "decimal key bytes must not leak: {debug}");
    }

    #[test]
    fn rejects_wrong_key_length() {
        assert!(matches!(
            SessionConfig::new(&[0u8; 32]),
            Err(SessionConfigError::InvalidSigningKey { .. })
        ));
    }

    #[test]
    fn rejects_empty_name() {
        let config = fresh_config().with_cookie_name("");
        assert!(config.into_layer(MemoryStore::default()).is_err());
    }

    #[test]
    fn rejects_zero_max_age() {
        let config = fresh_config().with_max_age(Duration::ZERO);
        assert!(config.into_layer(MemoryStore::default()).is_err());
    }

    #[test]
    fn rejects_zero_absolute_max_age() {
        let config = fresh_config().with_absolute_max_age(Duration::ZERO);
        assert!(config.into_layer(MemoryStore::default()).is_err());
    }

    #[test]
    fn production_cookie_name_is_host_prefixed() {
        let config = fresh_config();
        assert_eq!(config.cookie_name(), "__Host-id");
        assert!(config.cookie_name().starts_with("__Host-"));
        assert!(config.secure());
    }

    #[test]
    fn rejects_host_prefixed_cookie_with_secure_false() {
        let config = fresh_config().with_secure(false);
        let result = config.into_layer(MemoryStore::default());
        assert!(result.is_err(), "__Host-id + Secure=false must be rejected");
    }

    #[test]
    fn accepts_non_host_cookie_with_secure_false() {
        let config = fresh_config().with_cookie_name("sid").with_secure(false);
        assert!(config.into_layer(MemoryStore::default()).is_ok());
    }

    #[test]
    fn key_generate_produces_64_bytes() {
        let key = SessionKey::generate().expect("rng");
        assert_eq!(key.as_bytes().len(), 64);
    }

    #[test]
    fn key_from_bytes_rejects_wrong_length() {
        assert!(matches!(
            SessionKey::from_bytes(&[0u8; 32]),
            Err(SessionConfigError::InvalidSigningKey { .. })
        ));
        assert!(SessionKey::from_bytes(&[0u8; 64]).is_ok());
    }

    #[test]
    fn key_debug_redacts() {
        let key = SessionKey::from_bytes(&[0xf0; 64]).expect("64 bytes");
        let debug = format!("{key:?}");
        assert!(debug.contains("redacted"));
        assert!(!debug.contains("f0"), "Debug leaked individual key byte");
    }

    #[test]
    fn key_clone_preserves_bytes() {
        let key = SessionKey::generate().expect("rng");
        let clone = key.clone();
        assert_eq!(key.as_bytes(), clone.as_bytes());
    }
}
