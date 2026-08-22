//! Bounds on a `multipart/form-data` request body.
//!
//! A multipart body is a stream the client writes and the server reads until
//! the client stops. Every one of those is an attacker's choice, and each is a
//! separate way to exhaust a process:
//!
//! | What the client controls | The attack | The bound here |
//! |---|---|---|
//! | how many bytes it sends in total | one request that fills the disk or the heap | [`MultipartLimits::with_total_bytes`] |
//! | how many bytes it puts in one part | a single part that is the whole body | [`MultipartLimits::with_field_bytes`] |
//! | how many parts it declares | a thousand tiny parts, each cheap, together not | [`MultipartLimits::with_fields`] |
//! | how *slowly* it sends them | a byte a minute, holding a task and a socket open | [`MultipartLimits::with_read_timeout`] |
//!
//! The third row is the one that is usually missed. A body-size cap says
//! nothing about part *count*: fifty thousand parts of two bytes each fit
//! inside a 1 MiB body, and the cost of a part is not its length -- it is a
//! header parse, an allocation and, in most upload handlers, a filename
//! sanitization and a storage round trip. The fourth row is missed for the
//! opposite reason: it is not about size at all, and no size cap can catch
//! it, because the request that never finishes never exceeds anything.
//!
//! # This is the inner bound, not the only one
//!
//! Stage 12 of the request pipeline ([`crate::application::pipeline`]) is
//! `tower-http`'s `RequestBodyLimitLayer`, and axum's `Multipart` extractor
//! reads through it. That limit applies to the *body*, before a byte reaches
//! the parser, and it stays the outer wall: nothing here can raise it, and
//! nothing here needs to, because a body over it is refused with a `413`
//! without being buffered.
//!
//! What that limit cannot express is the other three rows of the table. It
//! counts bytes, and it counts them once for the whole body.
//! [`MultipartLimits`] is the inner bound that knows the body has *parts*: a
//! per-part cap, a part count, and a clock. Carrying a total here as well is
//! not redundant -- an application that never configured stage 12 still gets
//! one, and an application that did gets to make a single upload route
//! stricter than the rest of the application without loosening anything.
//!
//! # The per-route override
//!
//! [`MultipartLimits`] is a [`tower::Layer`]. Applying it to a route puts the
//! configuration in that request's extensions, where an upload extractor
//! reads it back with [`MultipartLimits::from_extensions`]:
//!
//! ```
//! use arcature::http::multipart::MultipartLimits;
//!
//! let router: axum::Router = axum::Router::new().route(
//!     "/avatar",
//!     axum::routing::post(|| async { "ok" }).layer(
//!         MultipartLimits::new()
//!             .with_total_bytes(2 * 1024 * 1024)
//!             .with_fields(4),
//!     ),
//! );
//! # let _ = router;
//! ```
//!
//! A request whose route carries no layer gets [`MultipartLimits::new`], the
//! conservative defaults below. There is no way to end up with *no* bound.

use std::convert::Infallible;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::Multipart;
use axum::extract::multipart::Field;
use axum::http::{Extensions, Request, Response, StatusCode};
use bytes::Bytes;
use tower::{Layer, Service};

use crate::api::{Problem, ProblemKind};

/// The default cap on the whole decoded body: 16 MiB.
///
/// Sized for the documents and photographs an ordinary form accepts, not for
/// video. An application that needs more says so; an application that needs
/// less should say that too, and most should.
pub const DEFAULT_TOTAL_BYTES: u64 = 16 * 1024 * 1024;

/// The default cap on any single part: 8 MiB.
///
/// Deliberately below [`DEFAULT_TOTAL_BYTES`], so the common shape -- one file
/// beside a handful of text inputs -- cannot be turned into one part that is
/// the entire budget.
pub const DEFAULT_FIELD_BYTES: u64 = 8 * 1024 * 1024;

/// The default cap on the number of parts: 32.
///
/// A form with more than thirty-two parts in it is a form nobody designed.
pub const DEFAULT_FIELDS: usize = 32;

