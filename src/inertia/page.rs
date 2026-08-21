//! The Inertia page object wire type, component name, and page options.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::http::StatusCode;
use serde::Serialize;
use serde_json::Value;

/// A frontend page component name. Cheaply cloneable (`Arc<str>`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Component(Arc<str>);

impl Component {
    /// Create a component name from a string.
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        Component(name.into())
    }

    /// The component name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Component {
    fn from(s: &str) -> Self {
        Component(Arc::from(s))
    }
}

impl From<String> for Component {
    fn from(s: String) -> Self {
        Component(Arc::from(s))
    }
}

impl std::fmt::Display for Component {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One page of an infinite scroll, as the client identifies it.
///
/// The official client types this as `string | number | null` and tests it
/// with `!!state.nextPage`, so the two representations are not
/// interchangeable at the edges: a cursor of `0` or `""` reads as *exhausted*.
/// [`ScrollProp`] therefore models "there is no further page" as `None`, and
/// this type never has to stand in for absence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum PageIdentifier {
    /// A numeric page number or offset.
    Number(i64),
    /// A cursor, slug, or any other opaque string.
    Text(String),
}

impl From<i64> for PageIdentifier {
    fn from(value: i64) -> Self {
        PageIdentifier::Number(value)
    }
}

impl From<u32> for PageIdentifier {
    fn from(value: u32) -> Self {
        PageIdentifier::Number(i64::from(value))
    }
}

impl From<&str> for PageIdentifier {
    fn from(value: &str) -> Self {
        PageIdentifier::Text(value.to_string())
    }
}

impl From<String> for PageIdentifier {
    fn from(value: String) -> Self {
        PageIdentifier::Text(value)
    }
}

/// The infinite-scroll state the client needs to ask for the next slice.
///
/// Attached to a prop with [`infinite_scroll`](super::props::infinite_scroll)
/// and emitted under the page object's `scrollProps`, keyed by that prop's
/// name. The client reads `previousPage`/`nextPage` to decide which
/// directions are still loadable and echoes the direction it chose back as
/// `X-Inertia-Infinite-Scroll-Merge-Intent`.
///
/// Every field is serialized even when empty: the client reads
/// `scrollProp.nextPage` directly, and an omitted key and an explicit `null`
/// are the same thing to it only because `null` is what it expects to find.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollProp {
    page_name: String,
    previous_page: Option<PageIdentifier>,
    next_page: Option<PageIdentifier>,
    current_page: Option<PageIdentifier>,
    reset: bool,
}

impl ScrollProp {
    /// Create scroll state for the query parameter named `page_name`.
    ///
    /// Every page identifier starts absent, which the client reads as "there
    /// is nothing in that direction". Fill in the ones that exist.
    pub fn new(page_name: impl Into<String>) -> Self {
        ScrollProp {
            page_name: page_name.into(),
            previous_page: None,
            next_page: None,
            current_page: None,
            reset: false,
        }
    }

    /// The page that was just loaded.
    pub fn current(mut self, page: impl Into<PageIdentifier>) -> Self {
        self.current_page = Some(page.into());
        self
    }

    /// The page before this one. Leave unset when this is the first page.
    pub fn previous(mut self, page: impl Into<PageIdentifier>) -> Self {
        self.previous_page = Some(page.into());
        self
    }

    /// The page after this one. Leave unset when this is the last page.
    pub fn next(mut self, page: impl Into<PageIdentifier>) -> Self {
        self.next_page = Some(page.into());
        self
    }

    /// Tell the client to throw away what it has and start from this page.
    pub fn reset(mut self) -> Self {
        self.reset = true;
        self
    }

    /// The query parameter this scroll state pages through.
    pub fn page_name(&self) -> &str {
        &self.page_name
    }
}

/// One entry of the page object's `onceProps` map.
///
/// `prop` is the dotted path of the prop whose value the client should hold
/// on to. `expires_at` is a millisecond epoch on the *client's* clock: the
/// client compares it against its own `Date.now()`, and on re-delivery keeps
/// the value it stored the first time rather than the one sent now.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OncePropEntry {
    pub prop: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

/// The Inertia page object, ready to serialize to JSON.
#[derive(Debug, Serialize)]
pub(crate) struct Page {
    pub component: String,
    pub props: Value,
    pub url: String,
    /// `string | null` on the client. An application with no build step has
    /// no asset version to report, and `null` is how it says so -- the key
    /// itself is not optional.
    pub version: Option<String>,
    #[serde(flatten)]
    pub metadata: PageMetadata,
}

