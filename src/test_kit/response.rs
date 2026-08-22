//! [`TestResponse`] and the assertions.
//!
//! Every assertion here prints the value it actually saw. A failure that says
//! only `assertion failed` costs another run of the suite to turn into
//! information, and a test kit is judged by what it tells you at two in the
//! morning.
//!
//! Every assertion here also fails when the thing it names is absent. An
//! assertion that passes on a missing prop, a missing field, or an empty
//! error bag is worse than no assertion at all, because it reports success.

use axum::http::{HeaderMap, StatusCode};
// Only the Inertia and Problem assertions read a header by name, so the
// import follows them rather than the module.
#[cfg(any(feature = "inertia", feature = "api"))]
use axum::http::header;
use axum::response::Response;
use http_body_util::BodyExt;
use serde_json::Value;

use super::preview;

/// How much of a body a failure message prints.
const PREVIEW_LIMIT: usize = 2000;

/// A response with its body already collected.
#[derive(Debug, Clone)]
pub struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl TestResponse {
    /// Collect a response body into memory.
    pub(crate) async fn collect(response: Response) -> Self {
        let (parts, body) = response.into_parts();
        let body = body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("response body could not be read: {error}"))
            .to_bytes()
            .to_vec();
        Self {
            status: parts.status,
            headers: parts.headers,
            body,
        }
    }

    /// The status code.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The response headers.
    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// The raw body bytes.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// The body as UTF-8 text, lossily.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

impl TestResponse {
    /// The body parsed as JSON.
    ///
    /// # Panics
    ///
    /// Panics if the body is not JSON, printing the body.
    #[must_use]
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "response body is not JSON ({error})\n  status: {}\n  body: {}",
                self.status,
                preview(&self.body, PREVIEW_LIMIT)
            )
        })
    }

    /// The value at a dotted `path`, or `None` when the path does not exist.
    ///
    /// Path segments are object keys or array indices: `props.users.0.email`.
    #[must_use]
    pub fn json_path(&self, path: &str) -> Option<Value> {
        resolve(&self.json(), path).cloned()
    }

    /// Assert the status code.
    ///
    /// # Panics
    ///
    /// Panics when the status differs, printing the status seen and the body
    /// -- which is where a 500 says what went wrong.
    pub fn assert_status(&self, expected: StatusCode) -> &Self {
        assert!(
            self.status == expected,
            "expected status {expected}, got {}\n  body: {}",
            self.status,
            preview(&self.body, PREVIEW_LIMIT)
        );
        self
    }

    /// Assert `200 OK`.
    ///
    /// # Panics
    ///
    /// Panics when the status is not `200`.
    pub fn assert_ok(&self) -> &Self {
        self.assert_status(StatusCode::OK)
    }

    /// Assert a header is present with an exact value.
    ///
    /// # Panics
    ///
    /// Panics when the header is absent or differs, listing what was sent.
    pub fn assert_header(&self, name: &str, expected: &str) -> &Self {
        let actual = self.headers.get(name).map(|value| {
            value
                .to_str()
                .map_or_else(|_| format!("{value:?}"), str::to_owned)
        });
        match actual {
            Some(actual) if actual == expected => self,
            Some(actual) => panic!("header `{name}`: expected `{expected}`, got `{actual}`"),
            None => panic!(
                "header `{name}` is absent; the response carries: {}",
                header_names(&self.headers)
            ),
        }
    }
}

impl TestResponse {
    /// Assert the response redirects to `location`.
    ///
    /// Accepts the 3xx family and the Inertia `409` control responses, whose
    /// destination is in `X-Inertia-Location` or `X-Inertia-Redirect` rather
    /// than `Location` -- a test should not have to know which of the three
    /// a handler chose to say the same thing.
    ///
    /// # Panics
    ///
    /// Panics when the response does not redirect, or redirects elsewhere.
    pub fn assert_redirect(&self, location: &str) -> &Self {
        let Some((header_name, actual)) = self.redirect_target() else {
            panic!(
                "expected a redirect to `{location}`, got status {} with no redirect header\n  body: {}",
                self.status,
                preview(&self.body, PREVIEW_LIMIT)
            );
        };
        assert!(
            actual == location,
            "expected a redirect to `{location}`, got `{actual}` (status {}, via `{header_name}`)",
            self.status
        );
        self
    }

