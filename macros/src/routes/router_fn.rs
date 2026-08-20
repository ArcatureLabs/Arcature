//! Generation of the `<name>_routes()` router function.
//!
//! One responsibility: turn the entry tree into the statements that build a
//! `Vec<Route<S>>` and hand it to `Routes::<S>::new`.
//!
//! The tree is walked rather than the flat list because middleware scoping is
//! a tree property: a group's middleware wraps that group's routes and no
//! others. Paths, by contrast, are resolved at macro time, so a group is
//! emitted as a `RouteGroup` with an empty prefix -- purely a middleware
//! carrier over routes that already carry their full path.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned as _;

use super::action;
use super::declaration::{
    ResourceDeclaration, RouteEntry, RouteGroup, RoutesDeclaration, SingleRoute,
};
use super::flatten::action_handler;
use super::path;

/// Generates the router function for a declaration.
pub fn expand(decl: &RoutesDeclaration, state_ty: &TokenStream) -> TokenStream {
    let vis = &decl.visibility;
    let fn_ident = &decl.routes_fn_ident;
    let body = statements(&decl.entries, "", state_ty);

    quote! {
        /// Builds the routes declared in this block.
        ///
        /// Every path is already absolute: group prefixes are resolved at
        /// compile time, so the returned routes can be merged into any
        /// router without further rewriting.
        #vis fn #fn_ident() -> ::arcature::Routes<#state_ty> {
            let mut routes: ::std::vec::Vec<::arcature::Route<#state_ty>> =
                ::std::vec::Vec::new();
            #body
            ::arcature::Routes::<#state_ty>::new(routes)
        }
    }
}

