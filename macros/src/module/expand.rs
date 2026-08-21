//! Code generation for `module!`.
//!
//! Emits a `<vis> const <NAME>_MODULE: ::arcature::ModuleDescriptor` and a
//! `<name>_module()` accessor. The descriptor is a `const` with no
//! allocation: every field is a `&'static` slice of string literals or
//! const-constructible binding structs.

use proc_macro2::TokenStream;
use quote::quote;

use super::declaration::ModuleDeclaration;
use super::schedule_spec::ScheduleSpec;

/// Generates the descriptor const and its accessor for a parsed `module!`
/// declaration.
pub fn expand(declaration: &ModuleDeclaration) -> TokenStream {
    let ModuleDeclaration {
        visibility,
        name,
        ident,
        imports,
        exports,
        controllers,
        services,
        policies,
        ..
    } = declaration;

    let accessor = syn::Ident::new(&format!("{}_module", name.to_lowercase()), ident.span());

    // Per-controller method metadata, parallel to `controllers`: each entry
    // is the controller's `ControllerMetadata::METHODS` slice, carrying the
    // method name and the page identity derived from the handler's `Page<T>`
    // return type. The UAG joins `RouteDescriptor.handler` to these entries
    // to infer the route-to-page edge. The controller type must be in scope
    // at the `module!` invocation site.
    let controller_methods = controllers.iter().map(|controller| {
        let controller = syn::Ident::new(controller, ident.span());
        quote! { <#controller as ::arcature::ControllerMetadata>::METHODS }
    });

    // The `routes:` section names a `&'static [RouteDescriptor]` const,
    // typically emitted by a sibling `routes!` block. Absent means the
    // module declares no routes.
    let routes = match &declaration.routes {
        Some(path) => quote! { #path },
        None => quote! { &[] },
    };

    let listeners = declaration.listeners.iter().map(|(event, listener)| {
        quote! { ::arcature::ListenerBinding { event: #event, listener: #listener } }
    });

    let jobs = declaration.jobs.iter().map(|(kind, version, handler)| {
        quote! { ::arcature::JobBinding { kind: #kind, version: #version, handler: #handler } }
    });

    let commands = declaration.commands.iter().map(|(name, function)| {
        quote! { ::arcature::CommandBinding { name: #name, function: #function } }
    });

    let schedules = declaration
        .schedules
        .iter()
        .map(|(kind, version, cadence)| {
            let cadence = match cadence {
                ScheduleSpec::Every { seconds } => {
                    quote! { ::arcature::ScheduleCadence::Every { seconds: #seconds } }
                }
                ScheduleSpec::Daily { hour, minute } => {
                    quote! { ::arcature::ScheduleCadence::Daily { hour: #hour, minute: #minute } }
                }
            };
            quote! {
                ::arcature::ScheduleBinding {
                    job: #kind,
                    version: #version,
                    cadence: #cadence,
                }
            }
        });

    // Each page contributes the identity off its own `PAGE_CONTRACT_ENTRY`,
    // rather than a name repeated here. The const only exists on a type
    // `#[page]` accepted, so a `Serialize`-only type named in `pages:` is a
    // compile error at this line -- the Client Exposure Firewall holds
    // without the module macro re-checking anything.
    let pages = declaration.pages.iter().map(|path| {
        quote! { #path::PAGE_CONTRACT_ENTRY.name }
    });

    let doc = format!("Returns the module descriptor for the `{name}` module.");

    quote! {
        #visibility const #ident: ::arcature::ModuleDescriptor =
            ::arcature::ModuleDescriptor {
                name: #name,
                imports: &[#(#imports),*],
                exports: &[#(#exports),*],
                controllers: &[#(#controllers),*],
                controller_methods: &[#(#controller_methods),*],
                services: &[#(#services),*],
                policies: &[#(#policies),*],
                routes: #routes,
                listeners: &[#(#listeners),*],
                jobs: &[#(#jobs),*],
                commands: &[#(#commands),*],
                schedules: &[#(#schedules),*],
                pages: &[#(#pages),*],
            };

        #[doc = #doc]
        ///
        /// The accessor exists so downstream code -- `application!`, chiefly
        /// -- can reach the descriptor by a predictable name without
        /// knowing the generated const identifier.
        #visibility fn #accessor() -> &'static ::arcature::ModuleDescriptor {
            &#ident
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn expand_module(tokens: proc_macro2::TokenStream) -> String {
        let declaration: ModuleDeclaration = syn::parse2(tokens).expect("module should parse");
        expand(&declaration).to_string()
    }

    #[test]
    fn emits_the_descriptor_const_and_accessor() {
        let s = expand_module(quote! { pub Accounts {} });
        assert!(s.contains("const ACCOUNTS_MODULE"), "got: {s}");
        assert!(s.contains(":: arcature :: ModuleDescriptor"), "got: {s}");
        assert!(s.contains("fn accounts_module ()"), "got: {s}");
        assert!(s.contains("\"Accounts\""), "got: {s}");
    }

    #[test]
    fn honours_the_declared_visibility() {
        assert!(expand_module(quote! { pub Accounts {} }).contains("pub const"));
        assert!(!expand_module(quote! { Accounts {} }).contains("pub const"));
    }

    #[test]
    fn emits_controller_names_and_their_method_metadata() {
        let s = expand_module(quote! { A { controllers: [HomeController] } });
        assert!(s.contains("\"HomeController\""), "got: {s}");
        assert!(
            s.contains("< HomeController as :: arcature :: ControllerMetadata > :: METHODS"),
            "got: {s}"
        );
    }

    #[test]
    fn routes_defaults_to_an_empty_slice_and_uses_the_named_const() {
        assert!(expand_module(quote! { A {} }).contains("routes : & []"));
        assert!(expand_module(quote! { A { routes: A_ROUTES } }).contains("routes : A_ROUTES"));
    }

    #[test]
    fn emits_listener_job_and_command_bindings() {
        let s = expand_module(quote! {
            A {
                listeners: [UserRegistered => send_welcome],
                jobs: [send_email v2 => handle],
                commands: ["users:prune" => prune],
            }
        });
        assert!(s.contains("ListenerBinding"), "got: {s}");
        assert!(s.contains("\"UserRegistered\""), "got: {s}");
        assert!(s.contains("JobBinding"), "got: {s}");
        assert!(s.contains("version : 2i16"), "got: {s}");
        assert!(s.contains("CommandBinding"), "got: {s}");
        assert!(s.contains("\"users:prune\""), "got: {s}");
    }

    #[test]
    fn reads_each_page_identity_off_its_contract_entry() {
        let s = expand_module(quote! { A { pages: [HomePage, pages::NewLinkPage] } });
        assert!(
            s.contains("HomePage :: PAGE_CONTRACT_ENTRY . name"),
            "got: {s}"
        );
        assert!(
            s.contains("pages :: NewLinkPage :: PAGE_CONTRACT_ENTRY . name"),
            "got: {s}"
        );
    }

    #[test]
    fn pages_defaults_to_an_empty_slice() {
        assert!(expand_module(quote! { A {} }).contains("pages : & []"));
    }

    #[test]
    fn emits_both_schedule_cadences() {
        let s = expand_module(quote! {
            A { schedules: [sweep every "5m", digest daily "03:30"] }
        });
        assert!(s.contains("Every { seconds : 300u64 }"), "got: {s}");
        assert!(
            s.contains("Daily { hour : 3u8 , minute : 30u8 }"),
            "got: {s}"
        );
    }
}
