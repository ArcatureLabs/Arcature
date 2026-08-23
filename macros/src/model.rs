//! `#[model(table = "users")]` -- a SeaORM entity model with the query facade.
//!
//! # Why this expands to a module
//!
//! SeaORM's `DeriveEntityModel` is not a derive that can be applied to any
//! struct. It generates a whole family of sibling items -- `Entity`, `Column`,
//! `PrimaryKey`, `ActiveModel` -- into the surrounding scope, and it requires
//! the annotated struct to be named exactly `Model`, because the items it
//! generates refer to it by that name. Applying it to `struct User` does not
//! compile.
//!
//! So `#[model]` gives each model its own module. Inside, the struct is
//! `Model` and SeaORM is happy; outside, the parent scope gets `User` and the
//! four companion types under predictable names:
//!
//! ```ignore
//! // Not compiled: `arcature-macros` cannot depend on `arcature`
//! // (that is the cycle `lib.rs` describes), so an example naming
//! // Arcature items has nothing here to compile against.
//! #[model(table = "users")]
//! pub struct User {
//!     #[sea_orm(primary_key)]
//!     pub id: i64,
//!     pub email: String,
//! }
//! ```
//!
//! yields `User`, `UserEntity`, `UserActiveModel`, `UserColumn`,
//! `UserPrimaryKey` and `UserRelation`. Queries read `User::query(&db)` --
//! see [`arcature::database::Model`], the blanket-implemented trait that puts
//! the query facade on the row type rather than on `UserEntity`.
//!
//! # What this macro does not do
//!
//! The generated `Relation` enum is empty, so a model declared this way has
//! no relations. There is no syntax to add them: the enum lives inside the
//! generated module, which the application cannot write into. A model that
//! needs relations should be written as a plain SeaORM entity module instead
//! -- `#[model]` is the short path, not the only one. Saying so is better
//! than accepting a `relations` argument that silently does nothing.
//!
//! One file, one macro: this is the entirety of the `#[model]` expansion.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{ItemStruct, Lit, Meta, parse_macro_input};

use crate::util::to_snake_case;

