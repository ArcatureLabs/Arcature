//! Code generation for `application!`.
//!
//! Emits up to three free functions, all named after the application:
//!
//! * `<name>_graph()` -- collects the declared modules' descriptors and
//!   builds a validated [`ApplicationGraph`].
//! * `<name>_routes()` -- merges the declared route builders into one
//!   `Routes<S>`.
//! * `<name>_page_contracts()` -- aggregates the declared page contracts
//!   into a `PageContracts` registry (only when `page_contracts:` is given).

use proc_macro2::TokenStream;
use quote::quote;

use super::declaration::ApplicationDeclaration;

/// Generates the application's graph, route-composition, and page-contract
/// functions.
pub fn expand(declaration: &ApplicationDeclaration) -> TokenStream {
    let graph = expand_graph_fn(declaration);
    let routes = expand_routes_fn(declaration);
    let page_contracts = expand_page_contracts_fn(declaration);

    quote! {
        #graph
        #routes
        #page_contracts
    }
}

/// The `<name>_graph()` function: collect the module descriptors and build
/// the validated graph.
fn expand_graph_fn(declaration: &ApplicationDeclaration) -> TokenStream {
    let visibility = &declaration.visibility;
    let ident = fn_ident(declaration, "graph");

    // Each entry is a path to a `fn() -> &'static ModuleDescriptor`. Cloning
    // the descriptor is cheap -- every field is a `&'static` slice, so the
    // clone allocates nothing.
    let descriptors = declaration
        .modules
        .iter()
        .map(|path| quote! { (*#path()).clone() });

    quote! {
        /// Builds the application module graph from the declared modules.
        ///
        /// Validates for duplicate modules, unknown imports, and circular
        /// dependencies, returning the first failure as a `GraphError`.
        ///
        /// This function is side-effect-free: it binds no socket, opens no
        /// database, cache, storage, or SMTP connection, starts no worker,
        /// and runs no migration. It is a validation gate the application
        /// calls before wiring the real runtime.
        #visibility fn #ident() -> ::std::result::Result<
            ::arcature::ApplicationGraph,
            ::arcature::GraphError,
        > {
            let modules: ::std::vec::Vec<::arcature::ModuleDescriptor> = ::std::vec![
                #(#descriptors),*
            ];
            ::arcature::ApplicationGraph::new(modules)
        }
    }
}

/// The `<name>_routes()` function: merge the declared route builders.
fn expand_routes_fn(declaration: &ApplicationDeclaration) -> TokenStream {
    let visibility = &declaration.visibility;
    let ident = fn_ident(declaration, "routes");

    let state = match &declaration.state {
        Some(path) => quote! { #path },
        None => quote! { () },
    };

    let body = match declaration.routes.split_first() {
        // Seeding from the first builder rather than an empty `Routes` lets
        // the concrete state type come from the builders' signatures.
        Some((first, rest)) => {
            let merges = rest.iter().map(|path| quote! { .merge(#path()) });
            quote! { #first() #(#merges)* }
        }
        None => quote! { ::arcature::Routes::<#state>::new(::std::vec::Vec::new()) },
    };

    quote! {
        /// Composes the application router from the declared module route
        /// builders.
        ///
        /// Each entry in the `routes:` section is a zero-argument
        /// `fn() -> Routes<S>`, typically a `routes!`-generated accessor;
        /// this merges them in declaration order into one `Routes<S>`, where
        /// `S` is the type declared in the `state:` clause. With no
        /// `routes:` section it returns an empty router, so an application
        /// composing routes by hand is unaffected.
        ///
        /// Cross-cutting layers stay explicit: apply them with `.layer(...)`
        /// on the returned router.
        #visibility fn #ident() -> ::arcature::Routes<#state> {
            #body
        }
    }
}

/// The `<name>_page_contracts()` function, generated only when the
/// `page_contracts:` section is present.
fn expand_page_contracts_fn(declaration: &ApplicationDeclaration) -> TokenStream {
    if declaration.page_contracts.is_empty() {
        return TokenStream::new();
    }

    let visibility = &declaration.visibility;
    let ident = fn_ident(declaration, "page_contracts");
    let pages = &declaration.page_contracts;

    quote! {
        /// Aggregates the declared page contracts into a typed
        /// `PageContracts` registry.
        ///
        /// Each entry is a type with a `PAGE_CONTRACT_ENTRY` const generated
        /// by `#[page("...")]`, so this replaces a hand-written registration
        /// chain. The Client Exposure Firewall holds: that const only exists
        /// for types implementing `ClientData`, so a `Serialize`-only type
        /// can never enter the registry. Duplicate and case-conflicting page
        /// identities are rejected here.
        ///
        /// The expansion names `::arcature::inertia::PageContracts`, so the
        /// `arcature` dependency needs its `inertia` feature enabled. An
        /// application without Inertia simply declares no `page_contracts:`
        /// section.
        #visibility fn #ident() -> ::std::result::Result<
            ::arcature::inertia::PageContracts,
            ::arcature::inertia::ContractError,
        > {
            let mut registry = ::arcature::inertia::PageContracts::new();
            #(
                registry = registry.register_entry(&#pages::PAGE_CONTRACT_ENTRY)?;
            )*
            ::std::result::Result::Ok(registry)
        }
    }
}

/// Builds `<name>_<suffix>` as an identifier, lower-cased from the declared
/// application name.
fn fn_ident(declaration: &ApplicationDeclaration, suffix: &str) -> syn::Ident {
    syn::Ident::new(
        &format!("{}_{suffix}", declaration.name.to_lowercase()),
        declaration.ident.span(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn expand_application(tokens: proc_macro2::TokenStream) -> String {
        let declaration: ApplicationDeclaration =
            syn::parse2(tokens).expect("application should parse");
        expand(&declaration).to_string()
    }

    #[test]
    fn emits_the_graph_function_calling_each_module_accessor() {
        let s = expand_application(quote! {
            pub App { modules: [accounts::accounts_module, links::links_module] }
        });
        assert!(s.contains("fn app_graph ()"), "got: {s}");
        assert!(s.contains("ApplicationGraph :: new"), "got: {s}");
        assert!(s.contains("accounts :: accounts_module ()"), "got: {s}");
        assert!(s.contains("links :: links_module ()"), "got: {s}");
    }

    #[test]
    fn honours_the_declared_visibility() {
        assert!(expand_application(quote! { pub App { modules: [a::m] } }).contains("pub fn"));
        assert!(!expand_application(quote! { App { modules: [a::m] } }).contains("pub fn"));
    }

    #[test]
    fn routes_default_to_an_empty_unit_state_router() {
        let s = expand_application(quote! { App { modules: [a::m] } });
        assert!(s.contains("Routes :: < () > :: new"), "got: {s}");
    }

    #[test]
    fn routes_merge_in_declaration_order() {
        let s = expand_application(quote! {
            App {
                modules: [a::m],
                routes: [accounts::routes, links::routes],
                state: AppState,
            }
        });
        assert!(
            s.contains("-> :: arcature :: Routes < AppState >"),
            "got: {s}"
        );
        assert!(
            s.contains("accounts :: routes () . merge (links :: routes ())"),
            "got: {s}"
        );
    }

    #[test]
    fn no_page_contracts_function_without_the_section() {
        let s = expand_application(quote! { App { modules: [a::m] } });
        assert!(!s.contains("page_contracts"), "got: {s}");
    }

    #[test]
    fn page_contracts_function_registers_each_entry() {
        let s = expand_application(quote! {
            App {
                modules: [a::m],
                page_contracts: [home::HomePage, links::NewLinkPage],
            }
        });
        assert!(s.contains("fn app_page_contracts ()"), "got: {s}");
        assert!(
            s.contains("home :: HomePage :: PAGE_CONTRACT_ENTRY"),
            "got: {s}"
        );
        assert!(
            s.contains("links :: NewLinkPage :: PAGE_CONTRACT_ENTRY"),
            "got: {s}"
        );
    }
}
