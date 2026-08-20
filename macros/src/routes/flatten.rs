//! Flattening the entry tree into the concrete route list.
//!
//! One responsibility: resolve group prefixes and expand resources so that
//! every declared route becomes one [`ExpandedRoute`] with its final path,
//! final dotted name, and resolved handler. The metadata const and the route
//! helper module are both generated from this flat list; the router function
//! keeps the tree, because middleware scoping is a tree property.

use super::action;
use super::declaration::{ResourceDeclaration, RouteEntry, RouteGroup, SingleRoute};
use super::method::RouteMethodKind;
use super::path;

/// One fully resolved route.
pub struct ExpandedRoute {
    /// The HTTP method.
    pub method: RouteMethodKind,
    /// The final path, with every enclosing group prefix applied.
    pub path: String,
    /// The dotted route name, or an empty string for an unnamed route.
    pub name: String,
    /// The handler path (a resource action resolves to `Controller::action`).
    pub handler: syn::Path,
    /// The Inertia pages this route renders.
    pub pages: Vec<String>,
    /// The typed request type of an Action route.
    pub action: Option<syn::Path>,
    /// The typed response type of a Query route.
    pub query: Option<syn::Type>,
    /// The typed query-string contract of a Query route.
    pub query_string: Option<syn::Path>,
}

/// Flattens entries under `prefix` into concrete routes.
pub fn entries(entries: &[RouteEntry], prefix: &str) -> Vec<ExpandedRoute> {
    let mut flat = Vec::new();
    for entry in entries {
        match entry {
            RouteEntry::Route(route) => flat.push(single(route, prefix)),
            RouteEntry::Group(group) => flat.extend(self::group(group, prefix)),
            RouteEntry::Resource(res) => flat.extend(resource(res, prefix)),
        }
    }
    flat
}

fn single(route: &SingleRoute, prefix: &str) -> ExpandedRoute {
    ExpandedRoute {
        method: route.method,
        path: path::join(prefix, &route.path),
        name: route.name.clone().unwrap_or_default(),
        handler: route.handler.clone(),
        pages: route.pages.clone(),
        action: route.action.clone(),
        // Unboxed here: `ExpandedRoute` is a transient flat value, never an
        // enum variant, so the indirection that keeps `RouteEntry` small
        // buys nothing.
        query: route.query.clone().map(|boxed| *boxed),
        query_string: route.query_string.clone(),
    }
}

fn group(group: &RouteGroup, prefix: &str) -> Vec<ExpandedRoute> {
    entries(&group.entries, &path::join(prefix, &group.prefix))
}

/// Expands a resource into one route per selected action.
///
/// A resource action declares no pages, `action:`, or `query:`: an action
/// that carries a typed contract is written as an explicit route instead, so
/// the contract edge stays visible at the declaration site rather than being
/// implied by a convention.
fn resource(resource: &ResourceDeclaration, prefix: &str) -> Vec<ExpandedRoute> {
    let base = path::join(prefix, &resource.path);
    let param = path::singularize(&path::last_segment(&resource.name));

    action::selected(&resource.only, &resource.except)
        .into_iter()
        .map(|name| {
            let (method, suffix) = action::route(name, &param);
            ExpandedRoute {
                method,
                path: format!("{base}{suffix}"),
                name: format!("{}.{name}", resource.name),
                handler: action_handler(&resource.controller, name),
                pages: Vec::new(),
                action: None,
                query: None,
                query_string: None,
            }
        })
        .collect()
}

/// Appends `::<action>` to a controller path (`LinksController::index`).
pub fn action_handler(controller: &syn::Path, action: &str) -> syn::Path {
    let mut segments = controller.segments.clone();
    segments.push(syn::PathSegment::from(syn::Ident::new(
        action,
        syn::spanned::Spanned::span(controller),
    )));
    syn::Path {
        leading_colon: controller.leading_colon,
        segments,
    }
}

#[cfg(test)]
mod tests {
    use super::{ExpandedRoute, entries};
    use crate::routes::declaration::RoutesDeclaration;
    use crate::routes::method::RouteMethodKind;

    fn flatten(tokens: proc_macro2::TokenStream) -> Vec<ExpandedRoute> {
        let decl = syn::parse2::<RoutesDeclaration>(tokens).expect("parses");
        entries(&decl.entries, "")
    }

    #[test]
    fn a_plain_route_keeps_its_path() {
        let flat = flatten(quote::quote! { app { get "/" => home { name: home } } });
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].path, "/");
        assert_eq!(flat[0].name, "home");
    }

    #[test]
    fn group_prefixes_are_applied() {
        let flat = flatten(quote::quote! {
            app { group "/auth" { get "/login" => login { name: auth.login } } }
        });
        assert_eq!(flat[0].path, "/auth/login");
    }

    #[test]
    fn nested_group_prefixes_compose() {
        let flat = flatten(quote::quote! {
            app {
                group "/api" {
                    group "/v1" { get "/links" => index { name: api.v1.links } }
                }
            }
        });
        assert_eq!(flat[0].path, "/api/v1/links");
    }

    #[test]
    fn a_root_route_inside_a_group_is_the_prefix() {
        let flat = flatten(quote::quote! {
            app { group "/auth" { get "/" => index } }
        });
        assert_eq!(flat[0].path, "/auth");
    }

    #[test]
    fn a_resource_expands_to_seven_rest_routes() {
        let flat = flatten(quote::quote! {
            app { resource "/links" => LinksController { name: links } }
        });
        let paths: Vec<_> = flat
            .iter()
            .map(|r| (r.method, r.path.as_str(), r.name.as_str()))
            .collect();
        assert_eq!(
            paths,
            vec![
                (RouteMethodKind::Get, "/links", "links.index"),
                (RouteMethodKind::Get, "/links/new", "links.create"),
                (RouteMethodKind::Post, "/links", "links.store"),
                (RouteMethodKind::Get, "/links/{link}", "links.show"),
                (RouteMethodKind::Get, "/links/{link}/edit", "links.edit"),
                (RouteMethodKind::Put, "/links/{link}", "links.update"),
                (RouteMethodKind::Delete, "/links/{link}", "links.destroy"),
            ]
        );
    }

    #[test]
    fn a_resource_handler_is_the_controller_action() {
        let flat = flatten(quote::quote! {
            app { resource "/links" => LinksController { name: links, only: [index] } }
        });
        assert_eq!(
            crate::routes::type_name::handler(&flat[0].handler),
            "LinksController::index"
        );
    }

    #[test]
    fn a_resource_inside_a_group_is_prefixed() {
        let flat = flatten(quote::quote! {
            app {
                group "/admin" {
                    resource "/links" => LinksController { name: admin.links, only: [show] }
                }
            }
        });
        assert_eq!(flat[0].path, "/admin/links/{link}");
    }
}
