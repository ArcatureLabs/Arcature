//! The application key, and the subkeys derived from it.

use secrecy::{ExposeSecret, SecretSlice};
use zeroize::Zeroize;

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// The number of bytes `APP_KEY` carries, and the only length accepted.
const KEY_BYTES: usize = 64;

/// The domain separator every derivation starts from.
///
/// It is versioned because it is part of the key schedule: changing it
/// changes every subkey, which invalidates every token and every signature in
/// flight. A `v2` here would be a new format, introduced alongside `v1`
/// rather than in place of it.
const DERIVATION_DOMAIN: &[u8] = b"arcature/kdf/v1";

/// The label for the `Encrypter`'s subkey.
pub(crate) const ENCRYPTER_LABEL: &[u8] = b"encrypter";

/// The application's master secret: the 64 bytes behind `APP_KEY`.
///
/// This is the same material `arcature::auth::SessionKey` holds and the same
/// 128 hexadecimal characters `arc key:generate` writes into `.env`. There is
/// deliberately no second secret to configure: one key per deployment is one
/// thing to rotate, one thing to store, and one thing to leave out of a
/// repository.
///
/// # Nothing uses these bytes directly
///
/// `APP_KEY` is never handed to a cipher or a MAC as-is. Every consumer gets
/// its own 32-byte subkey, derived as `HMAC-SHA256(APP_KEY, domain ‖ label)`
/// with a distinct label, so no two consumers ever see the same bytes and
/// neither of them sees the cookie signing key. Sharing one key across two
/// algorithms is how a weakness in either becomes a weakness in both, and how
/// a chosen-ciphertext oracle in one becomes a forgery in the other.
///
/// The derivation is a PRF call rather than a full HKDF because there is
/// nothing to extract from: `APP_KEY` is already 64 uniformly random bytes
/// from the OS RNG, which is exactly the input HKDF's expand step wants. It is
/// also exactly one HMAC block wide, so it is used as an HMAC key without
/// being pre-hashed.
///
/// # Debug never shows the key
///
/// The bytes live in a [`secrecy::SecretSlice`], which zeroizes on drop, and
/// the `Debug` impl prints a placeholder.
///
/// ```
/// use arcature::crypt::AppKey;
///
/// // In an application:
/// //     let key = AppKey::from_hex(&arcature::config::env_required("APP_KEY")?)?;
/// let key = AppKey::from_hex(&"4a".repeat(64))?;
/// assert_eq!(format!("{key:?}"), "AppKey(<redacted 64-byte key>)");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[non_exhaustive]
pub struct AppKey {
    inner: SecretSlice<u8>,
}

impl AppKey {
    /// Take the key from 64 raw bytes.
    ///
    /// # Errors
    ///
    /// [`AppKeyError::WrongLength`] if `bytes` is not exactly 64 long.
    ///
    /// ```
    /// use arcature::crypt::{AppKey, AppKeyError};
    ///
    /// assert!(AppKey::from_bytes(&[7u8; 64]).is_ok());
    /// assert!(matches!(
    ///     AppKey::from_bytes(&[7u8; 32]),
    ///     Err(AppKeyError::WrongLength)
    /// ));
    /// ```
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AppKeyError> {
        if bytes.len() != KEY_BYTES {
            return Err(AppKeyError::WrongLength);
        }
        Ok(Self {
            inner: SecretSlice::from(bytes.to_vec()),
        })
    }

