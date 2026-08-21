//! `#[derive(DxComponent)]` -- the foundation DX derive.
//!
//! Generates an `impl ::arcature::DxComponent` for the annotated type,
//! providing a static component name used in application-graph inspection
//! without runtime reflection.
//!
//! ## Helper attribute
//!
//! ```ignore
//! #[derive(DxComponent)]
//! #[dx_component(name = "CustomName")]
//! struct MyStruct;
//! ```
//!
//! `name` overrides the generated component name (defaults to the type
//! name). Unknown keys and non-string values produce `ARC-M002`.
//!
//! One file, one macro: this is the entirety of the `#[derive(DxComponent)]`
//! expansion.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};

/// The implementation of `#[derive(DxComponent)]`. Called by the thin
/// `lib.rs` entrypoint. Returns a token stream (the generated impl) or a
/// [`MacroError`] (converted to `compile_error!` by the entrypoint).
pub fn derive(input: TokenStream) -> MacroResult {
    let ast: syn::DeriveInput =
        syn::parse2(input).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;

    let name_override = parse_name_attribute(&ast)?;
    let name = name_override.unwrap_or_else(|| ast.ident.to_string());
    let ident = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics ::arcature::DxComponent for #ident #ty_generics #where_clause {
            const NAME: &'static str = #name;
        }
    })
}

/// Extracts `name = "..."` from the `#[dx_component(...)]` helper attribute.
/// Returns `None` when the attribute is absent.
fn parse_name_attribute(input: &syn::DeriveInput) -> Result<Option<String>, MacroError> {
    let mut name: Option<String> = None;

    for attr in &input.attrs {
        if !attr.path().is_ident("dx_component") {
            continue;
        }

        let list = attr
            .meta
            .require_list()
            .map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM002, e))?;

        let metas = list
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )
            .map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM002, e))?;

        for meta in metas {
            let syn::Meta::NameValue(nv) = meta else {
                return Err(MacroError::new(
                    MacroErrorCode::ArcM002,
                    meta.path().span(),
                    "expected `name = \"...\"`",
                ));
            };
            if !nv.path.is_ident("name") {
                let key = nv
                    .path
                    .get_ident()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                return Err(MacroError::new(
                    MacroErrorCode::ArcM002,
                    nv.path.span(),
                    format!("unknown key `{key}`; expected `name`"),
                ));
            }
            let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = nv.value
            else {
                return Err(MacroError::new(
                    MacroErrorCode::ArcM002,
                    nv.value.span(),
                    "`name` must be a string literal",
                ));
            };
            if name.is_some() {
                return Err(MacroError::new(
                    MacroErrorCode::ArcM002,
                    s.span(),
                    "`name` specified more than once",
                ));
            }
            name = Some(s.value());
        }
    }

    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_generates_impl_with_default_name() {
        let input = quote! { struct MyType; };
        let tokens = derive(input).unwrap().to_string();
        assert!(
            tokens.contains(":: arcature :: DxComponent"),
            "expected ::arcature::DxComponent, got: {tokens}"
        );
        assert!(
            tokens.contains("\"MyType\""),
            "expected \"MyType\", got: {tokens}"
        );
    }

    #[test]
    fn derive_uses_name_override() {
        let input = quote! { #[dx_component(name = "Custom")] struct MyType; };
        let tokens = derive(input).unwrap().to_string();
        assert!(
            tokens.contains("\"Custom\""),
            "expected \"Custom\", got: {tokens}"
        );
        assert!(
            !tokens.contains("\"MyType\""),
            "should not use type name: {tokens}"
        );
    }

    #[test]
    fn derive_handles_generics() {
        let input = quote! { struct Foo<T> { x: T } };
        let tokens = derive(input).unwrap().to_string();
        assert!(
            tokens.contains("DxComponent for Foo"),
            "expected impl DxComponent for Foo, got: {tokens}"
        );
        assert!(
            tokens.contains("< T"),
            "expected generic parameter T, got: {tokens}"
        );
    }

    #[test]
    fn unknown_key_is_arc_m002() {
        let input = quote! { #[dx_component(bogus = 1)] struct Foo; };
        let err = derive(input).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
        let msg = err.to_compile_error().to_string();
        assert!(msg.contains("unknown key"), "got: {msg}");
    }

    #[test]
    fn non_string_name_is_arc_m002() {
        let input = quote! { #[dx_component(name = 42)] struct Foo; };
        let err = derive(input).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
        let msg = err.to_compile_error().to_string();
        assert!(msg.contains("must be a string literal"), "got: {msg}");
    }

    #[test]
    fn duplicate_name_is_arc_m002() {
        let input = quote! { #[dx_component(name = "A", name = "B")] struct Foo; };
        let err = derive(input).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
        let msg = err.to_compile_error().to_string();
        assert!(msg.contains("more than once"), "got: {msg}");
    }
}
