//! [`ModuleDeclaration`]: the parsed `module!` body and its `syn::Parse`
//! implementation.
//!
//! The body is an optional visibility, a module name, and a braced set of
//! named sections. Each section is `keyword: <payload>`, the trailing comma
//! is optional, and section order is free.

use syn::parse::ParseStream;

use super::schedule_spec::{ScheduleSpec, parse_interval, parse_time};

/// The parsed `module!` input.
#[derive(Debug)]
pub struct ModuleDeclaration {
    /// Whether the generated descriptor const is `pub`.
    pub visibility: syn::Visibility,
    /// The module name (e.g. `"Accounts"`).
    pub name: String,
    /// The identifier of the generated const (e.g. `ACCOUNTS_MODULE`).
    pub ident: syn::Ident,
    /// Imported module names.
    pub imports: Vec<String>,
    /// Exported capability names.
    pub exports: Vec<String>,
    /// Controller type names.
    pub controllers: Vec<String>,
    /// Service type names.
    pub services: Vec<String>,
    /// Policy type names.
    pub policies: Vec<String>,
    /// Optional path to a `&'static [RouteDescriptor]` const, typically the
    /// `<NAME>_ROUTES` const emitted by a sibling `routes!` block. When
    /// present, the route metadata is threaded into the module descriptor so
    /// the application graph -- and the UAG -- carries routes per module,
    /// not just a per-module router function. `None` leaves `routes` empty.
    pub routes: Option<syn::Path>,
    /// Event -> listener bindings, as (event name, listener name) pairs.
    pub listeners: Vec<(String, String)>,
    /// Job handler bindings, as (kind, version, handler name) triples.
    pub jobs: Vec<(String, i16, String)>,
    /// Command bindings, as (name, function name) pairs.
    pub commands: Vec<(String, String)>,
    /// Schedule bindings, as (job kind, version, cadence) triples.
    pub schedules: Vec<(String, i16, ScheduleSpec)>,
}

/// The section keywords the `module!` body accepts.
mod keyword {
    syn::custom_keyword!(imports);
    syn::custom_keyword!(exports);
    syn::custom_keyword!(controllers);
    syn::custom_keyword!(services);
    syn::custom_keyword!(policies);
    syn::custom_keyword!(routes);
    syn::custom_keyword!(listeners);
    syn::custom_keyword!(jobs);
    syn::custom_keyword!(commands);
    syn::custom_keyword!(schedules);
}

impl syn::parse::Parse for ModuleDeclaration {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let visibility: syn::Visibility = input.parse()?;
        let name_ident: syn::Ident = input.parse()?;
        let name = name_ident.to_string();
        let ident = syn::Ident::new(
            &format!("{}_MODULE", name.to_uppercase()),
            name_ident.span(),
        );

        let content;
        syn::braced!(content in input);

        let mut declaration = ModuleDeclaration {
            visibility,
            name,
            ident,
            imports: Vec::new(),
            exports: Vec::new(),
            controllers: Vec::new(),
            services: Vec::new(),
            policies: Vec::new(),
            routes: None,
            listeners: Vec::new(),
            jobs: Vec::new(),
            commands: Vec::new(),
            schedules: Vec::new(),
        };

        while !content.is_empty() {
            declaration.parse_section(&content)?;
            let _: Option<syn::Token![,]> = content.parse()?;
        }

        Ok(declaration)
    }
}

