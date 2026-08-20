//! Arcature macro error codes.
//!
//! Codes follow the format `ARC-M<NNN>` and are rendered as
//! `error[ARC-MNNN]` in compiler diagnostics. A code is added only when a
//! macro introduces a new, distinct failure mode -- never speculatively.
//! The scheme is designed to be greppable and maintainable.
//!
//! A macro must **never** panic on ordinary syntax mistakes. Instead, it
//! returns `Result<TokenStream, MacroError>` and the thin `lib.rs`
//! entrypoint converts an error into a `compile_error!` invocation via
//! [`MacroError::to_compile_error`]. This satisfies the invariant: never
//! intentionally expose users to "proc macro panicked" for ordinary syntax
//! mistakes.

use proc_macro2::TokenStream;

/// A stable error code for Arcature macro diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroErrorCode {
    /// Input could not be parsed as the expected Rust syntax.
    ///
    /// Emitted when `syn::parse2` fails -- the input tokens do not form a
    /// valid Rust syntax tree for the macro's expected input type.
    ArcM001,

    /// A macro attribute argument is invalid or unexpected.
    ///
    /// Emitted when a helper attribute contains an unknown key, a wrong
    /// value type, or a missing required value.
    ArcM002,

    /// A resource action name is not recognized.
    ///
    /// Emitted when `only` or `except` in a `resource` route contains an
    /// unknown action name (not one of: index, create, store, show, edit,
    /// update, destroy).
    ArcM003,

    /// A controller method signature is invalid.
    ///
    /// Emitted when a method in a `#[controller]` impl block is not `pub`,
    /// not `async`, or has no return type annotation.
    ArcM004,

    /// A `#[route_model]` attribute argument is missing or invalid.
    ///
    /// Emitted when `#[route_model]` is missing a required argument
    /// (`entity`, `key`, or `key_type`), or has an argument with the wrong
    /// value type.
    ArcM005,

    /// A `#[service]` or `#[provider]` attribute argument is invalid, or
    /// the annotated item is not a plain struct with named fields.
    ///
    /// Emitted when `#[service]` / `#[provider]` is applied to an enum,
    /// union, tuple struct, or unit struct, or when an unknown attribute
    /// key is supplied.
    ArcM006,

    /// A `#[middleware]` function signature is invalid.
    ///
    /// Emitted when `#[middleware]` is applied to a non-`pub`, non-`async`
    /// function, or a function with no return type annotation.
    ArcM007,

    /// A `#[listener(Event)]` function signature is invalid.
    ///
    /// Emitted when `#[listener]` is applied to a non-`pub`, non-`async`
    /// function, or a function with no return type annotation.
    ArcM008,

    /// A `#[derive(Job)]` attribute argument is invalid.
    ///
    /// Emitted when `#[job(...)]` contains an unknown key, a wrong value
    /// type, an out-of-range `version` (must be >= 1), or an out-of-range
    /// `attempts` (must be >= 1).
    ArcM009,

    /// A `#[job_handler]` function signature is invalid.
    ///
    /// Emitted when `#[job_handler]` is applied to a non-`pub`, non-`async`
    /// function, or a function with no return type annotation.
    ArcM010,

    /// A `#[command("name")]` function signature is invalid.
    ///
    /// Emitted when `#[command]` is applied to a non-`pub`, non-`async`
    /// function, or a function with no return type annotation.
    ArcM011,

    /// A `#[arcature::test]` attribute argument is invalid or missing.
    ///
    /// Emitted when `#[arcature::test]` is missing the required
    /// `app = <expr>` key, has an unknown key, or the `app` value is a
    /// string literal rather than a router-building expression.
    ArcM012,

    /// A `#[request_cache]` attribute argument is invalid or missing.
    ///
    /// Emitted when `#[request_cache]` is missing the required
    /// `name = "..."` or `key = "..."` argument, has an unknown key, a
    /// wrong value type, or supplies both `key` and `keys`.
    ArcM013,

    /// A `#[request_cache]` function signature is invalid.
    ///
    /// Emitted when `#[request_cache]` is applied to a non-`pub`,
    /// non-`async` function, or a function with no return type annotation,
    /// or a non-function item.
    ArcM014,
}

