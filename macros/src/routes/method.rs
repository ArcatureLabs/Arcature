//! The HTTP method of a `routes!` entry.
//!
//! One responsibility: the macro-side method enum and the two renderings the
//! expansion needs -- the `Route::<S>::<fn>` constructor name and the
//! `RouteMethod::<Variant>` metadata ident.

/// The HTTP method as parsed from a `routes!` method keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteMethodKind {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl RouteMethodKind {
    /// Returns the `arcature::Route` constructor name (`get`, `post`, ...).
    pub const fn constructor(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Patch => "patch",
            Self::Delete => "delete",
            Self::Head => "head",
            Self::Options => "options",
        }
    }

    /// Returns the `arcature::RouteMethod` variant name (`Get`, `Post`, ...).
    pub const fn variant(self) -> &'static str {
        match self {
            Self::Get => "Get",
            Self::Post => "Post",
            Self::Put => "Put",
            Self::Patch => "Patch",
            Self::Delete => "Delete",
            Self::Head => "Head",
            Self::Options => "Options",
        }
    }

    /// Returns true when the method is state-changing (POST/PUT/PATCH/DELETE).
    ///
    /// An `action:` route must use an unsafe method: a GET must never mutate.
    pub const fn is_unsafe(self) -> bool {
        matches!(self, Self::Post | Self::Put | Self::Patch | Self::Delete)
    }
}

#[cfg(test)]
mod tests {
    use super::RouteMethodKind;

    #[test]
    fn constructor_is_the_lowercase_method() {
        assert_eq!(RouteMethodKind::Get.constructor(), "get");
        assert_eq!(RouteMethodKind::Delete.constructor(), "delete");
    }

    #[test]
    fn variant_is_the_pascal_case_method() {
        assert_eq!(RouteMethodKind::Patch.variant(), "Patch");
    }

    #[test]
    fn only_mutating_methods_are_unsafe() {
        assert!(RouteMethodKind::Post.is_unsafe());
        assert!(RouteMethodKind::Delete.is_unsafe());
        assert!(!RouteMethodKind::Get.is_unsafe());
        assert!(!RouteMethodKind::Head.is_unsafe());
        assert!(!RouteMethodKind::Options.is_unsafe());
    }
}
