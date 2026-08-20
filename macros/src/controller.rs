//! `#[controller]` — an Axum controller with route metadata.
//!
//! An attribute macro applied to an `impl` block. Emits the impl unchanged
//! (the methods remain genuine Axum handlers) and additionally emits
//! `impl ControllerMetadata`, whose `METHODS` associated const carries one
//! [`ControllerMethod`] per handler: its name, its parameter names, and the
//! page identity derived from the **return type**.
//!
//! The page edge is read from the signature, never the body. A handler
//! returning `Page<T>` or `Result<Page<T>, E>` yields
//! `page: Some(<T>::PAGE_CONTRACT.name())` — a const expression that only
//! exists when `T` is a `#[page]` type, so a non-page type fails to compile.
//! That is the Client Exposure Firewall applied to the return type. Any
//! other return shape (`Response`, `Json<T>`, `Redirect`, `impl
//! IntoResponse`) yields `page: None`; such a handler declares its page on
//! the route, or carries an explicit `#[page("Name")]` helper attribute.
//!
//! One file, one macro: this is the entirety of the `#[controller]`
//! expansion.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{FnArg, ImplItem, ImplItemFn, ItemImpl, Pat, PathArguments, ReturnType, Type};

use crate::diagnostic::{MacroError, MacroErrorCode, MacroResult};

/// The `#[controller]` attribute macro.
pub fn controller(_attr: TokenStream, item: TokenStream) -> MacroResult {
    let mut item_impl: ItemImpl =
        syn::parse2(item).map_err(|err| MacroError::from_syn(MacroErrorCode::ArcM001, err))?;

    let mut methods = Vec::new();
    for item in &mut item_impl.items {
        if let ImplItem::Fn(method) = item {
            validate(method)?;
            methods.push(describe(method)?);
        }
    }

    let self_ty = &item_impl.self_ty;
    let (impl_generics, _, where_clause) = item_impl.generics.split_for_impl();

    Ok(quote! {
        #item_impl

        #[automatically_derived]
        impl #impl_generics ::arcature::ControllerMetadata for #self_ty #where_clause {
            const METHODS: &'static [::arcature::ControllerMethod] = &[
                #(#methods),*
            ];
        }
    })
}

/// Enforces the controller-method contract: `pub async fn` with a return
/// type and no `self` receiver (an Axum handler is a free function).
fn validate(method: &ImplItemFn) -> Result<(), MacroError> {
    let sig = &method.sig;
    let span = sig.ident.span();

    if !matches!(method.vis, syn::Visibility::Public(_)) {
        return Err(MacroError::new(
            MacroErrorCode::ArcM004,
            span,
            "controller methods must be `pub`",
        ));
    }
    if sig.asyncness.is_none() {
        return Err(MacroError::new(
            MacroErrorCode::ArcM004,
            span,
            "controller methods must be `async fn`",
        ));
    }
    if matches!(sig.output, ReturnType::Default) {
        return Err(MacroError::new(
            MacroErrorCode::ArcM004,
            span,
            "controller methods must have a return type",
        ));
    }
    if sig
        .inputs
        .iter()
        .any(|arg| matches!(arg, FnArg::Receiver(_)))
    {
        return Err(MacroError::new(
            MacroErrorCode::ArcM004,
            span,
            "controller methods must not take `self` — a controller method is an \
             Axum handler, so its parameters are extractors",
        ));
    }

    Ok(())
}

