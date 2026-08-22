//! Signed, expiring URLs.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use secrecy::{ExposeSecret, SecretSlice};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::base64url;
use super::key::{AppKey, URL_SIGNER_LABEL};
use crate::config::AppConfig;

/// The query parameter the signature is carried in.
const SIGNATURE_PARAM: &str = "signature";

/// The query parameter the expiry is carried in.
const EXPIRES_PARAM: &str = "expires";

/// The signature format's version tag.
///
/// It rides inside the parameter value rather than in a parameter of its own,
/// so a future format is a change to one string a verifier already parses. A
/// `v2` reader keeps the `v1` branch and every link already in somebody's
/// inbox keeps working.
const SIGNATURE_VERSION: &str = "v1";

/// The domain separator the MAC starts from. Distinct from the encrypter's,
/// so the two constructions cannot be confused even if a key ever were.
const MAC_DOMAIN: &[u8] = b"arcature/signed-url/v1";

/// The MAC's length in bytes: HMAC-SHA256, untruncated.
const MAC_BYTES: usize = 32;

/// RFC 3986's unreserved set. Everything outside it is percent-encoded, which
/// includes `&`, `=`, `+`, `%`, `#`, space and every non-ASCII byte -- so a
/// parameter value can never introduce a parameter.
///
/// Note that a space becomes `%20` and never `+`: `+` is an
/// `application/x-www-form-urlencoded` convention, not a URL one, and a
/// verifier that undid it would decode a literal `+` in a signed value into
/// something that was never signed.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// The source of "now" a [`UrlSigner`] measures expiry against.
///
/// This is a trait rather than a call to [`SystemTime::now`] for one reason:
/// a test for "the link stops working after an hour" that is written against
/// the real clock either sleeps for an hour or proves nothing. An injected
/// clock lets the same test move time forward instantly, which is why the
/// expiry tests in this crate assert on a value rather than on a delay.
///
/// ```
/// use arcature::crypt::Clock;
///
/// struct Frozen(u64);
/// impl Clock for Frozen {
///     fn now_unix(&self) -> u64 {
///         self.0
///     }
/// }
///
/// assert_eq!(Frozen(1_700_000_000).now_unix(), 1_700_000_000);
/// ```
pub trait Clock: Send + Sync + 'static {
    /// Seconds since the Unix epoch.
    fn now_unix(&self) -> u64;
}

/// The default [`Clock`]: the operating system's wall clock.
///
/// A time before the Unix epoch reads as `0`, which makes every expiry in the
/// past and every signed link invalid. That is the safe direction to fail: a
/// machine whose clock has fallen off the epoch should refuse links, not
/// honour them forever.
///
/// ```
/// use arcature::crypt::{Clock, SystemClock};
///
/// // Comfortably after 2020, unless the machine's clock is wrong.
/// assert!(SystemClock::new().now_unix() > 1_577_836_800);
/// ```
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct SystemClock;

impl SystemClock {
    /// The wall clock.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Clock for SystemClock {
    fn now_unix(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs())
    }
}