/// The conditional metadata fields of the Inertia page object.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageMetadata {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub encrypt_history: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub clear_history: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub preserve_fragment: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flash: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shared_props: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub merge_props: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prepend_props: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deep_merge_props: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub match_props_on: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub scroll_props: BTreeMap<String, ScrollProp>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub once_props: BTreeMap<String, OncePropEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred_props: Option<BTreeMap<String, Vec<String>>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rescued_props: Vec<String>,
}

impl PageMetadata {
    pub(crate) fn apply_options(&mut self, options: PageOptions) {
        self.encrypt_history = options.encrypt_history;
        self.clear_history = options.clear_history;
        self.preserve_fragment = options.preserve_fragment;
        self.flash = options.flash;
        for key in options.match_props_on {
            self.record_match_on(&key);
        }
    }

    pub(crate) fn record_merge(&mut self, strategy: super::props::MergeStrategy, path: &str) {
        let entry = path.to_string();
        let entries = match strategy {
            super::props::MergeStrategy::Merge => &mut self.merge_props,
            super::props::MergeStrategy::Prepend => &mut self.prepend_props,
            super::props::MergeStrategy::DeepMerge => &mut self.deep_merge_props,
        };
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }

    /// Record `<array prop path>.<identity field>`, the shape the client's
    /// lookup expects: it strips the last segment and compares the remainder
    /// against the array's prop path for exact equality.
    pub(crate) fn record_match_on(&mut self, key: &str) {
        let entry = key.to_string();
        if !self.match_props_on.contains(&entry) {
            self.match_props_on.push(entry);
        }
    }

    pub(crate) fn record_scroll(&mut self, prop: &str, scroll: ScrollProp) {
        self.scroll_props.insert(prop.to_string(), scroll);
    }

    pub(crate) fn record_once(&mut self, key: &str, prop: &str, expires_at: Option<u64>) {
        self.once_props.insert(
            key.to_string(),
            OncePropEntry {
                prop: prop.to_string(),
                expires_at,
            },
        );
    }

    pub(crate) fn record_deferred(&mut self, group: &str, path: &str) {
        self.deferred_props
            .get_or_insert_with(BTreeMap::new)
            .entry(group.to_string())
            .or_default()
            .push(path.to_string());
    }

    pub(crate) fn record_rescued(&mut self, path: &str) {
        self.rescued_props.push(path.to_string());
    }

    pub(crate) fn record_shared(&mut self, key: &str) {
        if !self.shared_props.iter().any(|existing| existing == key) {
            self.shared_props.push(key.to_string());
        }
    }
}

/// Page-level metadata selected by a handler.
#[derive(Debug, Default, Clone)]
pub struct PageOptions {
    pub(crate) encrypt_history: bool,
    pub(crate) clear_history: bool,
    pub(crate) preserve_fragment: bool,
    pub(crate) flash: Option<Value>,
    pub(crate) match_props_on: Vec<String>,
    pub(crate) status: Option<StatusCode>,
}

impl PageOptions {
    /// Create page options with every conditional field disabled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the official client to encrypt this page's history state.
    pub fn encrypt_history(mut self) -> Self {
        self.encrypt_history = true;
        self
    }

    /// Ask the official client to clear encrypted history state.
    pub fn clear_history(mut self) -> Self {
        self.clear_history = true;
        self
    }

    /// Preserve the originating request fragment.
    pub fn preserve_fragment(mut self) -> Self {
        self.preserve_fragment = true;
        self
    }

    /// Attach already-produced one-time flash data.
    pub fn flash(mut self, flash: Value) -> Self {
        self.flash = Some(flash);
        self
    }

