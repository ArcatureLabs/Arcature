//! The localization subsystem's error type.
//!
//! Two rules shape what these variants are allowed to carry.
//!
//! **A rejected locale tag is never quoted back.** The string that failed
//! validation is, by construction, the one place in this subsystem where a
//! request's bytes arrive unexamined: a header value, a query parameter, a
//! value pulled out of a session. Writing it into an error's `Display` puts
//! it one `tracing::warn!` away from a log line, and a log line is a text
//! format with no escaping -- a tag containing `\n` writes a second entry, a
//! tag containing a terminal escape sequence is read by whoever `cat`s the
//! file. So [`LocaleRejection`] says *what was wrong* and never *what was
//! sent*, and the three shapes it distinguishes are enough to debug a
//! developer's own typo.
//!
//! **A message key is not a secret, and a translated string may be.** A key
//! is a constant in the application's source, so naming one in an error is
//! safe and useful. The formatted value is not: it can carry whatever the
//! caller interpolated into it. That is why [`I18nError::Format`] reports
//! Fluent's own diagnostics and never the partially formatted output.

use std::fmt;

use super::LocaleId;

/// Why a string was refused as a locale tag.
///
/// Deliberately coarse, and deliberately free of the input. See the module
/// documentation for the reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocaleRejection {
    /// The tag was empty.
    Empty,
    /// The tag was longer than any well-formed BCP-47 language identifier.
    TooLong,
    /// The tag is not a well-formed BCP-47 language identifier: an empty or
    /// over-long subtag, a byte outside `[A-Za-z0-9-]`, or a subtag sequence
    /// that is not a language, script, region and variants in that order.
    NotWellFormed,
}

impl fmt::Display for LocaleRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::Empty => "it was empty",
            Self::TooLong => "it was too long to be a language identifier",
            Self::NotWellFormed => "it is not a well-formed BCP-47 language identifier",
        };
        formatter.write_str(reason)
    }
}

/// Something went wrong loading or using a translation catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum I18nError {
    /// A string was refused as a locale tag.
    ///
    /// The string itself is not carried. See the module documentation.
    InvalidLocale(LocaleRejection),
    /// An `.ftl` source did not parse, or its messages collided with ones
    /// already in the catalog.
    ///
    /// This is a developer error by definition: the source is a file in the
    /// repository, so a catalog that fails here fails on the first request
    /// after a deploy, for everyone, identically.
    Parse {
        /// The catalog's locale.
        locale: LocaleId,
        /// Fluent's diagnostics, one per problem, already rendered to text
        /// because `fluent-syntax`'s error type is not a public dependency of
        /// this crate.
        errors: Vec<String>,
    },
    /// The catalog has no message under that key -- and neither does the
    /// default catalog, when the lookup went through
    /// [`Catalogs`](super::Catalogs).
    Missing {
        /// The catalog the lookup started in.
        locale: LocaleId,
        /// The message key, and the attribute after a `.` when one was asked
        /// for. Both come from the application's own source.
        key: String,
    },
    /// The message exists but could not be formatted: a placeable named an
    /// argument that was not supplied, a selector had no matching variant, a
    /// referenced term is absent.
    Format {
        /// The catalog that owns the message.
        locale: LocaleId,
        /// The message key.
        key: String,
        /// Fluent's diagnostics, rendered to text.
        errors: Vec<String>,
    },
}

impl fmt::Display for I18nError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLocale(reason) => write!(formatter, "invalid locale tag: {reason}"),
            Self::Parse { locale, errors } => write!(
                formatter,
                "the `{locale}` catalog did not parse: {}",
                errors.join("; ")
            ),
            Self::Missing { locale, key } => write!(
                formatter,
                "no message `{key}` in the `{locale}` catalog or the default one"
            ),
            Self::Format {
                locale,
                key,
                errors,
            } => write!(
                formatter,
                "message `{key}` in the `{locale}` catalog could not be formatted: {}",
                errors.join("; ")
            ),
        }
    }
}

impl std::error::Error for I18nError {}

/// Fold a translation failure into the framework error vocabulary as a plain
/// internal error.
///
/// The same argument `src/view/error.rs` makes, for the same reason:
/// `Error`'s own `IntoResponse` writes its `Display` text into the `detail`
/// field of the problem document outside production, so anything put here is
/// one `APP_ENV` away from the wire. A message key is a fragment of the
/// application's source tree and Fluent's diagnostics quote the catalog. The
/// operator gets both through `tracing`; the client gets neither.
impl From<I18nError> for crate::Error {
    fn from(error: I18nError) -> Self {
        report(&error);
        crate::Error::Other("translation failed".into())
    }
}

/// Record a translation failure for the operator.
fn report(error: &I18nError) {
    #[cfg(feature = "observe")]
    tracing::error!(%error, "a translation failed; the client gets a generic 500");
    #[cfg(not(feature = "observe"))]
    let _ = error;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the whole module exists for: nothing an attacker typed
    /// comes back out of the error, not even through `Debug`.
    #[test]
    fn a_rejected_tag_is_never_quoted_back() {
        let hostile = "../../etc/passwd\nFAKE LOG LINE";
        let error = LocaleId::parse(hostile).unwrap_err();

        assert!(!error.to_string().contains("passwd"));
        assert!(!error.to_string().contains('\n'));
        assert!(!format!("{error:?}").contains("passwd"));
    }

    #[test]
    fn the_framework_error_carries_no_catalog_detail() {
        let error = I18nError::Missing {
            locale: LocaleId::parse("en").unwrap(),
            key: "billing-invoice-overdue".into(),
        };
        assert!(error.to_string().contains("billing-invoice-overdue"));

        let framework: crate::Error = error.into();
        assert_eq!(framework.status(), 500);
        assert_eq!(framework.code(), "internal_error");
        assert!(!framework.to_string().contains("billing-invoice-overdue"));
    }
}
