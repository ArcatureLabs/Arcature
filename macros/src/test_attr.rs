//! `#[arcature::test(app = ...)]` -- an async test that receives a booted
//! application.
//!
//! ## Syntax
//!
//! ```ignore
//! // Not compiled: `arcature-macros` cannot depend on `arcature`
//! // (that is the cycle `lib.rs` describes), so an example naming
//! // Arcature items has nothing here to compile against.
//! #[arcature::test(app = my_app::router())]
//! async fn the_index_page_lists_users(app: TestApp) {
//!     app.get("/users").send().await.assert_ok();
//! }
//! ```
//!
//! The `app` value is any expression producing something that implements
//! `IntoTestApp` -- a `TestApp`, an `axum::Router`, or an
//! `arcature::Application<()>`. It is an expression rather than a string so
//! the compiler checks it: a typo in a builder name is a compile error at the
//! attribute, not a runtime panic inside the test.
//!
//! ## Why the attribute takes the app at all
//!
//! Nothing in Arcature registers an ambient application, so there is nowhere
//! for the macro to find one implicitly. Naming it here is the whole
//! difference between a test kit you can reason about and a global the suite
//! silently shares.
//!
//! ## What it expands to
//!
//! An ordinary `#[test]` function that builds a Tokio runtime, builds the
//! app, binds it to the single parameter, and runs the body. Attributes
//! written above it -- `#[ignore]`, `#[should_panic]` -- are kept, so the
//! usual test tooling still applies.
//!
//! A missing or malformed `app` key, and a signature that is not an `async
//! fn` of exactly one parameter, both produce `error[ARC-M012]`; a
//! non-function item produces `error[ARC-M001]`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Expr, Meta, Token};

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};

/// The implementation of `#[arcature::test(app = ...)]`. Called by the thin
/// `lib.rs` entrypoint; never panics.
pub fn test_attr(attr: TokenStream, item: TokenStream) -> MacroResult {
    let span = if attr.is_empty() {
        item.span()
    } else {
        attr.span()
    };
    let app = parse_app(attr, span)?;
    let item_fn: syn::ItemFn =
        syn::parse2(item).map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM001, e))?;
    let (pattern, ty) = single_parameter(&item_fn)?;

    if item_fn.sig.asyncness.is_none() {
        return Err(MacroError::new(
            MacroErrorCode::ArcM012,
            item_fn.sig.fn_token.span(),
            "#[arcature::test] expects an `async fn`; the body is run on a Tokio runtime the macro builds",
        ));
    }

    let attrs = &item_fn.attrs;
    let vis = &item_fn.vis;
    let ident = &item_fn.sig.ident;
    let output = &item_fn.sig.output;
    let body = &item_fn.block;

    Ok(quote! {
        #(#attrs)*
        #[::core::prelude::v1::test]
        #vis fn #ident() #output {
            ::arcature::test_kit::block_on(async move {
                let #pattern: #ty = ::arcature::test_kit::IntoTestApp::into_test_app(#app);
                #body
            })
        }
    })
}

/// Parse `app = <expr>` out of the attribute arguments.
fn parse_app(attr: TokenStream, span: proc_macro2::Span) -> Result<Expr, MacroError> {
    if attr.is_empty() {
        return Err(MacroError::new(
            MacroErrorCode::ArcM012,
            span,
            "#[arcature::test] needs `app = <expr>`; there is no ambient application to fall back on",
        ));
    }
    let parser = Punctuated::<Meta, Token![,]>::parse_terminated;
    let metas = parser
        .parse2(attr)
        .map_err(|e| MacroError::from_syn(MacroErrorCode::ArcM012, e))?;

    let mut app: Option<Expr> = None;
    for meta in metas {
        let Meta::NameValue(name_value) = &meta else {
            return Err(MacroError::new(
                MacroErrorCode::ArcM012,
                meta.span(),
                "#[arcature::test] takes only `app = <expr>`",
            ));
        };
        if !name_value.path.is_ident("app") {
            let key = name_value
                .path
                .get_ident()
                .map_or_else(|| "<path>".to_owned(), ToString::to_string);
            return Err(MacroError::new(
                MacroErrorCode::ArcM012,
                name_value.path.span(),
                format!("`{key}` is not a #[arcature::test] argument; the only key is `app`"),
            ));
        }
        if app.is_some() {
            return Err(MacroError::new(
                MacroErrorCode::ArcM012,
                name_value.span(),
                "`app` is given twice",
            ));
        }
        if let Expr::Lit(literal) = &name_value.value
            && matches!(literal.lit, syn::Lit::Str(_))
        {
            return Err(MacroError::new(
                MacroErrorCode::ArcM012,
                literal.span(),
                "`app` takes an expression that builds the application, not a string; a string would only be checked at runtime",
            ));
        }
        app = Some(name_value.value.clone());
    }

    app.ok_or_else(|| {
        MacroError::new(
            MacroErrorCode::ArcM012,
            span,
            "#[arcature::test] needs `app = <expr>`",
        )
    })
}

