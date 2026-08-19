//! Props: the prop model, builder, shared props, resolver, and resolution engine.
//!
//! A prop is one of: eager (a serialized value), always (always included),
//! lazy (resolved on full visit or when selected), optional (resolved only on
//! partial reload), or deferred (announced on full visit, resolved on
//! follow-up). Merge labels tell the client how to merge a prop.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};

use super::error::InertiaError;
use super::page::PageMetadata;
use super::request::{InertiaRequest, PartialSelection};

/// A boxed future returning a prop value or an error.
pub(crate) type PropFuture =
    Pin<Box<dyn Future<Output = Result<Value, Box<dyn std::error::Error + Send + Sync>>> + Send>>;

/// A boxed future for shared-optional resolvers (request-aware).
pub(crate) type SharedPropFuture =
    Pin<Box<dyn Future<Output = Result<Value, Box<dyn std::error::Error + Send + Sync>>> + Send>>;

/// A type-erased async resolver: `() -> PropFuture`.
pub(crate) type Resolver = Arc<dyn Fn() -> PropFuture + Send + Sync>;

/// A type-erased request-aware resolver: `&InertiaRequest -> SharedPropFuture`.
pub(crate) type SharedResolver = Arc<dyn Fn(&InertiaRequest) -> SharedPropFuture + Send + Sync>;

/// The merge strategy a prop carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStrategy {
    Merge,
    Prepend,
    DeepMerge,
}

/// A single prop with optional merge behavior.
#[derive(Clone)]
pub struct Prop {
    base: BaseProp,
    merge: Option<MergeStrategy>,
}

/// The base behavior of a prop.
#[derive(Clone)]
pub(crate) enum BaseProp {
    Eager(SerializedValue),
    Always(SerializedValue),
    Lazy(Resolver),
    Optional(Resolver),
    Deferred {
        resolver: Resolver,
        group: Option<Arc<str>>,
        rescue: bool,
    },
}

/// A pre-serialized value (avoids re-serializing on every response).
#[derive(Clone)]
pub(crate) struct SerializedValue {
    value: Value,
}

impl SerializedValue {
    pub(crate) fn new(value: Value) -> Self {
        SerializedValue { value }
    }

    pub(crate) fn resolve(&self) -> Result<Value, InertiaError> {
        Ok(self.value.clone())
    }
}

impl Prop {
    pub(crate) fn base(&self) -> &BaseProp {
        &self.base
    }

    pub(crate) fn merge(&self) -> Option<MergeStrategy> {
        self.merge
    }
}

/// The collection of props for a page.
#[derive(Clone, Default)]
pub struct Props {
    entries: Vec<(Arc<str>, Prop)>,
}

impl Props {
    /// An empty props collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a prop at a dotted key.
    pub fn with(mut self, key: impl Into<Arc<str>>, prop: Prop) -> Self {
        self.entries.push((key.into(), prop));
        self
    }

    /// Add eager `errors` prop (always included).
    pub fn errors(self, errors: impl Serialize) -> Self {
        let value = serde_json::to_value(&errors).unwrap_or(Value::Null);
        self.with(
            "errors",
            Prop {
                base: BaseProp::Always(SerializedValue::new(value)),
                merge: None,
            },
        )
    }

    /// Build from a serialized JSON object (flattens to dotted eager props).
    pub(crate) fn from_serialized(value: Value) -> Result<Self, InertiaError> {
        let map = match value {
            Value::Object(m) => m,
            _ => return Err(InertiaError::PropsMustBeObject),
        };
        let mut props = Props::new();
        for (key, val) in map {
            props = props.with(
                key,
                Prop {
                    base: BaseProp::Eager(SerializedValue::new(val)),
                    merge: None,
                },
            );
        }
        Ok(props)
    }

    pub(crate) fn into_entries(self) -> Vec<(Arc<str>, Prop)> {
        self.entries
    }
}

/// Shared props registered on `InertiaConfig`.
#[derive(Clone, Default)]
pub struct SharedProps {
    entries: Vec<(Arc<str>, SharedProp)>,
}