/// Mints and checks URLs that carry their own proof of origin, and optionally
/// their own deadline.
///
/// A signed URL is the answer to "let this one person fetch this one thing,
/// without giving them an account and without leaving the door open". The
/// link is self-contained: nothing is written down when it is issued and
/// nothing is looked up when it is presented, so it costs a MAC to make and a
/// MAC to check.
///
/// # What the signature covers
///
/// The path and **every** query parameter, including `expires`. Change any
/// byte of any of them and [`verify`](Self::verify) returns
/// [`SignedUrlError::Mismatch`]. In particular, moving the expiry forward is
/// not a way to extend a link -- an expiry outside the signature would make
/// the whole feature decorative, so it is inside it.
///
/// Parameters are canonicalised -- sorted, and compared after percent-decoding
/// -- before the MAC is computed. A link whose query has been reordered by a
/// mail client or a redirect still verifies; a link whose query has been
/// *edited* does not.
///
/// # What it does not cover
///
/// The origin is not in the MAC, because it does not have to be: `verify`
/// requires the URL to sit under the configured `APP_URL`, so a link lifted to
/// another host is rejected before a MAC is computed at all.
///
/// A signed URL is a **bearer token in a query string**. It will end up in
/// browser history, in `Referer` headers and in access logs. Give it the
/// shortest lifetime the use allows, and do not sign an action that a replay
/// would make worse.
///
/// # Key
///
/// The key is a subkey of [`AppKey`] under its own label -- not the
/// encrypter's key and not the session cookie key. See [`AppKey`].
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
///
/// use arcature::config::AppConfig;
/// use arcature::crypt::{AppKey, Clock, SignedUrlError, UrlSigner};
///
/// struct Frozen(u64);
/// impl Clock for Frozen {
///     fn now_unix(&self) -> u64 {
///         self.0
///     }
/// }
///
/// let key = AppKey::from_hex(&"4a".repeat(64))?;
/// let config = AppConfig::new().url("https://example.com");
/// let signer = UrlSigner::new(&key, &config).with_clock(Arc::new(Frozen(1_000)));
///
/// let url = signer.sign_temporary("/invoices/9", &[("as", "pdf")], Duration::from_secs(60))?;
/// assert!(url.starts_with("https://example.com/invoices/9?"));
/// assert_eq!(signer.verify(&url), Ok(()));
///
/// // The same link, one second past its deadline.
/// let later = UrlSigner::new(&key, &config).with_clock(Arc::new(Frozen(1_061)));
/// assert_eq!(later.verify(&url), Err(SignedUrlError::Expired));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[non_exhaustive]
pub struct UrlSigner {
    key: SecretSlice<u8>,
    base: String,
    clock: Arc<dyn Clock>,
}

