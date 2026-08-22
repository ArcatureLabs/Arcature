//! Serving a stored object back out, as a download and never as a page.
//!
//! # Why serving is its own security problem
//!
//! Accepting an upload safely and serving it safely are two different jobs,
//! and doing the first perfectly buys nothing if the second hands the file to
//! a browser as a document. A stored file served inline is content the
//! attacker wrote, on the application's own origin, with the application's
//! cookies attached -- which is stored XSS with extra steps. The classic
//! shapes are an HTML file served as `text/html`, an SVG (an XML document
//! that may carry script) served as an image, and a file of any type served
//! with a `Content-Type` the browser decides to second-guess.
//!
//! [`Attachment`] answers all three at once, and it answers them by
//! construction rather than by asking the caller to remember:
//!
//! * **`Content-Disposition: attachment`.** The browser saves the file
//!   instead of rendering it. Nothing is a document, so nothing has an
//!   origin, so nothing has script.
//! * **`X-Content-Type-Options: nosniff`.** Without it a browser is free to
//!   disagree with the declared type and render what it thinks it found,
//!   which is precisely the disagreement an attacker engineers.
//! * **`Content-Security-Policy: default-src 'none'; sandbox`.** The belt to
//!   the disposition header's braces, for the browser or the plugin that
//!   renders the response anyway.
//!
//! The `Content-Type` itself is never taken from the request. It comes from
//! [`sniff`](crate::storage::sniff)ing the object's own leading bytes, and
//! falls back to `application/octet-stream` when they say nothing -- the
//! type that means "I am not telling you what to do with this".
//!
//! # The filename is a label, still
//!
//! A download's suggested filename is the one place a client-authored string
//! legitimately travels back out, so it goes out as a
//! [`SafeFilename`](crate::storage::SafeFilename) and nothing else. The type
//! is the argument: a `&str` parameter here would be a header-injection hole
//! waiting for the one caller who forgot. The value is then emitted twice per
//! RFC 6266 -- an ASCII-only `filename=` for old parsers and an RFC 5987
//! `filename*=UTF-8''` for everything since -- because a name like
//! `báo-cáo.pdf` is not representable in the first form and silently
//! mangling it is worse than sending both.

use std::fmt::Write as _;

use axum::body::{Body, Bytes};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::http::security::NO_SNIFF;
use crate::storage::error::StorageError;
use crate::storage::sniff::{SNIFF_BYTES, SniffedType, sniff};
use crate::storage::{Disk, SafeFilename, StoragePath};

/// The media type that declines to say what the bytes are, which is the right
/// answer for an object whose signature is unrecognized.
pub const OCTET_STREAM: &str = "application/octet-stream";

/// The policy sent with every [`Attachment`].
///
/// `default-src 'none'` means the response may load nothing, and `sandbox`
/// (with no allow-list) puts it in an opaque origin with scripts, forms and
/// same-origin access all off. It costs nothing on a response the browser was
/// going to save anyway, and it is the difference between a mishandled
/// `Content-Disposition` being a bug and being a stored-XSS.
pub const DOWNLOAD_CSP: &str = "default-src 'none'; sandbox";

/// A stored object on its way back out to a client, as a download.
///
/// Build it from bytes with [`Attachment::from_bytes`] or stream it off a
/// disk with [`Attachment::from_disk`], optionally attach a suggested
/// filename with [`Attachment::with_filename`], and return it from a handler.
///
/// # Example
///
/// ```no_run
/// use arcature::http::download::Attachment;
/// use arcature::storage::{AllowedExtensions, Disk, SafeFilename, StoragePath};
///
/// # async fn handler(disk: &Disk, key: &StoragePath) -> Result<axum::response::Response, arcature::Error> {
/// use axum::response::IntoResponse as _;
///
/// let label = SafeFilename::parse("báo cáo.pdf", &AllowedExtensions::documents())
///     .map_err(|_| arcature::bad_request("that filename is not storable"))?;
///
/// Ok(Attachment::from_disk(disk, key)
///     .await
///     .map_err(|_| arcature::not_found("no such file"))?
///     .with_filename(&label)
///     .into_response())
/// # }
/// ```
#[non_exhaustive]
pub struct Attachment {
    body: Body,
    content_type: &'static str,
    content_length: Option<u64>,
    filename: Option<String>,
}

