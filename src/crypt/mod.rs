//! Keyed cryptography for application data, derived from `APP_KEY`.
//!
//! Everything here hangs off one secret. [`AppKey`] holds the 64 bytes `arc
//! key:generate` writes into `.env`, and hands each consumer a labelled
//! 32-byte subkey of its own. Nothing in this module uses `APP_KEY` directly,
//! and no two consumers share a subkey.
//!
//! # What is here
//!
//! * [`Encrypter`] -- XChaCha20-Poly1305 authenticated encryption. Turns
//!   bytes into an opaque, versioned token and refuses to return a single
//!   byte of one that has been altered.
//!
//! # Off by default
//!
//! The whole module is behind the `crypt` feature, so a build that does not
//! ask for it carries no cipher and is byte-for-byte what it was before this
//! module existed. The moment a build can produce ciphertext is the moment
//! somebody has to own a key rotation story, and enabling the feature is
//! where that decision is written down.
//!
//! # Tokens are versioned
//!
//! Every value this module emits starts with a format version, so replacing
//! an algorithm later is an additive change: the new reader keeps the old
//! branch, tokens already in flight keep working, and nothing has to be
//! re-issued during a deploy. A format with no version can only ever be
//! changed by breaking every holder of an outstanding token at once.
//!
//! ```
//! use arcature::crypt::{AppKey, Encrypter};
//!
//! // In an application, from `APP_KEY`:
//! //     let key = AppKey::from_hex(&arcature::config::env_required("APP_KEY")?)?;
//! let key = AppKey::from_hex(&"4a".repeat(64))?;
//! let encrypter = Encrypter::new(&key);
//!
//! let token = encrypter.encrypt_string("invoice 4417")?;
//! assert_eq!(encrypter.decrypt_string(&token)?, "invoice 4417");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod base64url;
mod encrypter;
mod key;

pub use encrypter::{DecryptError, EncryptError, Encrypter};
pub use key::{AppKey, AppKeyError};
