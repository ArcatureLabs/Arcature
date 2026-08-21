//! `#[request_cache]` -- makes a resolver memoized for the life of one
//! request.
//!
//! Emits three things: the resolver's original body, moved verbatim into a
//! nested uncached function; a wrapper under the original name that consults
//! the request's [`RequestCache`] before calling it; and a
//! `pub const <FN>_REQUEST_CACHE: ::arcature::RequestCacheDescriptor` so the
//! UAG and inspection tooling can report "this resolver is memoized per
//! request by `<key_fields>`" without running it.
//!
//! ## Syntax
//!
//! ```ignore
//! // Not compiled: `arcature-macros` cannot depend on `arcature`
//! // (that is the cycle `lib.rs` describes), so an example naming
//! // Arcature items has nothing here to compile against.
//! #[request_cache(name = "load_profile", key = "user_id")]
//! pub async fn load_profile(
//!     cache: RequestCache,
//!     user_id: u64,
//! ) -> Result<Profile, ProfileError> {
//!     // runs once per (user_id, request)
//!     Profile::load(user_id).await
//! }
//! ```
//!
//! A composite key uses `keys = ["a", "b"]` in place of `key = "a"`. Bad
//! attribute arguments produce `error[ARC-M013]`; a bad signature produces
//! `error[ARC-M014]`; a non-function item produces `error[ARC-M001]`.
//!
//! ## What the wrapper does
//!
//! ```ignore
//! // Not compiled: `arcature-macros` cannot depend on `arcature`
//! // (that is the cycle `lib.rs` describes), so an example naming
//! // Arcature items has nothing here to compile against.
//! let key = RequestCacheKey::new("load_profile").field("user_id", &user_id);
//! if let Some(hit) = cache.get(&key) { return Ok(hit); }
//! let outcome = uncached(cache.clone(), user_id).await;
//! if let Ok(ref value) = outcome { cache.insert(&key, value.clone()); }
//! outcome
//! ```
//!
//! Only `Ok` values are memoized. A failed resolve is not a fact about the
//! request -- retrying it within the same request is the developer's call,
//! and caching an error would make that impossible.
//!
//! ## What the signature has to say
//!
//! The store is reachable only through the request, so the resolver must
//! name it: exactly one parameter of type `RequestCache`, which the wrapper
//! reads the memo out of and clones on into the body. There is no ambient
//! store to fall back on, and that is deliberate -- a memo keyed by nothing
//! but a resolver name, living in a `thread_local!`, is how one request ends
//! up serving another request's data.
//!
//! The return type must be a `Result`, every key field must be a parameter,
//! and the `Ok` type must be `Clone` (the wrapper hands out a copy per hit
//! and keeps one). Each is checked here so the failure lands on the
//! developer's own `#[request_cache]` line rather than inside generated
//! code.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};
use crate::signature::validate_public_async_fn;

/// The type name a resolver's cache parameter must have.
const CACHE_TYPE: &str = "RequestCache";

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

    let parameters = parameters(&item_fn)?;
    let cache = cache_parameter(&item_fn, &parameters)?;
    let key_idents = key_parameters(&item_fn, &parameters, &key_fields)?;
    require_result_return(&item_fn)?;

    let const_ident = syn::Ident::new(
        &format!(
            "{}_REQUEST_CACHE",
            item_fn.sig.ident.to_string().to_ascii_uppercase()
        ),
        item_fn.sig.ident.span(),
    );

    let wrapper = expand_wrapper(
        &item_fn,
        &parameters,
        cache,
        &key_fields,
        &key_idents,
        &name,
    );

    Ok(quote! {
        #wrapper

        pub const #const_ident: ::arcature::RequestCacheDescriptor =
            ::arcature::RequestCacheDescriptor {
                name: #name,
                key_fields: &[#(#key_fields),*],
            };
    })
}

