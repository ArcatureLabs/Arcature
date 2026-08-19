//! Static pages, assets, and the root HTML document.
//!
//! Serves static files from a directory and the root HTML document for Inertia.
//! The [`MaintenanceGuard`] gates the request pipeline during maintenance.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// Static pages: serves files from a directory.
#[derive(Clone, Debug)]
pub struct Pages {
    inner: Arc<PagesInner>,
}

#[derive(Debug)]
struct PagesInner {
    root: PathBuf,
    /// Optional index file served for directory requests (e.g. "index.html").
    index: Option<String>,
}

impl Pages {
    /// Create a pages handler serving from `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(PagesInner {
                root: root.into(),
                index: Some("index.html".to_string()),
            }),
        }
    }

    /// Create a pages handler with no default index file.
    #[must_use]
    pub fn without_index(root: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(PagesInner {
                root: root.into(),
                index: None,
            }),
        }
    }

    /// The root directory.
    #[must_use]
    pub fn root(&self) -> &std::path::Path {
        &self.inner.root
    }

    /// Serve a file at the given relative path. Returns `404` if not found,
    /// `403` if the path escapes the root.
    pub async fn serve(&self, relative: &str) -> Response {
        // Reject absolute paths and traversal.
        let path = std::path::Path::new(relative);
        if path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return StatusCode::FORBIDDEN.into_response();
        }
        let full = self.inner.root.join(path);

        // If it is a directory, try the index file.
        let full = if full.is_dir() {
            if let Some(index) = &self.inner.index {
                full.join(index)
            } else {
                return StatusCode::FORBIDDEN.into_response();
            }
        } else {
            full
        };

        match tokio::fs::read(&full).await {
            Ok(bytes) => {
                let mime = mime_for(&full);
                let mut response = Response::new(Body::from(bytes));
                *response.status_mut() = StatusCode::OK;
                if let Ok(value) = HeaderValue::from_str(mime) {
                    response.headers_mut().insert(axum::http::header::CONTENT_TYPE, value);
                }
                response
            }
            Err(_) => StatusCode::NOT_FOUND.into_response(),
        }
    }
}

/// A simple MIME type guess from the file extension.
fn mime_for(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("txt") => "text/plain; charset=utf-8",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// A maintenance guard. When engaged, the request pipeline returns `503
/// Service Unavailable` for all requests except a health check.
#[derive(Clone, Debug, Default)]
pub struct MaintenanceGuard {
    engaged: Arc<std::sync::atomic::AtomicBool>,
}

impl MaintenanceGuard {
    /// Create a disengaged guard.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Engage the guard (returns `503`).
    pub fn engage(&self) {
        self.engaged.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Disengage the guard.
    pub fn disengage(&self) {
        self.engaged.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether the guard is engaged.
    #[must_use]
    pub fn is_engaged(&self) -> bool {
        self.engaged.load(std::sync::atomic::Ordering::Relaxed)
    }
}
