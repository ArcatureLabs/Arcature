//! `page!(Type { fields })` -- constructs a `Page<T>` response with a
//! compile-time `ClientData` assertion.
//!
//! ## Syntax
//!
//! ```ignore
//! page!(ShowUserPage {
//!     user: UserResource::from(user),
//! })
//! ```
//!
//! Expands to, conceptually:
//!
//! ```ignore
//! {
//!     const fn _assert_client_data<T: ::arcature::inertia::ClientData>() {}
//!     _assert_client_data::<ShowUserPage>();
//!     ::arcature::Page(ShowUserPage { user: UserResource::from(user) })
//! }
//! ```
//!
//! The assertion is the point: a plain `Serialize` type that is not a
//! declared `#[page]`/`#[resource]` fails to compile at the call site rather
//! than silently reaching the browser.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};

/// The implementation of `page!`. Called by the thin `lib.rs` entrypoint.
/// Returns a [`MacroError`] (converted to `compile_error!` by the
/// entrypoint) on failure -- never panics.
pub fn page_macro(input: TokenStream) -> MacroResult {
    // `Type { fields }` is valid Rust expression syntax (a struct
    // constructor), so parse it as one and read the type back off it.
    let expr: syn::Expr =
        syn::parse2(input).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;

    let syn::Expr::Struct(expr_struct) = &expr else {
        return Err(MacroError::new(
            MacroErrorCode::ArcM002,
            expr.span(),
            "page! requires a struct constructor expression, \
             e.g. `page!(ShowUserPage { ... })`",
        ));
    };
    let type_path = &expr_struct.path;

    Ok(quote! {{
        // A plain Serialize type that is not a declared page/resource
        // fails here rather than reaching the browser.
        const fn _assert_client_data<T: ::arcature::inertia::ClientData>() {}
        _assert_client_data::<#type_path>();
        ::arcature::Page(#expr)
    }})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_the_constructor_in_a_page() {
        let s = page_macro(quote! { ShowUserPage { user: user_resource } })
            .unwrap()
            .to_string();
        assert!(s.contains(":: arcature :: Page"), "got: {s}");
        assert!(s.contains("ShowUserPage"), "got: {s}");
        assert!(s.contains("user_resource"), "got: {s}");
    }

    #[test]
    fn asserts_the_type_implements_client_data() {
        let s = page_macro(quote! { HomePage { title: t } })
            .unwrap()
            .to_string();
        assert!(
            s.contains("_assert_client_data :: < HomePage >"),
            "got: {s}"
        );
        assert!(
            s.contains(":: arcature :: inertia :: ClientData"),
            "got: {s}"
        );
    }

    #[test]
    fn accepts_a_qualified_type_path() {
        let s = page_macro(quote! { pages::HomePage { title: t } })
            .unwrap()
            .to_string();
        assert!(
            s.contains("_assert_client_data :: < pages :: HomePage >"),
            "got: {s}"
        );
    }

    #[test]
    fn accepts_an_empty_struct_literal() {
        let s = page_macro(quote! { BlankPage {} }).unwrap().to_string();
        assert!(
            s.contains("_assert_client_data :: < BlankPage >"),
            "got: {s}"
        );
    }

    #[test]
    fn rejects_a_non_struct_expression() {
        let err = page_macro(quote! { make_page() }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }

    #[test]
    fn rejects_input_that_is_not_an_expression() {
        let err = page_macro(quote! { pub struct Foo {} }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM001);
    }
}