    /// The redirect destination and the header it came from.
    #[must_use]
    pub fn redirect_target(&self) -> Option<(&'static str, String)> {
        let candidates: [(&'static str, bool); 3] = [
            ("location", self.status.is_redirection()),
            ("x-inertia-location", self.status == StatusCode::CONFLICT),
            ("x-inertia-redirect", self.status == StatusCode::CONFLICT),
        ];
        for (name, eligible) in candidates {
            if !eligible {
                continue;
            }
            if let Some(value) = self.headers.get(name).and_then(|value| value.to_str().ok()) {
                return Some((name, value.to_owned()));
            }
        }
        None
    }

    /// Assert the JSON value at a dotted `path`.
    ///
    /// # Panics
    ///
    /// Panics when the path does not exist -- naming the longest prefix that
    /// did and what was there -- or when the value differs.
    pub fn assert_json_path(&self, path: &str, expected: impl Into<Value>) -> &Self {
        let expected = expected.into();
        let root = self.json();
        match resolve(&root, path) {
            Some(actual) if *actual == expected => self,
            Some(actual) => panic!("json path `{path}`: expected {expected}, got {actual}"),
            None => panic!(
                "json path `{path}` does not exist; {}",
                nearest(&root, path)
            ),
        }
    }
}

#[cfg(feature = "inertia")]
impl TestResponse {
    /// The Inertia page object.
    ///
    /// An Inertia visit answers with the bare page object as JSON; a first
    /// load answers with the root HTML document carrying the same object in a
    /// `<script data-page=... type="application/json">` block. This reads
    /// whichever one arrived so a test does not have to care which it asked
    /// for.
    ///
    /// # Panics
    ///
    /// Panics when the response carries no page object at all, printing the
    /// status and the body.
    #[must_use]
    pub fn inertia_page(&self) -> Value {
        if let Some(page) = self.try_inertia_page() {
            return page;
        }
        panic!(
            "response carries no Inertia page object\n  status: {}\n  content-type: {}\n  body: {}",
            self.status,
            self.headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("(none)"),
            preview(&self.body, PREVIEW_LIMIT)
        );
    }

    /// The Inertia page object, or `None` when there is none.
    #[must_use]
    pub fn try_inertia_page(&self) -> Option<Value> {
        let inertia = self
            .headers
            .get("x-inertia")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("true"));
        if inertia {
            return serde_json::from_slice(&self.body).ok();
        }
        let html = std::str::from_utf8(&self.body).ok()?;
        let json = extract_data_page(html)?;
        serde_json::from_str(&json).ok()
    }
}

#[cfg(feature = "inertia")]
impl TestResponse {
    /// Assert the Inertia component name, e.g. `users/index`.
    ///
    /// # Panics
    ///
    /// Panics when there is no page object, when it has no `component` key,
    /// or when the component differs.
    pub fn assert_inertia_component(&self, expected: &str) -> &Self {
        let page = self.inertia_page();
        match page.get("component").and_then(Value::as_str) {
            Some(actual) if actual == expected => self,
            Some(actual) => {
                panic!("expected Inertia component `{expected}`, got `{actual}`")
            }
            None => panic!(
                "expected Inertia component `{expected}`, but the page object has no `component` key\n  page: {page}"
            ),
        }
    }

    /// Assert a prop, addressed by a dotted path under `props`.
    ///
    /// `assert_inertia_prop("users.0.email", "ada@example.com")` reads
    /// `props.users.0.email` of the page object.
    ///
    /// # Panics
    ///
    /// Panics when there is no page object, when the prop is absent -- an
    /// absent prop is a failure, never a pass -- or when it differs.
    pub fn assert_inertia_prop(&self, path: &str, expected: impl Into<Value>) -> &Self {
        let expected = expected.into();
        let page = self.inertia_page();
        let props = page.get("props").unwrap_or(&Value::Null);
        match resolve(props, path) {
            Some(actual) if *actual == expected => self,
            Some(actual) => panic!("Inertia prop `{path}`: expected {expected}, got {actual}"),
            None => panic!(
                "Inertia prop `{path}` does not exist; {}",
                nearest(props, path)
            ),
        }
    }
}

impl TestResponse {
    /// Assert the response reports a validation failure for `field`.
    ///
    /// Reads the `errors` member of an RFC 9457 problem document, and -- with
    /// the `inertia` feature -- the `props.errors` bag of a page object, so
    /// the same assertion works for an API endpoint and for the page a form
    /// post came back to.
    ///
    /// # Panics
    ///
    /// Panics when the body carries no error bag, or when the bag has no
    /// entry for `field`. An empty bag fails; it never passes silently.
    pub fn assert_validation_error(&self, field: &str) -> &Self {
        let Some(errors) = self.validation_errors() else {
            panic!(
                "expected a validation error for `{field}`, but the response carries no error bag\n  status: {}\n  body: {}",
                self.status,
                preview(&self.body, PREVIEW_LIMIT)
            );
        };
        assert!(
            resolve(&errors, field).is_some(),
            "expected a validation error for `{field}`; the bag holds {}",
            keys_of(&errors)
        );
        self
    }

