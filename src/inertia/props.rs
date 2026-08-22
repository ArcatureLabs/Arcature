//! Props: the prop model, builder, shared props, resolver, and resolution engine.
//!
//! A prop is one of: eager (a serialized value), always (always included),
//! lazy (resolved on full visit or when selected), optional (resolved only on
//! partial reload), or deferred (announced on full visit, resolved on
//! follow-up).
//!
//! On top of that base sit four independent labels, each of which is a
//! separate wrapper so they compose in any order:
//!
//! * a **merge strategy** ([`merge`], [`prepend`], [`deep_merge`]), optionally
//!   aimed at a nested path with [`merge_path`];
//! * an **identity field** ([`match_on`]) so a merge updates array items in
//!   place instead of appending duplicates;
//! * **infinite-scroll state** ([`infinite_scroll`]), which also lets the
//!   request's merge intent pick the direction;
//! * **once** ([`once`], [`once_with`]), which lets the client keep the value
//!   and the server stop sending it;
//! * **rescue** ([`rescue`]), which turns a resolver failure into an announced
//!   omission rather than a failed render.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value};

use super::error::InertiaError;
use super::page::{PageMetadata, ScrollProp};
use super::request::{InertiaRequest, MergeIntent, PartialSelection};

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

/// A single prop and the labels attached to it.
#[derive(Clone)]
pub struct Prop {
    base: BaseProp,
    merge: Option<MergeStrategy>,
    /// A path *inside* this prop that the merge labels point at, relative to
    /// the prop's own key. A paginator lives at `posts` while the array the
    /// client merges lives at `posts.data`.
    merge_path: Option<Arc<str>>,
    /// The field that identifies an array item, appended to the merge path to
    /// form one `matchPropsOn` entry.
    match_on: Option<Arc<str>>,
    scroll: Option<ScrollProp>,
    once: Option<OnceProp>,
    rescue: bool,
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
    },
}

/// How a once prop identifies itself and when the client should let it go.
///
/// A once prop is sent the first time and then withheld: the client stores it,
/// names its key in `X-Inertia-Except-Once-Props` on every later request, and
/// the server answers by emitting the `onceProps` entry with no value behind
/// it. That is the whole point -- an expensive prop that never changes gets
/// computed and transferred once per session rather than once per visit.
#[derive(Debug, Clone, Default)]
pub struct OnceProp {
    key: Option<Arc<str>>,
    ttl: Option<Duration>,
}

impl OnceProp {
    /// A once prop keyed by its own prop path and held indefinitely.
    pub fn new() -> Self {
        Self::default()
    }

    /// Use an explicit key instead of the prop path.
    ///
    /// Worth setting when the value's identity is narrower than its location:
    /// keying on `settings-v2` rather than `settings` means a deploy that
    /// changes the shape invalidates every client's copy, because the key it
    /// holds is no longer one the server names.
    pub fn key(mut self, key: impl Into<Arc<str>>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Stop the client reusing the stored value after `ttl`.
    ///
    /// The deadline travels as a millisecond epoch that the client compares
    /// against its own clock, so a client whose clock is badly wrong will
    /// expire early or late. It is a cache hint, not an access control.
    pub fn ttl(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    fn resolved_key<'a>(&'a self, path: &'a str) -> &'a str {
        self.key.as_deref().unwrap_or(path)
    }

    fn expires_at(&self, now_ms: u64) -> Option<u64> {
        self.ttl.map(|ttl| {
            let millis = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX);
            now_ms.saturating_add(millis)
        })
    }
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
    fn from_base(base: BaseProp) -> Prop {
        Prop {
            base,
            merge: None,
            merge_path: None,
            match_on: None,
            scroll: None,
            once: None,
            rescue: false,
        }
    }

    pub(crate) fn base(&self) -> &BaseProp {
        &self.base
    }

    /// The dotted path the merge labels apply to, given this prop's key.
    fn merge_key(&self, key: &str) -> String {
        match self.merge_path.as_deref() {
            Some(suffix) => format!("{key}.{suffix}"),
            None => key.to_string(),
        }
    }

    /// The strategy to record, after the request's scroll intent has had its
    /// say. Intent only speaks for props that carry scroll state; everywhere
    /// else the label the handler wrote is the label that ships.
    fn strategy_for(&self, intent: Option<MergeIntent>) -> Option<MergeStrategy> {
        match (self.scroll.is_some(), intent) {
            (true, Some(MergeIntent::Prepend)) => Some(MergeStrategy::Prepend),
            (true, _) => Some(self.merge.unwrap_or(MergeStrategy::Merge)),
            (false, _) => self.merge,
        }
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
            Prop::from_base(BaseProp::Always(SerializedValue::new(value))),
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
                Prop::from_base(BaseProp::Eager(SerializedValue::new(val))),
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
            SharedProp::Page(Prop::from_base(BaseProp::Eager(SerializedValue::new(
                value,
            )))),
        ));
        self
    }

    /// Add an always-included shared prop.
    pub fn always(mut self, key: impl Into<Arc<str>>, value: impl Serialize) -> Self {
        let value = serde_json::to_value(&value).unwrap_or(Value::Null);
        self.entries.push((
            key.into(),
            SharedProp::Page(Prop::from_base(BaseProp::Always(SerializedValue::new(
                value,
            )))),
        ));
        self
    }

    /// Add an arbitrary shared prop, with whatever labels it carries.
    ///
    /// The typed path for the cases [`with`](Self::with) and
    /// [`always`](Self::always) do not cover -- a shared `auth.user` that is
    /// [`once`], say, or a shared list that merges.
    pub fn prop(mut self, key: impl Into<Arc<str>>, prop: Prop) -> Self {
        self.entries.push((key.into(), SharedProp::Page(prop)));
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
    Prop::from_base(BaseProp::Eager(SerializedValue::new(value)))
}

