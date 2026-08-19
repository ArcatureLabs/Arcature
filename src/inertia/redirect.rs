//! Redirect and location responses (Inertia v3 redirect behavior).

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use super::error::InertiaError;
use super::headers::Headers;

/// A redirect type the adapter knows how to emit.
#[derive(Debug, Clone)]
pub enum Redirect {
    /// A standard redirect: 302 for GET-origin, 303 for PUT/PATCH/DELETE.
    Standard {
        location: String,
        method: axum::http::Method,
    },
    /// An external redirect: 409 + `X-Inertia-Location`.
    External { location: String },
    /// A fragment redirect: 409 + `X-Inertia-Redirect`.
    Fragment { location: String },
}

impl Redirect {
    /// Build a standard redirect, selecting 302 vs 303 from the method.
    pub fn to(location: impl Into<String>, method: axum::http::Method) -> Self {
        Redirect::Standard {
            location: location.into(),
            method,
        }
    }

    /// Convert this redirect into an Axum [`Response`].
    pub fn build(self) -> Result<Response, InertiaError> {
        Ok(match self {
            Redirect::Standard { location, method } => {
                let status = if matches!(
                    method,
                    axum::http::Method::PUT | axum::http::Method::PATCH | axum::http::Method::DELETE
                ) {
                    StatusCode::SEE_OTHER
                } else {
                    StatusCode::FOUND
                };
                standard_response(&location, status)?
            }
            Redirect::External { location } => control_response(&location, Headers::LOCATION)?,
            Redirect::Fragment { location } => control_response(&location, Headers::REDIRECT)?,
        })
    }
}

impl IntoResponse for Redirect {
    fn into_response(self) -> Response {
        match self.build() {
            Ok(r) => r,
            Err(e) => e.into_response(),
        }
    }
}

fn standard_response(location: &str, status: StatusCode) -> Result<Response, InertiaError> {
    let value = HeaderValue::from_str(location)
        .map_err(axum::http::Error::from)
        .map_err(InertiaError::Location)?;
    let mut headers = HeaderMap::new();
    headers.insert(axum::http::header::LOCATION, value);
    Ok((status, headers, Body::empty()).into_response())
}

fn control_response(location: &str, header: HeaderName) -> Result<Response, InertiaError> {
    let value = HeaderValue::from_str(location)
        .map_err(axum::http::Error::from)
        .map_err(InertiaError::Location)?;
    let mut headers = HeaderMap::new();
    headers.insert(header, value);
    Ok((StatusCode::CONFLICT, headers, Body::empty()).into_response())
}

/// Convenience: build a standard 302/303 redirect (method-selected).
pub fn redirect(location: impl Into<String>, method: axum::http::Method) -> Redirect {
    Redirect::to(location, method)
}

/// Convenience: build an external redirect (409 + `X-Inertia-Location`).
pub fn external(location: impl Into<String>) -> Redirect {
    Redirect::External {
        location: location.into(),
    }
}

/// Convenience: build a fragment redirect (409 + `X-Inertia-Redirect`).
pub fn fragment(location: impl Into<String>) -> Redirect {
    Redirect::Fragment {
        location: location.into(),
    }
}
