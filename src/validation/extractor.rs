//! Axum extractors that combine extraction + validation and map rejections to
//! [`Problem`](crate::Problem) responses.
//!
//! These mirror Axum's `Json`/`Form`/`Query`/`Path` extractors but validate the
//! extracted value with [`validator::Validate`] and return RFC 9457
//! [`Problem`](crate::Problem) documents for both extraction failures (mapped
//! via [`crate::validation::rejection`]) and validation failures. Using them
//! validates the payload exactly once (the extractor owns the validation; the
//! handler does not re-validate).
//!
//! An application is free to use raw `axum::Json` / `Query` / `Path` instead
//! and call [`crate::validate_or_problem`] itself, or to ignore validation
//! entirely -- these extractors are opt-in ergonomics, not a requirement.

use axum::extract::{Form, FromRequest, FromRequestParts, Json, Path, Query};
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

use crate::api::Problem;
use crate::validation::errors::validation_problem;
use crate::validation::rejection::{
    from_form_rejection, from_json_rejection, from_path_rejection, from_query_rejection,
};

/// A validated JSON body extractor.
///
/// Wraps [`axum::Json<T>`]: extracts and deserializes the JSON body, then
/// validates `T` with [`validator::Validate`]. Rejections and validation
/// failures are returned as RFC 9457 [`Problem`] responses
/// (`application/problem+json`).
///
/// `T` must implement [`serde::de::DeserializeOwned`] (for the JSON body) and
/// [`validator::Validate`] (for the rules). The payload is validated exactly
/// once.
pub struct ValidatedJson<T>(pub T);

/// A validated form body extractor. See [`ValidatedJson`] for semantics.
pub struct ValidatedForm<T>(pub T);

/// A validated query-string extractor. See [`ValidatedJson`] for semantics.
pub struct ValidatedQuery<T>(pub T);

/// A validated path-parameter extractor. See [`ValidatedJson`] for semantics.
pub struct ValidatedPath<T>(pub T);

impl<T> ValidatedJson<T> {
    /// Consume the wrapper and return the validated value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> ValidatedForm<T> {
    /// Consume the wrapper and return the validated value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> ValidatedQuery<T> {
    /// Consume the wrapper and return the validated value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> ValidatedPath<T> {
    /// Consume the wrapper and return the validated value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T, S> FromRequest<S> for ValidatedJson<T>
where
    T: DeserializeOwned + validator::Validate,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state)
            .await
            .map_err(|rejection| from_json_rejection(&rejection).into_response())?;
        match validator::Validate::validate(&value) {
            Ok(()) => Ok(ValidatedJson(value)),
            Err(errors) => Err(validation_problem(errors).into_response()),
        }
    }
}

impl<T, S> FromRequest<S> for ValidatedForm<T>
where
    T: DeserializeOwned + validator::Validate,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        let Form(value) = Form::<T>::from_request(req, state)
            .await
            .map_err(|rejection| from_form_rejection(&rejection).into_response())?;
        match validator::Validate::validate(&value) {
            Ok(()) => Ok(ValidatedForm(value)),
            Err(errors) => Err(validation_problem(errors).into_response()),
        }
    }
}

impl<T, S> FromRequestParts<S> for ValidatedQuery<T>
where
    T: DeserializeOwned + validator::Validate,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let Query(value) = Query::<T>::from_request_parts(parts, state)
            .await
            .map_err(|rejection| from_query_rejection(&rejection).into_response())?;
        match validator::Validate::validate(&value) {
            Ok(()) => Ok(ValidatedQuery(value)),
            Err(errors) => Err(validation_problem(errors).into_response()),
        }
    }
}

impl<T, S> FromRequestParts<S> for ValidatedPath<T>
where
    T: DeserializeOwned + Send + Sync + validator::Validate,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let Path(value) = Path::<T>::from_request_parts(parts, state)
            .await
            .map_err(|rejection| from_path_rejection(&rejection).into_response())?;
        match validator::Validate::validate(&value) {
            Ok(()) => Ok(ValidatedPath(value)),
            Err(errors) => Err(validation_problem(errors).into_response()),
        }
    }
}
