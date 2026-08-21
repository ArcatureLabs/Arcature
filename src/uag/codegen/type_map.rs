//! The one place a Rust type string becomes something else.
//!
//! `FieldShape.ty` is the Rust type as written (`"String"`,
//! `"Option<Vec<i64>>"`). Two generators need to translate it: the
//! TypeScript emitters and the OpenAPI document. Rather than write the
//! mapping twice, the string is parsed once into a [`TypeShape`] and each
//! generator renders that shape into its own target.
//!
//! The mapping is deliberately total and deliberately small:
//!
//! | Rust | TypeScript | JSON Schema |
//! |---|---|---|
//! | `String`, `&str`, `char` | `string` | `{"type": "string"}` |
//! | any integer or float | `number` | `{"type": "number"}` |
//! | `bool` | `boolean` | `{"type": "boolean"}` |
//! | `Option<T>` | `T \| undefined` | `T` or null, and not required |
//! | `Vec<T>` | `T[]` | array of `T` |
//! | anything else | `unknown` | `{}` |
//!
//! The fallback is `unknown`, never `any`. A field the mapping does not
//! recognise should make the consuming TypeScript fail to compile until
//! someone narrows it; `any` would let the same field flow silently into a
//! runtime error.

use serde_json::{Value, json};

/// A Rust type reduced to what crosses the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeShape {
    /// A UTF-8 string.
    String,
    /// Any Rust integer or float. JSON has one number type, so the width is
    /// not carried: pretending a `u64` survives JavaScript intact would be
    /// a lie the generated types cannot back up.
    Number,
    /// A boolean.
    Boolean,
    /// A value that may be absent.
    Optional(Box<TypeShape>),
    /// A homogeneous list.
    Array(Box<TypeShape>),
    /// A type this mapping does not model.
    Unknown,
}

impl TypeShape {
    /// Whether this shape is an `Option<...>`, and therefore not required.
    #[must_use]
    pub fn is_optional(&self) -> bool {
        matches!(self, Self::Optional(_))
    }

    /// The shape with one layer of `Option` removed. A form field's editor
    /// type is the inner type; its optionality is a separate fact.
    #[must_use]
    pub fn unwrapped(&self) -> &Self {
        match self {
            Self::Optional(inner) => inner,
            other => other,
        }
    }
}

/// Parse a Rust type string into its wire shape.
///
/// Unrecognised types map to [`TypeShape::Unknown`] rather than failing:
/// one exotic field must not stop the whole artifact from generating.
#[must_use]
pub fn parse(rust_type: &str) -> TypeShape {
    let ty = normalize(rust_type);

    if let Some(inner) = generic_argument(&ty, "Option") {
        return TypeShape::Optional(Box::new(parse(inner)));
    }
    if let Some(inner) = generic_argument(&ty, "Vec") {
        return TypeShape::Array(Box::new(parse(inner)));
    }

    match ty.as_str() {
        "String" | "str" | "char" => TypeShape::String,
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" | "f32" | "f64" => TypeShape::Number,
        "bool" => TypeShape::Boolean,
        _ => TypeShape::Unknown,
    }
}

/// Render a shape as TypeScript.
#[must_use]
pub fn typescript(shape: &TypeShape) -> String {
    match shape {
        TypeShape::String => "string".to_owned(),
        TypeShape::Number => "number".to_owned(),
        TypeShape::Boolean => "boolean".to_owned(),
        TypeShape::Unknown => "unknown".to_owned(),
        TypeShape::Optional(inner) => format!("{} | undefined", typescript(inner)),
        TypeShape::Array(inner) => match **inner {
            // A union element needs parentheses before `[]` binds to it.
            TypeShape::Optional(_) => format!("({})[]", typescript(inner)),
            _ => format!("{}[]", typescript(inner)),
        },
    }
}

/// Render a shape as a JSON Schema (OpenAPI 3.1 dialect).
///
/// `Option<T>` becomes `anyOf [T, null]` because serde writes an absent
/// `Option` as JSON `null`, not as an omitted key. Requiredness is a
/// separate fact the caller records in the object's `required` list.
#[must_use]
pub fn json_schema(shape: &TypeShape) -> Value {
    match shape {
        TypeShape::String => json!({ "type": "string" }),
        TypeShape::Number => json!({ "type": "number" }),
        TypeShape::Boolean => json!({ "type": "boolean" }),
        // An empty schema accepts anything, which is the honest statement
        // about a type this mapping does not model.
        TypeShape::Unknown => json!({}),
        TypeShape::Optional(inner) => json!({
            "anyOf": [json_schema(inner), { "type": "null" }]
        }),
        TypeShape::Array(inner) => json!({
            "type": "array",
            "items": json_schema(inner),
        }),
    }
}

