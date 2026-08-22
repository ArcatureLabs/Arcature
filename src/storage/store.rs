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
/// ```no_run
/// use arcature::storage::{Storage, StorageConfig, StoragePath};
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// // A second S3 disk would be `.disk("s3", StorageConfig::s3(..))`, which
/// // needs the non-default `storage-s3` feature.
/// let storage = Storage::builder()
///     .disk("local", StorageConfig::fs("/var/lib/myapp/storage")?)
///     .default_disk("local")
///     .connect()
///     .await?;
///
/// // Every path is validated before it reaches a backend.
/// let path = StoragePath::new("photos/img.jpg")?;
/// storage.disk("local").put(&path, b"hello").await?;
/// let bytes = storage.disk("local").get(&path).await?;
/// # let _ = bytes;
/// # Ok(())
/// # }
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

/// The key prefix every in-flight upload is written under.
///
/// One directory, so a disk's real content is not interleaved with half-
/// written objects, and an operator can see at a glance whether anything was
/// left behind by a process that died mid-upload.
#[cfg(feature = "uploads")]
pub const STAGING_PREFIX: &str = "_staging";

/// A monotonically increasing part of the staging key, so two uploads that
/// start inside the same nanosecond still get different keys.
#[cfg(feature = "uploads")]
static STAGING_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A unique key for one in-flight upload.
///
/// Process id, wall-clock nanoseconds and a process-local counter. Uniqueness
/// is the whole requirement -- the key is transient, lives under
/// [`STAGING_PREFIX`], and is never derived from anything the client sent, so
/// there is nothing here for a request to influence.
#[cfg(feature = "uploads")]
fn staging_path() -> StoragePath {
    let counter = STAGING_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let pid = std::process::id();
    StoragePath::new(&format!(
        "{STAGING_PREFIX}/{pid:08x}-{nanos:032x}-{counter:016x}.part"
    ))
    .expect("a hex-only key under a fixed ASCII prefix is always a valid object key")
}

#[cfg(feature = "uploads")]
impl Disk {
    /// Begin a streaming upload whose object key will be its own SHA-256.
    ///
    /// Chunks are written through [`Disk::writer`] as they arrive and hashed
    /// on the way past, so the whole object is never resident. Because the
    /// key is not known until the last byte has been seen, the bytes go to a
    /// unique key under [`STAGING_PREFIX`] first and are moved onto the
    /// content-addressed key by [`UploadWriter::finish`].
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] if the staging object cannot be
    /// opened for writing.
    pub async fn begin_upload(&self) -> Result<UploadWriter, StorageError> {
        self.open_upload(None).await
    }

    /// As [`Disk::begin_upload`], with the finished object placed under an
    /// application-chosen prefix (`avatars/ab/cd/<digest>.<ext>`).
    ///
    /// The prefix is the application's own string -- never anything that came
    /// off the request -- and it is validated here, before a byte is written,
    /// rather than after the whole body has been streamed to a key that
    /// turns out to have nowhere to go.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Path`] if `prefix` is not a valid relative
    /// key, and [`StorageError::Backend`] if the staging object cannot be
    /// opened.
    pub async fn begin_upload_under(&self, prefix: &str) -> Result<UploadWriter, StorageError> {
        let prefix = prefix.trim_end_matches('/');
        StoragePath::new(prefix)?;
        self.open_upload(Some(prefix.to_string())).await
    }

    async fn open_upload(&self, prefix: Option<String>) -> Result<UploadWriter, StorageError> {
        let staging = staging_path();
        let writer = self.writer(&staging).await?;
        Ok(UploadWriter {
            disk: self.clone(),
            staging,
            prefix,
            writer,
            hasher: crate::storage::content::ContentHasher::new(),
            head: Vec::new(),
        })
    }
}

/// An upload in flight: bytes go to the disk as they arrive, and the SHA-256
/// of what has gone past is kept beside them.
///
/// Created by [`Disk::begin_upload`]. Feed it with [`UploadWriter::write`],
/// then either [`UploadWriter::finish`] (close, then move the object onto its
/// content-addressed key) or [`UploadWriter::abort`] (drop the staging
/// object).
///
/// **Neither is optional.** Dropping an `UploadWriter` without calling one of
/// them leaves the staging object behind, and on an S3-compatible backend
/// leaves the multipart upload un-closed -- there is no `Drop` that can fix
/// that, because both fixes are `async`.
///
/// # Example
///
/// ```no_run
/// use arcature::storage::{Extension, Storage, StorageConfig};
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let storage = Storage::connect(StorageConfig::fs("/var/lib/myapp/storage")?).await?;
/// let mut upload = storage.default_disk().begin_upload_under("avatars").await?;
///
/// // In a real handler these chunks come from a `BoundedField`.
/// for chunk in [&b"\x89PNG\r\n\x1a\n"[..], b"...."] {
///     upload.write(chunk).await?;
/// }
///
/// let address = upload.finish(Extension::parse("png")?).await?;
/// println!("stored {} bytes at {}", address.byte_len(), address.path().as_str());
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "uploads")]
pub struct UploadWriter {
    disk: Disk,
    staging: StoragePath,
    prefix: Option<String>,
    writer: opendal::Writer,
    hasher: crate::storage::content::ContentHasher,
    head: Vec<u8>,
}

