//! [`LocaleId`]: a locale tag that has already been proven well-formed.
//!
//! # Why a newtype and not a `String`
//!
//! Every locale a running application uses arrives from somewhere hostile or
//! semi-hostile: an `Accept-Language` header, a `?lang=` parameter, a value
//! read back out of a session. A `String` carries none of that history, so
//! the check has to be repeated at every use, and the one call site that
//! forgets is the bug.
//!
//! `LocaleId` moves the check to the one place a value can be built.
//! [`LocaleId::parse`] is the only constructor, it is fallible, and what comes
//! out is a canonical BCP-47 language identifier: at most 35 bytes, subtags
//! of 1--8 ASCII alphanumerics separated by `-`, in language-script-region-
//! variant order. `..`, `/`, `\`, a NUL byte, a newline and a 10 KB string are
//! all rejected before anything else in the framework sees them.
//!
//! That does not make it safe to build a path out of one, and nothing in this
//! subsystem does -- see `src/i18n/mod.rs` on why catalogs are compiled in
//! rather than read from a directory a request can name. It does mean that a
//! function taking a `LocaleId` cannot be handed a request's raw bytes by
//! accident, because there is no way to spell the conversion that does not go
//! through the validator.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use super::error::{I18nError, LocaleRejection};

/// The longest string accepted as a locale tag.
///
/// A BCP-47 language identifier of `language-script-region` plus two variants
/// is 32 bytes; 35 leaves a little room and still refuses anything that could
/// plausibly be a payload. The bound is checked before the tag is parsed, so
/// an attacker cannot spend the server's time on a megabyte of `a-`.
const MAX_TAG_LEN: usize = 35;

/// A validated, canonical locale tag.
///
/// ```
/// use arcature::i18n::LocaleId;
///
/// // Canonical casing is applied on the way in, so `en-us` and `EN-US`
/// // are the same locale and compare equal.
/// let locale = LocaleId::parse("en-us").unwrap();
/// assert_eq!(locale.as_str(), "en-US");
/// assert_eq!(locale, LocaleId::parse("EN-US").unwrap());
/// assert_eq!(locale.language(), "en");
///
/// // Anything that is not a language identifier is refused here, once,
/// // rather than at every call site that would otherwise take a `String`.
/// assert!(LocaleId::parse("../../etc/passwd").is_err());
/// assert!(LocaleId::parse("en\0").is_err());
/// assert!(LocaleId::parse(&"a".repeat(4096)).is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct LocaleId(Arc<str>);

impl LocaleId {
    /// Validate and canonicalize a locale tag.
    ///
    /// # Errors
    ///
    /// [`I18nError::InvalidLocale`] if the string is empty, longer than 35
    /// bytes, or not a well-formed BCP-47 language identifier. The rejected
    /// string is not carried in the error; see `src/i18n/error.rs` for why.
    pub fn parse(tag: &str) -> Result<Self, I18nError> {
        Self::well_formed(tag).map_err(I18nError::InvalidLocale)?;

        // Only now, on a string already known to be short and alphanumeric,
        // is the real parser allowed to run. It is the half that knows a
        // script subtag from a region one, and it is also the half that
        // produces the canonical casing.
        let parsed: unic_langid::LanguageIdentifier = tag
            .parse()
            .map_err(|_| I18nError::InvalidLocale(LocaleRejection::NotWellFormed))?;

        Ok(Self(Arc::from(parsed.to_string().as_str())))
    }

    /// The canonical tag, e.g. `en-US`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The primary language subtag, e.g. `en` for `en-US`.
    ///
    /// This is what a fallback chain matches on when no registered locale is
    /// an exact match for what the client asked for.
    #[must_use]
    pub fn language(&self) -> &str {
        self.0.split('-').next().unwrap_or(&self.0)
    }

    /// The cheap, allocation-free half of the check.
    ///
    /// Everything here is a bound on the *shape* of the input rather than a
    /// judgement about locales: length, subtag length, and the byte set. It
    /// runs first so that the structured parser below it only ever sees a
    /// string that is at most 35 ASCII alphanumerics and dashes.
    fn well_formed(tag: &str) -> Result<(), LocaleRejection> {
        if tag.is_empty() {
            return Err(LocaleRejection::Empty);
        }
        if tag.len() > MAX_TAG_LEN {
            return Err(LocaleRejection::TooLong);
        }
        for subtag in tag.split('-') {
            if subtag.is_empty() || subtag.len() > 8 {
                return Err(LocaleRejection::NotWellFormed);
            }
            if !subtag.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
                return Err(LocaleRejection::NotWellFormed);
            }
        }
        Ok(())
    }

    /// The `unic-langid` value `fluent-bundle` needs to build a bundle.
    ///
    /// Infallible in practice and crate-private on purpose: the tag stored in
    /// `self` is the canonical rendering of a value this type already parsed,
    /// so re-parsing it round-trips. Keeping the conversion here means
    /// `unic-langid` never appears in a public signature, so a major version
    /// of it is a patch release of Arcature rather than a breaking one.
    pub(crate) fn to_langid(&self) -> unic_langid::LanguageIdentifier {
        self.0
            .parse()
            .unwrap_or_else(|_| unic_langid::LanguageIdentifier::default())
    }
}

