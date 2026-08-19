//! Validation shared by enqueue and the registry so the queue and the
//! handlers agree on what a valid job identity is.

/// The maximum length of a job kind string, in bytes.
pub const KIND_MAX_LEN: usize = 128;

/// Validate a job kind string.
///
/// A kind must be non-empty, at most 128 bytes, and contain only ASCII
/// `[a-zA-Z0-9_.:-]`. These are safe, low-cardinality values to store and
/// index.
pub fn validate_kind(kind: &str) -> Result<(), String> {
    if kind.is_empty() {
        return Err("kind must not be empty".to_string());
    }
    if kind.len() > KIND_MAX_LEN {
        return Err(format!("kind must not exceed {KIND_MAX_LEN} bytes"));
    }
    if !kind.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b':' | b'_' | b'-')) {
        return Err("kind must contain only ASCII alphanumeric, '.', ':', '_', or '-'".to_string());
    }
    Ok(())
}

/// Validate a payload version. Must be >= 1 (version 0 is rejected).
pub fn validate_version(version: i16) -> Result<(), String> {
    if version < 1 {
        return Err(format!("version must be >= 1, got {version}"));
    }
    Ok(())
}