impl ModuleDeclaration {
    /// Parses one `keyword: <payload>` section into `self`.
    fn parse_section(&mut self, input: ParseStream<'_>) -> syn::Result<()> {
        let lookahead = input.lookahead1();

        if lookahead.peek(keyword::imports) {
            self.imports = parse_keyed(input, |i| i.parse::<keyword::imports>(), parse_idents)?;
        } else if lookahead.peek(keyword::exports) {
            self.exports = parse_keyed(input, |i| i.parse::<keyword::exports>(), parse_idents)?;
        } else if lookahead.peek(keyword::controllers) {
            self.controllers =
                parse_keyed(input, |i| i.parse::<keyword::controllers>(), parse_idents)?;
        } else if lookahead.peek(keyword::services) {
            self.services = parse_keyed(input, |i| i.parse::<keyword::services>(), parse_idents)?;
        } else if lookahead.peek(keyword::policies) {
            self.policies = parse_keyed(input, |i| i.parse::<keyword::policies>(), parse_idents)?;
        } else if lookahead.peek(keyword::routes) {
            let path = parse_keyed(input, |i| i.parse::<keyword::routes>(), |i| i.parse())?;
            // A module owns exactly one routes block.
            if self.routes.is_some() {
                return Err(syn::Error::new(
                    input.span(),
                    "duplicate `routes:` section -- a module may declare at most one",
                ));
            }
            self.routes = Some(path);
        } else if lookahead.peek(keyword::listeners) {
            self.listeners =
                parse_keyed(input, |i| i.parse::<keyword::listeners>(), parse_listeners)?;
        } else if lookahead.peek(keyword::jobs) {
            self.jobs = parse_keyed(input, |i| i.parse::<keyword::jobs>(), parse_jobs)?;
        } else if lookahead.peek(keyword::commands) {
            self.commands = parse_keyed(input, |i| i.parse::<keyword::commands>(), parse_commands)?;
        } else if lookahead.peek(keyword::schedules) {
            self.schedules =
                parse_keyed(input, |i| i.parse::<keyword::schedules>(), parse_schedules)?;
        } else {
            return Err(lookahead.error());
        }

        Ok(())
    }
}