/// Generates the memoizing wrapper plus the nested uncached function that
/// holds the developer's original body.
fn expand_wrapper(
    item_fn: &syn::ItemFn,
    parameters: &[syn::Ident],
    cache: usize,
    key_fields: &[String],
    key_idents: &[syn::Ident],
    name: &str,
) -> TokenStream {
    let cache_ident = &parameters[cache];

    let mut uncached = item_fn.clone();
    uncached.attrs.clear();
    uncached.vis = syn::Visibility::Inherited;
    uncached.sig.ident = format_ident!("__arc_uncached_{}", item_fn.sig.ident);
    // The body receives the store so nested memoized resolvers can be
    // called from it, but plenty of bodies have no nested call and would
    // otherwise warn about an unused parameter they never wrote. Borrowing
    // it counts as a use without blanket-allowing the lint over the
    // developer's own code.
    uncached
        .block
        .stmts
        .insert(0, syn::parse_quote! { let _ = &#cache_ident; });
    let uncached_ident = &uncached.sig.ident;

    // The body consumes its parameters, so the cache handle is cloned on --
    // the wrapper still needs it to record the result afterwards. Every
    // other parameter moves, exactly as the developer wrote it.
    let forwarded = parameters.iter().enumerate().map(|(i, ident)| {
        if i == cache {
            quote! { ::std::clone::Clone::clone(&#ident) }
        } else {
            quote! { #ident }
        }
    });

    let attrs = &item_fn.attrs;
    let vis = &item_fn.vis;
    let signature = &item_fn.sig;

    quote! {
        #(#attrs)*
        #vis #signature {
            #uncached

            let __arc_key = ::arcature::dx::RequestCacheKey::new(#name)
                #( .field(#key_fields, &#key_idents) )*;

            if let ::std::option::Option::Some(__arc_hit) =
                ::arcature::dx::RequestCache::get(&#cache_ident, &__arc_key)
            {
                return ::std::result::Result::Ok(__arc_hit);
            }

            let __arc_outcome = #uncached_ident(#(#forwarded),*).await;

            // Only a success is a fact about this request; an error is left
            // uncached so the caller may retry it.
            if let ::std::result::Result::Ok(ref __arc_value) = __arc_outcome {
                ::arcature::dx::RequestCache::insert(
                    &#cache_ident,
                    &__arc_key,
                    ::std::clone::Clone::clone(__arc_value),
                );
            }

            __arc_outcome
        }
    }
}

/// The resolver's parameter names, in declaration order.
///
/// The wrapper forwards each one by name, so a pattern parameter (`(a, b):
/// (u8, u8)`) has nothing to forward and is rejected here rather than
/// producing a confusing error inside generated code.
fn parameters(item_fn: &syn::ItemFn) -> Result<Vec<syn::Ident>, MacroError> {
    item_fn
        .sig
        .inputs
        .iter()
        .map(|input| match input {
            syn::FnArg::Typed(pat_type) => match &*pat_type.pat {
                syn::Pat::Ident(pat_ident) => Ok(pat_ident.ident.clone()),
                other => Err(MacroError::new(
                    MacroErrorCode::ArcM014,
                    other.span(),
                    "#[request_cache] parameters must be plain names -- the \
                     generated wrapper forwards each parameter by name.",
                )),
            },
            syn::FnArg::Receiver(receiver) => Err(MacroError::new(
                MacroErrorCode::ArcM014,
                receiver.span(),
                "#[request_cache] applies to a free function, not a method.",
            )),
        })
        .collect()
}

/// Finds the single `RequestCache` parameter, returning its position.
fn cache_parameter(item_fn: &syn::ItemFn, parameters: &[syn::Ident]) -> Result<usize, MacroError> {
    let positions: Vec<usize> = item_fn
        .sig
        .inputs
        .iter()
        .enumerate()
        .filter(|(_, input)| match input {
            syn::FnArg::Typed(pat_type) => is_cache_type(&pat_type.ty),
            syn::FnArg::Receiver(_) => false,
        })
        .map(|(i, _)| i)
        .collect();

    match positions.as_slice() {
        [only] => Ok(*only),
        [] => Err(MacroError::new(
            MacroErrorCode::ArcM014,
            item_fn.sig.ident.span(),
            format!(
                "#[request_cache] requires a `{CACHE_TYPE}` parameter -- the memo \
                 store is reachable only through the request, so the resolver \
                 must take one (e.g. `cache: {CACHE_TYPE}`)."
            ),
        )),
        [_, second, ..] => Err(MacroError::new(
            MacroErrorCode::ArcM014,
            parameters[*second].span(),
            format!("#[request_cache] accepts exactly one `{CACHE_TYPE}` parameter."),
        )),
    }
}

/// Whether a parameter's type is the request memo store. Matched on the last
/// path segment so `RequestCache`, `arcature::RequestCache`, and
/// `arcature::dx::RequestCache` all count.
fn is_cache_type(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == CACHE_TYPE),
        _ => false,
    }
}

/// Resolves each declared key field to the parameter it names.
fn key_parameters(
    item_fn: &syn::ItemFn,
    parameters: &[syn::Ident],
    key_fields: &[String],
) -> Result<Vec<syn::Ident>, MacroError> {
    key_fields
        .iter()
        .map(|field| {
            parameters
                .iter()
                .find(|ident| *ident == field)
                .cloned()
                .ok_or_else(|| {
                    MacroError::new(
                        MacroErrorCode::ArcM013,
                        item_fn.sig.ident.span(),
                        format!(
                            "#[request_cache] key field `{field}` is not a parameter of \
                             `{}` -- the cache key is built from the resolver's own \
                             arguments.",
                            item_fn.sig.ident
                        ),
                    )
                })
        })
        .collect()
}

/// Rejects a resolver that does not return a `Result`.
///
/// The wrapper caches the `Ok` value and passes an `Err` straight through,
/// so a non-`Result` return has no shape for it to work with.
fn require_result_return(item_fn: &syn::ItemFn) -> Result<(), MacroError> {
    let syn::ReturnType::Type(_, ty) = &item_fn.sig.output else {
        return Ok(());
    };

    let is_result = match &**ty {
        syn::Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Result"),
        _ => false,
    };

    if is_result {
        Ok(())
    } else {
        Err(MacroError::new(
            MacroErrorCode::ArcM014,
            ty.span(),
            "#[request_cache] resolvers must return a `Result` -- the wrapper \
             memoizes the `Ok` value and passes an `Err` through uncached.",
        ))
    }
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
                Profile::load(user_id).await
            }
        }
    }

    fn expand(attr: TokenStream) -> String {
        request_cache(attr, resolver()).unwrap().to_string()
    }

    #[test]
    fn emits_a_descriptor_const() {
        let s = expand(quote! { name = "load_profile", key = "user_id" });
        assert!(s.contains("LOAD_PROFILE_REQUEST_CACHE"), "got: {s}");
        assert!(s.contains("RequestCacheDescriptor"), "got: {s}");
        assert!(s.contains("\"user_id\""), "got: {s}");
    }

    #[test]
    fn keeps_the_original_body_in_a_nested_uncached_function() {
        let s = expand(quote! { name = "load_profile", key = "user_id" });
        assert!(s.contains("fn __arc_uncached_load_profile"), "got: {s}");
        assert!(s.contains("Profile :: load (user_id) . await"), "got: {s}");
    }

    #[test]
    fn consults_the_cache_before_computing() {
        let s = expand(quote! { name = "load_profile", key = "user_id" });
        assert!(
            s.contains(":: arcature :: dx :: RequestCacheKey :: new (\"load_profile\")"),
            "got: {s}"
        );
        assert!(s.contains(". field (\"user_id\" , & user_id)"), "got: {s}");
        assert!(
            s.contains(":: arcature :: dx :: RequestCache :: get (& cache , & __arc_key)"),
            "got: {s}"
        );
    }

    #[test]
    fn records_only_a_successful_result() {
        let s = expand(quote! { name = "load_profile", key = "user_id" });
        assert!(
            s.contains("if let :: std :: result :: Result :: Ok (ref __arc_value) = __arc_outcome"),
            "got: {s}"
        );
        assert!(s.contains("RequestCache :: insert"), "got: {s}");
    }

    #[test]
    fn clones_the_cache_handle_into_the_uncached_call() {
        let s = expand(quote! { name = "load_profile", key = "user_id" });
        assert!(
            s.contains("__arc_uncached_load_profile (:: std :: clone :: Clone :: clone (& cache) , user_id)"),
            "got: {s}"
        );
    }

    #[test]
    fn a_composite_key_contributes_every_field() {
        let s = request_cache(
            quote! { name = "load", keys = ["tenant_id", "user_id"] },
            quote! {
                pub async fn load(
                    cache: RequestCache,
                    tenant_id: u64,
                    user_id: u64,
                ) -> Result<Profile> {
                    todo!()
                }
            },
        )
        .unwrap()
        .to_string();
        assert!(
            s.contains(". field (\"tenant_id\" , & tenant_id)"),
            "got: {s}"
        );
        assert!(s.contains(". field (\"user_id\" , & user_id)"), "got: {s}");
    }

    #[test]
    fn a_fully_qualified_cache_type_is_still_the_cache_parameter() {
        let s = request_cache(
            quote! { name = "load", key = "id" },
            quote! {
                pub async fn load(memo: ::arcature::dx::RequestCache, id: u64) -> Result<u8> {
                    todo!()
                }
            },
        )
        .unwrap()
        .to_string();
        assert!(
            s.contains("RequestCache :: get (& memo , & __arc_key)"),
            "got: {s}"
        );
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
    fn rejects_a_key_field_that_is_not_a_parameter() {
        let err =
            request_cache(quote! { name = "load", key = "tenant_id" }, resolver()).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM013);
        assert!(
            err.to_compile_error()
                .to_string()
                .contains("not a parameter"),
            "got: {}",
            err.to_compile_error()
        );
    }

    #[test]
    fn rejects_a_resolver_with_no_cache_parameter() {
        let err = request_cache(
            quote! { name = "load", key = "id" },
            quote! { pub async fn load(id: u64) -> Result<Profile> { todo!() } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM014);
        assert!(
            err.to_compile_error()
                .to_string()
                .contains("RequestCache` parameter"),
            "got: {}",
            err.to_compile_error()
        );
    }

    #[test]
    fn rejects_a_resolver_with_two_cache_parameters() {
        let err = request_cache(
            quote! { name = "load", key = "id" },
            quote! {
                pub async fn load(a: RequestCache, b: RequestCache, id: u64) -> Result<u8> {
                    todo!()
                }
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM014);
    }

    #[test]
    fn rejects_a_resolver_that_does_not_return_a_result() {
        let err = request_cache(
            quote! { name = "load", key = "id" },
            quote! { pub async fn load(cache: RequestCache, id: u64) -> Profile { todo!() } },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM014);
        assert!(
            err.to_compile_error().to_string().contains("Result"),
            "got: {}",
            err.to_compile_error()
        );
    }

    #[test]
    fn rejects_a_destructuring_parameter() {
        let err = request_cache(
            quote! { name = "load", key = "id" },
            quote! {
                pub async fn load(cache: RequestCache, (id, _): (u64, u64)) -> Result<u8> {
                    todo!()
                }
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM014);
    }

    #[test]
    fn rejects_a_bad_signature() {
        let err = request_cache(
            quote! { name = "load", key = "id" },
            quote! { pub fn load(cache: RequestCache, id: u64) -> Result<Profile> { todo!() } },
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
