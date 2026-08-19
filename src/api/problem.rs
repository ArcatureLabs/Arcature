//! RFC 9457 Problem Details for API errors.
//!
//! [`Problem`] is a typed serde struct serialized to
//! `application/problem+json`. It carries the standard members (`type`,
//! `title`, `status`, `detail`, `instance`) plus arbitrary extension members
//! (e.g. `errors` for field-level validation failures). It is **not** the
//! `{ "success": false, "message": "..." }` shape; applications that want
//! their own body shape are free to ignore `Problem` and return any
//! [`axum::response::IntoResponse`].
//!
//! # Security
//!
//! `Problem` never leaks internal detail. The `detail` member must be a short,
//! client-safe message -- never an internal error chain, SQL, a file path, a
//! stack trace, or a credential. `Problem::of`, `Problem::custom`, and the
//! `IntoResponse` impl add no server-only context. Extension members added
//! via the builder methods are the application's responsibility to keep
//! client-safe.
//!
//! This module is always available (no feature gate): it needs only
//! `serde`/`serde_json`/`axum`, which are always-on, and the validation
//! subsystem depends on it for the 422 validation-failure response.

use std::collections::BTreeMap;

use axum::http::StatusCode;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use serde::{Serialize, Serializer};
use serde_json::{Number, Value};

use super::kind::ProblemKind;

/// The RFC 9457 media type for problem details.
///
/// `application/problem+json` per <https://www.rfc-editor.org/rfc/rfc9457>.
pub const PROBLEM_JSON: &str = "application/problem+json";

/// Fallback body if serialization fails. `Problem` is constructed from
/// controlled strings, so this is unreachable in practice, but production
/// code must not panic on a serialization error.
const FALLBACK_BODY: &str =
    r#"{"type":"urn:arcature:problem:internal","title":"Internal server error","status":500}"#;

/// The standard RFC 9457 member names that extensions must not shadow.
const STANDARD_MEMBERS: [&str; 5] = ["type", "title", "status", "detail", "instance"];

/// A [RFC 9457] Problem Details document for API errors.
///
/// Construct a distinguished-category problem with [`Problem::of`], build one
/// with [`Problem::builder`], or construct a fully custom one with
/// [`Problem::custom`] (an explicit `type` URI + status outside the
/// [`ProblemKind`] list). Convert to an HTTP response via the
/// [`axum::response::IntoResponse`] impl.
///
/// [RFC 9457]: https://www.rfc-editor.org/rfc/rfc9457
#[derive(Debug, Clone)]
pub struct Problem {
    /// The problem category, resolving `type`/`title`/`status` when no
    /// custom override is set.
    kind: ProblemKind,
    /// Custom overrides for `type`/`title`/`status` (set together by
    /// [`Problem::custom`], absent for [`Problem::of`]). Boxed to keep the
    /// common [`Problem::of`] case small on the stack.
    custom: Option<Box<CustomParts>>,
    /// Short, client-safe human description (RFC 9457 `detail`). `None`
    /// omits the member.
    detail: Option<String>,
    /// A URI reference identifying the specific occurrence (RFC 9457
    /// `instance`), typically the request path. `None` omits the member.
    instance: Option<String>,
    /// Extension members serialized alongside the standard members. Keys
    /// must not shadow `type`/`title`/`status`/`detail`/`instance`; the
    /// builder enforces this.
    extensions: BTreeMap<String, Value>,
}

/// The custom `type`/`title`/`status` overrides for a [`Problem::custom`]
/// problem. Held in a `Box` inside [`Problem`] so the common [`Problem::of`]
/// path carries only a nullable pointer instead of three full-sized
/// `Option<String>`/`Option<StatusCode>` fields.
#[derive(Debug, Clone)]
struct CustomParts {
    type_uri: String,
    title: String,
    status: StatusCode,
}

impl Problem {
    /// Build a problem of a distinguished [`ProblemKind`].
    ///
    /// The `type`, `title`, and `status` members come from `kind`. `detail`
    /// and `instance` are omitted unless added via the builder methods.
    #[must_use]
    pub fn of(kind: ProblemKind) -> Self {
        Self {
            kind,
            custom: None,
            detail: None,
            instance: None,
            extensions: BTreeMap::new(),
        }
    }

    /// Start a builder for a distinguished [`ProblemKind`].
    #[must_use]
    pub fn builder(kind: ProblemKind) -> ProblemBuilder {
        ProblemBuilder::new(kind)
    }

