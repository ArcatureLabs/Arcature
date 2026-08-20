//! `#[derive(Event)]` — a typed in-process event for the `Dispatcher`.
//!
//! Emits `impl DxComponent { const NAME = <type name> }` and `impl Event {}`.
//!
//! One file, one macro: this is the entirety of the `#[derive(Event)]`
//! expansion.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::DeriveInput;

/// The `#[derive(Event)]` derive macro entry point.
pub fn derive_event(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
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