/// The default limit on how long a single read may block: 30 seconds.
///
/// Per read, not per request: a large upload over a slow link is many reads
/// that each return promptly, and it is not what this refuses. What it
/// refuses is the connection that goes quiet with the request half-sent.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// What a `multipart/form-data` body is allowed to cost.
///
/// Construct with [`MultipartLimits::new`] and narrow with the `with_*`
/// methods. Apply it to a route as a [`tower::Layer`] to override the defaults
/// there; read it back inside an extractor with
/// [`MultipartLimits::from_extensions`].
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use arcature::http::multipart::MultipartLimits;
///
/// let limits = MultipartLimits::new()
///     .with_total_bytes(4 * 1024 * 1024)
///     .with_field_bytes(4 * 1024 * 1024)
///     .with_fields(8)
///     .with_read_timeout(Duration::from_secs(10));
///
/// assert_eq!(limits.fields(), 8);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultipartLimits {
    total_bytes: u64,
    field_bytes: u64,
    fields: usize,
    read_timeout: Duration,
}

impl MultipartLimits {
    /// The conservative defaults: [`DEFAULT_TOTAL_BYTES`],
    /// [`DEFAULT_FIELD_BYTES`], [`DEFAULT_FIELDS`], [`DEFAULT_READ_TIMEOUT`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            total_bytes: DEFAULT_TOTAL_BYTES,
            field_bytes: DEFAULT_FIELD_BYTES,
            fields: DEFAULT_FIELDS,
            read_timeout: DEFAULT_READ_TIMEOUT,
        }
    }

    /// Cap the total number of decoded body bytes.
    #[must_use]
    pub const fn with_total_bytes(mut self, bytes: u64) -> Self {
        self.total_bytes = bytes;
        self
    }

    /// Cap the number of bytes in any single part.
    #[must_use]
    pub const fn with_field_bytes(mut self, bytes: u64) -> Self {
        self.field_bytes = bytes;
        self
    }

    /// Cap the number of parts.
    ///
    /// Zero means the body may contain no parts at all. That is a coherent
    /// thing to say, and it is not read as "unlimited" -- nothing here reads
    /// any value as unlimited.
    #[must_use]
    pub const fn with_fields(mut self, fields: usize) -> Self {
        self.fields = fields;
        self
    }

    /// Cap how long a single read of the body may block.
    #[must_use]
    pub const fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// The total-bytes cap.
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// The per-part bytes cap.
    #[must_use]
    pub const fn field_bytes(&self) -> u64 {
        self.field_bytes
    }

    /// The part-count cap.
    #[must_use]
    pub const fn fields(&self) -> usize {
        self.fields
    }

    /// The per-read timeout.
    #[must_use]
    pub const fn read_timeout(&self) -> Duration {
        self.read_timeout
    }

    /// The limits that apply to one request: the route's override if the
    /// layer was applied, otherwise [`MultipartLimits::new`].
    ///
    /// Never `None`. An extractor on a route that forgot the layer still runs
    /// bounded, because "no limits configured" is the failure mode this
    /// module exists to prevent.
    #[must_use]
    pub fn from_extensions(extensions: &Extensions) -> Self {
        extensions
            .get::<MultipartLimits>()
            .copied()
            .unwrap_or_default()
    }
}

impl Default for MultipartLimits {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for MultipartLimits {
    type Service = MultipartLimitsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MultipartLimitsService {
            inner,
            limits: *self,
        }
    }
}

/// The service [`MultipartLimits`] wraps around: it puts the limits in the
/// request extensions and does nothing else.
#[derive(Clone, Debug)]
pub struct MultipartLimitsService<S> {
    inner: S,
    limits: MultipartLimits,
}

impl<S, B> Service<Request<B>> for MultipartLimitsService<S>
where
    S: Service<Request<B>, Response = Response<axum::body::Body>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = Response<axum::body::Body>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: Request<B>) -> Self::Future {
        // Swap in the clone and drive the original: only the original is
        // known ready, and `poll_ready` readiness does not survive cloning.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        request.extensions_mut().insert(self.limits);
        Box::pin(async move { inner.call(request).await })
    }
}

/// A bound was exceeded, or the parser refused the body.
///
/// Neither `Display` nor [`MultipartError::problem`] quotes the request. They
/// name the limit that was hit -- which the client already knew -- and
/// nothing else.
#[non_exhaustive]
#[derive(Debug)]
pub enum MultipartError {
    /// The body declared more parts than [`MultipartLimits::fields`] allows.
    TooManyFields {
        /// The cap that was reached.
        limit: usize,
    },
    /// One part was longer than [`MultipartLimits::field_bytes`] allows.
    FieldTooLarge {
        /// The cap that was exceeded.
        limit: u64,
    },
    /// The body was longer than [`MultipartLimits::total_bytes`] allows.
    BodyTooLarge {
        /// The cap that was exceeded.
        limit: u64,
    },
    /// A single read blocked for longer than
    /// [`MultipartLimits::read_timeout`].
    ReadTimeout {
        /// The timeout that elapsed.
        after: Duration,
    },
    /// The multipart parser rejected the body.
    Parse {
        /// The upstream axum error.
        source: axum::extract::multipart::MultipartError,
    },
}