impl fmt::Display for LocaleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for LocaleId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for LocaleId {
    type Err = I18nError;

    fn from_str(tag: &str) -> Result<Self, Self::Err> {
        Self::parse(tag)
    }
}

/// Serializes as the canonical tag, so a locale reaches a client or a session
/// as `"en-US"` and not as a struct.
impl serde::Serialize for LocaleId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_language_parses() {
        assert_eq!(LocaleId::parse("fr").unwrap().as_str(), "fr");
    }

    #[test]
    fn casing_is_canonicalized() {
        for spelling in ["zh-hant-hk", "ZH-HANT-HK", "zh-Hant-HK"] {
            assert_eq!(LocaleId::parse(spelling).unwrap().as_str(), "zh-Hant-HK");
        }
    }

    #[test]
    fn the_language_subtag_is_the_first_one() {
        assert_eq!(LocaleId::parse("pt-BR").unwrap().language(), "pt");
        assert_eq!(LocaleId::parse("pt").unwrap().language(), "pt");
    }

    /// The table this whole type exists for. Each of these is something a
    /// request can contain, and none of them may become a `LocaleId`.
    #[test]
    fn hostile_shapes_are_refused() {
        let cases: &[(&str, LocaleRejection)] = &[
            ("", LocaleRejection::Empty),
            ("../../etc/passwd", LocaleRejection::NotWellFormed),
            ("..", LocaleRejection::NotWellFormed),
            ("../en", LocaleRejection::NotWellFormed),
            ("en/../..", LocaleRejection::NotWellFormed),
            ("en\\..\\..", LocaleRejection::NotWellFormed),
            ("en\0", LocaleRejection::NotWellFormed),
            ("\0", LocaleRejection::NotWellFormed),
            ("en\nSet-Cookie: a=b", LocaleRejection::NotWellFormed),
            ("en US", LocaleRejection::NotWellFormed),
            ("en_US", LocaleRejection::NotWellFormed),
            ("en-", LocaleRejection::NotWellFormed),
            ("-en", LocaleRejection::NotWellFormed),
            ("en--US", LocaleRejection::NotWellFormed),
            ("%2e%2e%2f", LocaleRejection::NotWellFormed),
            ("C:", LocaleRejection::NotWellFormed),
            ("~", LocaleRejection::NotWellFormed),
            ("$(id)", LocaleRejection::NotWellFormed),
            ("en\u{202e}", LocaleRejection::NotWellFormed),
        ];

        for (input, expected) in cases {
            match LocaleId::parse(input) {
                Err(I18nError::InvalidLocale(reason)) => {
                    assert_eq!(reason, *expected, "wrong rejection reason for {input:?}")
                }
                other => panic!("{input:?} was not rejected: {other:?}"),
            }
        }
    }

    #[test]
    fn an_overlong_tag_is_refused_before_it_is_parsed() {
        let long = "a".repeat(4096);
        assert!(matches!(
            LocaleId::parse(&long),
            Err(I18nError::InvalidLocale(LocaleRejection::TooLong))
        ));

        // And so is one that is only just too long, so the bound is the
        // length and not the byte set.
        let boundary = "en-".to_string() + &"a".repeat(33);
        assert_eq!(boundary.len(), MAX_TAG_LEN + 1);
        assert!(matches!(
            LocaleId::parse(&boundary),
            Err(I18nError::InvalidLocale(LocaleRejection::TooLong))
        ));
    }

    #[test]
    fn a_locale_serializes_as_its_tag() {
        let json = serde_json::to_string(&LocaleId::parse("en-GB").unwrap()).unwrap();
        assert_eq!(json, "\"en-GB\"");
    }

    #[test]
    fn from_str_agrees_with_parse() {
        let parsed: LocaleId = "de-AT".parse().unwrap();
        assert_eq!(parsed, LocaleId::parse("de-AT").unwrap());
    }

    #[test]
    fn the_canonical_tag_round_trips_into_a_langid() {
        let locale = LocaleId::parse("zh-hant-hk").unwrap();
        assert_eq!(locale.to_langid().to_string(), "zh-Hant-HK");
    }
}
