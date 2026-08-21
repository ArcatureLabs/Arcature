//! `#[middleware]` -- turns an async function into a middleware value.
//!
//! Arcature's pipeline takes `Middleware` *values*, not bare functions: a
//! middleware is a `Clone` type whose `handle` returns a boxed future. That
//! contract is what lets `routes!` write `middleware: [RequireAuth]` and let
//! groups fold their layers. Writing it by hand for a function that only ever
//! needs `(request, next)` is pure plumbing -- a unit struct, a `Clone`
//! derive, and a `Box::pin` around the call.
//!
//! `#[middleware]` writes exactly that plumbing and nothing else. The
//! function is emitted unchanged next to it, so the behaviour stays a normal
//! `async fn` the reader can follow and call directly in a test.
//!
//! ## Syntax
//!
//! ```ignore
//! // Not compiled: `arcature-macros` cannot depend on `arcature`
//! // (that is the cycle `lib.rs` describes), so an example naming
//! // Arcature items has nothing here to compile against.
//! #[middleware]
//! pub async fn require_auth(request: Request, next: Next) -> Result<Response> {
//!     next.run(request).await.pipe(Ok)
//! }
//! ```
//!
//! generates a `pub struct RequireAuth` implementing `Middleware`, usable as
//! `middleware: [RequireAuth]` in a `routes!` group or resource.
//!
//! The generated type's name is the function's name in PascalCase. Pass an
//! explicit name to override it: `#[middleware(RequireAdmin)]`.
//!
//! The function must be `pub async fn` with a return type; other shapes
//! produce `error[ARC-M007]`. A non-function item produces `error[ARC-M001]`,
//! and an attribute argument that is not a single identifier produces
//! `error[ARC-M009]`.
//!
//! One file, one macro: this is the entirety of the `#[middleware]`
//! expansion.

use proc_macro2::TokenStream;
use quote::quote;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};
use crate::signature::validate_public_async_fn;
use crate::util::to_pascal_case;

/// The implementation of `#[middleware]`.
pub fn middleware(attr: TokenStream, item: TokenStream) -> MacroResult {
    let item_fn: syn::ItemFn =
        syn::parse2(item).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;

    validate_public_async_fn(&item_fn, MacroErrorCode::ArcM007, "#[middleware]")?;

    let fn_ident = item_fn.sig.ident.clone();
    let type_ident = type_name(attr, &fn_ident)?;
    let visibility = item_fn.vis.clone();
    let doc = format!("The [`{fn_ident}`] middleware, as a pipeline value.");

    Ok(quote! {
        #item_fn

        #[doc = #doc]
        #[derive(Clone, Copy, Debug, Default)]
        #visibility struct #type_ident;

        impl ::arcature::Middleware for #type_ident {
            fn handle(
                &self,
                request: ::arcature::routing::Request,
                next: ::arcature::Next,
            ) -> ::std::pin::Pin<
                ::std::boxed::Box<
                    dyn ::std::future::Future<
                        Output = ::arcature::Result<::arcature::routing::Response>,
                    > + ::std::marker::Send,
                >,
            > {
                ::std::boxed::Box::pin(#fn_ident(request, next))
            }
        }
    })
}

/// Resolves the generated type's name: the attribute argument when given,
/// otherwise the function name in PascalCase.
fn type_name(attr: TokenStream, fn_ident: &syn::Ident) -> Result<syn::Ident, MacroError> {
    if attr.is_empty() {
        return Ok(syn::Ident::new(
            &to_pascal_case(&fn_ident.to_string()),
            fn_ident.span(),
        ));
    }
    syn::parse2::<syn::Ident>(attr).map_err(|e| {
        MacroError::new(
            MacroErrorCode::ArcM009,
            e.span(),
            "`#[middleware]` takes an optional type name, e.g. `#[middleware(RequireAdmin)]`",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(attr: TokenStream, item: TokenStream) -> String {
        middleware(attr, item).unwrap().to_string()
    }

    fn valid_fn() -> TokenStream {
        quote! {
            pub async fn require_auth(
                request: Request,
                next: Next,
            ) -> Result<Response> {
                next.run(request).await
            }
        }
    }

    #[test]
    fn the_function_is_emitted_unchanged() {
        let out = expand(quote! {}, valid_fn());
        assert!(out.contains("pub async fn require_auth"));
    }

    #[test]
    fn the_generated_type_is_the_pascal_case_function_name() {
        let out = expand(quote! {}, valid_fn());
        assert!(out.contains("pub struct RequireAuth"));
        assert!(out.contains("impl :: arcature :: Middleware for RequireAuth"));
    }

    #[test]
    fn the_type_delegates_to_the_function() {
        let out = expand(quote! {}, valid_fn());
        assert!(out.contains("Box :: pin (require_auth (request , next))"));
    }

    #[test]
    fn the_generated_type_is_cloneable() {
        let out = expand(quote! {}, valid_fn());
        assert!(out.contains("# [derive (Clone , Copy , Debug , Default)]"));
    }

    #[test]
    fn an_explicit_name_overrides_the_default() {
        let out = expand(quote! { RequireAdmin }, valid_fn());
        assert!(out.contains("pub struct RequireAdmin"));
        assert!(!out.contains("pub struct RequireAuth"));
    }

    #[test]
    fn the_type_inherits_the_function_visibility() {
        let out = expand(quote! {}, valid_fn());
        assert!(out.contains("pub struct RequireAuth"));
    }

    #[test]
    fn rejects_non_pub_fn() {
        let err = middleware(
            quote! {},
            quote! { async fn require_auth(r: Request, n: Next) -> Result<Response> { todo!() } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM007);
    }

    #[test]
    fn rejects_non_async_fn() {
        let err = middleware(
            quote! {},
            quote! { pub fn require_auth(r: Request, n: Next) -> Result<Response> { todo!() } },
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

    #[test]
    fn rejects_a_non_ident_attribute() {
        let err = middleware(quote! { "RequireAdmin" }, valid_fn()).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM009);
    }
}
