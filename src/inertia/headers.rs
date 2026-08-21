//! The Inertia v3 protocol header constants.
//!
//! Single source of truth for the header names this module recognizes.

use axum::http::HeaderName;

/// Grouping type carrying the protocol `HeaderName` constants.
pub(crate) struct Headers;

impl Headers {
    /// `X-Inertia` — request: `true` marks an Inertia request; response: `true`
    /// marks an Inertia JSON page response.
    pub const INERTIA: HeaderName = HeaderName::from_static("x-inertia");

    /// `X-Inertia-Version` — the client's current asset version (GET only);
    /// echoed on a version-mismatch `409`.
    pub const VERSION: HeaderName = HeaderName::from_static("x-inertia-version");

    /// `X-Inertia-Partial-Component` — the component a partial reload targets.
    pub const PARTIAL_COMPONENT: HeaderName =
        HeaderName::from_static("x-inertia-partial-component");

    /// `X-Inertia-Partial-Data` — comma-separated prop keys to include.
    pub const PARTIAL_DATA: HeaderName = HeaderName::from_static("x-inertia-partial-data");

    /// `X-Inertia-Partial-Except` — comma-separated prop keys to exclude.
    pub const PARTIAL_EXCEPT: HeaderName = HeaderName::from_static("x-inertia-partial-except");

    /// `X-Inertia-Reset` — comma-separated prop paths to reset.
    pub const RESET: HeaderName = HeaderName::from_static("x-inertia-reset");

    /// `X-Inertia-Error-Bag` — names the error bag to scope validation errors.
    pub const ERROR_BAG: HeaderName = HeaderName::from_static("x-inertia-error-bag");

    /// `X-Inertia-Except-Once-Props` — comma-separated *once keys* the client
    /// already holds. The server withholds those values and still emits their
    /// `onceProps` entries; the client fills them back in from its own copy.
    pub const EXCEPT_ONCE_PROPS: HeaderName =
        HeaderName::from_static("x-inertia-except-once-props");

    /// `X-Inertia-Infinite-Scroll-Merge-Intent` — `prepend` when the client is
    /// loading the previous page of an infinite scroll, `append` for the next.
    pub const MERGE_INTENT: HeaderName =
        HeaderName::from_static("x-inertia-infinite-scroll-merge-intent");

    /// `X-Inertia-Location` — on a `409`, the destination for a full
    /// `window.location` visit (external redirect or version mismatch).
    pub const LOCATION: HeaderName = HeaderName::from_static("x-inertia-location");

    /// `X-Inertia-Redirect` — on a `409`, the full redirect URL (including
    /// fragment) for a fragment redirect; the client makes a fresh Inertia GET.
    pub const REDIRECT: HeaderName = HeaderName::from_static("x-inertia-redirect");

    /// `Purpose` — `prefetch` on prefetch visits.
    pub const PURPOSE: HeaderName = HeaderName::from_static("purpose");

    /// `Vary` — the standard HTTP `Vary` header, merged with `X-Inertia`.
    pub const VARY: HeaderName = HeaderName::from_static("vary");
}

/// Well-known protocol header *values*.
pub(crate) struct Values;

impl Values {
    /// The literal `X-Inertia` value sent by the official client.
    pub const INERTIA_TRUE: &str = "true";

    /// `Purpose: prefetch` — sent on prefetch visits.
    pub const PURPOSE_PREFETCH: &str = "prefetch";

    /// Merge intent for the *previous* side of an infinite scroll.
    pub const MERGE_INTENT_PREPEND: &str = "prepend";

    /// Merge intent for the *next* side of an infinite scroll.
    pub const MERGE_INTENT_APPEND: &str = "append";
}
