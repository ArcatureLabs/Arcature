//! Password hashing, verification, and the zeroize-on-drop plaintext wrapper.
//!
//! Arcature does not implement Argon2id; it configures the audited [`argon2`]
//! crate. This module owns: constructing the [`argon2::Argon2`] context from
//! resolved [`PasswordConfig`], generating a 16-byte cryptographically-random
//! salt with the certified `getrandom` OS RNG, emitting a PHC-formatted
//! [`PasswordHashString`] suitable for storage, and the constant-time
//! verification path.

use std::fmt;

use argon2::PasswordHasher as _;
use argon2::password_hash::{PasswordHash, PasswordVerifier as _, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use secrecy::{ExposeSecret, SecretSlice};

use crate::auth::{PasswordConfig, PasswordHashError, PasswordVerifyError};

/// The number of random salt bytes generated per hash (the `password_hash`
/// recommended length).
const SALT_LEN: usize = 16;

/// An Argon2id password hasher configured from [`PasswordConfig`].
///
/// Construct with [`PasswordHasher::new`]. The hasher is `Clone` (the
/// underlying `Argon2` context is cheaply clonable) and `Send + Sync`, so it
/// works as normal Axum state.
#[derive(Clone)]
pub struct PasswordHasher {
    argon2: Argon2<'static>,
}

impl PasswordHasher {
    /// Build an Argon2id v19 hasher from resolved cost parameters.
    ///
    /// The parameters are validated by [`argon2::Params::new`] here, so an
    /// impossible combination (memory below the minimum, zero iterations,
    /// parallelism out of range) becomes a typed
    /// [`PasswordHashError::InvalidParams`] immediately, before any password
    /// is hashed.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordHashError::InvalidParams`] if the cost parameters
    /// are out of range.
    pub fn new(config: PasswordConfig) -> Result<Self, PasswordHashError> {
        let params = config
            .to_params()
            .map_err(|error| PasswordHashError::InvalidParams {
                detail: error.to_string(),
            })?;
        // Algorithm::Argon2id (default) and Version::V0x13 (default, "v19").
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        Ok(Self { argon2 })
    }

    /// Hash a plaintext password with a fresh random salt, returning a
    /// PHC-formatted [`PasswordHashString`].
    ///
    /// The salt is 16 bytes drawn from the certified `getrandom` OS RNG and
    /// base64url-encoded into the PHC string. The plaintext password is never
    /// copied, stored, or logged; only the caller holds it (ideally in a
    /// [`PasswordSecret`]).
    ///
    /// # Errors
    ///
    /// Returns [`PasswordHashError::Hash`] if the Argon2id computation fails
    /// (e.g. on extremely large parameters).
    pub fn hash(&self, password: &[u8]) -> Result<PasswordHashString, PasswordHashError> {
        let salt = generate_salt().map_err(|detail| PasswordHashError::InvalidParams { detail })?;
        let password_hash = self
            .argon2
            .hash_password(password, salt.as_salt())
            .map_err(|source| PasswordHashError::Hash { source })?;
        Ok(PasswordHashString::from_password_hash(password_hash))
    }

    /// Check whether a stored hash was computed under the hasher's current
    /// Argon2id parameters.
    ///
    /// This reads the `m`/`t`/`p` fields from the PHC string and compares them
    /// against [`PasswordConfig`]. It does not verify the password; call this
    /// only after [`verify_password`] succeeds (rehashing a mismatched
    /// password is pointless and a timing hazard).
    #[must_use]
    pub fn needs_rehash(&self, stored: &PasswordHashString) -> RehashOutcome {
        let parsed = stored.password_hash();
        let stored_params: Params = match (&parsed).try_into() {
            Ok(params) => params,
            Err(_) => return RehashOutcome::NeedsRehash,
        };
        let current = self.argon2.params();
        if stored_params.m_cost() == current.m_cost()
            && stored_params.t_cost() == current.t_cost()
            && stored_params.p_cost() == current.p_cost()
        {
            RehashOutcome::Current
        } else {
            RehashOutcome::NeedsRehash
        }
    }

    pub(crate) fn argon2(&self) -> &Argon2<'static> {
        &self.argon2
    }
}

impl fmt::Debug for PasswordHasher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PasswordHasher")
            .field("algorithm", &"argon2id")
            .field("version", &"v19")
            .finish_non_exhaustive()
    }
}

