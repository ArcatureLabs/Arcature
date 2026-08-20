//! Generation of the `<NAME>_ROUTES` metadata const.
//!
//! One responsibility: emit a `&'static [RouteDescriptor]` describing every
//! declared route. This is the inspection artifact behind `arc routes`,
//! `arc check`, and the Unified Application Graph.
//!
//! The typed edges are resolved here, at compile time: an `action:` type
//! becomes `<T as RequestMetadata>::FIELDS` and a `query:` type becomes
//! `<T as ResourceMetadata>::FIELDS`. Nothing is reflected at runtime, and a
//! route naming a type that does not carry the metadata is a compile error at
//! the declaration site rather than a gap discovered later.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned as _;

use super::declaration::RoutesDeclaration;
use super::flatten::ExpandedRoute;
use super::type_name;

/// Generates the metadata const for a declaration.
pub fn expand(decl: &RoutesDeclaration, flat: &[ExpandedRoute]) -> TokenStream {
    let vis = &decl.visibility;
    let const_ident = &decl.routes_const_ident;
    let descriptors = flat.iter().map(descriptor);

    quote! {
        /// Route metadata for inspection (`arc routes`, `arc check`).
        #vis const #const_ident: &[::arcature::RouteDescriptor] = &[
            #(#descriptors),*
        ];
    }
}

fn descriptor(route: &ExpandedRoute) -> TokenStream {
    let method = syn::Ident::new(route.method.variant(), route.handler.span());
    let path = &route.path;
    let name = &route.name;
    let handler = type_name::handler(&route.handler);
    let pages = &route.pages;

    let (action_fields, action_type) = request_edge(route.action.as_ref());
    let (query_string_fields, query_string_type) = request_edge(route.query_string.as_ref());
    let (query_fields, query_type, query_array) = query_edge(route.query.as_ref());

    quote! {
        ::arcature::RouteDescriptor {
            method: ::arcature::RouteMethod::#method,
            path: #path,
            name: #name,
            handler: #handler,
            pages: &[#(#pages),*],
            action_fields: #action_fields,
            action_type: #action_type,
            query_fields: #query_fields,
            query_type: #query_type,
            query_array: #query_array,
            query_string_fields: #query_string_fields,
            query_string_type: #query_string_type,
        }
    }
}

/// Resolves a request-typed edge (`action:`, `query_string:`) into its field
/// slice and type name. `(&[], "")` when the edge is absent.
///
/// The type name doubles as the "this edge exists" signal: a no-body action
/// has empty fields, so only the name distinguishes it from a plain route.
fn request_edge(request: Option<&syn::Path>) -> (TokenStream, String) {
    match request {
        Some(path) => (
            quote! { <#path as ::arcature::RequestMetadata>::FIELDS },
            type_name::final_segment(path),
        ),
        None => (quote! { &[] }, String::new()),
    }
}

/// Resolves the `query:` edge into its element field slice, element type
/// name, and collection flag.
fn query_edge(query: Option<&syn::Type>) -> (TokenStream, String, TokenStream) {
    match query {
        Some(ty) => {
            let (element, is_array) = type_name::unwrap_vec(ty);
            (
                quote! { <#element as ::arcature::ResourceMetadata>::FIELDS },
                type_name::of_type(element),
                if is_array {
                    quote! { true }
                } else {
                    quote! { false }
                },
            )
        }
        None => (quote! { &[] }, String::new(), quote! { false }),
    }
}

#[cfg(test)]
mod tests {
    use super::expand;
    use crate::routes::declaration::RoutesDeclaration;
    use crate::routes::flatten;
    use quote::quote;

    fn generated(tokens: proc_macro2::TokenStream) -> String {
        let decl = syn::parse2::<RoutesDeclaration>(tokens).expect("parses");
        let flat = flatten::entries(&decl.entries, "");
        expand(&decl, &flat).to_string()
    }

    #[test]
    fn the_const_is_named_after_the_declaration() {
        let out = generated(quote! { pub app { get "/" => home } });
        assert!(out.contains("const APP_ROUTES : & [:: arcature :: RouteDescriptor]"));
    }

    #[test]
    fn method_and_path_are_baked_in() {
        let out = generated(quote! { app { post "/links" => store } });
        assert!(out.contains("RouteMethod :: Post"));
        assert!(out.contains("path : \"/links\""));
    }

    #[test]
    fn the_handler_is_rendered_as_a_string() {
        let out = generated(quote! { app { get "/l" => LinksController::index } });
        assert!(out.contains("handler : \"LinksController::index\""));
    }

    #[test]
    fn pages_become_a_static_slice() {
        let out = generated(quote! {
            app { get "/" => home { pages: ["Home", "Welcome"] } }
        });
        assert!(out.contains("pages : & [\"Home\" , \"Welcome\"]"));
    }

    #[test]
    fn a_route_without_edges_carries_empty_metadata() {
        let out = generated(quote! { app { get "/" => home } });
        assert!(out.contains("action_fields : & []"));
        assert!(out.contains("action_type : \"\""));
        assert!(out.contains("query_array : false"));
    }

    #[test]
    fn an_action_resolves_request_metadata() {
        let out = generated(quote! {
            app { post "/links" => store { action: StoreLinkRequest } }
        });
        assert!(out.contains("< StoreLinkRequest as :: arcature :: RequestMetadata > :: FIELDS"));
        assert!(out.contains("action_type : \"StoreLinkRequest\""));
    }

    #[test]
    fn a_collection_query_resolves_the_element_type() {
        let out = generated(quote! {
            app { get "/links" => index { query: Vec<LinkResource> } }
        });
        assert!(out.contains("< LinkResource as :: arcature :: ResourceMetadata > :: FIELDS"));
        assert!(out.contains("query_type : \"LinkResource\""));
        assert!(out.contains("query_array : true"));
    }

    #[test]
    fn a_single_query_is_not_an_array() {
        let out = generated(quote! {
            app { get "/links/{link}" => show { query: LinkResource } }
        });
        assert!(out.contains("query_array : false"));
        assert!(out.contains("query_type : \"LinkResource\""));
    }

    #[test]
    fn a_query_string_resolves_request_metadata() {
        let out = generated(quote! {
            app {
                get "/links" => index { query: Vec<LinkResource>, query_string: LinkSearchRequest }
            }
        });
        assert!(out.contains("query_string_type : \"LinkSearchRequest\""));
        assert!(out.contains("< LinkSearchRequest as :: arcature :: RequestMetadata > :: FIELDS"));
    }

    #[test]
    fn resource_actions_get_descriptors() {
        let out = generated(quote! {
            app { resource "/links" => LinksController { name: links, only: [index, destroy] } }
        });
        assert!(out.contains("name : \"links.index\""));
        assert!(out.contains("name : \"links.destroy\""));
        assert!(out.contains("RouteMethod :: Delete"));
    }
}