impl std::fmt::Debug for Attachment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Attachment")
            .field("content_type", &self.content_type)
            .field("content_length", &self.content_length)
            .field("filename", &self.filename)
            .finish_non_exhaustive()
    }
}

impl Attachment {
    /// Serve an object that is already in memory.
    ///
    /// The media type is sniffed from the bytes. Prefer
    /// [`Attachment::from_disk`] for anything that came off a disk: this
    /// method needs the whole object as one buffer, which is the thing the
    /// upload path went to some trouble to avoid.
    #[must_use]
    pub fn from_bytes(bytes: impl Into<Bytes>) -> Self {
        let bytes = bytes.into();
        let content_type = media_type(sniff(&bytes));
        let content_length = Some(bytes.len() as u64);
        Self {
            body: Body::from(bytes),
            content_type,
            content_length,
            filename: None,
        }
    }

    /// Stream an object off a disk.
    ///
    /// Only the leading [`SNIFF_BYTES`] are read up front, to decide the
    /// media type; the remainder is streamed, so the response costs one
    /// buffer rather than one object.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] if the object cannot be stat'd or
    /// read -- including when it does not exist, which a handler should
    /// translate into a 404 rather than passing through.
    pub async fn from_disk(disk: &Disk, path: &StoragePath) -> Result<Self, StorageError> {
        use futures::StreamExt as _;

        let length = disk.stat(path).await?.content_length();
        let reader = disk.reader(path).await?;

        let head_len = length.min(SNIFF_BYTES as u64);
        let head: Bytes = reader.read(0..head_len).await?.to_bytes();
        let content_type = media_type(sniff(&head));

        // A short object is entirely inside the sniff window, and asking the
        // backend for an empty tail range is a request that some of them
        // reject rather than answer with nothing.
        let body = if head.len() as u64 >= length {
            Body::from(head)
        } else {
            let tail = reader.into_bytes_stream(head.len() as u64..).await?;
            Body::from_stream(
                futures::stream::once(async move { Ok::<Bytes, std::io::Error>(head) }).chain(tail),
            )
        };

        Ok(Self {
            body,
            content_type,
            content_length: Some(length),
            filename: None,
        })
    }

    /// Suggest a filename to save the download under.
    ///
    /// Takes a [`SafeFilename`] rather than a string on purpose -- see the
    /// [module documentation](self).
    #[must_use]
    pub fn with_filename(mut self, filename: &SafeFilename) -> Self {
        self.filename = Some(filename.to_string());
        self
    }

    /// Override the media type with one derived from the object's bytes.
    ///
    /// Useful when the type was sniffed at upload time and stored beside the
    /// object, so serving it does not have to read the bytes again. There is
    /// deliberately no way to set an arbitrary string here: every media type
    /// this response can carry is one some bytes were recognized as.
    #[must_use]
    pub fn with_content_type(mut self, sniffed: SniffedType) -> Self {
        self.content_type = sniffed.mime();
        self
    }

    /// The media type this response will send.
    #[must_use]
    pub fn content_type(&self) -> &'static str {
        self.content_type
    }
}

impl IntoResponse for Attachment {
    fn into_response(self) -> Response {
        let mut response = Response::new(self.body);
        *response.status_mut() = StatusCode::OK;
        let headers = response.headers_mut();

        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(self.content_type),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static(NO_SNIFF),
        );
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(DOWNLOAD_CSP),
        );
        // `content_disposition` emits ASCII only, so the conversion cannot
        // fail; the fallback keeps the header present rather than losing the
        // `attachment` disposition if that ever stops being true.
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::try_from(content_disposition(self.filename.as_deref()))
                .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
        );
        if let Some(length) = self.content_length
            && let Ok(value) = HeaderValue::try_from(length.to_string())
        {
            headers.insert(header::CONTENT_LENGTH, value);
        }

        response
    }
}

/// Media types a browser will happily execute if it ever renders the
/// response, and which are therefore never sent from here.
///
/// `Content-Disposition: attachment` should already have made this moot, and
/// `nosniff` and the sandbox policy should have made it moot twice. This is
/// the third lock, and it is here because the failure it guards against is
/// stored XSS on the application's own origin: the cost of being wrong is
/// asymmetric enough that three cheap locks beat two.
const SCRIPTABLE: &[&str] = &[
    "text/html",
    "application/xhtml+xml",
    "image/svg+xml",
    "text/xml",
    "application/xml",
];

