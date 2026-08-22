//! What the bytes say they are, as opposed to what the client said they were.
//!
//! # Why the client's answer is not an answer
//!
//! A multipart part arrives with two claims about its type, and the client
//! wrote both of them. `Content-Type: image/png` is a string in a header the
//! uploader chose. `filename="avatar.png"` is a string in the same header,
//! chosen by the same uploader. Neither is evidence, and treating either as
//! evidence is the whole of the classic upload bug: `shell.php` renamed to
//! `avatar.png`, stored under a name the server later resolves, and then
//! executed or served back as script.
//!
//! The extension whitelist and content addressing already close most of that:
//! a refused extension never reaches the disk, and a content-addressed key
//! means no byte of the request reaches the path. What neither answers is
//! whether the object *is* what its accepted extension says. That question
//! has exactly one source of evidence -- the bytes themselves.
//!
//! # What this module does, and what it deliberately does not
//!
//! It compares the leading [`SNIFF_BYTES`] of the object against a table of
//! magic-number signatures, via the `infer` crate. That is a prefix
//! comparison and nothing more.
//!
//! It does **not** decode. No image is parsed, no dimensions are read, no
//! pixel is produced. An image decoder is an interpreter for
//! attacker-controlled input, and decoders are the densest source of memory
//! CVEs in any web stack; running one in the request path trades a
//! type-confusion bug for a heap-corruption bug. If an application needs to
//! know an image's dimensions or wants a thumbnail, that work belongs in a
//! queue worker with its own memory bound, not here.
//!
//! It is also not a virus scanner and not a claim of safety. A file can be
//! genuinely, verifiably a PNG and still be malicious. What this answers is
//! narrower and worth having on its own: *the bytes and the extension agree*.
//!
//! # The rule
//!
//! [`verify`] holds the extension and the bytes to a symmetric agreement:
//!
//! * An extension with a known signature (`png`, `jpg`, `pdf`, ...) **must**
//!   sniff to that signature. Bytes that sniff to nothing are refused, not
//!   waved through -- "unrecognized" is exactly what a PHP script renamed
//!   `.jpg` looks like.
//! * An extension with no signature (`txt`, `csv`, an application's own)
//!   must sniff to *nothing*. If a `.txt` upload's first bytes are a PE
//!   header or a zip local-file header, the extension and the content
//!   disagree just as loudly, only in the other direction.
//!
//! Both directions are refusals, so there is no "unknown" state an upload can
//! land in and be accepted by default.

use std::fmt;

use crate::storage::error::SniffError;
use crate::storage::filename::Extension;

/// How many leading bytes are enough to identify a format.
///
/// Every signature in the table is well inside this, and it is a fixed cost:
/// the sniff buffer is the only part of an upload that is ever held in
/// memory, so it is bounded here rather than by the size of the object.
pub const SNIFF_BYTES: usize = 512;

/// A type recognized from an object's leading bytes.
///
/// Carries both halves of the answer: the canonical extension for the format
/// and its media type, the latter being what a download response should send
/// as `Content-Type` -- a value derived from the bytes rather than copied
/// from the request.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SniffedType {
    extension: &'static str,
    mime: &'static str,
}

impl SniffedType {
    /// The canonical extension for the recognized format, lowercase and
    /// without a dot (`"png"`, `"jpg"`, `"pdf"`).
    #[must_use]
    pub fn extension(&self) -> &'static str {
        self.extension
    }

    /// The media type for the recognized format (`"image/png"`).
    #[must_use]
    pub fn mime(&self) -> &'static str {
        self.mime
    }
}

impl fmt::Display for SniffedType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.mime)
    }
}

/// Identify an object from its leading bytes, or return `None` if they match
/// no known signature.
///
/// Only the first [`SNIFF_BYTES`] are read; passing more is harmless and
/// passing fewer is fine as long as the prefix is intact.
#[must_use]
pub fn sniff(bytes: &[u8]) -> Option<SniffedType> {
    let head = &bytes[..bytes.len().min(SNIFF_BYTES)];
    infer::get(head).map(|kind| SniffedType {
        extension: kind.extension(),
        mime: kind.mime_type(),
    })
}

