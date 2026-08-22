//! Keyed cryptography for application data, derived from `APP_KEY`.
//!
//! Everything here hangs off one secret. [`AppKey`] holds the 64 bytes `arc
//! key:generate` writes into `.env`, and hands each consumer a labelled
//! 32-byte subkey of its own. Nothing in this module uses `APP_KEY` directly,
//! and no two consumers share a subkey -- so a weakness found in one
//! construction stays inside it.
//!
//! # What is here
//!
//! * `Encrypter` (feature `crypt`) -- XChaCha20-Poly1305 authenticated
//!   encryption. Turns bytes into an opaque, versioned token and refuses to
//!   return a single byte of one that has been altered.
//! * `UrlSigner` (feature `signed-urls`) -- HMAC-SHA256 over a canonicalised
//!   URL. Mints a link that carries its own proof of origin and, optionally,
//!   its own deadline.
//!
//! # Off by default, and separately
//!
//! Both features are off unless asked for, so a build that wants neither is
//! byte-for-byte what it was before this module existed. They are two
//! features rather than one because they cost different things: signing needs
//! a MAC, encrypting needs a cipher, and an application that only hands out
//! one-hour download links has no reason to carry an AEAD.
//!
//! # Everything emitted is versioned
//!
//! Every value this module produces starts with a format version, so replacing
//! an algorithm later is an additive change: the new reader keeps the old
//! branch, tokens and links already in flight keep working, and nothing has to
//! be re-issued during a deploy. A format with no version can only ever be
//! changed by breaking every holder of an outstanding token at once.
//!
//! # Nothing here compares a secret with `==`
//!
//! A `==` on a MAC returns at the first differing byte, and an attacker who
//! can measure that difference recovers a valid signature a byte at a time.
//! The URL signer uses `subtle::ConstantTimeEq`; the encrypter leans on the
//! AEAD's own tag check, which is constant-time for the same reason.
//!
//! ```
//! use arcature::crypt::AppKey;
//!
//! // In an application, from `APP_KEY`:
//! //     let key = AppKey::from_hex(&arcature::config::env_required("APP_KEY")?)?;
//! let key = AppKey::from_hex(&"4a".repeat(64))?;
//!
//! // `Encrypter::new(&key)` and `UrlSigner::new(&key, &config)` each take a
//! // labelled subkey of this, never the key itself -- and there is no
//! // accessor that would hand one out.
//! assert_eq!(format!("{key:?}"), "AppKey(<redacted 64-byte key>)");
//! # Ok::<(), arcature::crypt::AppKeyError>(())
//! ```

mod base64url;
#[cfg(feature = "crypt")]
mod encrypter;
mod key;
#[cfg(feature = "signed-urls")]
mod signer;

#[cfg(feature = "crypt")]
pub use encrypter::{DecryptError, EncryptError, Encrypter};
pub use key::{AppKey, AppKeyError};
#[cfg(feature = "signed-urls")]
pub use signer::{Clock, SignedUrlError, SystemClock, UrlSigner};
