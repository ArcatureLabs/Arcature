//! `#[job_handler]` -- annotates a function as a job handler.
//!
//! Validates the function signature (`pub async fn` with a return type) and
//! emits the function unchanged, so it remains a genuine async function the
//! application registers with `::arcature::jobs::Registry::add` at startup.
//!
//! ## Syntax
//!
//! ```ignore
//! // Not compiled: `arcature-macros` cannot depend on `arcature`
//! // (that is the cycle `lib.rs` describes), so an example naming
//! // Arcature items has nothing here to compile against.
//! #[job_handler]
//! pub async fn handle_send_email(job: SendVerificationEmail) -> Result<()> {
//!     ...
//! }
//! ```
//!
//! A bad signature produces `error[ARC-M010]`; attribute arguments produce
//! `error[ARC-M009]`; a non-function item produces `error[ARC-M001]`.
//!
//! ## What this macro does NOT do
//!
//! It generates no `JobBinding` const, because the handler's proc-macro
//! cannot see the job kind and version -- those come from `#[derive(Job)]`
//! on the job struct. `module!`'s `jobs:` section carries that metadata for
//! inspection. The macro also does not register handlers or enqueue jobs:
//! registration (`Registry::add`) and enqueue (`jobs.enqueue(..)`) stay
//! explicit in application code.

use proc_macro2::TokenStream;
use quote::quote;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};
use crate::signature::validate_public_async_fn;

/// The implementation of `#[job_handler]`. Called by the thin `lib.rs`
/// entrypoint. Returns a [`MacroError`] (converted to `compile_error!` by
/// the entrypoint) on failure -- never panics.
pub fn job_handler(attr: TokenStream, item: TokenStream) -> MacroResult {
    if !attr.is_empty() {
        return Err(MacroError::new(
            MacroErrorCode::ArcM009,
            proc_macro2::Span::call_site(),
            format!("#[job_handler] takes no arguments (got: {attr})"),
        ));
    }

    let item_fn: syn::ItemFn =
        syn::parse2(item).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;

    validate_public_async_fn(&item_fn, MacroErrorCode::ArcM010, "#[job_handler]")?;

    Ok(quote! { #item_fn })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_valid_function_through_unchanged() {
        let expanded = job_handler(
            quote! {},
            quote! { pub async fn handle(job: SendEmail) -> Result<()> { Ok(()) } },
        )
        .unwrap();
        let s = expanded.to_string();
        assert!(s.contains("handle"), "got: {s}");
        assert!(s.contains("async"), "got: {s}");
        assert!(!s.contains("_JOB_HANDLER"), "got: {s}");
    }

    #[test]
    fn rejects_attribute_arguments() {
        let err = job_handler(
            quote! { foo = 1 },
            quote! { pub async fn handle() -> Result<()> { Ok(()) } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM009);
    }

    #[test]
    fn rejects_non_fn_item() {
        let err = job_handler(quote! {}, quote! { pub struct Foo {} }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM001);
    }

    #[test]
    fn rejects_non_async_fn() {
        let err = job_handler(
            quote! {},
            quote! { pub fn handle() -> Result<()> { Ok(()) } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM010);
    }

    #[test]
    fn rejects_non_pub_fn() {
        let err = job_handler(
            quote! {},
            quote! { async fn handle() -> Result<()> { Ok(()) } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM010);
    }

    #[test]
    fn rejects_missing_return_type() {
        let err =
            job_handler(quote! {}, quote! { pub async fn handle(job: SendEmail) {} }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM010);
    }
}
