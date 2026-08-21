//! Distinguished RFC 9457 problem categories.
//!
//! Each [`ProblemKind`] carries a fixed HTTP status, a short human title, and
//! a stable `type` URI that identifies the problem category. An application
//! remains free to construct its own [`crate::Problem`] with a custom `type`
//! for categories outside this list.

use axum::http::StatusCode;

/// A distinguished RFC 9457 problem category.
///
/// Each variant resolves to a fixed status, title, and `type` URI via
/// [`ProblemKind::status`], [`ProblemKind::title`], and
/// [`ProblemKind::type_uri`]. The `type` URI is a stable identifier (a
/// `urn:arcature:problem:` URN), not a promise of a fetchable document; RFC
/// 9457 permits a `type` that does not resolve to human-readable
/// documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemKind {
    /// The request body or a parameter failed validation (422).
    Validation,
    /// Authentication is required or failed (401).
    Authentication,
    /// The authenticated principal is not allowed (403).
    Authorization,
    /// The requested resource does not exist (404).
    NotFound,
    /// The request conflicts with the current state (409).
    Conflict,
    /// The client sent too many requests (429).
    RateLimit,
    /// An internal server error occurred (500).
    Internal,
    /// The request body was not valid JSON (400).
    MalformedJson,
    /// The request content type is not supported (415).
    UnsupportedMediaType,
    /// The request body exceeded the size limit (413).
    PayloadTooLarge,
    /// A generic client error (400).
    BadRequest,
    /// The request took longer than the server was willing to wait (408).
    Timeout,
    /// The path exists but not for this method (405).
    MethodNotAllowed,
    /// The server cannot handle the request right now -- maintenance,
    /// shedding load, or a dependency that is down (503).
    Unavailable,
}

impl ProblemKind {
    /// The HTTP status code for this category.
    #[must_use]
    pub const fn status(self) -> StatusCode {
        match self {
            Self::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Authentication => StatusCode::UNAUTHORIZED,
            Self::Authorization => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict => StatusCode::CONFLICT,
            Self::RateLimit => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            Self::MalformedJson => StatusCode::BAD_REQUEST,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::BadRequest => StatusCode::BAD_REQUEST,
            Self::Timeout => StatusCode::REQUEST_TIMEOUT,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// The short human-readable `title` for this category (RFC 9457 `title`).
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Validation => "Validation failed",
            Self::Authentication => "Authentication required",
            Self::Authorization => "Access denied",
            Self::NotFound => "Resource not found",
            Self::Conflict => "Request conflicts with current state",
            Self::RateLimit => "Rate limit exceeded",
            Self::Internal => "Internal server error",
            Self::MalformedJson => "Malformed JSON request body",
            Self::UnsupportedMediaType => "Unsupported media type",
            Self::PayloadTooLarge => "Request body too large",
            Self::BadRequest => "Bad request",
            Self::Timeout => "Request timed out",
            Self::MethodNotAllowed => "Method not allowed",
            Self::Unavailable => "Service unavailable",
        }
    }