/// Emits the statements that push a list of entries onto `routes`.
fn statements(entries: &[RouteEntry], prefix: &str, state_ty: &TokenStream) -> TokenStream {
    let pieces = entries.iter().map(|entry| match entry {
        RouteEntry::Route(route) => push_route(route, prefix, state_ty),
        RouteEntry::Group(group) => push_group(group, prefix, state_ty),
        RouteEntry::Resource(resource) => push_resource(resource, prefix, state_ty),
    });
    quote! { #(#pieces)* }
}

fn push_route(route: &SingleRoute, prefix: &str, state_ty: &TokenStream) -> TokenStream {
    let constructed = construct(
        state_ty,
        route.method.constructor(),
        &path::join(prefix, &route.path),
        &route.handler,
        route.name.as_deref().unwrap_or_default(),
    );
    quote! { routes.push(#constructed); }
}

fn push_group(group: &RouteGroup, prefix: &str, state_ty: &TokenStream) -> TokenStream {
    let full_prefix = path::join(prefix, &group.prefix);
    let inner = statements(&group.entries, &full_prefix, state_ty);

    if group.middleware.is_empty() {
        return inner;
    }
    scoped(inner, &group.middleware, state_ty)
}

fn push_resource(
    resource: &ResourceDeclaration,
    prefix: &str,
    state_ty: &TokenStream,
) -> TokenStream {
    let base = path::join(prefix, &resource.path);
    let param = path::singularize(&path::last_segment(&resource.name));

    let pushes = action::selected(&resource.only, &resource.except)
        .into_iter()
        .map(|name| {
            let (method, suffix) = action::route(name, &param);
            let constructed = construct(
                state_ty,
                method.constructor(),
                &format!("{base}{suffix}"),
                &action_handler(&resource.controller, name),
                &format!("{}.{name}", resource.name),
            );
            quote! { routes.push(#constructed); }
        });
    let inner = quote! { #(#pushes)* };
    let bind_check = bind_check(resource.bind.as_ref());

    let routes = if resource.middleware.is_empty() {
        inner
    } else {
        scoped(inner, &resource.middleware, state_ty)
    };
    quote! { #bind_check #routes }
}

/// Emits the compile-time check behind a resource's `bind: Model` option.
///
/// Binding is performed by the handler's `Bound<Model>` extractor, not by the
/// router, so the declaration's job is to make the claim checkable: naming a
/// type that does not implement `RouteModel` fails to compile here, at the
/// route declaration, instead of at whichever handler happens to extract it.
fn bind_check(bind: Option<&syn::Path>) -> TokenStream {
    match bind {
        Some(model) => quote! {
            {
                const fn assert_route_model<T: ::arcature::RouteModel>() {}
                assert_route_model::<#model>();
            }
        },
        None => TokenStream::new(),
    }
}

/// Wraps already-pushed routes in a middleware-carrying group.
///
/// Middleware is attached in reverse declaration order because each
/// `.middleware(..)` call layers outside the previous one; reversing makes
/// the first-listed middleware the outermost -- first to see the request,
/// last to see the response.
fn scoped(inner: TokenStream, middleware: &[syn::Path], state_ty: &TokenStream) -> TokenStream {
    let layers = middleware
        .iter()
        .rev()
        .map(|mw| quote! { .middleware(#mw) });
    quote! {
        routes.extend(::arcature::IntoRoutes::into_routes(
            ::arcature::RouteGroup::<#state_ty>::new("", {
                let mut routes: ::std::vec::Vec<::arcature::Route<#state_ty>> =
                    ::std::vec::Vec::new();
                #inner
                routes
            })
            #(#layers)*
        ));
    }
}

/// Emits `Route::<S>::<method>(path, handler)` with an optional `.name(..)`.
fn construct(
    state_ty: &TokenStream,
    constructor: &str,
    path: &str,
    handler: &syn::Path,
    name: &str,
) -> TokenStream {
    let constructor = syn::Ident::new(constructor, handler.span());
    let route = quote! { ::arcature::Route::<#state_ty>::#constructor(#path, #handler) };
    if name.is_empty() {
        route
    } else {
        quote! { #route.name(#name) }
    }
}

#[cfg(test)]
mod tests {
    use super::expand;
    use crate::routes::declaration::RoutesDeclaration;
    use quote::quote;

    fn generated(tokens: proc_macro2::TokenStream) -> String {
        let decl = syn::parse2::<RoutesDeclaration>(tokens).expect("parses");
        let state = match &decl.state {
            Some(ty) => quote! { #ty },
            None => quote! { () },
        };
        expand(&decl, &state).to_string()
    }

    #[test]
    fn the_function_is_named_after_the_declaration() {
        let out = generated(quote! { pub app { get "/" => home } });
        assert!(out.contains("fn app_routes"));
        assert!(out.contains(":: arcature :: Routes < () >"));
    }

    #[test]
    fn the_state_type_flows_into_every_route() {
        let out = generated(quote! { app { state: AppState; get "/" => home } });
        assert!(out.contains(":: arcature :: Routes < AppState >"));
        assert!(out.contains(":: arcature :: Route :: < AppState > :: get"));
    }

    #[test]
    fn a_named_route_carries_its_name() {
        let out = generated(quote! { app { get "/" => home { name: home } } });
        assert!(out.contains(". name (\"home\")"));
    }

    #[test]
    fn an_unnamed_route_has_no_name_call() {
        let out = generated(quote! { app { get "/" => home } });
        assert!(!out.contains(". name ("));
    }

    #[test]
    fn group_paths_are_resolved_at_compile_time() {
        let out = generated(quote! {
            app { group "/auth" { get "/login" => login } }
        });
        assert!(out.contains("\"/auth/login\""));
    }

    #[test]
    fn a_group_without_middleware_is_inlined() {
        let out = generated(quote! { app { group "/auth" { get "/login" => login } } });
        assert!(!out.contains("RouteGroup"));
    }

    #[test]
    fn group_middleware_is_applied_in_reverse_declaration_order() {
        let out = generated(quote! {
            app {
                group "/admin" {
                    middleware: [First, Second];
                    get "/panel" => panel
                }
            }
        });
        let second = out.find(". middleware (Second)").expect("Second applied");
        let first = out.find(". middleware (First)").expect("First applied");
        assert!(
            second < first,
            "the first-listed middleware must be outermost"
        );
    }

    #[test]
    fn a_resource_emits_every_action_route() {
        let out = generated(quote! {
            app { resource "/links" => LinksController { name: links } }
        });
        assert!(out.contains("LinksController :: index"));
        assert!(out.contains("LinksController :: destroy"));
        assert!(out.contains("\"/links/{link}/edit\""));
    }

    #[test]
    fn resource_middleware_scopes_to_its_actions() {
        let out = generated(quote! {
            app {
                resource "/links" => LinksController { name: links, only: [index], middleware: [Auth] }
            }
        });
        assert!(out.contains("RouteGroup :: < () > :: new (\"\""));
        assert!(out.contains(". middleware (Auth)"));
    }
}
