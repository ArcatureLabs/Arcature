//! Semantic checks over a parsed `routes!` declaration.
//!
//! One responsibility: reject declarations the grammar accepts but the
//! framework cannot honour -- duplicate route names (the name-to-URL map
//! would be ambiguous), unknown resource actions, and the method/intent
//! mismatches that let a GET mutate or a mutation masquerade as a read.

use std::collections::BTreeSet;

use syn::spanned::Spanned as _;

use super::action;
use super::declaration::{RouteEntry, RoutesDeclaration};
use super::method::RouteMethodKind;
use crate::diagnostic::{MacroError, MacroErrorCode};

/// Validates a parsed declaration.
pub fn validate(decl: &RoutesDeclaration) -> Result<(), MacroError> {
    let mut seen = BTreeSet::new();
    check_entries(&decl.entries, &mut seen)
}

fn check_entries(entries: &[RouteEntry], seen: &mut BTreeSet<String>) -> Result<(), MacroError> {
    for entry in entries {
        match entry {
            RouteEntry::Route(route) => {
                if let Some(name) = &route.name
                    && !seen.insert(name.clone())
                {
                    return Err(MacroError::new(
                        MacroErrorCode::ArcM002,
                        route.handler.span(),
                        format!("duplicate route name `{name}`"),
                    ));
                }
                if route.action.is_some() && !route.method.is_unsafe() {
                    return Err(MacroError::new(
                        MacroErrorCode::ArcM002,
                        route.handler.span(),
                        "an `action:` route must use a non-safe method \
                         (POST/PUT/PATCH/DELETE); GET is for `query:`",
                    ));
                }
                if route.query.is_some() && route.method != RouteMethodKind::Get {
                    return Err(MacroError::new(
                        MacroErrorCode::ArcM002,
                        route.handler.span(),
                        "a `query:` route must use GET; mutations are `action:`",
                    ));
                }
            }
            RouteEntry::Group(group) => check_entries(&group.entries, seen)?,
            RouteEntry::Resource(resource) => {
                let span = resource.controller.span();
                check_action_names(&resource.only, "only", span)?;
                check_action_names(&resource.except, "except", span)?;

                for name in action::selected(&resource.only, &resource.except) {
                    let full = format!("{}.{name}", resource.name);
                    if !seen.insert(full.clone()) {
                        return Err(MacroError::new(
                            MacroErrorCode::ArcM002,
                            span,
                            format!("duplicate route name `{full}`"),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn check_action_names(
    names: &[String],
    list: &str,
    span: proc_macro2::Span,
) -> Result<(), MacroError> {
    for name in names {
        if !action::is_valid(name) {
            return Err(MacroError::new(
                MacroErrorCode::ArcM003,
                span,
                format!(
                    "unknown resource action `{name}` in `{list}`; expected one of: {}",
                    action::ALL.join(", ")
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate;
    use crate::diagnostic::MacroErrorCode;
    use crate::routes::declaration::RoutesDeclaration;

    fn check(tokens: proc_macro2::TokenStream) -> Result<(), MacroErrorCode> {
        let decl = syn::parse2::<RoutesDeclaration>(tokens).expect("parses");
        validate(&decl).map_err(|e| e.code())
    }

    #[test]
    fn a_clean_declaration_validates() {
        check(quote::quote! {
            app {
                get "/" => home { name: home }
                resource "/links" => LinksController { name: links }
            }
        })
        .unwrap();
    }

    #[test]
    fn duplicate_route_names_are_rejected() {
        let code = check(quote::quote! {
            app {
                get "/a" => a { name: home }
                get "/b" => b { name: home }
            }
        })
        .unwrap_err();
        assert_eq!(code, MacroErrorCode::ArcM002);
    }

    #[test]
    fn duplicate_names_across_a_group_boundary_are_rejected() {
        let code = check(quote::quote! {
            app {
                get "/a" => a { name: home }
                group "/x" { get "/b" => b { name: home } }
            }
        })
        .unwrap_err();
        assert_eq!(code, MacroErrorCode::ArcM002);
    }

    #[test]
    fn a_resource_colliding_with_a_route_name_is_rejected() {
        let code = check(quote::quote! {
            app {
                get "/l" => l { name: links.index }
                resource "/links" => LinksController { name: links }
            }
        })
        .unwrap_err();
        assert_eq!(code, MacroErrorCode::ArcM002);
    }

    #[test]
    fn unnamed_routes_never_collide() {
        check(quote::quote! {
            app {
                get "/a" => a
                get "/b" => b
            }
        })
        .unwrap();
    }

    #[test]
    fn a_get_action_is_rejected() {
        let code = check(quote::quote! {
            app { get "/links" => store { action: StoreLinkRequest } }
        })
        .unwrap_err();
        assert_eq!(code, MacroErrorCode::ArcM002);
    }

    #[test]
    fn a_post_query_is_rejected() {
        let code = check(quote::quote! {
            app { post "/links" => index { query: Vec<LinkResource> } }
        })
        .unwrap_err();
        assert_eq!(code, MacroErrorCode::ArcM002);
    }

    #[test]
    fn an_unknown_only_action_is_rejected() {
        let code = check(quote::quote! {
            app { resource "/links" => C { name: links, only: [list] } }
        })
        .unwrap_err();
        assert_eq!(code, MacroErrorCode::ArcM003);
    }

    #[test]
    fn an_unknown_except_action_is_rejected() {
        let code = check(quote::quote! {
            app { resource "/links" => C { name: links, except: [list] } }
        })
        .unwrap_err();
        assert_eq!(code, MacroErrorCode::ArcM003);
    }
}
