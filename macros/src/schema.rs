//! Shared type-to-schema mapping for `#[page]` and `#[resource]`.
//!
//! Maps a Rust field type to a `::arcature::inertia::PropsSchema` builder
//! call that constructs the corresponding browser exposure schema. This is
//! the compile-time edge of the Client Exposure Firewall: named
//! (non-primitive) field types map to `PropsSchema::nested::<T>` /
//! `nested_array::<T>` / `nested_optional::<T>`, each of which requires
//! `T: ClientData`. A field type that is only `Serialize` (an internal
//! domain model) therefore fails to compile -- the program does not
//! type-check.
//!
//! The mapping is deliberately conservative. Only the primitive scalars and
//! the `Option`/`Vec` wrappers are recognised; everything else is treated as
//! a named type that must implement `ClientData`. This keeps the boundary
//! explicit: no heuristic silently certifies a type as browser-safe.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{GenericArgument, PathArguments, Type, TypePath};

use crate::diagnostic::{MacroError, MacroErrorCode};

/// Maps a single named field to its schema builder-chain fragment.
///
/// Returns a [`MacroError`] (code `ARC-M002`) if the field type is too
/// complex to map (a bare reference, a slice, an unsupported generic).
/// Named types that do not match a known primitive are treated as nested
/// `ClientData`; that bound is enforced at the generated call site.
pub fn map_field(field_name: &str, ty: &Type) -> Result<TokenStream, MacroError> {
    match ty {
        // `Option<T>`: optional for a primitive T, nested_optional for a
        // named ClientData T.
        Type::Path(type_path) if is_path_ident(type_path, "Option") => {
            map_option(field_name, type_path)
        }

        // `Vec<T>`: an array of primitives, or nested_array of ClientData.
        Type::Path(type_path) if is_path_ident(type_path, "Vec") => map_vec(field_name, type_path),

        // A bare path: a primitive scalar or a named ClientData type.
        Type::Path(type_path) => Ok(map_path(field_name, type_path)),

        // Unsupported shapes. Do not guess -- return a clear diagnostic.
        _ => Err(MacroError::new(
            MacroErrorCode::ArcM002,
            ty.span(),
            format!(
                "#[page]/#[resource] field `{field_name}` has an unsupported type; \
                 use a named struct, a primitive, Option<T>, or Vec<T>"
            ),
        )),
    }
}

