//! `routes.ts` -- the typed route table and the `route()` helper.
//!
//! The point of this file is one compile error: calling
//! `route('links.show')` without `{ link }` must not type-check. That is
//! achieved with a conditional rest-argument tuple, so the parameter object
//! is required exactly when the path has parameters and forbidden when it
//! does not.
//!
//! The file imports nothing. An application referencing a route it renamed
//! in Rust gets a `tsc` error the next time typegen runs, and it gets it
//! without installing anything.

use super::{GENERATED_HEADER, ts_string};
use crate::uag::schema::UagArtifact;

/// The type-level machinery appended after the route table.
///
/// `[ParamName<N>] extends [never]` is the standard way to ask "is this
/// union empty" without the check distributing over the union; a route with
/// no parameters has `params: readonly []`, whose `[number]` element type
/// is `never`, and therefore takes no second argument at all.
const HELPER: &str = r#"
export type RouteName = keyof typeof routes;

type ParamName<N extends RouteName> = (typeof routes)[N]["params"][number];

type RouteParams<N extends RouteName> = { [K in ParamName<N>]: string | number };

type RouteArgs<N extends RouteName> = [ParamName<N>] extends [never]
  ? []
  : [params: RouteParams<N>];

/**
 * Builds the path for a named route.
 *
 * Omitting a required parameter is a compile error, not a runtime surprise.
 */
export function route<N extends RouteName>(name: N, ...args: RouteArgs<N>): string;
export function route(name: RouteName, params?: Record<string, string | number>): string {
  return routes[name].path.replace(/\{([^}]+)\}/g, (_match: string, token: string) => {
    const wildcard = token.startsWith("*");
    const key = wildcard ? token.slice(1) : token;
    const value = params?.[key];
    if (value === undefined) {
      throw new Error(`route(${name}): missing parameter "${key}"`);
    }
    const encoded = String(value);
    // A wildcard stands for many path segments, so its separators survive.
    return wildcard
      ? encoded.split("/").map(encodeURIComponent).join("/")
      : encodeURIComponent(encoded);
  });
}

/** The HTTP method a named route answers on. */
export function routeMethod(name: RouteName): string {
  return routes[name].method;
}
"#;

/// Generate `routes.ts` from the artifact.
///
/// Only named routes appear: an unnamed route has no key a caller could
/// write, so listing it would add noise without adding reachability.
/// Names are emitted in sorted order, which makes the file's diff a
/// function of the route set rather than of module declaration order.
#[must_use]
pub fn generate(artifact: &UagArtifact) -> String {
    let mut entries: Vec<(&str, &str, &str, &[String])> = artifact
        .routes()
        .iter()
        .filter(|r| !r.name.is_empty())
        .map(|r| {
            (
                r.name.as_str(),
                r.method.as_str(),
                r.path.as_str(),
                r.params.as_slice(),
            )
        })
        .collect();
    entries.sort_unstable();
    // Two routes sharing a name would become two keys in one object
    // literal, which TypeScript rejects outright. Collapsing to the first
    // after sorting keeps the file compilable and keeps the choice
    // deterministic; the duplicate itself is a Rust-side mistake.
    entries.dedup_by(|a, b| a.0 == b.0);

    let mut out = String::from(GENERATED_HEADER);
    out.push_str("\nexport const routes = {\n");
    for (name, method, path, params) in entries {
        let params = params
            .iter()
            .map(|p| ts_string(p))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "  {}: {{ method: {}, path: {}, params: [{params}] }},\n",
            ts_string(name),
            ts_string(method),
            ts_string(path),
        ));
    }
    out.push_str("} as const;\n");
    out.push_str(HELPER);
    out
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::uag::schema::UagRoute;

    fn route(name: &str, method: &str, path: &str, params: &[&str]) -> UagRoute {
        UagRoute {
            module: "Links".to_owned(),
            method: method.to_owned(),
            path: path.to_owned(),
            name: name.to_owned(),
            handler: "LinksController::show".to_owned(),
            params: params.iter().map(|p| (*p).to_owned()).collect(),
            pages: BTreeSet::new(),
            action: None,
            query: None,
            query_string: None,
            policies: BTreeSet::new(),
        }
    }

    fn artifact(routes: Vec<UagRoute>) -> UagArtifact {
        UagArtifact::new(BTreeMap::new(), routes, BTreeMap::new())
    }

    #[test]
    fn the_generated_file_imports_nothing() {
        let ts = generate(&artifact(vec![route("links.index", "GET", "/links", &[])]));
        assert!(!ts.contains("import "), "{ts}");
        assert!(!ts.contains("require("), "{ts}");
    }

    #[test]
    fn a_route_becomes_a_table_entry_with_its_method_path_and_params() {
        let ts = generate(&artifact(vec![route(
            "links.show",
            "GET",
            "/links/{link}",
            &["link"],
        )]));
        assert!(
            ts.contains(
                r#"  "links.show": { method: "GET", path: "/links/{link}", params: ["link"] },"#
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_parameterless_route_gets_an_empty_params_tuple() {
        let ts = generate(&artifact(vec![route("links.index", "GET", "/links", &[])]));
        assert!(ts.contains(r#"params: [] }"#), "{ts}");
    }

    #[test]
    fn the_route_name_union_is_derived_from_the_table() {
        let ts = generate(&artifact(Vec::new()));
        assert!(
            ts.contains("export type RouteName = keyof typeof routes;"),
            "{ts}"
        );
    }

    #[test]
    fn a_renamed_route_changes_the_generated_union() {
        let before = generate(&artifact(vec![route(
            "links.show",
            "GET",
            "/l/{id}",
            &["id"],
        )]));
        let after = generate(&artifact(vec![route(
            "links.detail",
            "GET",
            "/l/{id}",
            &["id"],
        )]));
        assert!(before.contains(r#""links.show""#));
        assert!(!after.contains(r#""links.show""#));
        assert!(after.contains(r#""links.detail""#));
    }

    #[test]
    fn unnamed_routes_are_left_out_because_nothing_can_reference_them() {
        let ts = generate(&artifact(vec![route("", "GET", "/internal", &[])]));
        assert!(!ts.contains("/internal"), "{ts}");
    }

    #[test]
    fn table_order_does_not_depend_on_declaration_order() {
        let one = generate(&artifact(vec![
            route("b", "GET", "/b", &[]),
            route("a", "GET", "/a", &[]),
        ]));
        let two = generate(&artifact(vec![
            route("a", "GET", "/a", &[]),
            route("b", "GET", "/b", &[]),
        ]));
        assert_eq!(one, two);
    }

    #[test]
    fn two_routes_sharing_a_name_collapse_to_one_key() {
        let ts = generate(&artifact(vec![
            route("links.show", "GET", "/a", &[]),
            route("links.show", "GET", "/b", &[]),
        ]));
        assert_eq!(ts.matches(r#""links.show":"#).count(), 1, "{ts}");
    }

    #[test]
    fn generating_twice_yields_byte_identical_output() {
        let art = artifact(vec![route("links.show", "GET", "/l/{id}", &["id"])]);
        assert_eq!(generate(&art), generate(&art));
    }
}