impl UrlSigner {
    /// Build a signer over a subkey of `key`, rooted at the configured
    /// `APP_URL`.
    ///
    /// The base URL comes from [`AppConfig::base_url`] because a signed link is
    /// built with no request in scope -- it goes in an email, or in a job's
    /// output -- and behind a reverse proxy the `Host` header is not
    /// authoritative anyway. `APP_URL` is the only thing that knows where the
    /// application actually answers from.
    ///
    /// ```
    /// use arcature::config::AppConfig;
    /// use arcature::crypt::{AppKey, UrlSigner};
    ///
    /// let key = AppKey::from_hex(&"4a".repeat(64))?;
    /// let signer = UrlSigner::new(&key, &AppConfig::new().url("https://example.com/"));
    ///
    /// // The trailing slash on APP_URL is not doubled.
    /// let url = signer.sign("/files/report.csv", &[])?;
    /// assert!(url.starts_with("https://example.com/files/report.csv?"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new(key: &AppKey, config: &AppConfig) -> Self {
        Self {
            key: key.subkey(URL_SIGNER_LABEL),
            base: config.base_url().to_string(),
            clock: Arc::new(SystemClock),
        }
    }

    /// Replace the clock expiry is measured against. The default is
    /// [`SystemClock`].
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    ///
    /// use arcature::config::AppConfig;
    /// use arcature::crypt::{AppKey, Clock, UrlSigner};
    ///
    /// struct Frozen(u64);
    /// impl Clock for Frozen {
    ///     fn now_unix(&self) -> u64 {
    ///         self.0
    ///     }
    /// }
    ///
    /// let key = AppKey::from_hex(&"4a".repeat(64))?;
    /// let config = AppConfig::new().url("https://example.com");
    /// let signer = UrlSigner::new(&key, &config).with_clock(Arc::new(Frozen(500)));
    ///
    /// let url = signer.sign_temporary("/x", &[], Duration::from_secs(10))?;
    /// assert!(url.contains("expires=510"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Sign a URL that does not expire.
    ///
    /// Useful for a link whose validity is a property of the target rather
    /// than of time -- an unsubscribe link, say, which stops working when the
    /// subscription does. Prefer [`sign_temporary`](Self::sign_temporary)
    /// whenever a deadline makes sense: a permanent signed URL is a bearer
    /// token with no end date.
    ///
    /// # Errors
    ///
    /// [`SignedUrlError::ReservedParameter`] if a parameter is named
    /// `signature` or `expires`, and [`SignedUrlError::QueryInPath`] if `path`
    /// carries a `?` or a `#` -- pass the query as `params` instead, so it is
    /// canonicalised and signed rather than appended.
    ///
    /// ```
    /// use arcature::config::AppConfig;
    /// use arcature::crypt::{AppKey, SignedUrlError, UrlSigner};
    ///
    /// let key = AppKey::from_hex(&"4a".repeat(64))?;
    /// let signer = UrlSigner::new(&key, &AppConfig::new().url("https://example.com"));
    ///
    /// let url = signer.sign("/unsubscribe", &[("list", "weekly")])?;
    /// assert_eq!(signer.verify(&url), Ok(()));
    ///
    /// assert_eq!(
    ///     signer.sign("/x", &[("signature", "mine")]),
    ///     Err(SignedUrlError::ReservedParameter)
    /// );
    /// assert_eq!(
    ///     signer.sign("/x?a=1", &[]),
    ///     Err(SignedUrlError::QueryInPath)
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn sign(&self, path: &str, params: &[(&str, &str)]) -> Result<String, SignedUrlError> {
        self.build(path, params, None)
    }

    /// Sign a URL that stops verifying `valid_for` from now.
    ///
    /// "Now" is whatever the [`Clock`] says. The deadline is written into the
    /// `expires` query parameter as a Unix timestamp in seconds, and the
    /// signature covers it, so the link cannot be extended by editing it.
    ///
    /// A URL is valid up to and including its expiry second, and invalid from
    /// the next one.
    ///
    /// # Errors
    ///
    /// The same as [`sign`](Self::sign).
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    ///
    /// use arcature::config::AppConfig;
    /// use arcature::crypt::{AppKey, Clock, SignedUrlError, UrlSigner};
    ///
    /// struct Frozen(u64);
    /// impl Clock for Frozen {
    ///     fn now_unix(&self) -> u64 {
    ///         self.0
    ///     }
    /// }
    ///
    /// let key = AppKey::from_hex(&"4a".repeat(64))?;
    /// let config = AppConfig::new().url("https://example.com");
    /// let at = |second| UrlSigner::new(&key, &config).with_clock(Arc::new(Frozen(second)));
    ///
    /// let url = at(100).sign_temporary("/download/42", &[], Duration::from_secs(30))?;
    ///
    /// assert_eq!(at(130).verify(&url), Ok(()));
    /// assert_eq!(at(131).verify(&url), Err(SignedUrlError::Expired));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn sign_temporary(
        &self,
        path: &str,
        params: &[(&str, &str)],
        valid_for: Duration,
    ) -> Result<String, SignedUrlError> {
        let expires_at = self.clock.now_unix().saturating_add(valid_for.as_secs());
        self.build(path, params, Some(expires_at))
    }

    /// Check a URL this signer produced.
    ///
    /// `url` may be absolute (as [`sign`](Self::sign) returns it) or the
    /// path-and-query a request carries. An absolute URL that is not under the
    /// configured base is rejected without a MAC being computed, so a link
    /// lifted to another host never verifies -- including a host that merely
    /// starts with the right characters, like `https://example.com.evil/`.
    ///
    /// The signature is checked before the expiry, so a tampered URL reports
    /// tampering whatever its deadline says.
    ///
    /// # Errors
    ///
    /// See [`SignedUrlError`]. Every variant means the URL was **not**
    /// accepted; there is no partially-valid outcome.
    ///
    /// ```
    /// use arcature::config::AppConfig;
    /// use arcature::crypt::{AppKey, SignedUrlError, UrlSigner};
    ///
    /// let key = AppKey::from_hex(&"4a".repeat(64))?;
    /// let signer = UrlSigner::new(&key, &AppConfig::new().url("https://example.com"));
    ///
    /// let url = signer.sign("/reports/7", &[("format", "csv")])?;
    ///
    /// // Absolute, and the same thing as a bare path and query.
    /// assert_eq!(signer.verify(&url), Ok(()));
    /// let relative = url.trim_start_matches("https://example.com");
    /// assert_eq!(signer.verify(relative), Ok(()));
    ///
    /// // Another origin that shares a prefix with this one.
    /// let moved = url.replace("https://example.com", "https://example.com.evil");
    /// assert_eq!(signer.verify(&moved), Err(SignedUrlError::ForeignOrigin));
    ///
    /// // An edited parameter.
    /// let edited = url.replace("format=csv", "format=xls");
    /// assert_eq!(signer.verify(&edited), Err(SignedUrlError::Mismatch));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn verify(&self, url: &str) -> Result<(), SignedUrlError> {
        let Presented {
            path,
            params,
            signature,
        } = self.parse(url)?;

        let expected = self.signature_of(&path, &params);
        // `ConstantTimeEq`, never `==`. A byte-by-byte comparison that returns
        // at the first difference is a timing oracle: an attacker who can
        // measure it recovers a valid signature one byte at a time, which is
        // roughly 8,000 guesses instead of 2^256.
        if !bool::from(signature.as_slice().ct_eq(&expected)) {
            return Err(SignedUrlError::Mismatch);
        }

        // Only now: the expiry is authentic, because the check above proved
        // the whole query is.
        if let Some((_, value)) = params.iter().find(|(name, _)| name == EXPIRES_PARAM) {
            let expires_at: u64 = value.parse().map_err(|_| SignedUrlError::Malformed)?;
            if self.clock.now_unix() > expires_at {
                return Err(SignedUrlError::Expired);
            }
        }

        Ok(())
    }

    /// Assemble and sign a URL.
    fn build(
        &self,
        path: &str,
        params: &[(&str, &str)],
        expires_at: Option<u64>,
    ) -> Result<String, SignedUrlError> {
        if path.contains('?') || path.contains('#') {
            return Err(SignedUrlError::QueryInPath);
        }
        let path = canonical_path(path);

        let mut all = Vec::with_capacity(params.len() + 1);
        for (name, value) in params {
            if *name == SIGNATURE_PARAM || *name == EXPIRES_PARAM {
                return Err(SignedUrlError::ReservedParameter);
            }
            all.push(((*name).to_string(), (*value).to_string()));
        }
        if let Some(expires_at) = expires_at {
            all.push((EXPIRES_PARAM.to_string(), expires_at.to_string()));
        }

        let signature = self.signature_of(&path, &all);

        // Emit in the same canonical order the MAC used, so a URL this signer
        // produces twice is the same string twice.
        sort_canonically(&mut all);
        let mut url = format!("{}{path}?", self.base);
        for (name, value) in &all {
            url.push_str(&utf8_percent_encode(name, UNRESERVED).to_string());
            url.push('=');
            url.push_str(&utf8_percent_encode(value, UNRESERVED).to_string());
            url.push('&');
        }
        url.push_str(SIGNATURE_PARAM);
        url.push('=');
        url.push_str(SIGNATURE_VERSION);
        url.push('.');
        url.push_str(&base64url::encode(&signature));
        Ok(url)
    }

    /// Split a URL into the pieces the MAC is computed over, plus the
    /// signature it presented.
    fn parse(&self, url: &str) -> Result<Presented, SignedUrlError> {
        let rest = match url.strip_prefix(self.base.as_str()) {
            // The guard is the whole point: without it `https://example.com`
            // is a prefix of `https://example.com.evil/x`, and a link would
            // verify on a host the application does not own.
            Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') => rest,
            Some(_) => return Err(SignedUrlError::ForeignOrigin),
            None if url.starts_with('/') => url,
            None => return Err(SignedUrlError::ForeignOrigin),
        };
        // A fragment is never sent to a server, so it is not part of what was
        // signed and not part of what is checked.
        let rest = rest.split('#').next().unwrap_or("");
        let (raw_path, raw_query) = rest.split_once('?').unwrap_or((rest, ""));
        let path = canonical_path(raw_path);

        let mut params = Vec::new();
        let mut presented = None;
        for pair in raw_query.split('&').filter(|pair| !pair.is_empty()) {
            let (raw_name, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
            let name = decode_component(raw_name)?;
            let value = decode_component(raw_value)?;
            if name == SIGNATURE_PARAM {
                // Two signatures is an attempt to have a verifier read one and
                // a router read the other. There is no reading that is right.
                if presented.is_some() {
                    return Err(SignedUrlError::Malformed);
                }
                presented = Some(value);
                continue;
            }
            params.push((name, value));
        }

        let presented = presented.ok_or(SignedUrlError::MissingSignature)?;
        let body = presented
            .strip_prefix(SIGNATURE_VERSION)
            .and_then(|rest| rest.strip_prefix('.'))
            .ok_or(SignedUrlError::UnknownSignatureVersion)?;
        let signature = base64url::decode(body).ok_or(SignedUrlError::Malformed)?;

        Ok(Presented {
            path,
            params,
            signature,
        })
    }

    /// The MAC over the canonical form of `path` and `params`.
    ///
    /// Canonical means two things. Parameters are **sorted**, so a reordered
    /// query produces the same MAC and a link survives being rewritten. And
    /// every field is **length-prefixed**, so no two different inputs can
    /// serialise to the same bytes: without the prefixes, `a=bc` and `ab=c`
    /// would feed the MAC one identical string, and an attacker who can move a
    /// character across the boundary would have a forgery for free.
    fn signature_of(&self, path: &str, params: &[(String, String)]) -> [u8; MAC_BYTES] {
        let mut ordered: Vec<&(String, String)> = params.iter().collect();
        ordered.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));

        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.key.expose_secret())
            .expect("HMAC-SHA256 accepts a key of any length");
        mac.update(MAC_DOMAIN);
        feed(&mut mac, path.as_bytes());
        mac.update(&(ordered.len() as u64).to_be_bytes());
        for (name, value) in ordered {
            feed(&mut mac, name.as_bytes());
            feed(&mut mac, value.as_bytes());
        }

        let digest = mac.finalize().into_bytes();
        let mut signature = [0u8; MAC_BYTES];
        signature.copy_from_slice(&digest);
        signature
    }
}

