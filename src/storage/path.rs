//! A validated object key for storage operations.
//!
//! [`StoragePath`] is the single path type that [`crate::storage::Storage`]
//! operations accept. It is constructed once, validated synchronously, and
//! then reused across any number of operations without re-validation. This is
//! the safety boundary that prevents path traversal, absolute-path escape,
//! and control characters from reaching the storage backend.

use std::fmt;

use crate::storage::error::StoragePathError;

/// A validated object key (path) for storage operations.
///
/// A `StoragePath` is a relative, forward-slash-separated key that is safe to
/// pass to any storage backend. Construction validates the key once; all
/// subsequent use is zero-cost -- [`StoragePath::as_str`] returns the
/// validated `&str`.
///
/// # What is rejected
///
/// * Empty strings.
/// * Keys beginning with `/` (absolute paths).
/// * Keys containing a `..` path segment (traversal).
/// * Keys containing a backslash.
/// * Keys containing ASCII control characters (U+0000-U+001F, U+007F).
/// * Keys containing empty segments (double slashes `//` or trailing `/`
///   followed by nothing).
///
/// # What is allowed
///
/// * Trailing slashes are permitted -- they are meaningful as list prefixes.
/// * Unicode characters of all kinds.
/// * Single dots that are part of a longer segment (e.g. `file.txt`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoragePath {
    key: String,
}

impl StoragePath {
    /// Construct a validated object key.
    ///
    /// # Errors
    ///
    /// Returns [`StoragePathError`] if the key fails any safety check.
    pub fn new(key: &str) -> Result<Self, StoragePathError> {
        validate(key)?;
        Ok(Self {
            key: key.to_owned(),
        })
    }

    /// The validated object key as a `&str`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.key
    }
}

#[cfg(feature = "uploads")]
impl StoragePath {
    /// Sanitize a client-authored filename into a single-segment object key.
    ///
    /// [`StoragePath::new`] validates and rejects: it has no opinion about how
    /// to *repair* a name, so `Ảnh chụp màn hình.png` -- an entirely ordinary
    /// filename -- passes it, while `photo (1).jpg:stream` does not, and a
    /// sanitizer that rejects real names teaches applications to bypass the
    /// sanitizer. This constructor runs the full
    /// [`SafeFilename`](crate::storage::SafeFilename) pipeline first: it
    /// discards directory components, normalizes to NFC, refuses control and
    /// bidi characters and Windows device names outright, replaces the
    /// characters that are merely dangerous downstream, and checks the
    /// extension against `allowed`.
    ///
    /// The result is always exactly one path segment. Nothing the client wrote
    /// can add a second one.
    ///
    /// # Prefer a content address
    ///
    /// A sanitized name is safe to *store*, but it is still a name the client
    /// chose, which means two users can collide on it and one can overwrite
    /// the other's object. Where the key does not have to be human-readable,
    /// derive it from the object's own bytes instead and keep the sanitized
    /// filename as metadata for the download header.
    ///
    /// # Errors
    ///
    /// Returns [`FilenameError`](crate::storage::FilenameError) if the name
    /// cannot be repaired into something safe: it is empty, over-long, holds a
    /// control or invisible formatting character, is a traversal component, is
    /// a reserved device name, or its extension is missing, malformed or not
    /// on the whitelist.
    ///
    /// # Example
    ///
    /// ```
    /// use arcature::storage::{AllowedExtensions, FilenameError, StoragePath};
    ///
    /// let images = AllowedExtensions::images();
    ///
    /// // An ordinary name survives, diacritics and spaces included.
    /// let key = StoragePath::from_filename("Ảnh chụp màn hình.PNG", &images)?;
    /// assert_eq!(key.as_str(), "Ảnh chụp màn hình.png");
    ///
    /// // A traversal payload loses its directory components, and what is left
    /// // still has to pass the whitelist.
    /// let key = StoragePath::from_filename("../../etc/passwd.png", &images)?;
    /// assert_eq!(key.as_str(), "passwd.png");
    ///
    /// // Executables are not on the whitelist, whatever they are called.
    /// assert_eq!(
    ///     StoragePath::from_filename("shell.php", &images),
    ///     Err(FilenameError::ExtensionNotAllowed)
    /// );
    /// # Ok::<(), FilenameError>(())
    /// ```
    pub fn from_filename(
        filename: &str,
        allowed: &crate::storage::filename::AllowedExtensions,
    ) -> Result<Self, crate::storage::error::FilenameError> {
        let safe = crate::storage::filename::SafeFilename::parse(filename, allowed)?;
        // `SafeFilename` is one segment of NFC text with no separator, no
        // control character and a non-empty stem, so the key checks below
        // cannot fail. The call is kept rather than asserted away so that the
        // two validators can never silently drift apart.
        Self::new(&safe.to_string()).map_err(|_| crate::storage::error::FilenameError::Empty)
    }
}