/// Maps `Option<T>` for the field named `field_name`.
fn map_option(field_name: &str, type_path: &TypePath) -> Result<TokenStream, MacroError> {
    let inner = single_generic_arg(type_path)?;
    let Type::Path(inner_path) = inner else {
        return Err(MacroError::new(
            MacroErrorCode::ArcM002,
            inner.span(),
            format!(
                "#[page]/#[resource] field `{field_name}` has an unsupported Option<T> inner type"
            ),
        ));
    };

    Ok(match primitive_contract_call(inner_path) {
        Some(prim) => quote! { .optional(#field_name, #prim) },
        // The `T: ClientData` bound is enforced at the call site.
        None => {
            let inner_ty = &inner_path.path;
            quote! { .nested_optional::<#inner_ty>(#field_name) }
        }
    })
}

/// Maps `Vec<T>` for the field named `field_name`.
fn map_vec(field_name: &str, type_path: &TypePath) -> Result<TokenStream, MacroError> {
    let inner = single_generic_arg(type_path)?;
    let Type::Path(inner_path) = inner else {
        return Err(MacroError::new(
            MacroErrorCode::ArcM002,
            inner.span(),
            format!(
                "#[page]/#[resource] field `{field_name}` has an unsupported Vec<T> inner type"
            ),
        ));
    };

    Ok(match primitive_contract_call(inner_path) {
        Some(prim) => quote! {
            .required(
                #field_name,
                ::arcature::inertia::ContractType::array(#prim),
            )
        },
        // The `T: ClientData` bound is enforced at the call site.
        None => {
            let inner_ty = &inner_path.path;
            quote! { .nested_array::<#inner_ty>(#field_name) }
        }
    })
}

/// Maps a bare path: a primitive scalar or a named `ClientData` type.
fn map_path(field_name: &str, type_path: &TypePath) -> TokenStream {
    match primitive_contract_call(type_path) {
        Some(prim) => quote! { .required(#field_name, #prim) },
        // A named type that is not a known primitive becomes `nested::<T>`.
        // The `T: ClientData` bound is the compile-time exposure firewall.
        None => {
            let ty = &type_path.path;
            quote! { .nested::<#ty>(#field_name) }
        }
    }
}

/// Returns the single generic argument of a path type, or an error.
fn single_generic_arg(type_path: &TypePath) -> Result<&Type, MacroError> {
    let last = type_path.path.segments.last().ok_or_else(|| {
        MacroError::new(
            MacroErrorCode::ArcM002,
            type_path.span(),
            "empty path in #[page]/#[resource] field type",
        )
    })?;

    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return Err(MacroError::new(
            MacroErrorCode::ArcM002,
            last.arguments.span(),
            "expected generic arguments in #[page]/#[resource] field type",
        ));
    };

    match args.args.first() {
        Some(GenericArgument::Type(ty)) => Ok(ty),
        Some(arg) => Err(MacroError::new(
            MacroErrorCode::ArcM002,
            arg.span(),
            "expected a type argument in #[page]/#[resource] field type",
        )),
        None => Err(MacroError::new(
            MacroErrorCode::ArcM002,
            args.span(),
            "expected one generic argument in #[page]/#[resource] field type",
        )),
    }
}

/// Whether the last segment of a path has the given identifier name.
fn is_path_ident(type_path: &TypePath, name: &str) -> bool {
    type_path
        .path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == name)
}

/// The `::arcature::inertia::ContractType::<prim>()` call for a recognised
/// primitive scalar path, or `None` when the type is a named type that must
/// implement `ClientData`.
fn primitive_contract_call(type_path: &TypePath) -> Option<TokenStream> {
    let name = type_path.path.segments.last()?.ident.to_string();
    match name.as_str() {
        "String" | "str" | "char" | "Url" => {
            Some(quote! { ::arcature::inertia::ContractType::string() })
        }
        "bool" => Some(quote! { ::arcature::inertia::ContractType::boolean() }),
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128"
        | "isize" | "f32" | "f64" => {
            Some(quote! { ::arcature::inertia::ContractType::number() })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(field: &str, ty_str: &str) -> Result<String, MacroError> {
        let ty: Type = syn::parse_str(ty_str).expect("test type string should parse");
        map_field(field, &ty).map(|tokens| tokens.to_string())
    }

    #[test]
    fn string_field_is_a_required_string() {
        let s = map("name", "String").unwrap();
        assert!(s.contains("required"), "got: {s}");
        assert!(s.contains("string ()"), "got: {s}");
    }

    #[test]
    fn bool_field_is_a_required_boolean() {
        let s = map("active", "bool").unwrap();
        assert!(s.contains("boolean ()"), "got: {s}");
    }

    #[test]
    fn integer_and_float_fields_are_required_numbers() {
        assert!(map("count", "i64").unwrap().contains("number ()"));
        assert!(map("ratio", "f32").unwrap().contains("number ()"));
    }

    #[test]
    fn option_of_primitive_is_an_optional_prop() {
        let s = map("bio", "Option<String>").unwrap();
        assert!(s.contains("optional"), "got: {s}");
        assert!(!s.contains("nested_optional"), "got: {s}");
    }

    #[test]
    fn option_of_named_type_is_nested_optional() {
        let s = map("avatar", "Option<AvatarResource>").unwrap();
        assert!(s.contains("nested_optional :: < AvatarResource >"), "got: {s}");
    }

    #[test]
    fn vec_of_primitive_is_a_required_array() {
        let s = map("tags", "Vec<String>").unwrap();
        assert!(s.contains("array"), "got: {s}");
        assert!(!s.contains("nested_array"), "got: {s}");
    }

    #[test]
    fn vec_of_named_type_is_nested_array() {
        let s = map("posts", "Vec<PostResource>").unwrap();
        assert!(s.contains("nested_array :: < PostResource >"), "got: {s}");
    }

    #[test]
    fn named_type_is_nested_and_requires_client_data() {
        let s = map("author", "UserResource").unwrap();
        assert!(s.contains("nested :: < UserResource >"), "got: {s}");
    }

    #[test]
    fn qualified_paths_resolve_by_their_last_segment() {
        assert!(map("name", "std::string::String").unwrap().contains("string ()"));
        assert!(map("bio", "std::option::Option<String>").unwrap().contains("optional"));
    }

    #[test]
    fn reference_types_are_rejected() {
        let err = map("name", "&'static str").unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }

    #[test]
    fn tuple_types_are_rejected() {
        let err = map("pair", "(i64, i64)").unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }

    #[test]
    fn option_without_a_type_argument_is_rejected() {
        let err = map("bio", "Option").unwrap_err();
        assert_eq!(err.code(), MacroErrorCode::ArcM002);
    }
}
