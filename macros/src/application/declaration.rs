//! [`ApplicationDeclaration`]: the parsed `application!` body, its
//! `syn::Parse` implementation, and its duplicate-module validation.

use std::collections::BTreeSet;

use syn::parse::ParseStream;
use syn::spanned::Spanned;

use crate::diagnostic::{MacroError, MacroErrorCode};

/// The parsed `application!` input.
#[derive(Debug)]
pub struct ApplicationDeclaration {
    /// Whether the generated items are `pub`.
    pub visibility: syn::Visibility,
    /// The application name (e.g. `"App"`).
    pub name: String,
    /// The identifier the generated names are derived from (e.g.
    /// `APP_GRAPH`).
    pub ident: syn::Ident,
    /// The module accessor paths (e.g. `accounts::accounts_module`). Each
    /// resolves to a `fn() -> &'static ModuleDescriptor`.
    pub modules: Vec<syn::Path>,
    /// Optional router-builder paths, each a `fn() -> Routes<S>` that the
    /// generated `<name>_routes()` merges. When absent, `<name>_routes()`
    /// returns an empty `Routes<S>`, so an application that prefers a
    /// hand-written route-composition file keeps working.
    pub routes: Vec<syn::Path>,
    /// Optional router state type (the `S` in each builder's `Routes<S>`
    /// and the return type of `<name>_routes()`). Defaults to `()`.
    pub state: Option<syn::Path>,
    /// Optional page-contract type paths, each a type whose `#[page("...")]`
    /// generated a `PAGE_CONTRACT_ENTRY` const. The generated
    /// `<name>_page_contracts()` aggregates them into a `PageContracts`
    /// registry. When absent, no such function is generated.
    pub page_contracts: Vec<syn::Path>,
}

/// The section keywords the `application!` body accepts.
mod keyword {
    syn::custom_keyword!(modules);
    syn::custom_keyword!(routes);
    syn::custom_keyword!(state);
    syn::custom_keyword!(page_contracts);
}

impl syn::parse::Parse for ApplicationDeclaration {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let visibility: syn::Visibility = input.parse()?;
        let name_ident: syn::Ident = input.parse()?;
        let name = name_ident.to_string();
        let ident = syn::Ident::new(&format!("{}_GRAPH", name.to_uppercase()), name_ident.span());

        let content;
        syn::braced!(content in input);

        let mut declaration = ApplicationDeclaration {
            visibility,
            name,
            ident,
            modules: Vec::new(),
            routes: Vec::new(),
            state: None,
            page_contracts: Vec::new(),
        };

        while !content.is_empty() {
            declaration.parse_section(&content)?;
            let _: Option<syn::Token![,]> = content.parse()?;
        }

        if declaration.modules.is_empty() {
            return Err(syn::Error::new(
                name_ident.span(),
                "an application must have at least one module",
            ));
        }

        Ok(declaration)
    }
}

impl ApplicationDeclaration {
    /// Parses one `keyword: <payload>` section into `self`.
    fn parse_section(&mut self, input: ParseStream<'_>) -> syn::Result<()> {
        let lookahead = input.lookahead1();

        if lookahead.peek(keyword::modules) {
            input.parse::<keyword::modules>()?;
            input.parse::<syn::Token![:]>()?;
            self.modules = parse_path_list(input)?;
        } else if lookahead.peek(keyword::routes) {
            input.parse::<keyword::routes>()?;
            input.parse::<syn::Token![:]>()?;
            self.routes = parse_path_list(input)?;
        } else if lookahead.peek(keyword::state) {
            input.parse::<keyword::state>()?;
            input.parse::<syn::Token![:]>()?;
            self.state = Some(input.parse()?);
        } else if lookahead.peek(keyword::page_contracts) {
            input.parse::<keyword::page_contracts>()?;
            input.parse::<syn::Token![:]>()?;
            self.page_contracts = parse_path_list(input)?;
        } else {
            return Err(lookahead.error());
        }

        Ok(())
    }
}

/// Parses `[path::one, path::two, ...]` into the paths it lists.
fn parse_path_list(input: ParseStream<'_>) -> syn::Result<Vec<syn::Path>> {
    let content;
    syn::bracketed!(content in input);
    Ok(
        syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated(&content)?
            .into_iter()
            .collect(),
    )
}

/// Validates a parsed declaration: the same module may not be listed twice.
pub fn validate(declaration: &ApplicationDeclaration) -> Result<(), MacroError> {
    let mut seen = BTreeSet::new();
    for path in &declaration.modules {
        let rendered = render_path(path);
        if !seen.insert(rendered.clone()) {
            return Err(MacroError::new(
                MacroErrorCode::ArcM002,
                path.span(),
                format!("duplicate module path `{rendered}`"),
            ));
        }
    }
    Ok(())
}

/// Renders a path as its `::`-joined segment idents, for diagnostics and
/// duplicate detection.
fn render_path(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn parse(tokens: proc_macro2::TokenStream) -> syn::Result<ApplicationDeclaration> {
        syn::parse2(tokens)
    }

    #[test]
    fn derives_the_graph_ident_from_the_application_name() {
        let declaration = parse(quote! { pub App { modules: [a::a_module] } }).unwrap();
        assert_eq!(declaration.name, "App");
        assert_eq!(declaration.ident.to_string(), "APP_GRAPH");
    }

    #[test]
    fn parses_every_section() {
        let declaration = parse(quote! {
            pub App {
                modules: [accounts::accounts_module, links::links_module],
                routes: [accounts::routes, links::routes],
                state: AppState,
                page_contracts: [home::HomePage],
            }
        })
        .unwrap();
        assert_eq!(declaration.modules.len(), 2);
        assert_eq!(declaration.routes.len(), 2);
        assert!(declaration.state.is_some());
        assert_eq!(declaration.page_contracts.len(), 1);
    }

    #[test]
    fn optional_sections_default_to_empty() {
        let declaration = parse(quote! { App { modules: [a::a_module] } }).unwrap();
        assert!(declaration.routes.is_empty());
        assert!(declaration.state.is_none());
        assert!(declaration.page_contracts.is_empty());
    }

    #[test]
    fn requires_at_least_one_module() {
        let err = parse(quote! { App { modules: [] } }).unwrap_err();
        assert!(err.to_string().contains("at least one module"));
    }

    #[test]
    fn rejects_an_unknown_section() {
        assert!(parse(quote! { App { modules: [a::m], widgets: [X] } }).is_err());
    }

    #[test]
    fn validate_accepts_distinct_modules() {
        let declaration = parse(quote! { App { modules: [a::a_module, b::b_module] } }).unwrap();
        assert!(validate(&declaration).is_ok());
    }

    #[test]
    fn validate_rejects_a_duplicate_module() {
        let declaration = parse(quote! { App { modules: [a::a_module, a::a_module] } }).unwrap();
        let err = validate(&declaration).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
        assert!(err.to_compile_error().to_string().contains("a::a_module"));
    }
}
