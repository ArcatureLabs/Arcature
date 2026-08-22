//! Translation catalogs: one [`Catalog`] per locale, and the [`Catalogs`]
//! registry that holds them and names the default.
//!
//! # The registry is also the whitelist
//!
//! [`Catalogs`] is the only answer in the framework to "is this a locale this
//! application has?". Locale negotiation matches against it and nothing else,
//! so a locale that is not registered cannot be selected, cannot be looked up,
//! and cannot reach a formatter. The set is fixed when the registry is built,
//! at startup, from values the application's own code supplied.
//!
//! # Fallback
//!
//! Two levels, and they are separate on purpose.
//!
//! *Locale* fallback happens before a catalog is chosen and belongs to
//! negotiation: an unregistered locale becomes the default one.
//!
//! *Message* fallback happens inside [`Catalogs::translate`]: a key the
//! chosen catalog does not have is looked up in the default catalog before
//! the call fails. That is what makes a partially translated locale usable --
//! a new string ships in `en`, and a `fr` page shows the English sentence
//! rather than a `500` or an empty span. It is per key, so it does not hide a
//! missing catalog; a locale with no catalog at all was never selectable.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};

use super::args::{ArgValue, TranslationArgs};
use super::error::I18nError;
use super::locale::LocaleId;

/// The messages of one locale, parsed and ready to format.
///
/// Built from `.ftl` source the application supplies -- in practice
/// `include_str!` over a file in the repository. A catalog may be assembled
/// from several sources with [`with_source`](Catalog::with_source), which is
/// how a large application splits its messages by area without splitting the
/// locale.
///
/// ```
/// use arcature::i18n::{Catalog, LocaleId};
///
/// let catalog = Catalog::parse(
///     LocaleId::parse("en").unwrap(),
///     "app-name = Arcature\nfarewell = Goodbye",
/// )
/// .unwrap();
///
/// assert_eq!(catalog.message("app-name").unwrap(), "Arcature");
/// assert!(catalog.has("farewell"));
/// assert!(!catalog.has("greeting"));
/// ```
#[non_exhaustive]
pub struct Catalog {
    locale: LocaleId,
    /// The concurrent bundle, not the single-threaded one: a catalog is built
    /// once at startup and then formatted from every request at once, so it
    /// has to be `Sync`. `fluent_bundle::FluentBundle` memoizes its
    /// plural-rule machinery behind a `RefCell` and is not.
    bundle: FluentBundle<FluentResource>,
}

impl Catalog {
    /// Parse `.ftl` source into a catalog for `locale`.
    ///
    /// # Errors
    ///
    /// [`I18nError::Parse`] if the source is not valid Fluent. The source is
    /// a file in the repository, so this is a developer error: it fails the
    /// same way on every machine, on the first call after a deploy.
    pub fn parse(locale: LocaleId, ftl: &str) -> Result<Self, I18nError> {
        let mut bundle = FluentBundle::new_concurrent(vec![locale.to_langid()]);
        // Fluent's default. Placeables come out wrapped in U+2068/U+2069, the
        // Unicode isolation marks, so an Arabic name interpolated into an
        // English sentence does not drag the punctuation around it to the
        // other side of the line. They are invisible in a browser and visible
        // in a byte-for-byte assertion, which is the only reason
        // `isolating(false)` exists.
        bundle.set_use_isolating(true);

        Self { locale, bundle }.add(ftl)
    }

    /// Add another `.ftl` source to this catalog.
    ///
    /// # Errors
    ///
    /// [`I18nError::Parse`] if the source does not parse, or if it defines a
    /// message or term the catalog already has. A silent overwrite would make
    /// the meaning of a key depend on file order.
    pub fn with_source(self, ftl: &str) -> Result<Self, I18nError> {
        self.add(ftl)
    }

    /// Turn Unicode bidi isolation of placeables on or off.
    ///
    /// On by default, and it should stay on in anything a person reads: it is
    /// what keeps a right-to-left value from reordering the left-to-right
    /// sentence around it. Turn it off for a test that compares formatted
    /// output byte for byte, or for a non-display sink such as a log line.
    #[must_use]
    pub fn isolating(mut self, isolating: bool) -> Self {
        self.bundle.set_use_isolating(isolating);
        self
    }

    /// The locale this catalog is for.
    #[must_use]
    pub fn locale(&self) -> &LocaleId {
        &self.locale
    }