/// Whether a stored hash needs rehashing under the current parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RehashOutcome {
    /// The stored hash already matches the hasher's parameters.
    Current,
    /// The stored hash was computed under different parameters; rehash the
    /// password under the current parameters at the next successful login.
    NeedsRehash,
}

/// An owned PHC-format Argon2id password hash string.
///
/// The stored form: a self-describing string like
/// `$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>` that encodes the algorithm,
/// version, cost parameters, salt, and hash output. Construct it with
/// [`PasswordHasher::hash`]; parse and verify it with [`verify_password`].
///
/// The string does not contain the plaintext password and is safe to store
/// and log at the configured cost. It implements `Debug`/`Display` as the raw
/// PHC string (which contains only the hash, never the password).
#[derive(Clone)]
pub struct PasswordHashString {
    inner: argon2::password_hash::PasswordHashString,
}

impl PasswordHashString {
    pub(crate) fn from_password_hash(hash: PasswordHash<'_>) -> Self {
        Self {
            inner: argon2::password_hash::PasswordHashString::from(hash),
        }
    }

    /// Parse a stored PHC string. Returns
    /// [`PasswordVerifyError::MalformedHash`] if the string is not a valid
    /// Argon2id PHC hash.
    ///
    /// # Errors
    ///
    /// - [`PasswordVerifyError::MalformedHash`] -- `stored` is not a valid PHC
    ///   string.
    pub fn new(stored: &str) -> Result<Self, PasswordVerifyError> {
        let inner = argon2::password_hash::PasswordHashString::new(stored).map_err(|e| {
            PasswordVerifyError::MalformedHash {
                detail: e.to_string(),
            }
        })?;
        Ok(Self { inner })
    }

    /// The raw PHC string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.inner.as_str()
    }

    pub(crate) fn password_hash(&self) -> PasswordHash<'_> {
        self.inner.password_hash()
    }
}

impl fmt::Debug for PasswordHashString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The PHC string contains only the hash, never the password. It is
        // safe to display; this is the whole point of the PHC format.
        write!(formatter, "PasswordHashString({})", self.as_str())
    }
}

impl fmt::Display for PasswordHashString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.as_str())
    }
}

/// Verify a plaintext password against a stored Argon2id hash.
///
/// This is the constant-time verification path; the comparison is delegated to
/// the audited `argon2`/`password-hash` primitives.
///
/// # Errors
///
/// - [`PasswordVerifyError::MalformedHash`] -- `stored` is not a valid PHC
///   string.
/// - [`PasswordVerifyError::PasswordMismatch`] -- the password does not match.
/// - [`PasswordVerifyError::Verify`] -- the Argon2id computation failed.
pub fn verify_password(
    hasher: &PasswordHasher,
    password: &[u8],
    stored: &PasswordHashString,
) -> Result<(), PasswordVerifyError> {
    let parsed = stored.password_hash();
    hasher
        .argon2()
        .verify_password(password, &parsed)
        .map_err(|error| match error {
            // password_hash::Error::Password is the mismatch sentinel.
            argon2::password_hash::Error::Password => PasswordVerifyError::PasswordMismatch,
            other => PasswordVerifyError::Verify { source: other },
        })
}

/// A zeroize-on-drop wrapper for a plaintext password.
///
/// The plaintext password must never appear in `Debug`, `Display`, error
/// output, or logs. [`PasswordSecret`] holds the password bytes in a
/// [`secrecy::SecretSlice`] that zeroes them on drop and redacts every
/// formatting path.
pub struct PasswordSecret {
    inner: SecretSlice<u8>,
}

impl PasswordSecret {
    /// Wrap a password from any byte source. The bytes are copied into a
    /// heap allocation that is zeroized on drop.
    #[must_use]
    pub fn new(password: impl AsRef<[u8]>) -> Self {
        let bytes: Vec<u8> = password.as_ref().to_vec();
        Self {
            inner: SecretSlice::from(bytes),
        }
    }

    /// Borrow the raw password bytes for hashing/verification. This is the
    /// only way to reach the plaintext; it is not exposed through `Debug` or
    /// `Display`.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        self.inner.expose_secret()
    }
}

impl fmt::Debug for PasswordSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "PasswordSecret(<{} redacted bytes>)",
            self.inner.expose_secret().len()
        )
    }
}

