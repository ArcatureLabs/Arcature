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
use crate::http::security::CspNonce;

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

/// Build a [`ScriptBody`] from a serialized page JSON, the page-element id and
/// this request's Content-Security-Policy nonce.
///
/// The nonce goes on the payload script because that script *is* the
/// application: a policy that blocks `<script data-page>` does not degrade an
/// Inertia page, it leaves a blank `<div>`. With no nonce the markup is
/// byte-for-byte what it was before nonces existed.
pub(crate) fn build_script_body(
    page_json: &str,
    page_id: &str,
    nonce: Option<CspNonce>,
) -> ScriptBody {
    let escaped = escape_script_body(page_json);
    let attribute = nonce.as_ref().map(CspNonce::attribute).unwrap_or_default();
    let html = format!(
        "<script{attribute} data-page=\"{page_id}\" type=\"application/json\">{escaped}</script><div id=\"{page_id}\"></div>"
    );
    ScriptBody::from_escaped(Arc::from(html), nonce)
}

/// Serialize a [`Page`] to JSON.
pub(crate) fn serialize(page: &Page) -> Result<String, InertiaError> {
    serde_json::to_string(page).map_err(InertiaError::from)
}

/// Build the initial-page HTML response (for non-Inertia requests).
pub(crate) fn html(
    page: &Page,
    config: &InertiaConfig,
    nonce: Option<CspNonce>,
    status: StatusCode,
) -> Result<Response, InertiaError> {
    let page_json = serialize(page)?;
    let script_body = build_script_body(&page_json, config.page_id(), nonce);
    let document = config.root_document().render(script_body);

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    ensure_vary_x_inertia(&mut headers);
    Ok((status, headers, Body::from(document)).into_response())
}

/// Build the Inertia JSON response from a serialized page object.
///
/// `status` is the page's own, which is not always `200`: the client decides
/// a response is an Inertia page from the `X-Inertia` header and treats
/// `>= 400` as an event to raise, not a reason to stop rendering. That is how
/// a 404 keeps the application's layout.
pub(crate) fn json_response(page_json: String, status: StatusCode) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(Headers::INERTIA, HeaderValue::from_static("true"));
    ensure_vary_x_inertia(&mut headers);
    (status, headers, Body::from(page_json)).into_response()
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
    fn the_payload_script_carries_the_nonce_when_there_is_one() {
        // A `script-src 'nonce-X'` policy with an un-nonced payload script
        // does not harden the page, it leaves a blank mount point.
        let nonce = CspNonce::generate().expect("OS RNG");
        let body = build_script_body(r#"{"a":1}"#, "app", Some(nonce.clone())).to_string();
        assert!(
            body.starts_with(&format!("<script nonce=\"{nonce}\" data-page=\"app\"")),
            "unexpected markup: {body}"
        );
    }

    #[test]
    fn the_payload_script_is_unchanged_when_there_is_no_nonce() {
        let body = build_script_body(r#"{"a":1}"#, "app", None).to_string();
        assert_eq!(
            body,
            "<script data-page=\"app\" type=\"application/json\">{\"a\":1}</script>\
             <div id=\"app\"></div>"
        );
        assert!(!body.contains("nonce"));
    }

    #[test]
    fn a_root_document_can_read_the_nonce_off_the_script_body() {
        // The documented path for a hand-written `RootDocument` that emits
        // scripts of its own, which the framework never sees.
        let nonce = CspNonce::generate().expect("OS RNG");
        let body = build_script_body("{}", "app", Some(nonce.clone()));
        assert_eq!(body.nonce().map(CspNonce::as_str), Some(nonce.as_str()));
        assert_eq!(body.nonce_attribute(), format!(" nonce=\"{nonce}\""));

        let none = build_script_body("{}", "app", None);
        assert!(none.nonce().is_none());
        assert_eq!(none.nonce_attribute(), "");
    }

    #[test]
    fn a_page_response_keeps_the_status_the_page_asked_for() {
        // `isInertiaResponse` keys on the header, not the status, so a 404
        // page object still renders -- inside the application's own layout
        // rather than the browser's error page.
        let response = json_response("{}".to_string(), StatusCode::NOT_FOUND);
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[Headers::INERTIA], "true");
        assert_eq!(response.headers()[Headers::VARY], "X-Inertia");
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