    /// Whether this catalog defines a message under `key`.
    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.bundle
            .get_message(key)
            .is_some_and(|message| message.value().is_some())
    }

    /// Format a message that takes no arguments.
    ///
    /// # Errors
    ///
    /// [`I18nError::Missing`] if the catalog has no such message,
    /// [`I18nError::Format`] if it has one that needs arguments.
    pub fn message(&self, key: &str) -> Result<String, I18nError> {
        self.translate(key, &TranslationArgs::new())
    }

    /// Format a message with arguments.
    ///
    /// This is where Fluent earns its place: `args` reach the message's own
    /// selectors, so the catalog picks the plural form, the gender agreement
    /// and the number format for its locale, in the catalog, without the
    /// calling code knowing how many plural categories the language has.
    ///
    /// ```
    /// use arcature::i18n::{Catalog, LocaleId, TranslationArgs};
    ///
    /// // Two plural categories in English; Polish has four, and the
    /// // difference lives in the catalog rather than in this function.
    /// let catalog = Catalog::parse(
    ///     LocaleId::parse("en").unwrap(),
    ///     r#"
    /// unread = { $count ->
    ///     [one] One unread message
    ///    *[other] { $count } unread messages
    /// }
    /// "#,
    /// )
    /// .unwrap()
    /// .isolating(false);
    ///
    /// let one = TranslationArgs::new().with("count", 1);
    /// let many = TranslationArgs::new().with("count", 7);
    ///
    /// assert_eq!(catalog.translate("unread", &one).unwrap(), "One unread message");
    /// assert_eq!(catalog.translate("unread", &many).unwrap(), "7 unread messages");
    /// ```
    ///
    /// # Errors
    ///
    /// [`I18nError::Missing`] if the catalog has no message under `key`,
    /// [`I18nError::Format`] if formatting reported a problem -- an argument
    /// the message needed and did not get, a term it references that is not
    /// defined.
    pub fn translate(&self, key: &str, args: &TranslationArgs) -> Result<String, I18nError> {
        let message = self
            .bundle
            .get_message(key)
            .ok_or_else(|| self.missing(key))?;
        let pattern = message.value().ok_or_else(|| self.missing(key))?;

        let fluent_args = to_fluent_args(args);
        let mut errors = Vec::new();
        let formatted = self
            .bundle
            .format_pattern(pattern, Some(&fluent_args), &mut errors);
        self.finish(key, &formatted, &errors)
    }

    /// Format a message *attribute* -- the `.placeholder` in
    /// `search-field = Search\n    .placeholder = Search the archive`.
    ///
    /// Attributes are how Fluent keeps the strings of one UI element together
    /// so a translator sees them as a unit, and they are addressed
    /// separately because each one lands somewhere different in the markup.
    ///
    /// # Errors
    ///
    /// [`I18nError::Missing`] if the message or the attribute is absent,
    /// [`I18nError::Format`] as [`translate`](Self::translate).
    pub fn attribute(
        &self,
        key: &str,
        attribute: &str,
        args: &TranslationArgs,
    ) -> Result<String, I18nError> {
        let path = format!("{key}.{attribute}");
        let message = self
            .bundle
            .get_message(key)
            .ok_or_else(|| self.missing(&path))?;
        let pattern = message
            .get_attribute(attribute)
            .ok_or_else(|| self.missing(&path))?
            .value();

        let fluent_args = to_fluent_args(args);
        let mut errors = Vec::new();
        let formatted = self
            .bundle
            .format_pattern(pattern, Some(&fluent_args), &mut errors);
        self.finish(&path, &formatted, &errors)
    }

    /// Parse one source and fold it in, refusing an overwrite.
    fn add(mut self, ftl: &str) -> Result<Self, I18nError> {
        let resource =
            FluentResource::try_new(ftl.to_owned()).map_err(|(_, errors)| I18nError::Parse {
                locale: self.locale.clone(),
                errors: errors.iter().map(ToString::to_string).collect(),
            })?;

        self.bundle
            .add_resource(resource)
            .map_err(|errors| I18nError::Parse {
                locale: self.locale.clone(),
                errors: errors.iter().map(ToString::to_string).collect(),
            })?;

        Ok(self)
    }

    /// Decide what a completed `format_pattern` call meant.
    ///
    /// Split out rather than shared through a `pattern` argument because the
    /// pattern type is `fluent_syntax::ast::Pattern`, and `fluent-bundle`
    /// does not re-export it: naming it in a signature would mean adding
    /// `fluent-syntax` as a direct dependency to write one parameter type.
    fn finish(
        &self,
        key: &str,
        formatted: &str,
        errors: &[fluent_bundle::FluentError],
    ) -> Result<String, I18nError> {
        if errors.is_empty() {
            Ok(formatted.to_owned())
        } else {
            // The partially formatted string is deliberately dropped rather
            // than returned alongside the diagnostics. Fluent's recovery for
            // an unresolved placeable is to emit the placeable's own source
            // text, so what it hands back on the error path is a string with
            // `{ $count }` in it -- and a caller with a `String` in hand will
            // put it on a page.
            Err(I18nError::Format {
                locale: self.locale.clone(),
                key: key.to_owned(),
                errors: errors.iter().map(ToString::to_string).collect(),
            })
        }
    }

    fn missing(&self, key: &str) -> I18nError {
        I18nError::Missing {
            locale: self.locale.clone(),
            key: key.to_owned(),
        }
    }
}

