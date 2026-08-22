//! An extractor for one uploaded file, with every upload check applied
//! before the handler runs.
//!
//! # What it does before a handler sees anything
//!
//! An upload endpoint has to get five separate things right, and the failure
//! mode of forgetting any one of them is a stored file that is not what it
//! looks like. [`UploadedFile`] does all five as a condition of extraction,
//! so a handler that compiles has them:
//!
//! 1. **Bounds.** Total size, per-part size, part count and a read timeout,
//!    from [`MultipartLimits`] in the request extensions -- conservative
//!    defaults if no route override was applied.
//! 2. **A whitelisted extension.** From [`UploadPolicy`] in the request
//!    extensions, defaulting to [`AllowedExtensions::images`].
//! 3. **A sanitized filename.** [`SafeFilename`] strips the path, the
//!    controls, the bidi overrides and the Windows device names, and the
//!    result is metadata -- never a key.
//! 4. **Bytes that match the extension.** [`sniff::verify`] holds the two to
//!    agreement, so `shell.php` renamed `avatar.png` is refused here rather
//!    than discovered later.
//! 5. **A client-safe rejection.** Every refusal is an RFC 9457 problem
//!    document built by [`validation_problem`], so an upload failure reads
//!    like any other field validation failure and no part of the request is
//!    quoted back.
//!
//! # This one buffers, on purpose, and it is the small-file path
//!
//! An extractor runs to completion before the handler is called, so it has
//! nowhere to put the bytes except memory. That buffer is bounded by
//! [`MultipartLimits::field_bytes`], and lowering that on an upload route is
//! how an application keeps it small.
//!
//! For anything bigger than an avatar, do not use this. Drive
//! [`BoundedMultipart`] in the handler and write each chunk through
//! [`UploadWriter`](crate::storage::UploadWriter), which never holds more
//! than one chunk regardless of object size. This extractor is the
//! convenience; that is the load-bearing path.
//!
//! # It is not authorization
//!
//! A validated upload is not an authorized upload, exactly as a validated
//! request is not an authorized request. Whether this user may write to this
//! disk under this prefix is a separate, explicit decision.

use std::borrow::Cow;
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Bytes;
use axum::extract::{FromRequest, Multipart, Request};
use axum::http::Extensions;
use axum::response::{IntoResponse, Response};
use tower::{Layer, Service};
use validator::{ValidationError, ValidationErrors};

use crate::api::Problem;
use crate::http::multipart::{BoundedMultipart, MultipartLimits};
use crate::storage::content::ContentAddress;
use crate::storage::error::UploadError;
use crate::storage::sniff::{self, SniffedType};
use crate::storage::{AllowedExtensions, Disk, SafeFilename};
use crate::validation::errors::validation_problem;
use crate::validation::rejection::from_multipart_rejection;

/// The field name every upload rejection is reported under.
///
/// A fixed key, never the part's own name. The part name is a string the
/// client wrote, and the rejection module's rule is that no part of the
/// request is reflected in a response body.
pub const UPLOAD_FIELD: &str = "file";

/// What an upload route will accept.
///
/// A [`tower::Layer`], applied to the routes that take uploads. A route
/// without it gets [`UploadPolicy::new`] -- the image whitelist -- because
/// "no policy configured" must not mean "anything goes".
///
/// # Example
///
/// ```
/// use arcature::storage::AllowedExtensions;
/// use arcature::validation::upload::UploadPolicy;
///
/// let policy = UploadPolicy::new(AllowedExtensions::documents()).with_field("attachment");
///
/// assert_eq!(policy.field(), Some("attachment"));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadPolicy {
    allowed: AllowedExtensions,
    field: Option<String>,
}

impl UploadPolicy {
    /// A policy admitting `allowed` and taking the first file part it finds.
    #[must_use]
    pub fn new(allowed: AllowedExtensions) -> Self {
        Self {
            allowed,
            field: None,
        }
    }

    /// Require the file to arrive in the part named `field`.
    ///
    /// Without this the first part carrying a `filename=` wins, which is
    /// convenient and slightly sloppy: a form with two file inputs becomes
    /// order-dependent. Naming the field removes the ambiguity.
    #[must_use]
    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    /// The extensions this policy admits.
    #[must_use]
    pub fn allowed(&self) -> &AllowedExtensions {
        &self.allowed
    }

    /// The required part name, if one was set.
    #[must_use]
    pub fn field(&self) -> Option<&str> {
        self.field.as_deref()
    }

    /// The policy for one request: the route's override if the layer was
    /// applied, otherwise [`UploadPolicy::new`] with the image whitelist.
    ///
    /// Never `None`, for the same reason
    /// [`MultipartLimits::from_extensions`] is never `None`.
    #[must_use]
    pub fn from_extensions(extensions: &Extensions) -> Self {
        extensions
            .get::<UploadPolicy>()
            .cloned()
            .unwrap_or_default()
    }
}

