//! Content-addressed object names: the stored path is a hash of the bytes.
//!
//! # Why the name comes from the content
//!
//! Path traversal is not one bug, it is a family of them, and the family is
//! large because the input is a string with structure the attacker chose:
//! `../`, `..\`, `%2e%2e%2f`, `....//`, `..%c0%af`, a NUL that truncates the
//! name in the C library underneath, an NTFS alternate data stream, a symlink
//! the previous upload planted. A sanitizer answers each of those one at a
//! time, and it is only ever as complete as the last person who thought about
//! it.
//!
//! Content addressing answers all of them at once, structurally: the object
//! key is `SHA-256(bytes)` plus a whitelisted extension, so **no byte of the
//! request reaches the path**. There is nothing left for a traversal payload
//! to be *in*. The client's filename becomes what it always should have been
//! -- a label carried alongside the object, rendered in a
//! `Content-Disposition` header and shown in a UI, and never resolved by
//! anything.
//!
//! [`SafeFilename`](crate::storage::SafeFilename) still exists and is still
//! worth running, for the label. The two layers answer different questions:
//! the sanitizer makes the *metadata* safe to display, and content addressing
//! makes the *path* safe to resolve. Neither substitutes for the other.
//!
//! # What else falls out of it
//!
//! * **Deduplication.** The same bytes uploaded twice land on the same key and
//!   occupy the storage once.
//! * **Idempotent writes.** Re-uploading is a no-op rather than a race between
//!   two writers for one name.
//! * **A fanned-out tree.** The key is `ab/cd/abcd....ext`, two levels of
//!   two hex digits, so a filesystem disk gets 65,536 leaf directories instead
//!   of one directory with a million entries in it.
//!
//! # What it is not
//!
//! It is not an access-control mechanism. A digest is unguessable in practice,
//! but "unguessable URL" is not authorization -- it leaks through referrers,
//! logs and browser history like any other URL. Authorize the download.
//!
//! It is also not a claim that the bytes are safe. The hash says only that
//! this is the same file as last time. Whether the file should have been
//! accepted at all is what the extension whitelist and the magic-byte check
//! are for.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::storage::error::StoragePathError;
use crate::storage::filename::Extension;
use crate::storage::path::StoragePath;

/// The length of a SHA-256 digest rendered as lowercase hex.
pub const DIGEST_HEX_LEN: usize = 64;

/// A streaming SHA-256 over an object's bytes.
///
/// Fed a chunk at a time so an upload never has to exist in memory all at
/// once; the digest is only known when the last chunk has gone past, which is
/// exactly why an upload is written to a staging key first and moved to its
/// content-addressed key afterwards.
///
/// # Example
///
/// ```
/// use arcature::storage::{ContentHasher, Extension};
///
/// let mut hasher = ContentHasher::new();
/// hasher.update(b"hello, ");
/// hasher.update(b"world");
/// let address = hasher.finish(Extension::parse("txt").unwrap());
///
/// assert_eq!(address.byte_len(), 12);
/// assert_eq!(
///     address.digest(),
///     "09ca7e4eaa6e8ae9c7d261167129184883644d07dfba7cbfbc4c8a2e08360d5b"
/// );
/// ```
#[derive(Clone)]
pub struct ContentHasher {
    hasher: Sha256,
    bytes: u64,
}

impl ContentHasher {
    /// Start hashing.
    #[must_use]
    pub fn new() -> Self {
        Self {
            hasher: Sha256::new(),
            bytes: 0,
        }
    }

    /// Feed the next chunk.
    pub fn update(&mut self, chunk: &[u8]) {
        self.hasher.update(chunk);
        self.bytes = self.bytes.saturating_add(chunk.len() as u64);
    }

    /// How many bytes have been fed so far.
    #[must_use]
    pub fn byte_len(&self) -> u64 {
        self.bytes
    }

    /// Finish, pairing the digest with the extension the object will carry.
    #[must_use]
    pub fn finish(self, extension: Extension) -> ContentAddress {
        ContentAddress {
            digest: hex_lower(&self.hasher.finalize()),
            bytes: self.bytes,
            extension,
        }
    }
}

impl Default for ContentHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ContentHasher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContentHasher")
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

/// The name an object is stored under: its SHA-256 digest plus a whitelisted
/// [`Extension`].
///
/// # Example
///
/// ```
/// use arcature::storage::{ContentAddress, Extension};
///
/// let address = ContentAddress::of(b"hello", Extension::parse("PNG").unwrap());
///
/// // Two levels of fan-out, then the full digest, then the extension.
/// assert_eq!(
///     address.path().as_str(),
///     "2c/f2/2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824.png"
/// );
///
/// // The same bytes always land on the same key.
/// assert_eq!(address, ContentAddress::of(b"hello", Extension::parse("png").unwrap()));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentAddress {
    digest: String,
    bytes: u64,
    extension: Extension,
}

