//! `#[request]` — a validated request struct (with `#[validate(...)]` rules).
//!
//! An attribute macro applied to a named struct. Prepends
//! `#[derive(::arcature::validator::Validate)]` so the struct is validated by
//! the `validator` crate's `#[validate(...)]` field attributes. The user must
//! also derive `Deserialize` (the macro does not add it to avoid duplicate
//! derives).
//!
//! One file, one macro: this is the entirety of the `#[request]` expansion.

use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemStruct, parse_macro_input};

/// The `#[request]` attribute macro.
pub fn request(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut item_struct = parse_macro_input!(item as ItemStruct);

    let derive_attr: syn::Attribute = syn::parse_quote! {
        #[derive(::arcature::validator::Validate)]
    };
    // Tell the validator derive to emit code against Arcature's re-export of
    // the `validator` crate, so downstream apps don't need `validator` as a
    // direct dependency. The `#[validate(...)]` helper attribute MUST come
    // after `#[derive(Validate)]` (Rust requires helper attributes to follow
    // the derive that introduces them).
    let crate_attr: syn::Attribute = syn::parse_quote! {
        #[validate(crate = "::arcature::validator")]
    };
    item_struct.attrs.insert(0, derive_attr);
    item_struct.attrs.insert(1, crate_attr);

    quote! { #item_struct }.into()
}
