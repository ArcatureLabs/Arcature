//! `#[provider]` -- declares a struct as an Arcature provider.
//!
//! A **provider** is a long-lived application resource constructed during
//! startup -- a Stripe client, a search client, a signer. The macro
//! generates two impls:
//!
//! 1. `impl ::arcature::DxComponent` -- the static `NAME`.
//! 2. `impl ::arcature::Provider` -- the typed init failure (`Error`) and
//!    the dependency names (`DEPS`).
//!
//! ## Syntax
//!
//! ```ignore
//! #[provider(error = StripeInitError, deps = [Db, Config])]
//! pub struct StripeClient { client: HttpClient }
//! ```
//!
//! The struct must be a plain struct (named or tuple). Unit structs,
//! enums, and unions are rejected with `error[ARC-M006]`, as is an unknown
//! argument.
//!
//! ## The arguments are the impl
//!
//! `Provider` carries exactly two things the struct definition cannot
//! state: how init can fail, and what init depends on. Rather than leave
//! the impl to be written by hand -- which made `#[provider]` a name
//! generator that claimed to declare providers -- the macro takes both as
//! arguments and writes the impl from them. Everything it emits is
//! something the developer said out loud.
//!
//! `error` defaults to [`core::convert::Infallible`]: a provider whose
//! declaration mentions no failure is declaring that its init cannot fail,
//! and `Infallible` is that statement in the type system -- a
//! `Result<Self, Infallible>` constructor cannot return `Err`. `deps`
//! defaults to empty, matching the trait's own default.
//!
//! Init itself stays hand-written. The constructor's signature is
//! provider-specific (`&Db`, a config struct, an HTTP client) and calling
//! an external service is business behavior, not mechanical plumbing, so
//! the trait does not carry it and the macro does not invent it.
//!
//! ## Custom name
//!
//! ```ignore
//! #[provider(name = "Stripe")]
//! pub struct StripeClient { client: HttpClient }
//! ```
//!
//! One file, one macro: this is the entirety of the `#[provider]`
//! expansion.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};

/// The implementation of `#[provider]`. Parses the attribute arguments and
/// struct, then expands the struct with `impl DxComponent` and
/// `impl Provider`.
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

    // No declared failure means init cannot fail: `Infallible` says that in
    // the type system rather than inventing an error enum the developer
    // never wrote.
    let error_ty = args.error.map_or_else(
        || quote! { ::core::convert::Infallible },
        |ty| quote! { #ty },
    );

    let deps = args.deps.iter().map(simple_path_name).collect::<Vec<_>>();

    Ok(quote! {
        #item_struct

        impl #impl_generics ::arcature::DxComponent for #struct_name #ty_generics #where_clause {
            const NAME: &'static str = #name_lit;
        }

        impl #impl_generics ::arcature::Provider for #struct_name #ty_generics #where_clause {
            type Error = #error_ty;
            const DEPS: &'static [&'static str] = &[#(#deps),*];
        }
    })
}

/// Renders a dependency path as its simple type name (`database::Db` ->
/// `"Db"`), so a fully qualified path in the attribute still joins to the
/// graph node the resource registered under.
fn simple_path_name(path: &syn::Path) -> String {
    path.segments.last().map_or_else(
        || path.to_token_stream().to_string(),
        |s| s.ident.to_string(),
    )
}

/// The parsed `#[provider(...)]` attribute arguments.
struct ProviderArgs {
    /// The inspection name, from `name = "..."`; defaults to the type name.
    name: Option<String>,
    /// The typed init failure, from `error = Path`; defaults to
    /// `Infallible`.
    error: Option<syn::Path>,
    /// The dependency type names, from `deps = [A, B]`.
    deps: Vec<syn::Path>,
}

impl Parse for ProviderArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ProviderArgs {
            name: None,
            error: None,
            deps: Vec::new(),
        };

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            let _: syn::Token![=] = input.parse()?;

            match ident.to_string().as_str() {
                "name" => args.name = Some(input.parse::<syn::LitStr>()?.value()),
                "error" => args.error = Some(input.parse()?),
                "deps" => args.deps = parse_dep_list(input)?,
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "unknown `#[provider]` argument `{other}`; \
                             expected `name`, `error`, or `deps`"
                        ),
                    ));
                }
            }

            let _ = input.parse::<syn::Token![,]>();
        }

        Ok(args)
    }
}

/// Parses `[A, B::C]` into the dependency paths it lists.
fn parse_dep_list(input: ParseStream) -> syn::Result<Vec<syn::Path>> {
    let content;
    syn::bracketed!(content in input);
    Ok(
        syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated(&content)?
            .into_iter()
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(attr: TokenStream, item: TokenStream) -> String {
        provider(attr, item).unwrap().to_string()
    }

    #[test]
    fn generates_dx_component_impl() {
        let s = expand(
            quote! {},
            quote! { pub struct StripeClient { client: HttpClient } },
        );
        assert!(s.contains("DxComponent"), "missing DxComponent: {s}");
        assert!(s.contains("\"StripeClient\""), "wrong NAME: {s}");
    }

    #[test]
    fn generates_a_provider_impl() {
        let s = expand(
            quote! {},
            quote! { pub struct StripeClient { client: HttpClient } },
        );
        assert!(
            s.contains(":: arcature :: Provider for StripeClient"),
            "missing Provider: {s}"
        );
    }

    #[test]
    fn an_undeclared_failure_becomes_infallible() {
        let s = expand(
            quote! {},
            quote! { pub struct StripeClient { client: HttpClient } },
        );
        assert!(
            s.contains("type Error = :: core :: convert :: Infallible"),
            "got: {s}"
        );
        assert!(
            s.contains("DEPS : & 'static [& 'static str] = & []"),
            "got: {s}"
        );
    }

    #[test]
    fn uses_the_declared_error_type() {
        let s = expand(
            quote! { error = stripe::StripeInitError },
            quote! { pub struct StripeClient { client: HttpClient } },
        );
        assert!(
            s.contains("type Error = stripe :: StripeInitError"),
            "got: {s}"
        );
    }

    #[test]
    fn records_the_declared_dependencies_by_simple_name() {
        let s = expand(
            quote! { deps = [Db, config::StripeConfig] },
            quote! { pub struct StripeClient { client: HttpClient } },
        );
        assert!(s.contains("\"Db\""), "got: {s}");
        assert!(s.contains("\"StripeConfig\""), "got: {s}");
    }

    #[test]
    fn does_not_generate_service_or_resolve_impls() {
        let s = expand(
            quote! {},
            quote! { pub struct StripeClient { client: HttpClient } },
        );
        assert!(!s.contains("impl :: arcature :: Service"), "got: {s}");
        assert!(!s.contains("impl :: arcature :: Resolve"), "got: {s}");
    }

    #[test]
    fn accepts_tuple_struct() {
        let s = expand(quote! {}, quote! { pub struct StripeClient(HttpClient); });
        assert!(s.contains("DxComponent"), "missing DxComponent: {s}");
    }

    #[test]
    fn uses_name_override() {
        let s = expand(
            quote! { name = "Stripe" },
            quote! { pub struct StripeClient { client: HttpClient } },
        );
        assert!(s.contains("\"Stripe\""), "expected Stripe NAME: {s}");
        assert!(
            !s.contains("\"StripeClient\""),
            "should not use type name: {s}"
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
