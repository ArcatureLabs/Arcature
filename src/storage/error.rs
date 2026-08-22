//! Storage subsystem error types.

use std::fmt;

/// Configuration validation failure for [`crate::storage::StorageConfig`].
#[derive(Debug)]
pub enum StorageConfigError {
    /// The filesystem `root` is empty or whitespace-only.
    EmptyRoot,
    /// The S3 `bucket` is empty or whitespace-only.
    EmptyBucket,
}

impl fmt::Display for StorageConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRoot => write!(formatter, "filesystem root must not be empty"),
            Self::EmptyBucket => write!(formatter, "s3 bucket must not be empty"),
        }
    }
}

impl std::error::Error for StorageConfigError {}

/// Failure from a [`crate::storage::Storage`] data-path operation.
///
/// The upstream [`opendal::Error`] is preserved in the [`StorageError::Backend`]
/// variant for source chaining and `ErrorKind` inspection.
#[derive(Debug)]
pub enum StorageError {
    /// The upstream storage backend rejected the operation.
    Backend {
        /// The upstream OpenDAL error.
        source: opendal::Error,
    },
    /// A path that was expected to be valid failed validation.
    Path {
        /// The path-validation error.
        source: StoragePathError,
    },
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend { source } => write!(formatter, "storage operation failed: {source}"),
            Self::Path { source } => write!(formatter, "invalid storage path: {source}"),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend { source } => Some(source),
            Self::Path { source } => Some(source),
        }
    }
}

impl From<opendal::Error> for StorageError {
    fn from(source: opendal::Error) -> Self {
        Self::Backend { source }
    }
}

impl From<StoragePathError> for StorageError {
    fn from(source: StoragePathError) -> Self {
        Self::Path { source }
    }
}

/// Path validation failure for [`crate::storage::StoragePath`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoragePathError {
    /// The path is the empty string.
    Empty,
    /// The path begins with `/`.
    Absolute,
    /// The path contains a `..` path segment.
    Traversal,
    /// The path contains a backslash.
    Backslash,
    /// The path contains an ASCII control character.
    ControlChar,
    /// The path contains an empty segment.
    EmptySegment,
}

impl fmt::Display for StoragePathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "object key must not be empty"),
            Self::Absolute => write!(formatter, "object key must not start with '/'"),
            Self::Traversal => {
                write!(formatter, "object key must not contain a '..' path segment")
            }
            Self::Backslash => write!(formatter, "object key must not contain a backslash"),
            Self::ControlChar => {
                write!(formatter, "object key must not contain control characters")
            }
            Self::EmptySegment => {
                write!(formatter, "object key must not contain empty path segments")
            }
        }
    }
}

impl std::error::Error for StoragePathError {}

/// Filename sanitization failure for
/// [`SafeFilename::parse`](crate::storage::SafeFilename::parse) and its
/// helpers.
///
/// This is deliberately a separate enum from [`StoragePathError`] rather than
/// more variants on it. The two answer different questions -- "is this key
/// safe to resolve?" versus "is this client-authored label safe to keep?" --
/// and a caller that wants to tell an uploader *why* their file was refused
/// needs the second vocabulary, not the first.
#[cfg(feature = "uploads")]
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilenameError {
    /// The filename was empty, whitespace-only, or nothing but directory
    /// components.
    Empty,
    /// The filename was longer than the sanitizer is willing to look at.
    TooLong,
    /// The filename contained a control, bidi or other invisible format
    /// character. These are never repaired: their presence is the attack.
    ControlChar,
    /// The filename was `.` or `..` once its directory components were
    /// discarded.
    Traversal,
    /// The filename had no extension, and the extension is what the whitelist
    /// is checked against.
    MissingExtension,
    /// The extension was empty, over-long, or not ASCII alphanumeric.
    InvalidExtension,
    /// The extension is well-formed but not on the caller's whitelist.
    ExtensionNotAllowed,
    /// Everything before the extension was removed by sanitization, leaving no
    /// name at all.
    EmptyStem,
    /// The name is a Windows reserved device name such as `CON` or `NUL`.
    /// Opening it opens a device, not a file.
    ReservedName,
}

#[cfg(feature = "uploads")]
impl fmt::Display for FilenameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "filename must not be empty"),
            Self::TooLong => write!(formatter, "filename is too long"),
            Self::ControlChar => write!(
                formatter,
                "filename must not contain control or invisible formatting characters"
            ),
            Self::Traversal => {
                write!(formatter, "filename must not be a path traversal component")
            }
            Self::MissingExtension => write!(formatter, "filename must have a file extension"),
            Self::InvalidExtension => write!(
                formatter,
                "file extension must be 1 to 16 ASCII alphanumeric characters"
            ),
            Self::ExtensionNotAllowed => write!(formatter, "file extension is not allowed"),
            Self::EmptyStem => write!(formatter, "filename must have a name before its extension"),
            Self::ReservedName => write!(formatter, "filename is a reserved device name"),
        }
    }
}

#[cfg(feature = "uploads")]
impl std::error::Error for FilenameError {}

/// Failure from [`crate::storage::Storage::connect`].
#[derive(Debug)]
pub enum StorageConnectError {
    /// Configuration validation failed before any backend construction.
    Config {
        /// The specific configuration error.
        source: StorageConfigError,
    },
    /// The OpenDAL operator could not be built.
    Backend {
        /// The upstream OpenDAL error.
        source: opendal::Error,
    },
}

impl fmt::Display for StorageConnectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config { source } => write!(formatter, "storage configuration invalid: {source}"),
            Self::Backend { source } => {
                write!(formatter, "storage backend construction failed: {source}")
            }
        }
    }
}

impl std::error::Error for StorageConnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config { source } => Some(source),
            Self::Backend { source } => Some(source),
        }
    }
}

impl From<StorageConfigError> for StorageConnectError {
    fn from(source: StorageConfigError) -> Self {
        Self::Config { source }
    }
}

impl From<opendal::Error> for StorageConnectError {
    fn from(source: opendal::Error) -> Self {
        Self::Backend { source }
    }
}