/// Generate a 16-byte random salt and base64url-encode it as a
/// [`SaltString`].
///
/// Uses the certified `getrandom` 0.4 OS RNG. This sidesteps the
/// `rand_core` 0.6/0.10 version split between `password-hash` 0.5 (which
/// wants `rand_core` 0.6 for `SaltString::generate`) and `getrandom` 0.4
/// (which re-exports `rand_core` 0.10): `SaltString::encode_b64` takes raw
/// bytes and needs no RNG trait object. The salt length is the
/// `password_hash` recommended 16 bytes.
fn generate_salt() -> Result<SaltString, String> {
    let mut bytes = [0u8; SALT_LEN];
    getrandom::fill(&mut bytes).map_err(|e| format!("getrandom failed: {e}"))?;
    SaltString::encode_b64(&bytes).map_err(|e| format!("salt encode failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_a_password_to_phc_string() {
        let hasher = PasswordHasher::new(PasswordConfig::recommended()).expect("valid params");
        let hash = hasher
            .hash(b"correct horse battery staple")
            .expect("hash ok");
        let s = hash.as_str();
        assert!(s.starts_with("$argon2id$"), "got: {s}");
        assert!(s.contains("$v=19$"), "got: {s}");
        assert_eq!(s.matches('$').count(), 5, "got: {s}");
    }

    #[test]
    fn different_salts_each_hash() {
        let hasher = PasswordHasher::new(PasswordConfig::recommended()).expect("valid params");
        let h1 = hasher.hash(b"same password").expect("hash 1");
        let h2 = hasher.hash(b"same password").expect("hash 2");
        assert_ne!(h1.as_str(), h2.as_str(), "salts should differ");
    }

    #[test]
    fn rejects_zero_iterations() {
        let result = PasswordHasher::new(PasswordConfig::new(19_456, 0, 1));
        assert!(matches!(
            result,
            Err(PasswordHashError::InvalidParams { .. })
        ));
    }

    #[test]
    fn debug_does_not_leak_password() {
        let hasher = PasswordHasher::new(PasswordConfig::recommended()).expect("valid params");
        let debug = format!("{hasher:?}");
        assert!(!debug.contains("password"));
    }

    #[test]
    fn verifies_correct_password() {
        let h = PasswordHasher::new(PasswordConfig::recommended()).expect("valid params");
        let hash = h.hash(b"correct horse battery staple").expect("hash");
        assert!(verify_password(&h, b"correct horse battery staple", &hash).is_ok());
    }

    #[test]
    fn rejects_wrong_password() {
        let h = PasswordHasher::new(PasswordConfig::recommended()).expect("valid params");
        let hash = h.hash(b"correct horse battery staple").expect("hash");
        assert!(matches!(
            verify_password(&h, b"wrong password", &hash),
            Err(PasswordVerifyError::PasswordMismatch)
        ));
    }

    #[test]
    fn rejects_malformed_hash() {
        let result = PasswordHashString::new("not-a-phc-string");
        assert!(matches!(
            result,
            Err(PasswordVerifyError::MalformedHash { .. })
        ));
    }

    #[test]
    fn debug_and_display_show_phc_not_password() {
        let h = PasswordHasher::new(PasswordConfig::recommended()).expect("valid params");
        let hash = h.hash(b"secret-password-value").expect("hash");
        let debug = format!("{hash:?}");
        let display = format!("{hash}");
        assert!(!debug.contains("secret-password-value"));
        assert!(!display.contains("secret-password-value"));
        assert!(debug.contains("argon2id"));
    }

    #[test]
    fn fresh_hash_does_not_need_rehash() {
        let h = PasswordHasher::new(PasswordConfig::recommended()).expect("valid");
        let hash = h.hash(b"password").expect("hash");
        assert_eq!(h.needs_rehash(&hash), RehashOutcome::Current);
    }

    #[test]
    fn hash_under_old_params_needs_rehash() {
        let old = PasswordHasher::new(PasswordConfig::new(19_456, 2, 1)).expect("valid");
        let hash = old.hash(b"password").expect("hash");
        // New hasher uses higher memory cost.
        let new = PasswordHasher::new(PasswordConfig::new(47_104, 2, 1)).expect("valid");
        assert_eq!(new.needs_rehash(&hash), RehashOutcome::NeedsRehash);
    }

    #[test]
    fn secret_debug_redacts_password() {
        let secret = PasswordSecret::new("hunter2");
        let debug = format!("{secret:?}");
        assert!(!debug.contains("hunter2"));
        assert!(debug.contains("redacted"));
    }

    #[test]
    fn secret_expose_returns_bytes() {
        let secret = PasswordSecret::new("hunter2");
        assert_eq!(secret.expose(), b"hunter2");
    }
}
