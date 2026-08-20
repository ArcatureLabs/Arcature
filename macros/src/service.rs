//! `#[service]` -- declares a struct as an Arcature service.
//!
//! A **service** is cheap per-request composition from application
//! resources. The macro generates three impls:
//!
//! 1. `impl ::arcature::DxComponent` -- the static `NAME` for
//!    `arc services` inspection.
//! 2. `impl ::arcature::Service` -- the service marker with `DEPS`
//!    metadata for `arc check` graph validation.
//! 3. `impl ::arcature::Resolve<S>` -- per-state construction from the
//!    `Resolve<S>` impls of the struct's field types.
//!
//! ## Syntax
//!
//! ```ignore
//! #[service]
//! pub struct LinkService {
//!     db: Db,
//!     cache: Cache,
//! }
//! ```
//!
//! The struct must be a plain struct with named fields (not a tuple
//! struct, unit struct, enum, or union). Each field type must itself
//! implement `Resolve<S>` -- `#[service]` generates `T::resolve(state)`
//! calls for each field. Built-in resources (`Db`) have `Resolve<S>` impls
//! provided by Arcature; other services compose transitively.
//!
//! ## Service dependency cycles are impossible
//!
//! Services compose by value. A service `A` that depends on `B` stores `B`
//! as a field -- `A` contains `B` contains `A` would require infinite
//! size. `rustc` rejects this at compile time. No runtime cycle detection
//! is needed.
//!
//! ## No hidden behavior
//!
//! `#[service]` generates only construction glue. It does NOT generate
//! business methods, database transactions, authorization decisions, or
//! event dispatch. The struct's methods are written by the developer and
//! remain visible in application code.
//!
//! ## Custom name
//!
//! ```ignore
//! #[service(name = "LinkSvc")]
//! pub struct LinkService { db: Db }
//! ```
//!
//! One file, one macro: this is the entirety of the `#[service]` expansion.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};

/// The implementation of `#[service]`. Parses the attribute arguments and
/// struct, then expands the struct with `impl DxComponent`,
/// `impl Service`, and `impl Resolve<S>`.
pub fn service(attr: TokenStream, item: TokenStream) -> MacroResult {
    let args: ServiceArgs =
        syn::parse2(attr).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM006, e))?;

    let item_struct: syn::ItemStruct =
        syn::parse2(item).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;

    // Services compose by value from named fields; tuple structs and unit
    // structs do not carry the field names the macro needs to generate
    // `T::resolve(state)` calls.
    let syn::Fields::Named(named) = &item_struct.fields else {
        return Err(MacroError::new(
            MacroErrorCode::ArcM006,
            item_struct.fields.span(),
            "#[service] requires a struct with named fields (not a tuple struct, \
             unit struct, enum, or union). Services compose by value from \
             named dependencies.",
        ));
    };

    let struct_name = &item_struct.ident;
    let (impl_generics, ty_generics, where_clause) = item_struct.generics.split_for_impl();

    let name_lit = args.name.unwrap_or_else(|| struct_name.to_string());

    let fields: Vec<&syn::Field> = named.named.iter().collect();

    // DEPS metadata: the simple type name of each field (e.g. "Db",
    // "Cache", "LinkService"). This feeds `arc check` graph validation.
    let deps_lits: Vec<String> = fields.iter().map(|f| simple_type_name(&f.ty)).collect();

    let field_idents: Vec<&syn::Ident> = fields.iter().filter_map(|f| f.ident.as_ref()).collect();
    let field_types: Vec<&syn::Type> = fields.iter().map(|f| &f.ty).collect();

    // Each field type must itself implement Resolve<S>.
    let resolve_bounds = field_types.iter().map(|ty| {
        quote! { #ty: ::arcature::Resolve<S> }
    });

    // The Resolve<S> impl needs `S` as an extra generic parameter in
    // addition to the struct's own generics. Clone the struct's generics,
    // insert `S` at the front, and split for impl.
    let mut resolve_generics = item_struct.generics.clone();
    let s_param: syn::GenericParam = syn::parse_quote!(S);
    resolve_generics.params.insert(0, s_param);
    let (resolve_impl_generics, _, _) = resolve_generics.split_for_impl();

    Ok(quote! {
        #item_struct

        impl #impl_generics ::arcature::DxComponent for #struct_name #ty_generics #where_clause {
            const NAME: &'static str = #name_lit;
        }

        impl #impl_generics ::arcature::Service for #struct_name #ty_generics #where_clause {
            const DEPS: &'static [&'static str] = &[#(#deps_lits),*];
        }

        impl #resolve_impl_generics ::arcature::Resolve<S> for #struct_name #ty_generics
        where
            #(#resolve_bounds,)*
            S: ::core::marker::Send + ::core::marker::Sync,
            #where_clause
        {
            fn resolve(state: &S) -> Self {
                Self {
                    #(#field_idents: <#field_types as ::arcature::Resolve<S>>::resolve(state)),*
                }
            }
        }
    })
}

