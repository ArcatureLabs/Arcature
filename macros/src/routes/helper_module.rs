//! Generation of the `<name>_route` URL helper module.
//!
//! One responsibility: emit a typed function per named route that builds its
//! URL. A parameterless route returns `&'static str`; a parameterized one
//! takes each `{param}` as an `impl Display` argument and returns `String`.
//! Naming a route wrong then becomes a compile error instead of a 404.
//!
//! Dotted names nest: `auth.login` becomes `app_route::auth::login()`, and
//! `home` stays at `app_route::home()`.

use std::collections::BTreeMap;

use proc_macro2::{Span, TokenStream};
use quote::quote;

use super::declaration::RoutesDeclaration;
use super::flatten::ExpandedRoute;
use super::path;

/// Generates the helper module for a declaration.
pub fn expand(decl: &RoutesDeclaration, flat: &[ExpandedRoute]) -> TokenStream {
    let vis = &decl.visibility;
    let mod_ident = &decl.route_mod_ident;

    let mut root: Vec<Helper> = Vec::new();
    let mut nested: BTreeMap<String, Vec<Helper>> = BTreeMap::new();

    for route in flat {
        // An unnamed route has nothing to be looked up by, so it gets no
        // helper: the only way to reach it is the literal path.
        if route.name.is_empty() {
            continue;
        }
        match route.name.split_once('.') {
            Some((first, rest)) => nested
                .entry(first.to_string())
                .or_default()
                .push(Helper::new(&rest.replace('.', "_"), &route.path)),
            None => root.push(Helper::new(&route.name, &route.path)),
        }
    }

    let root_fns = root.iter().map(Helper::expand);
    let nested_mods = nested.iter().map(|(name, helpers)| {
        let ident = syn::Ident::new(name, Span::call_site());
        let fns = helpers.iter().map(Helper::expand);
        quote! {
            pub mod #ident {
                #(#fns)*
            }
        }
    });

    quote! {
        /// Typed URL helpers for the routes declared in this block.
        ///
        /// A route without path parameters returns `&'static str`; one with
        /// parameters takes them as `impl Display` and returns `String`.
        #vis mod #mod_ident {
            #(#root_fns)*
            #(#nested_mods)*
        }
    }
}

/// One generated helper function.
struct Helper {
    func_name: String,
    path: String,
    params: Vec<String>,
}

impl Helper {
    fn new(func_name: &str, route_path: &str) -> Self {
        Helper {
            func_name: func_name.to_string(),
            path: route_path.to_string(),
            params: path::params(route_path),
        }
    }

    fn expand(&self) -> TokenStream {
        let ident = syn::Ident::new(&self.func_name, Span::call_site());
        let path = &self.path;

        if self.params.is_empty() {
            return quote! {
                pub fn #ident() -> &'static str { #path }
            };
        }

        let params: Vec<syn::Ident> = self
            .params
            .iter()
            .map(|p| syn::Ident::new(p, Span::call_site()))
            .collect();
        let declarations = params
            .iter()
            .map(|p| quote! { #p: impl ::std::fmt::Display });

        quote! {
            pub fn #ident( #(#declarations),* ) -> ::std::string::String {
                ::std::format!(#path #(, #params = #params)*)
            }
        }
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
    fn the_module_is_named_after_the_declaration() {
        let out = generated(quote! { pub app { get "/" => home { name: home } } });
        assert!(out.contains("pub mod app_route"));
    }

    #[test]
    fn a_parameterless_route_returns_a_static_str() {
        let out = generated(quote! { app { get "/" => home { name: home } } });
        assert!(out.contains("pub fn home () -> & 'static str { \"/\" }"));
    }

    #[test]
    fn a_dotted_name_nests_into_a_module() {
        let out = generated(quote! {
            app { get "/login" => login { name: auth.login } }
        });
        assert!(out.contains("pub mod auth"));
        assert!(out.contains("pub fn login ()"));
    }

    #[test]
    fn a_deeply_dotted_name_flattens_below_the_first_segment() {
        let out = generated(quote! {
            app { get "/l" => index { name: api.v1.links } }
        });
        assert!(out.contains("pub mod api"));
        assert!(out.contains("pub fn v1_links ()"));
    }

    #[test]
    fn a_parameterized_route_formats_its_path() {
        let out = generated(quote! {
            app { get "/links/{link}" => show { name: links.show } }
        });
        assert!(out.contains("link : impl :: std :: fmt :: Display"));
        assert!(out.contains(":: std :: string :: String"));
        assert!(out.contains("format ! (\"/links/{link}\" , link = link)"));
    }

    #[test]
    fn multiple_parameters_are_all_arguments() {
        let out = generated(quote! {
            app { get "/teams/{team}/links/{link}" => show { name: links.show } }
        });
        assert!(out.contains("team : impl :: std :: fmt :: Display"));
        assert!(out.contains("link : impl :: std :: fmt :: Display"));
    }

    #[test]
    fn unnamed_routes_get_no_helper() {
        let out = generated(quote! { app { get "/" => home } });
        assert!(!out.contains("pub fn "));
    }

    #[test]
    fn resource_helpers_land_under_the_resource_name() {
        let out = generated(quote! {
            app { resource "/links" => LinksController { name: links } }
        });
        assert!(out.contains("pub mod links"));
        assert!(out.contains("pub fn index ()"));
        assert!(out.contains("pub fn edit ("));
    }
}