/// Builds the `ControllerMethod` const expression for one handler, stripping
/// the `#[page("Name")]` helper attribute from the emitted method.
fn describe(method: &mut ImplItemFn) -> Result<TokenStream, MacroError> {
    let name = method.sig.ident.to_string();
    let params = method.sig.inputs.iter().map(param_name).collect::<Vec<_>>();

    let page = match take_page_attribute(method)? {
        Some(explicit) => quote! { ::core::option::Option::Some(#explicit) },
        None => page_from_return_type(&method.sig.output),
    };

    Ok(quote! {
        ::arcature::ControllerMethod {
            name: #name,
            params: &[#(#params),*],
            page: #page,
        }
    })
}

/// The parameter's binding name. A destructuring pattern
/// (`Path((a, b)): Path<(u32, u32)>`) has no single name; it reports `"_"`
/// rather than inventing one, keeping the slice's arity honest.
fn param_name(arg: &FnArg) -> String {
    match arg {
        FnArg::Typed(typed) => match &*typed.pat {
            Pat::Ident(ident) => ident.ident.to_string(),
            _ => "_".to_string(),
        },
        // Rejected by `validate`; unreachable in a well-formed expansion.
        FnArg::Receiver(_) => "self".to_string(),
    }
}

/// Removes a `#[page("Name")]` helper attribute from the method and returns
/// the declared page identity. The attribute is the escape hatch for a
/// handler that renders a page but does not return `Page<T>`; it must be
/// stripped, or the real `#[page]` attribute macro would try to expand it.
fn take_page_attribute(method: &mut ImplItemFn) -> Result<Option<String>, MacroError> {
    let Some(index) = method.attrs.iter().position(|a| a.path().is_ident("page")) else {
        return Ok(None);
    };
    let attr = method.attrs.remove(index);
    let literal: syn::LitStr = attr.parse_args().map_err(|_| {
        MacroError::new(
            MacroErrorCode::ArcM002,
            attr.path()
                .get_ident()
                .map_or_else(proc_macro2::Span::call_site, syn::Ident::span),
            "#[page] on a controller method requires a string literal page name, \
             e.g. #[page(\"users/show\")]",
        )
    })?;

    let name = literal.value();
    if name.is_empty() {
        return Err(MacroError::new(
            MacroErrorCode::ArcM002,
            literal.span(),
            "#[page] page name must not be empty",
        ));
    }
    Ok(Some(name))
}

/// Derives the page identity from the return type: `Page<T>` or
/// `Result<Page<T>, E>` yields `Some(<T>::PAGE_CONTRACT.name())`, everything
/// else yields `None`.
fn page_from_return_type(output: &ReturnType) -> TokenStream {
    let ReturnType::Type(_, ty) = output else {
        return quote! { ::core::option::Option::None };
    };

    let page_type = page_argument(ty).or_else(|| {
        // `Result<Page<T>>` / `Result<Page<T>, E>` — look through the ok type.
        first_generic_argument(ty, "Result").and_then(page_argument)
    });

    match page_type {
        Some(inner) => quote! {
            ::core::option::Option::Some(<#inner>::PAGE_CONTRACT.name())
        },
        None => quote! { ::core::option::Option::None },
    }
}

/// The `T` of a `Page<T>` type, if `ty` is one.
fn page_argument(ty: &Type) -> Option<&Type> {
    first_generic_argument(ty, "Page")
}

/// The first generic type argument of `ty`, when `ty` is a path type whose
/// final segment is named `expected` (so both `Page<T>` and
/// `arcature::Page<T>` match).
fn first_generic_argument<'a>(ty: &'a Type, expected: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else { return None };
    let segment = path.path.segments.last()?;
    if segment.ident != expected {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.iter().find_map(|argument| match argument {
        syn::GenericArgument::Type(inner) => Some(inner),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn expand(tokens: proc_macro2::TokenStream) -> String {
        controller(proc_macro2::TokenStream::new(), tokens)
            .expect("controller should expand")
            .to_string()
    }

    fn expand_err(tokens: proc_macro2::TokenStream) -> MacroError {
        controller(proc_macro2::TokenStream::new(), tokens).expect_err("should be rejected")
    }

    #[test]
    fn emits_the_impl_unchanged_alongside_the_metadata_impl() {
        let s = expand(quote! {
            impl HomeController {
                pub async fn index() -> Response { todo!() }
            }
        });
        assert!(s.contains("impl HomeController"), "got: {s}");
        assert!(s.contains("pub async fn index"), "got: {s}");
        assert!(
            s.contains("impl :: arcature :: ControllerMetadata for HomeController"),
            "got: {s}"
        );
        assert!(s.contains("const METHODS :"), "got: {s}");
    }

    #[test]
    fn records_the_method_name_and_parameter_names() {
        let s = expand(quote! {
            impl LinkController {
                pub async fn store(auth: Current, input: StoreLink) -> Response { todo!() }
            }
        });
        assert!(s.contains("name : \"store\""), "got: {s}");
        assert!(s.contains("params : & [\"auth\" , \"input\"]"), "got: {s}");
    }

    #[test]
    fn a_destructuring_parameter_reports_underscore_rather_than_a_made_up_name() {
        let s = expand(quote! {
            impl A {
                pub async fn show(Path((a, b)): Path<(u32, u32)>) -> Response { todo!() }
            }
        });
        assert!(s.contains("params : & [\"_\"]"), "got: {s}");
    }

    #[test]
    fn derives_the_page_identity_from_a_bare_page_return_type() {
        let s = expand(quote! {
            impl A {
                pub async fn index() -> Page<HomePage> { todo!() }
            }
        });
        assert!(
            s.contains("Some (< HomePage > :: PAGE_CONTRACT . name ())"),
            "got: {s}"
        );
    }

    #[test]
    fn derives_the_page_identity_through_result_with_one_or_two_arguments() {
        let one = expand(quote! {
            impl A { pub async fn index() -> Result<Page<HomePage>> { todo!() } }
        });
        assert!(one.contains("< HomePage > :: PAGE_CONTRACT"), "got: {one}");

        let two = expand(quote! {
            impl A { pub async fn index() -> Result<Page<HomePage>, Error> { todo!() } }
        });
        assert!(two.contains("< HomePage > :: PAGE_CONTRACT"), "got: {two}");
    }

    #[test]
    fn derives_the_page_identity_through_a_qualified_path() {
        let s = expand(quote! {
            impl A {
                pub async fn index() -> arcature::Result<arcature::Page<pages::HomePage>> { todo!() }
            }
        });
        assert!(
            s.contains("< pages :: HomePage > :: PAGE_CONTRACT"),
            "got: {s}"
        );
    }

    #[test]
    fn a_non_page_return_type_has_no_page_edge() {
        for output in [
            quote! { Response },
            quote! { Result<Response> },
            quote! { Json<UserResource> },
            quote! { impl IntoResponse },
        ] {
            let s = expand(quote! {
                impl A { pub async fn index() -> #output { todo!() } }
            });
            assert!(
                s.contains("page : :: core :: option :: Option :: None"),
                "got: {s}"
            );
        }
    }

    #[test]
    fn an_explicit_page_attribute_is_stripped_and_wins_over_the_return_type() {
        let s = expand(quote! {
            impl A {
                #[page("users/show")]
                pub async fn show() -> Response { todo!() }
            }
        });
        assert!(s.contains("Some (\"users/show\")"), "got: {s}");
        assert!(
            !s.contains("# [page"),
            "attribute must be stripped, got: {s}"
        );
    }

    #[test]
    fn an_explicit_page_attribute_overrides_a_page_return_type() {
        let s = expand(quote! {
            impl A {
                #[page("Overridden")]
                pub async fn index() -> Page<HomePage> { todo!() }
            }
        });
        assert!(s.contains("Some (\"Overridden\")"), "got: {s}");
        assert!(!s.contains("PAGE_CONTRACT"), "got: {s}");
    }

    #[test]
    fn an_empty_page_attribute_name_is_rejected() {
        let err = expand_err(quote! {
            impl A {
                #[page("")]
                pub async fn index() -> Response { todo!() }
            }
        });
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }

    #[test]
    fn a_non_literal_page_attribute_is_rejected() {
        let err = expand_err(quote! {
            impl A {
                #[page(users::show)]
                pub async fn index() -> Response { todo!() }
            }
        });
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }

    #[test]
    fn an_empty_impl_emits_an_empty_methods_slice() {
        let s = expand(quote! { impl A {} });
        assert!(s.contains("const METHODS :"), "got: {s}");
        assert!(s.contains("& []"), "got: {s}");
    }

    #[test]
    fn generic_controllers_carry_their_generics_onto_the_metadata_impl() {
        let s = expand(quote! {
            impl<S> Controller<S> where S: Clone {
                pub async fn index() -> Response { todo!() }
            }
        });
        assert!(
            s.contains("impl < S > :: arcature :: ControllerMetadata for Controller < S >"),
            "got: {s}"
        );
        assert!(s.contains("where S : Clone"), "got: {s}");
    }

    #[test]
    fn rejects_a_private_method() {
        let err = expand_err(quote! {
            impl A { async fn index() -> Response { todo!() } }
        });
        assert_eq!(err.code(), MacroErrorCode::ArcM004);
        assert!(err.to_compile_error().to_string().contains("pub"));
    }

    #[test]
    fn rejects_a_sync_method() {
        let err = expand_err(quote! {
            impl A { pub fn index() -> Response { todo!() } }
        });
        assert_eq!(err.code(), MacroErrorCode::ArcM004);
        assert!(err.to_compile_error().to_string().contains("async"));
    }

    #[test]
    fn rejects_a_method_without_a_return_type() {
        let err = expand_err(quote! {
            impl A { pub async fn index() { } }
        });
        assert_eq!(err.code(), MacroErrorCode::ArcM004);
        assert!(err.to_compile_error().to_string().contains("return type"));
    }

    #[test]
    fn rejects_a_self_receiver() {
        let err = expand_err(quote! {
            impl A { pub async fn index(&self) -> Response { todo!() } }
        });
        assert_eq!(err.code(), MacroErrorCode::ArcM004);
        assert!(err.to_compile_error().to_string().contains("self"));
    }

    #[test]
    fn rejects_input_that_is_not_an_impl_block() {
        let err = expand_err(quote! { pub struct NotAnImpl; });
        assert_eq!(err.code(), MacroErrorCode::ArcM001);
    }
}