    /// Build a fully custom problem with an explicit `type` URI and status.
    ///
    /// Use this for an application-specific problem category outside the
    /// [`ProblemKind`] list. The `type` is the RFC 9457 `type` URI; pass
    /// `"about:blank"` when the `status` reason phrase is the only semantics.
    /// The `title` defaults to the `status` reason phrase.
    #[must_use]
    pub fn custom<T>(type_uri: T, status: StatusCode) -> Self
    where
        T: Into<String>,
    {
        Self {
            kind: ProblemKind::Internal,
            custom: Some(Box::new(CustomParts {
                type_uri: type_uri.into(),
                title: reason_phrase(status),
                status,
            })),
            detail: None,
            instance: None,
            extensions: BTreeMap::new(),
        }
    }

    /// Set the short, client-safe `detail` message.
    #[must_use]
    pub fn with_detail<D>(mut self, detail: D) -> Self
    where
        D: Into<String>,
    {
        self.detail = Some(detail.into());
        self
    }

    /// Set the `instance` URI (typically the request path).
    #[must_use]
    pub fn with_instance<I>(mut self, instance: I) -> Self
    where
        I: Into<String>,
    {
        self.instance = Some(instance.into());
        self
    }

    /// Add a single extension member (a key/value serialized as JSON).
    ///
    /// Keys shadowing a standard member (`type`/`title`/`status`/`detail`/
    /// `instance`) are dropped, so the standard members can never be
    /// overwritten by an extension. Values serializing to `null` are dropped.
    #[must_use]
    pub fn with_extension<V>(mut self, key: &str, value: V) -> Self
    where
        V: Serialize,
    {
        if let Some(value) = serialize_extension(key, value) {
            self.extensions.insert(key.to_string(), value);
        }
        self
    }

    /// Add several extension members from a serializable map.
    ///
    /// `entries` is serialized to a JSON object; each top-level key/value pair
    /// becomes an extension member. Keys shadowing standard members and
    /// `null` values are dropped.
    #[must_use]
    pub fn with_extensions<E>(mut self, entries: &E) -> Self
    where
        E: Serialize,
    {
        extend_from(&mut self.extensions, entries);
        self
    }

    /// The HTTP status code for this problem.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.custom
            .as_ref()
            .map(|c| c.status)
            .unwrap_or_else(|| self.kind.status())
    }

    /// The RFC 9457 `type` URI.
    #[must_use]
    pub fn type_uri(&self) -> &str {
        self.custom
            .as_ref()
            .map(|c| c.type_uri.as_str())
            .unwrap_or_else(|| self.kind.type_uri())
    }

    /// The RFC 9457 `title`.
    #[must_use]
    pub fn title(&self) -> &str {
        self.custom
            .as_ref()
            .map(|c| c.title.as_str())
            .unwrap_or_else(|| self.kind.title())
    }

    /// Serialize this problem to a `serde_json::Value`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        serialize_problem(self)
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = serde_json::to_vec(&self).unwrap_or_else(|_| FALLBACK_BODY.as_bytes().to_vec());
        let len = body.len();
        let mut response = (status, body).into_response();
        response.headers_mut().insert(
            CONTENT_TYPE,
            axum::http::HeaderValue::from_static(PROBLEM_JSON),
        );
        response
            .headers_mut()
            .insert(CONTENT_LENGTH, axum::http::HeaderValue::from(len));
        response
    }
}

/// Builder for [`Problem`].
#[derive(Debug, Clone)]
pub struct ProblemBuilder {
    problem: Problem,
}

impl ProblemBuilder {
    /// Start a builder for a distinguished [`ProblemKind`].
    #[must_use]
    pub fn new(kind: ProblemKind) -> Self {
        Self {
            problem: Problem::of(kind),
        }
    }

    /// Set the short, client-safe `detail` message.
    #[must_use]
    pub fn detail<D>(mut self, detail: D) -> Self
    where
        D: Into<String>,
    {
        self.problem.detail = Some(detail.into());
        self
    }

    /// Set the `instance` URI (typically the request path).
    #[must_use]
    pub fn instance<I>(mut self, instance: I) -> Self
    where
        I: Into<String>,
    {
        self.problem.instance = Some(instance.into());
        self
    }

    /// Add an extension member.
    #[must_use]
    pub fn extension<V>(mut self, key: &str, value: V) -> Self
    where
        V: Serialize,
    {
        if let Some(value) = serialize_extension(key, value) {
            self.problem.extensions.insert(key.to_string(), value);
        }
        self
    }

    /// Add several extension members from a serializable map.
    #[must_use]
    pub fn extensions<E>(mut self, entries: &E) -> Self
    where
        E: Serialize,
    {
        extend_from(&mut self.problem.extensions, entries);
        self
    }

    /// Build the [`Problem`].
    #[must_use]
    pub fn build(self) -> Problem {
        self.problem
    }
}

/// True if `name` is a standard RFC 9457 member.
fn is_standard_member(name: &str) -> bool {
    STANDARD_MEMBERS.contains(&name)
}

