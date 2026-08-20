//! Bracketed-list and dotted-name parsers shared by the `routes!` grammar.
//!
//! One responsibility: the small, repeated token shapes -- `[a, b]`,
//! `["A", "B"]`, `[Path, Path]`, and `auth.login` -- that appear in several
//! route options and would otherwise be duplicated at each use site.

use syn::parse::ParseStream;
use syn::punctuated::Punctuated;

/// Parses `[ident, ident]` into the identifier strings.
pub fn idents(input: ParseStream<'_>) -> syn::Result<Vec<String>> {
    let content;
    syn::bracketed!(content in input);
    let items = Punctuated::<syn::Ident, syn::Token![,]>::parse_terminated(&content)?;
    Ok(items.into_iter().map(|i| i.to_string()).collect())
}

/// Parses `["A", "B"]` into the literal string values.
pub fn strings(input: ParseStream<'_>) -> syn::Result<Vec<String>> {
    let content;
    syn::bracketed!(content in input);
    let items = Punctuated::<syn::LitStr, syn::Token![,]>::parse_terminated(&content)?;
    Ok(items.into_iter().map(|lit| lit.value()).collect())
}

/// Parses `[Auth, RateLimit]` into the paths.
pub fn paths(input: ParseStream<'_>) -> syn::Result<Vec<syn::Path>> {
    let content;
    syn::bracketed!(content in input);
    let items = Punctuated::<syn::Path, syn::Token![,]>::parse_terminated(&content)?;
    Ok(items.into_iter().collect())
}

/// Parses a dotted route name (`ident(.ident)*`, e.g. `auth.login`).
///
/// Route names use single-dot separators, never `::`: they name a route in
/// the URL namespace, not a Rust item.
pub fn dotted_name(input: ParseStream<'_>) -> syn::Result<String> {
    let first: syn::Ident = input.parse()?;
    let mut name = first.to_string();
    while input.peek(syn::Token![.]) {
        let _: syn::Token![.] = input.parse()?;
        let segment: syn::Ident = input.parse()?;
        name.push('.');
        name.push_str(&segment.to_string());
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::{dotted_name, idents, paths, strings};
    use syn::parse::Parser as _;

    #[test]
    fn ident_list_is_collected() {
        let parsed = idents.parse2(quote::quote!([index, show])).unwrap();
        assert_eq!(parsed, vec!["index".to_string(), "show".to_string()]);
    }

    #[test]
    fn string_list_is_collected() {
        let parsed = strings.parse2(quote::quote!(["Home", "About"])).unwrap();
        assert_eq!(parsed, vec!["Home".to_string(), "About".to_string()]);
    }

    #[test]
    fn path_list_is_collected() {
        let parsed = paths.parse2(quote::quote!([Auth, guard::Guest])).unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn dotted_name_joins_segments_with_dots() {
        let parsed = dotted_name.parse2(quote::quote!(api.v1.links)).unwrap();
        assert_eq!(parsed, "api.v1.links");
    }

    #[test]
    fn single_segment_name_has_no_dot() {
        let parsed = dotted_name.parse2(quote::quote!(home)).unwrap();
        assert_eq!(parsed, "home");
    }
}