impl Default for UploadPolicy {
    fn default() -> Self {
        Self::new(AllowedExtensions::images())
    }
}

impl<S> Layer<S> for UploadPolicy {
    type Service = UploadPolicyService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        UploadPolicyService {
            inner,
            policy: self.clone(),
        }
    }
}

/// The service [`UploadPolicy`] wraps around: it puts the policy in the
/// request extensions and does nothing else.
#[derive(Clone, Debug)]
pub struct UploadPolicyService<S> {
    inner: S,
    policy: UploadPolicy,
}

impl<S, B> Service<axum::http::Request<B>> for UploadPolicyService<S>
where
    S: Service<axum::http::Request<B>, Response = Response, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: axum::http::Request<B>) -> Self::Future {
        // Swap in the clone and drive the original: only the original is
        // known ready, and `poll_ready` readiness does not survive cloning.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        request.extensions_mut().insert(self.policy.clone());
        Box::pin(async move { inner.call(request).await })
    }
}

/// One uploaded file that has passed every check in the
/// [module documentation](self).
///
/// The bytes are in memory, bounded by
/// [`MultipartLimits::field_bytes`](crate::http::MultipartLimits::field_bytes).
/// [`UploadedFile::store`] writes them to a disk under their own content
/// address.
///
/// # Example
///
/// ```no_run
/// use arcature::storage::Disk;
/// use arcature::validation::upload::UploadedFile;
///
/// async fn store_avatar(disk: Disk, upload: UploadedFile) -> arcature::Result<String> {
///     // Sanitized label, verified bytes, whitelisted extension -- all true
///     // before this line runs.
///     let address = upload
///         .store_under(&disk, "avatars")
///         .await
///         .map_err(|_| arcature::bad_request("that file could not be stored"))?;
///
///     Ok(address.path().as_str().to_string())
/// }
/// ```
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct UploadedFile {
    filename: SafeFilename,
    content_type: Option<SniffedType>,
    bytes: Bytes,
}

impl UploadedFile {
    /// The sanitized filename. **Metadata**: show it, send it in a
    /// `Content-Disposition`, never resolve it as a path.
    #[must_use]
    pub fn filename(&self) -> &SafeFilename {
        &self.filename
    }

    /// What the bytes were recognized as, if their format has a signature.
    ///
    /// Derived from the object, never from the part's `Content-Type` header.
    #[must_use]
    pub fn content_type(&self) -> Option<SniffedType> {
        self.content_type
    }

    /// The file's bytes.
    #[must_use]
    pub fn bytes(&self) -> &Bytes {
        &self.bytes
    }

    /// How many bytes the file has.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    /// Consume the wrapper and return the bytes.
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.bytes
    }

    /// Write the file to `disk` under its content-addressed key.
    ///
    /// # Errors
    ///
    /// Returns [`UploadError::Storage`] if the backend fails, and
    /// [`UploadError::Content`] if the bytes and the extension disagree --
    /// which extraction already ruled out, so it means the object changed
    /// between the two, not that a client got through.
    pub async fn store(&self, disk: &Disk) -> Result<ContentAddress, UploadError> {
        self.write(disk, None).await
    }

    /// Write the file to `disk` under an application-chosen prefix.
    ///
    /// The prefix is the application's own string -- `"avatars"`, not
    /// anything off the request.
    ///
    /// # Errors
    ///
    /// As [`UploadedFile::store`], plus [`UploadError::Storage`] wrapping a
    /// path error if `prefix` is not a valid key.
    pub async fn store_under(
        &self,
        disk: &Disk,
        prefix: &str,
    ) -> Result<ContentAddress, UploadError> {
        self.write(disk, Some(prefix)).await
    }

    async fn write(
        &self,
        disk: &Disk,
        prefix: Option<&str>,
    ) -> Result<ContentAddress, UploadError> {
        let mut upload = match prefix {
            Some(prefix) => disk.begin_upload_under(prefix).await?,
            None => disk.begin_upload().await?,
        };
        if let Err(failure) = upload.write(self.bytes.clone()).await {
            // The staging object outlives a failed write unless something
            // removes it, and nothing else will.
            let _ = upload.abort().await;
            return Err(failure.into());
        }
        upload
            .finish_verified(self.filename.extension().clone())
            .await
    }
}

