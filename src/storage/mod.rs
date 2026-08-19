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
pub mod path;
pub mod store;

pub use config::{FsConfig, S3Config, StorageConfig};
pub use error::{StorageConfigError, StorageConnectError, StorageError, StoragePathError};
pub use path::StoragePath;
pub use store::{Disk, Storage, StorageBuilder};

// Re-export the certified OpenDAL and bytes crates so downstream code targets
// the Arcature-pinned versions.
pub use bytes;
pub use opendal;
