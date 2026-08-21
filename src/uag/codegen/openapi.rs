//! The OpenAPI 3.1 document, generated from the same artifact as the
//! TypeScript.
//!
//! There is no `utoipa` here and no annotation to keep in sync with the
//! handler. Everything in the document already exists in the UAG: the
//! `routes!` macro baked the request and response field shapes into the
//! route descriptor, and `#[validate(...)]` rules travel with the fields.
//! An API description derived from a second, hand-written source drifts;
//! this one cannot.
//!
//! Two things are deliberately *not* invented:
//!
//! * A route whose response shape is unknown gets no `responses` object.
//!   OpenAPI 3.1 permits that, and it is the honest statement -- claiming a
//!   `200` for a handler that redirects would make the document worse than
//!   silence.
//! * Only the validation rules with an exact JSON Schema counterpart become
//!   constraints. `regex(...)` names a Rust const, not a pattern the
//!   document could carry, so it is left out rather than guessed at.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::type_map::{self, TypeShape};
use crate::uag::schema::{UagArtifact, UagField, UagPayload, UagRoute};

/// The parts of the document that cannot be derived from the application
/// graph.
///
/// There is no `generated_at`: a timestamp would make every regeneration a
/// diff, which is the one thing the artifact exists to avoid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenApiOptions {
    /// The API title.
    pub title: String,
    /// The API version, as the application defines it.
    pub version: String,
    /// An optional description.
    pub description: Option<String>,
}

impl Default for OpenApiOptions {
    fn default() -> Self {
        Self {
            title: "Arcature application".to_owned(),
            version: "0.0.0".to_owned(),
            description: None,
        }
    }
}

/// The OpenAPI version this generator emits.
pub const OPENAPI_VERSION: &str = "3.1.0";

/// Generate the OpenAPI document as a JSON value.
#[must_use]
pub fn generate(artifact: &UagArtifact, options: &OpenApiOptions) -> Value {
    let mut info = Map::new();
    info.insert("title".to_owned(), json!(options.title));
    info.insert("version".to_owned(), json!(options.version));
    if let Some(description) = &options.description {
        info.insert("description".to_owned(), json!(description));
    }

    let mut paths: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    let mut schemas: BTreeMap<String, Value> = BTreeMap::new();

    for route in artifact.routes() {
        register_schemas(route, &mut schemas);
        let item = paths.entry(openapi_path(&route.path)).or_default();
        // A duplicate method on one path is a Rust-side conflict that
        // `validate` reports; the first declaration wins here so the
        // document stays a valid OpenAPI object either way.
        item.entry(route.method.to_lowercase())
            .or_insert_with(|| operation(route));
    }

    let mut document = Map::new();
    document.insert("openapi".to_owned(), json!(OPENAPI_VERSION));
    document.insert("info".to_owned(), Value::Object(info));
    document.insert(
        "paths".to_owned(),
        Value::Object(
            paths
                .into_iter()
                .map(|(path, item)| (path, Value::Object(item)))
                .collect(),
        ),
    );
    if !schemas.is_empty() {
        document.insert(
            "components".to_owned(),
            json!({ "schemas": Value::Object(schemas.into_iter().collect()) }),
        );
    }
    Value::Object(document)
}

/// Generate the OpenAPI document as deterministic pretty JSON.
///
/// # Errors
///
/// Returns the `serde_json` error if serialization fails.
pub fn generate_json(
    artifact: &UagArtifact,
    options: &OpenApiOptions,
) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&generate(artifact, options))
}

/// Rewrites an axum path into OpenAPI's template syntax. The two agree on
/// `{name}`; only the wildcard marker has to go.
fn openapi_path(path: &str) -> String {
    path.replace("{*", "{")
}

/// Adds any named payload types this route references to the component
/// schema map.
fn register_schemas(route: &UagRoute, schemas: &mut BTreeMap<String, Value>) {
    if let Some(action) = &route.action {
        insert_named(action.type_name.as_str(), &action.fields, schemas);
    }
    if let Some(query) = &route.query {
        insert_named(query.type_name.as_str(), &query.fields, schemas);
    }
    // Query-string fields become `in: query` parameters rather than a body
    // schema, so they are inlined at the use site and registered nowhere.
}

/// Registers one named object schema. An unnamed payload is inlined at its
/// use site instead.
fn insert_named(type_name: &str, fields: &[UagField], schemas: &mut BTreeMap<String, Value>) {
    if type_name.is_empty() {
        return;
    }
    schemas
        .entry(type_name.to_owned())
        .or_insert_with(|| object_schema(fields));
}

