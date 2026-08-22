//! The arguments a message is formatted with.
//!
//! # Why this is not `fluent_bundle::FluentArgs`
//!
//! Two reasons, and the second is the one that matters.
//!
//! `FluentArgs<'a>` borrows its values, so a handler that builds arguments
//! from anything it computed has to keep those values alive until after the
//! format call -- a lifetime puzzle in exchange for one avoided allocation
//! per placeable, on a path that is about to allocate a `String` anyway.
//! [`TranslationArgs`] owns its values and is trivially movable.
//!
//! The load-bearing reason is the crate boundary. `FluentArgs` in a public
//! signature would make `fluent-bundle`'s version part of Arcature's public
//! API, so a `0.16` to `0.17` bump upstream -- a routine event for a crate at
//! `0.x` -- would be a breaking change here, forcing a minor release of a
//! framework at `0.x`, which under this project's SemVer rule is the breaking
//! bump. Owning a two-variant value type instead costs a `match` and buys the
//! freedom to move under it.

/// A value interpolated into a message.
///
/// The integer and float cases are separate because CLDR treats them
/// separately: in several languages `1` and `1.0` fall into different plural
/// categories, and collapsing both into an `f64` would quietly pick the wrong
/// one. Fluent formats an integer with no fraction digits and a float with
/// the ones it has, which is also what a reader expects of a price next to a
/// count.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ArgValue {
    /// A string, interpolated as-is. Escaping is the view layer's job, not
    /// the catalog's: Fluent produces text, and what makes that text safe in
    /// HTML is askama's autoescaper or the Inertia client's `textContent`.
    Text(String),
    /// A whole number. Selects a plural category as an integer.
    Integer(i64),
    /// A fractional number. Selects a plural category as a decimal, which is
    /// not always the same answer as the integer nearest to it.
    Number(f64),
}

impl From<String> for ArgValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for ArgValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<i64> for ArgValue {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<i32> for ArgValue {
    fn from(value: i32) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<u32> for ArgValue {
    fn from(value: u32) -> Self {
        Self::Integer(i64::from(value))
    }
}

/// Saturates rather than wrapping. A count that overflows an `i64` is already
/// nonsense, and the plural category of `i64::MAX` is the same as the one of
/// any other implausibly large number.
impl From<usize> for ArgValue {
    fn from(value: usize) -> Self {
        Self::Integer(i64::try_from(value).unwrap_or(i64::MAX))
    }
}

impl From<f64> for ArgValue {
    fn from(value: f64) -> Self {
        Self::Number(value)
    }
}

/// The named arguments a message is formatted with.
///
/// ```
/// use arcature::i18n::{Catalog, LocaleId, TranslationArgs};
///
/// let catalog = Catalog::parse(
///     LocaleId::parse("en").unwrap(),
///     "invoice = { $name } owes { $amount }",
/// )
/// .unwrap()
/// .isolating(false);
///
/// let args = TranslationArgs::new()
///     .with("name", "Ada")
///     .with("amount", 12.5);
///
/// assert_eq!(catalog.translate("invoice", &args).unwrap(), "Ada owes 12.5");
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct TranslationArgs {
    /// A `Vec` and not a map: a message has a handful of placeables, a linear
    /// scan over three entries beats hashing three keys, and insertion order
    /// makes a `Debug` dump read like the call that produced it.
    entries: Vec<(String, ArgValue)>,
}

impl TranslationArgs {
    /// No arguments.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a named argument.
    ///
    /// Setting the same name twice keeps the later value.
    #[must_use]
    pub fn with(mut self, name: impl Into<String>, value: impl Into<ArgValue>) -> Self {
        let name = name.into();
        let value = value.into();
        match self
            .entries
            .iter_mut()
            .find(|(existing, _)| *existing == name)
        {
            Some(entry) => entry.1 = value,
            None => self.entries.push((name, value)),
        }
        self
    }

    /// Whether any argument has been set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many arguments have been set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The arguments, in the order they were set.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ArgValue)> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_keep_their_insertion_order() {
        let args = TranslationArgs::new()
            .with("b", 1)
            .with("a", 2)
            .with("c", "three");
        let names: Vec<&str> = args.iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["b", "a", "c"]);
        assert_eq!(args.len(), 3);
        assert!(!args.is_empty());
    }

    #[test]
    fn setting_a_name_twice_keeps_the_later_value() {
        let args = TranslationArgs::new().with("n", 1).with("n", 2);
        assert_eq!(args.len(), 1);
        assert_eq!(args.iter().next().unwrap().1, &ArgValue::Integer(2));
    }

    #[test]
    fn an_integer_and_a_float_are_different_values() {
        assert_eq!(ArgValue::from(1_i64), ArgValue::Integer(1));
        assert_eq!(ArgValue::from(1.0_f64), ArgValue::Number(1.0));
        assert_ne!(ArgValue::from(1_i64), ArgValue::from(1.0_f64));
    }

    #[test]
    fn a_usize_too_large_for_an_i64_saturates() {
        assert_eq!(ArgValue::from(7_usize), ArgValue::Integer(7));
        assert_eq!(ArgValue::from(usize::MAX), ArgValue::Integer(i64::MAX));
    }

    #[test]
    fn an_empty_set_is_empty() {
        assert!(TranslationArgs::new().is_empty());
        assert_eq!(TranslationArgs::new().len(), 0);
    }
}
