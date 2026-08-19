//! Response builders: the safe script-body escaper, the initial-page HTML
//! response, the Inertia JSON page response, and the `Vary: X-Inertia` merge.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use super::config::{InertiaConfig, ScriptBody};
use super::error::InertiaError;
use super::headers::Headers;
use super::page::Page;

/// Escape `json` for safe embedding inside a `<script type="application/json">`
/// element. A provably-unbreakable superset of the official slash-only escape.
pub(crate) fn escape_script_body(json: &str) -> String {
    if json
        .bytes()
        .all(|b| b != b'<' && b != b'>' && b != b'&' && b != b'/')
    {
        return json.to_string();
    }
    let mut out = String::with_capacity(json.len() + json.len() / 16);
    for character in json.chars() {
        match character {
            '<' => out.push_str(r"\u003c"),
            '>' => out.push_str(r"\u003e"),
            '&' => out.push_str(r"\u0026"),
            '/' => out.push_str(r"\/"),
            other => out.push(other),
        }
    }
    out
}

/// Build a [`ScriptBody`] from a serialized page JSON and the page-element id.
pub(crate) fn build_script_body(page_json: &str, page_id: &str) -> ScriptBody {
    let escaped = escape_script_body(page_json);
    let html = format!(
        "<script data-page=\"{page_id}\" type=\"application/json\">{escaped}</script><div id=\"{page_id}\"></div>"
    );
    ScriptBody::from_escaped(Arc::from(html))
}

/// Serialize a [`Page`] to JSON.
pub(crate) fn serialize(page: &Page) -> Result<String, InertiaError> {
    serde_json::to_string(page).map_err(InertiaError::from)
}

/// Build the initial-page HTML response (for non-Inertia requests).
pub(crate) fn html(page: &Page, config: &InertiaConfig) -> Result<Response, InertiaError> {
    let page_json = serialize(page)?;
    let script_body = build_script_body(&page_json, config.page_id());
    let document = config.root_document().render(script_body);

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    ensure_vary_x_inertia(&mut headers);
    Ok((StatusCode::OK, headers, Body::from(document)).into_response())
}

/// Build the Inertia JSON response from a serialized page object.
pub(crate) fn json_response(page_json: String) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(Headers::INERTIA, HeaderValue::from_static("true"));
    ensure_vary_x_inertia(&mut headers);
    (StatusCode::OK, headers, Body::from(page_json)).into_response()
}

/// Ensure `Vary: X-Inertia` is present on `headers`, merging into any existing
/// `Vary` value without duplicating or discarding application values.
pub(crate) fn ensure_vary_x_inertia(headers: &mut HeaderMap) {
    if headers.get_all(Headers::VARY).iter().next().is_none() {
        headers.insert(Headers::VARY, HeaderValue::from_static("X-Inertia"));
        return;
    }
    let mut tokens: Vec<&str> = Vec::new();
    let mut has_star = false;
    for value in headers.get_all(Headers::VARY).iter() {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for token in value
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            if token == "*" {
                has_star = true;
            }
            if !tokens
                .iter()
                .any(|existing: &&str| existing.eq_ignore_ascii_case(token))
            {
                tokens.push(token);
            }
        }
    }
    if has_star {
        return;
    }
    if !tokens
        .iter()
        .any(|token| token.eq_ignore_ascii_case(Headers::INERTIA.as_str()))
    {
        tokens.push("X-Inertia");
    }
    let combined = tokens.join(", ");
    if let Ok(value) = HeaderValue::from_str(&combined) {
        headers.insert(Headers::VARY, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_no_special_chars_unchanged() {
        assert_eq!(escape_script_body(r#"{"a":1}"#), r#"{"a":1}"#);
    }

    #[test]
    fn escape_breakout_neutralized() {
        let escaped = escape_script_body(r#"{"u":"</script>"}"#);
        assert!(!escaped.contains("</script>"));
        let parsed: serde_json::Value = serde_json::from_str(&escaped).unwrap();
        assert_eq!(parsed["u"], "</script>");
    }

    #[test]
    fn escape_comment_breakout_neutralized() {
        let hostile = r#"{"x":"<!--<script>"}"#;
        let escaped = escape_script_body(hostile);
        assert!(!escaped.contains("<!--"));
        let parsed: serde_json::Value = serde_json::from_str(&escaped).unwrap();
        assert_eq!(parsed["x"], "<!--<script>");
    }

    #[test]
    fn vary_merges_into_existing() {
        let mut h = HeaderMap::new();
        h.insert(Headers::VARY, HeaderValue::from_static("Cookie"));
        ensure_vary_x_inertia(&mut h);
        assert_eq!(h.get(Headers::VARY).unwrap(), "Cookie, X-Inertia");
    }

    #[test]
    fn vary_does_not_duplicate() {
        let mut h = HeaderMap::new();
        h.insert(Headers::VARY, HeaderValue::from_static("X-Inertia"));
        ensure_vary_x_inertia(&mut h);
        assert_eq!(h.get(Headers::VARY).unwrap(), "X-Inertia");
    }
}
