//! Maps Axum extractor rejections to [`Problem`](crate::Problem) details.
//!
//! Axum's `Json`/`Query`/`Form`/`Path` extractors reject malformed input with
//! typed rejection enums (`JsonRejection`, `QueryRejection`, `FormRejection`,
//! `PathRejection`). This module maps each to a client-safe
//! [`Problem`](crate::Problem) so the API returns RFC 9457 documents instead
//! of axum's plain-text rejection bodies.
//!
//! The mapping never reflects hostile input back to the client verbatim: the
//! `detail` is a fixed per-category string, not the raw deserializer error
//! (which may echo parts of the request). The raw error is reachable only in
//! code that already owns the rejection (server-side), never in the response
//! body.

use axum::extract::rejection::{FormRejection, JsonRejection, PathRejection, QueryRejection};

use crate::api::{Problem, ProblemKind};

/// Map a JSON-body rejection to a [`Problem`].
///
/// Distinguishes missing content type, malformed JSON syntax, and a
/// semantically-invalid (but syntactically valid) JSON body -- each a
/// different [`ProblemKind`] with a client-safe, fixed `detail`.
#[must_use]
pub fn from_json_rejection(rejection: &JsonRejection) -> Problem {
    match rejection {
        JsonRejection::MissingJsonContentType(_) => Problem::of(ProblemKind::UnsupportedMediaType)
            .with_detail("Request body must be application/json"),
        JsonRejection::JsonSyntaxError(_) => {
            Problem::of(ProblemKind::MalformedJson).with_detail("Request body is not valid JSON")
        }
        JsonRejection::JsonDataError(_) => Problem::of(ProblemKind::Validation)
            .with_detail("Request body JSON does not match the expected schema"),
        JsonRejection::BytesRejection(_) => {
            Problem::of(ProblemKind::PayloadTooLarge).with_detail("Request body could not be read")
        }
        // `JsonRejection` is `#[non_exhaustive]`; a future axum variant maps
        // to a generic bad request.
        _ => Problem::of(ProblemKind::BadRequest).with_detail("Request body could not be parsed"),
    }
}

/// Map a query-string rejection to a [`Problem`].
#[must_use]
pub fn from_query_rejection(rejection: &QueryRejection) -> Problem {
    match rejection {
        QueryRejection::FailedToDeserializeQueryString(_) => {
            Problem::of(ProblemKind::BadRequest).with_detail("Query string is malformed")
        }
        _ => Problem::of(ProblemKind::BadRequest).with_detail("Query string is malformed"),
    }
}

/// Map a path-parameter rejection to a [`Problem`].
#[must_use]
pub fn from_path_rejection(rejection: &PathRejection) -> Problem {
    match rejection {
        PathRejection::FailedToDeserializePathParams(_) => {
            Problem::of(ProblemKind::BadRequest).with_detail("Path parameters are malformed")
        }
        PathRejection::MissingPathParams(_) => Problem::of(ProblemKind::Internal)
            .with_detail("Route could not be matched to path parameters"),
        _ => Problem::of(ProblemKind::BadRequest).with_detail("Path parameters are malformed"),
    }
}

/// Map a form rejection to a [`Problem`].
#[must_use]
pub fn from_form_rejection(rejection: &FormRejection) -> Problem {
    match rejection {
        FormRejection::InvalidFormContentType(_) => Problem::of(ProblemKind::UnsupportedMediaType)
            .with_detail("Form request must be application/x-www-form-urlencoded"),
        FormRejection::FailedToDeserializeForm(_) => {
            Problem::of(ProblemKind::BadRequest).with_detail("Form body is malformed")
        }
        FormRejection::FailedToDeserializeFormBody(_) => Problem::of(ProblemKind::Validation)
            .with_detail("Form body does not match the expected schema"),
        FormRejection::BytesRejection(_) => {
            Problem::of(ProblemKind::PayloadTooLarge).with_detail("Form body could not be read")
        }
        _ => Problem::of(ProblemKind::BadRequest).with_detail("Form body is malformed"),
    }
}