impl MultipartError {
    /// The status this failure deserves.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::TooManyFields { .. } | Self::FieldTooLarge { .. } | Self::BodyTooLarge { .. } => {
                StatusCode::PAYLOAD_TOO_LARGE
            }
            Self::ReadTimeout { .. } => StatusCode::REQUEST_TIMEOUT,
            // axum already maps the parser's own errors, including the body
            // limit from stage 12 arriving as a `StreamReadFailed`.
            Self::Parse { source } => source.status(),
        }
    }

    /// The RFC 9457 problem document for this failure.
    ///
    /// The `detail` is a fixed per-category string. Axum's
    /// `MultipartError::body_text` is deliberately not used: it can carry the
    /// parser's own message, which quotes header bytes the client wrote.
    #[must_use]
    pub fn problem(&self) -> Problem {
        let kind = match self.status() {
            StatusCode::PAYLOAD_TOO_LARGE => ProblemKind::PayloadTooLarge,
            StatusCode::REQUEST_TIMEOUT => ProblemKind::Timeout,
            StatusCode::BAD_REQUEST => ProblemKind::BadRequest,
            // A parser failure axum maps to 5xx is ours, not the client's.
            _ => ProblemKind::Internal,
        };
        let detail = match self {
            Self::TooManyFields { .. } => "Request has too many multipart fields",
            Self::FieldTooLarge { .. } => "A multipart field is too large",
            Self::BodyTooLarge { .. } => "Request body is too large",
            Self::ReadTimeout { .. } => "Timed out reading the request body",
            Self::Parse { .. } => "Request body is not a well-formed multipart/form-data body",
        };
        Problem::of(kind).with_detail(detail)
    }
}

impl fmt::Display for MultipartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyFields { limit } => {
                write!(formatter, "multipart body has more than {limit} fields")
            }
            Self::FieldTooLarge { limit } => {
                write!(formatter, "a multipart field exceeded {limit} bytes")
            }
            Self::BodyTooLarge { limit } => {
                write!(formatter, "the multipart body exceeded {limit} bytes")
            }
            Self::ReadTimeout { after } => write!(
                formatter,
                "reading the multipart body blocked for more than {after:?}"
            ),
            Self::Parse { .. } => write!(formatter, "the multipart body could not be parsed"),
        }
    }
}

impl std::error::Error for MultipartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse { source } => Some(source),
            _ => None,
        }
    }
}

/// Byte and part counters, shared between a [`BoundedMultipart`] and whichever
/// [`BoundedField`] it has currently handed out.
#[derive(Debug, Default)]
struct Counters {
    fields: usize,
    total: u64,
}

/// A [`Multipart`] that stops when a [`MultipartLimits`] is exceeded.
///
/// This wraps axum's parser rather than replacing it. Every read goes through
/// a timeout, every part is counted before it is handed out, and every chunk
/// is added to both a per-part and a whole-body total. The first bound
/// crossed ends the request.
///
/// Parts are handed out one at a time and borrow the parser. That is
/// `multer`'s requirement, not a choice made here: a multipart body is a
/// stream, and part *n+1* cannot be read before part *n* has been consumed.
pub struct BoundedMultipart {
    inner: Multipart,
    limits: MultipartLimits,
    counters: Counters,
}

impl fmt::Debug for BoundedMultipart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundedMultipart")
            .field("limits", &self.limits)
            .field("counters", &self.counters)
            .finish_non_exhaustive()
    }
}

impl BoundedMultipart {
    /// Bound an already-extracted [`Multipart`].
    #[must_use]
    pub fn new(inner: Multipart, limits: MultipartLimits) -> Self {
        Self {
            inner,
            limits,
            counters: Counters::default(),
        }
    }

    /// The limits in force.
    #[must_use]
    pub fn limits(&self) -> MultipartLimits {
        self.limits
    }

    /// How many parts have been handed out so far.
    #[must_use]
    pub fn fields_read(&self) -> usize {
        self.counters.fields
    }

