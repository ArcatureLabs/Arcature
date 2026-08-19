//! Storage configuration: filesystem and S3-compatible backends, with
//! credential redaction for S3.
//!
//! [`StorageConfig`] selects a storage backend and its resolved parameters.
//! It is the single configuration type passed to
//! [`crate::storage::Storage::connect`].

use std::fmt;

use crate::storage::error::StorageConfigError;

/// Resolved storage configuration: selects a backend and its parameters.
///
/// Construct with [`StorageConfig::fs`] (filesystem) or
/// [`StorageConfig::s3`] (S3-compatible), then pass to
/// [`crate::storage::Storage::connect`].
///
/// Each variant is gated by the corresponding feature (`storage-fs` for fs,
/// `storage-s3` for s3). The default features enable `storage-fs`; enable the
/// `storage-s3` feature for S3-compatible object storage.
#[derive(Debug, Clone)]
pub enum StorageConfig {
    /// Local POSIX filesystem backend (feature `storage-fs`).
    #[cfg(feature = "storage-fs")]
    Fs(FsConfig),
    /// S3-compatible object storage backend (feature `storage-s3`).
    #[cfg(feature = "storage-s3")]
    S3(S3Config),
}

impl StorageConfig {
    /// Filesystem backend configuration.
    ///
    /// # Errors
    ///
    /// Returns [`StorageConfigError::EmptyRoot`] if `root` is empty or
    /// whitespace-only.
    #[cfg(feature = "storage-fs")]
    pub fn fs(root: impl Into<String>) -> Result<Self, StorageConfigError> {
        Ok(Self::Fs(FsConfig::new(root)?))
    }

    /// S3-compatible backend configuration.
    #[cfg(feature = "storage-s3")]
    #[must_use]
    pub fn s3(config: S3Config) -> Self {
        Self::S3(config)
    }

    /// Validate the configuration before any storage work.
    pub(crate) fn validate(&self) -> Result<(), StorageConfigError> {
        match self {
            #[cfg(feature = "storage-fs")]
            Self::Fs(_) => Ok(()),
            #[cfg(feature = "storage-s3")]
            Self::S3(_) => Ok(()),
            #[allow(unreachable_patterns)]
            _ => Ok(()),
        }
    }
}

/// Resolved configuration for the local POSIX filesystem storage backend.
///
/// The filesystem backend has no credentials. `FsConfig` derives `Debug`
/// safely -- nothing is redacted because nothing is sensitive.
#[derive(Debug, Clone)]
pub struct FsConfig {
    root: String,
}

impl FsConfig {
    /// Create filesystem configuration with the given root directory.
    ///
    /// # Errors
    ///
    /// Returns [`StorageConfigError::EmptyRoot`] if `root` is empty or
    /// whitespace-only.
    pub fn new(root: impl Into<String>) -> Result<Self, StorageConfigError> {
        let root = root.into();
        if root.trim().is_empty() {
            return Err(StorageConfigError::EmptyRoot);
        }
        Ok(Self { root })
    }

    /// The configured root directory.
    #[must_use]
    pub fn root(&self) -> &str {
        &self.root
    }

    /// Convert into an OpenDAL `Fs` service builder.
    #[cfg(feature = "storage-fs")]
    pub(crate) fn into_builder(self) -> opendal::services::Fs {
        opendal::services::Fs::default().root(&self.root)
    }
}

/// Resolved configuration for an S3-compatible object storage backend.
///
/// # Credential redaction
///
/// `S3Config` implements `Debug` manually. It never exposes the
/// `access_key_id` value in full or the `secret_access_key` at all. Only a
/// boolean indicator of whether credentials are set appears in `Debug`.
#[derive(Clone)]
pub struct S3Config {
    bucket: String,
    endpoint: Option<String>,
    region: Option<String>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    root: Option<String>,
}