impl fmt::Debug for Catalog {
    /// `FluentBundle` is not `Debug`, and dumping every message of every
    /// locale into a log line would be unhelpful even if it were.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Catalog")
            .field("locale", &self.locale)
            .finish_non_exhaustive()
    }
}

/// Convert to Fluent's borrowed argument type at the last possible moment.
///
/// Kept as a private function rather than a `From` impl: an
/// `impl From<&ArgValue> for FluentValue` would be a public impl naming a
/// `fluent-bundle` type, which is exactly the API-surface commitment
/// `src/i18n/args.rs` explains this crate is not making.
fn to_fluent_args(args: &TranslationArgs) -> FluentArgs<'_> {
    let mut fluent = FluentArgs::new();
    for (name, value) in args.iter() {
        let value = match value {
            ArgValue::Text(text) => FluentValue::from(text.as_str()),
            ArgValue::Integer(number) => FluentValue::from(*number),
            ArgValue::Number(number) => FluentValue::from(*number),
        };
        fluent.set(name, value);
    }
    fluent
}

/// Every catalog the application has, and which one is the default.
///
/// Cheap to clone -- the catalogs sit behind an `Arc` -- so a copy per
/// request costs a refcount bump. Build it once at startup.
///
/// ```
/// use arcature::i18n::{Catalog, Catalogs, LocaleId};
///
/// let en = LocaleId::parse("en").unwrap();
/// let fr = LocaleId::parse("fr").unwrap();
///
/// let catalogs = Catalogs::new(
///     Catalog::parse(en.clone(), "greeting = Hello\nfarewell = Goodbye").unwrap(),
/// )
/// .with(Catalog::parse(fr.clone(), "greeting = Bonjour").unwrap());
///
/// assert_eq!(catalogs.default_locale(), &en);
/// assert_eq!(catalogs.message(&fr, "greeting").unwrap(), "Bonjour");
///
/// // `farewell` was never translated into French. The page still renders,
/// // in English, instead of failing.
/// assert_eq!(catalogs.message(&fr, "farewell").unwrap(), "Goodbye");
///
/// // A locale nobody registered is not in the set, and that is the check
/// // locale negotiation is built on.
/// assert!(!catalogs.contains(&LocaleId::parse("de").unwrap()));
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Catalogs {
    default: LocaleId,
    by_locale: Arc<BTreeMap<LocaleId, Arc<Catalog>>>,
}

impl Catalogs {
    /// Start a registry from the catalog that is also its default.
    ///
    /// The default is a `Catalog` and not a `LocaleId` on purpose: a registry
    /// whose default locale has no catalog is a configuration that fails at
    /// the first request in the worst language, and this signature makes it
    /// unspellable.
    #[must_use]
    pub fn new(default: Catalog) -> Self {
        let default_locale = default.locale().clone();
        let mut by_locale = BTreeMap::new();
        by_locale.insert(default_locale.clone(), Arc::new(default));
        Self {
            default: default_locale,
            by_locale: Arc::new(by_locale),
        }
    }

    /// Register another locale's catalog.
    ///
    /// Registering the same locale twice keeps the later catalog.
    #[must_use]
    pub fn with(mut self, catalog: Catalog) -> Self {
        Arc::make_mut(&mut self.by_locale).insert(catalog.locale().clone(), Arc::new(catalog));
        self
    }

    /// The locale used when nothing better is registered.
    #[must_use]
    pub fn default_locale(&self) -> &LocaleId {
        &self.default
    }

    /// The default locale's catalog, which always exists.
    #[must_use]
    pub fn default_catalog(&self) -> &Catalog {
        // Established by `new` and never removed: nothing in the API deletes
        // a catalog, and `with` only inserts.
        &self.by_locale[&self.default]
    }