    /// How many body bytes have been read so far, across every part.
    #[must_use]
    pub fn bytes_read(&self) -> u64 {
        self.counters.total
    }

    /// The next part, or `None` at the end of the body.
    ///
    /// # Errors
    ///
    /// [`MultipartError::TooManyFields`] once the part count is reached,
    /// [`MultipartError::ReadTimeout`] if the read blocks, and
    /// [`MultipartError::Parse`] if the body is malformed.
    pub async fn next_field(&mut self) -> Result<Option<BoundedField<'_>>, MultipartError> {
        // Destructured so the returned part can borrow the parser mutably
        // while the counters are borrowed mutably beside it.
        let Self {
            inner,
            limits,
            counters,
        } = self;
        let limits = *limits;

        // Checked *before* the read, so the part past the cap is refused
        // rather than parsed and then refused.
        if counters.fields >= limits.fields {
            return Err(MultipartError::TooManyFields {
                limit: limits.fields,
            });
        }

        let field = tokio::time::timeout(limits.read_timeout, inner.next_field())
            .await
            .map_err(|_| MultipartError::ReadTimeout {
                after: limits.read_timeout,
            })?
            .map_err(|source| MultipartError::Parse { source })?;

        match field {
            None => Ok(None),
            Some(field) => {
                counters.fields += 1;
                Ok(Some(BoundedField {
                    field,
                    limits,
                    counters,
                    field_bytes: 0,
                }))
            }
        }
    }
}

/// One part of a [`BoundedMultipart`], read a chunk at a time.
pub struct BoundedField<'a> {
    field: Field<'a>,
    limits: MultipartLimits,
    counters: &'a mut Counters,
    field_bytes: u64,
}

impl fmt::Debug for BoundedField<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundedField")
            .field("name", &self.field.name())
            .field("bytes_read", &self.field_bytes)
            .finish_non_exhaustive()
    }
}

