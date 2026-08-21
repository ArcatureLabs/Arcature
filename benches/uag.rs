//! What rebuilding the Unified Application Graph costs.
//!
//! `arc dev` reassembles the UAG and rewrites the generated TypeScript after
//! every backend restart, inside a dev loop the plan holds to 2.5 seconds from
//! keystroke to browser. That makes this artifact a budget line: if assembling
//! and serialising it is measured in microseconds it is free against a Rust
//! rebuild, and if it is not, the dev loop has a second problem nobody is
//! looking at.
//!
//! One iteration is one whole pass over the artifact — assemble, serialise,
//! validate, or generate one file — so the reported time is seconds per
//! `arc typegen`-equivalent step, not per route.
//!
//! Two application sizes. `small` is roughly what the `arc new` scaffold emits;
//! `large` is twelve modules of eight routes, which is a real application and
//! the size at which an accidental quadratic would show.
//!
//! Nothing here touches the filesystem. Validation runs without a pages
//! directory, which skips the component-file check — that check is a `stat` per
//! page and would turn this into a disk benchmark.

use std::collections::BTreeMap;
use std::hint::black_box;

use arcature::dx::application_graph::ApplicationGraph;
use arcature::dx::controller_metadata::ControllerMethod;
use arcature::dx::field_metadata::FieldShape;
use arcature::dx::graph::ModuleDescriptor;
use arcature::dx::route_metadata::{RouteDescriptor, RouteMethod};
use arcature::inertia::contracts::{ContractArtifact, ContractType, PageSchema, PropsSchema};
use arcature::uag::codegen::{forms_ts, openapi, pages_dts, routes_ts};
use arcature::uag::{OpenApiOptions, UagArtifact, ValidateOptions, build, validate};
use criterion::{Criterion, criterion_group, criterion_main};

/// Routes per module in both sizes: the seven REST actions of a `resource`
/// plus one extra.
const ROUTES_PER_MODULE: usize = 8;

/// Modules in the `large` application.
const LARGE_MODULES: usize = 12;

/// Leak a `String` into the `&'static str` the metadata types are built from.
///
/// The DSL macros produce genuinely `'static` data because they emit literals.
/// A bench that generates the same shapes at run time has to buy that lifetime
/// somehow, and leaking a bounded, once-per-process set of strings is the
/// honest way to do it — the alternative is to measure an arena instead of the
/// thing under test.
fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

/// The field shapes a `store` action declares, as `#[request]` would.
fn action_fields(module: usize) -> &'static [FieldShape] {
    Box::leak(Box::new([
        FieldShape {
            name: "title",
            ty: "String",
            validates: &["length(min = 1, max = 200)"],
        },
        FieldShape {
            name: "body",
            ty: "String",
            validates: &["length(min = 1)"],
        },
        FieldShape {
            name: leak(format!("field_{module}")),
            ty: "Option<u32>",
            validates: &["range(min = 0, max = 100)"],
        },
    ]))
}

/// One module route table: the seven REST actions plus a search endpoint.
fn routes(module: usize) -> &'static [RouteDescriptor] {
    let collection = leak(format!("/m{module}/items"));
    let with_id = leak(format!("/m{module}/items/{{id}}"));
    let name = |action: &str| leak(format!("m{module}.{action}"));
    let handler = |action: &str| leak(format!("Module{module}Controller::{action}"));

    let blank = RouteDescriptor {
        method: RouteMethod::Get,
        path: collection,
        name: "",
        handler: "",
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
    let policy = leak(format!("Module{module}Policy"));

    let table = vec![
        RouteDescriptor {
            name: name("index"),
            handler: handler("index"),
            query_string_fields: Box::leak(Box::new([FieldShape {
                name: "search",
                ty: "Option<String>",
                validates: &["length(max = 64)"],
            }])),
            query_string_type: leak(format!("Module{module}Filter")),
            policies: Box::leak(Box::new([policy])),
            ..blank
        },
        RouteDescriptor {
            path: with_id,
            name: name("show"),
            handler: handler("show"),
            ..blank
        },
        RouteDescriptor {
            path: leak(format!("/m{module}/items/create")),
            name: name("create"),
            handler: handler("create"),
            ..blank
        },
        RouteDescriptor {
            path: leak(format!("/m{module}/items/{{id}}/edit")),
            name: name("edit"),
            handler: handler("edit"),
            ..blank
        },
        RouteDescriptor {
            method: RouteMethod::Post,
            name: name("store"),
            handler: handler("store"),
            action_fields: action_fields(module),
            action_type: leak(format!("Store{module}Request")),
            ..blank
        },
        RouteDescriptor {
            method: RouteMethod::Put,
            path: with_id,
            name: name("update"),
            handler: handler("update"),
            action_fields: action_fields(module),
            action_type: leak(format!("Update{module}Request")),
            ..blank
        },
        RouteDescriptor {
            method: RouteMethod::Delete,
            path: with_id,
            name: name("destroy"),
            handler: handler("destroy"),
            ..blank
        },
        RouteDescriptor {
            path: leak(format!("/m{module}/items/search")),
            name: name("search"),
            handler: handler("search"),
            ..blank
        },
    ];
    assert_eq!(table.len(), ROUTES_PER_MODULE);
    Box::leak(table.into_boxed_slice())
}

