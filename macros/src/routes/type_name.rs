//! Rendering `syn` paths and types as the plain names baked into route
//! metadata.
//!
//! One responsibility: turn AST nodes into the `&'static str` names the
//! `RouteDescriptor` const carries (`handler`, `action_type`, `query_type`),
//! and unwrap a `Vec<T>` query into its element type plus a collection flag.

/// Renders a handler path as `Segment::Segment` (`LinksController::index`).
pub fn handler(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

/// Renders a path's final segment (`crate::requests::StoreLink` -> `StoreLink`).
///
/// This is the name the TypeScript codegen uses for the generated interface,
/// so it is the bare type name, never the module route to it.
pub fn final_segment(path: &syn::Path) -> String {
    path.segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default()
}

/// Renders a type's final path segment, or `""` for a non-path type.
pub fn of_type(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Unwraps a query type: `Vec<T>` -> `(T, true)`, anything else -> `(ty, false)`.
///
/// The flag becomes `RouteDescriptor::query_array`, which tells the codegen
/// whether the route responds with `T` or `T[]`.
pub fn unwrap_vec(ty: &syn::Type) -> (&syn::Type, bool) {
    let syn::Type::Path(type_path) = ty else {
        return (ty, false);
    };
    let Some(last) = type_path.path.segments.last() else {
        return (ty, false);
    };
    if last.ident != "Vec" {
        return (ty, false);
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return (ty, false);
    };
    let Some(syn::GenericArgument::Type(inner)) = args.args.first() else {
        return (ty, false);
    };
    (inner, true)
}

#[cfg(test)]
mod tests {
    use super::{final_segment, handler, of_type, unwrap_vec};

    #[test]
    fn handler_joins_segments_with_double_colon() {
        let path: syn::Path = syn::parse_quote!(LinksController::index);
        assert_eq!(handler(&path), "LinksController::index");
    }

    #[test]
    fn final_segment_drops_the_module_route() {
        let path: syn::Path = syn::parse_quote!(crate::requests::StoreLinkRequest);
        assert_eq!(final_segment(&path), "StoreLinkRequest");
    }

    #[test]
    fn of_type_reads_the_final_segment() {
        let ty: syn::Type = syn::parse_quote!(crate::resources::LinkResource);
        assert_eq!(of_type(&ty), "LinkResource");
    }

    #[test]
    fn of_type_is_empty_for_a_non_path_type() {
        let ty: syn::Type = syn::parse_quote!((u8, u8));
        assert_eq!(of_type(&ty), "");
    }

    #[test]
    fn vec_query_unwraps_to_its_element() {
        let ty: syn::Type = syn::parse_quote!(Vec<LinkResource>);
        let (element, is_array) = unwrap_vec(&ty);
        assert!(is_array);
        assert_eq!(of_type(element), "LinkResource");
    }

    #[test]
    fn single_query_is_not_an_array() {
        let ty: syn::Type = syn::parse_quote!(LinkResource);
        let (element, is_array) = unwrap_vec(&ty);
        assert!(!is_array);
        assert_eq!(of_type(element), "LinkResource");
    }
}