impl<S> FromRequest<S> for UploadedFile
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let limits = MultipartLimits::from_extensions(request.extensions());
        let policy = UploadPolicy::from_extensions(request.extensions());

        let multipart = Multipart::from_request(request, state)
            .await
            .map_err(|rejection| from_multipart_rejection(&rejection).into_response())?;
        let mut multipart = BoundedMultipart::new(multipart, limits);

        loop {
            let field = multipart
                .next_field()
                .await
                .map_err(|error| error.problem().into_response())?;
            let Some(field) = field else {
                return Err(upload_problem("missing", "No file was uploaded").into_response());
            };

            // The part's own name and filename have to be copied out before
            // the body is read: reading consumes the field.
            let name = field.name().map(str::to_owned);
            let Some(filename) = field.file_name().map(str::to_owned) else {
                // A part with no `filename=` is an ordinary form field. Read
                // and discard it so its bytes still count against the total
                // bound, then keep looking.
                let _ = field
                    .bytes()
                    .await
                    .map_err(|error| error.problem().into_response())?;
                continue;
            };
            if let Some(required) = policy.field()
                && name.as_deref() != Some(required)
            {
                let _ = field
                    .bytes()
                    .await
                    .map_err(|error| error.problem().into_response())?;
                continue;
            }

            let bytes = field
                .bytes()
                .await
                .map_err(|error| error.problem().into_response())?;

            let filename = SafeFilename::parse(&filename, policy.allowed()).map_err(|_| {
                upload_problem(
                    "extension",
                    "That filename or file type is not accepted here",
                )
                .into_response()
            })?;

            let content_type = sniff::verify(&bytes, filename.extension()).map_err(|_| {
                upload_problem(
                    "contents",
                    "The file's contents do not match its file extension",
                )
                .into_response()
            })?;

            return Ok(Self {
                filename,
                content_type,
                bytes,
            });
        }
    }
}

