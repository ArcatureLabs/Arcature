//! `module!` -- declares a module's metadata as a `const ModuleDescriptor`.
//!
//! A module is the unit of composition in the Arcature DSL: it names the
//! controllers, services, policies, routes, pages, listeners, jobs,
//! commands, and schedules that belong together, and the modules it
//! imports.
//!
//! ```ignore
//! module! {
//!     pub Accounts {
//!         imports: [Notifications],
//!         exports: [AuthenticationService],
//!         controllers: [SessionsController, UsersController],
//!         services: [AuthenticationService],
//!         policies: [UserPolicy],
//!         routes: ACCOUNTS_ROUTES,
//!         listeners: [
//!             UserRegistered => send_welcome,
//!         ],
//!         jobs: [
//!             send_email v1 => handle_send_email,
//!         ],
//!         commands: [
//!             "users:prune" => prune_users,
//!         ],
//!         schedules: [
//!             cleanup_sessions every "5m",
//!         ],
//!         pages: [pages::SignInPage, pages::ProfilePage],
//!     }
//! }
//! ```
//!
//! Every section is optional and defaults to empty; section order is free.
//!
//! ## What this macro does NOT do
//!
//! `module!` does not build or run anything. It emits a `const
//! ModuleDescriptor` plus a `<name>_module()` accessor -- pure, allocation
//! free metadata. Wiring stays explicit: the application registers
//! controllers, services, listeners, job handlers, and commands itself, and
//! `application!` validates the graph these descriptors form.
//!
//! Each responsibility lives in its own file:
//!
//! * [`declaration`] -- the parsed [`ModuleDeclaration`] and its `syn::Parse`
//!   implementation.
//! * [`schedule_spec`] -- the [`ScheduleSpec`] cadence and its interval/time
//!   string parsers.
//! * [`validate`] -- duplicate-entry checks across the sections.
//! * [`expand`] -- code generation for the descriptor const and accessor.

pub mod declaration;
pub mod expand;
pub mod schedule_spec;
pub mod validate;

pub use declaration::ModuleDeclaration;

use proc_macro2::TokenStream;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};

/// The implementation of `module!`. Called by the thin `lib.rs` entrypoint:
/// parse, validate, expand. Returns a [`MacroError`] (converted to
/// `compile_error!` by the entrypoint) on failure -- never panics.
pub fn module(input: TokenStream) -> MacroResult {
    let declaration: ModuleDeclaration =
        syn::parse2(input).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;
    validate::validate(&declaration)?;
    Ok(expand::expand(&declaration))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn expands_a_full_module_declaration() {
        let s = module(quote! {
            pub Accounts {
                imports: [Notifications],
                exports: [AuthenticationService],
                controllers: [SessionsController],
                services: [AuthenticationService],
                policies: [UserPolicy],
                routes: ACCOUNTS_ROUTES,
                listeners: [UserRegistered => send_welcome],
                jobs: [send_email v2 => handle_send_email],
                commands: ["users:prune" => prune_users],
                schedules: [cleanup_sessions every "5m"],
                pages: [pages::SignInPage],
            }
        })
        .unwrap()
        .to_string();

        assert!(s.contains("ACCOUNTS_MODULE"), "got: {s}");
        assert!(s.contains("fn accounts_module ()"), "got: {s}");
        assert!(s.contains("ACCOUNTS_ROUTES"), "got: {s}");
        assert!(s.contains("\"users:prune\""), "got: {s}");
        assert!(
            s.contains("pages :: SignInPage :: PAGE_CONTRACT_ENTRY"),
            "got: {s}"
        );
    }

    #[test]
    fn reports_a_page_listed_twice_as_arc_m002() {
        let err = module(quote! { A { pages: [P, pages::P] } }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }

    #[test]
    fn reports_a_syntax_error_as_arc_m001() {
        let err = module(quote! { pub Accounts { unknown: [A] } }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM001);
    }

    #[test]
    fn reports_a_duplicate_entry_as_arc_m002() {
        let err = module(quote! { pub Accounts { imports: [A, A] } }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }
}