#[cfg(feature = "uploads")]
impl std::fmt::Debug for UploadWriter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UploadWriter")
            .field("staging", &self.staging)
            .field("prefix", &self.prefix)
            .field("byte_len", &self.hasher.byte_len())
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "uploads")]
impl UploadWriter {
    /// The transient key the bytes are being written to.
    #[must_use]
    pub fn staging_path(&self) -> &StoragePath {
        &self.staging
    }

    /// How many bytes have been written so far.
    #[must_use]
    pub fn byte_len(&self) -> u64 {
        self.hasher.byte_len()
    }

    /// Write the next chunk.
    ///
    /// The chunk goes to the backend and to the hasher, and is then dropped.
    /// Nothing accumulates: an upload of any size costs one chunk of memory,
    /// which is what makes the size caps in
    /// [`MultipartLimits`](crate::http::multipart::MultipartLimits) a policy
    /// rather than the only thing standing between a request and the heap.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] wrapping the upstream OpenDAL error.
    pub async fn write(&mut self, chunk: impl Into<bytes::Bytes>) -> Result<(), StorageError> {
        let chunk = chunk.into();
        self.hasher.update(&chunk);
        if self.head.len() < crate::storage::sniff::SNIFF_BYTES {
            let room = crate::storage::sniff::SNIFF_BYTES - self.head.len();
            self.head.extend_from_slice(&chunk[..chunk.len().min(room)]);
        }
        self.writer.write(chunk).await?;
        Ok(())
    }

    /// Close the object and move it onto its content-addressed key.
    ///
    /// The close is what completes the object on an S3-compatible backend,
    /// and it happens before the move, so a failed upload never leaves a
    /// truncated object under a digest that does not describe it.
    ///
    /// If an object already exists at the destination the staging copy is
    /// deleted instead of moved. That is not an optimisation: the key *is*
    /// the digest, so an object already there has exactly these bytes, and
    /// overwriting it would be a no-op that can still fail -- on Windows a
    /// rename over a file another reader has open does.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] if the close, the existence check,
    /// the move or the cleanup fails, and [`StorageError::Path`] if the
    /// prefix and the digest do not form a valid key.
    pub async fn finish(
        mut self,
        extension: crate::storage::filename::Extension,
    ) -> Result<crate::storage::content::ContentAddress, StorageError> {
        self.writer.close().await?;

        let address = self.hasher.finish(extension);
        let destination = match &self.prefix {
            Some(prefix) => address.path_under(prefix)?,
            None => address.path(),
        };

        if self.disk.exists(&destination).await? {
            self.disk.delete(&self.staging).await?;
        } else {
            self.disk.rename(&self.staging, &destination).await?;
        }
        Ok(address)
    }

    /// Discard the upload and remove the staging object.
    ///
    /// The right call on any rejection after the first byte -- a bound
    /// exceeded, an extension refused, a sniffed type that disagrees with the
    /// declared one.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] if the staging object cannot be
    /// deleted. The backend's own `abort` is attempted first and its failure
    /// is deliberately ignored: a filesystem disk does not implement one, and
    /// the delete is what actually removes the bytes on every backend.
    pub async fn abort(mut self) -> Result<(), StorageError> {
        let _ = self.writer.abort().await;
        self.disk.delete(&self.staging).await?;
        Ok(())
    }

    /// The object's leading bytes, at most
    /// [`SNIFF_BYTES`](crate::storage::sniff::SNIFF_BYTES) of them.
    ///
    /// This buffer is the only part of an upload that is ever held in memory,
    /// and its size does not depend on the size of the object.
    #[must_use]
    pub fn head(&self) -> &[u8] {
        &self.head
    }

    /// What the object's leading bytes were recognized as, if anything.
    ///
    /// A statement about the bytes, unlike the part's `Content-Type` and
    /// unlike its filename, both of which the client wrote.
    #[must_use]
    pub fn sniffed(&self) -> Option<crate::storage::sniff::SniffedType> {
        crate::storage::sniff::sniff(&self.head)
    }

