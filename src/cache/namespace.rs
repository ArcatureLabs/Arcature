//! A deterministic key namespace prefix that prevents accidental key collisions
//! between applications or environments.
//!
//! `Namespace` applies a fixed prefix to every key before it reaches the
//! backend. The prefix is stored as an `Arc<str>` so cloning a
//! [`crate::cache::Cache`] does not copy it. The separator is `:` (the
//! Redis/Valkey convention), and the prefix is constructed so that
//! `(a + bc)` and `(ab + c)` never collide.

use std::fmt;
use std::sync::Arc;

use crate::cache::CacheConfigError;

/// A key namespace prefix.
#[derive(Clone)]
pub struct Namespace(Arc<str>);

impl Namespace {
    /// No namespace: the caller's key is used verbatim.
    ///
    /// This is a distinct, explicit value -- not `None` at the type level --
    /// so that the absence of namespacing is a deliberate choice.
    #[must_use]
    pub fn none() -> Self {
        Self::empty()
    }

    fn empty() -> Self {
        Self(Arc::from(""))
    }

    /// Create a namespace from a prefix string.
    ///
    /// # Errors
    ///
    /// Returns [`CacheConfigError::EmptyNamespace`] if the prefix is empty
    /// (use [`Namespace::none`] for no namespacing),
    /// [`CacheConfigError::NamespaceEndsWithSeparator`] if it ends with `:`,
    /// or [`CacheConfigError::NamespaceContainsControlChar`] if it contains a
    /// control character.
    pub fn new(prefix: &str) -> Result<Self, CacheConfigError> {
        if prefix.is_empty() {
            return Err(CacheConfigError::EmptyNamespace);
        }
        if prefix.ends_with(':') {
            return Err(CacheConfigError::NamespaceEndsWithSeparator);
        }
        if prefix.chars().any(|c| c.is_control()) {
            return Err(CacheConfigError::NamespaceContainsControlChar);
        }
        Ok(Self(Arc::from(prefix)))
    }

    /// Return the full backend key for a caller key: `prefix:key`, or just
    /// `key` when no namespace is set.
    pub(crate) fn resolve(&self, key: &str) -> String {
        if self.0.is_empty() {
            return key.to_string();
        }
        let mut full = String::with_capacity(self.0.len() + 1 + key.len());
        full.push_str(&self.0);
        full.push(':');
        full.push_str(key);
        full
    }

    /// Whether this namespace is the empty (no-prefix) namespace.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Namespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Namespace").field(&self.0.as_ref()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_key_with_separator() {
        let ns = Namespace::new("myapp").expect("valid");
        assert_eq!(ns.resolve("user:42"), "myapp:user:42");
    }

    #[test]
    fn none_namespace_uses_key_verbatim() {
        let ns = Namespace::none();
        assert_eq!(ns.resolve("user:42"), "user:42");
        assert!(ns.is_empty());
    }

    #[test]
    fn rejects_empty_prefix() {
        assert!(matches!(
            Namespace::new(""),
            Err(CacheConfigError::EmptyNamespace)
        ));
    }

    #[test]
    fn rejects_trailing_separator() {
        assert!(matches!(
            Namespace::new("myapp:"),
            Err(CacheConfigError::NamespaceEndsWithSeparator)
        ));
    }

    #[test]
    fn rejects_control_chars() {
        assert!(matches!(
            Namespace::new("my\0app"),
            Err(CacheConfigError::NamespaceContainsControlChar)
        ));
    }

    #[test]
    fn no_collision_between_split_points() {
        let ns_a = Namespace::new("a").expect("valid");
        let ns_ab = Namespace::new("ab").expect("valid");
        assert_ne!(ns_a.resolve("bc"), ns_ab.resolve("c"));
    }
}
