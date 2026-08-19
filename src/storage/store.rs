//! The [`Storage`] handle: an ergonomic boundary over an OpenDAL
//! [`opendal::Operator`], with a named-disk registry and the
//! `Storage::disk(name).put(...)` facade.
//!
//! `Storage` holds a registry of named disks (each an OpenDAL `Operator`),
//! plus typed operations: `put`, `get`, `delete`, `exists`, `stat`, `list`,
//! `copy`, `rename`, and streaming `reader`/`writer`.
//!
//! `Storage` is `Clone + Send + Sync + 'static` so it works as normal Axum
//! state.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;

use crate::storage::config::StorageConfig;
use crate::storage::error::{StorageConnectError, StorageError};
use crate::storage::path::StoragePath;

/// The Arcature storage facade: a registry of named disks over OpenDAL
/// operators.
///
/// Build a `Storage` with [`Storage::builder`], registering one or more named
/// disks via [`StorageBuilder::disk`]. Each disk is built from a resolved
/// [`StorageConfig`] (filesystem or S3-compatible) on `connect`. The default
/// disk (registered first or named with [`StorageBuilder::default_disk`]) is
/// returned by [`Storage::default_disk`]; any registered disk is returned by
/// [`Storage::disk`].
///
/// The data-path methods live on [`Disk`], the handle returned by
/// [`Storage::disk`]. All paths are validated through [`StoragePath`].
///
/// # Example
///
/// ```ignore
/// let storage = Storage::builder()
///     .disk("local", StorageConfig::fs("/var/lib/myapp/storage")?)
///     .disk("s3", StorageConfig::s3(S3Config::new("my-bucket")?))
///     .default_disk("local")
///     .connect()
///     .await?;
///
/// let path = StoragePath::new("photos/img.jpg")?;
/// storage.disk("local").put(&path, b"hello").await?;
/// let bytes = storage.disk("local").get(&path).await?;
/// ```
#[derive(Clone)]
pub struct Storage {
    inner: Arc<StorageInner>,
}

struct StorageInner {
    disks: HashMap<String, Disk>,
    default_name: String,
}

/// A handle to a single named storage disk, with typed data-path operations.
///
/// Constructed by [`Storage::disk`] or [`Storage::default_disk`]. Cloning is
/// cheap (the underlying [`opendal::Operator`] is `Arc`-backed).
#[derive(Clone)]
pub struct Disk {
    pub(crate) operator: opendal::Operator,
}

impl Storage {
    /// Start a builder for a multi-disk storage facade.
    #[must_use]
    pub fn builder() -> StorageBuilder {
        StorageBuilder {
            disks: Vec::new(),
            default_name: None,
        }
    }

    /// Build a `Storage` from a single resolved configuration, registered as
    /// the default disk named `"default"`. This is the convenience path for
    /// applications that use a single storage backend.
    ///
    /// # Errors
    ///
    /// Returns [`StorageConnectError`] if the configuration is invalid or the
    /// backend cannot be built.
    pub async fn connect(config: StorageConfig) -> Result<Storage, StorageConnectError> {
        config.validate()?;
        let operator = build_operator(&config).await?;
        let mut disks = HashMap::new();
        disks.insert("default".to_string(), Disk { operator });
        Ok(Storage {
            inner: Arc::new(StorageInner {
                disks,
                default_name: "default".to_string(),
            }),
        })
    }

    /// Get a named disk. Panics if the disk name was not registered. Use
    /// [`Storage::try_disk`] for a fallible lookup.
    #[must_use]
    pub fn disk(&self, name: &str) -> Disk {
        self.try_disk(name)
            .unwrap_or_else(|| panic!("storage disk `{name}` is not registered"))
    }

    /// Get a named disk, or `None` if the name was not registered.
    #[must_use]
    pub fn try_disk(&self, name: &str) -> Option<Disk> {
        self.inner.disks.get(name).cloned()
    }

    /// Get the default disk (the first registered disk, or the one named via
    /// [`StorageBuilder::default_disk`]).
    #[must_use]
    pub fn default_disk(&self) -> Disk {
        self.disk(&self.inner.default_name)
    }

    /// The registered disk names.
    #[must_use]
    pub fn disk_names(&self) -> Vec<&str> {
        self.inner.disks.keys().map(String::as_str).collect()
    }
}

impl Disk {
    /// Construct a `Disk` from a configured OpenDAL [`Operator`]. This is the
    /// escape hatch for experts who configure the operator themselves.
    #[must_use]
    pub fn from_operator(operator: opendal::Operator) -> Self {
        Self { operator }
    }

    /// Get the underlying [`opendal::Operator`] for raw access. Paths passed
    /// to the raw operator are **not** validated by Arcature.
    #[must_use]
    pub fn operator(&self) -> &opendal::Operator {
        &self.operator
    }

    /// Write an entire object from a byte buffer. This is the `put` facade
    /// method.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] wrapping the upstream OpenDAL error.
    pub async fn put(&self, path: &StoragePath, data: &[u8]) -> Result<(), StorageError> {
        self.operator
            .write(path.as_str(), opendal::Buffer::from(data.to_vec()))
            .await?;
        Ok(())
    }

    /// Read an entire object into a [`Bytes`] buffer. This is the `get` facade
    /// method.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] wrapping the upstream OpenDAL error
    /// (e.g. `NotFound` if the object does not exist).
    pub async fn get(&self, path: &StoragePath) -> Result<Bytes, StorageError> {
        let buffer = self.operator.read(path.as_str()).await?;
        Ok(buffer.to_bytes())
    }