    /// The stable `type` URI identifying this category (RFC 9457 `type`).
    ///
    /// These are `urn:arcature:problem:` URNs used as stable, opaque
    /// identifiers -- not fetchable URLs. RFC 9457 permits a `type` that does
    /// not dereference to human-readable documentation; clients treat unknown
    /// `type` values as `"about:blank"`-equivalent. Using a URN (not an
    /// `https://` URL that implies a docs site exists) keeps Arcature honest
    /// about what the identifier promises.
    #[must_use]
    pub const fn type_uri(self) -> &'static str {
        match self {
            Self::Validation => "urn:arcature:problem:validation",
            Self::Authentication => "urn:arcature:problem:authentication",
            Self::Authorization => "urn:arcature:problem:authorization",
            Self::NotFound => "urn:arcature:problem:not-found",
            Self::Conflict => "urn:arcature:problem:conflict",
            Self::RateLimit => "urn:arcature:problem:rate-limit",
            Self::Internal => "urn:arcature:problem:internal",
            Self::MalformedJson => "urn:arcature:problem:malformed-json",
            Self::UnsupportedMediaType => "urn:arcature:problem:unsupported-media-type",
            Self::PayloadTooLarge => "urn:arcature:problem:payload-too-large",
            Self::BadRequest => "urn:arcature:problem:bad-request",
            Self::Timeout => "urn:arcature:problem:timeout",
            Self::MethodNotAllowed => "urn:arcature:problem:method-not-allowed",
            Self::Unavailable => "urn:arcature:problem:unavailable",
        }
    }

    /// Every variant, for exhaustive iteration.
    ///
    /// A `const` slice rather than a derive: it is what the tests below walk
    /// to prove the three tables stay in step, and adding a variant without
    /// adding it here makes that test fail rather than silently under-check.
    pub const ALL: &'static [ProblemKind] = &[
        Self::Validation,
        Self::Authentication,
        Self::Authorization,
        Self::NotFound,
        Self::Conflict,
        Self::RateLimit,
        Self::Internal,
        Self::MalformedJson,
        Self::UnsupportedMediaType,
        Self::PayloadTooLarge,
        Self::BadRequest,
        Self::Timeout,
        Self::MethodNotAllowed,
        Self::Unavailable,
    ];

    /// The kind a bare `status` corresponds to, if any.
    ///
    /// Used by [`ErrorMapping`](crate::http::ErrorMapping) to give a status a
    /// body when whatever produced it did not. The mapping is deliberately
    /// partial: a status with no distinguished kind falls back to a generic
    /// client or server problem rather than being forced into a category it
    /// does not belong to.
    ///
    /// Ambiguous statuses resolve to the kind a *layer* would have produced:
    /// `400` is [`BadRequest`](Self::BadRequest), not `MalformedJson`, because
    /// a bare `400` from the pipeline is not evidence about JSON.
    #[must_use]
    pub const fn for_status(status: StatusCode) -> Option<Self> {
        match status {
            StatusCode::BAD_REQUEST => Some(Self::BadRequest),
            StatusCode::UNAUTHORIZED => Some(Self::Authentication),
            StatusCode::FORBIDDEN => Some(Self::Authorization),
            StatusCode::NOT_FOUND => Some(Self::NotFound),
            StatusCode::METHOD_NOT_ALLOWED => Some(Self::MethodNotAllowed),
            StatusCode::REQUEST_TIMEOUT => Some(Self::Timeout),
            StatusCode::CONFLICT => Some(Self::Conflict),
            StatusCode::PAYLOAD_TOO_LARGE => Some(Self::PayloadTooLarge),
            StatusCode::UNSUPPORTED_MEDIA_TYPE => Some(Self::UnsupportedMediaType),
            StatusCode::UNPROCESSABLE_ENTITY => Some(Self::Validation),
            StatusCode::TOO_MANY_REQUESTS => Some(Self::RateLimit),
            StatusCode::INTERNAL_SERVER_ERROR => Some(Self::Internal),
            StatusCode::SERVICE_UNAVAILABLE => Some(Self::Unavailable),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProblemKind;
    use axum::http::StatusCode;

    #[test]
    fn each_kind_has_distinct_type_uri() {
        let mut seen = std::collections::HashSet::new();
        for &kind in ProblemKind::ALL {
            assert!(seen.insert(kind.type_uri()), "duplicate type_uri");
            assert!(!kind.title().is_empty());
            assert!(kind.status().is_client_error() || kind.status().is_server_error());
        }
    }

    #[test]
    fn for_status_round_trips_every_kind_that_owns_its_status() {
        // `MalformedJson` and `BadRequest` share `400`; the shared status
        // resolves to the generic kind, which is the only honest answer when
        // all the mapper has is a status code.
        for &kind in ProblemKind::ALL {
            if kind == ProblemKind::MalformedJson {
                continue;
            }
            assert_eq!(
                ProblemKind::for_status(kind.status()),
                Some(kind),
                "{kind:?} does not round-trip through its own status"
            );
        }
    }

    #[test]
    fn a_status_with_no_distinguished_kind_maps_to_nothing() {
        assert_eq!(ProblemKind::for_status(StatusCode::IM_A_TEAPOT), None);
        assert_eq!(ProblemKind::for_status(StatusCode::BAD_GATEWAY), None);
    }

    #[test]
    fn status_matches_category() {
        assert_eq!(ProblemKind::Validation.status().as_u16(), 422);
        assert_eq!(ProblemKind::Authentication.status().as_u16(), 401);
        assert_eq!(ProblemKind::Authorization.status().as_u16(), 403);
        assert_eq!(ProblemKind::NotFound.status().as_u16(), 404);
        assert_eq!(ProblemKind::Conflict.status().as_u16(), 409);
        assert_eq!(ProblemKind::RateLimit.status().as_u16(), 429);
        assert_eq!(ProblemKind::Internal.status().as_u16(), 500);
    }
}
