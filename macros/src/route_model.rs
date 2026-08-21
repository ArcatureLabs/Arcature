//! `#[route_model(...)]` -- declares a type as a route-bindable model.
//!
//! Emits the struct unchanged plus an `impl ::arcature::RouteModel` that
//! loads the model by a route parameter through SeaORM's
//! `Entity::find_by_id(key).one(db.orm())`. No ORM rewrite: the generated
//! code calls SeaORM directly through `::arcature::database::Db`.
//!
//! ## Syntax
//!
//! ```ignore
//! // Not compiled: `arcature-macros` cannot depend on `arcature`
//! // (that is the cycle `lib.rs` describes), so an example naming
//! // Arcature items has nothing here to compile against.
//! #[route_model(entity = link::Entity, key = "id", key_type = i64)]
//! pub struct Link(pub link::Model);
//! ```
//!
//! * `entity` -- the SeaORM entity path (required).
//! * `key` -- the route parameter name (optional, default `"id"`).
//! * `key_type` -- the typed key (required, e.g. `i64`, `Uuid`).
//!
//! Bad attribute arguments produce `error[ARC-M005]`; a non-struct item
//! produces `error[ARC-M001]`; a unit struct produces `error[ARC-M002]`.
//!
//! ## Binding does NOT imply authorization
//!
//! The generated `load` returns `None` for 404. It does NOT check
//! authorization -- that is a separate, explicit step (Policies).
//!
//! ## Custom keys
//!
//! For lookups that are not a primary-key `find_by_id` (e.g. slug plus a
//! tenant scope), write the `impl RouteModel` by hand. The macro covers the
//! common case; custom queries stay explicit.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};

/// The implementation of `#[route_model(...)]`. Called by the thin `lib.rs`
/// entrypoint. Returns a [`MacroError`] (converted to `compile_error!` by
/// the entrypoint) on failure -- never panics.
pub fn route_model(attr: TokenStream, item: TokenStream) -> MacroResult {
    let RouteModelArgs {
        entity,
        key_param,
        key_type,
    } = syn::parse2(attr).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM005, e))?;

    let item_struct: syn::ItemStruct =
        syn::parse2(item).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;

    if matches!(item_struct.fields, syn::Fields::Unit) {
        return Err(MacroError::new(
            MacroErrorCode::ArcM002,
            item_struct.fields.span(),
            "#[route_model] requires a struct (tuple or named), not a unit struct",
        ));
    }

    let struct_name = &item_struct.ident;
    let (impl_generics, ty_generics, where_clause) = item_struct.generics.split_for_impl();

    Ok(quote! {
        #item_struct

        impl #impl_generics ::arcature::RouteModel for #struct_name #ty_generics
        #where_clause
        {
            type Key = #key_type;
            type Error = ::arcature::sea_orm::DbErr;
            const KEY_PARAM: &'static str = #key_param;

            async fn load(
                key: Self::Key,
                db: &::arcature::database::Db,
            ) -> ::std::result::Result<::std::option::Option<Self>, Self::Error> {
                use ::arcature::sea_orm::EntityTrait as _;
                let model = #entity::find_by_id(key).one(db.orm()).await?;
                ::std::result::Result::Ok(model.map(#struct_name))
            }
        }
    })
}

/// The parsed `#[route_model(...)]` attribute arguments.
struct RouteModelArgs {
    /// The SeaORM entity path (e.g. `link::Entity`).
    entity: syn::Path,
    /// The route parameter name (e.g. `"id"`).
    key_param: String,
    /// The typed key (e.g. `i64`, `Uuid`).
    key_type: syn::Type,
}

impl Parse for RouteModelArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut entity: Option<syn::Path> = None;
        let mut key_param: Option<String> = None;
        let mut key_type: Option<syn::Type> = None;

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            let _: syn::Token![=] = input.parse()?;

            match ident.to_string().as_str() {
                "entity" => entity = Some(input.parse()?),
                "key" => key_param = Some(input.parse::<syn::LitStr>()?.value()),
                "key_type" => key_type = Some(input.parse()?),
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "unknown `#[route_model]` argument `{other}`; \
                             expected `entity`, `key`, or `key_type`"
                        ),
                    ));
                }
            }

            let _ = input.parse::<syn::Token![,]>();
        }

        Ok(RouteModelArgs {
            entity: entity.ok_or_else(|| {
                syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "#[route_model] requires an `entity` argument, e.g. `entity = link::Entity`",
                )
            })?,
            key_type: key_type.ok_or_else(|| {
                syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "#[route_model] requires a `key_type` argument, e.g. `key_type = i64`",
                )
            })?,
            key_param: key_param.unwrap_or_else(|| "id".to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand_link(attr: TokenStream) -> String {
        route_model(attr, quote! { pub struct Link(pub link::Model); })
            .unwrap()
            .to_string()
    }

    #[test]
    fn generates_impl_route_model() {
        let s = expand_link(quote! { entity = link::Entity, key = "id", key_type = i64 });
        assert!(s.contains(":: arcature :: RouteModel"), "got: {s}");
        assert!(s.contains("KEY_PARAM"), "got: {s}");
        assert!(s.contains("find_by_id"), "got: {s}");
    }

    #[test]
    fn uses_arcature_absolute_paths() {
        let s = expand_link(quote! { entity = link::Entity, key_type = i64 });
        assert!(s.contains(":: arcature :: database :: Db"), "got: {s}");
        assert!(s.contains(":: arcature :: sea_orm :: DbErr"), "got: {s}");
    }

    #[test]
    fn defaults_key_param_to_id() {
        let s = expand_link(quote! { entity = post::Entity, key_type = uuid::Uuid });
        assert!(s.contains("\"id\""), "got: {s}");
    }

    #[test]
    fn honours_custom_key_param() {
        let s = expand_link(quote! { entity = post::Entity, key = "slug", key_type = String });
        assert!(s.contains("\"slug\""), "got: {s}");
    }

    #[test]
    fn maps_loaded_model_to_self() {
        let s = expand_link(quote! { entity = link::Entity, key_type = i64 });
        assert!(s.contains("model . map (Link)"), "got: {s}");
    }

    #[test]
    fn rejects_enum_item() {
        let err = route_model(
            quote! { entity = link::Entity, key_type = i64 },
            quote! { enum Status { Active } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM001);
    }

    #[test]
    fn rejects_unit_struct() {
        let err = route_model(
            quote! { entity = link::Entity, key_type = i64 },
            quote! { pub struct Link; },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }

    #[test]
    fn rejects_missing_entity() {
        let err = route_model(
            quote! { key_type = i64 },
            quote! { pub struct Link(pub M); },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM005);
    }

    #[test]
    fn rejects_missing_key_type() {
        let err = route_model(
            quote! { entity = link::Entity },
            quote! { pub struct Link(pub M); },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM005);
    }

    #[test]
    fn rejects_unknown_argument() {
        let err = route_model(
            quote! { entity = link::Entity, key_type = i64, foo = 42 },
            quote! { pub struct Link(pub M); },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM005);
    }
}