impl fmt::Debug for UrlSigner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UrlSigner")
            .field("base", &self.base)
            .field("key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// A URL taken apart into the three things verification needs.
///
/// A named struct rather than a tuple because the two byte-strings in it --
/// a parameter value and a signature -- are exactly the pair a reader must
/// not mix up.
struct Presented {
    /// The path, with exactly one leading slash.
    path: String,
    /// Every query parameter except `signature`, percent-decoded, in the
    /// order the URL presented them.
    params: Vec<(String, String)>,
    /// The MAC the URL claims, already base64url-decoded.
    signature: Vec<u8>,
}

/// Feed one length-prefixed field into the MAC.
fn feed(mac: &mut Hmac<Sha256>, bytes: &[u8]) {
    mac.update(&(bytes.len() as u64).to_be_bytes());
    mac.update(bytes);
}

/// Sort parameters into the one order the MAC is defined over.
fn sort_canonically(params: &mut [(String, String)]) {
    params.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
}

/// Exactly one leading slash, so a caller writing `"files"` and a caller
/// writing `"/files"` sign the same thing.
fn canonical_path(path: &str) -> String {
    format!("/{}", path.trim_start_matches('/'))
}

/// Percent-decode one query component.
fn decode_component(raw: &str) -> Result<String, SignedUrlError> {
    percent_decode_str(raw)
        .decode_utf8()
        .map(std::borrow::Cow::into_owned)
        .map_err(|_| SignedUrlError::Malformed)
}

/// Why a URL could not be signed, or would not verify.
///
/// The first two are programming errors reported at signing time. The rest are
/// verification outcomes, and every one of them means the URL was rejected --
/// none is a "close enough".
///
/// The distinction between [`Mismatch`](Self::Mismatch) and
/// [`Expired`](Self::Expired) is worth logging: the first is somebody editing
/// a link, the second is somebody using a link too late, and an application
/// that treats them alike cannot tell an attack from a slow reader.
///
/// ```
/// use arcature::config::AppConfig;
/// use arcature::crypt::{AppKey, SignedUrlError, UrlSigner};
///
/// let key = AppKey::from_hex(&"4a".repeat(64))?;
/// let signer = UrlSigner::new(&key, &AppConfig::new().url("https://example.com"));
///
/// assert_eq!(
///     signer.verify("/unsigned"),
///     Err(SignedUrlError::MissingSignature)
/// );
/// assert!(
///     SignedUrlError::Expired
///         .to_string()
///         .contains("expired")
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignedUrlError {
    /// A parameter was named `signature` or `expires`. Those two belong to the
    /// signer; a caller that could set them could contradict it.
    ReservedParameter,
    /// The path carried a `?` or a `#`. Pass query parameters as parameters,
    /// so they are canonicalised and signed.
    QueryInPath,
    /// The URL is absolute and is not under the configured `APP_URL`.
    ForeignOrigin,
    /// There is no `signature` parameter.
    MissingSignature,
    /// The signature does not carry a version tag this build can read.
    UnknownSignatureVersion,
    /// The URL, or something inside it, is not well-formed: an unreadable
    /// percent-escape, a signature that is not base64url, or two signatures.
    Malformed,
    /// The signature does not match the URL. It was edited, or it was signed
    /// under a different key.
    Mismatch,
    /// The signature is genuine and the deadline has passed.
    Expired,
}

impl fmt::Display for SignedUrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ReservedParameter => {
                "`signature` and `expires` are reserved for the signer and cannot be set by a \
                 caller"
            }
            Self::QueryInPath => {
                "the path carries a query string or a fragment; pass query parameters separately \
                 so they can be signed"
            }
            Self::ForeignOrigin => "the URL is not under the application's configured APP_URL",
            Self::MissingSignature => "the URL carries no signature",
            Self::UnknownSignatureVersion => "the signature does not carry a known version tag",
            Self::Malformed => "the URL is not well-formed",
            Self::Mismatch => {
                "the signature does not match the URL; it was altered or was signed under a \
                 different key"
            }
            Self::Expired => "the signature is valid but the URL has expired",
        })
    }
}

