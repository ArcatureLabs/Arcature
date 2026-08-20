//! `#[listener(Event)]` — register an event listener with dispatch metadata.
//!
//! An attribute macro applied to an `async fn`. Emits the function unchanged
//! (it remains a genuine listener closure the app registers on the
//! `Dispatcher`) and, next to it, a `pub static LISTENER_BINDING:
//! ListenerBinding` const recording the event-to-listener binding for `arc
//! check` / `arc modules` inspection.
//!
//! One file, one macro: this is the entirety of the `#[listener]` expansion.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{ItemFn, ReturnType, Type, parse_macro_input};

/// The `#[listener(EventType)]` attribute macro.
pub fn listener(attr: TokenStream, item: TokenStream) -> TokenStream {
    let event_type = match parse_listener_attr(attr) {
        Ok(t) => t,
        Err(e) => return e.to_compile_error().into(),
    };
    let item_fn = parse_macro_input!(item as ItemFn);

    if let Err(e) = validate(&item_fn) {
        return e.to_compile_error().into();
    }

    let fn_name = &item_fn.sig.ident;
    let listener_name = fn_name.to_string();

    quote! {
        #item_fn

        #[doc = "The compile-time event-to-listener binding for inspection."]
        pub static LISTENER_BINDING: ::arcature::events::ListenerBinding =
            ::arcature::events::ListenerBinding {
                event: #event_type,
                listener: #listener_name,
            };
    }
    .into()
}

/// Parse the `#[listener(EventType)]` argument: a single type path.
fn parse_listener_attr(attr: TokenStream) -> syn::Result<String> {
    if attr.is_empty() {
        return Err(syn::Error::new(
            Span::call_site(),
            "#[listener(EventType)] requires an event type argument",
        ));
    }
    let ty: Type = syn::parse(attr)?;
    Ok(quote!(#ty).to_string().replace(' ', ""))
}

/// Validate the function is `async fn` with a return type.
fn validate(item_fn: &ItemFn) -> syn::Result<()> {
    let sig = &item_fn.sig;
    if sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            &sig.ident,
            "listeners must be `async fn`",
        ));
    }
    if matches!(sig.output, ReturnType::Default) {
        return Err(syn::Error::new_spanned(
            &sig.ident,
            "listeners must have a return type",
        ));
    }
    Ok(())
}
