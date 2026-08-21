//! End-to-end cover for the Unified Application Graph.
//!
//! The unit tests inside `src/uag` each pin one function. This file pins the
//! seam between them: a module graph and a contract registry go in, and an
//! artifact, a diagnostic list, and four generated files come out. A change
//! that keeps every unit test green but breaks the hand-off -- a route that
//! never reaches the route table, a page the validator cannot see -- fails
//! here.

#![cfg(feature = "uag")]

use std::collections::BTreeMap;
use std::fs;

use arcature::dx::application_graph::ApplicationGraph;
use arcature::dx::controller_metadata::ControllerMethod;
use arcature::dx::field_metadata::FieldShape;
use arcature::dx::graph::ModuleDescriptor;
use arcature::dx::route_metadata::{RouteDescriptor, RouteMethod};
use arcature::inertia::contracts::{ContractArtifact, ContractType, PageSchema, PropsSchema};
use arcature::uag::codegen::{forms_ts, index_ts, openapi, pages_dts, routes_ts};
use arcature::uag::{UagArtifact, UagDiagnostic, ValidateOptions, build, validate};

const INDEX: RouteDescriptor = RouteDescriptor {
    method: RouteMethod::Get,
    path: "/links",
    name: "links.index",
    handler: "LinkController::index",
    pages: &[],
    action_fields: &[],
    action_type: "",
    query_fields: &[],
    query_type: "",
    query_array: false,
    query_string_fields: &[FieldShape {
        name: "search",
        ty: "Option<String>",
        validates: &["length(max = 64)"],
    }],
    query_string_type: "LinkFilter",
    policies: &["LinkPolicy"],
};

const SHOW: RouteDescriptor = RouteDescriptor {
    method: RouteMethod::Get,
    path: "/links/{id}",
    name: "links.show",
    handler: "LinkController::show",
    pages: &[],
    action_fields: &[],
    action_type: "",
    query_fields: &[],
    query_type: "",
    query_array: false,
    query_string_fields: &[],
    query_string_type: "",
    policies: &[],
};

const STORE: RouteDescriptor = RouteDescriptor {
    method: RouteMethod::Post,
    path: "/links",
    name: "links.store",
    handler: "LinkController::store",
    pages: &[],
    action_fields: &[
        FieldShape {
            name: "url",
            ty: "String",
            validates: &["url", "length(min = 1, max = 2048)"],
        },
        FieldShape {
            name: "title",
            ty: "Option<String>",
            validates: &[],
        },
        FieldShape {
            name: "hits",
            ty: "u32",
            validates: &["range(min = 0, max = 1000)"],
        },
    ],
    action_type: "StoreLinkRequest",
    query_fields: &[],
    query_type: "",
    query_array: false,
    query_string_fields: &[],
    query_string_type: "",
    policies: &[],
};

/// The methods the `#[controller]` macro would emit for `LinkController`.
///
/// `index` and `show` return `Page<T>`, so they carry a page identity; the
/// route descriptors above declare no `pages:`, which is exactly the case
/// the inference join exists for.
const LINK_METHODS: &[ControllerMethod] = &[
    ControllerMethod {
        name: "index",
        params: &[],
        page: Some("links/Index"),
    },
    ControllerMethod {
        name: "show",
        params: &["id"],
        page: Some("links/Show"),
    },
    ControllerMethod {
        name: "store",
        params: &[],
        page: None,
    },
];

fn links_module() -> ModuleDescriptor {
    let mut module = ModuleDescriptor::new("Links");
    module.exports = &["LinkService"];
    module.controllers = &["LinkController"];
    module.controller_methods = &[LINK_METHODS];
    module.services = &["LinkService"];
    module.routes = &[INDEX, SHOW, STORE];
    module.pages = &["links/Index", "links/Show"];
    // `INDEX` is guarded by `LinkPolicy`, so the module has to declare it.
    // Without this the fixture is not the complete application it claims to
    // be, and `UndeclaredPolicy` fires -- correctly, because a route that
    // reads as guarded and names a policy nobody declares is not guarded.
    module.policies = &["LinkPolicy"];
    module
}

fn contracts() -> ContractArtifact {
    let mut pages = BTreeMap::new();
    pages.insert(
        "links/Index".to_owned(),
        PageSchema::new(
            PropsSchema::new().required("links", ContractType::array(ContractType::string())),
        ),
    );
    pages.insert(
        "links/Show".to_owned(),
        PageSchema::new(
            PropsSchema::new()
                .required("url", ContractType::string())
                .optional("title", ContractType::string()),
        ),
    );
    ContractArtifact::new(pages)
}

fn artifact() -> UagArtifact {
    let graph = ApplicationGraph::new(vec![links_module()]).expect("the module graph is valid");
    build(&graph, &contracts())
}

/// Lays the page components out on disk the way a Vite application does, so
/// the component check has something real to resolve against.
fn pages_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp directory is available");
    fs::create_dir_all(dir.path().join("links")).expect("the nested page directory is created");
    for page in ["links/Index", "links/Show"] {
        fs::write(
            dir.path().join(format!("{page}.tsx")),
            "export default {}\n",
        )
        .expect("the component file is written");
    }
    dir
}

#[test]
fn every_declared_route_reaches_the_artifact() {
    let artifact = artifact();
    let names: Vec<&str> = artifact.routes().iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["links.index", "links.store", "links.show"]);
}

#[test]
fn a_route_inherits_the_page_its_handler_returns() {
    let artifact = artifact();
    let show = artifact
        .routes()
        .iter()
        .find(|r| r.name == "links.show")
        .expect("the show route is present");
    assert!(show.pages.contains("links/Show"));
}

