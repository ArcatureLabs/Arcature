//! Authenticated encryption for application data.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use secrecy::ExposeSecret;

use super::base64url;
use super::key::{AppKey, ENCRYPTER_LABEL};

/// The token format's version tag, and the separator after it.
///
/// A token carries its own format so replacing the algorithm is an additive
/// change rather than a breaking one: a future `v2.` reader keeps the `v1.`
/// branch, existing tokens keep decrypting, and nothing in flight has to be
/// re-issued during a deploy. A format with no version is a format that can
/// only ever be changed by breaking every caller at once.
const VERSION: &str = "v1";

/// The nonce is 192 bits, which is the whole reason this is the X variant.
const NONCE_BYTES: usize = 24;

/// Poly1305 tag length. Used only to reject a token too short to hold one
/// before handing it to the AEAD.
const TAG_BYTES: usize = 16;

/// Associated data bound into every `v1` token.
///
/// It is not secret and it is not the message; it is a statement the tag
/// covers. Binding the version string means a `v1` token cannot be re-labelled
/// as a `v2` one, so a future version that weakens something cannot be reached
/// by editing four characters of an existing token.
const ASSOCIATED_DATA: &[u8] = b"arcature/crypt/v1";

/// Authenticated encryption for values an application has to hand out and get
/// back: a token in a link, an opaque cursor, a payload in a queue an operator
/// can read.
///
/// The algorithm is **XChaCha20-Poly1305**. Every message gets a fresh 192-bit
/// nonce from the OS RNG, which is wide enough that a random draw per message
/// is safe for any number of messages -- so there is no counter for a caller
/// to manage and no way for a caller to reuse one. The alternative, AES-GCM,
/// has a 96-bit nonce: random nonces there have a birthday bound a busy
/// application can reach, and a repeat does not just leak plaintext, it leaks
/// the authentication subkey and hands the attacker forgery as well.
///
/// # This is encryption, not signing
///
/// A token is confidential *and* authenticated. Nothing about the plaintext is
/// visible, and nothing that has been altered decrypts -- [`decrypt`] returns
/// an error rather than a partial or best-effort plaintext, so there is no
/// state in which a caller has attacker-influenced bytes and does not know it.
/// If you want the value to stay readable and only need it to be
/// tamper-evident, this is the wrong tool.
///
/// # Not a password store, and not a database column
///
/// Passwords go through `arcature::auth::PasswordHasher`: hashing is one-way
/// and encryption is not, and a stolen `APP_KEY` turns an encrypted password
/// table into a plaintext one. Encrypting a column you
/// then want to query is also a trap -- two encryptions of one value differ,
/// by design, so `WHERE email = ?` finds nothing.
///
/// # Key
///
/// The key is a subkey of [`AppKey`], derived under its own label, so it is
/// not the URL signing key and not the session cookie key. See [`AppKey`].
///
/// ```
/// use arcature::crypt::{AppKey, Encrypter};
///
/// let key = AppKey::from_hex(&"4a".repeat(64))?;
/// let encrypter = Encrypter::new(&key);
///
/// let token = encrypter.encrypt_string("order 4417")?;
/// assert!(token.starts_with("v1."));
/// assert_eq!(encrypter.decrypt_string(&token)?, "order 4417");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// [`decrypt`]: Encrypter::decrypt
#[non_exhaustive]
pub struct Encrypter {
    cipher: XChaCha20Poly1305,
}