/// Builds the operation object for one route.
fn operation(route: &UagRoute) -> Value {
    let mut op = Map::new();
    op.insert("operationId".to_owned(), json!(operation_id(route)));
    if !route.module.is_empty() {
        op.insert("tags".to_owned(), json!([route.module]));
    }

    let mut parameters: Vec<Value> = route
        .params
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "in": "path",
                "required": true,
                "schema": { "type": "string" },
            })
        })
        .collect();
    if let Some(qs) = &route.query_string {
        for field in &qs.fields {
            let shape = type_map::parse(&field.ty);
            parameters.push(json!({
                "name": field.name,
                "in": "query",
                "required": !shape.is_optional(),
                "schema": field_schema(field),
            }));
        }
    }
    if !parameters.is_empty() {
        op.insert("parameters".to_owned(), Value::Array(parameters));
    }

    if let Some(action) = &route.action {
        op.insert(
            "requestBody".to_owned(),
            json!({
                "required": true,
                "content": { "application/json": { "schema": payload_ref(action) } },
            }),
        );
    }

    if let Some(responses) = responses(route) {
        op.insert("responses".to_owned(), responses);
    }

    Value::Object(op)
}

/// The operation identifier. A route name is already the application's own
/// stable handle, so it is used verbatim; an unnamed route falls back to a
/// slug of its method and path, which is unique because a duplicate
/// method+path is itself reported as an error.
fn operation_id(route: &UagRoute) -> String {
    if !route.name.is_empty() {
        return route.name.clone();
    }
    let slug: String = route
        .path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{}{}", route.method.to_lowercase(), slug)
}

/// What the route answers with, when the artifact knows. `None` when it
/// does not -- see the module docs.
fn responses(route: &UagRoute) -> Option<Value> {
    if let Some(query) = &route.query {
        let schema = if query.array {
            json!({ "type": "array", "items": payload_ref_named(&query.type_name, &query.fields) })
        } else {
            payload_ref_named(&query.type_name, &query.fields)
        };
        return Some(json!({
            "200": {
                "description": "The query result.",
                "content": { "application/json": { "schema": schema } },
            }
        }));
    }
    if !route.pages.is_empty() {
        let pages: Vec<&str> = route.pages.iter().map(String::as_str).collect();
        return Some(json!({
            "200": {
                "description": format!("The Inertia page `{}`.", pages.join("`, `")),
                "content": { "text/html": {} },
            }
        }));
    }
    None
}

/// A `$ref` to the named component, or the inline object schema when the
/// payload has no type name.
fn payload_ref(payload: &UagPayload) -> Value {
    payload_ref_named(&payload.type_name, &payload.fields)
}

/// See [`payload_ref`]; split out because a query carries its element type
/// name and fields separately from the array flag.
fn payload_ref_named(type_name: &str, fields: &[UagField]) -> Value {
    if type_name.is_empty() {
        object_schema(fields)
    } else {
        json!({ "$ref": format!("#/components/schemas/{type_name}") })
    }
}

