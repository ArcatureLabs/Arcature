//! Proc-macro crate for Arcature.
//!
//! Provides the attribute/derive macros that power the Arcature developer
//! experience:
//! - `#[model(table = "users")]` — a SeaORM entity model with the query facade.
//! - `#[request]` — a validated request struct (with `#[validate(...)]` rules).
//! - `#[controller]` — an Axum controller with route metadata.
//! - `#[derive(Job)]` — a typed background job with a `JobModel` const.
//! - `#[derive(Event)]` — a typed in-process event for the `Dispatcher`.
//!
//! All expansions reference Arcature APIs via absolute `::arcature::` paths
//! that resolve in the downstream app crate. This crate must NOT depend on
//! `arcature` (would create a cycle); it depends only on `syn`, `quote`, and
//! `proc-macro2`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, ItemImpl, ItemStruct, Lit, LitStr, Meta};

// ===========================================================================
// #[model(table = "users")]
// ===========================================================================
//
// An attribute macro applied to a named struct. Prepends the SeaORM
// `DeriveEntityModel` derive and the `#[sea_orm(table_name = "...")]`
// attribute, so the user writes only the fields. The user still annotates
// the primary key with `#[sea_orm(primary_key)]` on the field.

#[proc_macro_attribute]
pub fn model(attr: TokenStream, item: TokenStream) -> TokenStream {
    let table = match parse_model_attr(attr) {
        Ok(t) => t,
        Err(e) => return e.to_compile_error().into(),
    };
    let mut item_struct = parse_macro_input!(item as ItemStruct);

    // Prepend the SeaORM derives and the table_name attribute.
    let derive_attr: syn::Attribute = syn::parse_quote! {
        #[derive(::arcature::sea_orm::DeriveEntityModel, ::arcature::Serialize, ::arcature::Deserialize)]
    };
    let table_attr: syn::Attribute = syn::parse_quote! {
        #[sea_orm(table_name = #table)]
    };
    item_struct.attrs.insert(0, table_attr);
    item_struct.attrs.insert(0, derive_attr);

    let ident = &item_struct.ident;
    quote! {
        #item_struct

        impl ::arcature::database::Model for #ident {
            type Entity = #ident::Entity;
        }
    }
    .into()
}

/// Parse the `table = "..."` argument for `#[model]`.
fn parse_model_attr(attr: TokenStream) -> syn::Result<String> {
    if attr.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[model(table = \"...\")] requires a `table` argument",
        ));
    }
    let meta: Meta = syn::parse(attr)?;
    match meta {
        Meta::NameValue(nv) if nv.path.is_ident("table") => {
            if let syn::Expr::Lit(syn::ExprLit { lit: Lit::Str(s), .. }) = nv.value {
                Ok(s.value())
            } else {
                Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "#[model(...)] expects `table = \"name\"`",
                ))
            }
        }
        _ => Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[model(...)] expects `table = \"name\"`",
        )),
    }
}

// ===========================================================================
// #[request]
// ===========================================================================
//
// An attribute macro applied to a named struct. Prepends
// `#[derive(::arcature::Deserialize, ::arcature::Validate)]` so the struct
// is deserializable and the `validator` crate's `#[validate(...)]` field
// attributes are processed. The struct is emitted unchanged otherwise.