impl MacroErrorCode {
    /// Returns the diagnostic label, e.g. `"error[ARC-M001]"`.
    pub fn label(self) -> &'static str {
        match self {
            Self::ArcM001 => "error[ARC-M001]",
            Self::ArcM002 => "error[ARC-M002]",
            Self::ArcM003 => "error[ARC-M003]",
            Self::ArcM004 => "error[ARC-M004]",
            Self::ArcM005 => "error[ARC-M005]",
            Self::ArcM006 => "error[ARC-M006]",
            Self::ArcM007 => "error[ARC-M007]",
            Self::ArcM008 => "error[ARC-M008]",
            Self::ArcM009 => "error[ARC-M009]",
            Self::ArcM010 => "error[ARC-M010]",
            Self::ArcM011 => "error[ARC-M011]",
            Self::ArcM012 => "error[ARC-M012]",
            Self::ArcM013 => "error[ARC-M013]",
            Self::ArcM014 => "error[ARC-M014]",
        }
    }
}

/// A typed diagnostic error from an Arcature macro.
///
/// Carries a [`MacroErrorCode`] for greppable diagnostics and a `syn::Error`
/// for span-accurate error location. Created via [`MacroError::new`] (for
/// macro-specific validation failures) or [`MacroError::from_syn`] (when
/// wrapping a `syn::Error` from a `parse2` call).
#[derive(Debug)]
pub struct MacroError {
    code: MacroErrorCode,
    inner: syn::Error,
}

impl MacroError {
    /// Creates a new diagnostic error at `span` with the given code and
    /// message. The rendered output is `error[ARC-MNNN]: <message>`.
    pub fn new(code: MacroErrorCode, span: proc_macro2::Span, message: impl Into<String>) -> Self {
        let rendered = format!("{}: {}", code.label(), message.into());
        Self {
            code,
            inner: syn::Error::new(span, rendered),
        }
    }

    /// Wraps a `syn::Error` from a parse failure, prefixing it with the
    /// Arcature error code so the user sees a consistent diagnostic format.
    pub fn from_syn(code: MacroErrorCode, err: syn::Error) -> Self {
        let span = err.span();
        let rendered = format!("{}: {}", code.label(), err);
        Self {
            code,
            inner: syn::Error::new(span, rendered),
        }
    }

    /// Returns the error code.
    pub fn code(&self) -> MacroErrorCode {
        self.code
    }

    /// Converts the error into a `compile_error!` token stream. The thin
    /// `lib.rs` entrypoint calls this to emit the error as a compiler
    /// diagnostic rather than panicking.
    pub fn to_compile_error(&self) -> TokenStream {
        self.inner.to_compile_error()
    }
}

impl From<MacroError> for TokenStream {
    fn from(err: MacroError) -> TokenStream {
        err.to_compile_error()
    }
}

/// The result type every Arcature macro implementation returns.
pub type MacroResult = Result<TokenStream, MacroError>;

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;

    #[test]
    fn labels_are_stable() {
        assert_eq!(MacroErrorCode::ArcM001.label(), "error[ARC-M001]");
        assert_eq!(MacroErrorCode::ArcM002.label(), "error[ARC-M002]");
        assert_eq!(MacroErrorCode::ArcM003.label(), "error[ARC-M003]");
    }

    #[test]
    fn codes_are_copy_eq() {
        let a = MacroErrorCode::ArcM001;
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn new_error_renders_code_and_message() {
        let err = MacroError::new(MacroErrorCode::ArcM002, Span::call_site(), "bad attribute");
        let tokens_str = err.to_compile_error().to_string();
        assert!(
            tokens_str.contains("ARC-M002"),
            "expected ARC-M002 in output, got: {tokens_str}"
        );
        assert!(
            tokens_str.contains("bad attribute"),
            "expected 'bad attribute' in output, got: {tokens_str}"
        );
    }

    #[test]
    fn from_syn_preserves_span_and_adds_code() {
        let syn_err = syn::Error::new(Span::call_site(), "unexpected token");
        let err = MacroError::from_syn(MacroErrorCode::ArcM001, syn_err);
        let tokens_str = err.to_compile_error().to_string();
        assert!(
            tokens_str.contains("ARC-M001"),
            "expected ARC-M001 in output, got: {tokens_str}"
        );
        assert!(
            tokens_str.contains("unexpected token"),
            "expected 'unexpected token' in output, got: {tokens_str}"
        );
    }
}
