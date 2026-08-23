//! The Arcature error vocabulary.
//!
//! Arcature uses typed errors throughout its public API. Controllers write
//! `Result<Response>` (or `Result<T>`) and the framework maps the typed error
//! to the correct HTTP behavior: 404 for not found, 422 for validation, 401/403
//! for auth, 500 for internal failures, and the database/extension variants for
//! subsystem-specific failures.
//!
//! Production responses never leak secrets or internal implementation detail.
//! In development the response body is richer so a developer can diagnose a
//! failure without recompiling.

use std::fmt;

use axum::http::HeaderValue;

/// The single error type returned by Arcature framework code and the
/// `Result<T>` alias used across the public API.
///
/// Variants are kept intentionally small. Each carries only the context a
/// caller or operator needs; subsystems that need more detail attach it via
/// the [`Error::context`] string, which is redacted in production responses.
#[derive(Debug)]
pub enum Error {
    /// A resource was not found. Maps to HTTP 404.
    NotFound(String),
    /// The request was invalid beyond validation (e.g. a bad route parameter
    /// shape). Maps to HTTP 400.
    BadRequest(String),
    /// Authorization failed: the user is not authenticated. Maps to HTTP 401.
    Unauthorized,
    /// Authorization failed: the user is authenticated but not allowed. Maps
    /// to HTTP 403.
    Forbidden(String),
    /// Request validation failed. Maps to HTTP 422 and, on browser routes,
    /// integrates with Inertia shared errors.
    Validation(Vec<ValidationError>),
    /// A configured redirect target was rejected (open-redirect guard).
    Redirect(String),
    /// An I/O failure (file storage, template read, etc.). Maps to HTTP 500.
    Io(String),
    /// A database error. Maps to HTTP 500.
    Database(String),
    /// A cache (Redis/Valkey) error.
    Cache(String),
    /// A storage (OpenDAL) error.
    Storage(String),
    /// A mail (SMTP) error.
    Mail(String),
    /// A job queue error.
    Job(String),
    /// A serialization error (serde_json). Maps to HTTP 500.
    Serialization(String),
    /// A configuration error surfaced before the server boots.
    Config(String),
    /// A generic, framework-internal failure that did not fit a more specific
    /// variant. Maps to HTTP 500.
    Other(String),
}

/// A single validation failure, keyed by the offending field path.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

impl Error {
    /// Attach human-readable context. Returns `self` for chaining.
    #[must_use]
    pub fn context(mut self, ctx: impl Into<String>) -> Self {
        match &mut self {
            Error::NotFound(s)
            | Error::BadRequest(s)
            | Error::Redirect(s)
            | Error::Io(s)
            | Error::Database(s)
            | Error::Cache(s)
            | Error::Storage(s)
            | Error::Mail(s)
            | Error::Job(s)
            | Error::Serialization(s)
            | Error::Config(s)
            | Error::Other(s) => {
                if s.is_empty() {
                    *s = ctx.into();
                } else {
                    *s = format!("{}: {}", *s, ctx.into());
                }
            }
            Error::Forbidden(s) => {
                if s.is_empty() {
                    *s = ctx.into();
                } else {
                    *s = format!("{}: {}", *s, ctx.into());
                }
            }
            Error::Unauthorized | Error::Validation(_) => {}
        }
        self
    }

    /// The canonical HTTP status code for this error.
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            Error::NotFound(_) => 404,
            Error::BadRequest(_) => 400,
            Error::Unauthorized => 401,
            Error::Forbidden(_) => 403,
            Error::Validation(_) => 422,
            Error::Redirect(_) => 400,
            Error::Io(_)
            | Error::Database(_)
            | Error::Cache(_)
            | Error::Storage(_)
            | Error::Mail(_)
            | Error::Job(_)
            | Error::Serialization(_)
            | Error::Config(_)
            | Error::Other(_) => 500,
        }
    }

    /// A short, stable error code (used by the API problem-detail `type`
    /// field and by `arc doctor`).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Error::NotFound(_) => "not_found",
            Error::BadRequest(_) => "bad_request",
            Error::Unauthorized => "unauthorized",
            Error::Forbidden(_) => "forbidden",
            Error::Validation(_) => "validation_failed",
            Error::Redirect(_) => "invalid_redirect",
            Error::Io(_) => "io_error",
            Error::Database(_) => "database_error",
            Error::Cache(_) => "cache_error",
            Error::Storage(_) => "storage_error",
            Error::Mail(_) => "mail_error",
            Error::Job(_) => "job_error",
            Error::Serialization(_) => "serialization_error",
            Error::Config(_) => "config_error",
            Error::Other(_) => "internal_error",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(s) => write!(f, "not found: {s}"),
            Error::BadRequest(s) => write!(f, "bad request: {s}"),
            Error::Unauthorized => write!(f, "unauthorized"),
            Error::Forbidden(s) => write!(f, "forbidden: {s}"),
            Error::Validation(errs) => {
                write!(f, "validation failed: ")?;
                for (i, e) in errs.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{}: {}", e.field, e.message)?;
                }
                Ok(())
            }
            Error::Redirect(s) => write!(f, "invalid redirect: {s}"),
            Error::Io(s) => write!(f, "io error: {s}"),
            Error::Database(s) => write!(f, "database error: {s}"),
            Error::Cache(s) => write!(f, "cache error: {s}"),
            Error::Storage(s) => write!(f, "storage error: {s}"),
            Error::Mail(s) => write!(f, "mail error: {s}"),
            Error::Job(s) => write!(f, "job error: {s}"),
            Error::Serialization(s) => write!(f, "serialization error: {s}"),
            Error::Config(s) => write!(f, "config error: {s}"),
            Error::Other(s) => write!(f, "internal error: {s}"),
        }
    }
}