impl Encrypter {
    /// Build an encrypter over a subkey of `key`.
    ///
    /// Cheap enough to do per request, though an application will normally
    /// build one at startup and keep it in state.
    ///
    /// ```
    /// use arcature::crypt::{AppKey, Encrypter};
    ///
    /// let key = AppKey::from_hex(&"4a".repeat(64))?;
    /// let encrypter = Encrypter::new(&key);
    /// assert_eq!(format!("{encrypter:?}"), "Encrypter(XChaCha20-Poly1305, <redacted key>)");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new(key: &AppKey) -> Self {
        let subkey = key.subkey(ENCRYPTER_LABEL);
        let material = Key::try_from(subkey.expose_secret())
            .expect("a derived subkey is exactly the 32 bytes XChaCha20 takes");
        Self {
            cipher: XChaCha20Poly1305::new(&material),
        }
    }

    /// Encrypt `plaintext` into a `v1.` token.
    ///
    /// The token is unpadded base64url after the version tag, so it is safe in
    /// a URL path, a query value, a cookie, and a JSON string with no further
    /// escaping.
    ///
    /// # Errors
    ///
    /// [`EncryptError::Rng`] if the operating system's random number generator
    /// fails -- and the reason that is an error rather than a fallback is that
    /// a nonce from anything but the OS RNG is not a nonce.
    /// [`EncryptError::Oversized`] if the plaintext is larger than the cipher
    /// can address, which no allocation on a 64-bit target reaches.
    ///
    /// ```
    /// use arcature::crypt::{AppKey, Encrypter};
    ///
    /// let encrypter = Encrypter::new(&AppKey::from_hex(&"4a".repeat(64))?);
    ///
    /// let token = encrypter.encrypt(b"\x00\x01\x02")?;
    /// assert_eq!(encrypter.decrypt(&token)?, b"\x00\x01\x02");
    ///
    /// // A fresh nonce per message: the same input never gives the same token.
    /// assert_ne!(encrypter.encrypt(b"same")?, encrypter.encrypt(b"same")?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<String, EncryptError> {
        let mut nonce = [0u8; NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|_| EncryptError::Rng)?;
        let xnonce =
            XNonce::try_from(&nonce[..]).expect("NONCE_BYTES is XChaCha20-Poly1305's nonce length");

        let payload = Payload {
            msg: plaintext,
            aad: ASSOCIATED_DATA,
        };
        let sealed = self
            .cipher
            .encrypt(&xnonce, payload)
            .map_err(|_| EncryptError::Oversized)?;

        let mut raw = Vec::with_capacity(NONCE_BYTES + sealed.len());
        raw.extend_from_slice(&nonce);
        raw.extend_from_slice(&sealed);
        Ok(format!("{VERSION}.{}", base64url::encode(&raw)))
    }

    /// Encrypt a string. Equivalent to [`encrypt`](Self::encrypt) over its
    /// bytes; [`decrypt_string`](Self::decrypt_string) is the other half.
    ///
    /// # Errors
    ///
    /// See [`encrypt`](Self::encrypt).
    ///
    /// ```
    /// use arcature::crypt::{AppKey, Encrypter};
    ///
    /// let encrypter = Encrypter::new(&AppKey::from_hex(&"4a".repeat(64))?);
    /// let token = encrypter.encrypt_string("caractère")?;
    /// assert_eq!(encrypter.decrypt_string(&token)?, "caractère");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn encrypt_string(&self, plaintext: &str) -> Result<String, EncryptError> {
        self.encrypt(plaintext.as_bytes())
    }

    /// Decrypt a token produced by [`encrypt`](Self::encrypt).
    ///
    /// # This fails closed
    ///
    /// A token whose bytes have changed anywhere -- version tag, nonce,
    /// ciphertext, tag -- returns [`DecryptError::Authentication`] and no
    /// plaintext at all. There is no partial result and no "decrypted but
    /// unverified" path, because a caller holding attacker-chosen bytes that
    /// look like plaintext is the whole failure mode an AEAD exists to
    /// prevent.
    ///
    /// # Errors
    ///
    /// [`DecryptError::UnknownVersion`] if the token does not start with a
    /// version this build understands, [`DecryptError::Malformed`] if what
    /// follows is not base64url or is too short to hold a nonce and a tag, and
    /// [`DecryptError::Authentication`] if it does not authenticate.
    ///
    /// ```
    /// use arcature::crypt::{AppKey, DecryptError, Encrypter};
    ///
    /// let encrypter = Encrypter::new(&AppKey::from_hex(&"4a".repeat(64))?);
    /// let token = encrypter.encrypt(b"secret")?;
    ///
    /// assert_eq!(encrypter.decrypt(&token)?, b"secret");
    ///
    /// // One character changed anywhere is a failure, not a partial answer.
    /// let mut tampered: Vec<char> = token.chars().collect();
    /// tampered[3] = if tampered[3] == 'A' { 'B' } else { 'A' };
    /// let tampered: String = tampered.into_iter().collect();
    /// assert_eq!(
    ///     encrypter.decrypt(&tampered),
    ///     Err(DecryptError::Authentication)
    /// );
    ///
    /// assert!(matches!(
    ///     encrypter.decrypt("v9.AAAA"),
    ///     Err(DecryptError::UnknownVersion)
    /// ));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn decrypt(&self, token: &str) -> Result<Vec<u8>, DecryptError> {
        let body = token
            .strip_prefix(VERSION)
            .and_then(|rest| rest.strip_prefix('.'))
            .ok_or(DecryptError::UnknownVersion)?;

        let raw = base64url::decode(body).ok_or(DecryptError::Malformed)?;
        if raw.len() < NONCE_BYTES + TAG_BYTES {
            return Err(DecryptError::Malformed);
        }

        let (nonce, sealed) = raw.split_at(NONCE_BYTES);
        let xnonce = XNonce::try_from(nonce).map_err(|_| DecryptError::Malformed)?;
        let payload = Payload {
            msg: sealed,
            aad: ASSOCIATED_DATA,
        };
        self.cipher
            .decrypt(&xnonce, payload)
            .map_err(|_| DecryptError::Authentication)
    }

    /// Decrypt a token and require the plaintext to be UTF-8.
    ///
    /// # Errors
    ///
    /// Everything [`decrypt`](Self::decrypt) returns, plus
    /// [`DecryptError::NotUtf8`] when the authenticated plaintext is not valid
    /// UTF-8. That last one cannot be caused by an attacker: it only happens
    /// when [`encrypt`](Self::encrypt) was given bytes and
    /// `decrypt_string` was used to read them back.
    ///
    /// ```
    /// use arcature::crypt::{AppKey, DecryptError, Encrypter};
    ///
    /// let encrypter = Encrypter::new(&AppKey::from_hex(&"4a".repeat(64))?);
    ///
    /// let token = encrypter.encrypt(&[0xff, 0xfe])?;
    /// assert!(matches!(
    ///     encrypter.decrypt_string(&token),
    ///     Err(DecryptError::NotUtf8)
    /// ));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn decrypt_string(&self, token: &str) -> Result<String, DecryptError> {
        String::from_utf8(self.decrypt(token)?).map_err(|_| DecryptError::NotUtf8)
    }
}

