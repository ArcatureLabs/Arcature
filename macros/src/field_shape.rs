//! Shared field-shape extraction for `#[request]` and `#[resource]`.
//!
//! Both macros derive a `&'static [::arcature::FieldShape]` view of a
//! struct's named fields: the field name, the Rust type rendered as a clean
//! string, and the `#[validate(...)]` rule strings (present only on
//! `#[request]` inputs). The Rust -> TypeScript type mapping lives in
//! `arcature-build` codegen -- the single generator -- so this module
//! renders no TypeScript; it faithfully captures the Rust type string and
//! lets codegen own the cross-stack mapping.
//!
//! The output is plain `&'static` data: the macro emits a
//! `const FIELDS: &'static [::arcature::FieldShape] = &[ ... ];` slice,
//! which `routes!` resolves at compile time when a route declares
//! `action: T` or `query: T`, baking the field shapes into the
//! `RouteDescriptor` const with no runtime type registry and no
//! `TypeId`/`Any` container.

use proc_macro2::{TokenStream, TokenTree};
use quote::{ToTokens, quote};
use syn::spanned::Spanned;

use crate::diagnostic::{MacroError, MacroErrorCode};

/// A field's extracted shape: name, clean Rust type string, and validate
/// rule strings.
///
/// This is the macro-side intermediate representation; the macro emits one
/// `::arcature::FieldShape` (the `&'static` runtime type) per entry.
pub struct FieldShapeData {
    /// The field name (e.g. `"url"`).
    pub name: String,
    /// The Rust type as a clean string (e.g. `"String"`, `"Option<String>"`,
    /// `"Vec<i64>"`). Rendered by [`clean_token_stream`] so generics nest
    /// without proc_macro2's default inter-token spacing.
    pub ty: String,
    /// The `#[validate(...)]` rule strings, in source order (e.g. `["url"]`,
    /// `["length(min=1, max=120)"]`). Empty for `#[resource]` fields, which
    /// carry no validation attributes.
    pub validates: Vec<String>,
}

/// Collects the field shapes of a named struct, in declaration order.
///
/// Returns a [`MacroError`] (code `ARC-M002`) if the struct does not have
/// named fields. The `#[request]`/`#[resource]` parsers already enforce
/// this, so the error path is defence in depth.
pub fn collect_field_shapes(
    item_struct: &syn::ItemStruct,
) -> Result<Vec<FieldShapeData>, MacroError> {
    let syn::Fields::Named(named) = &item_struct.fields else {
        return Err(MacroError::new(
            MacroErrorCode::ArcM002,
            item_struct.fields.span(),
            "field-shape extraction requires a struct with named fields",
        ));
    };

    Ok(named
        .named
        .iter()
        .map(|field| FieldShapeData {
            name: field
                .ident
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            ty: clean_token_stream(&field.ty.to_token_stream()),
            validates: extract_validate_rules(&field.attrs),
        })
        .collect())
}

