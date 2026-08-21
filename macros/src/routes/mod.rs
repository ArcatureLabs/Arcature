//! The `routes!` declaration DSL.
//!
//! One responsibility: drive the parse -> validate -> expand pipeline for a
//! `routes!` block. Each stage lives in its own module; this one only wires
//! them together.
//!
//! ```ignore
//! routes! {
//!     pub app {
//!         state: AppState;
//!
//!         get "/" => home { name: home, page: "Home" }
//!
//!         group "/auth" {
//!             middleware: [Guest];
//!             get  "/login" => SessionsController::create { name: auth.login }
//!             post "/login" => SessionsController::store  {
//!                 name: auth.store,
//!                 action: LoginRequest
//!             }
//!         }
//!
//!         resource "/links" => LinksController {
//!             name: links,
//!             except: [edit],
//!             middleware: [Auth]
//!         }
//!     }
//! }
//! ```
//!
//! The block above generates three items:
//!
//! - `pub fn app_routes() -> Routes<AppState>` -- the routes themselves, with
//!   group prefixes already resolved and middleware scoped to its group.
//! - `pub const APP_ROUTES: &[RouteDescriptor]` -- the inspection metadata
//!   behind `arc routes`, `arc build`, and the Unified Application Graph.
//! - `pub mod app_route` -- typed URL helpers (`app_route::auth::login()`),
//!   so a misspelled route name fails to compile rather than 404.

mod action;
mod declaration;
mod expand;
mod flatten;
mod helper_module;
mod keywords;
mod list;
mod metadata_const;
mod method;
mod options;
mod path;
mod router_fn;
mod type_name;
mod validate;

use proc_macro2::TokenStream;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};
use declaration::RoutesDeclaration;

/// Expands a `routes!` invocation.
pub fn routes(input: TokenStream) -> MacroResult {
    let declaration = syn::parse2::<RoutesDeclaration>(input)
        .map_err(|error| MacroError::from_syn(MacroErrorCode::ArcM001, error))?;
    validate::validate(&declaration)?;
    Ok(expand::expand(&declaration))
}

#[cfg(test)]
mod tests {
    use super::routes;
    use crate::diagnostic::MacroErrorCode;

    #[test]
    fn a_full_declaration_expands() {
        let out = routes(quote::quote! {
            pub app {
                state: AppState;
                get "/" => home { name: home, page: "Home" }
                group "/auth" {
                    middleware: [Guest];
                    get "/login" => SessionsController::create { name: auth.login }
                }
                resource "/links" => LinksController { name: links, except: [edit] }
            }
        })
        .unwrap()
        .to_string();

        assert!(out.contains("fn app_routes"));
        assert!(out.contains("const APP_ROUTES"));
        assert!(out.contains("mod app_route"));
        assert!(out.contains("\"/auth/login\""));
        assert!(!out.contains("links.edit"));
    }

    #[test]
    fn a_syntax_error_reports_arc_m001() {
        let error = routes(quote::quote! { app { get => home } }).unwrap_err();
        assert_eq!(error.code(), MacroErrorCode::ArcM001);
    }

    #[test]
    fn a_semantic_error_reports_its_own_code() {
        let error = routes(quote::quote! {
            app {
                get "/a" => a { name: home }
                get "/b" => b { name: home }
            }
        })
        .unwrap_err();
        assert_eq!(error.code(), MacroErrorCode::ArcM002);
    }

    #[test]
    fn an_unknown_resource_action_reports_arc_m003() {
        let error = routes(quote::quote! {
            app { resource "/l" => C { name: l, only: [list] } }
        })
        .unwrap_err();
        assert_eq!(error.code(), MacroErrorCode::ArcM003);
    }
}