impl std::fmt::Debug for Encrypter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Encrypter(XChaCha20-Poly1305, <redacted key>)")
    }
}

/// Why a value could not be encrypted.
///
/// Neither variant is reachable from anything a request carries: encryption of
/// a value that fits in memory has no failure mode except the machine's own
/// randomness being unavailable.
///
/// ```
/// use arcature::crypt::EncryptError;
///
/// assert!(EncryptError::Rng.to_string().contains("random"));
/// assert!(EncryptError::Oversized.to_string().contains("too large"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EncryptError {
    /// The operating system's random number generator failed, so no nonce
    /// could be drawn and nothing was encrypted.
    Rng,
    /// The plaintext is larger than the cipher can address. Not reachable on a
    /// 64-bit target, where the limit is far past what can be allocated.
    Oversized,
}

impl std::fmt::Display for EncryptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Rng => {
                "the operating system's random number generator failed; nothing was encrypted"
            }
            Self::Oversized => "the plaintext is too large for the cipher; nothing was encrypted",
        })
    }
}

impl std::error::Error for EncryptError {}

/// Why a token could not be decrypted.
///
/// The variants distinguish *shapes* of failure so an application can tell a
/// stale link from an attack in its own logs. None of them is safe to treat as
/// "nearly valid": every one of them means no plaintext was produced.
///
/// ```
/// use arcature::crypt::{AppKey, DecryptError, Encrypter};
///
/// let encrypter = Encrypter::new(&AppKey::from_hex(&"4a".repeat(64))?);
///
/// assert_eq!(encrypter.decrypt("nonsense"), Err(DecryptError::UnknownVersion));
/// assert_eq!(encrypter.decrypt("v1.not base64"), Err(DecryptError::Malformed));
/// assert_eq!(encrypter.decrypt("v1.AAAA"), Err(DecryptError::Malformed));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecryptError {
    /// The token does not carry a version tag this build can read. Either it
    /// is not an Arcature token, or it was minted by a newer release.
    UnknownVersion,
    /// The version was understood, but what followed is not unpadded base64url
    /// or is too short to hold a nonce and an authentication tag.
    Malformed,
    /// The authentication tag did not match. The token was altered, or it was
    /// minted under a different key. No plaintext was produced.
    Authentication,
    /// The token authenticated, but the plaintext inside is not valid UTF-8.
    /// Only reachable from [`Encrypter::decrypt_string`].
    NotUtf8,
}