    /// Take the key from the 128 hexadecimal characters `arc key:generate`
    /// writes.
    ///
    /// Surrounding whitespace is trimmed, and either case of hexadecimal is
    /// accepted -- the generator writes lowercase, but a value that has been
    /// through a secret store and back may not have stayed that way, and
    /// rejecting an unambiguous key on its capitalisation would be a fault
    /// report with no fault behind it.
    ///
    /// # Errors
    ///
    /// [`AppKeyError::Empty`] when there is nothing but whitespace,
    /// [`AppKeyError::NotHexadecimal`] when a character is not a hexadecimal
    /// digit or the length is odd, and [`AppKeyError::WrongLength`] when the
    /// decoded value is not 64 bytes.
    ///
    /// ```
    /// use arcature::crypt::{AppKey, AppKeyError};
    ///
    /// assert!(AppKey::from_hex(&"4a".repeat(64)).is_ok());
    /// // `arc key:generate` writes lowercase; uppercase decodes the same.
    /// assert!(AppKey::from_hex(&"4A".repeat(64)).is_ok());
    ///
    /// assert!(matches!(AppKey::from_hex("   "), Err(AppKeyError::Empty)));
    /// assert!(matches!(
    ///     AppKey::from_hex(&"zz".repeat(64)),
    ///     Err(AppKeyError::NotHexadecimal)
    /// ));
    /// assert!(matches!(
    ///     AppKey::from_hex("4a4a"),
    ///     Err(AppKeyError::WrongLength)
    /// ));
    /// ```
    pub fn from_hex(hex: &str) -> Result<Self, AppKeyError> {
        let hex = hex.trim();
        if hex.is_empty() {
            return Err(AppKeyError::Empty);
        }
        if !hex.len().is_multiple_of(2) {
            return Err(AppKeyError::NotHexadecimal);
        }

        let mut bytes = Vec::with_capacity(hex.len() / 2);
        for pair in hex.as_bytes().chunks(2) {
            // Deliberately not `u8::from_str_radix`: it accepts a leading
            // sign, so the pair `+4` would decode to 4 and a 128-character
            // string of them would be read as a valid key. A decoder for a
            // secret should accept exactly one spelling of it.
            let high = hex_digit(pair[0]).ok_or(AppKeyError::NotHexadecimal)?;
            let low = hex_digit(pair[1]).ok_or(AppKeyError::NotHexadecimal)?;
            bytes.push((high << 4) | low);
        }

        let key = Self::from_bytes(&bytes);
        bytes.zeroize();
        key
    }

    /// Derive the 32-byte subkey for `label`.
    ///
    /// `pub(crate)` on purpose: a caller who can name a label can make two
    /// subsystems share a key, which is the mistake the derivation exists to
    /// prevent. Labels are constants in this module.
    pub(crate) fn subkey(&self, label: &[u8]) -> SecretSlice<u8> {
        // `new_from_slice` only fails for key lengths HMAC cannot take, and
        // HMAC takes every length.
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.inner.expose_secret())
            .expect("HMAC-SHA256 accepts a key of any length");
        mac.update(DERIVATION_DOMAIN);
        // Length-prefixed, so no two labels can ever produce the same input:
        // without it `b"ab" ‖ b"c"` and `b"a" ‖ b"bc"` are one string.
        mac.update(&(label.len() as u64).to_be_bytes());
        mac.update(label);

        let mut derived = mac.finalize().into_bytes();
        let subkey = SecretSlice::from(derived.to_vec());
        derived.as_mut_slice().zeroize();
        subkey
    }
}

impl std::fmt::Debug for AppKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AppKey(<redacted 64-byte key>)")
    }
}

/// Why an `APP_KEY` value could not be read.
///
/// Every variant is about the value's shape, never its content: nothing here
/// echoes the key, so an error that reaches a log carries no secret with it.
///
/// ```
/// use arcature::crypt::{AppKey, AppKeyError};
///
/// let error = AppKey::from_hex("").expect_err("empty");
/// assert!(matches!(error, AppKeyError::Empty));
/// assert!(error.to_string().contains("arc key:generate"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AppKeyError {
    /// The value was empty or entirely whitespace.
    Empty,
    /// The value contained something that is not a hexadecimal digit, or had
    /// an odd number of digits.
    NotHexadecimal,
    /// The value decoded, but not to 64 bytes.
    WrongLength,
}

impl std::fmt::Display for AppKeyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self {
            Self::Empty => "APP_KEY is empty",
            Self::NotHexadecimal => "APP_KEY is not hexadecimal",
            Self::WrongLength => "APP_KEY is not 128 hexadecimal characters (64 bytes)",
        };
        write!(
            formatter,
            "{detail}; run `arc key:generate` to write a valid one into .env"
        )
    }
}

/// One hexadecimal digit, either case, as its numeric value.
///
/// Byte-wise rather than `char`-wise, which also settles the non-ASCII case:
/// a multi-byte character cannot have a continuation byte in this range, so a
/// key pasted with a stray accented letter is refused rather than sliced.
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

impl std::error::Error for AppKeyError {}

#[cfg(test)]
mod tests {
    use super::{AppKey, AppKeyError, ENCRYPTER_LABEL, KEY_BYTES};
    use secrecy::ExposeSecret;

    fn key(fill: u8) -> AppKey {
        AppKey::from_bytes(&[fill; KEY_BYTES]).expect("64 bytes")
    }

    #[test]
    fn hex_and_bytes_agree() {
        let from_hex = AppKey::from_hex(&"4a".repeat(KEY_BYTES)).expect("valid hex");
        let from_bytes = key(0x4a);
        assert_eq!(
            from_hex.subkey(ENCRYPTER_LABEL).expose_secret(),
            from_bytes.subkey(ENCRYPTER_LABEL).expose_secret()
        );
    }

