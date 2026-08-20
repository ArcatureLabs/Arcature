//! The optional `{ ... }` block that follows a route handler.
//!
//! One responsibility: parse `name:`, `page:`/`pages:`, `action:`, `query:`,
//! and `query_string:` into a [`RouteOptions`], rejecting the combinations
//! that are contradictory by construction:
//!
//! - `page:` and `pages:` on the same route (one declares one page, the other
//!   declares the set; declaring both says nothing coherent).
//! - `action:` and `query:` on the same route (a route mutates or it reads).
//! - `query_string:` without `query:` (a query-string contract is the typed
//!   input of a query route; there is nothing for it to belong to otherwise).
//!
//! Constraints that need the HTTP method (an action must not be a GET, a
//! query must be one) are checked in `validate`, after the method keyword is
//! known.

use syn::parse::ParseStream;

use super::keywords::option as keyword;
use super::list;

/// The parsed contents of a route's options block.
#[derive(Debug, Default)]
pub struct RouteOptions {
    /// The dotted route name (`name: auth.login`).
    pub name: Option<String>,
    /// The Inertia pages this route renders (`page:` or `pages:`).
    pub pages: Vec<String>,
    /// The typed request type of an Action route (`action: StoreLinkRequest`).
    pub action: Option<syn::Path>,
    /// The typed response type of a Query route (`query: Vec<LinkResource>`).
    pub query: Option<syn::Type>,
    /// The typed query-string contract of a Query route.
    pub query_string: Option<syn::Path>,
}

/// Parses the options block, or returns the defaults when no block follows.
pub fn parse(input: ParseStream<'_>) -> syn::Result<RouteOptions> {
    if !input.peek(syn::token::Brace) {
        return Ok(RouteOptions::default());
    }

    let content;
    syn::braced!(content in input);

    let mut options = RouteOptions::default();
    let mut saw_page = false;
    let mut saw_pages = false;

    while !content.is_empty() {
        let lookahead = content.lookahead1();
        if lookahead.peek(keyword::name) {
            let _: keyword::name = content.parse()?;
            let _: syn::Token![:] = content.parse()?;
            options.name = Some(list::dotted_name(&content)?);
        } else if lookahead.peek(keyword::page) {
            if saw_pages {
                return Err(both_page_forms(content.span()));
            }
            let _: keyword::page = content.parse()?;
            let _: syn::Token![:] = content.parse()?;
            let lit: syn::LitStr = content.parse()?;
            options.pages.push(lit.value());
            saw_page = true;
        } else if lookahead.peek(keyword::pages) {
            if saw_page {
                return Err(both_page_forms(content.span()));
            }
            let _: keyword::pages = content.parse()?;
            let _: syn::Token![:] = content.parse()?;
            options.pages = list::strings(&content)?;
            if options.pages.is_empty() {
                return Err(syn::Error::new(
                    content.span(),
                    "`pages:` must list at least one page, e.g. `pages: [\"Home\"]`",
                ));
            }
            saw_pages = true;
        } else if lookahead.peek(keyword::action) {
            if options.query.is_some() {
                return Err(both_action_and_query(content.span()));
            }
            let _: keyword::action = content.parse()?;
            let _: syn::Token![:] = content.parse()?;
            options.action = Some(content.parse()?);
        } else if lookahead.peek(keyword::query) {
            if options.action.is_some() {
                return Err(both_action_and_query(content.span()));
            }
            let _: keyword::query = content.parse()?;
            let _: syn::Token![:] = content.parse()?;
            options.query = Some(content.parse()?);
        } else if lookahead.peek(keyword::query_string) {
            if options.query.is_none() {
                return Err(syn::Error::new(
                    content.span(),
                    "`query_string:` requires `query:` on the same route \
                     (a query-string contract is a query route's input)",
                ));
            }
            let _: keyword::query_string = content.parse()?;
            let _: syn::Token![:] = content.parse()?;
            options.query_string = Some(content.parse()?);
        } else {
            return Err(lookahead.error());
        }
        let _: Option<syn::Token![,]> = content.parse()?;
    }

    Ok(options)
}

fn both_page_forms(span: proc_macro2::Span) -> syn::Error {
    syn::Error::new(span, "a route may declare `page:` or `pages:`, not both")
}

fn both_action_and_query(span: proc_macro2::Span) -> syn::Error {
    syn::Error::new(span, "a route may declare `action:` or `query:`, not both")
}

#[cfg(test)]
mod tests {
    use super::parse;
    use syn::parse::Parser as _;

    #[test]
    fn no_block_yields_defaults() {
        let options = parse.parse2(quote::quote!()).unwrap();
        assert!(options.name.is_none());
        assert!(options.pages.is_empty());
    }

    #[test]
    fn name_page_and_action_are_parsed() {
        let options = parse
            .parse2(quote::quote!({ name: links.store, page: "Links/Create", action: StoreLinkRequest }))
            .unwrap();
        assert_eq!(options.name.as_deref(), Some("links.store"));
        assert_eq!(options.pages, vec!["Links/Create".to_string()]);
        assert!(options.action.is_some());
    }

    #[test]
    fn pages_list_is_parsed() {
        let options = parse
            .parse2(quote::quote!({ pages: ["Home", "Welcome"] }))
            .unwrap();
        assert_eq!(options.pages.len(), 2);
    }

    #[test]
    fn page_and_pages_together_are_rejected() {
        let error = parse
            .parse2(quote::quote!({ page: "Home", pages: ["Home"] }))
            .unwrap_err();
        assert!(error.to_string().contains("not both"));
    }

    #[test]
    fn empty_pages_list_is_rejected() {
        let error = parse.parse2(quote::quote!({ pages: [] })).unwrap_err();
        assert!(error.to_string().contains("at least one page"));
    }

    #[test]
    fn action_and_query_together_are_rejected() {
        let error = parse
            .parse2(quote::quote!({ action: StoreLink, query: LinkResource }))
            .unwrap_err();
        assert!(error.to_string().contains("not both"));
    }

    #[test]
    fn query_string_without_query_is_rejected() {
        let error = parse
            .parse2(quote::quote!({ query_string: SearchRequest }))
            .unwrap_err();
        assert!(error.to_string().contains("requires `query:`"));
    }

    #[test]
    fn query_string_with_query_is_accepted() {
        let options = parse
            .parse2(quote::quote!({ query: Vec<LinkResource>, query_string: SearchRequest }))
            .unwrap();
        assert!(options.query.is_some());
        assert!(options.query_string.is_some());
    }

    #[test]
    fn an_unknown_option_is_rejected() {
        let error = parse.parse2(quote::quote!({ nope: 1 })).unwrap_err();
        assert!(!error.to_string().is_empty());
    }
}