impl std::error::Error for Error {}

// --- Convenience constructors ------------------------------------------------

/// Shorthand for a not-found error.
#[must_use]
pub fn not_found(what: impl Into<String>) -> Error {
    Error::NotFound(what.into())
}

/// Shorthand for a bad-request error.
#[must_use]
pub fn bad_request(what: impl Into<String>) -> Error {
    Error::BadRequest(what.into())
}

/// Shorthand for a forbidden error.
#[must_use]
pub fn forbidden(what: impl Into<String>) -> Error {
    Error::Forbidden(what.into())
}

// --- conversions from subsystem errors --------------------------------------

impl axum::response::IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        let status = axum::http::StatusCode::from_u16(self.status())
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);

        // Production responses never leak internal detail. Development
        // responses are richer so a developer can diagnose a failure without
        // recompiling. We use the `APP_ENV` environment variable: anything
        // other than `production`/`prod` is treated as development.
        let is_production = matches!(
            std::env::var("APP_ENV")
                .ok()
                .map(|v| v.to_ascii_lowercase())
                .as_deref(),
            Some("production") | Some("prod")
        );

        if is_production {
            return (
                status,
                [(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                )],
                serde_json::json!({
                    "type": format!("urn:arcature:problem:{}", self.code()),
                    "title": self.code(),
                    "status": self.status(),
                })
                .to_string(),
            )
                .into_response();
        }

        let body = match &self {
            Error::Validation(errs) => serde_json::json!({
                "type": format!("urn:arcature:problem:{}", self.code()),
                "title": "validation_failed",
                "status": self.status(),
                "errors": errs,
            }),
            other => serde_json::json!({
                "type": format!("urn:arcature:problem:{}", other.code()),
                "title": other.code(),
                "status": other.status(),
                "detail": other.to_string(),
            }),
        };

        (
            status,
            [(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            body.to_string(),
        )
            .into_response()
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

#[cfg(feature = "database")]
impl From<sea_orm::DbErr> for Error {
    fn from(e: sea_orm::DbErr) -> Self {
        // SeaORM's `RecordNotFound` maps to a 404 so model lookups compose
        // naturally with `?`.
        match e {
            sea_orm::DbErr::RecordNotFound(msg) => Error::NotFound(msg),
            other => Error::Database(other.to_string()),
        }
    }
}

#[cfg(feature = "database")]
impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Error::NotFound("row not found".to_string()),
            other => Error::Database(other.to_string()),
        }
    }
}

#[cfg(feature = "cache")]
impl From<redis::RedisError> for Error {
    fn from(e: redis::RedisError) -> Self {
        Error::Cache(e.to_string())
    }
}

#[cfg(feature = "cache")]
impl From<crate::cache::CacheError> for Error {
    fn from(e: crate::cache::CacheError) -> Self {
        Error::Cache(e.to_string())
    }
}

#[cfg(feature = "cache")]
impl From<crate::cache::CacheConnectError> for Error {
    fn from(e: crate::cache::CacheConnectError) -> Self {
        Error::Cache(e.to_string())
    }
}

#[cfg(feature = "mail")]
impl From<lettre::error::Error> for Error {
    fn from(e: lettre::error::Error) -> Self {
        Error::Mail(e.to_string())
    }
}

#[cfg(feature = "mail")]
impl From<crate::mail::MailSendError> for Error {
    fn from(e: crate::mail::MailSendError) -> Self {
        Error::Mail(e.to_string())
    }
}

#[cfg(feature = "mail")]
impl From<crate::mail::MailConfigError> for Error {
    fn from(e: crate::mail::MailConfigError) -> Self {
        Error::Mail(e.to_string())
    }
}

