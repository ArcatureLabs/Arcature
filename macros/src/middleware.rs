//! `#[middleware]` -- annotates a function as Arcature middleware.
//!
//! Validates the function signature (`pub`, `async`, return type present)
//! and passes the function through unchanged so it remains a genuine Axum
//! `from_fn` middleware. The function is wired into the pipeline via
//! `::arcature::from_fn(name)` in the `routes!` macro's `middleware:` lists
//! or via the application builder.
//!
//! ## Syntax
//!
//! ```ignore
//! #[middleware]
//! pub async fn require_auth(
//!     auth: Auth<User>,
//!     request: Request,
//!     next: Next,
//! ) -> Response {
//!     next.run(request).await
//! }
//! ```
//!
//! The function must be `pub async fn` with a return type. Non-pub,
//! non-async, or missing-return-type signatures produce `error[ARC-M007]`.
//! Non-function inputs produce `error[ARC-M001]`.
//!
//! ## No wrapper is generated
//!
//! The function is already a genuine Axum middleware; the macro removes no
//! mechanical plumbing beyond the signature check, and hides no behavior.
//! Its value is the compile-time contract: a function marked
//! `#[middleware]` is guaranteed to have the shape the pipeline requires.
//!
//! One file, one macro: this is the entirety of the `#[middleware]`
//! expansion.

use proc_macro2::TokenStream;
use quote::quote;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};
use crate::signature::validate_public_async_fn;

/// The implementation of `#[middleware]`. Validates the signature and
/// emits the function unchanged.
pub fn middleware(_attr: TokenStream, item: TokenStream) -> MacroResult {
    let item_fn: syn::ItemFn =
        syn::parse2(item).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;

    validate_public_async_fn(&item_fn, MacroErrorCode::ArcM007, "#[middleware]")?;

    Ok(quote! { #item_fn })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_valid_middleware_through() {
        let expanded = middleware(
            quote! {},
            quote! {
                pub async fn require_auth(request: Request, next: Next) -> Response {
                    next.run(request).await
                }
            },
        )
        .unwrap()
        .to_string();
        assert!(expanded.contains("require_auth"), "fn missing: {expanded}");
        assert!(expanded.contains("async"), "async missing: {expanded}");
    }

    #[test]
    fn rejects_non_pub_fn() {
        let err = middleware(
            quote! {},
            quote! { async fn require_auth(r: Request, n: Next) -> Response { todo!() } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM007);
    }

    #[test]
    fn rejects_non_async_fn() {
        let err = middleware(
            quote! {},
            quote! { pub fn require_auth(r: Request, n: Next) -> Response { todo!() } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM007);
    }

    #[test]
    fn rejects_missing_return_type() {
        let err = middleware(
            quote! {},
            quote! { pub async fn require_auth(r: Request, n: Next) { } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM007);
    }

    #[test]
    fn rejects_non_function() {
        let err = middleware(quote! {}, quote! { pub struct NotAFunction; }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM001);
    }
}
