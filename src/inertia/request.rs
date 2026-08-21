//! The validated Inertia request context.

use std::sync::Arc;

use axum::http::{HeaderMap, Method, Uri};

use super::headers::{Headers, Values};

/// The Inertia-specific request semantics, parsed once from the request.
#[derive(Debug, Clone)]
pub struct InertiaRequest {
    is_inertia: bool,
    method: Method,
    url: Arc<str>,
    referer: Option<Arc<str>>,
    request_version: Option<Arc<str>>,
    partial_component: Option<Arc<str>>,
    only: Vec<Arc<str>>,
    except: Vec<Arc<str>>,
    reset: Vec<Arc<str>>,
    error_bag: Option<Arc<str>>,
    except_once: Vec<Arc<str>>,
    merge_intent: Option<MergeIntent>,
    is_prefetch: bool,
}

/// Which end of an infinite scroll the client is loading.
///
/// Sent as `X-Inertia-Infinite-Scroll-Merge-Intent`. It decides whether the
/// incoming page of records goes in front of or behind the ones the client
/// already has, which on the wire is the difference between `prependProps`
/// and `mergeProps` for the scroll prop's array path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeIntent {
    /// Loading backwards: put the incoming records before the existing ones.
    Prepend,
    /// Loading forwards: put the incoming records after the existing ones.
    Append,
}

impl MergeIntent {
    fn parse(value: &str) -> Option<MergeIntent> {
        match value {
            Values::MERGE_INTENT_PREPEND => Some(MergeIntent::Prepend),
            Values::MERGE_INTENT_APPEND => Some(MergeIntent::Append),
            // The client sends exactly those two. Anything else is not a
            // dialect to guess at -- treat it as absent and merge normally.
            _ => None,
        }
    }
}

/// A resolved partial-reload selection for a specific page component.
#[derive(Debug, Clone)]
pub struct PartialSelection {
    pub only: Vec<Arc<str>>,
    pub except: Vec<Arc<str>>,
    pub reset: Vec<Arc<str>>,
}

/// The maximum number of keys accepted in one comma-separated header (DoS bound).
const MAX_SELECTOR_KEYS: usize = 64;
const MAX_SELECTOR_BYTES: usize = 8 * 1024;

fn parse_comma_list(value: Option<&str>) -> Vec<Arc<str>> {
    let Some(value) = value else {
        return Vec::new();
    };
    let bounded: &str = if value.len() > MAX_SELECTOR_BYTES {
        &value[..value.floor_char_boundary(MAX_SELECTOR_BYTES)]
    } else {
        value
    };
    let mut out: Vec<Arc<str>> = Vec::new();
    for part in bounded.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        if out.iter().any(|existing: &Arc<str>| &**existing == trimmed) {
            continue;
        }
        if out.len() >= MAX_SELECTOR_KEYS {
            break;
        }
        out.push(Arc::from(trimmed));
    }
    out
}

