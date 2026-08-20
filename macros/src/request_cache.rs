//! `#[request_cache]` -- declares a per-request memoized resolver.
//!
//! Emits the annotated function **unchanged** plus a
//! `pub const <FN>_REQUEST_CACHE: ::arcature::RequestCacheDescriptor`
//! carrying the resolver name and its key field names, so the UAG,
//! `arc check`, and inspection tooling can report "this resolver is
//! memoized per request by `<key_fields>`" without running it.
//!
//! ## Syntax
//!
//! ```ignore
//! #[request_cache(name = "load_profile", key = "user_id")]
//! pub async fn load_profile(
//!     cache: RequestCache,
//!     user_id: u64,
//! ) -> Result<Profile, RequestCacheError> {
//!     cache.get_or_compute("load_profile", &user_id, || async {
//!         // the expensive resolver body, run once per (user_id, request)
//!         Ok(profile)
//!     }).await
//! }
//! ```
//!
//! A composite key uses `keys = ["a", "b"]` in place of `key = "a"`. Bad
//! attribute arguments produce `error[ARC-M013]`; a bad signature produces
//! `error[ARC-M014]`; a non-function item produces `error[ARC-M001]`.
//!
//! ## What this macro does NOT do
//!
//! It injects no memoization logic. The developer writes the
//! `cache.get_or_compute(...)` call in the body, exactly as `#[service]`,
//! `#[listener]`, and `#[job_handler]` leave their behaviour explicit. The
//! macro generates only the descriptor metadata.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};
use crate::signature::validate_public_async_fn;

/// The implementation of `#[request_cache]`. Called by the thin `lib.rs`
/// entrypoint. Returns a [`MacroError`] (converted to `compile_error!` by
/// the entrypoint) on failure -- never panics.
pub fn request_cache(attr: TokenStream, item: TokenStream) -> MacroResult {
    let args: RequestCacheArgs =
        syn::parse2(attr).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM013, e))?;

    let item_fn: syn::ItemFn =
        syn::parse2(item).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;

    validate_public_async_fn(&item_fn, MacroErrorCode::ArcM014, "#[request_cache]")?;

    let name = args.name.ok_or_else(|| {
        MacroError::new(
            MacroErrorCode::ArcM013,
            item_fn.sig.ident.span(),
            "#[request_cache] requires `name = \"...\"` (the resolver name)",
        )
    })?;
    let key_fields = args.key_fields.ok_or_else(|| {
        MacroError::new(
            MacroErrorCode::ArcM013,
            item_fn.sig.ident.span(),
            "#[request_cache] requires `key = \"...\"` (single key field) or \
             `keys = [\"a\", \"b\"]` (composite key)",
        )
    })?;

    let const_ident = syn::Ident::new(
        &format!(
            "{}_REQUEST_CACHE",
            item_fn.sig.ident.to_string().to_ascii_uppercase()
        ),
        item_fn.sig.ident.span(),
    );

    Ok(quote! {
        #item_fn

        pub const #const_ident: ::arcature::RequestCacheDescriptor =
            ::arcature::RequestCacheDescriptor {
                name: #name,
                key_fields: &[#(#key_fields),*],
            };
    })
}

/// The parsed `#[request_cache(...)]` attribute arguments.
struct RequestCacheArgs {
    /// The resolver name, from `name = "..."`.
    name: Option<String>,
    /// The key field names: one from `key = "..."`, several from
    /// `keys = ["a", "b"]`.
    key_fields: Option<Vec<String>>,
}

impl Parse for RequestCacheArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name: Option<String> = None;
        let mut single_key: Option<String> = None;
        let mut multi_keys: Option<Vec<String>> = None;

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            let _: syn::Token![=] = input.parse()?;

            match ident.to_string().as_str() {
                "name" => name = Some(input.parse::<syn::LitStr>()?.value()),
                "key" => single_key = Some(input.parse::<syn::LitStr>()?.value()),
                "keys" => multi_keys = Some(parse_key_array(input)?),
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "unknown `#[request_cache]` argument `{other}`; \
                             expected `name`, `key`, or `keys`"
                        ),
                    ));
                }
            }

            let _ = input.parse::<syn::Token![,]>();
        }

        if single_key.is_some() && multi_keys.is_some() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "#[request_cache] accepts `key` (single) or `keys` (composite), not both",
            ));
        }

        Ok(RequestCacheArgs {
            name,
            key_fields: single_key.map(|key| vec![key]).or(multi_keys),
        })
    }
}

/// Parses `["a", "b"]` into the key field names it lists.
fn parse_key_array(input: ParseStream) -> syn::Result<Vec<String>> {
    input
        .parse::<syn::ExprArray>()?
        .elems
        .into_iter()
        .map(|expr| match expr {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) => Ok(s.value()),
            other => Err(syn::Error::new(
                other.span(),
                "`keys` must be an array of string literals (e.g. `[\"a\", \"b\"]`)",
            )),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver() -> TokenStream {
        quote! {
            pub async fn load_profile(cache: RequestCache, user_id: u64) -> Result<Profile> {
                todo!()
            }
        }
    }

    #[test]
    fn emits_the_function_unchanged_and_a_descriptor_const() {
        let s = request_cache(
            quote! { name = "load_profile", key = "user_id" },
            resolver(),
        )
        .unwrap()
        .to_string();
        assert!(s.contains("load_profile"), "got: {s}");
        assert!(s.contains("LOAD_PROFILE_REQUEST_CACHE"), "got: {s}");
        assert!(s.contains("RequestCacheDescriptor"), "got: {s}");
        assert!(s.contains("\"user_id\""), "got: {s}");
    }

    #[test]
    fn injects_no_memoization_logic() {
        let s = request_cache(
            quote! { name = "load_profile", key = "user_id" },
            resolver(),
        )
        .unwrap()
        .to_string();
        assert!(!s.contains("get_or_compute"), "got: {s}");
    }

    #[test]
    fn accepts_a_composite_key() {
        let s = request_cache(
            quote! { name = "load", keys = ["tenant_id", "user_id"] },
            resolver(),
        )
        .unwrap()
        .to_string();
        assert!(s.contains("\"tenant_id\""), "got: {s}");
        assert!(s.contains("\"user_id\""), "got: {s}");
    }

    #[test]
    fn rejects_key_and_keys_together() {
        let err = request_cache(
            quote! { name = "load", key = "a", keys = ["a", "b"] },
            resolver(),
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM013);
    }

    #[test]
    fn rejects_a_missing_name() {
        let err = request_cache(quote! { key = "user_id" }, resolver()).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM013);
    }

    #[test]
    fn rejects_a_missing_key() {
        let err = request_cache(quote! { name = "load_profile" }, resolver()).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM013);
    }

    #[test]
    fn rejects_an_unknown_argument() {
        let err = request_cache(quote! { name = "a", key = "b", ttl = 5 }, resolver()).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM013);
    }

    #[test]
    fn rejects_a_non_string_key_element() {
        let err = request_cache(quote! { name = "a", keys = [42] }, resolver()).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM013);
    }

    #[test]
    fn rejects_a_bad_signature() {
        let err = request_cache(
            quote! { name = "load", key = "id" },
            quote! { pub fn load(id: u64) -> Profile { todo!() } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM014);
    }

    #[test]
    fn rejects_a_non_fn_item() {
        let err = request_cache(
            quote! { name = "load", key = "id" },
            quote! { pub struct Loader; },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM001);
    }
}