#[cfg(any(feature = "storage-fs", feature = "storage-s3"))]
impl From<opendal::Error> for Error {
    fn from(e: opendal::Error) -> Self {
        Error::Storage(e.to_string())
    }
}

#[cfg(any(feature = "storage-fs", feature = "storage-s3"))]
impl From<crate::storage::StorageError> for Error {
    fn from(e: crate::storage::StorageError) -> Self {
        Error::Storage(e.to_string())
    }
}

#[cfg(any(feature = "storage-fs", feature = "storage-s3"))]
impl From<crate::storage::StorageConnectError> for Error {
    fn from(e: crate::storage::StorageConnectError) -> Self {
        Error::Storage(e.to_string())
    }
}

/// The one conversion in this file that does not answer 500.
///
/// [`UploadError`](crate::storage::UploadError) is deliberately two halves,
/// and its own documentation says why: `Storage` is the server's problem and
/// `Content` is the client's, and "collapsing them into one error is how an
/// upload endpoint ends up reporting a rejected file as an outage". Without
/// this impl every upload handler writes that split by hand, in a `map_err`,
/// on the path where getting it wrong is invisible -- the endpoint works,
/// and only the status code lies.
///
/// The `Content` half becomes a validation failure on
/// [`UPLOAD_FIELD`](crate::UPLOAD_FIELD), which is the same shape and the
/// same RFC 9457 document the extractor already produces when it refuses a
/// file before the handler runs. A caller cannot tell whether the mismatch
/// was caught on the way in or on the way to disk, which is correct: it is
/// the same fact about the same file.
///
/// Nothing from the request is reflected. `SniffError` names the extension
/// the file was accepted under -- one of a whitelist the application chose
/// -- and the media type the bytes were recognized as, which is a
/// `&'static str`.
#[cfg(feature = "uploads")]
impl From<crate::storage::UploadError> for Error {
    fn from(e: crate::storage::UploadError) -> Self {
        use crate::storage::UploadError;

        match e {
            UploadError::Storage { source } => Error::Storage(source.to_string()),
            UploadError::Content { source } => Error::Validation(vec![ValidationError {
                field: crate::UPLOAD_FIELD.to_string(),
                message: source.to_string(),
            }]),
        }
    }
}

#[cfg(any(
    feature = "inertia",
    feature = "api",
    feature = "events",
    feature = "jobs"
))]
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Serialization(e.to_string())
    }
}

#[cfg(feature = "storage")]
impl From<bytes::BufMut> for Error {
    fn from(_: bytes::BufMut) -> Self {
        unreachable!("BufMut is an enum trait object; only used for documentation")
    }
}

/// The framework `Result` alias. Every public Arcature function that can fail
/// returns `Result<T>` (the `T` varies), so a controller writes `Result<Response>`.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(all(test, feature = "uploads"))]
mod upload_conversion_tests {
    use super::{Error, ValidationError};
    use crate::storage::error::{StorageError, StoragePathError};
    use crate::storage::filename::Extension;
    use crate::storage::{SniffError, UploadError};

    /// The whole point of the conversion. A file whose bytes disagree with
    /// its extension is a bad request, and the status code is the only part
    /// of that a caller can act on.
    #[test]
    fn rejected_content_answers_422_on_the_upload_field() {
        let declared = Extension::parse("jpg").expect("jpg parses");
        let error = Error::from(UploadError::Content {
            source: SniffError::Mismatch {
                declared,
                sniffed: "text/x-php",
            },
        });

        assert_eq!(error.status(), 422);
        assert_eq!(error.code(), "validation_failed");

        let Error::Validation(failures) = &error else {
            panic!("content failure must be a validation failure, got {error:?}");
        };
        let [ValidationError { field, message }] = failures.as_slice() else {
            panic!("expected exactly one failure, got {failures:?}");
        };
        assert_eq!(field, crate::UPLOAD_FIELD);
        assert!(
            message.contains("text/x-php") && message.contains("jpg"),
            "the message must name both what was claimed and what was found: {message}"
        );
    }

    /// The other half, and the reason the two are not collapsed: a backend
    /// that failed is an outage, and reporting it as a 4xx tells the operator
    /// nothing is wrong.
    #[test]
    fn a_failed_write_answers_500() {
        let error = Error::from(UploadError::Storage {
            source: StorageError::Path {
                source: StoragePathError::Traversal,
            },
        });

        assert_eq!(error.status(), 500);
        assert_eq!(error.code(), "storage_error");
        assert!(
            matches!(error, Error::Storage(_)),
            "storage failure must stay a storage error, got {error:?}"
        );
    }
}