impl std::fmt::Display for DecryptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnknownVersion => "the token does not carry a known version tag",
            Self::Malformed => "the token is not well-formed",
            Self::Authentication => {
                "the token failed authentication; it was altered or was \
                                     encrypted under a different key"
            }
            Self::NotUtf8 => "the token decrypted, but the plaintext is not valid UTF-8",
        })
    }
}

impl std::error::Error for DecryptError {}

#[cfg(test)]
mod tests {
    use super::{DecryptError, Encrypter, NONCE_BYTES, TAG_BYTES, VERSION};
    use crate::crypt::AppKey;
    use crate::crypt::base64url;

    fn encrypter(fill: u8) -> Encrypter {
        Encrypter::new(&AppKey::from_bytes(&[fill; 64]).expect("64 bytes"))
    }

    #[test]
    fn a_token_round_trips() {
        let encrypter = encrypter(0x4a);
        let token = encrypter.encrypt(b"the quick brown fox").expect("encrypt");
        assert_eq!(
            encrypter.decrypt(&token).expect("decrypt"),
            b"the quick brown fox"
        );
    }

    #[test]
    fn an_empty_plaintext_round_trips() {
        let encrypter = encrypter(0x4a);
        let token = encrypter.encrypt(b"").expect("encrypt");
        assert_eq!(
            encrypter.decrypt(&token).expect("decrypt"),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn arbitrary_bytes_round_trip() {
        let encrypter = encrypter(0x4a);
        let plaintext: Vec<u8> = (0..=255).collect();
        let token = encrypter.encrypt(&plaintext).expect("encrypt");
        assert_eq!(encrypter.decrypt(&token).expect("decrypt"), plaintext);
    }

    #[test]
    fn strings_round_trip_including_non_ascii() {
        let encrypter = encrypter(0x4a);
        let token = encrypter
            .encrypt_string("caractère — 文字")
            .expect("encrypt");
        assert_eq!(
            encrypter.decrypt_string(&token).expect("decrypt"),
            "caractère — 文字"
        );
    }

    #[test]
    fn a_token_is_url_safe_and_carries_its_version() {
        let token = encrypter(0x4a).encrypt(b"payload").expect("encrypt");
        let body = token
            .strip_prefix(VERSION)
            .and_then(|rest| rest.strip_prefix('.'))
            .expect("version tag");
        assert!(
            body.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{token}"
        );
    }

    #[test]
    fn a_token_carries_a_nonce_and_a_tag_and_the_ciphertext() {
        let token = encrypter(0x4a).encrypt(b"1234").expect("encrypt");
        let body = token.strip_prefix("v1.").expect("version tag");
        let raw = base64url::decode(body).expect("base64url");
        assert_eq!(raw.len(), NONCE_BYTES + TAG_BYTES + 4);
    }

    #[test]
    fn a_token_from_another_key_does_not_decrypt() {
        let token = encrypter(0x01).encrypt(b"payload").expect("encrypt");
        assert_eq!(
            encrypter(0x02).decrypt(&token),
            Err(DecryptError::Authentication)
        );
    }

    #[test]
    fn a_token_with_no_version_tag_is_refused() {
        let encrypter = encrypter(0x4a);
        let token = encrypter.encrypt(b"payload").expect("encrypt");
        let body = token.strip_prefix("v1.").expect("version tag");
        assert_eq!(encrypter.decrypt(body), Err(DecryptError::UnknownVersion));
    }

    #[test]
    fn a_truncated_token_is_malformed_rather_than_authenticated() {
        let encrypter = encrypter(0x4a);
        // A nonce and nothing else: shorter than any tag.
        let short = format!("v1.{}", base64url::encode(&[0u8; NONCE_BYTES]));
        assert_eq!(encrypter.decrypt(&short), Err(DecryptError::Malformed));
    }

    #[test]
    fn debug_never_shows_the_key() {
        assert_eq!(
            format!("{:?}", encrypter(0x4a)),
            "Encrypter(XChaCha20-Poly1305, <redacted key>)"
        );
    }
}