/// The media type for a sniff result, or the one that declines to say.
///
/// A recognized-but-scriptable type is downgraded rather than forwarded: the
/// honest answer for an HTML file being handed back as a download is "bytes",
/// not "document".
fn media_type(sniffed: Option<SniffedType>) -> &'static str {
    match sniffed.map(|kind| kind.mime()) {
        None => OCTET_STREAM,
        Some(mime) if SCRIPTABLE.contains(&mime) => OCTET_STREAM,
        Some(mime) => mime,
    }
}

/// Build an RFC 6266 `Content-Disposition` value, ASCII only.
fn content_disposition(filename: Option<&str>) -> String {
    let Some(filename) = filename else {
        return "attachment".to_string();
    };

    let mut value = String::from("attachment; filename=\"");
    value.push_str(&ascii_fallback(filename));
    value.push('"');

    // The extended form is only worth sending when the plain one lost
    // something. `is_ascii` is the exact condition: an all-ASCII name
    // survives the fallback unchanged apart from the quoting.
    if !filename.is_ascii() {
        value.push_str("; filename*=UTF-8''");
        value.push_str(&rfc5987_encode(filename));
    }
    value
}

/// The ASCII-only `filename=` form: anything outside printable ASCII, and the
/// two characters that end a quoted string, become `_`.
///
/// A [`SafeFilename`] already has no control characters in it. This runs
/// anyway, because "the caller definitely sanitized" is the assumption every
/// header-injection bug is built on, and the cost is one pass over 255 bytes.
fn ascii_fallback(filename: &str) -> String {
    filename
        .chars()
        .map(|character| match character {
            '"' | '\\' => '_',
            character if character.is_ascii_graphic() || character == ' ' => character,
            _ => '_',
        })
        .collect()
}