impl InertiaRequest {
    /// Parse the Inertia request context from headers, method, and URI.
    pub fn parse(headers: &HeaderMap, method: &Method, uri: &Uri) -> InertiaRequest {
        let is_inertia = headers
            .get(Headers::INERTIA)
            .and_then(|v| v.to_str().ok())
            .map(|v| v == Values::INERTIA_TRUE)
            .unwrap_or(false);

        let request_version = headers
            .get(Headers::VERSION)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
            .map(Arc::from);

        let partial_component = headers
            .get(Headers::PARTIAL_COMPONENT)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
            .map(Arc::from);

        let only = parse_comma_list(
            headers
                .get(Headers::PARTIAL_DATA)
                .and_then(|v| v.to_str().ok()),
        );
        let except = parse_comma_list(
            headers
                .get(Headers::PARTIAL_EXCEPT)
                .and_then(|v| v.to_str().ok()),
        );
        let reset = parse_comma_list(headers.get(Headers::RESET).and_then(|v| v.to_str().ok()));

        let error_bag = headers
            .get(Headers::ERROR_BAG)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
            .map(Arc::from);

        // Map keys of `page.onceProps`, not prop paths: the client sends the
        // keys it holds a value for, and the server answers by leaving those
        // values out while still naming the entries.
        let except_once = parse_comma_list(
            headers
                .get(Headers::EXCEPT_ONCE_PROPS)
                .and_then(|v| v.to_str().ok()),
        );

        let merge_intent = headers
            .get(Headers::MERGE_INTENT)
            .and_then(|v| v.to_str().ok())
            .and_then(MergeIntent::parse);

        let referer = headers
            .get(axum::http::header::REFERER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(Arc::from);

        let is_prefetch = headers
            .get(Headers::PURPOSE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v == Values::PURPOSE_PREFETCH)
            .unwrap_or(false);

        let url = Arc::from(uri.to_string());

        InertiaRequest {
            is_inertia,
            method: method.clone(),
            url,
            referer,
            request_version,
            partial_component,
            only,
            except,
            reset,
            error_bag,
            except_once,
            merge_intent,
            is_prefetch,
        }
    }

    /// Whether this is an Inertia request (`X-Inertia: true`).
    pub fn is_inertia(&self) -> bool {
        self.is_inertia
    }

    /// The request method.
    pub fn method(&self) -> &Method {
        &self.method
    }

    /// The request URL (path + query).
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The referring URL used by the official empty-response fallback.
    pub fn referer(&self) -> Option<&str> {
        self.referer.as_deref()
    }

    /// The asset version the client holds, if any.
    pub fn request_version(&self) -> Option<&str> {
        self.request_version.as_deref()
    }

    /// The requested error-bag name, if any.
    pub fn error_bag(&self) -> Option<&str> {
        self.error_bag.as_deref()
    }

    /// The once keys the client already holds a value for.
    ///
    /// These are keys of the page object's `onceProps` map, not prop paths.
    pub fn except_once(&self) -> &[Arc<str>] {
        &self.except_once
    }

    /// Whether `key` is a once key the client already holds.
    pub fn holds_once(&self, key: &str) -> bool {
        self.except_once.iter().any(|held| &**held == key)
    }

    /// Which end of an infinite scroll this request is loading, if any.
    pub fn merge_intent(&self) -> Option<MergeIntent> {
        self.merge_intent
    }

    /// Whether this is a prefetch visit (`Purpose: prefetch`).
    pub fn is_prefetch(&self) -> bool {
        self.is_prefetch
    }

    /// Resolve the partial-reload selection for `component`.
    pub fn partial_for(&self, component: &str) -> Option<PartialSelection> {
        let matches = self
            .partial_component
            .as_deref()
            .map(|c| c == component)
            .unwrap_or(false);
        if !matches {
            return None;
        }
        Some(PartialSelection {
            only: self.only.clone(),
            except: self.except.clone(),
            reset: self.reset.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn parse(pairs: &[(&'static str, &str)]) -> InertiaRequest {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                axum::http::HeaderName::from_static(name),
                HeaderValue::from_str(value).expect("header value"),
            );
        }
        InertiaRequest::parse(&headers, &Method::GET, &Uri::from_static("/users"))
    }

    #[test]
    fn a_request_without_inertia_headers_is_not_an_inertia_request() {
        let request = parse(&[]);
        assert!(!request.is_inertia());
        assert!(!request.is_prefetch());
        assert!(request.request_version().is_none());
        assert!(request.merge_intent().is_none());
        assert!(request.except_once().is_empty());
    }

    #[test]
    fn except_once_carries_map_keys_in_order_without_duplicates() {
        let request = parse(&[("x-inertia-except-once-props", " a , b ,a, c ")]);
        let keys: Vec<&str> = request.except_once().iter().map(|k| &**k).collect();
        assert_eq!(keys, ["a", "b", "c"]);
        assert!(request.holds_once("b"));
        assert!(!request.holds_once("d"));
    }

    #[test]
    fn the_merge_intent_is_only_ever_prepend_or_append() {
        assert_eq!(
            parse(&[("x-inertia-infinite-scroll-merge-intent", "prepend")]).merge_intent(),
            Some(MergeIntent::Prepend)
        );
        assert_eq!(
            parse(&[("x-inertia-infinite-scroll-merge-intent", "append")]).merge_intent(),
            Some(MergeIntent::Append)
        );
        // Not a dialect to guess at: an unknown value means "no intent", which
        // merges the way a plain partial reload would.
        assert_eq!(
            parse(&[("x-inertia-infinite-scroll-merge-intent", "sideways")]).merge_intent(),
            None
        );
    }

    #[test]
    fn a_partial_selection_only_applies_to_the_component_it_names() {
        let request = parse(&[
            ("x-inertia", "true"),
            ("x-inertia-partial-component", "users/index"),
            ("x-inertia-partial-data", "users,filters"),
            ("x-inertia-partial-except", "users.token"),
            ("x-inertia-reset", "users"),
        ]);
        assert!(request.partial_for("users/show").is_none());
        let selection = request.partial_for("users/index").expect("selection");
        assert_eq!(selection.only.len(), 2);
        assert_eq!(&*selection.except[0], "users.token");
        assert_eq!(&*selection.reset[0], "users");
    }

    #[test]
    fn a_selector_header_cannot_grow_without_bound() {
        // Both bounds are DoS limits, not protocol limits: a hostile client
        // must not be able to make the server allocate a key per byte.
        let many = (0..MAX_SELECTOR_KEYS * 2)
            .map(|i| format!("k{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let request = parse(&[("x-inertia-partial-data", &many)]);
        let selection = InertiaRequest {
            partial_component: Some(Arc::from("c")),
            ..request
        }
        .partial_for("c")
        .expect("selection");
        assert_eq!(selection.only.len(), MAX_SELECTOR_KEYS);
    }
}
