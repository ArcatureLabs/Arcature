//! `#[model(table = "users")]` — a SeaORM entity model with the query facade.
//!
//! An attribute macro applied to a named struct. Prepends the SeaORM
//! `DeriveEntityModel` derive and the `#[sea_orm(table_name = "...")]`
//! attribute, so the user writes only the fields. The user still annotates the
//! primary key with `#[sea_orm(primary_key)]` on the field. Also emits the
//! `database::Model` impl that binds the struct to the query facade.
//!
//! One file, one macro: this is the entirety of the `#[model]` expansion.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{ItemStruct, Lit, Meta, parse_macro_input};

/// The `#[model(table = "users")]` attribute macro.
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
            Span::call_site(),
            "#[model(table = \"...\")] requires a `table` argument",
        ));
    }
    let meta: Meta = syn::parse(attr)?;
    match meta {
        Meta::NameValue(nv) if nv.path.is_ident("table") => {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: Lit::Str(s), ..
            }) = nv.value
            {
                Ok(s.value())
            } else {
                Err(syn::Error::new(
                    Span::call_site(),
                    "#[model(...)] expects `table = \"name\"`",
                ))
            }
        }
        _ => Err(syn::Error::new(
            Span::call_site(),
            "#[model(...)] expects `table = \"name\"`",
        )),
    }
}