/// Parses `<keyword> : <payload>`, consuming the keyword and the colon and
/// delegating the payload to `payload`.
fn parse_keyed<K, F, P, T>(input: ParseStream<'_>, keyword: F, payload: P) -> syn::Result<T>
where
    F: FnOnce(ParseStream<'_>) -> syn::Result<K>,
    P: FnOnce(ParseStream<'_>) -> syn::Result<T>,
{
    keyword(input)?;
    let _: syn::Token![:] = input.parse()?;
    payload(input)
}

/// Parses `[Item, Item, ...]` into the identifier names it lists.
fn parse_idents(input: ParseStream<'_>) -> syn::Result<Vec<String>> {
    let content;
    syn::bracketed!(content in input);
    Ok(
        syn::punctuated::Punctuated::<syn::Ident, syn::Token![,]>::parse_terminated(&content)?
            .into_iter()
            .map(|ident| ident.to_string())
            .collect(),
    )
}

/// Parses `[Event => listener, ...]` into (event, listener) name pairs.
fn parse_listeners(input: ParseStream<'_>) -> syn::Result<Vec<(String, String)>> {
    let content;
    syn::bracketed!(content in input);

    let mut bindings = Vec::new();
    while !content.is_empty() {
        let event: syn::Ident = content.parse()?;
        let _: syn::Token![=>] = content.parse()?;
        let listener: syn::Ident = content.parse()?;
        bindings.push((event.to_string(), listener.to_string()));
        let _: Option<syn::Token![,]> = content.parse()?;
    }
    Ok(bindings)
}

/// Parses `[kind v1 => handler, ...]` into (kind, version, handler)
/// triples. The version is optional and defaults to 1.
fn parse_jobs(input: ParseStream<'_>) -> syn::Result<Vec<(String, i16, String)>> {
    let content;
    syn::bracketed!(content in input);

    let mut bindings = Vec::new();
    while !content.is_empty() {
        let kind: syn::Ident = content.parse()?;
        // Between the kind and `=>` there is either a version or nothing,
        // so any identifier here must be a version.
        let version = if content.peek(syn::Ident) {
            parse_version(&content.parse::<syn::Ident>()?)?
        } else {
            1
        };
        let _: syn::Token![=>] = content.parse()?;
        let handler: syn::Ident = content.parse()?;
        bindings.push((kind.to_string(), version, handler.to_string()));
        let _: Option<syn::Token![,]> = content.parse()?;
    }
    Ok(bindings)
}

/// Parses `["name" => function, ...]` into (name, function) pairs. The
/// command name is a string literal, so it may contain colons and dots.
fn parse_commands(input: ParseStream<'_>) -> syn::Result<Vec<(String, String)>> {
    let content;
    syn::bracketed!(content in input);

    let mut bindings = Vec::new();
    while !content.is_empty() {
        let name: syn::LitStr = content.parse()?;
        let _: syn::Token![=>] = content.parse()?;
        let function: syn::Ident = content.parse()?;
        bindings.push((name.value(), function.to_string()));
        let _: Option<syn::Token![,]> = content.parse()?;
    }
    Ok(bindings)
}

/// Parses `[kind [v2] every "5m", kind daily "03:00", ...]` into (kind,
/// version, cadence) triples. The version is optional and defaults to 1.
fn parse_schedules(input: ParseStream<'_>) -> syn::Result<Vec<(String, i16, ScheduleSpec)>> {
    let content;
    syn::bracketed!(content in input);

    let mut bindings = Vec::new();
    while !content.is_empty() {
        let kind: syn::Ident = content.parse()?;

        // The cadence keywords `every` and `daily` do not look like a
        // version, so peeking for a leading `v` is unambiguous.
        let version = match content.cursor().ident() {
            Some((ident, _)) if is_version_ident(&ident.to_string()) => {
                parse_version(&content.parse::<syn::Ident>()?)?
            }
            _ => 1,
        };

        bindings.push((kind.to_string(), version, parse_cadence(&content)?));
        let _: Option<syn::Token![,]> = content.parse()?;
    }
    Ok(bindings)
}

/// Parses `every "5m"` or `daily "03:00"` into a [`ScheduleSpec`].
fn parse_cadence(input: ParseStream<'_>) -> syn::Result<ScheduleSpec> {
    let lookahead = input.lookahead1();
    if !lookahead.peek(syn::Ident) {
        return Err(lookahead.error());
    }

    let cadence: syn::Ident = input.parse()?;
    let literal: syn::LitStr = input.parse()?;
    let value = literal.value();

    match cadence.to_string().as_str() {
        "every" => parse_interval(&value)
            .map(|seconds| ScheduleSpec::Every { seconds })
            .ok_or_else(|| {
                syn::Error::new(
                    literal.span(),
                    format!("invalid interval `{value}` (expected like `5m`, `1h`, `30s`, `1d`)"),
                )
            }),
        "daily" => parse_time(&value)
            .map(|(hour, minute)| ScheduleSpec::Daily { hour, minute })
            .ok_or_else(|| {
                syn::Error::new(
                    literal.span(),
                    format!("invalid time `{value}` (expected `HH:MM`)"),
                )
            }),
        other => Err(syn::Error::new(
            cadence.span(),
            format!("unknown cadence `{other}` (expected `every` or `daily`)"),
        )),
    }
}

/// Whether an identifier looks like a version marker (`v1`, `v2`, ...).
fn is_version_ident(s: &str) -> bool {
    s.len() > 1 && s.starts_with('v') && s[1..].bytes().all(|b| b.is_ascii_digit())
}

/// Parses a `v<N>` identifier into its version number.
fn parse_version(ident: &syn::Ident) -> syn::Result<i16> {
    let text = ident.to_string();
    let Some(digits) = text.strip_prefix('v') else {
        return Err(syn::Error::new(
            ident.span(),
            format!("expected a version like `v1` (got `{text}`)"),
        ));
    };

    let version: i16 = digits
        .parse()
        .map_err(|e| syn::Error::new(ident.span(), format!("invalid version `{text}`: {e}")))?;
    if version < 1 {
        return Err(syn::Error::new(ident.span(), "version must be >= 1"));
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn parse(tokens: proc_macro2::TokenStream) -> syn::Result<ModuleDeclaration> {
        syn::parse2(tokens)
    }

    #[test]
    fn derives_the_const_ident_from_the_module_name() {
        let declaration = parse(quote! { pub Accounts {} }).unwrap();
        assert_eq!(declaration.name, "Accounts");
        assert_eq!(declaration.ident.to_string(), "ACCOUNTS_MODULE");
    }

    #[test]
    fn every_section_defaults_to_empty() {
        let declaration = parse(quote! { Accounts {} }).unwrap();
        assert!(declaration.imports.is_empty());
        assert!(declaration.controllers.is_empty());
        assert!(declaration.routes.is_none());
        assert!(declaration.schedules.is_empty());
    }

    #[test]
    fn section_order_is_free() {
        let declaration = parse(quote! {
            pub Accounts {
                services: [AuthService],
                imports: [Notifications],
            }
        })
        .unwrap();
        assert_eq!(declaration.imports, vec!["Notifications"]);
        assert_eq!(declaration.services, vec!["AuthService"]);
    }

    #[test]
    fn parses_listener_bindings() {
        let declaration =
            parse(quote! { A { listeners: [UserRegistered => send_welcome] } }).unwrap();
        assert_eq!(
            declaration.listeners,
            vec![("UserRegistered".to_string(), "send_welcome".to_string())]
        );
    }

    #[test]
    fn job_version_defaults_to_one_and_can_be_given() {
        let declaration = parse(quote! {
            A { jobs: [send_email => handle, prune v3 => prune_handler] }
        })
        .unwrap();
        assert_eq!(declaration.jobs[0].1, 1);
        assert_eq!(declaration.jobs[1].1, 3);
    }

    #[test]
    fn parses_command_bindings_with_colons_in_the_name() {
        let declaration = parse(quote! { A { commands: ["users:prune" => prune] } }).unwrap();
        assert_eq!(
            declaration.commands,
            vec![("users:prune".to_string(), "prune".to_string())]
        );
    }

    #[test]
    fn parses_both_schedule_cadences() {
        let declaration = parse(quote! {
            A { schedules: [sweep every "5m", digest v2 daily "03:30"] }
        })
        .unwrap();
        assert_eq!(
            declaration.schedules[0].2,
            ScheduleSpec::Every { seconds: 300 }
        );
        assert_eq!(declaration.schedules[1].1, 2);
        assert_eq!(
            declaration.schedules[1].2,
            ScheduleSpec::Daily {
                hour: 3,
                minute: 30
            }
        );
    }

    #[test]
    fn parses_a_routes_path() {
        let declaration = parse(quote! { A { routes: ACCOUNTS_ROUTES } }).unwrap();
        assert!(declaration.routes.is_some());
    }

    #[test]
    fn rejects_a_second_routes_section() {
        let err = parse(quote! { A { routes: ONE, routes: TWO } }).unwrap_err();
        assert!(err.to_string().contains("duplicate `routes:`"));
    }

    #[test]
    fn rejects_an_unknown_section() {
        assert!(parse(quote! { A { widgets: [X] } }).is_err());
    }

    #[test]
    fn rejects_an_unknown_cadence() {
        let err = parse(quote! { A { schedules: [sweep hourly "5m"] } }).unwrap_err();
        assert!(err.to_string().contains("unknown cadence"));
    }

    #[test]
    fn rejects_an_invalid_interval() {
        let err = parse(quote! { A { schedules: [sweep every "5w"] } }).unwrap_err();
        assert!(err.to_string().contains("invalid interval"));
    }

    #[test]
    fn rejects_a_zero_version() {
        let err = parse(quote! { A { jobs: [send v0 => handle] } }).unwrap_err();
        assert!(err.to_string().contains("version must be >= 1"));
    }
}
