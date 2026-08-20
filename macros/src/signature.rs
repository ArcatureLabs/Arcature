//! Shared function-signature validation for the Arcature attribute macros.
//!
//! Several attribute macros (`#[middleware]`, `#[listener]`,
//! `#[job_handler]`, `#[command]`, `#[request_cache]`) attach to a function
//! and enforce the same shape: `pub async fn` with an explicit return type.
//! Each macro reports the failure under its own error code, so the check
//! takes the code as a parameter.
//!
//! The macros deliberately do NOT rewrite the function -- they validate the
//! contract and emit the item unchanged, so the function stays a genuine
//! Rust function that the reader can follow. Only the metadata const (where
//! a macro emits one) is generated.

use crate::diagnostic::{MacroError, MacroErrorCode};

/// Validates that `item_fn` is `pub`, `async`, and declares a return type.
///
/// `code` is the reporting macro's error code (e.g. `ArcM007` for
/// `#[middleware]`); `macro_name` is the macro spelled as the user writes
/// it (e.g. `"#[middleware]"`), used in the diagnostic message.
pub fn validate_public_async_fn(
    item_fn: &syn::ItemFn,
    code: MacroErrorCode,
    macro_name: &str,
) -> Result<(), MacroError> {
    let sig = &item_fn.sig;

    if !matches!(item_fn.vis, syn::Visibility::Public(_)) {
        return Err(MacroError::new(
            code,
            sig.ident.span(),
            format!("{macro_name} functions must be `pub`"),
        ));
    }

    if sig.asyncness.is_none() {
        return Err(MacroError::new(
            code,
            sig.ident.span(),
            format!("{macro_name} functions must be `async fn`"),
        ));
    }

    if matches!(sig.output, syn::ReturnType::Default) {
        return Err(MacroError::new(
            code,
            sig.ident.span(),
            format!("{macro_name} functions must declare a return type"),
        ));
    }

    Ok(())
}

/// Returns the parameter names of a function signature, in declaration
/// order. `self` receivers are skipped; non-identifier patterns (e.g.
/// destructuring) render as `"_"`.
///
/// Used by the metadata-emitting macros (`#[controller]`,
/// `#[request_cache]`) to record handler parameter names for `arc check` /
/// UAG inspection without parsing function bodies.
pub fn parameter_names(sig: &syn::Signature) -> Vec<String> {
    sig.inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Receiver(_) => None,
            syn::FnArg::Typed(pat_type) => Some(match &*pat_type.pat {
                syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                _ => "_".to_string(),
            }),
        })
        .collect()
}

/// Converts a `PascalCase` / `camelCase` identifier to `snake_case`.
///
/// Used to derive default kind / name strings from type names (e.g.
/// `SendVerificationEmail` -> `send_verification_email`).
pub fn to_snake_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 4);
    for (i, ch) in input.char_indices() {
        if ch.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn parse_fn(tokens: proc_macro2::TokenStream) -> syn::ItemFn {
        syn::parse2(tokens).expect("valid fn")
    }

    #[test]
    fn accepts_pub_async_fn_with_return_type() {
        let f = parse_fn(quote! { pub async fn handler() -> Response { todo!() } });
        assert!(validate_public_async_fn(&f, MacroErrorCode::ArcM007, "#[middleware]").is_ok());
    }

    #[test]
    fn rejects_private_fn() {
        let f = parse_fn(quote! { async fn handler() -> Response { todo!() } });
        let err =
            validate_public_async_fn(&f, MacroErrorCode::ArcM007, "#[middleware]").unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM007);
        assert!(err.to_compile_error().to_string().contains("pub"));
    }

    #[test]
    fn rejects_sync_fn() {
        let f = parse_fn(quote! { pub fn handler() -> Response { todo!() } });
        let err =
            validate_public_async_fn(&f, MacroErrorCode::ArcM008, "#[listener]").unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM008);
        assert!(err.to_compile_error().to_string().contains("async"));
    }

    #[test]
    fn rejects_missing_return_type() {
        let f = parse_fn(quote! { pub async fn handler() { } });
        let err =
            validate_public_async_fn(&f, MacroErrorCode::ArcM010, "#[job_handler]").unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM010);
        assert!(err.to_compile_error().to_string().contains("return type"));
    }

    #[test]
    fn parameter_names_lists_idents_in_order() {
        let f = parse_fn(quote! { pub async fn show(auth: Auth, link: Bound<Link>) -> R { todo!() } });
        assert_eq!(parameter_names(&f.sig), vec!["auth", "link"]);
    }

    #[test]
    fn parameter_names_renders_patterns_as_underscore() {
        let f = parse_fn(quote! { pub async fn show((a, b): (u8, u8)) -> R { todo!() } });
        assert_eq!(parameter_names(&f.sig), vec!["_"]);
    }

    #[test]
    fn snake_case_conversion() {
        assert_eq!(to_snake_case("SendVerificationEmail"), "send_verification_email");
        assert_eq!(to_snake_case("User"), "user");
        assert_eq!(to_snake_case("already_snake"), "already_snake");
    }
}