/// The single parameter the app is bound to.
///
/// One parameter, not zero and not several: the macro builds exactly one
/// value, and a test that wants more can build them in its body where the
/// reader can see it happen.
fn single_parameter(item_fn: &syn::ItemFn) -> Result<(&syn::Pat, &syn::Type), MacroError> {
    let inputs = &item_fn.sig.inputs;
    if inputs.len() != 1 {
        return Err(MacroError::new(
            MacroErrorCode::ArcM012,
            item_fn.sig.span(),
            format!(
                "#[arcature::test] expects exactly one parameter to receive the application, found {}",
                inputs.len()
            ),
        ));
    }
    match &inputs[0] {
        syn::FnArg::Typed(typed) => Ok((&typed.pat, &typed.ty)),
        syn::FnArg::Receiver(receiver) => Err(MacroError::new(
            MacroErrorCode::ArcM012,
            receiver.span(),
            "#[arcature::test] applies to a free function, not a method",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn expand(attr: TokenStream, item: TokenStream) -> MacroResult {
        test_attr(attr, item)
    }

    fn body() -> TokenStream {
        quote! {
            async fn the_index_lists_users(app: TestApp) {
                app.get("/users").send().await.assert_ok();
            }
        }
    }

    #[test]
    fn a_well_formed_test_expands_to_a_synchronous_test_function() {
        let tokens = expand(quote! { app = build_app() }, body())
            .expect("a well-formed test must expand")
            .to_string();
        assert!(tokens.contains("test"), "{tokens}");
        assert!(tokens.contains("block_on"), "{tokens}");
        assert!(tokens.contains("into_test_app"), "{tokens}");
        assert!(
            !tokens.contains("async fn the_index_lists_users"),
            "the generated function must not be async: {tokens}"
        );
    }

    #[test]
    fn the_app_expression_is_kept_verbatim() {
        let tokens = expand(quote! { app = my_app::router(state.clone()) }, body())
            .expect("a call expression is a valid app")
            .to_string();
        assert!(tokens.contains("my_app :: router"), "{tokens}");
    }

    #[test]
    fn other_attributes_survive_the_expansion() {
        let item = quote! {
            #[ignore = "needs postgres"]
            async fn it_writes_a_row(app: TestApp) {}
        };
        let tokens = expand(quote! { app = build_app() }, item)
            .expect("expansion must succeed")
            .to_string();
        assert!(tokens.contains("ignore"), "{tokens}");
    }

    #[test]
    fn a_missing_app_key_is_reported_as_arc_m012() {
        let error = expand(TokenStream::new(), body()).expect_err("a missing app is fatal");
        assert_eq!(error.code(), MacroErrorCode::ArcM012);
    }

    #[test]
    fn an_unknown_key_is_reported_as_arc_m012() {
        let error = expand(quote! { application = build_app() }, body())
            .expect_err("an unknown key is fatal");
        assert_eq!(error.code(), MacroErrorCode::ArcM012);
    }

    #[test]
    fn a_string_app_is_refused_because_it_would_only_fail_at_runtime() {
        let error =
            expand(quote! { app = "build_app" }, body()).expect_err("a string app is fatal");
        assert_eq!(error.code(), MacroErrorCode::ArcM012);
    }

    #[test]
    fn a_synchronous_test_function_is_refused() {
        let item = quote! { fn it_runs(app: TestApp) {} };
        let error = expand(quote! { app = build_app() }, item).expect_err("a sync fn is fatal");
        assert_eq!(error.code(), MacroErrorCode::ArcM012);
    }

    #[test]
    fn a_test_with_no_parameter_is_refused() {
        let item = quote! { async fn it_runs() {} };
        let error =
            expand(quote! { app = build_app() }, item).expect_err("a missing parameter is fatal");
        assert_eq!(error.code(), MacroErrorCode::ArcM012);
    }

    #[test]
    fn a_non_function_item_is_reported_as_arc_m001() {
        let item = quote! { struct NotAFunction; };
        let error =
            expand(quote! { app = build_app() }, item).expect_err("a non-function is fatal");
        assert_eq!(error.code(), MacroErrorCode::ArcM001);
    }

    #[test]
    fn a_return_type_is_carried_to_the_generated_function() {
        let item = quote! {
            async fn it_runs(app: TestApp) -> Result<(), Box<dyn std::error::Error>> { Ok(()) }
        };
        let tokens = expand(quote! { app = build_app() }, item)
            .expect("a fallible test must expand")
            .to_string();
        assert!(tokens.contains("Result < ()"), "{tokens}");
    }
}
