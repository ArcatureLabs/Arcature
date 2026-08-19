//! Request validation built on the [`validator`] crate, integrated with
//! Axum and RFC 9457 problem responses.
//!
//! This module owns three responsibilities, each in its own file:
//!
//! * [`errors`] -- walks `validator::ValidationErrors` into a client-safe JSON
//!   tree and builds the validation [`Problem`](crate::Problem).
//! * [`extractor`] -- `ValidatedJson`/`ValidatedForm`/`ValidatedQuery`/
//!   `ValidatedPath` Axum extractors that combine extraction + validation and
//!   map rejections to [`Problem`](crate::Problem).
//! * [`rejection`] -- maps Axum extractor rejections (`JsonRejection`,
//!   `QueryRejection`, `FormRejection`, `PathRejection`) to
//!   [`Problem`](crate::Problem).
//!
//! ## Validation is the trust boundary
//!
//! At the point a handler receives a validated request, the value has passed
//! [`validator::Validate::validate`]. The handler does not re-validate.
//!
//! **Validation must not imply authorization.** A validated request is not an
//! authorized request. Authorization is a separate, explicit step (the `auth`
//! subsystem's `Policy`).

pub mod errors;
pub mod extractor;
pub mod rejection;

pub use errors::{validate_or_problem, validation_problem};
pub use extractor::{ValidatedForm, ValidatedJson, ValidatedPath, ValidatedQuery};
pub use rejection::{
    from_form_rejection, from_json_rejection, from_path_rejection, from_query_rejection,
};

/// A request body that has passed validation.
///
/// `Validated<T>` is the high-level request DX type: it combines JSON body
/// extraction, deserialization, and validation into a single Axum
/// [`axum::extract::FromRequest`] extractor. When a controller takes
/// `input: Validated<StoreLinkRequest>`, the payload is extracted and
/// validated before the handler runs -- the handler may trust that validation
/// succeeded.
///
/// `Validated<T>` delegates to [`ValidatedJson<T>`]: it extracts and
/// deserializes the JSON body, validates `T` with [`validator::Validate`], and
/// maps rejections/validation failures to RFC 9457 [`Problem`] responses
/// (`application/problem+json`).
///
/// `T` must implement [`serde::de::DeserializeOwned`] (for the JSON body) and
/// [`validator::Validate`] (for the rules). The payload is validated exactly
/// once.
///
/// Use [`Validated::into_inner`] to extract the validated value in the handler.
///
/// # Example
///
/// ```ignore
/// use arcature::{Deserialize, Validated, Validate};
///
/// #[request]
/// pub struct StoreLinkRequest {
///     #[rule(required, url)]
///     pub url: String,
///     #[rule(required, length(min = 1, max = 120))]
///     pub title: String,
/// }
///
/// async fn store(input: Validated<StoreLinkRequest>) -> Result<Redirect> {
///     let data = input.into_inner();
///     // ... create link from `data` ...
///     redirect!(route::links::index())
/// }
/// ```
pub struct Validated<T>(pub T);

impl<T> Validated<T> {
    /// Consume the wrapper and return the validated inner value.
    ///
    /// After this call, the handler owns the validated `T` and may trust that
    /// validation succeeded.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T, S> axum::extract::FromRequest<S> for Validated<T>
where
    T: serde::de::DeserializeOwned + validator::Validate,
    S: Send + Sync,
{
    type Rejection = axum::response::Response;

    async fn from_request(
        req: axum::extract::Request,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let ValidatedJson(value) = ValidatedJson::<T>::from_request(req, state).await?;
        Ok(Validated(value))
    }
}

/// A marker trait implemented by types that serve as a validated request.
///
/// The `#[request]` proc-macro (in `arcature-macros`) derives `Deserialize` and
/// `Validate` on the request struct and implements this trait so `arc check`
/// and other tooling can identify request types. A request type must implement
/// [`serde::de::DeserializeOwned`] and [`validator::Validate`].
///
/// Application code rarely names this trait directly; the `#[request]` macro
/// implements it. It is here so request types are first-class in the framework
/// vocabulary even when the macro is not used (manual `impl Request for ...`
/// after deriving `Deserialize` + `Validate`).
pub trait Request: serde::de::DeserializeOwned + validator::Validate {}