    /// Whether `locale` is registered.
    ///
    /// This is the whitelist test. Locale negotiation asks this and nothing
    /// else.
    #[must_use]
    pub fn contains(&self, locale: &LocaleId) -> bool {
        self.by_locale.contains_key(locale)
    }

    /// The catalog for `locale`, if it is registered.
    #[must_use]
    pub fn catalog(&self, locale: &LocaleId) -> Option<&Catalog> {
        self.by_locale.get(locale).map(AsRef::as_ref)
    }

    /// Every registered locale, in canonical-tag order.
    pub fn locales(&self) -> impl ExactSizeIterator<Item = &LocaleId> {
        self.by_locale.keys()
    }

    /// Format a no-argument message in `locale`, falling back to the default
    /// catalog for a key `locale` has not translated yet.
    ///
    /// # Errors
    ///
    /// As [`translate`](Self::translate).
    pub fn message(&self, locale: &LocaleId, key: &str) -> Result<String, I18nError> {
        self.translate(locale, key, &TranslationArgs::new())
    }

    /// Format a message in `locale`, falling back to the default catalog for
    /// a key `locale` has not translated yet.
    ///
    /// # Errors
    ///
    /// [`I18nError::Missing`] if neither `locale`'s catalog nor the default
    /// one has the key, [`I18nError::Format`] if the message that was found
    /// could not be formatted. A formatting failure is *not* retried against
    /// the default catalog: the message was found, the caller's arguments did
    /// not fit it, and the same arguments will not fit the English one
    /// either.
    pub fn translate(
        &self,
        locale: &LocaleId,
        key: &str,
        args: &TranslationArgs,
    ) -> Result<String, I18nError> {
        if let Some(catalog) = self.catalog(locale)
            && catalog.has(key)
        {
            return catalog.translate(key, args);
        }
        self.default_catalog()
            .translate(key, args)
            .map_err(|error| match error {
                I18nError::Missing { key, .. } => I18nError::Missing {
                    locale: locale.clone(),
                    key,
                },
                other => other,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn en() -> LocaleId {
        LocaleId::parse("en").unwrap()
    }

    fn fr() -> LocaleId {
        LocaleId::parse("fr").unwrap()
    }

    fn catalog(locale: LocaleId, ftl: &str) -> Catalog {
        Catalog::parse(locale, ftl).unwrap().isolating(false)
    }

    /// A catalog is built once and formatted from every request at the same
    /// time. If this stops compiling, the bundle went back to the
    /// single-threaded memoizer and the whole subsystem is unusable in a
    /// handler.
    #[test]
    fn a_catalog_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Catalog>();
        assert_send_sync::<Catalogs>();
    }

    #[test]
    fn a_message_formats() {
        let catalog = catalog(en(), "greeting = Hello");
        assert_eq!(catalog.message("greeting").unwrap(), "Hello");
    }

    #[test]
    fn a_missing_message_is_an_error_and_not_an_empty_string() {
        let catalog = catalog(en(), "greeting = Hello");
        assert!(matches!(
            catalog.message("nope"),
            Err(I18nError::Missing { .. })
        ));
    }

    #[test]
    fn a_broken_source_is_a_parse_error() {
        let error = Catalog::parse(en(), "= no key here").unwrap_err();
        assert!(matches!(error, I18nError::Parse { .. }));
    }

    #[test]
    fn a_second_source_joins_the_same_catalog() {
        let catalog = catalog(en(), "a = A").with_source("b = B").unwrap();
        assert_eq!(catalog.message("a").unwrap(), "A");
        assert_eq!(catalog.message("b").unwrap(), "B");
    }

    /// Overwriting silently would make a key's meaning depend on the order
    /// the application happened to add its files in.
    #[test]
    fn a_redefinition_is_refused() {
        let error = catalog(en(), "a = A").with_source("a = B").unwrap_err();
        assert!(matches!(error, I18nError::Parse { .. }));
    }

    #[test]
    fn a_missing_argument_is_an_error_and_never_a_half_formatted_string() {
        let catalog = catalog(en(), "hello = Hello, { $name }.");
        let error = catalog.message("hello").unwrap_err();
        assert!(matches!(error, I18nError::Format { .. }));
        // Fluent's recovery text must not be what the caller gets back.
        assert!(!error.to_string().contains("Hello, {$name}"));
    }

    #[test]
    fn an_attribute_is_addressable_on_its_own() {
        let catalog = catalog(
            en(),
            "search = Search\n    .placeholder = Search the archive",
        );
        assert_eq!(catalog.message("search").unwrap(), "Search");
        assert_eq!(
            catalog
                .attribute("search", "placeholder", &TranslationArgs::new())
                .unwrap(),
            "Search the archive"
        );
        assert!(matches!(
            catalog.attribute("search", "title", &TranslationArgs::new()),
            Err(I18nError::Missing { .. })
        ));
    }

    /// The reason this subsystem is Fluent and not a `HashMap<String,
    /// String>`. Polish has four plural categories; a map keyed by
    /// "singular" and "plural" cannot express three of them, and no amount
    /// of calling code can fix that from outside the catalog.
    #[test]
    fn plural_categories_beyond_two_are_selected_correctly() {
        let catalog = catalog(
            LocaleId::parse("pl").unwrap(),
            r"files = { $count ->
    [one] plik
    [few] pliki
    [many] plikow
   *[other] pliku
}",
        );

        let of = |n: i64| {
            catalog
                .translate("files", &TranslationArgs::new().with("count", n))
                .unwrap()
        };

        assert_eq!(of(1), "plik");
        assert_eq!(of(2), "pliki");
        assert_eq!(of(5), "plikow");
        assert_eq!(of(22), "pliki");
    }

    #[test]
    fn isolation_marks_are_on_by_default() {
        let catalog = Catalog::parse(en(), "hi = Hi, { $name }!").unwrap();
        let formatted = catalog
            .translate("hi", &TranslationArgs::new().with("name", "Ada"))
            .unwrap();
        assert!(formatted.contains('\u{2068}'), "{formatted:?}");
        assert!(formatted.contains('\u{2069}'), "{formatted:?}");
    }

    #[test]
    fn the_default_catalog_is_always_present() {
        let catalogs = Catalogs::new(catalog(en(), "a = A"));
        assert_eq!(catalogs.default_locale(), &en());
        assert_eq!(catalogs.default_catalog().locale(), &en());
        assert!(catalogs.contains(&en()));
    }

    #[test]
    fn an_unregistered_locale_is_not_in_the_set() {
        let catalogs = Catalogs::new(catalog(en(), "a = A"));
        assert!(!catalogs.contains(&fr()));
        assert!(catalogs.catalog(&fr()).is_none());
    }

    #[test]
    fn a_registered_locale_wins_over_the_default() {
        let catalogs = Catalogs::new(catalog(en(), "greeting = Hello"))
            .with(catalog(fr(), "greeting = Bonjour"));
        assert_eq!(catalogs.message(&fr(), "greeting").unwrap(), "Bonjour");
        assert_eq!(catalogs.message(&en(), "greeting").unwrap(), "Hello");
    }

    #[test]
    fn an_untranslated_key_falls_back_to_the_default_catalog() {
        let catalogs = Catalogs::new(catalog(en(), "greeting = Hello\nbye = Goodbye"))
            .with(catalog(fr(), "greeting = Bonjour"));
        assert_eq!(catalogs.message(&fr(), "bye").unwrap(), "Goodbye");
    }

    /// The fallback is per key, so it cannot turn "this application has no
    /// German" into a silently English page for a locale that was never
    /// registered -- negotiation has already refused that locale by then.
    #[test]
    fn a_key_missing_everywhere_reports_the_locale_that_was_asked_for() {
        let catalogs = Catalogs::new(catalog(en(), "greeting = Hello"))
            .with(catalog(fr(), "greeting = Bonjour"));
        match catalogs.message(&fr(), "nope") {
            Err(I18nError::Missing { locale, key }) => {
                assert_eq!(locale, fr());
                assert_eq!(key, "nope");
            }
            other => panic!("expected a miss, got {other:?}"),
        }
    }

    #[test]
    fn registering_a_locale_twice_keeps_the_later_catalog() {
        let catalogs = Catalogs::new(catalog(en(), "a = first")).with(catalog(en(), "a = second"));
        assert_eq!(catalogs.message(&en(), "a").unwrap(), "second");
        assert_eq!(catalogs.locales().len(), 1);
    }

    #[test]
    fn cloning_the_registry_does_not_clone_the_catalogs() {
        let catalogs = Catalogs::new(catalog(en(), "a = A"));
        let clone = catalogs.clone();
        assert!(std::ptr::eq(
            std::ptr::from_ref(catalogs.default_catalog()),
            std::ptr::from_ref(clone.default_catalog())
        ));
    }
}