impl ContentAddress {
    /// Address a buffer that is already in memory.
    ///
    /// Prefer [`ContentHasher`] for anything arriving off a socket: this
    /// method needs the whole object as one slice, which is the thing an
    /// upload path is trying not to do.
    #[must_use]
    pub fn of(bytes: &[u8], extension: Extension) -> Self {
        let mut hasher = ContentHasher::new();
        hasher.update(bytes);
        hasher.finish(extension)
    }

    /// The SHA-256 digest, lowercase hex, [`DIGEST_HEX_LEN`] characters.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The object's size in bytes.
    #[must_use]
    pub fn byte_len(&self) -> u64 {
        self.bytes
    }

    /// The extension the object carries.
    #[must_use]
    pub fn extension(&self) -> &Extension {
        &self.extension
    }

    /// The object key: `ab/cd/<digest>.<extension>`.
    #[must_use]
    pub fn path(&self) -> StoragePath {
        StoragePath::new(&self.to_string())
            .expect("a hex digest and an ASCII-alphanumeric extension are always a valid key")
    }

    /// The object key under an application-chosen prefix, as
    /// `<prefix>/ab/cd/<digest>.<extension>`.
    ///
    /// The prefix is the application's own string -- a disk sub-tree such as
    /// `avatars` -- never anything that came off the request.
    ///
    /// # Errors
    ///
    /// Returns [`StoragePathError`] if `prefix` is not a valid relative key.
    pub fn path_under(&self, prefix: &str) -> Result<StoragePath, StoragePathError> {
        let prefix = prefix.trim_end_matches('/');
        StoragePath::new(&format!("{prefix}/{self}"))
    }
}

impl fmt::Display for ContentAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Indexing is safe: `digest` is exactly `DIGEST_HEX_LEN` ASCII hex
        // characters, produced here and nowhere else.
        write!(
            formatter,
            "{}/{}/{}.{}",
            &self.digest[0..2],
            &self.digest[2..4],
            self.digest,
            self.extension
        )
    }
}

/// Render bytes as lowercase hex.
fn hex_lower(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a `String` cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Extension {
        Extension::parse("png").unwrap()
    }

    #[test]
    fn matches_the_known_sha256_of_abc() {
        let address = ContentAddress::of(b"abc", png());
        assert_eq!(
            address.digest(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(address.digest().len(), DIGEST_HEX_LEN);
    }

    #[test]
    fn hashes_the_empty_object() {
        let address = ContentAddress::of(b"", png());
        assert_eq!(
            address.digest(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(address.byte_len(), 0);
    }

    #[test]
    fn streaming_matches_one_shot() {
        let mut hasher = ContentHasher::new();
        for chunk in [&b"a"[..], b"b", b"c"] {
            hasher.update(chunk);
        }
        assert_eq!(hasher.finish(png()), ContentAddress::of(b"abc", png()));
    }

    #[test]
    fn counts_bytes_as_it_goes() {
        let mut hasher = ContentHasher::new();
        hasher.update(&[0u8; 10]);
        assert_eq!(hasher.byte_len(), 10);
        hasher.update(&[0u8; 5]);
        assert_eq!(hasher.finish(png()).byte_len(), 15);
    }

    #[test]
    fn the_key_fans_out_two_levels() {
        let address = ContentAddress::of(b"abc", png());
        assert_eq!(
            address.path().as_str(),
            "ba/78/ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.png"
        );
    }

    #[test]
    fn a_prefix_is_joined_without_a_double_slash() {
        let address = ContentAddress::of(b"abc", png());
        let under = address.path_under("avatars/").unwrap();
        assert!(under.as_str().starts_with("avatars/ba/78/"));
        assert_eq!(under, address.path_under("avatars").unwrap());
    }

    #[test]
    fn a_hostile_prefix_is_refused_by_storage_path() {
        let address = ContentAddress::of(b"abc", png());
        assert!(address.path_under("../../etc").is_err());
        assert!(address.path_under("/absolute").is_err());
    }

    #[test]
    fn the_extension_is_normalized_before_it_reaches_the_key() {
        let address = ContentAddress::of(b"abc", Extension::parse("PNG").unwrap());
        assert!(address.path().as_str().ends_with(".png"));
    }

    #[test]
    fn different_bytes_give_different_keys() {
        assert_ne!(
            ContentAddress::of(b"a", png()).path(),
            ContentAddress::of(b"b", png()).path()
        );
    }
}
