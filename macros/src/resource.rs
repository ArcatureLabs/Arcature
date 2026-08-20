//! `#[resource]` -- declares a browser-safe API resource.
//!
//! Emits three things from the annotated struct:
//!
//! 1. The struct unchanged, with a `#[derive(::arcature::Serialize)]` that
//!    satisfies the `ClientData: Serialize` supertrait.
//! 2. `impl ::arcature::inertia::ClientData` -- the explicit
//!    browser-exposure opt-in, whose `exposure_schema()` is built from the
//!    struct's named fields.
//! 3. `impl ::arcature::ResourceMetadata` -- the same fields as a
//!    `&'static [::arcature::FieldShape]` const, which `routes!` resolves
//!    when a route declares `query: T` (or `query: Vec<T>`), baking the
//!    typed response shape into the `RouteDescriptor`.
//!
//! ## Syntax
//!
//! ```ignore
//! #[resource]
//! pub struct UserResource {
//!     pub id: String,
//!     pub name: String,
//!     pub avatar: Option<AvatarResource>,
//! }
//! ```
//!
//! Database models and response resources are different concepts. A SeaORM
//! entity is NOT a `#[resource]`; application code converts explicitly via
//! `impl From<User> for UserResource`.
//!
//! ## Client Exposure Firewall
//!
//! `Serialize` does not imply browser-safe. A field type that is not a
//! recognised primitive maps to `PropsSchema::nested::<FieldType>`, which
//! requires `FieldType: ClientData`, so an internal domain model nested
//! inside a resource fails to compile.
//!
//! Unlike `#[page]`, `#[resource]` generates no `PAGE_CONTRACT`: resources
//! are values nested *inside* page props, not pages themselves.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};
use crate::field_shape::{collect_field_shapes, emit_field_shape_slice};
use crate::schema::map_field;

/// The implementation of `#[resource]`. Called by the thin `lib.rs`
/// entrypoint. Returns a [`MacroError`] (converted to `compile_error!` by
/// the entrypoint) on failure -- never panics.
pub fn resource(attr: TokenStream, item: TokenStream) -> MacroResult {
    if !attr.is_empty() {
        return Err(MacroError::new(
            MacroErrorCode::ArcM009,
            proc_macro2::Span::call_site(),
            format!("#[resource] takes no arguments (got: {attr})"),
        ));
    }

    let item_struct: syn::ItemStruct =
        syn::parse2(item).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;

    let syn::Fields::Named(named) = &item_struct.fields else {
        return Err(MacroError::new(
            MacroErrorCode::ArcM002,
            item_struct.fields.span(),
            "#[resource] requires a struct with named fields \
             (e.g. `struct Foo { field: Type }`)",
        ));
    };

    let field_chains = named
        .named
        .iter()
        .map(|field| {
            let name = field
                .ident
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default();
            map_field(&name, &field.ty)
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Resources carry no `#[validate(...)]` attributes, so every field's
    // `validates` slice comes back empty.
    let fields_slice = emit_field_shape_slice(&collect_field_shapes(&item_struct)?);

    let struct_name = &item_struct.ident;
    let (impl_generics, ty_generics, where_clause) = item_struct.generics.split_for_impl();

    Ok(quote! {
        #[derive(::arcature::Serialize)]
        #item_struct

        impl #impl_generics ::arcature::inertia::ClientData for #struct_name #ty_generics
        #where_clause
        {
            fn exposure_schema() -> ::arcature::inertia::PropsSchema {
                ::arcature::inertia::PropsSchema::new()
                    #( #field_chains )*
            }
        }

        impl #impl_generics ::arcature::ResourceMetadata for #struct_name #ty_generics
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
        resource(TokenStream::new(), item).unwrap().to_string()
    }

    #[test]
    fn generates_a_client_data_impl() {
        let s = expand(quote! {
            pub struct UserResource {
                pub id: String,
                pub name: String,
            }
        });
        assert!(
            s.contains(":: arcature :: inertia :: ClientData"),
            "got: {s}"
        );
        assert!(s.contains("exposure_schema"), "got: {s}");
    }

    #[test]
    fn adds_the_serialize_derive() {
        let s = expand(quote! { pub struct UserResource { pub id: String } });
        assert!(s.contains("Serialize"), "got: {s}");
    }

    #[test]
    fn does_not_generate_a_page_contract() {
        let s = expand(quote! { pub struct UserResource { pub id: String } });
        assert!(!s.contains("PAGE_CONTRACT"), "got: {s}");
    }

    #[test]
    fn generates_a_resource_metadata_impl() {
        let s = expand(quote! {
            pub struct LinkResource {
                pub id: String,
                pub tags: Vec<String>,
            }
        });
        assert!(s.contains(":: arcature :: ResourceMetadata"), "got: {s}");
        assert!(s.contains("FieldShape"), "got: {s}");
        assert!(s.contains("\"id\""), "got: {s}");
        assert!(s.contains("\"Vec<String>\""), "got: {s}");
    }

    #[test]
    fn resource_fields_carry_no_validate_rules() {
        let s = expand(quote! { pub struct UserResource { pub id: String } });
        assert!(s.contains("validates : & []"), "got: {s}");
    }

    #[test]
    fn nested_named_field_types_go_through_the_firewall() {
        let s = expand(quote! {
            pub struct PostResource {
                pub author: UserResource,
                pub tags: Vec<TagResource>,
            }
        });
        assert!(s.contains("nested :: < UserResource >"), "got: {s}");
        assert!(s.contains("nested_array :: < TagResource >"), "got: {s}");
    }

    #[test]
    fn rejects_attribute_arguments() {
        let err = resource(
            quote! { name = "x" },
            quote! { pub struct R { pub id: String } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM009);
    }

    #[test]
    fn rejects_enum_item() {
        let err = resource(TokenStream::new(), quote! { enum Status { Active } }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM001);
    }

    #[test]
    fn rejects_tuple_struct() {
        let err = resource(TokenStream::new(), quote! { struct Tuple(String); }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }

    #[test]
    fn rejects_unit_struct() {
        let err = resource(TokenStream::new(), quote! { struct Unit; }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }
}