/// An always-included prop (bypasses partial-reload filtering).
pub fn always(value: impl Serialize) -> Prop {
    let value = serde_json::to_value(&value).unwrap_or(Value::Null);
    Prop::from_base(BaseProp::Always(SerializedValue::new(value)))
}

/// A lazy prop: resolved on full visits and when selected.
pub fn lazy<F, Fut>(resolver: F) -> Prop
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
{
    let erased: Resolver = Arc::new(move || Box::pin(resolver()) as PropFuture);
    Prop::from_base(BaseProp::Lazy(erased))
}

/// An optional prop: resolved only when selected on a partial reload.
pub fn optional<F, Fut>(resolver: F) -> Prop
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
{
    let erased: Resolver = Arc::new(move || Box::pin(resolver()) as PropFuture);
    Prop::from_base(BaseProp::Optional(erased))
}

/// A deferred prop: announced (not resolved) on full visits; resolved on
/// follow-up partial reloads in the default group.
pub fn deferred<F, Fut>(resolver: F) -> Prop
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
{
    let erased: Resolver = Arc::new(move || Box::pin(resolver()) as PropFuture);
    Prop::from_base(BaseProp::Deferred {
        resolver: erased,
        group: None,
    })
}

/// A deferred prop in a named group.
pub fn deferred_group<F, Fut>(group: impl Into<Arc<str>>, resolver: F) -> Prop
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
{
    let erased: Resolver = Arc::new(move || Box::pin(resolver()) as PropFuture);
    Prop::from_base(BaseProp::Deferred {
        resolver: erased,
        group: Some(group.into()),
    })
}

/// Attach a merge label to a prop.
pub fn merge(mut prop: Prop) -> Prop {
    prop.merge = Some(MergeStrategy::Merge);
    prop
}

/// Attach a prepend label to a prop.
pub fn prepend(mut prop: Prop) -> Prop {
    prop.merge = Some(MergeStrategy::Prepend);
    prop
}

/// Attach a deep-merge label to a prop.
pub fn deep_merge(mut prop: Prop) -> Prop {
    prop.merge = Some(MergeStrategy::DeepMerge);
    prop
}

/// Point this prop's merge labels at a path *inside* it.
///
/// `path` is relative to the prop's own key, so a paginator at `posts` whose
/// records live at `posts.data` is `merge_path("data", merge(eager(page)))`.
/// Without this the client would merge the paginator objects themselves and
/// the record arrays would replace one another.
pub fn merge_path(path: impl Into<Arc<str>>, mut prop: Prop) -> Prop {
    prop.merge_path = Some(path.into());
    prop
}

/// Identify array items by `field` so a merge updates them in place.
///
/// Without it a merge concatenates, and a record the client already has
/// arrives a second time. The field is appended to the prop's merge path to
/// make the `matchPropsOn` entry the client looks up.
pub fn match_on(field: impl Into<Arc<str>>, mut prop: Prop) -> Prop {
    prop.match_on = Some(field.into());
    prop
}