    /// What to read when the body is not itself JSON.
    ///
    /// Two definitions rather than one `cfg!` inside the caller, because the
    /// two arms are not the same *kind* of expression: with `inertia` on it is
    /// a method call that parses the body, and with it off it is the literal
    /// `None`. Written inline, the second spelling makes the closure look
    /// pointlessly lazy to clippy, and taking the suggested `or` would make
    /// the first spelling parse the body on every call whether or not the
    /// first parse already succeeded.
    #[cfg(feature = "inertia")]
    fn fallback_page(&self) -> Option<Value> {
        self.try_inertia_page()
    }

    /// No fallback: without `inertia` a body that is not JSON is not a page.
    #[cfg(not(feature = "inertia"))]
    fn fallback_page(&self) -> Option<Value> {
        None
    }

    /// The validation error bag, from wherever this response put it.
    #[must_use]
    pub fn validation_errors(&self) -> Option<Value> {
        let body: Value = serde_json::from_slice(&self.body)
            .ok()
            .or_else(|| self.fallback_page())?;
        if let Some(errors) = body.get("errors") {
            return Some(errors.clone());
        }
        body.get("props")
            .and_then(|props| props.get("errors"))
            .cloned()
    }
}

#[cfg(feature = "api")]
impl TestResponse {
    /// Assert the response is the RFC 9457 problem document for `kind`.
    ///
    /// Checks all three things that make a problem response correct: the
    /// status, the `application/problem+json` content type, and the `type`
    /// URI. Checking only the status would pass on a plain error page that
    /// happens to share the code.
    ///
    /// # Panics
    ///
    /// Panics when any of the three differ, naming which.
    pub fn assert_problem(&self, kind: crate::api::ProblemKind) -> &Self {
        self.assert_status(kind.status());
        let content_type = self
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("(none)");
        assert!(
            content_type.starts_with(crate::api::PROBLEM_JSON),
            "expected content type `{}`, got `{content_type}`",
            crate::api::PROBLEM_JSON
        );
        let body = self.json();
        let actual = body.get("type").and_then(Value::as_str).unwrap_or("(none)");
        assert!(
            actual == kind.type_uri(),
            "expected problem type `{}`, got `{actual}`\n  body: {body}",
            kind.type_uri()
        );
        self
    }
}

/// Resolve a dotted path against a JSON value.
///
/// A segment addresses an object key or, for an array, a decimal index.
fn resolve<'value>(root: &'value Value, path: &str) -> Option<&'value Value> {
    let mut current = root;
    for segment in path.split('.') {
        current = step(current, segment)?;
    }
    Some(current)
}

/// One path segment.
fn step<'value>(value: &'value Value, segment: &str) -> Option<&'value Value> {
    match value {
        Value::Object(map) => map.get(segment),
        Value::Array(items) => segment.parse::<usize>().ok().and_then(|i| items.get(i)),
        _ => None,
    }
}

/// Describe how far a failing path got, for the failure message.
///
/// Reporting only "path not found" leaves the reader guessing whether the
/// typo is in the first segment or the last. This names the longest prefix
/// that did resolve and what keys were available there.
fn nearest(root: &Value, path: &str) -> String {
    let mut current = root;
    let mut reached: Vec<&str> = Vec::new();
    for segment in path.split('.') {
        match step(current, segment) {
            Some(next) => {
                reached.push(segment);
                current = next;
            }
            None => {
                let at = if reached.is_empty() {
                    "the root".to_owned()
                } else {
                    format!("`{}`", reached.join("."))
                };
                return format!(
                    "`{segment}` is missing at {at}, which holds {}",
                    keys_of(current)
                );
            }
        }
    }
    String::new()
}

/// A short description of what a JSON value offers, for failure messages.
fn keys_of(value: &Value) -> String {
    match value {
        Value::Object(map) if map.is_empty() => "an empty object".to_owned(),
        Value::Object(map) => format!(
            "the keys [{}]",
            map.keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Array(items) => format!("an array of {} items", items.len()),
        other => format!("{other}"),
    }
}

/// The header names on a response, sorted, for failure messages.
fn header_names(headers: &HeaderMap) -> String {
    let mut names: Vec<&str> = headers.keys().map(axum::http::HeaderName::as_str).collect();
    names.sort_unstable();
    names.dedup();
    if names.is_empty() {
        return "no headers".to_owned();
    }
    format!("[{}]", names.join(", "))
}

/// Pull the JSON out of the `<script data-page=...>` block of a root document.
///
/// The framework escapes `<`, `>`, `&`, and `/` when it writes that block, so
/// the closing tag cannot occur inside the payload and a plain search for it
/// is exact rather than a heuristic. Those escapes are JSON escapes, so the
/// text found here parses directly.
#[cfg(feature = "inertia")]
fn extract_data_page(html: &str) -> Option<String> {
    let start = html.find("<script data-page=")?;
    let open_end = html[start..].find('>')? + start + 1;
    let close = html[open_end..].find("</script>")? + open_end;
    Some(html[open_end..close].to_owned())
}
