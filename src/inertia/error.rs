//! Typed errors for the Inertia adapter.

use std::fmt;
use std::sync::Arc;

/// A failure in the Inertia adapter.
#[derive(Debug)]
pub enum InertiaError {
    /// Serializing the page object to JSON failed.
    Serialize(Arc<serde_json::Error>),
    /// A redirect or location URL could not be encoded as an HTTP header value.
    Location(axum::http::Error),
    /// A prop resolver returned an error.
    PropResolution {
        path: Arc<str>,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The `Inertia` extractor was used without `InertiaLayer`.
    ConfigMissing,
    /// Typed page props did not serialize to a JSON object.
    PropsMustBeObject,
    /// An HTTP header value could not be constructed.
    Header(axum::http::Error),
}

impl fmt::Display for InertiaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InertiaError::Serialize(e) => write!(f, "inertia page serialization failed: {e}"),
            InertiaError::Location(e) => {
                write!(
                    f,
                    "inertia redirect location is not a valid header value: {e}"
                )
            }
            InertiaError::PropResolution { path, source } => {
                write!(f, "inertia prop `{path}` failed to resolve: {source}")
            }
            InertiaError::ConfigMissing => {
                write!(f, "inertia extractor used without InertiaLayer installed")
            }
            InertiaError::PropsMustBeObject => {
                f.write_str("inertia page props must be a JSON object")
            }
            InertiaError::Header(e) => {
                write!(f, "inertia header value could not be constructed: {e}")
            }
        }
    }
}

impl std::error::Error for InertiaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InertiaError::Serialize(e) => Some(e),
            InertiaError::Location(e) => Some(e),
            InertiaError::PropResolution { source, .. } => Some(source.as_ref()),
            InertiaError::ConfigMissing | InertiaError::PropsMustBeObject => None,
            InertiaError::Header(e) => Some(e),
        }
    }
}

impl From<serde_json::Error> for InertiaError {
    fn from(e: serde_json::Error) -> Self {
        InertiaError::Serialize(Arc::new(e))
    }
}

impl From<axum::http::Error> for InertiaError {
    fn from(e: axum::http::Error) -> Self {
        InertiaError::Header(e)
    }
}

impl From<InertiaError> for crate::Error {
    fn from(e: InertiaError) -> Self {
        crate::Error::Other(e.to_string())
    }
}

impl axum::response::IntoResponse for InertiaError {
    fn into_response(self) -> axum::response::Response {
        // A 500 is the honest status. The typed error is not leaked to the
        // body; it is available to the server log via Display/Debug.
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "inertia adapter error",
        )
            .into_response()
    }
}
