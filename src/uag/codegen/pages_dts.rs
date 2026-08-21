//! `pages.d.ts` -- the prop type of every registered page.
//!
//! The types come from [`ContractArtifact`], which is the Client Exposure
//! Firewall's output: a prop only has a schema because a `ClientData` type
//! put it there. So the generated declarations describe exactly what the
//! server is allowed to send, and a page component destructuring a prop the
//! server never sends fails to compile.
//!
//! Page identities like `"links/Show"` are not TypeScript identifiers, so
//! the output is one mapped type keyed by identity rather than one
//! interface per page. That also means a renamed page produces a key
//! change, which every use site sees.
//!
//! [`ContractArtifact`]: crate::inertia::contracts::ContractArtifact

use std::collections::BTreeMap;

use crate::inertia::contracts::{ContractType, PropSchema};
use crate::uag::schema::UagArtifact;

use super::{GENERATED_HEADER, ts_string};

/// Generate `pages.d.ts` from the artifact's registered page contracts.
#[must_use]
pub fn generate(artifact: &UagArtifact) -> String {
    let mut out = String::from(GENERATED_HEADER);
    out.push_str("\nexport type PageProps = {\n");
    for (name, page) in artifact.pages() {
        out.push_str(&format!("  {}: ", ts_string(name)));
        out.push_str(&object(page.props().fields(), 1));
        out.push_str(";\n");
    }
    out.push_str("};\n\n");
    out.push_str("export type PageName = keyof PageProps;\n\n");
    out.push_str("export type PropsOf<N extends PageName> = PageProps[N];\n");
    out
}

/// Renders one page's props, or `Record<string, never>` when a page takes
/// none -- an empty object literal type in TypeScript accepts any object,
/// which is the opposite of what "this page has no props" means.
fn object(fields: &BTreeMap<String, PropSchema>, depth: usize) -> String {
    if fields.is_empty() {
        return "Record<string, never>".to_owned();
    }
    let inner = "  ".repeat(depth + 1);
    let close = "  ".repeat(depth);
    let mut out = String::from("{\n");
    for (name, prop) in fields {
        let optional = if prop.is_required() { "" } else { "?" };
        out.push_str(&format!(
            "{inner}{}{optional}: {};\n",
            ts_string(name),
            contract_type(prop.ty(), depth + 1)
        ));
    }
    out.push_str(&close);
    out.push('}');
    out
}

/// Renders one contract type. The mapping is total by construction:
/// [`ContractType`] has exactly the variants JSON can carry, so unlike the
/// Rust type strings in `FieldShape` there is no `unknown` fallback here.
fn contract_type(ty: &ContractType, depth: usize) -> String {
    match ty {
        ContractType::Boolean => "boolean".to_owned(),
        ContractType::Number => "number".to_owned(),
        ContractType::String => "string".to_owned(),
        ContractType::Array { item } => match **item {
            // `T | null` needs parentheses before `[]` binds to it.
            ContractType::Nullable { .. } => format!("({})[]", contract_type(item, depth)),
            _ => format!("{}[]", contract_type(item, depth)),
        },
        ContractType::Object { fields } => object(fields, depth),
        ContractType::Nullable { item } => format!("{} | null", contract_type(item, depth)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inertia::contracts::{ClientData, PageSchema, PropsSchema};

    #[derive(serde::Serialize)]
    struct Tag;

    impl ClientData for Tag {
        fn exposure_schema() -> PropsSchema {
            PropsSchema::new().required("label", ContractType::string())
        }
    }

    fn artifact(pages: Vec<(&str, PropsSchema)>) -> UagArtifact {
        UagArtifact::new(
            BTreeMap::new(),
            Vec::new(),
            pages
                .into_iter()
                .map(|(name, props)| (name.to_owned(), PageSchema::new(props)))
                .collect(),
        )
    }

    #[test]
    fn the_generated_file_imports_nothing() {
        let dts = generate(&artifact(vec![("Home", PropsSchema::new())]));
        assert!(!dts.contains("import "), "{dts}");
    }

    #[test]
    fn a_required_prop_is_not_optional_and_an_optional_prop_is() {
        let dts = generate(&artifact(vec![(
            "Home",
            PropsSchema::new()
                .required("name", ContractType::string())
                .optional("greeting", ContractType::string()),
        )]));
        assert!(dts.contains(r#""name": string;"#), "{dts}");
        assert!(dts.contains(r#""greeting"?: string;"#), "{dts}");
    }

    #[test]
    fn a_page_identity_with_a_slash_is_still_a_valid_key() {
        let dts = generate(&artifact(vec![("links/Show", PropsSchema::new())]));
        assert!(
            dts.contains(r#"  "links/Show": Record<string, never>;"#),
            "{dts}"
        );
    }

    #[test]
    fn a_page_with_no_props_accepts_no_props() {
        let dts = generate(&artifact(vec![("Home", PropsSchema::new())]));
        assert!(dts.contains("Record<string, never>"), "{dts}");
        assert!(
            !dts.contains("{}"),
            "an empty object type would accept anything: {dts}"
        );
    }

    #[test]
    fn a_nullable_prop_becomes_a_null_union() {
        let dts = generate(&artifact(vec![(
            "Home",
            PropsSchema::new().required("bio", ContractType::nullable(ContractType::string())),
        )]));
        assert!(dts.contains(r#""bio": string | null;"#), "{dts}");
    }

    #[test]
    fn an_array_of_nullables_keeps_the_union_parenthesised() {
        let dts = generate(&artifact(vec![(
            "Home",
            PropsSchema::new().required(
                "notes",
                ContractType::array(ContractType::nullable(ContractType::string())),
            ),
        )]));
        assert!(dts.contains(r#""notes": (string | null)[];"#), "{dts}");
    }

    #[test]
    fn a_nested_client_data_object_is_inlined() {
        let dts = generate(&artifact(vec![(
            "Home",
            PropsSchema::new().nested_array::<Tag>("tags"),
        )]));
        assert!(dts.contains(r#""tags": {"#), "{dts}");
        assert!(dts.contains(r#""label": string;"#), "{dts}");
        assert!(dts.contains("}[];"), "{dts}");
    }

    #[test]
    fn the_page_name_union_and_props_lookup_are_exported() {
        let dts = generate(&artifact(vec![("Home", PropsSchema::new())]));
        assert!(
            dts.contains("export type PageName = keyof PageProps;"),
            "{dts}"
        );
        assert!(
            dts.contains("export type PropsOf<N extends PageName> = PageProps[N];"),
            "{dts}"
        );
    }

    #[test]
    fn generating_twice_yields_byte_identical_output() {
        let art = artifact(vec![(
            "Home",
            PropsSchema::new().required("name", ContractType::string()),
        )]);
        assert_eq!(generate(&art), generate(&art));
    }
}