impl fmt::Display for StoragePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.key)
    }
}

impl AsRef<str> for StoragePath {
    fn as_ref(&self) -> &str {
        &self.key
    }
}

/// Validate an object key against all safety checks.
///
/// A single trailing slash is permitted (list prefix). Empty segments
/// *within* the path (double slashes `//`) are rejected.
fn validate(key: &str) -> Result<(), StoragePathError> {
    if key.is_empty() {
        return Err(StoragePathError::Empty);
    }
    if key.starts_with('/') {
        return Err(StoragePathError::Absolute);
    }
    if key.contains('\\') {
        return Err(StoragePathError::Backslash);
    }
    if key.bytes().any(|b| b < 0x20 || b == 0x7F) {
        return Err(StoragePathError::ControlChar);
    }
    // A single trailing slash (list prefix) is allowed. Strip it before
    // checking for empty *internal* segments.
    let trimmed = key.strip_suffix('/').unwrap_or(key);
    for segment in trimmed.split('/') {
        if segment == ".." {
            return Err(StoragePathError::Traversal);
        }
        if segment.is_empty() {
            return Err(StoragePathError::EmptySegment);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_key() {
        assert!(StoragePath::new("hello.txt").is_ok());
    }

    #[test]
    fn accepts_nested_key() {
        assert!(StoragePath::new("photos/2026/img-001.jpg").is_ok());
    }

    #[test]
    fn accepts_trailing_slash_for_list_prefix() {
        assert!(StoragePath::new("photos/").is_ok());
    }

    #[test]
    fn as_str_roundtrips() {
        let key = "a/b/c.txt";
        let path = StoragePath::new(key).unwrap();
        assert_eq!(path.as_str(), key);
        assert_eq!(path.to_string(), key);
        assert_eq!(path.as_ref(), key);
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(StoragePath::new("").unwrap_err(), StoragePathError::Empty);
    }

    #[test]
    fn rejects_absolute() {
        assert_eq!(
            StoragePath::new("/etc/passwd").unwrap_err(),
            StoragePathError::Absolute
        );
    }

    #[test]
    fn rejects_traversal_leading() {
        assert_eq!(
            StoragePath::new("../secret").unwrap_err(),
            StoragePathError::Traversal
        );
    }

    #[test]
    fn rejects_traversal_middle() {
        assert_eq!(
            StoragePath::new("foo/../bar").unwrap_err(),
            StoragePathError::Traversal
        );
    }

    #[test]
    fn rejects_backslash() {
        assert_eq!(
            StoragePath::new("foo\\bar").unwrap_err(),
            StoragePathError::Backslash
        );
    }

    #[test]
    fn rejects_control_chars() {
        assert_eq!(
            StoragePath::new("foo\0bar").unwrap_err(),
            StoragePathError::ControlChar
        );
    }

    #[test]
    fn rejects_double_slash() {
        assert_eq!(
            StoragePath::new("foo//bar").unwrap_err(),
            StoragePathError::EmptySegment
        );
    }

    #[test]
    fn hostile_input_never_panics() {
        let hostile = [
            "",
            "/",
            "//",
            "../",
            "/..",
            "..\\..",
            "foo\0",
            "foo/../../../etc/passwd",
            "\x7F",
            "foo//bar//baz",
        ];
        for input in hostile {
            let result = StoragePath::new(input);
            assert!(result.is_err(), "expected rejection for {input:?}");
        }
    }
}
