//! The fluent request builder.
//!
//! `app.post("/users").json(&body).acting_as(&user).send().await` reads as
//! the request it makes. Nothing is sent until [`send`](TestRequest::send),
//! so a helper can hand a half-built request back to a caller that finishes
//! it.

use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
// The only cookie the harness sets is the session cookie.
#[cfg(feature = "auth")]
use axum::http::header::COOKIE;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode};

use super::app::TestApp;
use super::response::TestResponse;

/// Redirects followed before the harness gives up.
///
/// A redirect loop in a handler is a bug; following it forever turns that bug
/// into a hung test suite with no message.
const MAX_REDIRECTS: usize = 10;

/// A request under construction.
#[derive(Debug)]
pub struct TestRequest {
    app: TestApp,
    method: Method,
    uri: String,
    headers: HeaderMap,
    body: Vec<u8>,
    follow_redirects: bool,
    #[cfg(feature = "auth")]
    session_entries: Vec<(String, serde_json::Value)>,
}

impl TestRequest {
    pub(crate) fn new(app: TestApp, method: Method, uri: String) -> Self {
        Self {
            app,
            method,
            uri,
            headers: HeaderMap::new(),
            body: Vec::new(),
            follow_redirects: false,
            #[cfg(feature = "auth")]
            session_entries: Vec::new(),
        }
    }

    /// Set a header. Replaces any previous value for the same name.
    ///
    /// # Panics
    ///
    /// Panics if `name` or `value` is not a valid header -- a mistake in the
    /// test itself, reported where it was made.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        let name: HeaderName = name
            .parse()
            .unwrap_or_else(|_| panic!("`{name}` is not a valid header name"));
        let value: HeaderValue = value
            .parse()
            .unwrap_or_else(|_| panic!("`{value}` is not a valid header value"));
        self.headers.insert(name, value);
        self
    }
}

impl TestRequest {
    /// Send `value` as a JSON body, setting `Content-Type: application/json`.
    ///
    /// # Panics
    ///
    /// Panics if `value` does not serialize -- the test's own fixture is
    /// wrong, and failing here names it.
    #[must_use]
    pub fn json<T>(mut self, value: &T) -> Self
    where
        T: serde::Serialize + ?Sized,
    {
        self.body = serde_json::to_vec(value)
            .unwrap_or_else(|error| panic!("request body did not serialize to JSON: {error}"));
        self.headers
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        self
    }

    /// Send `value` as a form body, setting
    /// `Content-Type: application/x-www-form-urlencoded`.
    ///
    /// `value` is serialized to JSON first and must be a flat object; a
    /// value may be a scalar or an array of scalars (repeated key). A nested
    /// object has no single agreed form encoding, so it is refused rather
    /// than guessed at.
    ///
    /// # Panics
    ///
    /// Panics if `value` does not serialize, is not an object, or contains a
    /// nested object.
    #[must_use]
    pub fn form<T>(mut self, value: &T) -> Self
    where
        T: serde::Serialize + ?Sized,
    {
        let value = serde_json::to_value(value)
            .unwrap_or_else(|error| panic!("form body did not serialize: {error}"));
        self.body = encode_form(&value).into_bytes();
        self.headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        self
    }

    /// Send a raw body with an explicit content type.
    #[must_use]
    pub fn body(mut self, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        let value: HeaderValue = content_type
            .parse()
            .unwrap_or_else(|_| panic!("`{content_type}` is not a valid content type"));
        self.headers.insert(CONTENT_TYPE, value);
        self
    }

    /// Mark the request as an Inertia visit (`X-Inertia: true`), so the
    /// adapter answers with the page object rather than the root document.
    #[cfg(feature = "inertia")]
    #[must_use]
    pub fn inertia(self) -> Self {
        self.header("x-inertia", "true")
    }
}

impl TestRequest {
    /// Send the request as `user`.
    ///
    /// Writes `U::SESSION_KEY` and the authentication timestamp the auth
    /// boundary reads for absolute-lifetime enforcement -- the same two
    /// entries `AuthManager::login` writes. Omitting the timestamp would
    /// exercise the "session predates this feature" upgrade path instead of
    /// the ordinary logged-in one.
    ///
    /// Requires [`TestApp::with_sessions`].
    ///
    /// # Panics
    ///
    /// Panics if the user id does not serialize.
    #[cfg(feature = "auth")]
    #[must_use]
    pub fn acting_as<U>(mut self, user: &U) -> Self
    where
        U: crate::auth::AuthUser,
    {
        let id = serde_json::to_value(user.id())
            .unwrap_or_else(|error| panic!("user id did not serialize: {error}"));
        self.session_entries.push((U::SESSION_KEY.to_string(), id));
        self.session_entries.push((
            AUTH_AT_KEY.to_string(),
            serde_json::Value::from(now_unix_millis()),
        ));
        self
    }