/// The `#[model(table = "users")]` attribute macro.
pub fn model(attr: TokenStream, item: TokenStream) -> TokenStream {
    let table = match parse_model_attr(attr.into()) {
        Ok(t) => t,
        Err(e) => return e.to_compile_error().into(),
    };
    let item_struct = parse_macro_input!(item as ItemStruct);
    match expand(&table, item_struct) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Build the module-plus-re-exports expansion for one model struct.
fn expand(table: &str, mut item_struct: ItemStruct) -> syn::Result<proc_macro2::TokenStream> {
    // A SeaORM entity is tied to one table with one concrete column set, so
    // there is nothing a type parameter could vary. Rejecting generics here
    // gives one clear message instead of a page of errors from inside
    // `DeriveEntityModel`.
    if !item_struct.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_struct.generics,
            "#[model] does not support generic parameters: a SeaORM entity \
             maps to exactly one table with one column set",
        ));
    }
    if !matches!(item_struct.fields, syn::Fields::Named(_)) {
        return Err(syn::Error::new_spanned(
            &item_struct.fields,
            "#[model] requires a struct with named fields, one per column \
             (e.g. `struct User { id: i64 }`)",
        ));
    }

    let alias = item_struct.ident.clone();
    let vis = item_struct.vis.clone();
    let module = format_ident!("__arcature_model_{}", to_snake_case(&alias.to_string()));

    // Companion type names. `UserEntity` rather than `user::Entity` so the
    // application never has to name the generated module.
    let entity = format_ident!("{alias}Entity");
    let active = format_ident!("{alias}ActiveModel");
    let column = format_ident!("{alias}Column");
    let primary_key = format_ident!("{alias}PrimaryKey");
    let relation = format_ident!("{alias}Relation");

    // Inside the module the struct must be called `Model`; the doc comment
    // the application wrote travels with it, and the re-export below carries
    // the original name back out.
    let docs: Vec<syn::Attribute> = item_struct
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .cloned()
        .collect();
    item_struct.ident = syn::Ident::new("Model", alias.span());
    item_struct.vis = syn::parse_quote!(pub);

    let derive_attr: syn::Attribute = syn::parse_quote! {
        #[derive(
            ::core::clone::Clone,
            ::core::fmt::Debug,
            ::core::cmp::PartialEq,
            ::arcature::sea_orm::DeriveEntityModel,
            ::arcature::Serialize,
            ::arcature::Deserialize
        )]
    };
    let table_attr: syn::Attribute = syn::parse_quote! {
        #[sea_orm(table_name = #table)]
    };
    item_struct.attrs.insert(0, table_attr);
    item_struct.attrs.insert(0, derive_attr);

    Ok(quote! {
        #[doc(hidden)]
        #vis mod #module {
            // `DeriveEntityModel` and `DeriveRelation` are written for a
            // hand-rolled entity file: one in a crate that depends on SeaORM
            // directly and opens with the entity prelude. They emit
            // unqualified names on that assumption (`EntityTrait`,
            // `PrimaryKeyTrait`, `EnumIter`) and, for what the prelude does
            // not carry, paths *relative* to a crate named `sea_orm`
            // (`sea_orm::prelude::EntityName`, `sea_orm::sea_query::ValueType`).
            //
            // An application built on Arcature has neither. Supplying both
            // here is what keeps `#[model]` from obliging every application
            // that spells one model to add a second, separately-versioned
            // copy of SeaORM to its own manifest.
            use ::arcature::sea_orm;
            use ::arcature::sea_orm::entity::prelude::*;

            use super::*;

            // `use super::*` above is what lets a field name a type from the
            // application's scope, but it also drags in whatever that scope
            // shadows -- and an application module almost always opens with
            // `use arcature::prelude::*`, whose `Result<T>` is a one-parameter
            // alias. SeaORM's generated code writes bare `Result<_, DbErr>`,
            // which would then not even have the right arity. An explicit
            // import outranks a glob one, so re-asserting the core spellings
            // here puts them back beyond the reach of the surrounding scope.
            use ::core::option::Option::{self, None, Some};
            use ::core::result::Result::{self, Err, Ok};

            #item_struct

            /// No relations: see the `#[model]` documentation.
            #[derive(
                ::core::marker::Copy,
                ::core::clone::Clone,
                ::core::fmt::Debug,
                ::arcature::sea_orm::EnumIter,
                ::arcature::sea_orm::DeriveRelation
            )]
            pub enum Relation {}

            impl ::arcature::sea_orm::ActiveModelBehavior for ActiveModel {}
        }

        #( #docs )*
        #vis use #module::Model as #alias;

        #[doc = concat!("The SeaORM entity for [`", stringify!(#alias), "`].")]
        #vis use #module::Entity as #entity;

        #[doc = concat!("The insertable/updatable form of [`", stringify!(#alias), "`].")]
        #vis use #module::ActiveModel as #active;

        #[doc = concat!("The column set of [`", stringify!(#alias), "`].")]
        #vis use #module::Column as #column;

        #[doc = concat!("The primary key of [`", stringify!(#alias), "`].")]
        #vis use #module::PrimaryKey as #primary_key;

        #[doc = concat!("The (empty) relation set of [`", stringify!(#alias), "`].")]
        #vis use #module::Relation as #relation;
    })
}

