//! Validation of a parsed [`ModuleDeclaration`]: no section may list the
//! same entry twice.
//!
//! A duplicate is always a mistake -- registering the same controller,
//! importing the same module, or binding the same job kind twice means the
//! developer copy-pasted a line. Catching it here turns a silently
//! double-counted descriptor into `error[ARC-M002]` at the `module!` site.

use std::collections::BTreeSet;

use proc_macro2::Span;

use super::declaration::ModuleDeclaration;
use super::schedule_spec::ScheduleSpec;
use crate::diagnostic::{MacroError, MacroErrorCode};

/// Validates a parsed module declaration, returning `ARC-M002` on the first
/// duplicate entry found.
pub fn validate(declaration: &ModuleDeclaration) -> Result<(), MacroError> {
    let span = declaration.ident.span();

    check_names("imports", &declaration.imports, span)?;
    check_names("exports", &declaration.exports, span)?;
    check_names("controllers", &declaration.controllers, span)?;
    check_names("services", &declaration.services, span)?;
    check_names("policies", &declaration.policies, span)?;
    check_listeners(&declaration.listeners, span)?;
    check_jobs(&declaration.jobs, span)?;
    check_commands(&declaration.commands, span)?;
    check_schedules(&declaration.schedules, span)?;
    check_pages(&declaration.pages, span)?;

    Ok(())
}

/// Rejects a repeated name inside one of the plain ident sections.
fn check_names(section: &str, names: &[String], span: Span) -> Result<(), MacroError> {
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name.as_str()) {
            return Err(MacroError::new(
                MacroErrorCode::ArcM002,
                span,
                format!("duplicate entry `{name}` in `{section}`"),
            ));
        }
    }
    Ok(())
}

/// Rejects the same listener bound to the same event twice.
fn check_listeners(listeners: &[(String, String)], span: Span) -> Result<(), MacroError> {
    let mut seen = BTreeSet::new();
    for (event, listener) in listeners {
        if !seen.insert((event, listener)) {
            return Err(MacroError::new(
                MacroErrorCode::ArcM002,
                span,
                format!("duplicate listener binding `{listener}` for event `{event}`"),
            ));
        }
    }
    Ok(())
}

/// Rejects two handlers bound to the same job kind and version.
fn check_jobs(jobs: &[(String, i16, String)], span: Span) -> Result<(), MacroError> {
    let mut seen = BTreeSet::new();
    for (kind, version, _handler) in jobs {
        if !seen.insert((kind, version)) {
            return Err(MacroError::new(
                MacroErrorCode::ArcM002,
                span,
                format!("duplicate job binding `{kind}` v{version}"),
            ));
        }
    }
    Ok(())
}

/// Rejects the same command name bound twice.
fn check_commands(commands: &[(String, String)], span: Span) -> Result<(), MacroError> {
    let mut seen = BTreeSet::new();
    for (name, _function) in commands {
        if !seen.insert(name.as_str()) {
            return Err(MacroError::new(
                MacroErrorCode::ArcM002,
                span,
                format!("duplicate command binding `{name}`"),
            ));
        }
    }
    Ok(())
}

/// Rejects the same job kind and version scheduled twice: a job has at most
/// one cadence.
fn check_schedules(
    schedules: &[(String, i16, ScheduleSpec)],
    span: Span,
) -> Result<(), MacroError> {
    let mut seen = BTreeSet::new();
    for (kind, version, _cadence) in schedules {
        if !seen.insert((kind, version)) {
            return Err(MacroError::new(
                MacroErrorCode::ArcM002,
                span,
                format!("duplicate schedule binding `{kind}` v{version}"),
            ));
        }
    }
    Ok(())
}

/// Rejects the same page listed twice.
///
/// Compared by the final path segment, because that is the name the
/// descriptor records: `pages::HomePage` and `HomePage` are one page
/// written two ways, and listing both would double-count it in the graph.
fn check_pages(pages: &[syn::Path], span: Span) -> Result<(), MacroError> {
    let mut seen = BTreeSet::new();
    for page in pages {
        let name = page
            .segments
            .last()
            .map_or_else(String::new, |segment| segment.ident.to_string());
        if !seen.insert(name.clone()) {
            return Err(MacroError::new(
                MacroErrorCode::ArcM002,
                span,
                format!("duplicate entry `{name}` in `pages`"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn validate_module(tokens: proc_macro2::TokenStream) -> Result<(), MacroError> {
        let declaration: ModuleDeclaration = syn::parse2(tokens).expect("module should parse");
        validate(&declaration)
    }

    #[test]
    fn accepts_a_declaration_with_no_duplicates() {
        assert!(
            validate_module(quote! {
                pub Accounts {
                    imports: [Notifications],
                    controllers: [SessionsController, UsersController],
                    jobs: [send v1 => handle_one, send v2 => handle_two],
                }
            })
            .is_ok()
        );
    }

    #[test]
    fn rejects_a_duplicate_import() {
        let err = validate_module(quote! { A { imports: [B, B] } }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
        assert!(err.to_compile_error().to_string().contains("imports"));
    }

    #[test]
    fn rejects_a_duplicate_controller() {
        let err = validate_module(quote! { A { controllers: [C, C] } }).unwrap_err();
        assert!(err.to_compile_error().to_string().contains("controllers"));
    }

    #[test]
    fn rejects_a_duplicate_listener_binding() {
        let err = validate_module(quote! { A { listeners: [E => l, E => l] } }).unwrap_err();
        assert!(err.to_compile_error().to_string().contains("listener"));
    }

    #[test]
    fn allows_two_listeners_for_the_same_event() {
        assert!(validate_module(quote! { A { listeners: [E => one, E => two] } }).is_ok());
    }

    #[test]
    fn rejects_two_handlers_for_the_same_job_version() {
        let err = validate_module(quote! { A { jobs: [send => one, send => two] } }).unwrap_err();
        assert!(err.to_compile_error().to_string().contains("job binding"));
    }

    #[test]
    fn rejects_a_duplicate_command() {
        let err = validate_module(quote! { A { commands: ["p" => one, "p" => two] } }).unwrap_err();
        assert!(err.to_compile_error().to_string().contains("command"));
    }

    #[test]
    fn rejects_the_same_page_listed_twice_under_different_paths() {
        let err = validate_module(quote! { A { pages: [HomePage, pages::HomePage] } }).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
        assert!(err.to_compile_error().to_string().contains("pages"));
    }

    #[test]
    fn accepts_distinct_pages() {
        assert!(validate_module(quote! { A { pages: [HomePage, pages::NewLinkPage] } }).is_ok());
    }

    #[test]
    fn rejects_a_job_scheduled_twice() {
        let err = validate_module(quote! { A { schedules: [s every "5m", s daily "03:00"] } })
            .unwrap_err();
        assert!(err.to_compile_error().to_string().contains("schedule"));
    }
}