impl std::error::Error for SignedUrlError {}

#[cfg(test)]
mod tests {
    use super::{Clock, SignedUrlError, UrlSigner};
    use crate::config::AppConfig;
    use crate::crypt::AppKey;
    use std::sync::Arc;
    use std::time::Duration;

    /// A clock that says whatever it was built with.
    struct Frozen(u64);

    impl Clock for Frozen {
        fn now_unix(&self) -> u64 {
            self.0
        }
    }

    fn config() -> AppConfig {
        AppConfig::new().url("https://example.com")
    }

    fn signer_at(second: u64) -> UrlSigner {
        let key = AppKey::from_bytes(&[0x4a; 64]).expect("64 bytes");
        UrlSigner::new(&key, &config()).with_clock(Arc::new(Frozen(second)))
    }

    #[test]
    fn a_signed_url_verifies() {
        let signer = signer_at(1_000);
        let url = signer
            .sign("/reports/7", &[("format", "csv")])
            .expect("sign");
        assert_eq!(signer.verify(&url), Ok(()));
    }

    #[test]
    fn a_signed_url_is_rooted_at_the_configured_base() {
        let signer = signer_at(1_000);
        let url = signer.sign("/reports/7", &[]).expect("sign");
        assert!(url.starts_with("https://example.com/reports/7?"), "{url}");
    }