#[derive(Clone)]
pub(crate) enum SharedProp {
    Page(Prop),
    Optional(SharedResolver),
}

impl SharedProps {
    /// An empty shared-props collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an eager shared prop at a key.
    pub fn with(mut self, key: impl Into<Arc<str>>, value: impl Serialize) -> Self {
        let value = serde_json::to_value(&value).unwrap_or(Value::Null);
        self.entries.push((
            key.into(),
            SharedProp::Page(Prop {
                base: BaseProp::Eager(SerializedValue::new(value)),
                merge: None,
            }),
        ));
        self
    }

    /// Add an always-included shared prop.
    pub fn always(mut self, key: impl Into<Arc<str>>, value: impl Serialize) -> Self {
        let value = serde_json::to_value(&value).unwrap_or(Value::Null);
        self.entries.push((
            key.into(),
            SharedProp::Page(Prop {
                base: BaseProp::Always(SerializedValue::new(value)),
                merge: None,
            }),
        ));
        self
    }

    /// Add a request-aware optional shared prop (resolved only on partial).
    pub fn optional<F, Fut>(mut self, key: impl Into<Arc<str>>, resolver: F) -> Self
    where
        F: Fn(&InertiaRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, Box<dyn std::error::Error + Send + Sync>>>
            + Send
            + 'static,
    {
        let erased: SharedResolver = Arc::new(move |req| {
            let fut = resolver(req);
            Box::pin(fut) as SharedPropFuture
        });
        self.entries
            .push((key.into(), SharedProp::Optional(erased)));
        self
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Arc<str>, &SharedProp)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// --- Prop constructors -----------------------------------------------------

/// An eager prop: a serialized value included on full visits and when
/// selected by partial reload.
pub fn eager(value: impl Serialize) -> Prop {
    let value = serde_json::to_value(&value).unwrap_or(Value::Null);
    Prop {
        base: BaseProp::Eager(SerializedValue::new(value)),
        merge: None,
    }
}

/// An always-included prop (bypasses partial-reload filtering).
pub fn always(value: impl Serialize) -> Prop {
    let value = serde_json::to_value(&value).unwrap_or(Value::Null);
    Prop {
        base: BaseProp::Always(SerializedValue::new(value)),
        merge: None,
    }
}

/// A lazy prop: resolved on full visits and when selected.
pub fn lazy<F, Fut>(resolver: F) -> Prop
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
{
    let erased: Resolver = Arc::new(move || Box::pin(resolver()) as PropFuture);
    Prop {
        base: BaseProp::Lazy(erased),
        merge: None,
    }
}

/// An optional prop: resolved only when selected on a partial reload.
pub fn optional<F, Fut>(resolver: F) -> Prop
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
{
    let erased: Resolver = Arc::new(move || Box::pin(resolver()) as PropFuture);
    Prop {
        base: BaseProp::Optional(erased),
        merge: None,
    }
}

/// A deferred prop: announced (not resolved) on full visits; resolved on
/// follow-up partial reloads in the default group.
pub fn deferred<F, Fut>(resolver: F) -> Prop
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
{
    let erased: Resolver = Arc::new(move || Box::pin(resolver()) as PropFuture);
    Prop {
        base: BaseProp::Deferred {
            resolver: erased,
            group: None,
            rescue: false,
        },
        merge: None,
    }
}

/// A deferred prop in a named group.
pub fn deferred_group<F, Fut>(group: impl Into<Arc<str>>, resolver: F) -> Prop
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
{
    let erased: Resolver = Arc::new(move || Box::pin(resolver()) as PropFuture);
    Prop {
        base: BaseProp::Deferred {
            resolver: erased,
            group: Some(group.into()),
            rescue: false,
        },
        merge: None,
    }
}

/// Attach a merge label to a prop.
pub fn merge(prop: Prop) -> Prop {
    Prop {
        base: prop.base,
        merge: Some(MergeStrategy::Merge),
    }
}

/// Attach a prepend label to a prop.
pub fn prepend(prop: Prop) -> Prop {
    Prop {
        base: prop.base,
        merge: Some(MergeStrategy::Prepend),
    }
}

/// Attach a deep-merge label to a prop.
pub fn deep_merge(prop: Prop) -> Prop {
    Prop {
        base: prop.base,
        merge: Some(MergeStrategy::DeepMerge),
    }
}

// --- Resolution engine -----------------------------------------------------

/// The resolved page props and their metadata.
pub(crate) struct Resolved {
    pub props: Map<String, Value>,
    pub metadata: PageMetadata,
}

/// Resolve all props for a request.
pub(crate) async fn resolve(
    page: Props,
    shared: &SharedProps,
    request: &InertiaRequest,
    component: &str,
) -> Result<Resolved, InertiaError> {
    let partial = request.partial_for(component);
    let is_full = partial.is_none();
    let partial = partial.as_ref();
    let reset_paths: BTreeSet<&str> = match partial {
        Some(p) => p.reset.iter().map(|s| s.as_ref()).collect(),
        None => BTreeSet::new(),
    };

    let page_entries = page.into_entries();
    let page_keys: BTreeSet<&str> = page_entries.iter().map(|(k, _)| k.as_ref()).collect();

    let mut props: Map<String, Value> = Map::new();
    let mut metadata = PageMetadata::default();

    // Shared props first (lower precedence); page props win on collision.
    for (key, shared_prop) in shared.iter() {
        if page_keys.contains(key.as_ref()) {
            continue;
        }
        let top_level = top_level_key(key);
        let included = match shared_prop {
            SharedProp::Page(prop) => {
                resolve_page(
                    key,
                    prop,
                    is_full,
                    partial,
                    &reset_paths,
                    &mut props,
                    &mut metadata,
                )
                .await?
            }
            SharedProp::Optional(resolver) => {
                if is_full || !included(key, partial) {
                    false
                } else {
                    let value = (resolver)(request).await.map_err(|source| {
                        InertiaError::PropResolution {
                            path: Arc::from(key.as_ref()),
                            source,
                        }
                    })?;
                    insert_nested(&mut props, key, value);
                    true
                }
            }
        };
        if included {
            metadata.record_shared(top_level);
        }
    }

    // Page props overlay shared props (win on collision).
    for (key, prop) in &page_entries {
        resolve_page(
            key,
            prop,
            is_full,
            partial,
            &reset_paths,
            &mut props,
            &mut metadata,
        )
        .await?;
    }

    // `errors` defaults to `{}` when not provided.
    if !props.contains_key("errors") {
        props.insert("errors".to_string(), Value::Object(Map::new()));
    }

    Ok(Resolved { props, metadata })
}

fn top_level_key(key: &str) -> &str {
    key.split('.').next().unwrap_or(key)
}

fn insert_nested(props: &mut Map<String, Value>, dotted: &str, value: Value) {
    let mut segments = dotted.split('.');
    let first = match segments.next() {
        Some(s) => s,
        None => return,
    };
    let rest: Vec<&str> = segments.collect();
    if rest.is_empty() {
        props.insert(first.to_string(), value);
        return;
    }
    let entry = props
        .entry(first.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    insert_nested_into(entry, &rest, value);
}

fn insert_nested_into(current: &mut Value, segments: &[&str], value: Value) {
    if segments.is_empty() {
        *current = value;
        return;
    }
    let map = match current {
        Value::Object(m) => m,
        other => {
            *other = Value::Object(Map::new());
            let Value::Object(m) = other else {
                return;
            };
            m
        }
    };
    let key = segments[0];
    if segments.len() == 1 {
        map.insert(key.to_string(), value);
        return;
    }
    let entry = map
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    insert_nested_into(entry, &segments[1..], value);
}

fn included(path: &str, partial: Option<&PartialSelection>) -> bool {
    match partial {
        None => true,
        Some(p) => match_path_included(path, &p.only, &p.except),
    }
}

fn match_path_included(path: &str, only: &[Arc<str>], except: &[Arc<str>]) -> bool {
    // `only` = bidirectional dot-prefix match; `except` = descendant-or-equal.
    let selected = only.is_empty()
        || only.iter().any(|o| {
            path == o.as_ref()
                || path
                    .strip_prefix(o.as_ref())
                    .is_some_and(|r| r.starts_with('.'))
                || o.strip_prefix(path).is_some_and(|r| r.starts_with('.'))
        });
    selected
        && !except.iter().any(|e| {
            path == e.as_ref()
                || path
                    .strip_prefix(e.as_ref())
                    .is_some_and(|r| r.starts_with('.'))
        })
}

async fn resolve_page(
    key: &str,
    prop: &Prop,
    is_full: bool,
    partial: Option<&PartialSelection>,
    reset_paths: &BTreeSet<&str>,
    props: &mut Map<String, Value>,
    metadata: &mut PageMetadata,
) -> Result<bool, InertiaError> {
    let included = match prop.base() {
        BaseProp::Eager(value) => {
            if is_full || included(key, partial) {
                let value = filter_nested_value(key, value.resolve()?, partial);
                insert_nested(props, key, value);
                true
            } else {
                false
            }
        }
        BaseProp::Always(value) => {
            insert_nested(props, key, value.resolve()?);
            true
        }
        BaseProp::Lazy(resolver) => {
            if is_full || included(key, partial) {
                let value = (resolver)()
                    .await
                    .map_err(|source| InertiaError::PropResolution {
                        path: Arc::from(key),
                        source,
                    })?;
                insert_nested(props, key, value);
                true
            } else {
                false
            }
        }
        BaseProp::Optional(resolver) => {
            if is_full {
                false
            } else if included(key, partial) {
                let value = (resolver)()
                    .await
                    .map_err(|source| InertiaError::PropResolution {
                        path: Arc::from(key),
                        source,
                    })?;
                insert_nested(props, key, value);
                true
            } else {
                false
            }
        }
        BaseProp::Deferred {
            resolver,
            group,
            rescue,
        } => {
            if is_full {
                let group = group.as_deref().unwrap_or("default");
                metadata.record_deferred(group, key);
                false
            } else if included(key, partial) {
                match (resolver)().await {
                    Ok(value) => {
                        insert_nested(props, key, value);
                        true
                    }
                    Err(_) if *rescue => {
                        metadata.record_rescued(key);
                        false
                    }
                    Err(source) => {
                        return Err(InertiaError::PropResolution {
                            path: Arc::from(key),
                            source,
                        });
                    }
                }
            } else {
                false
            }
        }
    };

    if included && !reset_paths.contains(key) {
        if let Some(strategy) = prop.merge() {
            metadata.record_merge(strategy, key);
        }
    }

    Ok(included)
}

fn filter_nested_value(path: &str, value: Value, partial: Option<&PartialSelection>) -> Value {
    let Some(partial) = partial else {
        return value;
    };
    let Value::Object(map) = value else {
        return value;
    };
    let mut filtered = Map::new();
    for (key, value) in map {
        let child_path = format!("{path}.{key}");
        if included(&child_path, Some(partial)) {
            filtered.insert(key, filter_nested_value(&child_path, value, Some(partial)));
        }
    }
    Value::Object(filtered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_path_only_empty_includes_all() {
        assert!(match_path_included("users", &[], &[]));
    }

    #[test]
    fn match_path_only_exact() {
        let only = vec![Arc::from("users")];
        assert!(match_path_included("users", &only, &[]));
        assert!(!match_path_included("posts", &only, &[]));
    }

    #[test]
    fn match_path_only_descendant() {
        let only = vec![Arc::from("auth")];
        assert!(match_path_included("auth.user", &only, &[]));
    }

    #[test]
    fn match_path_except_removes() {
        let only = vec![Arc::from("auth")];
        let except = vec![Arc::from("auth.token")];
        assert!(!match_path_included("auth.token", &only, &except));
    }
}