    /// Delete an object. Idempotent: deleting a non-existent object is not an
    /// error.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] wrapping the upstream OpenDAL error
    /// for genuine failures.
    pub async fn delete(&self, path: &StoragePath) -> Result<(), StorageError> {
        self.operator.delete(path.as_str()).await?;
        Ok(())
    }

    /// Check whether an object exists.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] wrapping the upstream OpenDAL error
    /// for genuine failures. A missing object returns `Ok(false)`.
    pub async fn exists(&self, path: &StoragePath) -> Result<bool, StorageError> {
        let exists = self.operator.exists(path.as_str()).await?;
        Ok(exists)
    }

    /// Stat an object: return its [`opendal::Metadata`] (size, type, etag).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] wrapping the upstream OpenDAL error.
    pub async fn stat(&self, path: &StoragePath) -> Result<opendal::Metadata, StorageError> {
        let meta = self.operator.stat(path.as_str()).await?;
        Ok(meta)
    }

    /// List the entries directly under a path. `path` should typically end
    /// with `/` (e.g. `"photos/"`) to list the contents of a "directory".
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] wrapping the upstream OpenDAL error.
    pub async fn list(&self, path: &StoragePath) -> Result<Vec<opendal::Entry>, StorageError> {
        use futures::TryStreamExt;
        let lister = self.operator.lister(path.as_str()).await?;
        let mut entries = Vec::new();
        let mut lister = lister;
        while let Some(entry) = lister.try_next().await? {
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Open a streaming [`opendal::Reader`] for an object (range reads without
    /// buffering the whole object into memory).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] wrapping the upstream OpenDAL error.
    pub async fn reader(&self, path: &StoragePath) -> Result<opendal::Reader, StorageError> {
        let reader = self.operator.reader(path.as_str()).await?;
        Ok(reader)
    }

    /// Open a streaming [`opendal::Writer`] for an object. **Always call
    /// [`opendal::Writer::close`] when done** -- for S3-compatible backends
    /// the object is not complete until the writer is closed.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] wrapping the upstream OpenDAL error.
    pub async fn writer(&self, path: &StoragePath) -> Result<opendal::Writer, StorageError> {
        let writer = self.operator.writer(path.as_str()).await?;
        Ok(writer)
    }

    /// Copy an object to a new key.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] wrapping the upstream OpenDAL error.
    pub async fn copy(&self, from: &StoragePath, to: &StoragePath) -> Result<(), StorageError> {
        self.operator.copy(from.as_str(), to.as_str()).await?;
        Ok(())
    }

    /// Rename (move) an object to a new key.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] wrapping the upstream OpenDAL error.
    pub async fn rename(&self, from: &StoragePath, to: &StoragePath) -> Result<(), StorageError> {
        self.operator.rename(from.as_str(), to.as_str()).await?;
        Ok(())
    }
}

/// Build the OpenDAL [`opendal::Operator`] for the selected backend.
async fn build_operator(config: &StorageConfig) -> Result<opendal::Operator, opendal::Error> {
    match config {
        #[cfg(feature = "storage-fs")]
        StorageConfig::Fs(fs) => {
            let builder = fs.clone().into_builder();
            Operator::new(builder)
        }
        #[cfg(feature = "storage-s3")]
        StorageConfig::S3(s3) => {
            opendal::install_default();
            let builder = s3.clone().into_builder();
            Operator::new(builder)
        }
        #[allow(unreachable_patterns)]
        _ => unreachable!("no storage backend feature is enabled"),
    }
}

// Re-export OpenDAL's Operator::new via the right name for the match arms.
#[cfg(any(feature = "storage-fs", feature = "storage-s3"))]
use opendal::Operator;

/// Builder for [`Storage`], registering named disks.
pub struct StorageBuilder {
    disks: Vec<(String, StorageConfig)>,
    default_name: Option<String>,
}

impl StorageBuilder {
    /// Register a named disk with its resolved configuration. The first
    /// registered disk is the default unless overridden by
    /// [`StorageBuilder::default_disk`].
    #[must_use]
    pub fn disk(mut self, name: impl Into<String>, config: StorageConfig) -> Self {
        self.disks.push((name.into(), config));
        self
    }

    /// Name the default disk. Must match a disk registered via
    /// [`StorageBuilder::disk`].
    #[must_use]
    pub fn default_disk(mut self, name: impl Into<String>) -> Self {
        self.default_name = Some(name.into());
        self
    }

    /// Build the [`Storage`] facade, connecting every registered disk.
    ///
    /// # Errors
    ///
    /// Returns [`StorageConnectError`] if any disk's configuration is invalid
    /// or its backend cannot be built.
    pub async fn connect(self) -> Result<Storage, StorageConnectError> {
        if self.disks.is_empty() {
            return Err(StorageConnectError::Config {
                source: crate::storage::error::StorageConfigError::EmptyRoot,
            });
        }
        let default_name = self
            .default_name
            .unwrap_or_else(|| self.disks[0].0.clone());
        let mut disks = HashMap::new();
        for (name, config) in self.disks {
            config.validate()?;
            let operator = build_operator(&config).await?;
            disks.insert(name, Disk { operator });
        }
        if !disks.contains_key(&default_name) {
            return Err(StorageConnectError::Config {
                source: crate::storage::error::StorageConfigError::EmptyRoot,
            });
        }
        Ok(Storage {
            inner: Arc::new(StorageInner {
                disks,
                default_name,
            }),
        })
    }
}