    #[test]
    fn case_does_not_change_the_key() {
        let lower = AppKey::from_hex(&"ab".repeat(KEY_BYTES)).expect("lower");
        let upper = AppKey::from_hex(&"AB".repeat(KEY_BYTES)).expect("upper");
        assert_eq!(
            lower.subkey(ENCRYPTER_LABEL).expose_secret(),
            upper.subkey(ENCRYPTER_LABEL).expose_secret()
        );
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let padded = format!("  {}\n", "4a".repeat(KEY_BYTES));
        assert!(AppKey::from_hex(&padded).is_ok());
    }

    #[test]
    fn a_short_key_is_refused() {
        assert_eq!(
            AppKey::from_bytes(&[0u8; 32]).unwrap_err(),
            AppKeyError::WrongLength
        );
        assert_eq!(
            AppKey::from_hex(&"4a".repeat(32)).unwrap_err(),
            AppKeyError::WrongLength
        );
    }

    #[test]
    fn an_odd_number_of_digits_is_not_hexadecimal() {
        let odd = "4".repeat(127);
        assert_eq!(
            AppKey::from_hex(&odd).unwrap_err(),
            AppKeyError::NotHexadecimal
        );
    }

    #[test]
    fn a_non_hexadecimal_character_is_refused() {
        assert_eq!(
            AppKey::from_hex(&"zz".repeat(KEY_BYTES)).unwrap_err(),
            AppKeyError::NotHexadecimal
        );
        // `u8::from_str_radix` accepts a leading `+`, so a decoder built on
        // it reads this 128-character string as 64 valid bytes. The digit
        // table does not.
        assert_eq!(
            AppKey::from_hex(&"+4".repeat(KEY_BYTES)).unwrap_err(),
            AppKeyError::NotHexadecimal
        );
        // Same trap, other sign, and one with whitespace inside rather than
        // around the value.
        assert_eq!(
            AppKey::from_hex(&"-4".repeat(KEY_BYTES)).unwrap_err(),
            AppKeyError::NotHexadecimal
        );
        assert_eq!(
            AppKey::from_hex(&"4 ".repeat(KEY_BYTES)).unwrap_err(),
            AppKeyError::NotHexadecimal
        );
        // A non-ASCII character is refused rather than sliced mid-codepoint.
        assert_eq!(
            AppKey::from_hex(&"é".repeat(KEY_BYTES)).unwrap_err(),
            AppKeyError::NotHexadecimal
        );
    }

    // The property the whole derivation exists for.
    #[test]
    fn two_labels_give_two_unrelated_subkeys() {
        let key = key(0x11);
        let encrypter = key.subkey(ENCRYPTER_LABEL);
        let other = key.subkey(b"some-other-consumer");
        assert_ne!(encrypter.expose_secret(), other.expose_secret());
        assert_eq!(encrypter.expose_secret().len(), 32);
        assert_eq!(other.expose_secret().len(), 32);
    }

    #[test]
    fn a_subkey_is_not_the_master_key() {
        let key = key(0x22);
        assert_ne!(key.subkey(ENCRYPTER_LABEL).expose_secret(), &[0x22u8; 32]);
    }

    #[test]
    fn two_master_keys_give_two_subkeys() {
        assert_ne!(
            key(0x01).subkey(ENCRYPTER_LABEL).expose_secret(),
            key(0x02).subkey(ENCRYPTER_LABEL).expose_secret()
        );
    }

    #[test]
    fn derivation_is_deterministic() {
        assert_eq!(
            key(0x33).subkey(ENCRYPTER_LABEL).expose_secret(),
            key(0x33).subkey(ENCRYPTER_LABEL).expose_secret()
        );
    }

    // Length prefixing: without it `"a" ‖ "bc"` and `"ab" ‖ "c"` would be one
    // input. There is only one label argument, so the prefix is what keeps a
    // future label from colliding with a present one.
    #[test]
    fn labels_that_concatenate_alike_do_not_collide() {
        let key = key(0x44);
        assert_ne!(
            key.subkey(b"ab").expose_secret(),
            key.subkey(b"abc").expose_secret()
        );
    }

    #[test]
    fn debug_never_shows_the_key() {
        let rendered = format!("{:?}", key(0x55));
        assert_eq!(rendered, "AppKey(<redacted 64-byte key>)");
        assert!(!rendered.contains("55"));
    }
}