impl S3Config {
    /// Create S3 configuration for the given bucket.
    ///
    /// # Errors
    ///
    /// Returns [`StorageConfigError::EmptyBucket`] if `bucket` is empty or
    /// whitespace-only.
    pub fn new(bucket: impl Into<String>) -> Result<Self, StorageConfigError> {
        let bucket = bucket.into();
        if bucket.trim().is_empty() {
            return Err(StorageConfigError::EmptyBucket);
        }
        Ok(Self {
            bucket,
            endpoint: None,
            region: None,
            access_key_id: None,
            secret_access_key: None,
            root: None,
        })
    }

    /// Set the S3-compatible service endpoint URL.
    #[must_use]
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Set the AWS region.
    #[must_use]
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Set the access key id (public credential).
    #[must_use]
    pub fn access_key_id(mut self, access_key_id: impl Into<String>) -> Self {
        self.access_key_id = Some(access_key_id.into());
        self
    }

    /// Set the secret access key (private credential).
    #[must_use]
    pub fn secret_access_key(mut self, secret_access_key: impl Into<String>) -> Self {
        self.secret_access_key = Some(secret_access_key.into());
        self
    }

    /// Set a key prefix (`root`) within the bucket.
    #[must_use]
    pub fn root(mut self, root: impl Into<String>) -> Self {
        self.root = Some(root.into());
        self
    }

    /// The configured bucket name.
    #[must_use]
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// The configured endpoint, if set.
    #[must_use]
    pub fn endpoint_value(&self) -> Option<&str> {
        self.endpoint.as_deref()
    }

    /// The configured region, if set.
    #[must_use]
    pub fn region_value(&self) -> Option<&str> {
        self.region.as_deref()
    }

    /// The configured key prefix, if set.
    #[must_use]
    pub fn root_prefix(&self) -> Option<&str> {
        self.root.as_deref()
    }

    /// Convert into an OpenDAL `S3` service builder.
    #[cfg(feature = "storage-s3")]
    pub(crate) fn into_builder(self) -> opendal::services::S3 {
        let Self {
            bucket,
            endpoint,
            region,
            access_key_id,
            secret_access_key,
            root,
        } = self;
        let mut builder = opendal::services::S3::default().bucket(&bucket);
        if let Some(endpoint) = &endpoint {
            builder = builder.endpoint(endpoint);
        }
        if let Some(region) = &region {
            builder = builder.region(region);
        }
        if let Some(access_key_id) = &access_key_id {
            builder = builder.access_key_id(access_key_id);
        }
        if let Some(secret_access_key) = &secret_access_key {
            builder = builder.secret_access_key(secret_access_key);
        }
        if let Some(root) = &root {
            builder = builder.root(root);
        }
        builder
    }
}

impl fmt::Debug for S3Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3Config")
            .field("bucket", &self.bucket)
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("root", &self.root)
            .field("has_access_key_id", &self.access_key_id.is_some())
            .field("has_secret_access_key", &self.secret_access_key.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_accepts_non_empty_root() {
        let config = FsConfig::new("/tmp/arcature").unwrap();
        assert_eq!(config.root(), "/tmp/arcature");
    }

    #[test]
    fn fs_rejects_empty_root() {
        assert!(matches!(
            FsConfig::new(""),
            Err(StorageConfigError::EmptyRoot)
        ));
    }

    #[test]
    fn s3_accepts_non_empty_bucket() {
        let config = S3Config::new("my-bucket").unwrap();
        assert_eq!(config.bucket(), "my-bucket");
    }

    #[test]
    fn s3_rejects_empty_bucket() {
        assert!(matches!(
            S3Config::new(""),
            Err(StorageConfigError::EmptyBucket)
        ));
    }

    #[test]
    fn s3_debug_does_not_expose_credentials() {
        let config = S3Config::new("bkt")
            .unwrap()
            .access_key_id("AKIATESTKEY123")
            .secret_access_key("supersecretvalue456");
        let debug = format!("{config:?}");
        assert!(
            !debug.contains("AKIATESTKEY123"),
            "debug leaked access key id"
        );
        assert!(
            !debug.contains("supersecretvalue456"),
            "debug leaked secret access key"
        );
        assert!(debug.contains("has_access_key_id: true"));
    }
}