/// The signatures an extension is allowed to sniff to.
///
/// An extension that is absent from this table has no signature of its own --
/// a text format, or something an application defined -- and is held to the
/// other half of the rule: it must sniff to nothing at all.
///
/// The OOXML entries accept plain `zip` beside their own type because a
/// `.docx` *is* a zip archive, and whether the inner content-type part is
/// close enough to the front of the stream to be seen inside
/// [`SNIFF_BYTES`] depends on how the producing application ordered the
/// archive.
const SIGNATURES: &[(&str, &[&str])] = &[
    // Raster images.
    ("jpg", &["jpg"]),
    ("jpeg", &["jpg"]),
    ("png", &["png"]),
    ("gif", &["gif"]),
    ("webp", &["webp"]),
    ("bmp", &["bmp"]),
    ("ico", &["ico"]),
    ("tif", &["tif"]),
    ("tiff", &["tif"]),
    ("avif", &["avif"]),
    ("heic", &["heic"]),
    ("psd", &["psd"]),
    // Documents.
    ("pdf", &["pdf"]),
    ("epub", &["epub"]),
    ("rtf", &["rtf"]),
    ("docx", &["docx", "zip"]),
    ("xlsx", &["xlsx", "zip"]),
    ("pptx", &["pptx", "zip"]),
    // Archives.
    ("zip", &["zip"]),
    ("gz", &["gz"]),
    ("bz2", &["bz2"]),
    ("xz", &["xz"]),
    ("zst", &["zst"]),
    ("7z", &["7z"]),
    ("rar", &["rar"]),
    // Audio and video.
    ("mp3", &["mp3"]),
    ("wav", &["wav"]),
    ("flac", &["flac"]),
    ("ogg", &["ogg"]),
    ("mp4", &["mp4"]),
    ("m4a", &["m4a"]),
    ("webm", &["webm"]),
    ("mov", &["mov"]),
    ("avi", &["avi"]),
    // Fonts.
    ("woff", &["woff"]),
    ("woff2", &["woff2"]),
    ("ttf", &["ttf"]),
    ("otf", &["otf"]),
];

/// The signatures `extension` is required to sniff to, or `None` when the
/// extension names a format with no magic number.
#[must_use]
pub fn expected_signatures(extension: &Extension) -> Option<&'static [&'static str]> {
    SIGNATURES
        .iter()
        .find(|(name, _)| *name == extension.as_str())
        .map(|(_, signatures)| *signatures)
}

