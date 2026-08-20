//! `#[derive(Job)]` — a typed background job with a `JobModel` const.
//!
//! A derive macro. Emits:
//! 1. `impl DxComponent { const NAME = <type name> }`
//! 2. `impl Job {}` (the empty marker trait)
//! 3. An inherent `pub const JOB: JobModel<Self> = JobModel::new(kind, version, attempts)`
//!
//! Optional helper attribute `#[job(kind = "...", version = N, attempts = N)]`.
//!
//! One file, one macro: this is the entirety of the `#[derive(Job)]` expansion.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{DeriveInput, LitStr, parse_macro_input};

use crate::util::to_snake_case;

/// The `#[derive(Job)]` derive macro entry point.
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
