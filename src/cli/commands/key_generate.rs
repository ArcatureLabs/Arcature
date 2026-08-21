//! `arc key:generate [--show]` — mint the application's 64-byte secret.
//!
//! The key is the master secret for signed session cookies, so it is exactly
//! the shape [`SessionKey`](crate::auth::SessionKey) requires: 64 bytes from
//! the certified OS RNG. It is emitted as 128 lowercase hex characters rather
//! than base64 because hex has no alphabet variants to get wrong in a `.env`
//! parser, and length alone tells you whether the value is intact.
//!
//! By default the key is written into `.env` as `APP_KEY`, replacing any
//! existing line so re-running is idempotent from the file's point of view.
//! `--show` prints it instead and touches nothing, which is what a deploy
//! pipeline wants when the secret belongs in a secret store rather than on
//! disk.
//!
//! # Why this command is gated on `auth`
//!
//! The key type and the RNG behind it both live in the `auth` module. A CLI
//! built without `auth` has no certified source of randomness to offer, and
//! inventing a second one here would mean two ways to generate the same
//! secret with only one of them reviewed.

use std::path::{Path, PathBuf};

use crate::auth::SessionKey;

/// The `.env` variable the key is written to.
const KEY_VARIABLE: &str = "APP_KEY";

/// Execute `arc key:generate` against the current directory.
///
/// # Errors
///
/// See [`KeyGenerateError`].
pub fn run(show: bool) -> Result<(), KeyGenerateError> {
    let root = std::env::current_dir().map_err(|source| KeyGenerateError::Io {
        path: PathBuf::from("."),
        source,
    })?;
    let key = generate(show, &root)?;
    if show {
        println!("{KEY_VARIABLE}={key}");
    } else {
        println!("{KEY_VARIABLE} written to .env");
    }
    Ok(())
}

/// The testable half of [`run`]: mint the key and, unless `show`, persist it.
/// Returns the hex key either way.
///
/// # Errors
///
/// See [`KeyGenerateError`].
pub fn generate(show: bool, root: &Path) -> Result<String, KeyGenerateError> {
    let key = SessionKey::generate().map_err(|_| KeyGenerateError::Rng)?;
    let hex = to_hex(key.as_bytes());

    if show {
        return Ok(hex);
    }

    let env_path = root.join(".env");
    let existing = match std::fs::read_to_string(&env_path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(KeyGenerateError::NoEnvFile {
                path: env_path.clone(),
            });
        }
        Err(source) => {
            return Err(KeyGenerateError::Io {
                path: env_path,
                source,
            });
        }
    };

    std::fs::write(&env_path, upsert(&existing, &hex)).map_err(|source| KeyGenerateError::Io {
        path: env_path,
        source,
    })?;
    Ok(hex)
}

/// Replace the `APP_KEY` line if there is one, otherwise append it.
///
/// Rewriting in place rather than appending keeps the file from growing a new
/// key on every run, which is the failure mode where a `.env` ends up with
/// three keys and the loader silently picks one.
fn upsert(existing: &str, hex: &str) -> String {
    let assignment = format!("{KEY_VARIABLE}={hex}");
    let prefix = format!("{KEY_VARIABLE}=");

    let mut replaced = false;
    let mut lines: Vec<String> = existing
        .lines()
        .map(|line| {
            if line.trim_start().starts_with(&prefix) {
                replaced = true;
                assignment.clone()
            } else {
                line.to_string()
            }
        })
        .collect();

    if !replaced {
        if lines.last().is_some_and(|last| !last.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(assignment);
    }

    lines.join("\n").trim_end().to_string() + "\n"
}

/// Lowercase hex, written out rather than pulled in: 64 bytes is not worth a
/// dependency, and the encoding has to stay stable forever.
fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            // Writing into a String is infallible.
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// An error from the `key:generate` command.
#[derive(Debug)]
pub enum KeyGenerateError {
    /// The OS random number generator failed.
    Rng,
    /// There is no `.env` to write into.
    NoEnvFile { path: PathBuf },
    /// A filesystem operation failed.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for KeyGenerateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rng => formatter.write_str(
                "the operating system's random number generator failed; \
                 no key was generated",
            ),
            Self::NoEnvFile { path } => write!(
                formatter,
                "{} does not exist; run this from an application root, \
                 or use `arc key:generate --show` to print a key without \
                 writing a file",
                path.display()
            ),
            Self::Io { path, source } => {
                write!(formatter, "could not write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for KeyGenerateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_with_env(contents: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".env"), contents).expect("env");
        dir
    }

    #[test]
    fn a_generated_key_is_sixty_four_bytes_of_lowercase_hex() {
        let dir = project_with_env("APP_NAME=demo\n");
        let key = generate(false, dir.path()).expect("generated");
        assert_eq!(key.len(), 128);
        assert!(
            key.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn two_generated_keys_differ() {
        let dir = project_with_env("APP_NAME=demo\n");
        let first = generate(true, dir.path()).expect("first");
        let second = generate(true, dir.path()).expect("second");
        assert_ne!(first, second);
    }

    #[test]
    fn the_key_is_appended_when_the_env_file_has_none() {
        let dir = project_with_env("APP_NAME=demo\nAPP_ENV=development\n");
        let key = generate(false, dir.path()).expect("generated");
        let env = std::fs::read_to_string(dir.path().join(".env")).expect("read");
        assert!(env.contains("APP_NAME=demo"), "{env}");
        assert!(env.contains(&format!("APP_KEY={key}")), "{env}");
    }

    #[test]
    fn regenerating_replaces_the_key_rather_than_adding_a_second_one() {
        let dir = project_with_env("APP_KEY=stale\nAPP_ENV=development\n");
        let key = generate(false, dir.path()).expect("generated");
        let env = std::fs::read_to_string(dir.path().join(".env")).expect("read");
        assert_eq!(env.matches("APP_KEY=").count(), 1, "{env}");
        assert!(!env.contains("stale"), "{env}");
        assert!(env.contains(&key));
        assert!(env.contains("APP_ENV=development"), "{env}");
    }

    #[test]
    fn show_prints_a_key_without_touching_the_env_file() {
        let dir = project_with_env("APP_NAME=demo\n");
        let key = generate(true, dir.path()).expect("generated");
        assert_eq!(key.len(), 128);
        let env = std::fs::read_to_string(dir.path().join(".env")).expect("read");
        assert_eq!(env, "APP_NAME=demo\n");
    }

    #[test]
    fn a_missing_env_file_explains_the_show_flag() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = generate(false, dir.path()).expect_err("no .env");
        assert!(matches!(error, KeyGenerateError::NoEnvFile { .. }));
        assert!(error.to_string().contains("--show"));
    }

    #[test]
    fn hex_encoding_round_trips_a_known_pattern() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xab, 0xff]), "000fabff");
    }
}
