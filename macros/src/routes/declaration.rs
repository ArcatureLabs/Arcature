//! The parsed shape of a `routes!` block.
//!
//! One responsibility: turn the `routes!` token stream into a
//! [`RoutesDeclaration`] tree. No validation beyond what the grammar itself
//! rules out, and no code generation -- those are `validate` and `expand`.

use syn::parse::{Parse, ParseStream};

use super::keywords::{method as method_keyword, option as keyword};
use super::list;
use super::method::RouteMethodKind;
use super::options;

/// A whole `routes!` declaration.
#[derive(Debug)]
pub struct RoutesDeclaration {
    /// The visibility applied to every generated item.
    pub visibility: syn::Visibility,
    /// The generated router function ident (`app_routes`).
    pub routes_fn_ident: syn::Ident,
    /// The generated metadata const ident (`APP_ROUTES`).
    pub routes_const_ident: syn::Ident,
    /// The generated helper module ident (`app_route`).
    pub route_mod_ident: syn::Ident,
    /// The router state type from the optional `state: T;` clause.
    pub state: Option<syn::Type>,
    /// The top-level entries.
    pub entries: Vec<RouteEntry>,
}

/// One entry in a routes block or group.
#[derive(Debug)]
pub enum RouteEntry {
    /// A single `get "/path" => handler` route.
    Route(SingleRoute),
    /// A `group "/prefix" { .. }` block.
    Group(RouteGroup),
    /// A `resource "/path" => Controller { .. }` expansion.
    Resource(ResourceDeclaration),
}

/// A single declared route.
#[derive(Debug)]
pub struct SingleRoute {
    /// The HTTP method keyword that opened the entry.
    pub method: RouteMethodKind,
    /// The path as written, before group prefixes are applied.
    pub path: String,
    /// The handler function or controller method path.
    pub handler: syn::Path,
    /// The dotted route name, when declared.
    pub name: Option<String>,
    /// The Inertia pages this route renders.
    pub pages: Vec<String>,
    /// The typed request type of an Action route.
    pub action: Option<syn::Path>,
    /// The typed response type of a Query route.
    ///
    /// Boxed because a `syn::Type` is a large AST node and `SingleRoute`
    /// lives inside the `RouteEntry` enum, which is constructed once per
    /// route in every `routes!` block.
    pub query: Option<Box<syn::Type>>,
    /// The typed query-string contract of a Query route.
    pub query_string: Option<syn::Path>,
}

/// A prefixed group of entries with optional shared middleware.
#[derive(Debug)]
pub struct RouteGroup {
    /// The path prefix (e.g. `"/auth"`).
    pub prefix: String,
    /// Middleware values applied to every route in the group. The first
    /// listed runs outermost.
    pub middleware: Vec<syn::Path>,
    /// The nested entries.
    pub entries: Vec<RouteEntry>,
}

/// A RESTful resource expansion.
#[derive(Debug)]
pub struct ResourceDeclaration {
    /// The resource base path (e.g. `"/links"`).
    pub path: String,
    /// The controller the actions are methods on.
    pub controller: syn::Path,
    /// The dotted resource name; each action route is `<name>.<action>`.
    pub name: String,
    /// Actions to include (empty = all).
    pub only: Vec<String>,
    /// Actions to exclude.
    pub except: Vec<String>,
    /// The route model bound to the resource's path parameter. Checked at
    /// compile time to implement `RouteModel`; the loading itself is the
    /// handler's `Bound<Model>` extractor.
    pub bind: Option<syn::Path>,
    /// Middleware values applied to every action route.
    pub middleware: Vec<syn::Path>,
}

impl Parse for RoutesDeclaration {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let visibility: syn::Visibility = input.parse()?;
        let name_ident: syn::Ident = input.parse()?;
        let name = name_ident.to_string();
        let span = name_ident.span();

        let routes_fn_ident = syn::Ident::new(&format!("{name}_routes"), span);
        let routes_const_ident = syn::Ident::new(&format!("{}_ROUTES", name.to_uppercase()), span);
        let route_mod_ident = syn::Ident::new(&format!("{name}_route"), span);

        let content;
        syn::braced!(content in input);

        let state = parse_state_clause(&content)?;

        let mut entries = Vec::new();
        while !content.is_empty() {
            entries.push(parse_entry(&content)?);
            let _: Option<syn::Token![,]> = content.parse()?;
        }

        Ok(RoutesDeclaration {
            visibility,
            routes_fn_ident,
            routes_const_ident,
            route_mod_ident,
            state,
            entries,
        })
    }
}