/// Emits the `&[ ::arcature::FieldShape { .. }, .. ]` token stream for a
/// field-shape list. The caller wraps this in a trait impl's `const FIELDS`
/// initializer.
pub fn emit_field_shape_slice(fields: &[FieldShapeData]) -> TokenStream {
    let entries = fields.iter().map(|f| {
        let name = &f.name;
        let ty = &f.ty;
        let validates = &f.validates;
        quote! {
            ::arcature::FieldShape {
                name: #name,
                ty: #ty,
                validates: &[#(#validates),*],
            }
        }
    });
    quote! { &[#(#entries),*] }
}

/// Extracts the `#[validate(...)]` rule strings from a field's attributes,
/// in source order. Each rule is the cleaned inner token stream (e.g.
/// `"url"`, `"length(min=1, max=120)"`). Non-`validate` attributes are
/// ignored.
fn extract_validate_rules(attrs: &[syn::Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("validate"))
        .filter_map(|attr| match &attr.meta {
            syn::Meta::List(meta_list) => Some(clean_token_stream(&meta_list.tokens)),
            _ => None,
        })
        .filter(|rule| !rule.is_empty())
        .collect()
}

/// Renders a token stream as a clean string: generics nest without the
/// inter-token spaces `proc_macro2` inserts by default (`Option < String >`
/// becomes `Option<String>`), a space is kept between adjacent identifiers
/// (`dyn Trait`), and a space follows a comma (`HashMap<String, i64>`).
/// Recurses into groups so inner content is cleaned too.
fn clean_token_stream(ts: &TokenStream) -> String {
    let mut out = String::new();
    let mut prev_ident = false;
    let mut prev_was_comma = false;

    for token in ts.clone() {
        let s = match &token {
            TokenTree::Group(group) => {
                let (open, close) = match group.delimiter() {
                    proc_macro2::Delimiter::Parenthesis => ("(", ")"),
                    proc_macro2::Delimiter::Brace => ("{", "}"),
                    proc_macro2::Delimiter::Bracket => ("[", "]"),
                    proc_macro2::Delimiter::None => ("", ""),
                };
                let inner = clean_token_stream(&group.stream());
                format!("{open}{inner}{close}")
            }
            _ => token.to_string(),
        };

        let cur_ident = s
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let cur_is_close = s.starts_with([')', ']', '}']);

        // Space between two identifiers (e.g. `dyn Trait`).
        if cur_ident && prev_ident {
            out.push(' ');
        }
        // Space after a comma, unless the next token closes a delimiter
        // (e.g. `HashMap<String, i64>`, not `(a, )`).
        if prev_was_comma && !cur_is_close {
            out.push(' ');
        }

        out.push_str(&s);
        prev_ident = cur_ident;
        prev_was_comma = s.ends_with(',');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::{ItemStruct, Type};

    fn parse_struct(tokens: TokenStream) -> ItemStruct {
        syn::parse2(tokens).expect("struct should parse")
    }

    fn clean_type(s: &str) -> String {
        let ty: Type = syn::parse_str(s).expect("type should parse");
        clean_token_stream(&ty.to_token_stream())
    }

    #[test]
    fn clean_renders_simple_generics() {
        assert_eq!(clean_type("String"), "String");
        assert_eq!(clean_type("Option<String>"), "Option<String>");
        assert_eq!(clean_type("Vec<i64>"), "Vec<i64>");
    }

    #[test]
    fn clean_renders_nested_generics() {
        assert_eq!(clean_type("Option<Vec<String>>"), "Option<Vec<String>>");
        assert_eq!(clean_type("HashMap<String, i64>"), "HashMap<String, i64>");
    }

    #[test]
    fn clean_collapses_path_separators() {
        assert_eq!(clean_type("std::string::String"), "std::string::String");
    }

    #[test]
    fn collect_extracts_request_field_shapes() {
        let item = parse_struct(quote! {
            pub struct StoreLinkRequest {
                #[validate(url)]
                pub url: String,
                #[validate(length(min = 1, max = 120))]
                pub title: String,
                pub description: Option<String>,
            }
        });
        let shapes = collect_field_shapes(&item).expect("collect");
        assert_eq!(shapes.len(), 3);
        assert_eq!(shapes[0].name, "url");
        assert_eq!(shapes[0].ty, "String");
        assert_eq!(shapes[0].validates, vec!["url"]);
        assert_eq!(shapes[1].validates, vec!["length(min=1, max=120)"]);
        assert_eq!(shapes[2].ty, "Option<String>");
        assert!(shapes[2].validates.is_empty());
    }

    #[test]
    fn collect_extracts_resource_field_shapes_without_validates() {
        let item = parse_struct(quote! {
            pub struct LinkResource {
                pub id: String,
                pub tags: Vec<String>,
            }
        });
        let shapes = collect_field_shapes(&item).expect("collect");
        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes[0].ty, "String");
        assert!(shapes[0].validates.is_empty());
        assert_eq!(shapes[1].ty, "Vec<String>");
    }

    #[test]
    fn collect_rejects_tuple_struct() {
        let item = parse_struct(quote! { pub struct Wrapper(pub String); });
        let err = collect_field_shapes(&item).unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }

    #[test]
    fn emit_slice_renders_field_shape_consts() {
        let shapes = vec![
            FieldShapeData {
                name: "url".into(),
                ty: "String".into(),
                validates: vec!["url".into()],
            },
            FieldShapeData {
                name: "description".into(),
                ty: "Option<String>".into(),
                validates: vec![],
            },
        ];
        let s = emit_field_shape_slice(&shapes).to_string();
        assert!(s.contains("FieldShape"), "got: {s}");
        assert!(s.contains("\"url\""), "got: {s}");
        assert!(s.contains("\"Option<String>\""), "got: {s}");
        assert!(s.contains("validates"), "got: {s}");
    }
}
