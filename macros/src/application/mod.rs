//! `application!` -- composes modules into a validated application graph.
//!
//! ```ignore
//! // Not compiled: `arcature-macros` cannot depend on `arcature`
//! // (that is the cycle `lib.rs` describes), so an example naming
//! // Arcature items has nothing here to compile against.
//! application! {
//!     pub App {
//!         modules: [accounts::accounts_module, links::links_module],
//!         routes: [accounts::routes, links::routes],
//!         state: AppState,
//!         page_contracts: [home::HomePage, links::NewLinkPage],
//!     }
//! }
//! ```
//!
//! Only `modules:` is required; it lists the `<name>_module()` accessors
//! `module!` generated.
//!
//! ## What this macro does NOT do
//!
//! `application!` does not build or run the application. It emits free
//! functions -- `app_graph()`, `app_routes()`, and optionally
//! `app_page_contracts()` -- that the application's own bootstrap calls.
//! `app_graph()` is a side-effect-free validation gate: it binds no socket,
//! opens no connection, and starts no worker. Running the application stays
//! the `ApplicationBuilder`'s job.
//!
//! Each responsibility lives in its own file:
//!
//! * [`declaration`] -- the parsed [`ApplicationDeclaration`], its
//!   `syn::Parse` implementation, and duplicate-module validation.
//! * [`expand`] -- code generation for the three functions.

pub mod declaration;
pub mod expand;

pub use declaration::ApplicationDeclaration;

use proc_macro2::TokenStream;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};

/// The implementation of `application!`. Called by the thin `lib.rs`
/// entrypoint: parse, validate, expand. Returns a [`MacroError`] (converted
/// to `compile_error!` by the entrypoint) on failure -- never panics.
pub fn application(input: TokenStream) -> MacroResult {
    let declaration: ApplicationDeclaration =
        syn::parse2(input).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;
    declaration::validate(&declaration)?;
    Ok(expand::expand(&declaration))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn expands_a_full_application_declaration() {
        let s = application(quote! {
            pub App {
                modules: [accounts::accounts_module],
                routes: [accounts::routes],
                state: AppState,
                page_contracts: [home::HomePage],
            }
        })
        .unwrap()
        .to_string();

        assert!(s.contains("fn app_graph ()"), "got: {s}");
        assert!(s.contains("fn app_routes ()"), "got: {s}");
        assert!(s.contains("fn app_page_contracts ()"), "got: {s}");
    }

    #[test]
    fn reports_a_syntax_error_as_arc_m001() {
        let err = application(quote! { pub App { widgets: [X] } }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM001);
    }

    #[test]
    fn reports_a_duplicate_module_as_arc_m002() {
        let err = application(quote! { App { modules: [a::m, a::m] } }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }
}
