//! Argon2id password hashing configuration.
//!
//! These are the cost parameters for the memory-hard Argon2id KDF. Arcature
//! does not implement Argon2; it configures the audited [`argon2`] crate and
//! exposes the OWASP-recommended defaults.

use std::fmt;

use argon2::Params;

/// Resolved Argon2id cost parameters.
///
/// Construct with [`PasswordConfig::recommended`] for OWASP-aligned defaults,
/// or [`PasswordConfig::new`] for explicit tuning. The parameters are
/// validated by [`argon2::Params::new`] when the [`crate::PasswordHasher`] is
/// built, so an impossible combination becomes a typed
/// [`crate::PasswordHashError::InvalidParams`] rather than a silent
/// misconfiguration.
///
/// # Defaults
///
/// [`PasswordConfig::recommended`] selects Argon2id v19 with:
/// - `m_cost` = 19456 KiB (19 MiB) memory per hash
/// - `t_cost` = 2 iterations
/// - `p_cost` = 1 degree of parallelism
/// - 16-byte salts
///
/// These are the OWASP [Password Storage Cheat Sheet] recommendations for
/// Argon2id. Memory and iteration counts are workload- and hardware-dependent;
/// applications must tune for their deployment.
///
/// [Password Storage Cheat Sheet]: https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html
#[derive(Clone, Copy)]
pub struct PasswordConfig {
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
}

impl PasswordConfig {
    /// Build explicit Argon2id cost parameters.
    ///
    /// `memory_kib` is the memory cost in kibibytes (KiB). `iterations` is the
    /// time cost (passes over memory). `parallelism` is the degree of
    /// parallelism (lanes).
    ///
    /// The values are validated by [`argon2::Params`] at hasher construction
    /// time; see [`crate::PasswordHasher::new`].
    #[must_use]
    pub fn new(memory_kib: u32, iterations: u32, parallelism: u32) -> Self {
        Self {
            memory_kib,
            iterations,
            parallelism,
        }
    }

    /// OWASP-aligned Argon2id defaults (19 MiB, 2 iterations, 1 lane).
    #[must_use]
    pub fn recommended() -> Self {
        Self::new(19_456, 2, 1)
    }

    /// The memory cost in kibibytes (KiB).
    #[must_use]
    pub fn memory_kib(&self) -> u32 {
        self.memory_kib
    }

    /// The time cost (number of iterations).
    #[must_use]
    pub fn iterations(&self) -> u32 {
        self.iterations
    }

    /// The degree of parallelism (lanes).
    #[must_use]
    pub fn parallelism(&self) -> u32 {
        self.parallelism
    }

    /// Build the upstream [`argon2::Params`] for this configuration.
    pub(crate) fn to_params(self) -> Result<Params, argon2::Error> {
        Params::new(self.memory_kib, self.iterations, self.parallelism, None)
    }
}

impl fmt::Debug for PasswordConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PasswordConfig")
            .field("algorithm", &"argon2id")
            .field("memory_kib", &self.memory_kib)
            .field("iterations", &self.iterations)
            .field("parallelism", &self.parallelism)
            .finish()
    }
}

impl Default for PasswordConfig {
    fn default() -> Self {
        Self::recommended()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommended_params_are_owasp_defaults() {
        let config = PasswordConfig::recommended();
        assert_eq!(config.memory_kib(), 19_456);
        assert_eq!(config.iterations(), 2);
        assert_eq!(config.parallelism(), 1);
    }

    #[test]
    fn to_params_succeeds_for_recommended() {
        let config = PasswordConfig::recommended();
        assert!(config.to_params().is_ok());
    }

    #[test]
    fn debug_does_not_leak_secrets() {
        // PasswordConfig carries no secret material, but assert the Debug
        // output is the public cost parameters only (defensive).
        let config = PasswordConfig::recommended();
        let debug = format!("{config:?}");
        assert!(debug.contains("argon2id"));
        assert!(debug.contains("19456"));
    }
}