/// Attach infinite-scroll state to a prop.
///
/// Two things follow. The state is published under the page object's
/// `scrollProps` keyed by this prop's name, which is where the client's
/// scroll component reads the loadable directions from. And the prop becomes
/// intent-aware: a request carrying `X-Inertia-Infinite-Scroll-Merge-Intent:
/// prepend` gets a prepend label whatever the handler asked for, because the
/// client is loading backwards and only it knows that.
pub fn infinite_scroll(scroll: ScrollProp, mut prop: Prop) -> Prop {
    prop.scroll = Some(scroll);
    prop
}

/// Send this prop once, then let the client keep it.
///
/// The key is the prop's own path. Use [`once_with`] for an explicit key or
/// an expiry.
pub fn once(prop: Prop) -> Prop {
    once_with(OnceProp::new(), prop)
}

/// Send this prop once, with an explicit key or expiry.
pub fn once_with(spec: OnceProp, mut prop: Prop) -> Prop {
    prop.once = Some(spec);
    prop
}

/// Announce a resolver failure instead of failing the render.
///
/// The prop is left out of `props` and its path is listed under
/// `rescuedProps`, which the client carries forward so a component can show
/// the gap rather than the page dying around it. Only meaningful on a prop
/// with a resolver ([`lazy`], [`optional`], [`deferred`], [`deferred_group`]);
/// an eager value has nothing that can fail.
///
/// It swallows the error, so it belongs on props whose absence is survivable
/// -- a sidebar of suggestions, a usage chart -- and not on the ones the page
/// is about.
pub fn rescue(mut prop: Prop) -> Prop {
    prop.rescue = true;
    prop
}

// --- Resolution engine -----------------------------------------------------

/// The resolved page props and their metadata.
#[derive(Debug)]
pub(crate) struct Resolved {
    pub props: Map<String, Value>,
    pub metadata: PageMetadata,
}

/// Everything about the request that prop resolution needs, gathered once.
///
/// Passing it as one borrow keeps [`plan_page`] to four arguments and means a
/// new protocol input is a field here rather than another parameter threaded
/// through every call site.
struct ResolveContext<'a> {
    is_full: bool,
    partial: Option<&'a PartialSelection>,
    reset_paths: BTreeSet<&'a str>,
    intent: Option<MergeIntent>,
    now_ms: u64,
}

impl ResolveContext<'_> {
    fn included(&self, path: &str) -> bool {
        included(path, self.partial)
    }
}

