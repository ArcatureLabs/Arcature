//! Storage subsystem: object/file storage over OpenDAL, with a named-disk
//! registry and the `Storage::disk(name).put(...)` facade.
//!
//! This module owns the ergonomic boundary between an Arcature application
//! and object/file storage: a [`Storage`] facade over OpenDAL
//! [`opendal::Operator`]s, with a validated [`StoragePath`] object key, typed
//! errors, and resolved configuration.
//!
//! # What this module owns
//!
//! * A [`Storage`] facade wrapping a registry of named [`Disk`] handles, each
//!   an OpenDAL [`opendal::Operator`]. [`Storage::disk`] resolves a named
//!   disk; the data-path methods (`put`/`get`/`delete`/`exists`/`stat`/
//!   `list`/`copy`/`rename`/`reader`/`writer`) live on [`Disk`].
//! * A [`StoragePath`] validated object key that rejects path traversal,
//!   absolute paths, backslashes, control characters, and empty segments
//!   *before* any storage work runs.
//! * Resolved configuration: [`StorageConfig`] (selecting [`FsConfig`] or
//!   [`S3Config`]) -- accepted explicitly, credentials redacted.
//! * With the `uploads` feature, a `filename` sanitizer that turns a
//!   client-authored `filename=` parameter into a `SafeFilename` fit to keep
//!   as metadata, plus `StoragePath::from_filename` for the cases where that
//!   name is also used as a key.
//!
//! # What this module does not own
//!
//! It does not reimplement object-storage protocols, S3 signing, AWS
//! credential machinery, a multipart upload engine, TLS, or cryptography.
//! OpenDAL owns the protocol layer; the certified rustls + aws-lc-rs stack
//! owns TLS; Tokio owns the runtime.
//!
//! # Security note -- credentials are never logged
//!
//! [`S3Config`] implements `Debug` manually and redacts the access key id and
//! secret access key.

pub mod config;
pub mod error;
#[cfg(feature = "uploads")]
pub mod filename;
pub mod path;
pub mod store;

pub use config::{FsConfig, S3Config, StorageConfig};
#[cfg(feature = "uploads")]
pub use error::FilenameError;
pub use error::{StorageConfigError, StorageConnectError, StorageError, StoragePathError};
#[cfg(feature = "uploads")]
pub use filename::{AllowedExtensions, Extension, MAX_FILENAME_BYTES, SafeFilename};
pub use path::StoragePath;
pub use store::{Disk, Storage, StorageBuilder};

// Re-export the certified OpenDAL and bytes crates so downstream code targets
// the Arcature-pinned versions.
pub use bytes;
pub use opendal;