/// Convenience: Rust type string straight to TypeScript.
#[must_use]
pub fn rust_to_typescript(rust_type: &str) -> String {
    typescript(&parse(rust_type))
}

/// Strips references and lifetimes and reduces a path to its last segment,
/// so `&'a std::string::String` and `String` parse the same.
fn normalize(rust_type: &str) -> String {
    let mut ty = rust_type.trim();
    while let Some(rest) = ty.strip_prefix('&') {
        ty = rest.trim_start();
        if let Some(rest) = ty.strip_prefix('\'') {
            ty = rest
                .split_once(char::is_whitespace)
                .map_or("", |(_, r)| r)
                .trim_start();
        }
    }

    let (head, tail) = match ty.find('<') {
        Some(open) => (&ty[..open], &ty[open..]),
        None => (ty, ""),
    };
    let last = head.rsplit("::").next().unwrap_or(head).trim();
    format!("{last}{tail}")
}

/// The single generic argument of `name<...>`, if the type is exactly that.
fn generic_argument<'a>(ty: &'a str, name: &str) -> Option<&'a str> {
    let inner = ty
        .strip_prefix(name)?
        .strip_prefix('<')?
        .strip_suffix('>')?;
    Some(inner.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_and_chars_become_string() {
        for ty in ["String", "&str", "&'a str", "char", "std::string::String"] {
            assert_eq!(rust_to_typescript(ty), "string", "{ty}");
        }
    }

    #[test]
    fn every_integer_and_float_becomes_number() {
        for ty in ["i8", "i64", "u32", "usize", "f32", "f64", "i128"] {
            assert_eq!(rust_to_typescript(ty), "number", "{ty}");
        }
    }

    #[test]
    fn bool_becomes_boolean() {
        assert_eq!(rust_to_typescript("bool"), "boolean");
    }

    #[test]
    fn option_becomes_a_union_with_undefined() {
        assert_eq!(rust_to_typescript("Option<String>"), "string | undefined");
    }

    #[test]
    fn vec_becomes_an_array() {
        assert_eq!(rust_to_typescript("Vec<i64>"), "number[]");
    }

    #[test]
    fn a_vec_of_options_keeps_the_union_parenthesised() {
        assert_eq!(
            rust_to_typescript("Vec<Option<bool>>"),
            "(boolean | undefined)[]"
        );
    }

    #[test]
    fn nesting_survives_in_both_directions() {
        assert_eq!(
            rust_to_typescript("Option<Vec<String>>"),
            "string[] | undefined"
        );
    }

    #[test]
    fn an_unmodelled_type_becomes_unknown_and_never_any() {
        let ts = rust_to_typescript("HashMap<String, Value>");
        assert_eq!(ts, "unknown");
        assert!(
            !ts.contains("any"),
            "`any` hides errors, `unknown` catches them"
        );
    }

    #[test]
    fn an_option_is_optional_and_unwraps_to_its_inner_shape() {
        let shape = parse("Option<i32>");
        assert!(shape.is_optional());
        assert_eq!(shape.unwrapped(), &TypeShape::Number);
    }

    #[test]
    fn a_plain_type_unwraps_to_itself() {
        let shape = parse("bool");
        assert!(!shape.is_optional());
        assert_eq!(shape.unwrapped(), &TypeShape::Boolean);
    }

    #[test]
    fn json_schema_models_an_option_as_nullable() {
        assert_eq!(
            json_schema(&parse("Option<String>")),
            json!({ "anyOf": [{ "type": "string" }, { "type": "null" }] })
        );
    }

    #[test]
    fn json_schema_models_a_vec_as_an_array_of_its_item() {
        assert_eq!(
            json_schema(&parse("Vec<bool>")),
            json!({ "type": "array", "items": { "type": "boolean" } })
        );
    }

    #[test]
    fn json_schema_leaves_an_unmodelled_type_unconstrained() {
        assert_eq!(json_schema(&parse("MyEnum")), json!({}));
    }
}