    #[test]
    fn a_path_with_or_without_a_leading_slash_signs_the_same_thing() {
        let signer = signer_at(1_000);
        assert_eq!(
            signer.sign("reports/7", &[]).expect("sign"),
            signer.sign("/reports/7", &[]).expect("sign")
        );
    }

    #[test]
    fn the_relative_form_verifies_too() {
        let signer = signer_at(1_000);
        let url = signer.sign("/reports/7", &[("a", "1")]).expect("sign");
        let relative = url
            .strip_prefix("https://example.com")
            .expect("absolute form");
        assert_eq!(signer.verify(relative), Ok(()));
    }

    #[test]
    fn a_fragment_is_ignored() {
        let signer = signer_at(1_000);
        let url = signer.sign("/reports/7", &[]).expect("sign");
        assert_eq!(signer.verify(&format!("{url}#page-2")), Ok(()));
    }

    #[test]
    fn a_value_that_needs_escaping_round_trips() {
        let signer = signer_at(1_000);
        let url = signer
            .sign("/search", &[("q", "a&b=c d%e+f")])
            .expect("sign");
        assert!(!url.contains("a&b=c"), "the value must not split the query");
        assert_eq!(signer.verify(&url), Ok(()));
    }

    #[test]
    fn a_temporary_url_carries_its_deadline() {
        let url = signer_at(1_000)
            .sign_temporary("/x", &[], Duration::from_secs(60))
            .expect("sign");
        assert!(url.contains("expires=1060"), "{url}");
    }