/// The controller metadata `#[controller]` would emit for the module, so the
/// route-to-page inference join has something to resolve against.
fn controller_methods(module: usize) -> &'static [&'static [ControllerMethod]] {
    let index_page = leak(format!("m{module}/Index"));
    let show_page = leak(format!("m{module}/Show"));
    let methods: &'static [ControllerMethod] = Box::leak(Box::new([
        ControllerMethod {
            name: "index",
            params: &[],
            page: Some(index_page),
        },
        ControllerMethod {
            name: "show",
            params: &["id"],
            page: Some(show_page),
        },
        ControllerMethod {
            name: "store",
            params: &[],
            page: None,
        },
    ]));
    Box::leak(Box::new([methods]))
}

/// One module of an application of `count` modules.
///
/// Each module after the first imports the one before it, which is the shape
/// that makes the cycle check do real work rather than exit on an empty edge
/// set.
fn module(index: usize) -> ModuleDescriptor {
    let mut descriptor = ModuleDescriptor::new(leak(format!("Module{index}")));
    descriptor.controllers = Box::leak(Box::new([leak(format!("Module{index}Controller"))]));
    descriptor.controller_methods = controller_methods(index);
    descriptor.services = Box::leak(Box::new([leak(format!("Module{index}Service"))]));
    descriptor.policies = Box::leak(Box::new([leak(format!("Module{index}Policy"))]));
    descriptor.routes = routes(index);
    descriptor.pages = Box::leak(Box::new([
        leak(format!("m{index}/Index")),
        leak(format!("m{index}/Show")),
    ]));
    if index > 0 {
        descriptor.imports = Box::leak(Box::new([leak(format!("Module{}", index - 1))]));
    }
    descriptor
}

/// The page contracts the registry would hold for `count` modules.
fn contracts(count: usize) -> ContractArtifact {
    let mut pages = BTreeMap::new();
    for index in 0..count {
        pages.insert(
            format!("m{index}/Index"),
            PageSchema::new(
                PropsSchema::new()
                    .required("items", ContractType::array(ContractType::string()))
                    .required("total", ContractType::number()),
            ),
        );
        pages.insert(
            format!("m{index}/Show"),
            PageSchema::new(
                PropsSchema::new()
                    .required("title", ContractType::string())
                    .optional("body", ContractType::string()),
            ),
        );
    }
    ContractArtifact::new(pages)
}

/// The module graph of an application of `count` modules.
fn graph(count: usize) -> ApplicationGraph {
    ApplicationGraph::new((0..count).map(module).collect())
        .expect("the generated module graph is acyclic and fully resolved")
}

fn uag(c: &mut Criterion) {
    let sizes = [("small", 1usize), ("large", LARGE_MODULES)];

    let mut group = c.benchmark_group("uag");
    for (label, count) in sizes {
        let graph = graph(count);
        let contracts = contracts(count);
        let artifact: UagArtifact = build(&graph, &contracts);
        let openapi_options = OpenApiOptions::default();
        let validate_options = ValidateOptions::new();

        group.bench_function(format!("build/{label}"), |b| {
            b.iter(|| black_box(build(&graph, &contracts)));
        });
        group.bench_function(format!("to-json/{label}"), |b| {
            b.iter(|| black_box(artifact.to_json()));
        });
        group.bench_function(format!("validate/{label}"), |b| {
            b.iter(|| black_box(validate(&artifact, &validate_options)));
        });
        group.bench_function(format!("codegen-routes-ts/{label}"), |b| {
            b.iter(|| black_box(routes_ts::generate(&artifact)));
        });
        group.bench_function(format!("codegen-pages-dts/{label}"), |b| {
            b.iter(|| black_box(pages_dts::generate(&artifact)));
        });
        group.bench_function(format!("codegen-forms-ts/{label}"), |b| {
            b.iter(|| black_box(forms_ts::generate(&artifact)));
        });
        group.bench_function(format!("codegen-openapi/{label}"), |b| {
            b.iter(|| black_box(openapi::generate(&artifact, &openapi_options)));
        });

        // The whole `arc typegen` step in one row: assemble the artifact and
        // produce all four outputs. This is the number the dev-loop budget
        // cares about; the rows above exist to say which part of it moved.
        group.bench_function(format!("typegen-pass/{label}"), |b| {
            b.iter(|| {
                let artifact = build(&graph, &contracts);
                black_box((
                    routes_ts::generate(&artifact),
                    pages_dts::generate(&artifact),
                    forms_ts::generate(&artifact),
                    openapi::generate(&artifact, &openapi_options),
                ))
            });
        });
    }
    group.finish();
}

criterion_group!(benches, uag);
criterion_main!(benches);
