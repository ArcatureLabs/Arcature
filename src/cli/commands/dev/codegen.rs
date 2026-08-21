//! Regenerating the TypeScript after a restart, over the connection the
//! supervisor already has.
//!
//! # Why the endpoint and not a binary
//!
//! `arc typegen` can read the application graph by building the project's
//! `uag` binary, and in CI that is exactly what it does. Doing it here would
//! add a link step to every save, which is the one cost the whole one-port
//! topology exists to avoid. The application that just restarted already
//! holds the graph and already serves it at
//! [`uag_endpoint::PATH`](crate::application::uag_endpoint::PATH), so the
//! supervisor asks it -- over the IPC endpoint it just waited for, with no
//! port, no process and no build involved.
//!
//! # Why a failure here is a warning
//!
//! The backend is up and answering by the time this runs. Refusing to serve
//! because the generated TypeScript could not be refreshed would turn a stale
//! `routes.ts` into a dead development server, so every failure below is
//! printed and stepped over. The one worth reading is a validation
//! diagnostic: that is the application's own graph disagreeing with itself,
//! and it is reported in full rather than summarised.

use std::path::{Path, PathBuf};

use crate::axum::body::Body;
use crate::dev_proxy::endpoint::IpcEndpoint;
use crate::uag::UagArtifact;

/// How much of the graph response to read.
///
/// The artifact is pretty-printed JSON describing every route, page and
/// binding in the application, so it grows with the project; 64 MiB is far
/// past anything real and still a bound rather than an open invitation to
/// buffer whatever the socket produces.
const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

/// Fetch the graph from the running application and rewrite the TypeScript.
///
/// Returns the files that changed. An unchanged file is not rewritten, which
/// is what keeps this from triggering a Vite reload of its own on every
/// restart that did not touch a route.
///
/// # Errors
///
/// [`CodegenError`] for every step: the endpoint not answering, the response
/// not being the artifact, the graph not validating, or a file not being
/// writable.
pub(crate) async fn regenerate(root: &Path, app: &Path) -> Result<Vec<PathBuf>, CodegenError> {
    let artifact = fetch(app).await?;
    let root = root.to_path_buf();
    // `emit` is synchronous filesystem work; the supervisor's runtime has a
    // listener on it and should not be blocked writing three files.
    tokio::task::spawn_blocking(move || super::super::typegen::emit(&artifact, &root))
        .await
        .map_err(|error| CodegenError::Panicked {
            detail: error.to_string(),
        })?
        .map_err(|source| CodegenError::Emit {
            source: Box::new(source),
        })
}

/// Ask the application for its graph over `app`.
async fn fetch(app: &Path) -> Result<UagArtifact, CodegenError> {
    let stream = IpcEndpoint::new(app.to_path_buf())
        .connect()
        .await
        .map_err(|source| CodegenError::Connect { source })?;

    let request = crate::axum::extract::Request::builder()
        .method(crate::axum::http::Method::GET)
        .uri(crate::application::uag_endpoint::PATH)
        // hyper's HTTP/1.1 client requires a `Host`, and there is no name for
        // an IPC endpoint. The application does not route on it.
        .header(crate::axum::http::header::HOST, "arcature.dev")
        .body(Body::empty())
        .map_err(|error| CodegenError::Connect {
            source: std::io::Error::other(error.to_string()),
        })?;

    let response = crate::dev_proxy::service::forward(stream, request)
        .await
        .map_err(|error| CodegenError::Connect {
            source: std::io::Error::other(error.to_string()),
        })?;

    let status = response.status();
    if !status.is_success() {
        return Err(CodegenError::Refused { status });
    }

    let body = crate::axum::body::to_bytes(response.into_body(), MAX_ARTIFACT_BYTES)
        .await
        .map_err(|error| CodegenError::Connect {
            source: std::io::Error::other(error.to_string()),
        })?;

    serde_json::from_slice(&body).map_err(|source| CodegenError::Json { source })
}

/// Why the TypeScript could not be refreshed after a restart.
#[derive(Debug)]
pub(crate) enum CodegenError {
    /// The application did not answer on its endpoint.
    Connect {
        /// The underlying failure.
        source: std::io::Error,
    },
    /// The application answered, but not with the graph.
    Refused {
        /// What it answered with.
        status: crate::axum::http::StatusCode,
    },
    /// The response was not a graph this build understands.
    Json {
        /// Where the parse gave up.
        source: serde_json::Error,
    },
    /// Validation refused the graph, or a file could not be written.
    Emit {
        /// The failure `arc typegen` would have printed.
        source: Box<super::super::typegen::TypegenError>,
    },
    /// The thread doing the writing did not come back.
    Panicked {
        /// What the runtime said about it.
        detail: String,
    },
}

impl std::fmt::Display for CodegenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect { source } => write!(
                formatter,
                "could not read {} from the application: {source}",
                crate::application::uag_endpoint::PATH
            ),
            Self::Refused { status } => write!(
                formatter,
                "the application answered {status} for {}. The endpoint only \
                 exists in a build with the `dev` feature that called \
                 `.uag_endpoint(..)`; see `bootstrap/app.rs`",
                crate::application::uag_endpoint::PATH
            ),
            Self::Json { source } => write!(
                formatter,
                "the application graph could not be read: {source}. This \
                 usually means the application was built against a different \
                 version of arcature than `arc`"
            ),
            Self::Emit { source } => write!(formatter, "{source}"),
            Self::Panicked { detail } => {
                write!(formatter, "the codegen thread did not finish: {detail}")
            }
        }
    }
}

impl std::error::Error for CodegenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect { source } => Some(source),
            Self::Json { source } => Some(source),
            Self::Emit { source } => Some(source.as_ref()),
            Self::Refused { .. } | Self::Panicked { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CodegenError;

    #[test]
    fn a_refusal_names_the_endpoint_and_says_how_to_turn_it_on() {
        let error = CodegenError::Refused {
            status: crate::axum::http::StatusCode::NOT_FOUND,
        };
        let message = error.to_string();
        assert!(
            message.contains("/_arcature/uag.json"),
            "the message should name the endpoint: {message}"
        );
        assert!(
            message.contains("bootstrap/app.rs"),
            "the message should say where the opt-in lives: {message}"
        );
    }

    #[test]
    fn a_parse_failure_points_at_a_version_mismatch_rather_than_at_the_json() {
        let source = serde_json::from_slice::<crate::uag::UagArtifact>(b"{}")
            .expect_err("an empty object is not an artifact");
        let error = CodegenError::Json { source };
        assert!(error.to_string().contains("version of arcature"), "{error}");
    }
}
