//! `#[request]` -- a validated request struct (with `#[validate(...)]`
//! rules).
//!
//! An attribute macro applied to a named struct. It emits three things:
//!
//! 1. The struct with `#[derive(::arcature::validator::Validate)]`
//!    prepended, so the `validator` crate enforces the `#[validate(...)]`
//!    field attributes. The user still derives `Deserialize` themselves (the
//!    macro does not add it, to avoid a duplicate derive).
//! 2. `impl ::arcature::Request` -- the marker that makes the type
//!    first-class in tooling.
//! 3. `impl ::arcature::RequestMetadata` -- the struct's fields as a
//!    `&'static [::arcature::FieldShape]` const, which `routes!` resolves
//!    when a route declares `action: T`, baking the action's typed input
//!    shape into the `RouteDescriptor`.
//!
//! One file, one macro: this is the entirety of the `#[request]` expansion.

use proc_macro2::TokenStream;
use quote::quote;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};
use crate::field_shape::{collect_field_shapes, emit_field_shape_slice};

/// The implementation of `#[request]`. Called by the thin `lib.rs`
/// entrypoint. Returns a [`MacroError`] (converted to `compile_error!` by
/// the entrypoint) on failure -- never panics.
pub fn request(_attr: TokenStream, item: TokenStream) -> MacroResult {
    let mut item_struct: syn::ItemStruct =
        syn::parse2(item).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;

    // Collected before the derive is inserted: the field shapes describe the
    // fields the developer wrote, and the added attributes are struct-level.
    let fields_slice = emit_field_shape_slice(&collect_field_shapes(&item_struct)?);

    let derive_attr: syn::Attribute = syn::parse_quote! {
        #[derive(::arcature::validator::Validate)]
    };
    // Tell the validator derive to emit code against Arcature's re-export of
    // the `validator` crate, so downstream apps do not need `validator` as a
    // direct dependency. The `#[validate(...)]` helper attribute MUST come
    // after `#[derive(Validate)]`: Rust requires helper attributes to follow
    // the derive that introduces them.
    let crate_attr: syn::Attribute = syn::parse_quote! {
        #[validate(crate = "::arcature::validator")]
    };
    item_struct.attrs.insert(0, derive_attr);
    item_struct.attrs.insert(1, crate_attr);

    let struct_name = &item_struct.ident;
    let (impl_generics, ty_generics, where_clause) = item_struct.generics.split_for_impl();

    Ok(quote! {
        #item_struct

        impl #impl_generics ::arcature::Request for #struct_name #ty_generics #where_clause {}

        impl #impl_generics ::arcature::RequestMetadata for #struct_name #ty_generics
        #where_clause
        {
            const FIELDS: &'static [::arcature::FieldShape] = #fields_slice;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(item: TokenStream) -> String {
        request(TokenStream::new(), item).unwrap().to_string()
    }

    #[test]
    fn adds_the_validate_derive_and_crate_attribute() {
        let s = expand(quote! { pub struct StoreLinkRequest { pub url: String } });
        assert!(s.contains(":: arcature :: validator :: Validate"), "got: {s}");
        assert!(s.contains("crate = \"::arcature::validator\""), "got: {s}");
    }

    #[test]
    fn implements_the_request_marker() {
        let s = expand(quote! { pub struct StoreLinkRequest { pub url: String } });
        assert!(s.contains(":: arcature :: Request for StoreLinkRequest"), "got: {s}");
    }

    #[test]
    fn implements_request_metadata_with_the_field_shapes() {
        let s = expand(quote! {
            pub struct StoreLinkRequest {
                #[validate(url)]
                pub url: String,
                pub title: Option<String>,
            }
        });
        assert!(s.contains(":: arcature :: RequestMetadata"), "got: {s}");
        assert!(s.contains("\"url\""), "got: {s}");
        assert!(s.contains("\"Option<String>\""), "got: {s}");
    }

    #[test]
    fn carries_the_validate_rules_into_the_field_shapes() {
        let s = expand(quote! {
            pub struct StoreLinkRequest {
                #[validate(length(min = 1, max = 120))]
                pub title: String,
            }
        });
        assert!(s.contains("\"length(min=1, max=120)\""), "got: {s}");
    }

    #[test]
    fn rejects_a_non_struct_item() {
        let err = request(TokenStream::new(), quote! { enum Kind { A } }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM001);
    }

    #[test]
    fn rejects_a_tuple_struct() {
        let err = request(TokenStream::new(), quote! { pub struct R(String); }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }
}