/// Serialize an extension value, rejecting shadow keys and `null`.
fn serialize_extension<V>(key: &str, value: V) -> Option<Value>
where
    V: Serialize,
{
    if is_standard_member(key) {
        return None;
    }
    match serde_json::to_value(&value) {
        Ok(Value::Null) => None,
        Ok(value) => Some(value),
        Err(_) => None,
    }
}

/// Extend the extension map from a serializable object value.
fn extend_from<E>(extensions: &mut BTreeMap<String, Value>, entries: &E)
where
    E: Serialize,
{
    if let Ok(Value::Object(map)) = serde_json::to_value(entries) {
        for (key, value) in map {
            if is_standard_member(&key) || value.is_null() {
                continue;
            }
            extensions.insert(key, value);
        }
    }
}

/// The canonical reason phrase for a status (RFC 9457 `title` default).
fn reason_phrase(status: StatusCode) -> String {
    status
        .canonical_reason()
        .unwrap_or("Request error")
        .to_string()
}

/// Serialize a [`Problem`] to a JSON object value.
fn serialize_problem(problem: &Problem) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "type".to_string(),
        Value::String(problem.type_uri().to_string()),
    );
    map.insert(
        "title".to_string(),
        Value::String(problem.title().to_string()),
    );
    map.insert(
        "status".to_string(),
        Value::Number(Number::from(problem.status().as_u16())),
    );
    if let Some(detail) = &problem.detail {
        map.insert("detail".to_string(), Value::String(detail.clone()));
    }
    if let Some(instance) = &problem.instance {
        map.insert("instance".to_string(), Value::String(instance.clone()));
    }
    for (key, value) in &problem.extensions {
        map.insert(key.clone(), value.clone());
    }
    Value::Object(map)
}

impl Serialize for Problem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_problem(self).serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_problem_serializes_all_members() {
        let problem = Problem::of(ProblemKind::NotFound).with_detail("user 42 missing");
        let value = problem.to_json();
        assert_eq!(value["type"], "urn:arcature:problem:not-found");
        assert_eq!(value["title"], "Resource not found");
        assert_eq!(value["status"], 404);
        assert_eq!(value["detail"], "user 42 missing");
        assert!(value.get("instance").is_none());
    }

    #[test]
    fn omitted_members_are_absent() {
        let value = Problem::of(ProblemKind::Conflict).to_json();
        assert!(value.get("detail").is_none());
        assert!(value.get("instance").is_none());
    }

    #[test]
    fn extensions_are_serialized() {
        let problem = Problem::of(ProblemKind::Validation)
            .with_extension("errors", serde_json::json!({"name": ["required"]}));
        let value = problem.to_json();
        assert_eq!(value["errors"]["name"][0], "required");
    }

    #[test]
    fn standard_member_keys_are_rejected_as_extensions() {
        let problem = Problem::of(ProblemKind::Internal)
            .with_extension("type", "attacker")
            .with_extension("status", 200);
        let value = problem.to_json();
        assert_eq!(value["type"], "urn:arcature:problem:internal");
        assert_eq!(value["status"], 500);
        assert!(value.get("attacker").is_none());
    }

    #[test]
    fn null_extension_values_are_dropped() {
        let problem = Problem::of(ProblemKind::Internal).with_extension("trace", Value::Null);
        let value = problem.to_json();
        assert!(value.get("trace").is_none());
    }

    #[test]
    fn builder_builds_equivalent_problem() {
        let built = Problem::builder(ProblemKind::NotFound)
            .detail("missing")
            .instance("/users/42")
            .extension("retry", false)
            .build();
        let value = built.to_json();
        assert_eq!(value["status"], 404);
        assert_eq!(value["detail"], "missing");
        assert_eq!(value["instance"], "/users/42");
        assert_eq!(value["retry"], false);
    }

    #[test]
    fn custom_problem_uses_explicit_type_and_status() {
        let problem = Problem::custom(
            "https://example.com/probs/out-of-credit",
            StatusCode::PAYMENT_REQUIRED,
        )
        .with_detail("insufficient credit");
        let value = problem.to_json();
        assert_eq!(value["type"], "https://example.com/probs/out-of-credit");
        assert_eq!(value["status"], 402);
        assert_eq!(value["detail"], "insufficient credit");
        assert_eq!(value["title"], "Payment Required");
    }

    #[test]
    fn about_blank_custom_uses_reason_phrase_title() {
        let problem = Problem::custom("about:blank", StatusCode::BAD_REQUEST);
        let value = problem.to_json();
        assert_eq!(value["type"], "about:blank");
        assert_eq!(value["title"], "Bad Request");
        assert_eq!(value["status"], 400);
    }
}
