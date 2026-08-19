//! The Inertia page object wire type, component name, and page options.

use std::sync::Arc;

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

/// The Inertia page object, ready to serialize to JSON.
#[derive(Debug, Serialize)]
pub(crate) struct Page {
    pub component: String,
    pub props: Value,
    pub url: String,
    pub version: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred_props: Option<std::collections::BTreeMap<String, Vec<String>>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rescued_props: Vec<String>,
}

impl PageMetadata {
    pub(crate) fn apply_options(&mut self, options: PageOptions) {
        self.encrypt_history = options.encrypt_history;
        self.clear_history = options.clear_history;
        self.preserve_fragment = options.preserve_fragment;
        self.flash = options.flash;
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

    pub(crate) fn record_deferred(&mut self, group: &str, path: &str) {
        self.deferred_props
            .get_or_insert_with(std::collections::BTreeMap::new)
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
}