/// The current wall-clock time in milliseconds since the Unix epoch.
///
/// Only ever used to stamp a once prop's expiry, which the client compares
/// against its own clock. A clock before the epoch is not a reason to refuse
/// to render, so it reports zero and every deadline is simply in the past.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Resolve all props for a request.
pub(crate) async fn resolve(
    page: Props,
    shared: &SharedProps,
    request: &InertiaRequest,
    component: &str,
) -> Result<Resolved, InertiaError> {
    let partial = request.partial_for(component);
    let context = ResolveContext {
        is_full: partial.is_none(),
        reset_paths: match partial.as_ref() {
            Some(p) => p.reset.iter().map(|s| s.as_ref()).collect(),
            None => BTreeSet::new(),
        },
        partial: partial.as_ref(),
        intent: request.merge_intent(),
        now_ms: now_millis(),
    };

    let page_entries = page.into_entries();
    let page_keys: BTreeSet<&str> = page_entries.iter().map(|(k, _)| k.as_ref()).collect();

    // Decide first, resolve second, record third.
    //
    // The three used to be one pass: a prop was inspected, awaited and
    // recorded before the next one was looked at. Nothing about the props
    // required that -- it was the shape of the loop. Deciding every entry up
    // front leaves resolvers that are independent of one another by
    // construction, and a recording pass that runs in the declaration order
    // whatever order they finish in.
    let mut plans: Vec<Planned<'_>> = Vec::new();
    let mut resolvers: Vec<PropFuture> = Vec::new();

    // Shared props first (lower precedence); page props win on collision.
    for (key, shared_prop) in shared.iter() {
        if page_keys.contains(key.as_ref()) {
            continue;
        }
        let mut planned = match shared_prop {
            SharedProp::Page(prop) => plan_page(key, prop, request, &context, &mut resolvers),
            SharedProp::Optional(resolver) => Planned {
                key: key.as_ref(),
                prop: None,
                shared: false,
                once_key: None,
                withheld: false,
                action: if context.is_full || !context.included(key) {
                    Action::Nothing
                } else {
                    Action::Invoke {
                        at: park(&mut resolvers, (resolver)(request)),
                        rescue: false,
                    }
                },
            },
        };
        // Both shapes came from the shared set, and that is what decides
        // whether the key is announced as shared once it ships.
        planned.shared = true;
        plans.push(planned);
    }

    // Page props overlay shared props (win on collision).
    for (key, prop) in &page_entries {
        plans.push(plan_page(key, prop, request, &context, &mut resolvers));
    }

    let mut props: Map<String, Value> = Map::new();
    let mut metadata = PageMetadata::default();

    for plan in plans {
        let outcome = match plan.action {
            Action::Invoke { at, .. } => Some(resolvers[at].as_mut().await),
            _ => None,
        };
        apply(plan, outcome, &context, &mut props, &mut metadata)?;
    }

    // `errors` defaults to `{}` when not provided.
    if !props.contains_key("errors") {
        props.insert("errors".to_string(), Value::Object(Map::new()));
    }

    // The client reads `errors[bag] || {}` when a bag was requested, so an
    // un-nested map would be silently empty on the page that asked for it.
    // The empty default is nested too: `{}` and `{"bag": {}}` read the same,
    // and one rule is easier to trust than two.
    if let Some(bag) = request.error_bag() {
        let errors = props
            .remove("errors")
            .unwrap_or_else(|| Value::Object(Map::new()));
        let mut scoped = Map::new();
        scoped.insert(bag.to_string(), errors);
        props.insert("errors".to_string(), Value::Object(scoped));
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

/// What a resolver hands back once it finishes.
type ResolverOutcome = Result<Value, Box<dyn std::error::Error + Send + Sync>>;

/// One entry's decision, taken before any resolver is polled.
///
/// The point of writing the decision down instead of acting on it is that a
/// decision cannot observe another prop's result: the whole set is settled
/// while `props` and `metadata` are still empty. That is what makes the
/// resolvers safe to run in any order, and it is also what keeps the recorded
/// output in declaration order regardless of which one finished first.
struct Planned<'a> {
    key: &'a str,
    /// The prop the trailing labels come from -- once, scroll, merge, match.
    /// `None` for a shared optional, which carries none of them.
    prop: Option<&'a Prop>,
    /// Whether this key is announced as shared when it ships.
    shared: bool,
    once_key: Option<String>,
    withheld: bool,
    action: Action<'a>,
}

/// What a planned entry does once its resolver, if it has one, has finished.
enum Action<'a> {
    /// Nothing ships for this key.
    Nothing,
    /// A value already in hand: eager, always, or an eager one filtered down
    /// to the paths a partial reload asked for.
    Ready(Value),
    /// Serialising an eager or always value failed.
    ///
    /// Carried rather than raised on the spot so the failure still surfaces
    /// at this entry's turn. Raising it during planning would abandon props
    /// declared before it that the old sequential pass had already run.
    Failed(InertiaError),
    /// The resolver parked at this index, and the prop's rescue label.
    Invoke { at: usize, rescue: bool },
    /// A deferred prop announced on a full visit rather than resolved.
    Announce { group: &'a str },
}

/// Park a resolver's future and return the slot it landed in.
///
/// Building the future is not running it: an `async` body does nothing until
/// it is polled, so planning stays free of the work it is planning.
fn park(resolvers: &mut Vec<PropFuture>, future: PropFuture) -> usize {
    resolvers.push(future);
    resolvers.len() - 1
}

fn plan_page<'a>(
    key: &'a str,
    prop: &'a Prop,
    request: &InertiaRequest,
    context: &ResolveContext<'_>,
    resolvers: &mut Vec<PropFuture>,
) -> Planned<'a> {
    // A once prop the client already holds: the value is withheld, but the
    // entry still ships. The client fills the gap from its own copy, and it
    // only knows to do that for keys the response names.
    let once_key = prop
        .once
        .as_ref()
        .map(|spec| spec.resolved_key(key).to_string());
    let withheld = once_key
        .as_deref()
        .is_some_and(|held| request.holds_once(held));

    let ready = |result: Result<Value, InertiaError>| match result {
        Ok(value) => Action::Ready(value),
        Err(error) => Action::Failed(error),
    };

    let action = if withheld {
        Action::Nothing
    } else {
        match prop.base() {
            BaseProp::Eager(value) => {
                if context.is_full || context.included(key) {
                    ready(
                        value
                            .resolve()
                            .map(|value| filter_nested_value(key, value, context.partial)),
                    )
                } else {
                    Action::Nothing
                }
            }
            BaseProp::Always(value) => ready(value.resolve()),
            BaseProp::Lazy(resolver) => {
                if context.is_full || context.included(key) {
                    Action::Invoke {
                        at: park(resolvers, (resolver)()),
                        rescue: prop.rescue,
                    }
                } else {
                    Action::Nothing
                }
            }
            BaseProp::Optional(resolver) => {
                if !context.is_full && context.included(key) {
                    Action::Invoke {
                        at: park(resolvers, (resolver)()),
                        rescue: prop.rescue,
                    }
                } else {
                    Action::Nothing
                }
            }
            BaseProp::Deferred { resolver, group } => {
                if context.is_full {
                    Action::Announce {
                        group: group.as_deref().unwrap_or("default"),
                    }
                } else if context.included(key) {
                    Action::Invoke {
                        at: park(resolvers, (resolver)()),
                        rescue: prop.rescue,
                    }
                } else {
                    Action::Nothing
                }
            }
        }
    };

    Planned {
        key,
        prop: Some(prop),
        shared: false,
        once_key,
        withheld,
        action,
    }
}