/// The parsed `#[service(...)]` attribute arguments.
struct ServiceArgs {
    name: Option<String>,
}

impl Parse for ServiceArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name: Option<String> = None;

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            let _: syn::Token![=] = input.parse()?;

            match ident.to_string().as_str() {
                "name" => {
                    let lit: syn::LitStr = input.parse()?;
                    name = Some(lit.value());
                }
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("unknown `#[service]` argument `{other}`; expected `name`"),
                    ));
                }
            }

            let _ = input.parse::<syn::Token![,]>();
        }

        Ok(ServiceArgs { name })
    }
}

/// Extracts the simple (unqualified) type name for `DEPS` metadata.
///
/// For `Db` -> `"Db"`, for `arcature::database::Db` -> `"Db"`, for
/// `Vec<u8>` -> `"Vec"`. This is the name used in `arc check` graph
/// validation -- it matches the names registered in
/// `ModuleDescriptor.services` and the `DxComponent::NAME` of dependency
/// types.
fn simple_type_name(ty: &syn::Type) -> String {
    let s = ty.to_token_stream().to_string();
    let s = s.split("::").last().unwrap_or(&s);
    match s.find('<') {
        Some(idx) => s[..idx].trim().to_string(),
        None => s.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_three_impls() {
        let expanded = service(
            quote! {},
            quote! { pub struct LinkService { db: Db, cache: Cache } },
        )
        .unwrap()
        .to_string();

        assert!(expanded.contains("DxComponent"), "missing DxComponent: {expanded}");
        assert!(expanded.contains("Service"), "missing Service: {expanded}");
        assert!(expanded.contains("Resolve"), "missing Resolve: {expanded}");
        assert!(expanded.contains("\"LinkService\""), "wrong NAME: {expanded}");
        assert!(expanded.contains("\"Db\""), "missing Db in DEPS: {expanded}");
        assert!(expanded.contains("\"Cache\""), "missing Cache in DEPS: {expanded}");
    }

    #[test]
    fn uses_name_override() {
        let expanded = service(
            quote! { name = "LinkSvc" },
            quote! { pub struct LinkService { db: Db } },
        )
        .unwrap()
        .to_string();
        assert!(expanded.contains("\"LinkSvc\""), "expected LinkSvc NAME: {expanded}");
        assert!(
            !expanded.contains("\"LinkService\""),
            "should not use type name: {expanded}"
        );
    }

    #[test]
    fn generates_resolve_body_with_field_calls() {
        let expanded = service(quote! {}, quote! { pub struct Svc { db: Db } })
            .unwrap()
            .to_string();
        assert!(
            expanded.contains("resolve"),
            "expected resolve call: {expanded}"
        );
        assert!(expanded.contains("db :"), "expected field construction: {expanded}");
    }

    #[test]
    fn handles_generic_service() {
        let expanded = service(quote! {}, quote! { pub struct RepoService<T> { inner: T } })
            .unwrap()
            .to_string();
        assert!(expanded.contains("DxComponent"), "missing DxComponent: {expanded}");
        assert!(
            expanded.contains("S , T") || expanded.contains("S, T"),
            "expected <S, T> in resolve impl: {expanded}"
        );
    }

    #[test]
    fn rejects_tuple_struct() {
        let err = service(quote! {}, quote! { pub struct LinkService(Db); }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM006);
    }

    #[test]
    fn rejects_enum() {
        let err = service(quote! {}, quote! { enum Status { Active } }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM001);
    }

    #[test]
    fn rejects_unknown_argument() {
        let err = service(quote! { foo = 42 }, quote! { pub struct S { db: Db } }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM006);
    }

    #[test]
    fn simple_type_name_strips_paths_and_generics() {
        use syn::parse_str;
        assert_eq!(simple_type_name(&parse_str("Db").unwrap()), "Db");
        assert_eq!(
            simple_type_name(&parse_str("arcature::database::Db").unwrap()),
            "Db"
        );
        assert_eq!(simple_type_name(&parse_str("Vec<u8>").unwrap()), "Vec");
    }
}