#[test]
fn a_complete_application_validates_clean() {
    let dir = pages_dir();
    let options = ValidateOptions::new().with_pages_dir(dir.path());
    assert_eq!(validate(&artifact(), &options), Ok(()));
}

#[test]
fn a_page_the_registry_never_saw_is_caught_before_codegen() {
    let artifact = build(
        &ApplicationGraph::new(vec![links_module()]).expect("the module graph is valid"),
        &ContractArtifact::new(BTreeMap::new()),
    );
    let diagnostics =
        validate(&artifact, &ValidateOptions::new()).expect_err("the pages are unregistered");
    assert!(diagnostics.iter().any(
        |d| matches!(d, UagDiagnostic::UnregisteredPage { page, .. } if page == "links/Show")
    ));
}

#[test]
fn a_page_with_no_component_on_disk_is_reported() {
    let dir = tempfile::tempdir().expect("a temp directory is available");
    let options = ValidateOptions::new().with_pages_dir(dir.path());
    let diagnostics = validate(&artifact(), &options).expect_err("no components exist");
    assert!(
        diagnostics
            .iter()
            .all(|d| matches!(d, UagDiagnostic::MissingPageComponent { .. }))
    );
    assert_eq!(diagnostics.len(), 2);
}

#[test]
fn the_route_table_names_every_named_route() {
    let generated = routes_ts::generate(&artifact());
    for name in ["links.index", "links.show", "links.store"] {
        assert!(
            generated.contains(&format!("\"{name}\"")),
            "{name} is missing from:\n{generated}"
        );
    }
}

#[test]
fn a_parameterised_route_carries_its_parameter_into_typescript() {
    let generated = routes_ts::generate(&artifact());
    assert!(generated.contains("path: \"/links/{id}\", params: [\"id\"]"));
}

#[test]
fn the_prop_types_follow_the_registered_contracts() {
    let generated = pages_dts::generate(&artifact());
    let expected = concat!(
        "  \"links/Show\": {\n",
        "    \"title\"?: string;\n",
        "    \"url\": string;\n",
        "  };\n",
    );
    assert!(
        generated.contains(expected),
        "unexpected page props in:\n{generated}"
    );
    assert!(generated.contains("\"links\": string[];"));
}

#[test]
fn the_form_fields_follow_the_request_struct() {
    let generated = forms_ts::generate(&artifact());
    assert!(generated.contains("request: \"StoreLinkRequest\""));
    assert!(generated.contains("\"url\": { type: \"string\", optional: false, rules: [\"url\", \"length(min = 1, max = 2048)\"] }"));
    assert!(generated.contains("\"title\": { type: \"string\", optional: true, rules: [] }"));
}

#[test]
fn no_generated_typescript_file_imports_anything() {
    let artifact = artifact();
    for generated in [
        routes_ts::generate(&artifact),
        pages_dts::generate(&artifact),
        forms_ts::generate(&artifact),
        // The barrel re-exports the other three. `export * from "./routes"`
        // is a relative specifier inside the same generated directory, which
        // is the one kind of module reference the no-dependency rule allows
        // -- and the test still holds it to writing no `import`.
        index_ts::generate(),
    ] {
        assert!(
            !generated.contains("import "),
            "found an import in:\n{generated}"
        );
        assert!(!generated.contains("require("));
    }
}

#[test]
fn the_openapi_document_describes_the_action_route() {
    let document = openapi::generate(&artifact(), &openapi::OpenApiOptions::default());
    let post = &document["paths"]["/links"]["post"];
    assert_eq!(post["operationId"], "links.store");
    assert_eq!(
        post["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/StoreLinkRequest"
    );
    let schema = &document["components"]["schemas"]["StoreLinkRequest"];
    assert_eq!(schema["properties"]["url"]["format"], "uri");
    assert_eq!(schema["properties"]["hits"]["maximum"], 1000);
    assert_eq!(schema["required"], serde_json::json!(["url", "hits"]));
}

#[test]
fn the_openapi_document_describes_the_path_parameter() {
    let document = openapi::generate(&artifact(), &openapi::OpenApiOptions::default());
    let parameters = &document["paths"]["/links/{id}"]["get"]["parameters"];
    assert_eq!(parameters[0]["name"], "id");
    assert_eq!(parameters[0]["in"], "path");
    assert_eq!(parameters[0]["required"], true);
}

#[test]
fn regenerating_the_whole_bundle_yields_byte_identical_output() {
    let first = artifact();
    let second = artifact();
    assert_eq!(first.to_json().unwrap(), second.to_json().unwrap());
    assert_eq!(routes_ts::generate(&first), routes_ts::generate(&second));
    assert_eq!(pages_dts::generate(&first), pages_dts::generate(&second));
    assert_eq!(forms_ts::generate(&first), forms_ts::generate(&second));
    assert_eq!(
        openapi::generate_json(&first, &openapi::OpenApiOptions::default()).unwrap(),
        openapi::generate_json(&second, &openapi::OpenApiOptions::default()).unwrap()
    );
}

#[test]
fn the_artifact_json_carries_nothing_specific_to_this_machine() {
    let json = String::from_utf8(artifact().to_json().unwrap()).expect("the artifact is utf-8");
    for marker in ["C:\\", "/home/", "/Users/", "generated_at", "timestamp"] {
        assert!(!json.contains(marker), "{marker} leaked into the artifact");
    }
}
