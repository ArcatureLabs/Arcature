//! `#[controller]` — an Axum controller with route metadata.
//!
//! An attribute macro applied to an `impl` block. Emits the impl unchanged
//! (the methods remain genuine Axum handlers) and validates that each method
//! is `pub async fn` with a return type.
//!
//! One file, one macro: this is the entirety of the `#[controller]` expansion.

use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemImpl, parse_macro_input};

/// The `#[controller]` attribute macro.
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