/// Hold an object's bytes and its accepted extension to the rule described in
/// the [module documentation](self).
///
/// On success returns what the bytes were recognized as, which is `None` for
/// a signature-less extension whose bytes were correspondingly unrecognized.
///
/// # Errors
///
/// * [`SniffError::Unrecognized`] -- the extension has a signature and the
///   bytes match no signature at all. This is what a script renamed to an
///   image extension looks like.
/// * [`SniffError::Mismatch`] -- the bytes were recognized as a different
///   format than the extension claims, in either direction.
pub fn verify(bytes: &[u8], extension: &Extension) -> Result<Option<SniffedType>, SniffError> {
    let sniffed = sniff(bytes);
    match (expected_signatures(extension), sniffed) {
        // The extension promises a signature and the bytes deliver it.
        (Some(expected), Some(found)) if expected.contains(&found.extension()) => Ok(Some(found)),
        // The extension promises a signature and the bytes are something
        // else, or nothing recognizable at all.
        (Some(_), Some(found)) => Err(SniffError::Mismatch {
            declared: extension.clone(),
            sniffed: found.mime(),
        }),
        (Some(_), None) => Err(SniffError::Unrecognized {
            declared: extension.clone(),
        }),
        // The extension promises no signature, and neither do the bytes.
        (None, None) => Ok(None),
        // The extension promises no signature but the bytes are a known
        // binary format: the disagreement runs the other way.
        (None, Some(found)) => Err(SniffError::Mismatch {
            declared: extension.clone(),
            sniffed: found.mime(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extension(name: &str) -> Extension {
        Extension::parse(name).expect("a valid extension")
    }

    /// A minimal but genuine header for each format, long enough for the
    /// matcher and no longer.
    fn png() -> Vec<u8> {
        b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec()
    }

    fn jpeg() -> Vec<u8> {
        b"\xff\xd8\xff\xe0\x00\x10JFIF\x00".to_vec()
    }

    fn gif() -> Vec<u8> {
        b"GIF89a\x01\x00\x01\x00".to_vec()
    }

    fn pdf() -> Vec<u8> {
        b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec()
    }

    fn webp() -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&[0x1a, 0x00, 0x00, 0x00]);
        bytes.extend_from_slice(b"WEBPVP8 ");
        bytes
    }

    fn zip() -> Vec<u8> {
        b"PK\x03\x04\x14\x00\x00\x00\x00\x00".to_vec()
    }

    fn php() -> Vec<u8> {
        b"<?php system($_GET['c']); ?>".to_vec()
    }

    #[test]
    fn the_signature_table_names_types_infer_actually_reports() {
        // A table entry that no matcher can ever produce is a hole: the
        // extension would be permanently unstorable, or worse, silently
        // accepted by a stale alias. Pin the ones we can construct.
        for (bytes, expected) in [
            (png(), "png"),
            (jpeg(), "jpg"),
            (gif(), "gif"),
            (pdf(), "pdf"),
            (webp(), "webp"),
            (zip(), "zip"),
        ] {
            assert_eq!(
                sniff(&bytes).map(|found| found.extension()),
                Some(expected),
                "expected {expected} for its own magic bytes"
            );
        }
    }

    #[test]
    fn a_real_png_under_a_png_extension_is_accepted() {
        let found = verify(&png(), &extension("png")).expect("a png is a png");
        assert_eq!(found.map(|kind| kind.mime()), Some("image/png"));
    }

    #[test]
    fn jpg_and_jpeg_are_the_same_format() {
        assert!(verify(&jpeg(), &extension("jpg")).is_ok());
        assert!(verify(&jpeg(), &extension("jpeg")).is_ok());
    }

    #[test]
    fn a_php_script_renamed_to_jpg_is_refused() {
        // The whole point. Nothing about `<?php` is a JPEG, and nothing about
        // it is a recognized format either, so it must not fall through an
        // "unknown bytes are fine" hole.
        let error = verify(&php(), &extension("jpg")).expect_err("a script is not an image");
        assert!(matches!(error, SniffError::Unrecognized { .. }));
    }

    #[test]
    fn a_png_renamed_to_pdf_is_refused_as_a_mismatch() {
        let error = verify(&png(), &extension("pdf")).expect_err("a png is not a pdf");
        match error {
            SniffError::Mismatch { sniffed, .. } => assert_eq!(sniffed, "image/png"),
            other => panic!("expected a mismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_signature_less_extension_accepts_unrecognized_bytes() {
        // A `.txt` really is "no particular format", and PHP source is text.
        assert_eq!(verify(&php(), &extension("txt")), Ok(None));
        assert_eq!(verify(b"a,b,c\n1,2,3\n", &extension("csv")), Ok(None));
    }

    #[test]
    fn a_signature_less_extension_refuses_recognized_binary() {
        // The disagreement runs the other way: the name says "no format", the
        // bytes say "zip archive".
        let error = verify(&zip(), &extension("txt")).expect_err("a zip is not text");
        assert!(matches!(error, SniffError::Mismatch { .. }));
    }

    #[test]
    fn an_empty_object_matches_nothing() {
        assert_eq!(sniff(b""), None);
        assert!(verify(b"", &extension("png")).is_err());
        assert_eq!(verify(b"", &extension("txt")), Ok(None));
    }

    #[test]
    fn only_the_leading_bytes_are_looked_at() {
        // A signature buried past the window is not a signature. Otherwise an
        // attacker could hide a matching header behind a megabyte of padding
        // and a streaming caller could never afford to find it.
        let mut buried = vec![0u8; SNIFF_BYTES];
        buried.extend_from_slice(&png());
        assert_eq!(sniff(&buried), None);
    }

    #[test]
    fn a_truncated_prefix_is_still_enough() {
        // The sniff buffer fills a chunk at a time, and the first chunk off a
        // socket can be short.
        assert_eq!(sniff(&png()[..8]).map(|kind| kind.extension()), Some("png"));
    }

    #[test]
    fn every_built_in_whitelist_extension_has_a_decided_rule() {
        // Not that each has a signature -- `txt` and `csv` deliberately do
        // not -- but that the table's answer for each was chosen rather than
        // fallen into.
        let expected: &[(&str, bool)] = &[
            ("jpg", true),
            ("jpeg", true),
            ("png", true),
            ("gif", true),
            ("webp", true),
            ("pdf", true),
            ("txt", false),
            ("csv", false),
        ];
        for (name, has_signature) in expected {
            assert_eq!(
                expected_signatures(&extension(name)).is_some(),
                *has_signature,
                "{name}"
            );
        }
    }

    #[test]
    fn the_table_has_no_duplicate_extensions() {
        let mut names: Vec<&str> = SIGNATURES.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(
            names.len(),
            count,
            "a duplicate entry shadows the later one"
        );
    }
}