    /// Identify array items by a field so a merge updates in place instead of
    /// appending a duplicate.
    ///
    /// Each entry is `<array prop path>.<identity field>` -- `posts.data.id`
    /// for `posts.data` matched on `id`. The per-prop
    /// [`match_on`](super::props::match_on) says the same thing without
    /// spelling the path out, and is the better spelling when the array is a
    /// prop of this page rather than a path reached through several.
    pub fn match_props_on<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.match_props_on.extend(keys.into_iter().map(Into::into));
        self
    }

    /// Render this page with a status other than `200`.
    ///
    /// A page object is a legitimate body for an error status: the official
    /// client keys "this is an Inertia response" on the `X-Inertia` header,
    /// not the status, and treats `>= 400` as an `httpException` event that
    /// still renders the page. This is how a 404 or a 419 stays inside the
    /// application's own layout instead of falling back to the browser's.
    pub fn status(mut self, status: StatusCode) -> Self {
        self.status = Some(status);
        self
    }

    /// The status this page renders with, defaulting to `200`.
    pub(crate) fn resolved_status(&self) -> StatusCode {
        self.status.unwrap_or(StatusCode::OK)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(metadata: PageMetadata) -> Value {
        serde_json::to_value(Page {
            component: "users/index".to_string(),
            props: Value::Object(serde_json::Map::new()),
            url: "/users".to_string(),
            version: None,
            metadata,
        })
        .expect("page serializes")
    }

    #[test]
    fn an_absent_asset_version_is_null_and_not_a_missing_key() {
        // The client types `version` as `string | null` and reads it
        // unconditionally when building `X-Inertia-Version`.
        let page = json(PageMetadata::default());
        assert!(page.as_object().expect("object").contains_key("version"));
        assert_eq!(page["version"], Value::Null);
    }

    #[test]
    fn a_page_with_no_metadata_carries_no_metadata_keys() {
        let page = json(PageMetadata::default());
        let keys: Vec<&String> = page.as_object().expect("object").keys().collect();
        assert_eq!(keys, ["component", "props", "url", "version"]);
    }

    #[test]
    fn an_exhausted_scroll_direction_serializes_as_null_not_zero() {
        // `hasNext = () => !!state.nextPage`. A `0` here would read as
        // exhausted on a paginator that starts at page zero, and an omitted
        // key would read as exhausted too -- but only `null` says so on
        // purpose.
        let mut metadata = PageMetadata::default();
        metadata.record_scroll("posts", ScrollProp::new("page").current(1_i64));
        let scroll = &json(metadata)["scrollProps"]["posts"];
        assert_eq!(scroll["currentPage"], Value::from(1));
        assert_eq!(scroll["nextPage"], Value::Null);
        assert_eq!(scroll["previousPage"], Value::Null);
        assert_eq!(scroll["pageName"], Value::from("page"));
        assert_eq!(scroll["reset"], Value::Bool(false));
    }

    #[test]
    fn a_cursor_scroll_identifier_stays_a_string() {
        let mut metadata = PageMetadata::default();
        metadata.record_scroll("posts", ScrollProp::new("cursor").next("eyJpZCI6MX0"));
        assert_eq!(
            json(metadata)["scrollProps"]["posts"]["nextPage"],
            Value::from("eyJpZCI6MX0")
        );
    }

    #[test]
    fn a_once_entry_without_a_ttl_omits_expires_at() {
        let mut metadata = PageMetadata::default();
        metadata.record_once("dashboard", "stats", None);
        let entry = &json(metadata)["onceProps"]["dashboard"];
        assert_eq!(entry["prop"], Value::from("stats"));
        assert!(!entry.as_object().expect("object").contains_key("expiresAt"));
    }

    #[test]
    fn a_once_entry_with_a_ttl_reports_a_millisecond_epoch() {
        let mut metadata = PageMetadata::default();
        metadata.record_once("dashboard", "stats", Some(1_700_000_000_000));
        assert_eq!(
            json(metadata)["onceProps"]["dashboard"]["expiresAt"],
            Value::from(1_700_000_000_000_u64)
        );
    }

    #[test]
    fn match_props_on_does_not_repeat_a_key() {
        let mut metadata = PageMetadata::default();
        metadata.record_match_on("posts.data.id");
        metadata.record_match_on("posts.data.id");
        assert_eq!(metadata.match_props_on, ["posts.data.id"]);
    }

    #[test]
    fn page_level_match_keys_reach_the_metadata() {
        let mut metadata = PageMetadata::default();
        metadata.apply_options(PageOptions::new().match_props_on(["posts.data.id", "tags.slug"]));
        assert_eq!(metadata.match_props_on, ["posts.data.id", "tags.slug"]);
    }

    #[test]
    fn the_default_page_status_is_ok() {
        assert_eq!(PageOptions::new().resolved_status(), StatusCode::OK);
        assert_eq!(
            PageOptions::new()
                .status(StatusCode::NOT_FOUND)
                .resolved_status(),
            StatusCode::NOT_FOUND
        );
    }
}
