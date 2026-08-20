//! `#[provider]` -- declares a struct as an Arcature provider.
//!
//! A **provider** is a long-lived application resource constructed during
//! startup -- a Stripe client, a search client, a signer. The macro
//! generates only `impl ::arcature::DxComponent` (the static `NAME` for
//! `arc services` inspection). The developer writes `impl Provider` by
//! hand -- the `Error` type, `DEPS`, and the init logic are all business
//! behavior that the macro must not hide.
//!
//! ## Syntax
//!
//! ```ignore
//! #[provider]
//! pub struct StripeClient { client: HttpClient }
//! ```
//!
//! The struct must be a plain struct (named or tuple). Unit structs,
//! enums, and unions are rejected with `error[ARC-M006]`.
//!
//! ## Custom name
//!
//! ```ignore
//! #[provider(name = "Stripe")]
//! pub struct StripeClient { client: HttpClient }
//! ```
//!
//! ## Why the macro is thin
//!
//! `Provider` carries `type Error` and `const DEPS` -- these depend on the
//! provider's init logic and dependency graph, which the macro cannot
//! infer from the struct definition. Generating an empty `impl Provider`
//! would force the developer to override it, and generating a guessed
//! `Error`/`DEPS` would be fake. So the macro generates only the name
//! metadata, and the developer writes `impl Provider` explicitly:
//!
//! ```ignore
//! #[provider]
//! pub struct StripeClient { client: HttpClient }
//!
//! impl ::arcature::Provider for StripeClient {
//!     type Error = StripeInitError;
//!     const DEPS: &'static [&'static str] = &[];
//! }
//! ```
//!
//! One file, one macro: this is the entirety of the `#[provider]`
//! expansion.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};

/// The implementation of `#[provider]`. Parses the attribute arguments and
/// struct, then expands the struct with `impl DxComponent`.
pub fn provider(attr: TokenStream, item: TokenStream) -> MacroResult {
    let args: ProviderArgs =
        syn::parse2(attr).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM006, e))?;

    let item_struct: syn::ItemStruct =
        syn::parse2(item).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;

    // A provider must carry some state (a client handle, config, etc.).
    // Named and tuple structs are accepted; unit structs are not.
    if matches!(item_struct.fields, syn::Fields::Unit) {
        return Err(MacroError::new(
            MacroErrorCode::ArcM006,
            item_struct.span(),
            "#[provider] requires a struct with fields (named or tuple), \
             not a unit struct, enum, or union.",
        ));
    }

    let struct_name = &item_struct.ident;
    let (impl_generics, ty_generics, where_clause) = item_struct.generics.split_for_impl();

    let name_lit = args.name.unwrap_or_else(|| struct_name.to_string());

    Ok(quote! {
        #item_struct

        impl #impl_generics ::arcature::DxComponent for #struct_name #ty_generics #where_clause {
            const NAME: &'static str = #name_lit;
        }
    })
}

/// The parsed `#[provider(...)]` attribute arguments.
struct ProviderArgs {
    name: Option<String>,
}

impl Parse for ProviderArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name: Option<String> = None;

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            let _: syn::Token![=] = input.parse()?;

            match ident.to_string().as_str() {
                "name" => {
                    let lit: syn::LitStr = input.parse()?;
                    name = Some(lit.value());
                }
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("unknown `#[provider]` argument `{other}`; expected `name`"),
                    ));
                }
            }

            let _ = input.parse::<syn::Token![,]>();
        }

        Ok(ProviderArgs { name })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_dx_component_impl() {
        let expanded = provider(
            quote! {},
            quote! { pub struct StripeClient { client: HttpClient } },
        )
        .unwrap()
        .to_string();

        assert!(
            expanded.contains("DxComponent"),
            "missing DxComponent: {expanded}"
        );
        assert!(
            expanded.contains("\"StripeClient\""),
            "wrong NAME: {expanded}"
        );
        // The macro does NOT generate Service, Resolve, or Provider impls --
        // the developer writes `impl Provider` by hand.
        assert!(
            !expanded.contains("impl :: arcature :: Service"),
            "should not generate Service: {expanded}"
        );
        assert!(
            !expanded.contains("impl :: arcature :: Resolve"),
            "should not generate Resolve: {expanded}"
        );
        assert!(
            !expanded.contains("impl :: arcature :: Provider"),
            "should not generate Provider: {expanded}"
        );
    }

    #[test]
    fn accepts_tuple_struct() {
        let expanded = provider(quote! {}, quote! { pub struct StripeClient(HttpClient); })
            .unwrap()
            .to_string();
        assert!(
            expanded.contains("DxComponent"),
            "missing DxComponent: {expanded}"
        );
    }

    #[test]
    fn uses_name_override() {
        let expanded = provider(
            quote! { name = "Stripe" },
            quote! { pub struct StripeClient { client: HttpClient } },
        )
        .unwrap()
        .to_string();
        assert!(
            expanded.contains("\"Stripe\""),
            "expected Stripe NAME: {expanded}"
        );
        assert!(
            !expanded.contains("\"StripeClient\""),
            "should not use type name: {expanded}"
        );
    }

    #[test]
    fn rejects_unit_struct() {
        let err = provider(quote! {}, quote! { pub struct StripeClient; }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM006);
    }

    #[test]
    fn rejects_enum() {
        let err = provider(quote! {}, quote! { enum Status { Active } }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM001);
    }

    #[test]
    fn rejects_unknown_argument() {
        let err = provider(quote! { foo = 42 }, quote! { pub struct S { c: C } }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM006);
    }
}