#[proc_macro_attribute]
pub fn request(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut item_struct = parse_macro_input!(item as ItemStruct);

    let derive_attr: syn::Attribute = syn::parse_quote! {
        #[derive(::arcature::Deserialize, ::arcature::Validate)]
    };
    item_struct.attrs.insert(0, derive_attr);

    quote! { #item_struct }.into()
}

// ===========================================================================
// #[controller]
// ===========================================================================
//
// An attribute macro applied to an `impl` block. Emits the impl unchanged
// (the methods remain genuine Axum handlers) and validates that each method
// is `pub async fn` with a return type.

#[proc_macro_attribute]
pub fn controller(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_impl = parse_macro_input!(item as ItemImpl);

    // Validate: each fn must be pub, async, with a return type.
    for item in &item_impl.items {
        if let syn::ImplItem::Fn(method) = item {
            let sig = &method.sig;
            if !matches!(method.vis, syn::Visibility::Public(_)) {
                return syn::Error::new_spanned(
                    &method.sig.ident,
                    "controller methods must be `pub`",
                )
                .to_compile_error()
                .into();
            }
            if sig.asyncness.is_none() {
                return syn::Error::new_spanned(
                    &method.sig.ident,
                    "controller methods must be `async fn`",
                )
                .to_compile_error()
                .into();
            }
            if matches!(sig.output, syn::ReturnType::Default) {
                return syn::Error::new_spanned(
                    &method.sig.ident,
                    "controller methods must have a return type",
                )
                .to_compile_error()
                .into();
            }
        }
    }

    quote! { #item_impl }.into()
}

// ===========================================================================
// #[derive(Job)]
// ===========================================================================
//
// A derive macro. Emits:
// 1. `impl DxComponent { const NAME = <type name> }`
// 2. `impl Job {}` (the empty marker trait)
// 3. An inherent `pub const JOB: JobModel<Self> = JobModel::new(kind, version, attempts)`
//
// Optional helper attribute `#[job(kind = "...", version = N, attempts = N)]`.

#[proc_macro_derive(Job, attributes(job))]
pub fn derive_job(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_job(&input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

fn expand_job(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;
    let name = ident.to_string();

    // Parse the optional #[job(...)] helper attribute.
    let mut kind: Option<String> = None;
    let mut version: Option<i16> = None;
    let mut attempts: Option<u32> = None;

    for attr in &input.attrs {
        if attr.path().is_ident("job") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("kind") {
                    let value = meta.value()?;
                    let s: LitStr = value.parse()?;
                    kind = Some(s.value());
                    Ok(())
                } else if meta.path.is_ident("version") {
                    let value = meta.value()?;
                    let i: syn::LitInt = value.parse()?;
                    version = Some(i.base10_parse().unwrap_or(1));
                    Ok(())
                } else if meta.path.is_ident("attempts") {
                    let value = meta.value()?;
                    let i: syn::LitInt = value.parse()?;
                    attempts = Some(i.base10_parse().unwrap_or(3));
                    Ok(())
                } else {
                    Err(meta.error("unknown #[job(...)] key; expected kind, version, or attempts"))
                }
            })?;
        }
    }

    let kind = kind.unwrap_or_else(|| to_snake_case(&name));
    let version = version.unwrap_or(1);
    let attempts = attempts.unwrap_or(3);

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics ::arcature::DxComponent for #ident #ty_generics #where_clause {
            const NAME: &'static str = #name;
        }
        impl #impl_generics ::arcature::Job for #ident #ty_generics #where_clause {}

        #[doc = "The `JobModel` for this job. Use with `JobRequest::new` for typed enqueue."]
        impl #impl_generics #ident #ty_generics #where_clause {
            pub const JOB: ::arcature::jobs::JobModel<#ident #ty_generics> =
                ::arcature::jobs::JobModel::new(#kind, #version, #attempts);
        }
    })
}

// ===========================================================================
// #[derive(Event)]
// ===========================================================================
//
// Emits `impl DxComponent { const NAME = <type name> }` and `impl Event {}`.

#[proc_macro_derive(Event)]
pub fn derive_event(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_event(&input).into()
}

fn expand_event(input: &DeriveInput) -> TokenStream2 {
    let ident = &input.ident;
    let name = ident.to_string();
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    quote! {
        impl #impl_generics ::arcature::DxComponent for #ident #ty_generics #where_clause {
            const NAME: &'static str = #name;
        }
        impl #impl_generics ::arcature::Event for #ident #ty_generics #where_clause {}
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Convert a PascalCase name to snake_case.
fn to_snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 8);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(ch.to_ascii_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}
