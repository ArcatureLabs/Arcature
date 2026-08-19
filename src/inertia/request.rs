//! The validated Inertia request context.

use std::sync::Arc;

use axum::http::{HeaderMap, Method, Uri};

use super::headers::{Headers, Values};

/// The Inertia-specific request semantics, parsed once from the request.
#[derive(Debug, Clone)]
pub struct InertiaRequest {
    is_inertia: bool,
    method: Method,
    url: Arc<str>,
    referer: Option<Arc<str>>,
    request_version: Option<Arc<str>>,
    partial_component: Option<Arc<str>>,
    only: Vec<Arc<str>>,
    except: Vec<Arc<str>>,
    reset: Vec<Arc<str>>,
    error_bag: Option<Arc<str>>,
    is_prefetch: bool,
}

/// A resolved partial-reload selection for a specific page component.
#[derive(Debug, Clone)]
pub struct PartialSelection {
    pub only: Vec<Arc<str>>,
    pub except: Vec<Arc<str>>,
    pub reset: Vec<Arc<str>>,
}

/// The maximum number of keys accepted in one comma-separated header (DoS bound).
const MAX_SELECTOR_KEYS: usize = 64;
const MAX_SELECTOR_BYTES: usize = 8 * 1024;

fn parse_comma_list(value: Option<&str>) -> Vec<Arc<str>> {
    let Some(value) = value else {
        return Vec::new();
    };
    let bounded: &str = if value.len() > MAX_SELECTOR_BYTES {
        &value[..value.floor_char_boundary(MAX_SELECTOR_BYTES)]
    } else {
        value
    };
    let mut out: Vec<Arc<str>> = Vec::new();
    for part in bounded.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        if out.iter().any(|existing: &Arc<str>| &**existing == trimmed) {
            continue;
        }
        if out.len() >= MAX_SELECTOR_KEYS {
            break;
        }
        out.push(Arc::from(trimmed));
    }
    out
}

impl InertiaRequest {
    /// Parse the Inertia request context from headers, method, and URI.
    pub fn parse(headers: &HeaderMap, method: &Method, uri: &Uri) -> InertiaRequest {
        let is_inertia = headers
            .get(Headers::INERTIA)
            .and_then(|v| v.to_str().ok())
            .map(|v| v == Values::INERTIA_TRUE)
            .unwrap_or(false);

        let request_version = headers
            .get(Headers::VERSION)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
            .map(Arc::from);

        let partial_component = headers
            .get(Headers::PARTIAL_COMPONENT)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
            .map(Arc::from);

        let only = parse_comma_list(headers.get(Headers::PARTIAL_DATA).and_then(|v| v.to_str().ok()));
        let except = parse_comma_list(headers.get(Headers::PARTIAL_EXCEPT).and_then(|v| v.to_str().ok()));
        let reset = parse_comma_list(headers.get(Headers::RESET).and_then(|v| v.to_str().ok()));

        let error_bag = headers
            .get(Headers::ERROR_BAG)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
            .map(Arc::from);

        let referer = headers
            .get(axum::http::header::REFERER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(Arc::from);

        let is_prefetch = headers
            .get(Headers::PURPOSE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v == Values::PURPOSE_PREFETCH)
            .unwrap_or(false);

        let url = Arc::from(uri.to_string());

        InertiaRequest {
            is_inertia,
            method: method.clone(),
            url,
            referer,
            request_version,
            partial_component,
            only,
            except,
            reset,
            error_bag,
            is_prefetch,
        }
    }

    /// Whether this is an Inertia request (`X-Inertia: true`).
    pub fn is_inertia(&self) -> bool {
        self.is_inertia
    }

    /// The request method.
    pub fn method(&self) -> &Method {
        &self.method
    }

    /// The request URL (path + query).
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The referring URL used by the official empty-response fallback.
    pub fn referer(&self) -> Option<&str> {
        self.referer.as_deref()
    }

    /// The asset version the client holds, if any.
    pub fn request_version(&self) -> Option<&str> {
        self.request_version.as_deref()
    }

    /// The requested error-bag name, if any.
    pub fn error_bag(&self) -> Option<&str> {
        self.error_bag.as_deref()
    }

    /// Whether this is a prefetch visit (`Purpose: prefetch`).
    pub fn is_prefetch(&self) -> bool {
        self.is_prefetch
    }

    /// Resolve the partial-reload selection for `component`.
    pub fn partial_for(&self, component: &str) -> Option<PartialSelection> {
        let matches = self
            .partial_component
            .as_deref()
            .map(|c| c == component)
            .unwrap_or(false);
        if !matches {
            return None;
        }
        Some(PartialSelection {
            only: self.only.clone(),
            except: self.except.clone(),
            reset: self.reset.clone(),
        })
    }
}