/// Parses the optional leading `state: T;` clause.
///
/// It comes before the entries because it is a declaration-level concern (it
/// fixes the router's state type), not a route. The trailing `;` is required
/// so the clause reads as a declaration rather than an entry.
fn parse_state_clause(input: ParseStream<'_>) -> syn::Result<Option<syn::Type>> {
    if !input.peek(keyword::state) {
        return Ok(None);
    }
    let _: keyword::state = input.parse()?;
    let _: syn::Token![:] = input.parse()?;
    let ty: syn::Type = input.parse()?;
    let _: syn::Token![;] = input.parse()?;
    Ok(Some(ty))
}

/// Dispatches on the leading keyword of an entry.
fn parse_entry(input: ParseStream<'_>) -> syn::Result<RouteEntry> {
    let lookahead = input.lookahead1();
    let method = if lookahead.peek(method_keyword::get) {
        input
            .parse::<method_keyword::get>()
            .map(|_| RouteMethodKind::Get)
    } else if lookahead.peek(method_keyword::post) {
        input
            .parse::<method_keyword::post>()
            .map(|_| RouteMethodKind::Post)
    } else if lookahead.peek(method_keyword::put) {
        input
            .parse::<method_keyword::put>()
            .map(|_| RouteMethodKind::Put)
    } else if lookahead.peek(method_keyword::patch) {
        input
            .parse::<method_keyword::patch>()
            .map(|_| RouteMethodKind::Patch)
    } else if lookahead.peek(method_keyword::delete) {
        input
            .parse::<method_keyword::delete>()
            .map(|_| RouteMethodKind::Delete)
    } else if lookahead.peek(method_keyword::head) {
        input
            .parse::<method_keyword::head>()
            .map(|_| RouteMethodKind::Head)
    } else if lookahead.peek(method_keyword::options) {
        input
            .parse::<method_keyword::options>()
            .map(|_| RouteMethodKind::Options)
    } else if lookahead.peek(keyword::group) {
        return parse_group(input).map(RouteEntry::Group);
    } else if lookahead.peek(keyword::resource) {
        return parse_resource(input).map(RouteEntry::Resource);
    } else {
        return Err(lookahead.error());
    }?;

    parse_route(input, method).map(RouteEntry::Route)
}

/// Parses the remainder of a route entry, after its method keyword:
/// `"/path" => handler { options }`.
fn parse_route(input: ParseStream<'_>, method: RouteMethodKind) -> syn::Result<SingleRoute> {
    let path_lit: syn::LitStr = input.parse()?;
    let _: syn::Token![=>] = input.parse()?;
    let handler: syn::Path = input.parse()?;
    let options = options::parse(input)?;

    Ok(SingleRoute {
        method,
        path: path_lit.value(),
        handler,
        name: options.name,
        pages: options.pages,
        action: options.action,
        query: options.query.map(Box::new),
        query_string: options.query_string,
    })
}

/// Parses `group "/prefix" { middleware: [..]; <entries> }`.
fn parse_group(input: ParseStream<'_>) -> syn::Result<RouteGroup> {
    let _: keyword::group = input.parse()?;
    let prefix_lit: syn::LitStr = input.parse()?;

    let content;
    syn::braced!(content in input);

    let mut middleware = Vec::new();
    let mut entries = Vec::new();

    while !content.is_empty() {
        if content.peek(keyword::middleware) {
            let _: keyword::middleware = content.parse()?;
            let _: syn::Token![:] = content.parse()?;
            middleware = list::paths(&content)?;
            let _: Option<syn::Token![;]> = content.parse()?;
        } else {
            entries.push(parse_entry(&content)?);
            let _: Option<syn::Token![,]> = content.parse()?;
        }
    }

    Ok(RouteGroup {
        prefix: prefix_lit.value(),
        middleware,
        entries,
    })
}

/// Parses `resource "/path" => Controller { name: .., only: [..], .. }`.
fn parse_resource(input: ParseStream<'_>) -> syn::Result<ResourceDeclaration> {
    let _: keyword::resource = input.parse()?;
    let path_lit: syn::LitStr = input.parse()?;
    let _: syn::Token![=>] = input.parse()?;
    let controller: syn::Path = input.parse()?;

    let content;
    syn::braced!(content in input);

    let mut name = String::new();
    let mut only = Vec::new();
    let mut except = Vec::new();
    let mut bind = None;
    let mut middleware = Vec::new();

    while !content.is_empty() {
        let lookahead = content.lookahead1();
        if lookahead.peek(keyword::name) {
            let _: keyword::name = content.parse()?;
            let _: syn::Token![:] = content.parse()?;
            name = list::dotted_name(&content)?;
        } else if lookahead.peek(keyword::only) {
            let _: keyword::only = content.parse()?;
            let _: syn::Token![:] = content.parse()?;
            only = list::idents(&content)?;
        } else if lookahead.peek(keyword::except) {
            let _: keyword::except = content.parse()?;
            let _: syn::Token![:] = content.parse()?;
            except = list::idents(&content)?;
        } else if lookahead.peek(keyword::bind) {
            let _: keyword::bind = content.parse()?;
            let _: syn::Token![:] = content.parse()?;
            bind = Some(content.parse()?);
        } else if lookahead.peek(keyword::middleware) {
            let _: keyword::middleware = content.parse()?;
            let _: syn::Token![:] = content.parse()?;
            middleware = list::paths(&content)?;
        } else {
            return Err(lookahead.error());
        }
        let _: Option<syn::Token![,]> = content.parse()?;
    }

    if name.is_empty() {
        return Err(syn::Error::new(
            syn::spanned::Spanned::span(&controller),
            "a resource requires a `name` option, e.g. \
             `resource \"/links\" => LinksController { name: links }`",
        ));
    }

    Ok(ResourceDeclaration {
        path: path_lit.value(),
        controller,
        name,
        only,
        except,
        bind,
        middleware,
    })
}