/// Percent-encode to RFC 5987 `attr-char`, which is what `filename*=` accepts.
fn rfc5987_encode(filename: &str) -> String {
    const UNRESERVED: &[u8] = b"!#$&+-.^_`|~";

    let mut encoded = String::with_capacity(filename.len());
    for byte in filename.as_bytes() {
        if byte.is_ascii_alphanumeric() || UNRESERVED.contains(byte) {
            encoded.push(*byte as char);
        } else {
            // Writing to a `String` cannot fail.
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::storage::{AllowedExtensions, Storage, StorageConfig};

    const PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R',
    ];

    fn header_value(response: &Response, name: header::HeaderName) -> Option<String> {
        response
            .headers()
            .get(name)
            .map(|value| value.to_str().expect("an ASCII header").to_string())
    }

    fn safe(name: &str) -> SafeFilename {
        let allowed = AllowedExtensions::images()
            .with("pdf")
            .expect("pdf is valid")
            .with("txt")
            .expect("txt is valid");
        SafeFilename::parse(name, &allowed).expect("a storable name")
    }

    #[test]
    fn every_download_is_an_attachment_that_will_not_be_sniffed() {
        let response = Attachment::from_bytes(PNG.to_vec()).into_response();

        assert_eq!(
            header_value(&response, header::CONTENT_DISPOSITION).as_deref(),
            Some("attachment")
        );
        assert_eq!(
            header_value(&response, header::X_CONTENT_TYPE_OPTIONS).as_deref(),
            Some("nosniff")
        );
        assert_eq!(
            header_value(&response, header::CONTENT_SECURITY_POLICY).as_deref(),
            Some(DOWNLOAD_CSP)
        );
    }

    #[test]
    fn the_media_type_comes_from_the_bytes() {
        let response = Attachment::from_bytes(PNG.to_vec()).into_response();
        assert_eq!(
            header_value(&response, header::CONTENT_TYPE).as_deref(),
            Some("image/png")
        );
    }

    #[test]
    fn unrecognized_bytes_decline_to_say_what_they_are() {
        let response = Attachment::from_bytes(b"<?php echo 1; ?>".to_vec()).into_response();
        assert_eq!(
            header_value(&response, header::CONTENT_TYPE).as_deref(),
            Some(OCTET_STREAM)
        );
    }

    #[test]
    fn an_html_upload_is_never_served_as_a_document() {
        // The one that matters. Whatever the bytes are, the browser is told
        // to save them, not to render them.
        let response = Attachment::from_bytes(b"<html><script>alert(1)</script>".to_vec())
            .with_filename(&safe("notes.txt"))
            .into_response();

        assert_eq!(
            header_value(&response, header::CONTENT_TYPE).as_deref(),
            Some(OCTET_STREAM)
        );
        assert!(
            header_value(&response, header::CONTENT_DISPOSITION)
                .expect("a disposition")
                .starts_with("attachment;")
        );
    }

    #[test]
    fn an_ascii_filename_needs_only_the_plain_form() {
        assert_eq!(
            content_disposition(Some("report.pdf")),
            "attachment; filename=\"report.pdf\""
        );
    }

    #[test]
    fn a_vietnamese_filename_is_sent_in_both_forms() {
        let value = content_disposition(Some("báo-cáo.pdf"));
        assert!(
            value.starts_with("attachment; filename=\"b_o-c_o.pdf\""),
            "the ASCII fallback keeps the shape: {value}"
        );
        assert!(
            value.ends_with("; filename*=UTF-8''b%C3%A1o-c%C3%A1o.pdf"),
            "the extended form keeps the name: {value}"
        );
    }

    #[test]
    fn a_quote_cannot_close_the_quoted_string() {
        // A name that tries to end `filename="` early and append a parameter
        // of its own. Both quotes, and a backslash that would otherwise
        // escape one, are neutralized.
        let value = content_disposition(Some("a\"; x=\"b\\c.txt"));
        assert_eq!(value, "attachment; filename=\"a_; x=_b_c.txt\"");
        assert!(HeaderValue::try_from(value).is_ok());
    }

    #[test]
    fn a_newline_cannot_reach_the_header() {
        // `SafeFilename` refuses these, so this is the second lock on the
        // same door -- and the one that still holds if the first is ever
        // called with something else.
        let value = content_disposition(Some("a\r\nSet-Cookie: x=1.txt"));
        assert!(!value.contains('\r') && !value.contains('\n'), "{value}");
        assert!(HeaderValue::try_from(value).is_ok());
    }

    #[test]
    fn the_disposition_is_always_a_valid_header_value() {
        for name in [
            "a.txt",
            "báo cáo.pdf",
            "😀.png",
            "a\u{7f}b.txt",
            "very long name with spaces and (parens).pdf",
        ] {
            let value = content_disposition(Some(name));
            assert!(HeaderValue::try_from(&value).is_ok(), "{name} -> {value}");
        }
    }

    #[tokio::test]
    async fn an_object_is_streamed_off_the_disk_with_its_sniffed_type() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let config =
            StorageConfig::fs(root.path().to_string_lossy().into_owned()).expect("a valid root");
        let storage = Storage::connect(config).await.expect("the disk connects");
        let disk = storage.default_disk();

        // Bigger than the sniff window, so the tail really is streamed.
        let mut object = PNG.to_vec();
        object.extend(std::iter::repeat_n(b'x', SNIFF_BYTES * 3));
        let path = StoragePath::new("ab/cd/object.png").expect("a valid key");
        disk.put(&path, &object).await.expect("the write succeeds");

        let response = Attachment::from_disk(&disk, &path)
            .await
            .expect("the object is readable")
            .with_filename(&safe("photo.png"))
            .into_response();

        assert_eq!(
            header_value(&response, header::CONTENT_TYPE).as_deref(),
            Some("image/png")
        );
        assert_eq!(
            header_value(&response, header::CONTENT_LENGTH).as_deref(),
            Some(object.len().to_string().as_str())
        );

        let served = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is readable");
        assert_eq!(
            served.as_ref(),
            object.as_slice(),
            "no byte was lost at the seam"
        );
    }

    #[tokio::test]
    async fn an_object_shorter_than_the_sniff_window_survives_intact() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let config =
            StorageConfig::fs(root.path().to_string_lossy().into_owned()).expect("a valid root");
        let storage = Storage::connect(config).await.expect("the disk connects");
        let disk = storage.default_disk();

        let path = StoragePath::new("small.png").expect("a valid key");
        disk.put(&path, PNG).await.expect("the write succeeds");

        let response = Attachment::from_disk(&disk, &path)
            .await
            .expect("the object is readable")
            .into_response();
        let served = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is readable");
        assert_eq!(served.as_ref(), PNG);
    }
}
