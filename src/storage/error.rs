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
            Self::ControlChar => write!(formatter, "object key must not contain control characters"),
            Self::EmptySegment => {
                write!(formatter, "object key must not contain empty path segments")
            }
        }
    }
}

impl std::error::Error for StoragePathError {}

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