impl BoundedField<'_> {
    /// The part's form field name.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.field.name()
    }

    /// The part's `filename=` parameter, exactly as the client wrote it.
    ///
    /// Raw and unsanitized. Put it through
    /// [`SafeFilename::parse`](crate::storage::SafeFilename::parse) before it
    /// is displayed, and never resolve it as a path.
    #[must_use]
    pub fn file_name(&self) -> Option<&str> {
        self.field.file_name()
    }

    /// The part's `Content-Type` header, exactly as the client wrote it.
    ///
    /// Named "declared" because that is all it is: a claim, made by the
    /// client, about bytes the client also chose. Nothing in Arcature decides
    /// what a file *is* from this value.
    #[must_use]
    pub fn declared_content_type(&self) -> Option<&str> {
        self.field.content_type()
    }

    /// How many bytes of this part have been read so far.
    #[must_use]
    pub fn byte_len(&self) -> u64 {
        self.field_bytes
    }

    /// The next chunk of this part, or `None` at the end of it.
    ///
    /// # Errors
    ///
    /// [`MultipartError::FieldTooLarge`] or [`MultipartError::BodyTooLarge`]
    /// as soon as the chunk that crosses a cap arrives -- it is counted and
    /// then refused, so nothing past a cap is ever returned to the caller.
    /// [`MultipartError::ReadTimeout`] if the read blocks, and
    /// [`MultipartError::Parse`] if the body is malformed.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, MultipartError> {
        let chunk = tokio::time::timeout(self.limits.read_timeout, self.field.chunk())
            .await
            .map_err(|_| MultipartError::ReadTimeout {
                after: self.limits.read_timeout,
            })?
            .map_err(|source| MultipartError::Parse { source })?;

        let Some(chunk) = chunk else { return Ok(None) };

        let len = chunk.len() as u64;
        self.field_bytes = self.field_bytes.saturating_add(len);
        if self.field_bytes > self.limits.field_bytes {
            return Err(MultipartError::FieldTooLarge {
                limit: self.limits.field_bytes,
            });
        }
        self.counters.total = self.counters.total.saturating_add(len);
        if self.counters.total > self.limits.total_bytes {
            return Err(MultipartError::BodyTooLarge {
                limit: self.limits.total_bytes,
            });
        }
        Ok(Some(chunk))
    }

    /// Read the whole part into memory.
    ///
    /// Bounded by [`MultipartLimits::field_bytes`], which is the only reason
    /// this is safe to offer at all -- and it is for the *small* parts of a
    /// form, the text inputs beside the file. A file is streamed with
    /// [`chunk`](Self::chunk); buffering one defeats the point of a streaming
    /// parser.
    ///
    /// # Errors
    ///
    /// As [`chunk`](Self::chunk).
    pub async fn bytes(mut self) -> Result<Bytes, MultipartError> {
        let mut buffer = Vec::new();
        while let Some(chunk) = self.chunk().await? {
            buffer.extend_from_slice(&chunk);
        }
        Ok(Bytes::from(buffer))
    }

    /// Read the whole part as UTF-8 text, or `Ok(None)` if it is not UTF-8.
    ///
    /// Not-UTF-8 is `None` rather than an error because it is not a failure of
    /// the *transfer*: the body arrived intact and within every bound, and it
    /// is the caller's schema, not this parser, that decided the part was
    /// meant to be text.
    ///
    /// # Errors
    ///
    /// As [`chunk`](Self::chunk).
    pub async fn text(self) -> Result<Option<String>, MultipartError> {
        let bytes = self.bytes().await?;
        Ok(String::from_utf8(bytes.to_vec()).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::extract::FromRequest;

    const BOUNDARY: &str = "XbCaRcAtUrE";

    /// Build a `multipart/form-data` body from `(name, filename, content)`
    /// triples, and extract an axum `Multipart` from it.
    async fn multipart(parts: &[(&str, Option<&str>, &[u8])]) -> Multipart {
        let mut body: Vec<u8> = Vec::new();
        for (name, filename, content) in parts {
            body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            match filename {
                Some(filename) => body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n\r\n"
                    )
                    .as_bytes(),
                ),
                None => body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                ),
            }
            body.extend_from_slice(content);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

        let request = Request::builder()
            .method("POST")
            .header(
                axum::http::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(body))
            .expect("the hand-built multipart request is well-formed");
        Multipart::from_request(request, &())
            .await
            .expect("the boundary is valid")
    }

    /// Drain every part, returning how many were read.
    async fn drain(bounded: &mut BoundedMultipart) -> Result<usize, MultipartError> {
        let mut seen = 0;
        while let Some(mut field) = bounded.next_field().await? {
            while field.chunk().await?.is_some() {}
            seen += 1;
        }
        Ok(seen)
    }

    #[test]
    fn the_defaults_are_the_documented_ones() {
        let limits = MultipartLimits::new();
        assert_eq!(limits.total_bytes(), DEFAULT_TOTAL_BYTES);
        assert_eq!(limits.field_bytes(), DEFAULT_FIELD_BYTES);
        assert_eq!(limits.fields(), DEFAULT_FIELDS);
        assert_eq!(limits.read_timeout(), DEFAULT_READ_TIMEOUT);
        assert_eq!(limits, MultipartLimits::default());
    }

    #[test]
    fn a_request_with_no_layer_is_still_bounded() {
        let extensions = Extensions::new();
        assert_eq!(
            MultipartLimits::from_extensions(&extensions),
            MultipartLimits::new()
        );
    }

    #[test]
    fn the_route_override_is_read_back_from_the_extensions() {
        let configured = MultipartLimits::new().with_fields(3).with_total_bytes(99);
        let mut extensions = Extensions::new();
        extensions.insert(configured);
        assert_eq!(MultipartLimits::from_extensions(&extensions), configured);
    }

    #[tokio::test]
    async fn an_ordinary_form_passes_every_bound() {
        let mut bounded = BoundedMultipart::new(
            multipart(&[
                ("title", None, b"holiday"),
                ("file", Some("photo.png"), b"\x89PNG\r\n\x1a\n"),
            ])
            .await,
            MultipartLimits::new(),
        );
        assert_eq!(drain(&mut bounded).await.unwrap(), 2);
        assert_eq!(bounded.fields_read(), 2);
        assert_eq!(bounded.bytes_read(), 7 + 8);
    }

    #[tokio::test]
    async fn a_thousand_tiny_fields_are_refused_on_the_count() {
        // Every part is two bytes: the whole body is well inside any
        // byte-based cap, and only the count catches it.
        let parts: Vec<(String, Vec<u8>)> = (0..1000)
            .map(|index| (format!("f{index}"), b"ab".to_vec()))
            .collect();
        let borrowed: Vec<(&str, Option<&str>, &[u8])> = parts
            .iter()
            .map(|(name, content)| (name.as_str(), None, content.as_slice()))
            .collect();

        let mut bounded = BoundedMultipart::new(
            multipart(&borrowed).await,
            // A generous byte budget on purpose: the count is the only bound
            // that can fail this body.
            MultipartLimits::new()
                .with_total_bytes(u64::MAX)
                .with_field_bytes(u64::MAX)
                .with_fields(4),
        );

        let error = drain(&mut bounded).await.unwrap_err();
        assert!(matches!(error, MultipartError::TooManyFields { limit: 4 }));
        assert_eq!(error.status(), StatusCode::PAYLOAD_TOO_LARGE);
        // Exactly the cap was parsed, and not one part more.
        assert_eq!(bounded.fields_read(), 4);
    }

    #[tokio::test]
    async fn one_oversized_field_is_refused_on_the_per_field_cap() {
        let big = vec![b'x'; 4096];
        let mut bounded = BoundedMultipart::new(
            multipart(&[("file", Some("big.bin"), &big)]).await,
            MultipartLimits::new()
                .with_total_bytes(u64::MAX)
                .with_field_bytes(1024),
        );
        let error = drain(&mut bounded).await.unwrap_err();
        assert!(matches!(
            error,
            MultipartError::FieldTooLarge { limit: 1024 }
        ));
        assert_eq!(error.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn many_legal_fields_together_are_refused_on_the_total() {
        // Each part is inside the per-part cap; only their sum is not.
        let chunk = vec![b'y'; 512];
        let parts: Vec<(&str, Option<&str>, &[u8])> = (0..8)
            .map(|_| ("f", None::<&str>, chunk.as_slice()))
            .collect();
        let mut bounded = BoundedMultipart::new(
            multipart(&parts).await,
            MultipartLimits::new()
                .with_field_bytes(1024)
                .with_total_bytes(2048),
        );
        let error = drain(&mut bounded).await.unwrap_err();
        assert!(matches!(
            error,
            MultipartError::BodyTooLarge { limit: 2048 }
        ));
    }

    #[tokio::test]
    async fn a_body_that_stops_mid_part_times_out_rather_than_hanging() {
        // A body that promises more and never sends it: the stream stays
        // open, so without the clock this read never returns.
        use futures::StreamExt as _;
        let head = format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"slow.bin\"\r\n\r\npartial"
        );
        let stream =
            futures::stream::once(async move { Ok::<Bytes, std::io::Error>(Bytes::from(head)) })
                .chain(futures::stream::pending::<Result<Bytes, std::io::Error>>());
        let body = Body::from_stream(stream);

        let request = Request::builder()
            .method("POST")
            .header(
                axum::http::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(body)
            .expect("the hand-built multipart request is well-formed");
        let inner = Multipart::from_request(request, &())
            .await
            .expect("the boundary is valid");

        let mut bounded = BoundedMultipart::new(
            inner,
            MultipartLimits::new().with_read_timeout(Duration::from_millis(50)),
        );
        let error = drain(&mut bounded).await.unwrap_err();
        assert!(matches!(error, MultipartError::ReadTimeout { .. }));
        assert_eq!(error.status(), StatusCode::REQUEST_TIMEOUT);
    }

    #[tokio::test]
    async fn the_declared_content_type_is_carried_but_not_believed() {
        let mut bounded = BoundedMultipart::new(
            multipart(&[("file", Some("evil.jpg"), b"<?php echo 1; ?>")]).await,
            MultipartLimits::new(),
        );
        let field = bounded.next_field().await.unwrap().unwrap();
        assert_eq!(field.file_name(), Some("evil.jpg"));
        // The parser reports the header verbatim; nothing here acts on it.
        assert_eq!(field.declared_content_type(), None);
    }

    #[test]
    fn every_bound_reports_a_problem_that_never_quotes_the_request() {
        for error in [
            MultipartError::TooManyFields { limit: 1 },
            MultipartError::FieldTooLarge { limit: 1 },
            MultipartError::BodyTooLarge { limit: 1 },
            MultipartError::ReadTimeout {
                after: Duration::from_secs(1),
            },
        ] {
            let problem = error.problem();
            assert_eq!(problem.status(), error.status());
            let json = problem.to_json();
            let detail = json["detail"].as_str().unwrap_or_default();
            assert!(!detail.is_empty());
            // The limit is a server-side number; it never reaches the body.
            assert!(!detail.contains('1'));
        }
    }
}
