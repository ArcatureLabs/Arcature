//! `forms.ts` -- the field names, types, and validation rules of every
//! Action route.
//!
//! This file deliberately does not wrap `useForm`. Inertia already owns
//! form state, and a generated wrapper would be one more thing to version
//! against the adapter's own releases. What the frontend cannot derive on
//! its own is the *shape*: which fields the Rust request type declares,
//! which are optional, and which `#[validate(...)]` rules the server will
//! apply. That is what is emitted, as plain data the application feeds into
//! `useForm` itself.
//!
//! The rules are emitted verbatim from `FieldShape.validates` rather than
//! translated into a client validation library's dialect. Validation stays
//! server-side; these strings exist so a form can show a hint before the
//! round trip, not so the browser can re-implement the check.

use super::type_map;
use super::{GENERATED_HEADER, ts_string};
use crate::uag::schema::{UagArtifact, UagPayload};

/// Generate `forms.ts` from the artifact's Action routes.
///
/// Keyed by route name, because that is the name the caller already uses
/// with `route()`. An unnamed action route is skipped for the same reason
/// it is skipped in `routes.ts`: there is nothing to key it by.
#[must_use]
pub fn generate(artifact: &UagArtifact) -> String {
    let mut actions: Vec<(&str, &UagPayload)> = artifact
        .routes()
        .iter()
        .filter(|r| !r.name.is_empty())
        .filter_map(|r| r.action.as_ref().map(|a| (r.name.as_str(), a)))
        .collect();
    actions.sort_by_key(|(name, _)| *name);
    actions.dedup_by(|a, b| a.0 == b.0);

    let mut out = String::from(GENERATED_HEADER);

    out.push_str("\nexport const forms = {\n");
    for (name, action) in &actions {
        out.push_str(&format!("  {}: {{\n", ts_string(name)));
        out.push_str(&format!("    request: {},\n", ts_string(&action.type_name)));
        out.push_str("    fields: {\n");
        for field in &action.fields {
            let shape = type_map::parse(&field.ty);
            let rules = field
                .validates
                .iter()
                .map(|r| ts_string(r))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "      {}: {{ type: {}, optional: {}, rules: [{rules}] }},\n",
                ts_string(&field.name),
                ts_string(&type_map::typescript(shape.unwrapped())),
                shape.is_optional(),
            ));
        }
        out.push_str("    },\n");
        out.push_str("  },\n");
    }
    out.push_str("} as const;\n\n");

    out.push_str("export type FormName = keyof typeof forms;\n\n");
    out.push_str(
        "export type FormFieldName<N extends FormName> = keyof (typeof forms)[N][\"fields\"];\n\n",
    );

    out.push_str("/** The value type of every form field, as the server declares it. */\n");
    out.push_str("export type FormValues = {\n");
    for (name, action) in &actions {
        out.push_str(&format!("  {}: {{\n", ts_string(name)));
        for field in &action.fields {
            out.push_str(&format!(
                "    {}: {};\n",
                ts_string(&field.name),
                type_map::rust_to_typescript(&field.ty),
            ));
        }
        out.push_str("  };\n");
    }
    out.push_str("};\n");

    out
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::uag::schema::{UagField, UagRoute};

    fn field(name: &str, ty: &str, validates: &[&str]) -> UagField {
        UagField {
            name: name.to_owned(),
            ty: ty.to_owned(),
            validates: validates.iter().map(|v| (*v).to_owned()).collect(),
        }
    }

    fn action_route(name: &str, type_name: &str, fields: Vec<UagField>) -> UagRoute {
        UagRoute {
            module: "Links".to_owned(),
            method: "POST".to_owned(),
            path: "/links".to_owned(),
            name: name.to_owned(),
            handler: "LinksController::store".to_owned(),
            params: Vec::new(),
            pages: BTreeSet::new(),
            action: Some(UagPayload {
                type_name: type_name.to_owned(),
                fields,
            }),
            query: None,
            query_string: None,
            policies: BTreeSet::new(),
        }
    }

    fn store() -> UagArtifact {
        UagArtifact::new(
            BTreeMap::new(),
            vec![action_route(
                "links.store",
                "StoreLinkRequest",
                vec![
                    field("url", "String", &["url"]),
                    field("description", "Option<String>", &[]),
                ],
            )],
            BTreeMap::new(),
        )
    }

    #[test]
    fn the_generated_file_imports_nothing() {
        let ts = generate(&store());
        assert!(!ts.contains("import "), "{ts}");
        assert!(
            !ts.contains("useForm"),
            "the app wires useForm itself: {ts}"
        );
    }

    #[test]
    fn a_field_carries_its_type_optionality_and_rules() {
        let ts = generate(&store());
        assert!(
            ts.contains(r#"      "url": { type: "string", optional: false, rules: ["url"] },"#),
            "{ts}"
        );
    }

    #[test]
    fn an_option_field_is_marked_optional_and_reports_its_inner_type() {
        let ts = generate(&store());
        assert!(
            ts.contains(r#"      "description": { type: "string", optional: true, rules: [] },"#),
            "{ts}"
        );
    }

    #[test]
    fn the_value_type_keeps_the_undefined_union() {
        let ts = generate(&store());
        assert!(
            ts.contains(r#"    "description": string | undefined;"#),
            "{ts}"
        );
    }

    #[test]
    fn fields_keep_the_order_the_request_struct_declares() {
        let ts = generate(&store());
        let url = ts.find(r#""url": {"#).expect("url field");
        let description = ts.find(r#""description": {"#).expect("description field");
        assert!(url < description, "declaration order drives form order");
    }

    #[test]
    fn a_route_without_an_action_contributes_nothing() {
        let art = UagArtifact::new(
            BTreeMap::new(),
            vec![UagRoute {
                action: None,
                ..action_route("links.index", "", Vec::new())
            }],
            BTreeMap::new(),
        );
        assert!(!generate(&art).contains("links.index"));
    }

    #[test]
    fn an_unmodelled_field_type_is_unknown_and_never_any() {
        let art = UagArtifact::new(
            BTreeMap::new(),
            vec![action_route(
                "links.store",
                "R",
                vec![field("meta", "HashMap<String, String>", &[])],
            )],
            BTreeMap::new(),
        );
        let ts = generate(&art);
        assert!(ts.contains(r#"type: "unknown""#), "{ts}");
        assert!(!ts.contains(": any"), "{ts}");
    }

    #[test]
    fn validation_rules_are_emitted_verbatim() {
        let art = UagArtifact::new(
            BTreeMap::new(),
            vec![action_route(
                "links.store",
                "R",
                vec![field(
                    "title",
                    "String",
                    &["length(min=1,max=120)", "required"],
                )],
            )],
            BTreeMap::new(),
        );
        assert!(
            generate(&art).contains(r#"rules: ["length(min=1,max=120)", "required"]"#),
            "{}",
            generate(&art)
        );
    }

    #[test]
    fn generating_twice_yields_byte_identical_output() {
        assert_eq!(generate(&store()), generate(&store()));
    }
}