#[cfg(test)]
mod tests {
    use super::{RouteEntry, RoutesDeclaration};
    use crate::routes::method::RouteMethodKind;

    fn parse(tokens: proc_macro2::TokenStream) -> syn::Result<RoutesDeclaration> {
        syn::parse2::<RoutesDeclaration>(tokens)
    }

    #[test]
    fn generated_idents_follow_the_declaration_name() {
        let decl = parse(quote::quote! {
            pub app { get "/" => home { name: home } }
        })
        .unwrap();
        assert_eq!(decl.routes_fn_ident.to_string(), "app_routes");
        assert_eq!(decl.routes_const_ident.to_string(), "APP_ROUTES");
        assert_eq!(decl.route_mod_ident.to_string(), "app_route");
    }

    #[test]
    fn state_clause_is_optional() {
        let decl = parse(quote::quote! { app { get "/" => home } }).unwrap();
        assert!(decl.state.is_none());
    }

    #[test]
    fn state_clause_is_captured() {
        let decl = parse(quote::quote! {
            app { state: AppState; get "/" => home }
        })
        .unwrap();
        assert!(decl.state.is_some());
    }

    #[test]
    fn every_method_keyword_opens_a_route() {
        let decl = parse(quote::quote! {
            app {
                get "/a" => a
                post "/b" => b
                put "/c" => c
                patch "/d" => d
                delete "/e" => e
                head "/f" => f
                options "/g" => g
            }
        })
        .unwrap();
        assert_eq!(decl.entries.len(), 7);
        let RouteEntry::Route(first) = &decl.entries[0] else {
            panic!("expected a route");
        };
        assert_eq!(first.method, RouteMethodKind::Get);
    }

    #[test]
    fn groups_nest() {
        let decl = parse(quote::quote! {
            app {
                group "/auth" {
                    middleware: [Guest];
                    get "/login" => SessionsController::create { name: auth.login }
                    group "/admin" {
                        get "/panel" => panel { name: auth.admin.panel }
                    }
                }
            }
        })
        .unwrap();
        let RouteEntry::Group(group) = &decl.entries[0] else {
            panic!("expected a group");
        };
        assert_eq!(group.prefix, "/auth");
        assert_eq!(group.middleware.len(), 1);
        assert_eq!(group.entries.len(), 2);
    }

    #[test]
    fn resource_options_are_captured() {
        let decl = parse(quote::quote! {
            app {
                resource "/links" => LinksController {
                    name: links,
                    only: [index, show],
                    except: [edit],
                    bind: Link,
                    middleware: [Auth]
                }
            }
        })
        .unwrap();
        let RouteEntry::Resource(resource) = &decl.entries[0] else {
            panic!("expected a resource");
        };
        assert_eq!(resource.name, "links");
        assert_eq!(resource.only, vec!["index".to_string(), "show".to_string()]);
        assert_eq!(resource.except, vec!["edit".to_string()]);
        assert!(resource.bind.is_some());
        assert_eq!(resource.middleware.len(), 1);
    }

    #[test]
    fn a_resource_without_a_name_is_rejected() {
        let error = parse(quote::quote! {
            app { resource "/links" => LinksController { only: [index] } }
        })
        .unwrap_err();
        assert!(error.to_string().contains("requires a `name` option"));
    }

    #[test]
    fn an_unknown_entry_keyword_is_rejected() {
        let error = parse(quote::quote! { app { fetch "/" => home } }).unwrap_err();
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn query_and_query_string_reach_the_route() {
        let decl = parse(quote::quote! {
            app {
                get "/links" => LinksController::index {
                    name: links.index,
                    query: Vec<LinkResource>,
                    query_string: LinkSearchRequest
                }
            }
        })
        .unwrap();
        let RouteEntry::Route(route) = &decl.entries[0] else {
            panic!("expected a route");
        };
        assert!(route.query.is_some());
        assert!(route.query_string.is_some());
    }
}