/// Parse the `table = "..."` argument for `#[model]`.
fn parse_model_attr(attr: proc_macro2::TokenStream) -> syn::Result<String> {
    if attr.is_empty() {
        return Err(syn::Error::new(
            Span::call_site(),
            "#[model(table = \"...\")] requires a `table` argument",
        ));
    }
    let meta: Meta = syn::parse2(attr)?;
    match meta {
        Meta::NameValue(nv) if nv.path.is_ident("table") => {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: Lit::Str(s), ..
            }) = nv.value
            {
                let table = s.value();
                if table.is_empty() {
                    return Err(syn::Error::new(
                        s.span(),
                        "#[model(table = \"...\")] table name must not be empty",
                    ));
                }
                Ok(table)
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

#[cfg(test)]
mod tests {
    use super::expand;
    use quote::quote;

    fn render(table: &str, item: proc_macro2::TokenStream) -> String {
        expand(table, syn::parse2(item).expect("parses"))
            .unwrap()
            .to_string()
    }

    fn user() -> proc_macro2::TokenStream {
        quote! {
            pub struct User {
                #[sea_orm(primary_key)]
                pub id: i64,
                pub email: String,
            }
        }
    }

    #[test]
    fn the_struct_is_renamed_to_model_so_seaorm_accepts_it() {
        let s = render("users", user());
        assert!(s.contains("pub struct Model"), "got: {s}");
        assert!(s.contains("DeriveEntityModel"), "got: {s}");
    }

    #[test]
    fn the_original_name_comes_back_out_as_a_re_export() {
        let s = render("users", user());
        assert!(
            s.contains("use __arcature_model_user :: Model as User"),
            "got: {s}"
        );
    }

    #[test]
    fn the_companion_types_are_re_exported_under_predictable_names() {
        let s = render("users", user());
        for expected in [
            "Entity as UserEntity",
            "ActiveModel as UserActiveModel",
            "Column as UserColumn",
            "PrimaryKey as UserPrimaryKey",
            "Relation as UserRelation",
        ] {
            assert!(s.contains(expected), "missing `{expected}` in: {s}");
        }
    }

    #[test]
    fn the_table_name_reaches_the_seaorm_attribute() {
        let s = render("users", user());
        assert!(s.contains("table_name = \"users\""), "got: {s}");
    }

    #[test]
    fn field_attributes_survive_the_move_into_the_module() {
        let s = render("users", user());
        assert!(s.contains("primary_key"), "got: {s}");
    }

    #[test]
    fn an_empty_relation_enum_and_behaviour_impl_are_supplied() {
        let s = render("users", user());
        assert!(s.contains("pub enum Relation { }"), "got: {s}");
        assert!(
            s.contains("ActiveModelBehavior for ActiveModel"),
            "got: {s}"
        );
    }

    #[test]
    fn the_module_inherits_the_structs_visibility() {
        let s = render("users", user());
        assert!(s.contains("pub mod __arcature_model_user"), "got: {s}");

        let private = render("notes", quote! { struct Note { pub id: i64 } });
        assert!(
            private.contains("mod __arcature_model_note"),
            "got: {private}"
        );
        assert!(
            !private.contains("pub mod __arcature_model_note"),
            "got: {private}"
        );
    }

    #[test]
    fn a_multiword_name_gives_a_snake_case_module() {
        let s = render("blog_posts", quote! { pub struct BlogPost { pub id: i64 } });
        assert!(s.contains("__arcature_model_blog_post"), "got: {s}");
    }

    #[test]
    fn the_doc_comment_follows_the_name_the_application_sees() {
        let s = render(
            "users",
            quote! {
                /// A row of users.
                pub struct User { pub id: i64 }
            },
        );
        // Present twice: once inside the module on `Model`, once on the
        // re-export the application actually reads.
        assert!(s.matches("A row of users.").count() >= 2, "got: {s}");
    }

    #[test]
    fn generics_are_rejected_with_one_clear_message() {
        let err = expand(
            "users",
            syn::parse2(quote! { pub struct User<T> { pub id: T } }).unwrap(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("generic"), "got: {err}");
    }

    #[test]
    fn a_tuple_struct_is_rejected() {
        let err = expand(
            "users",
            syn::parse2(quote! { pub struct User(i64); }).unwrap(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("named fields"), "got: {err}");
    }

    #[test]
    fn a_missing_table_argument_is_rejected() {
        let err = super::parse_model_attr(proc_macro2::TokenStream::new()).unwrap_err();
        assert!(err.to_string().contains("requires a `table`"), "got: {err}");
    }

    #[test]
    fn an_empty_table_name_is_rejected() {
        let err = super::parse_model_attr(quote! { table = "" }).unwrap_err();
        assert!(err.to_string().contains("must not be empty"), "got: {err}");
    }

    #[test]
    fn an_argument_that_is_not_table_is_rejected() {
        let err = super::parse_model_attr(quote! { name = "users" }).unwrap_err();
        assert!(err.to_string().contains("table = "), "got: {err}");
    }

    #[test]
    fn a_non_string_table_name_is_rejected() {
        let err = super::parse_model_attr(quote! { table = 42 }).unwrap_err();
        assert!(err.to_string().contains("table = "), "got: {err}");
    }
}