/// Carry out one planned entry and record its labels.
///
/// `outcome` is the resolver's result for an [`Action::Invoke`], and `None`
/// for every other action.
fn apply(
    plan: Planned<'_>,
    outcome: Option<ResolverOutcome>,
    context: &ResolveContext<'_>,
    props: &mut Map<String, Value>,
    metadata: &mut PageMetadata,
) -> Result<(), InertiaError> {
    let key = plan.key;

    let included = match plan.action {
        Action::Nothing => false,
        Action::Ready(value) => {
            insert_nested(props, key, value);
            true
        }
        Action::Failed(error) => return Err(error),
        Action::Announce { group } => {
            metadata.record_deferred(group, key);
            false
        }
        Action::Invoke { rescue, .. } => {
            match outcome.expect("a planned resolver is awaited before it is applied") {
                Ok(value) => {
                    insert_nested(props, key, value);
                    true
                }
                Err(_) if rescue => {
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
        }
    };

    // The entry ships whenever the prop was in play -- sent this time, or
    // withheld because it was sent last time. A prop that a partial reload
    // simply did not select is not in play, and the client keeps both the
    // value and its own entry for it.
    if let Some(prop) = plan.prop
        && let (Some(spec), Some(once_key)) = (prop.once.as_ref(), plan.once_key.as_deref())
        && (included || plan.withheld)
    {
        metadata.record_once(once_key, key, spec.expires_at(context.now_ms));
    }

    if included {
        if let Some(prop) = plan.prop {
            if let Some(scroll) = prop.scroll.as_ref() {
                metadata.record_scroll(key, scroll.clone());
            }
            if !context.reset_paths.contains(key) {
                let merge_key = prop.merge_key(key);
                if let Some(strategy) = prop.strategy_for(context.intent) {
                    metadata.record_merge(strategy, &merge_key);
                }
                if let Some(field) = prop.match_on.as_deref() {
                    metadata.record_match_on(&format!("{merge_key}.{field}"));
                }
            }
        }
        if plan.shared {
            metadata.record_shared(top_level_key(key));
        }
    }

    Ok(())
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
    use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Uri};

    fn request(pairs: &[(&'static str, &str)]) -> InertiaRequest {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                HeaderName::from_static(name),
                HeaderValue::from_str(value).expect("header value"),
            );
        }
        InertiaRequest::parse(&headers, &Method::GET, &Uri::from_static("/posts"))
    }

    fn partial(component: &str, only: &str) -> InertiaRequest {
        request(&[
            ("x-inertia", "true"),
            ("x-inertia-partial-component", component),
            ("x-inertia-partial-data", only),
        ])
    }

    async fn run(props: Props, request: &InertiaRequest) -> Resolved {
        resolve(props, &SharedProps::new(), request, "posts/index")
            .await
            .expect("resolution succeeds")
    }

    fn metadata_json(resolved: &Resolved) -> Value {
        serde_json::to_value(&resolved.metadata).expect("metadata serializes")
    }

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

    #[tokio::test]
    async fn a_once_prop_is_sent_and_announced_on_the_first_visit() {
        let resolved = run(
            Props::new().with(
                "settings",
                once(eager(serde_json::json!({ "theme": "dark" }))),
            ),
            &request(&[]),
        )
        .await;
        assert_eq!(resolved.props["settings"]["theme"], Value::from("dark"));
        assert_eq!(
            metadata_json(&resolved)["onceProps"]["settings"]["prop"],
            Value::from("settings")
        );
    }

    #[tokio::test]
    async fn a_once_prop_the_client_holds_is_withheld_but_still_named() {
        // Omitting the entry as well would be the easy mistake: the client
        // only copies its stored value forward for keys the response names,
        // so the prop would vanish from the page instead of persisting.
        let resolved = run(
            Props::new().with(
                "settings",
                once(eager(serde_json::json!({ "theme": "dark" }))),
            ),
            &request(&[("x-inertia-except-once-props", "settings")]),
        )
        .await;
        assert!(!resolved.props.contains_key("settings"));
        assert_eq!(
            metadata_json(&resolved)["onceProps"]["settings"]["prop"],
            Value::from("settings")
        );
    }

    #[tokio::test]
    async fn a_once_prop_can_be_keyed_independently_of_its_path() {
        let spec = OnceProp::new().key("settings-v2");
        let resolved = run(
            Props::new().with("settings", once_with(spec, eager(1))),
            &request(&[("x-inertia-except-once-props", "settings")]),
        )
        .await;
        // The client holds `settings`, but this response is keyed
        // `settings-v2` -- a different key, so the value ships again.
        assert_eq!(resolved.props["settings"], Value::from(1));
        assert_eq!(
            metadata_json(&resolved)["onceProps"]["settings-v2"]["prop"],
            Value::from("settings")
        );
    }

    #[tokio::test]
    async fn a_once_ttl_becomes_a_deadline_ahead_of_now() {
        let spec = OnceProp::new().ttl(Duration::from_secs(60));
        let resolved = run(
            Props::new().with("settings", once_with(spec, eager(1))),
            &request(&[]),
        )
        .await;
        let expires = metadata_json(&resolved)["onceProps"]["settings"]["expiresAt"]
            .as_u64()
            .expect("a millisecond epoch");
        let now = now_millis();
        assert!(expires > now, "{expires} should be after {now}");
        assert!(expires <= now + 60_000);
    }

    #[tokio::test]
    async fn a_once_prop_a_partial_reload_skips_is_not_announced() {
        let resolved = run(
            Props::new()
                .with("settings", once(eager(1)))
                .with("posts", eager(2)),
            &partial("posts/index", "posts"),
        )
        .await;
        assert!(!resolved.props.contains_key("settings"));
        assert!(
            metadata_json(&resolved).get("onceProps").is_none(),
            "the prop was never in play, so there is nothing to say about it"
        );
    }

    #[tokio::test]
    async fn a_scroll_prop_appends_by_default_and_prepends_on_intent() {
        let build = || {
            Props::new().with(
                "posts",
                merge_path(
                    "data",
                    infinite_scroll(ScrollProp::new("page").current(2_i64).next(3_i64), eager(1)),
                ),
            )
        };

        let appended = run(build(), &request(&[])).await;
        let json = metadata_json(&appended);
        assert_eq!(json["mergeProps"], serde_json::json!(["posts.data"]));
        assert_eq!(json["scrollProps"]["posts"]["nextPage"], Value::from(3));

        let prepended = run(
            build(),
            &request(&[("x-inertia-infinite-scroll-merge-intent", "prepend")]),
        )
        .await;
        let json = metadata_json(&prepended);
        assert_eq!(json["prependProps"], serde_json::json!(["posts.data"]));
        assert!(json.get("mergeProps").is_none());
    }

    #[tokio::test]
    async fn merge_intent_does_not_speak_for_props_without_scroll_state() {
        // Intent describes one scroll's direction, not the response. A plain
        // merged prop that happened to ride along must keep its own label.
        let resolved = run(
            Props::new().with("notifications", merge(eager(1))),
            &request(&[("x-inertia-infinite-scroll-merge-intent", "prepend")]),
        )
        .await;
        let json = metadata_json(&resolved);
        assert_eq!(json["mergeProps"], serde_json::json!(["notifications"]));
        assert!(json.get("prependProps").is_none());
    }

    #[tokio::test]
    async fn match_on_names_the_array_path_and_the_identity_field() {
        // The client strips the last segment and compares the remainder
        // against the array's prop path, so the two have to line up exactly.
        let resolved = run(
            Props::new().with("posts", match_on("id", merge_path("data", merge(eager(1))))),
            &request(&[]),
        )
        .await;
        let json = metadata_json(&resolved);
        assert_eq!(json["mergeProps"], serde_json::json!(["posts.data"]));
        assert_eq!(json["matchPropsOn"], serde_json::json!(["posts.data.id"]));
    }

    #[tokio::test]
    async fn a_reset_path_drops_the_merge_label_but_keeps_the_value() {
        let resolved = resolve(
            Props::new().with("posts", merge(eager(vec![1, 2]))),
            &SharedProps::new(),
            &request(&[
                ("x-inertia", "true"),
                ("x-inertia-partial-component", "posts/index"),
                ("x-inertia-partial-data", "posts"),
                ("x-inertia-reset", "posts"),
            ]),
            "posts/index",
        )
        .await
        .expect("resolution succeeds");
        assert_eq!(resolved.props["posts"], serde_json::json!([1, 2]));
        assert!(metadata_json(&resolved).get("mergeProps").is_none());
    }

    #[tokio::test]
    async fn a_rescued_resolver_failure_is_announced_instead_of_fatal() {
        let resolved = run(
            Props::new().with(
                "suggestions",
                rescue(lazy(|| async { Err("upstream is down".into()) })),
            ),
            &request(&[]),
        )
        .await;
        assert!(!resolved.props.contains_key("suggestions"));
        assert_eq!(
            metadata_json(&resolved)["rescuedProps"],
            serde_json::json!(["suggestions"])
        );
    }

    #[tokio::test]
    async fn an_unrescued_resolver_failure_fails_the_render() {
        let error = resolve(
            Props::new().with("posts", lazy(|| async { Err("upstream is down".into()) })),
            &SharedProps::new(),
            &request(&[]),
            "posts/index",
        )
        .await
        .expect_err("the render must not silently lose a prop");
        assert!(matches!(error, InertiaError::PropResolution { .. }));
    }

    #[tokio::test]
    async fn a_deferred_prop_is_announced_first_and_resolved_on_follow_up() {
        let announced = run(
            Props::new().with(
                "stats",
                deferred_group("charts", || async { Ok(Value::from(7)) }),
            ),
            &request(&[]),
        )
        .await;
        assert!(!announced.props.contains_key("stats"));
        assert_eq!(
            metadata_json(&announced)["deferredProps"]["charts"],
            serde_json::json!(["stats"])
        );

        let delivered = run(
            Props::new().with(
                "stats",
                deferred_group("charts", || async { Ok(Value::from(7)) }),
            ),
            &partial("posts/index", "stats"),
        )
        .await;
        assert_eq!(delivered.props["stats"], Value::from(7));
    }

    #[tokio::test]
    async fn errors_are_scoped_to_the_requested_bag() {
        // `getScopedErrors` reads `errors[bag] || {}`, so a flat map is an
        // empty map to the form that asked for a bag.
        let resolved = run(
            Props::new().errors(serde_json::json!({ "email": "is required" })),
            &request(&[("x-inertia-error-bag", "createUser")]),
        )
        .await;
        assert_eq!(
            resolved.props["errors"],
            serde_json::json!({ "createUser": { "email": "is required" } })
        );
    }

    #[tokio::test]
    async fn an_empty_error_bag_is_nested_too() {
        let resolved = run(Props::new(), &request(&[("x-inertia-error-bag", "login")])).await;
        assert_eq!(resolved.props["errors"], serde_json::json!({ "login": {} }));
    }

    #[tokio::test]
    async fn errors_stay_flat_without_a_bag() {
        let resolved = run(Props::new(), &request(&[])).await;
        assert_eq!(resolved.props["errors"], serde_json::json!({}));
    }
}
