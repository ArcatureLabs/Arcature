//! Assembly of the three items a `routes!` block generates.
//!
//! One responsibility: flatten the entry tree once, pick the router state
//! type, and concatenate the router function, the metadata const, and the
//! URL helper module.

use proc_macro2::TokenStream;
use quote::quote;

use super::declaration::RoutesDeclaration;
use super::{flatten, helper_module, metadata_const, router_fn};

/// Generates every item for a validated declaration.
pub fn expand(decl: &RoutesDeclaration) -> TokenStream {
    let state_ty = state_type(decl);
    let flat = flatten::entries(&decl.entries, "");

    let router = router_fn::expand(decl, &state_ty);
    let metadata = metadata_const::expand(decl, &flat);
    let helpers = helper_module::expand(decl, &flat);

    quote! {
        #router
        #metadata
        #helpers
    }
}

/// The router's state type: the `state: T;` clause, or `()` when omitted.
///
/// One type is used for the whole declaration so that groups and the outer
/// router compose without a state mismatch.
fn state_type(decl: &RoutesDeclaration) -> TokenStream {
    match &decl.state {
        Some(ty) => quote! { #ty },
        None => quote! { () },
    }
}

#[cfg(test)]
mod tests {
    use super::expand;
    use crate::routes::declaration::RoutesDeclaration;

    #[test]
    fn all_three_items_are_emitted() {
        let decl = syn::parse2::<RoutesDeclaration>(quote::quote! {
            pub app {
                state: AppState;
                get "/" => home { name: home }
            }
        })
        .unwrap();
        let out = expand(&decl).to_string();
        assert!(out.contains("fn app_routes"));
        assert!(out.contains("const APP_ROUTES"));
        assert!(out.contains("mod app_route"));
    }

    #[test]
    fn a_stateless_declaration_uses_the_unit_state() {
        let decl =
            syn::parse2::<RoutesDeclaration>(quote::quote! { app { get "/" => home } }).unwrap();
        let out = expand(&decl).to_string();
        assert!(out.contains(":: arcature :: Routes < () >"));
    }
}
