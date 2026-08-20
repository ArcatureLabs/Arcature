//! `#[command("name")]` -- annotates a function as a typed application
//! command.
//!
//! Validates the function signature (`pub async fn` with a return type) and
//! generates a `pub const <FN>_COMMAND: ::arcature::CommandBinding` for
//! `arc check` / `arc modules` inspection. The function is emitted
//! unchanged.
//!
//! ## Syntax
//!
//! ```ignore
//! #[command("users:prune")]
//! pub async fn prune_users(users: UserService) -> Result<()> {
//!     users.prune_inactive().await
//! }
//! ```
//!
//! The attribute argument is the command name (a string literal). A missing,
//! empty, or non-string name produces `error[ARC-M009]`; a bad signature
//! produces `error[ARC-M011]`; a non-function item produces `error[ARC-M001]`.
//!
//! ## What this macro does NOT do
//!
//! It does not register the command at runtime -- the application registers
//! commands explicitly with `CommandRegistry::register` at startup, and
//! `module!`'s `commands:` section is inspection metadata only. It also does
//! not generate `impl Command`: `#[command]` annotates a function, and
//! function item types cannot be named in a trait impl.

use proc_macro2::TokenStream;
use quote::quote;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};
use crate::signature::validate_public_async_fn;

/// The implementation of `#[command("name")]`. Called by the thin `lib.rs`
/// entrypoint. Returns a [`MacroError`] (converted to `compile_error!` by
/// the entrypoint) on failure -- never panics.
pub fn command(attr: TokenStream, item: TokenStream) -> MacroResult {
    let name = parse_name(attr)?;
    let item_fn: syn::ItemFn =
        syn::parse2(item).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;

    validate_public_async_fn(&item_fn, MacroErrorCode::ArcM011, "#[command(...)]")?;

    let fn_name = item_fn.sig.ident.to_string();
    let const_ident = syn::Ident::new(
        &format!("{}_COMMAND", fn_name.to_uppercase()),
        item_fn.sig.ident.span(),
    );

    Ok(quote! {
        #item_fn

        #[allow(non_upper_case_globals)]
        pub const #const_ident: ::arcature::CommandBinding =
            ::arcature::CommandBinding { name: #name, function: #fn_name };
    })
}

/// Parses the attribute argument: a single non-empty string literal.
fn parse_name(attr: TokenStream) -> Result<String, MacroError> {
    let lit: syn::LitStr =
        syn::parse2(attr).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM009, e))?;
    let name = lit.value();
    if name.is_empty() {
        return Err(MacroError::new(
            MacroErrorCode::ArcM009,
            lit.span(),
            "#[command(\"...\")] name must not be empty",
        ));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_command_binding_const() {
        let expanded = command(
            quote! { "users:prune" },
            quote! { pub async fn prune_users() -> Result<()> { Ok(()) } },
        )
        .unwrap();
        let s = expanded.to_string();
        assert!(s.contains("\"users:prune\""), "got: {s}");
        assert!(s.contains("PRUNE_USERS_COMMAND"), "got: {s}");
        assert!(s.contains("CommandBinding"), "got: {s}");
        assert!(!s.contains("impl :: arcature :: Command"), "got: {s}");
    }

    #[test]
    fn rejects_non_fn_item() {
        let err = command(quote! { "test" }, quote! { pub struct Foo {} }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM001);
    }

    #[test]
    fn rejects_empty_name() {
        let err = command(
            quote! { "" },
            quote! { pub async fn handle() -> Result<()> { Ok(()) } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM009);
    }

    #[test]
    fn rejects_missing_name() {
        let err = command(
            quote! {},
            quote! { pub async fn handle() -> Result<()> { Ok(()) } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM009);
    }

    #[test]
    fn rejects_non_string_name() {
        let err = command(
            quote! { 42 },
            quote! { pub async fn handle() -> Result<()> { Ok(()) } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM009);
    }

    #[test]
    fn rejects_bad_signature() {
        let err = command(quote! { "test" }, quote! { pub fn handle() -> Result<()> { Ok(()) } })
            .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM011);
    }
}