    /// Put a value in the session the request arrives with.
    ///
    /// Requires [`TestApp::with_sessions`].
    ///
    /// # Panics
    ///
    /// Panics if `value` does not serialize.
    #[cfg(feature = "auth")]
    #[must_use]
    pub fn with_session<T>(mut self, key: &str, value: T) -> Self
    where
        T: serde::Serialize,
    {
        let value = serde_json::to_value(value)
            .unwrap_or_else(|error| panic!("session value for `{key}` did not serialize: {error}"));
        self.session_entries.push((key.to_string(), value));
        self
    }

    /// Follow `Location` redirects (up to ten) and return the final response.
    #[must_use]
    pub fn follow_redirects(mut self) -> Self {
        self.follow_redirects = true;
        self
    }
}

/// The session key the auth boundary stamps at login. Mirrors the private
/// constant in `auth::extract`; the harness has to write the same key the loader
/// reads, and a mismatch would show up as `acting_as` silently taking the
/// upgrade path.
#[cfg(feature = "auth")]
const AUTH_AT_KEY: &str = "__arcature_absolute_auth_at";

#[cfg(feature = "auth")]
fn now_unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

impl TestRequest {
    /// Send the request and collect the response.
    ///
    /// # Panics
    ///
    /// Panics if the request cannot be built (an invalid URI), if a session
    /// was requested without [`TestApp::with_sessions`], or if the response
    /// body cannot be read.
    pub async fn send(self) -> TestResponse {
        let Self {
            app,
            method,
            uri,
            headers,
            body,
            follow_redirects,
            #[cfg(feature = "auth")]
            session_entries,
        } = self;

        // Only the session cookie mutates the header map, so the binding is
        // mutable only when there are sessions to attach.
        #[cfg(feature = "auth")]
        let mut headers = headers;

        #[cfg(feature = "auth")]
        if !session_entries.is_empty() {
            let sessions = app.sessions().unwrap_or_else(|| {
                panic!(
                    "`acting_as` / `with_session` need a session store: build the \
                     application with `.session(config, sessions.store())` and the \
                     harness with `TestApp::new(app).with_sessions(sessions)`"
                )
            });
            let cookie = sessions
                .cookie_for(&session_entries)
                .await
                .unwrap_or_else(|error| panic!("session could not be seeded: {error}"));
            let value: HeaderValue = cookie
                .parse()
                .unwrap_or_else(|_| panic!("seeded cookie is not a valid header value"));
            headers.insert(COOKIE, value);
        }

        let mut response = dispatch(&app, &method, &uri, &headers, body).await;
        if follow_redirects {
            let mut hops = 0;
            while let Some(location) = redirect_target(&response) {
                assert!(
                    hops < MAX_REDIRECTS,
                    "redirect loop: still redirecting after {MAX_REDIRECTS} hops, last to `{location}`"
                );
                hops += 1;
                response = dispatch(&app, &Method::GET, &location, &headers, Vec::new()).await;
            }
        }
        TestResponse::collect(response).await
    }
}

/// The `Location` of a 3xx response, if it has one.
fn redirect_target(response: &axum::response::Response) -> Option<String> {
    let status = response.status();
    if !matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    ) {
        return None;
    }
    response
        .headers()
        .get(axum::http::header::LOCATION)?
        .to_str()
        .ok()
        .map(str::to_owned)
}

async fn dispatch(
    app: &TestApp,
    method: &Method,
    uri: &str,
    headers: &HeaderMap,
    body: Vec<u8>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method.clone()).uri(uri);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let request = builder
        .body(Body::from(body))
        .unwrap_or_else(|error| panic!("could not build a request for `{uri}`: {error}"));
    app.dispatch(request).await
}

/// Encode a flat JSON object as `application/x-www-form-urlencoded`.
///
/// Hand-rolled rather than pulled from a crate because the rule is short and
/// the alternative is a dependency in the production feature graph for the
/// sake of a test helper.
fn encode_form(value: &serde_json::Value) -> String {
    let serde_json::Value::Object(fields) = value else {
        panic!("a form body must be an object, got: {value}");
    };
    let mut out = String::new();
    for (key, field) in fields {
        match field {
            serde_json::Value::Array(items) => {
                for item in items {
                    push_pair(&mut out, key, &scalar(key, item));
                }
            }
            other => push_pair(&mut out, key, &scalar(key, other)),
        }
    }
    out
}

fn scalar(key: &str, value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Bool(flag) => flag.to_string(),
        serde_json::Value::Null => String::new(),
        other => panic!(
            "form field `{key}` is a nested {}, which has no single form encoding; \
             flatten it in the test or use `.json(..)`",
            if other.is_array() { "array" } else { "object" }
        ),
    }
}

fn push_pair(out: &mut String, key: &str, value: &str) {
    if !out.is_empty() {
        out.push('&');
    }
    percent_encode_into(out, key);
    out.push('=');
    percent_encode_into(out, value);
}

/// Percent-encode into `out` per the URL-encoded form serialization rules:
/// unreserved characters verbatim, space as `+`, everything else as `%XX`
/// over the UTF-8 bytes.
fn percent_encode_into(out: &mut String, text: &str) {
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(char::from(*byte));
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
}