    #[test]
    fn a_temporary_url_is_valid_up_to_and_including_its_deadline() {
        let url = signer_at(1_000)
            .sign_temporary("/x", &[], Duration::from_secs(60))
            .expect("sign");
        assert_eq!(signer_at(1_059).verify(&url), Ok(()));
        assert_eq!(signer_at(1_060).verify(&url), Ok(()));
        assert_eq!(signer_at(1_061).verify(&url), Err(SignedUrlError::Expired));
    }

    #[test]
    fn a_url_with_no_expiry_never_expires() {
        let url = signer_at(1_000).sign("/x", &[]).expect("sign");
        assert_eq!(signer_at(u64::MAX).verify(&url), Ok(()));
    }

    #[test]
    fn the_reserved_parameters_cannot_be_set_by_a_caller() {
        let signer = signer_at(1_000);
        assert_eq!(
            signer.sign("/x", &[("signature", "forged")]),
            Err(SignedUrlError::ReservedParameter)
        );
        assert_eq!(
            signer.sign("/x", &[("expires", "99999999999")]),
            Err(SignedUrlError::ReservedParameter)
        );
    }

    #[test]
    fn a_query_in_the_path_is_refused_rather_than_left_unsigned() {
        let signer = signer_at(1_000);
        assert_eq!(signer.sign("/x?a=1", &[]), Err(SignedUrlError::QueryInPath));
        assert_eq!(
            signer.sign("/x#frag", &[]),
            Err(SignedUrlError::QueryInPath)
        );
    }

    #[test]
    fn an_unsigned_url_is_refused() {
        assert_eq!(
            signer_at(1_000).verify("/x?a=1"),
            Err(SignedUrlError::MissingSignature)
        );
    }

    #[test]
    fn debug_never_shows_the_key() {
        let rendered = format!("{:?}", signer_at(1_000));
        assert!(rendered.contains("https://example.com"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }
}