/// An object schema over a field list. `required` lists every field whose
/// Rust type is not an `Option`.
fn object_schema(fields: &[UagField]) -> Value {
    let properties: Map<String, Value> = fields
        .iter()
        .map(|f| (f.name.clone(), field_schema(f)))
        .collect();
    let required: Vec<&str> = fields
        .iter()
        .filter(|f| !type_map::parse(&f.ty).is_optional())
        .map(|f| f.name.as_str())
        .collect();

    let mut schema = Map::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert("properties".to_owned(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".to_owned(), json!(required));
    }
    Value::Object(schema)
}

/// One field's schema, with the validation constraints applied to the
/// non-null branch: `Option<String>` with `length(max = 5)` is
/// `anyOf [{string, maxLength 5}, null]`, not a `maxLength` on the union.
fn field_schema(field: &UagField) -> Value {
    let shape = type_map::parse(&field.ty);
    let inner = shape.unwrapped();
    let mut schema = type_map::json_schema(inner);
    apply_rules(&mut schema, inner, &field.validates);
    if shape.is_optional() {
        json!({ "anyOf": [schema, { "type": "null" }] })
    } else {
        schema
    }
}

/// Translates the validation rules that have an exact JSON Schema
/// counterpart. Everything else is skipped: a constraint the document
/// cannot state faithfully is worse than an absent one, because a client
/// generator would enforce it.
fn apply_rules(schema: &mut Value, shape: &TypeShape, rules: &[String]) {
    let Value::Object(map) = schema else {
        return;
    };
    for rule in rules {
        let (name, args) = parse_rule(rule);
        match name {
            "email" => {
                map.insert("format".to_owned(), json!("email"));
            }
            "url" => {
                map.insert("format".to_owned(), json!("uri"));
            }
            "length" => {
                let (min_key, max_key) = match shape {
                    TypeShape::Array(_) => ("minItems", "maxItems"),
                    _ => ("minLength", "maxLength"),
                };
                for (key, bounds) in [("min", min_key), ("max", max_key)] {
                    if let Some(value) = number_arg(&args, key) {
                        map.insert(bounds.to_owned(), value);
                    }
                }
                if let Some(value) = number_arg(&args, "equal") {
                    map.insert(min_key.to_owned(), value.clone());
                    map.insert(max_key.to_owned(), value);
                }
            }
            "range" => {
                for (key, bounds) in [("min", "minimum"), ("max", "maximum")] {
                    if let Some(value) = number_arg(&args, key) {
                        map.insert(bounds.to_owned(), value);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Splits `"length(min = 1, max = 120)"` into `("length", [("min", "1"),
/// ("max", "120")])`. A bare rule such as `"email"` yields no arguments.
fn parse_rule(rule: &str) -> (&str, Vec<(&str, &str)>) {
    let rule = rule.trim();
    let Some(open) = rule.find('(') else {
        return (rule, Vec::new());
    };
    let name = rule[..open].trim();
    let body = rule[open + 1..].trim_end().trim_end_matches(')');
    let args = body
        .split(',')
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect();
    (name, args)
}

/// Reads one numeric rule argument. A non-numeric value is skipped rather
/// than coerced.
fn number_arg(args: &[(&str, &str)], key: &str) -> Option<Value> {
    let raw = args.iter().find(|(k, _)| *k == key)?.1;
    if let Ok(n) = raw.parse::<i64>() {
        return Some(json!(n));
    }
    raw.parse::<f64>().ok().map(|n| json!(n))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::uag::schema::UagQuery;

    fn field(name: &str, ty: &str, validates: &[&str]) -> UagField {
        UagField {
            name: name.to_owned(),
            ty: ty.to_owned(),
            validates: validates.iter().map(|v| (*v).to_owned()).collect(),
        }
    }

    fn route(method: &str, path: &str, name: &str) -> UagRoute {
        UagRoute {
            module: "Links".to_owned(),
            method: method.to_owned(),
            path: path.to_owned(),
            name: name.to_owned(),
            handler: "LinksController::index".to_owned(),
            params: crate::uag::from_graph::path_params(path),
            pages: BTreeSet::new(),
            action: None,
            query: None,
            query_string: None,
            policies: BTreeSet::new(),
        }
    }

    fn document(routes: Vec<UagRoute>) -> Value {
        let artifact = UagArtifact::new(BTreeMap::new(), routes, BTreeMap::new());
        generate(&artifact, &OpenApiOptions::default())
    }

    #[test]
    fn the_document_declares_openapi_three_one() {
        assert_eq!(document(Vec::new())["openapi"], json!("3.1.0"));
    }

    #[test]
    fn the_document_carries_no_timestamp() {
        let json = generate_json(
            &UagArtifact::new(BTreeMap::new(), Vec::new(), BTreeMap::new()),
            &OpenApiOptions::default(),
        )
        .unwrap();
        assert!(!json.contains("generated"), "{json}");
    }

    #[test]
    fn a_route_becomes_a_path_item_keyed_by_lowercase_method() {
        let doc = document(vec![route("GET", "/links", "links.index")]);
        assert_eq!(
            doc["paths"]["/links"]["get"]["operationId"],
            json!("links.index")
        );
    }

    #[test]
    fn a_path_parameter_becomes_a_required_path_parameter() {
        let doc = document(vec![route("GET", "/links/{link}", "links.show")]);
        assert_eq!(
            doc["paths"]["/links/{link}"]["get"]["parameters"][0],
            json!({
                "name": "link",
                "in": "path",
                "required": true,
                "schema": { "type": "string" },
            })
        );
    }

    #[test]
    fn a_wildcard_path_loses_its_axum_marker() {
        let doc = document(vec![route("GET", "/assets/{*rest}", "assets")]);
        assert!(doc["paths"]["/assets/{rest}"].is_object(), "{doc}");
    }

    #[test]
    fn an_action_becomes_a_request_body_referencing_a_component() {
        let mut r = route("POST", "/links", "links.store");
        r.action = Some(UagPayload {
            type_name: "StoreLinkRequest".to_owned(),
            fields: vec![
                field("url", "String", &["url"]),
                field("description", "Option<String>", &[]),
            ],
        });
        let doc = document(vec![r]);
        assert_eq!(
            doc["paths"]["/links"]["post"]["requestBody"]["content"]["application/json"]["schema"],
            json!({ "$ref": "#/components/schemas/StoreLinkRequest" })
        );
        let schema = &doc["components"]["schemas"]["StoreLinkRequest"];
        assert_eq!(schema["required"], json!(["url"]));
        assert_eq!(schema["properties"]["url"]["format"], json!("uri"));
        assert_eq!(
            schema["properties"]["description"],
            json!({ "anyOf": [{ "type": "string" }, { "type": "null" }] })
        );
    }

    #[test]
    fn a_length_rule_becomes_string_bounds() {
        let mut r = route("POST", "/links", "links.store");
        r.action = Some(UagPayload {
            type_name: "R".to_owned(),
            fields: vec![field("title", "String", &["length(min = 1, max = 120)"])],
        });
        let doc = document(vec![r]);
        let title = &doc["components"]["schemas"]["R"]["properties"]["title"];
        assert_eq!(title["minLength"], json!(1));
        assert_eq!(title["maxLength"], json!(120));
    }

    #[test]
    fn a_length_rule_on_a_list_becomes_item_bounds() {
        let mut r = route("POST", "/links", "links.store");
        r.action = Some(UagPayload {
            type_name: "R".to_owned(),
            fields: vec![field("tags", "Vec<String>", &["length(max=5)"])],
        });
        let doc = document(vec![r]);
        assert_eq!(
            doc["components"]["schemas"]["R"]["properties"]["tags"]["maxItems"],
            json!(5)
        );
    }

    #[test]
    fn a_range_rule_becomes_numeric_bounds() {
        let mut r = route("POST", "/links", "links.store");
        r.action = Some(UagPayload {
            type_name: "R".to_owned(),
            fields: vec![field("score", "i32", &["range(min = 0, max = 10)"])],
        });
        let doc = document(vec![r]);
        let score = &doc["components"]["schemas"]["R"]["properties"]["score"];
        assert_eq!(score["minimum"], json!(0));
        assert_eq!(score["maximum"], json!(10));
    }

    #[test]
    fn a_rule_with_no_json_schema_counterpart_is_left_out() {
        let mut r = route("POST", "/links", "links.store");
        r.action = Some(UagPayload {
            type_name: "R".to_owned(),
            fields: vec![field(
                "slug",
                "String",
                &["regex(SLUG_RE)", "custom(check)"],
            )],
        });
        let doc = document(vec![r]);
        assert_eq!(
            doc["components"]["schemas"]["R"]["properties"]["slug"],
            json!({ "type": "string" })
        );
    }

    #[test]
    fn a_query_route_describes_its_json_response() {
        let mut r = route("GET", "/links", "links.index");
        r.query = Some(UagQuery {
            type_name: "LinkResource".to_owned(),
            array: true,
            fields: vec![field("id", "i64", &[])],
        });
        let doc = document(vec![r]);
        assert_eq!(
            doc["paths"]["/links"]["get"]["responses"]["200"]["content"]["application/json"]["schema"],
            json!({
                "type": "array",
                "items": { "$ref": "#/components/schemas/LinkResource" },
            })
        );
    }

    #[test]
    fn a_query_string_field_becomes_a_query_parameter_not_a_component() {
        let mut r = route("GET", "/links", "links.index");
        r.query_string = Some(UagPayload {
            type_name: "LinkSearch".to_owned(),
            fields: vec![field("term", "Option<String>", &[])],
        });
        let doc = document(vec![r]);
        let param = &doc["paths"]["/links"]["get"]["parameters"][0];
        assert_eq!(param["in"], json!("query"));
        assert_eq!(param["required"], json!(false));
        assert!(doc["components"].is_null(), "{doc}");
    }

    #[test]
    fn a_page_route_describes_an_html_response() {
        let mut r = route("GET", "/", "home");
        r.pages = BTreeSet::from(["Home".to_owned()]);
        let doc = document(vec![r]);
        assert_eq!(
            doc["paths"]["/"]["get"]["responses"]["200"]["content"]["text/html"],
            json!({})
        );
    }

    #[test]
    fn a_route_with_no_known_response_shape_declares_none() {
        let doc = document(vec![route("DELETE", "/links/{link}", "links.destroy")]);
        assert!(
            doc["paths"]["/links/{link}"]["delete"]
                .get("responses")
                .is_none(),
            "an invented 200 would be worse than silence: {doc}"
        );
    }

    #[test]
    fn an_unnamed_route_gets_a_slug_operation_id() {
        let doc = document(vec![route("GET", "/links/{link}", "")]);
        assert_eq!(
            doc["paths"]["/links/{link}"]["get"]["operationId"],
            json!("get_links__link_")
        );
    }

    #[test]
    fn generating_twice_yields_byte_identical_output() {
        let artifact = UagArtifact::new(
            BTreeMap::new(),
            vec![route("GET", "/links", "links.index")],
            BTreeMap::new(),
        );
        let options = OpenApiOptions::default();
        assert_eq!(
            generate_json(&artifact, &options).unwrap(),
            generate_json(&artifact, &options).unwrap()
        );
    }
}