/// Build an RFC 9457 problem for an upload refusal, shaped exactly like any
/// other field validation failure.
///
/// `code` and `message` are both fixed strings chosen here. Nothing from the
/// request reaches the response: not the filename, not the declared content
/// type, not the sniffed one -- the last of those would let an uploader use
/// the endpoint as a free file-type oracle.
fn upload_problem(code: &'static str, message: &'static str) -> Problem {
    let mut error = ValidationError::new(code);
    error.message = Some(Cow::Borrowed(message));
    let mut errors = ValidationErrors::new();
    errors.add(UPLOAD_FIELD, error);
    validation_problem(errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{StatusCode, header};

    const BOUNDARY: &str = "XbArCaTuRe";
    const PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R',
    ];
    const PHP: &[u8] = b"<?php system($_GET['c']); ?>";

    /// Build a multipart body out of `(name, filename, bytes)` parts.
    fn body(parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, filename, bytes) in parts {
            out.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            out.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"").as_bytes(),
            );
            if let Some(filename) = filename {
                out.extend_from_slice(format!("; filename=\"{filename}\"").as_bytes());
            }
            out.extend_from_slice(b"\r\n\r\n");
            out.extend_from_slice(bytes);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        out
    }

    fn request(parts: &[(&str, Option<&str>, &[u8])], policy: Option<UploadPolicy>) -> Request {
        let mut builder = axum::http::Request::builder().method("POST").header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        );
        if let Some(policy) = policy {
            builder = builder.extension(policy);
        }
        builder.body(Body::from(body(parts))).expect("a request")
    }

    // The rejection is a whole `Response`, exactly as the extractor's own
    // `Rejection` type is; boxing it here would only hide the shape under test.
    #[allow(clippy::result_large_err)]
    async fn extract(
        parts: &[(&str, Option<&str>, &[u8])],
        policy: Option<UploadPolicy>,
    ) -> Result<UploadedFile, Response> {
        UploadedFile::from_request(request(parts, policy), &()).await
    }

    async fn problem_body(response: Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a readable body");
        (
            status,
            serde_json::from_slice(&bytes).expect("a problem document"),
        )
    }

    #[tokio::test]
    async fn a_png_under_a_png_name_is_extracted() {
        let file = extract(&[("file", Some("photo.PNG"), PNG)], None)
            .await
            .expect("a valid upload");

        assert_eq!(file.filename().to_string(), "photo.png");
        assert_eq!(
            file.content_type().map(|kind| kind.mime()),
            Some("image/png")
        );
        assert_eq!(file.byte_len(), PNG.len());
    }

    #[tokio::test]
    async fn a_php_script_renamed_to_png_is_refused() {
        let response = extract(&[("file", Some("avatar.png"), PHP)], None)
            .await
            .expect_err("a script is not an image");
        let (status, document) = problem_body(response).await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(document["errors"]["file"].to_string().contains("contents"));
    }

    #[tokio::test]
    async fn an_extension_off_the_whitelist_is_refused() {
        for name in ["shell.php", "payload.exe", "notes.txt"] {
            let response = extract(&[("file", Some(name), PNG)], None)
                .await
                .expect_err("the image whitelist refuses it");
            let (status, document) = problem_body(response).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{name}");
            assert!(
                document["errors"]["file"].to_string().contains("extension"),
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn a_hostile_filename_is_sanitized_rather_than_resolved() {
        let file = extract(&[("file", Some("../../etc/passwd.png"), PNG)], None)
            .await
            .expect("the path components are stripped, not obeyed");

        assert_eq!(file.filename().to_string(), "passwd.png");
        assert!(!file.filename().to_string().contains('/'));
    }

    #[tokio::test]
    async fn a_reserved_device_name_is_refused() {
        let response = extract(&[("file", Some("CON.png"), PNG)], None)
            .await
            .expect_err("CON is a device, not a file");
        assert_eq!(
            problem_body(response).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[tokio::test]
    async fn a_vietnamese_filename_survives() {
        let file = extract(&[("file", Some("ảnh đại diện.png"), PNG)], None)
            .await
            .expect("an accented name is a name");
        assert!(file.filename().to_string().ends_with(".png"));
    }

    #[tokio::test]
    async fn a_body_with_no_file_part_is_refused() {
        let response = extract(&[("title", None, b"hello")], None)
            .await
            .expect_err("there is no file here");
        let (status, document) = problem_body(response).await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(document["errors"]["file"].to_string().contains("missing"));
    }

    #[tokio::test]
    async fn ordinary_form_fields_are_skipped_to_reach_the_file() {
        let file = extract(
            &[
                ("title", None, b"my holiday"),
                ("file", Some("photo.png"), PNG),
            ],
            None,
        )
        .await
        .expect("the file is found past the text fields");
        assert_eq!(file.filename().to_string(), "photo.png");
    }

    #[tokio::test]
    async fn a_named_field_ignores_the_other_file_parts() {
        let policy = UploadPolicy::new(AllowedExtensions::images()).with_field("avatar");
        let file = extract(
            &[
                ("decoy", Some("first.png"), b"not an image at all"),
                ("avatar", Some("real.png"), PNG),
            ],
            Some(policy),
        )
        .await
        .expect("the named part is the one that counts");

        assert_eq!(file.filename().to_string(), "real.png");
    }

    #[tokio::test]
    async fn a_route_policy_widens_the_whitelist() {
        let pdf = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n";
        let policy = UploadPolicy::new(AllowedExtensions::documents());
        let file = extract(&[("file", Some("report.pdf"), pdf)], Some(policy))
            .await
            .expect("documents are allowed on this route");
        assert_eq!(
            file.content_type().map(|kind| kind.mime()),
            Some("application/pdf")
        );
    }

    #[tokio::test]
    async fn the_default_policy_is_the_narrow_one() {
        // A route that forgot the layer must not be the permissive route.
        let policy = UploadPolicy::from_extensions(&Extensions::new());
        assert_eq!(policy.allowed(), &AllowedExtensions::images());
        assert_eq!(policy.field(), None);
    }

    #[tokio::test]
    async fn no_part_of_the_request_is_quoted_back() {
        let response = extract(&[("file", Some("s3cret-name.php"), PHP)], None)
            .await
            .expect_err("refused");
        let (_, document) = problem_body(response).await;
        let rendered = document.to_string();

        assert!(!rendered.contains("s3cret-name"), "{rendered}");
        assert!(!rendered.contains("php"), "{rendered}");
    }

    #[tokio::test]
    async fn a_field_over_the_limit_is_a_413() {
        let limits = MultipartLimits::new().with_field_bytes(8);
        let mut request = request(&[("file", Some("photo.png"), PNG)], None);
        request.extensions_mut().insert(limits);

        let response = UploadedFile::from_request(request, &())
            .await
            .expect_err("the file is over the cap");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn a_body_that_is_not_multipart_is_refused_as_unsupported_media() {
        let request = axum::http::Request::builder()
            .method("POST")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .expect("a request");

        let response = UploadedFile::from_request(request, &())
            .await
            .expect_err("json is not a multipart body");
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn an_extracted_file_stores_under_its_own_digest() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let config = crate::storage::StorageConfig::fs(root.path().to_string_lossy().into_owned())
            .expect("a valid root");
        let storage = crate::storage::Storage::connect(config)
            .await
            .expect("the disk connects");
        let disk = storage.default_disk();

        let file = extract(&[("file", Some("photo.png"), PNG)], None)
            .await
            .expect("a valid upload");
        let address = file
            .store_under(&disk, "avatars")
            .await
            .expect("the store succeeds");

        assert!(address.path().as_str().ends_with(".png"));
        let stored = address.path_under("avatars").expect("a valid key");
        assert!(disk.exists(&stored).await.expect("the stat runs"));
        // Nothing was left behind in staging.
        assert!(disk.list(&stored).await.is_ok());
    }
}