    /// Whether the bytes seen so far agree with `extension`.
    ///
    /// Answerable from the first chunk, so a caller that wants to stop
    /// reading a hostile body early can ask before the last byte arrives.
    ///
    /// # Errors
    ///
    /// Returns [`SniffError`](crate::storage::SniffError) when the bytes and
    /// the extension disagree, in either direction.
    pub fn verify(
        &self,
        extension: &crate::storage::filename::Extension,
    ) -> Result<Option<crate::storage::sniff::SniffedType>, crate::storage::error::SniffError> {
        crate::storage::sniff::verify(&self.head, extension)
    }

    /// [`UploadWriter::finish`], refusing to keep an object whose bytes
    /// disagree with `extension`.
    ///
    /// This is the call an upload handler wants. On disagreement the staging
    /// object is removed before the error is returned, so a rejected upload
    /// costs no disk -- the ordering matters, because the obvious version of
    /// this (verify, return early, let the writer drop) leaves the bytes
    /// behind exactly when the caller least wants them kept.
    ///
    /// The check is on the leading bytes, so it is a statement about format,
    /// never about safety: nothing here decodes the object.
    ///
    /// # Errors
    ///
    /// Returns [`UploadError::Content`](crate::storage::UploadError::Content)
    /// if the bytes and the extension disagree, and
    /// [`UploadError::Storage`](crate::storage::UploadError::Storage) if the
    /// backend fails -- kept apart because the first is a 4xx and the second
    /// is a 5xx.
    pub async fn finish_verified(
        self,
        extension: crate::storage::filename::Extension,
    ) -> Result<crate::storage::content::ContentAddress, crate::storage::error::UploadError> {
        if let Err(rejection) = self.verify(&extension) {
            self.abort().await?;
            return Err(rejection.into());
        }
        Ok(self.finish(extension).await?)
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
        let default_name = self.default_name.unwrap_or_else(|| self.disks[0].0.clone());
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

#[cfg(all(test, feature = "uploads"))]
mod upload_tests {
    use super::*;

    use crate::storage::content::ContentAddress;
    use crate::storage::filename::Extension;

    fn png() -> Extension {
        Extension::parse("png").expect("png is a valid extension")
    }

    /// A filesystem disk rooted in a directory that dies with the test.
    async fn disk() -> (tempfile::TempDir, Disk) {
        let root = tempfile::tempdir().expect("a temporary directory");
        let config = StorageConfig::fs(root.path().to_string_lossy().into_owned())
            .expect("the temporary path is a valid root");
        let storage = Storage::connect(config).await.expect("the disk connects");
        let disk = storage.default_disk();
        (root, disk)
    }

    /// How many objects the disk holds under `prefix`.
    async fn count_under(disk: &Disk, prefix: &str) -> usize {
        let path = StoragePath::new(prefix).expect("a valid list prefix");
        match disk.list(&path).await {
            Ok(entries) => entries
                .iter()
                .filter(|entry| !entry.path().ends_with('/'))
                .count(),
            // A prefix that was never written does not exist on a filesystem
            // disk, and "no objects" is the answer either way.
            Err(_) => 0,
        }
    }

    #[tokio::test]
    async fn a_streamed_upload_lands_on_its_content_addressed_key() {
        let (_root, disk) = disk().await;

        let mut upload = disk.begin_upload().await.expect("the upload opens");
        for chunk in [&b"a"[..], b"b", b"c"] {
            upload.write(chunk).await.expect("the chunk is written");
        }
        assert_eq!(upload.byte_len(), 3);

        let address = upload.finish(png()).await.expect("the upload finishes");

        // The same key the one-shot hasher would have produced.
        assert_eq!(address, ContentAddress::of(b"abc", png()));
        assert_eq!(
            disk.get(&address.path())
                .await
                .expect("the object is there"),
            bytes::Bytes::from_static(b"abc")
        );
    }

    #[tokio::test]
    async fn the_staging_object_does_not_survive_the_upload() {
        let (_root, disk) = disk().await;

        let mut upload = disk.begin_upload().await.expect("the upload opens");
        let staging = upload.staging_path().clone();
        upload
            .write(&b"abc"[..])
            .await
            .expect("the chunk is written");
        upload.finish(png()).await.expect("the upload finishes");

        assert!(!disk.exists(&staging).await.expect("the check runs"));
        assert_eq!(count_under(&disk, "_staging/").await, 0);
    }

    #[tokio::test]
    async fn an_aborted_upload_leaves_nothing_behind() {
        let (_root, disk) = disk().await;

        let mut upload = disk.begin_upload().await.expect("the upload opens");
        let staging = upload.staging_path().clone();
        upload
            .write(&b"partial"[..])
            .await
            .expect("the chunk is written");
        upload.abort().await.expect("the abort succeeds");

        assert!(!disk.exists(&staging).await.expect("the check runs"));
        assert_eq!(count_under(&disk, "_staging/").await, 0);
    }

    #[tokio::test]
    async fn the_same_bytes_twice_occupy_one_object() {
        let (_root, disk) = disk().await;

        let mut first = disk.begin_upload().await.expect("the upload opens");
        first.write(&b"abc"[..]).await.expect("written");
        let one = first.finish(png()).await.expect("the upload finishes");

        let mut second = disk.begin_upload().await.expect("the upload opens");
        second.write(&b"abc"[..]).await.expect("written");
        let two = second.finish(png()).await.expect("the upload finishes");

        assert_eq!(one, two);
        // The second upload's staging copy was dropped, not moved over the
        // first, and nothing was left in the staging directory either way.
        assert_eq!(count_under(&disk, "_staging/").await, 0);
        assert_eq!(
            disk.get(&one.path()).await.expect("the object is there"),
            bytes::Bytes::from_static(b"abc")
        );
    }

    #[tokio::test]
    async fn an_application_prefix_is_honoured() {
        let (_root, disk) = disk().await;

        let mut upload = disk
            .begin_upload_under("avatars/")
            .await
            .expect("the upload opens");
        upload.write(&b"abc"[..]).await.expect("written");
        let address = upload.finish(png()).await.expect("the upload finishes");

        let path = address.path_under("avatars").expect("a valid key");
        assert!(path.as_str().starts_with("avatars/ba/78/"));
        assert!(disk.exists(&path).await.expect("the check runs"));
    }

    #[tokio::test]
    async fn a_hostile_prefix_is_refused_before_a_byte_is_written() {
        let (_root, disk) = disk().await;

        assert!(matches!(
            disk.begin_upload_under("../../etc").await,
            Err(StorageError::Path { .. })
        ));
        assert!(matches!(
            disk.begin_upload_under("/absolute").await,
            Err(StorageError::Path { .. })
        ));
        // Nothing was opened, so nothing was staged.
        assert_eq!(count_under(&disk, "_staging/").await, 0);
    }

    #[tokio::test]
    async fn two_uploads_in_flight_do_not_share_a_staging_key() {
        let (_root, disk) = disk().await;

        let first = disk.begin_upload().await.expect("the upload opens");
        let second = disk.begin_upload().await.expect("the upload opens");
        assert_ne!(first.staging_path(), second.staging_path());

        first.abort().await.expect("the abort succeeds");
        second.abort().await.expect("the abort succeeds");
    }

    const PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R',
    ];
    const PHP: &[u8] = b"<?php system($_GET['c']); ?>";

    #[tokio::test]
    async fn a_verified_upload_of_real_png_bytes_is_kept() {
        let (_root, disk) = disk().await;

        let mut upload = disk.begin_upload().await.expect("the upload opens");
        upload.write(PNG).await.expect("the write succeeds");
        assert_eq!(
            upload.sniffed().map(|kind| kind.mime()),
            Some("image/png"),
            "the bytes identify themselves"
        );

        let address = upload.finish_verified(png()).await.expect("a png is a png");
        assert!(disk.exists(&address.path()).await.expect("the stat runs"));
    }

    #[tokio::test]
    async fn a_php_script_renamed_to_png_is_refused_and_leaves_nothing() {
        let (_root, disk) = disk().await;

        let mut upload = disk.begin_upload().await.expect("the upload opens");
        upload.write(PHP).await.expect("the write succeeds");

        let error = upload
            .finish_verified(png())
            .await
            .expect_err("a script is not an image");
        assert!(matches!(
            error,
            crate::storage::error::UploadError::Content { .. }
        ));

        // The refusal is worthless if the bytes are still on the disk.
        assert_eq!(count_under(&disk, "_staging/").await, 0);
        let rejected = ContentAddress::of(PHP, png());
        assert!(
            !disk.exists(&rejected.path()).await.expect("the stat runs"),
            "the refused object must not have been promoted"
        );
    }

    #[tokio::test]
    async fn the_sniff_buffer_does_not_grow_with_the_object() {
        let (_root, disk) = disk().await;

        let mut upload = disk.begin_upload().await.expect("the upload opens");
        upload.write(PNG).await.expect("the write succeeds");
        for _ in 0..64 {
            upload
                .write(vec![0u8; 4096])
                .await
                .expect("the write succeeds");
        }

        assert_eq!(upload.head().len(), crate::storage::sniff::SNIFF_BYTES);
        assert_eq!(upload.byte_len(), PNG.len() as u64 + 64 * 4096);
        upload.abort().await.expect("the abort succeeds");
    }

    #[tokio::test]
    async fn the_verdict_is_available_from_the_first_chunk() {
        let (_root, disk) = disk().await;

        // A caller that wants to stop reading a hostile body early must be
        // able to ask before the last byte arrives.
        let mut upload = disk.begin_upload().await.expect("the upload opens");
        upload.write(&PHP[..8]).await.expect("the write succeeds");
        assert!(upload.verify(&png()).is_err());
        upload.abort().await.expect("the abort succeeds");
    }
}
