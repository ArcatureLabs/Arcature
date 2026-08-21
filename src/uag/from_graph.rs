//! Assembling the UAG from the metadata the DSL macros already emit.
//!
//! Nothing here invents a new metadata channel. Every fact in the artifact
//! is read out of one of:
//!
//! * [`ModuleDescriptor`](crate::dx::graph::ModuleDescriptor) -- imports,
//!   exports, controllers and their methods, services, policies, routes,
//!   listeners, jobs, commands, schedules, pages.
//! * [`RouteDescriptor`](crate::dx::route_metadata::RouteDescriptor) --
//!   method, path, name, handler, declared pages, and the action / query /
//!   query-string field shapes the `routes!` macro baked in.
//! * [`ContractArtifact`](crate::inertia::contracts::ContractArtifact) --
//!   the registered page prop schemas.
//!
//! The one derivation that happens here is the route -> page edge: when a
//! route declares no `page:`/`pages:`, the handler string
//! (`"Ctrl::method"`) is joined against the module's per-controller method
//! metadata and the method's `Page<T>` identity is used instead. That join
//! is why `ModuleDescriptor::controller_methods` is parallel to
//! `ModuleDescriptor::controllers`.

use std::collections::{BTreeMap, BTreeSet};

use crate::dx::application_graph::ApplicationGraph;
use crate::dx::controller_metadata::ControllerMethod;
use crate::dx::field_metadata::FieldShape;
use crate::dx::graph::ModuleDescriptor;
use crate::dx::route_metadata::RouteDescriptor;
use crate::inertia::contracts::ContractArtifact;

use super::schema::{
    UagArtifact, UagCadence, UagCommand, UagControllerMethod, UagField, UagJob, UagListener,
    UagModule, UagPayload, UagQuery, UagRoute, UagSchedule,
};

/// Build the artifact from a validated application graph and the registered
/// page contracts.
///
/// `contracts` is taken by reference rather than assembled here because the
/// registry enforces the Client Exposure Firewall at registration time; the
/// UAG only reads what already passed it. Pass
/// `PageContracts::new().artifact()` for an application with no pages.
#[must_use]
pub fn build(graph: &ApplicationGraph, contracts: &ContractArtifact) -> UagArtifact {
    let mut modules = BTreeMap::new();
    let mut routes = Vec::new();

    for descriptor in graph.modules() {
        let methods = controller_method_index(descriptor);
        for route in descriptor.routes {
            routes.push(route_from(descriptor.name, route, &methods));
        }
        modules.insert(descriptor.name.to_owned(), module_from(descriptor));
    }

    UagArtifact::new(modules, routes, contracts.pages().clone())
}

/// The path parameter names of an axum path pattern, in path order.
///
/// The wildcard marker is stripped (`{*rest}` yields `rest`) so a caller
/// naming the parameter in TypeScript writes the same identifier the Rust
/// extractor uses.
#[must_use]
pub fn path_params(path: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            break;
        };
        let name = after[..close].trim_start_matches('*');
        if !name.is_empty() {
            params.push(name.to_owned());
        }
        rest = &after[close + 1..];
    }
    params
}

/// Maps `"Controller"` -> its method metadata, for the route -> page join.
///
/// `controller_methods` is documented as parallel to `controllers`; `zip`
/// enforces that rather than trusting the index, so a descriptor built by
/// hand with mismatched lengths loses entries instead of panicking.
fn controller_method_index(
    descriptor: &ModuleDescriptor,
) -> BTreeMap<&'static str, &'static [ControllerMethod]> {
    descriptor
        .controllers
        .iter()
        .copied()
        .zip(descriptor.controller_methods.iter().copied())
        .collect()
}

/// Converts one module descriptor, without its routes.
fn module_from(descriptor: &ModuleDescriptor) -> UagModule {
    let controllers = descriptor
        .controllers
        .iter()
        .copied()
        .zip(descriptor.controller_methods.iter().copied())
        .map(|(name, methods)| {
            let methods = methods
                .iter()
                .map(|m| UagControllerMethod {
                    name: m.name.to_owned(),
                    params: m.params.iter().map(|p| (*p).to_owned()).collect(),
                    page: m.page.map(str::to_owned),
                })
                .collect();
            (name.to_owned(), methods)
        })
        .collect();

    UagModule {
        imports: owned_set(descriptor.imports),
        exports: owned_set(descriptor.exports),
        controllers,
        services: owned_set(descriptor.services),
        policies: owned_set(descriptor.policies),
        pages: owned_set(descriptor.pages),
        listeners: descriptor
            .listeners
            .iter()
            .map(|l| UagListener {
                event: l.event.to_owned(),
                listener: l.listener.to_owned(),
            })
            .collect(),
        jobs: descriptor
            .jobs
            .iter()
            .map(|j| UagJob {
                kind: j.kind.to_owned(),
                version: j.version,
                handler: j.handler.to_owned(),
            })
            .collect(),
        commands: descriptor
            .commands
            .iter()
            .map(|c| UagCommand {
                name: c.name.to_owned(),
                function: c.function.to_owned(),
            })
            .collect(),
        schedules: descriptor
            .schedules
            .iter()
            .map(|s| UagSchedule {
                job: s.job.to_owned(),
                version: s.version,
                cadence: match s.cadence {
                    crate::jobs::ScheduleCadence::Every { seconds } => {
                        UagCadence::Every { seconds }
                    }
                    crate::jobs::ScheduleCadence::Daily { hour, minute } => {
                        UagCadence::Daily { hour, minute }
                    }
                },
            })
            .collect(),
    }
}

/// Converts one route descriptor, resolving its page edge.
fn route_from(
    module: &'static str,
    route: &RouteDescriptor,
    methods: &BTreeMap<&'static str, &'static [ControllerMethod]>,
) -> UagRoute {
    let pages = if route.pages.is_empty() {
        inferred_page(route.handler, methods)
            .into_iter()
            .map(str::to_owned)
            .collect()
    } else {
        owned_set(route.pages)
    };

    UagRoute {
        module: module.to_owned(),
        method: route.method.as_str().to_owned(),
        path: route.path.to_owned(),
        name: route.name.to_owned(),
        handler: route.handler.to_owned(),
        params: path_params(route.path),
        pages,
        action: payload(route.action_type, route.action_fields),
        query: payload(route.query_type, route.query_fields).map(|p| UagQuery {
            type_name: p.type_name,
            array: route.query_array,
            fields: p.fields,
        }),
        query_string: payload(route.query_string_type, route.query_string_fields),
        policies: owned_set(route.policies),
    }
}

/// The page a handler renders, from its controller method's `Page<T>`
/// return type. `None` when the handler string is not `"Ctrl::method"`, the
/// controller is not in this module, or the method returns no page.
fn inferred_page(
    handler: &str,
    methods: &BTreeMap<&'static str, &'static [ControllerMethod]>,
) -> Option<&'static str> {
    let (controller, method) = handler.split_once("::")?;
    methods
        .get(controller)?
        .iter()
        .find(|m| m.name == method)?
        .page
}

/// A payload is present when the route named a type or carried field
/// shapes. Both are empty on a route that is neither an action nor a query.
fn payload(type_name: &str, fields: &[FieldShape]) -> Option<UagPayload> {
    if type_name.is_empty() && fields.is_empty() {
        return None;
    }
    Some(UagPayload {
        type_name: type_name.to_owned(),
        fields: fields
            .iter()
            .map(|f| UagField {
                name: f.name.to_owned(),
                ty: f.ty.to_owned(),
                validates: f.validates.iter().map(|v| (*v).to_owned()).collect(),
            })
            .collect(),
    })
}

/// Copies a `&'static [&'static str]` into an ordered owned set.
fn owned_set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dx::route_metadata::RouteMethod;

    const SHOW: RouteDescriptor = RouteDescriptor {
        method: RouteMethod::Get,
        path: "/links/{link}",
        name: "links.show",
        handler: "LinksController::show",
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
        handler: "LinksController::store",
        pages: &[],
        action_fields: &[
            FieldShape {
                name: "url",
                ty: "String",
                validates: &["url"],
            },
            FieldShape {
                name: "description",
                ty: "Option<String>",
                validates: &[],
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

    const METHODS: &[ControllerMethod] = &[
        ControllerMethod {
            name: "show",
            params: &["link"],
            page: Some("links/Show"),
        },
        ControllerMethod {
            name: "store",
            params: &["input"],
            page: None,
        },
    ];

    const LINKS: ModuleDescriptor = ModuleDescriptor {
        name: "Links",
        imports: &[],
        exports: &["LinksService"],
        controllers: &["LinksController"],
        controller_methods: &[METHODS],
        services: &["LinksService"],
        policies: &["LinkPolicy"],
        routes: &[SHOW, STORE],
        listeners: &[],
        jobs: &[],
        commands: &[],
        schedules: &[],
        pages: &["links/Show"],
    };

    fn built() -> UagArtifact {
        let graph = ApplicationGraph::new(vec![LINKS]).expect("the fixture graph is valid");
        build(&graph, &ContractArtifact::new(BTreeMap::new()))
    }

    #[test]
    fn every_module_route_reaches_the_artifact_once() {
        let artifact = built();
        assert_eq!(artifact.routes().len(), 2);
        assert!(artifact.routes().iter().all(|r| r.module == "Links"));
    }

    #[test]
    fn a_route_without_a_declared_page_inherits_the_handler_page() {
        let artifact = built();
        let show = artifact
            .routes()
            .iter()
            .find(|r| r.name == "links.show")
            .expect("the fixture declares links.show");
        assert_eq!(
            show.pages.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["links/Show"]
        );
    }

    #[test]
    fn a_handler_returning_no_page_leaves_the_route_pageless() {
        let artifact = built();
        let store = artifact
            .routes()
            .iter()
            .find(|r| r.name == "links.store")
            .expect("the fixture declares links.store");
        assert!(store.pages.is_empty());
    }

    #[test]
    fn a_declared_page_wins_over_the_inferred_one() {
        const DECLARED: RouteDescriptor = RouteDescriptor {
            method: RouteMethod::Get,
            path: "/links/{link}",
            name: "links.show",
            handler: "LinksController::show",
            pages: &["links/Custom"],
            action_fields: &[],
            action_type: "",
            query_fields: &[],
            query_type: "",
            query_array: false,
            query_string_fields: &[],
            query_string_type: "",
            policies: &[],
        };
        let mut module = LINKS;
        module.routes = &[DECLARED];
        let graph = ApplicationGraph::new(vec![module]).unwrap();
        let artifact = build(&graph, &ContractArtifact::new(BTreeMap::new()));
        assert!(artifact.routes()[0].pages.contains("links/Custom"));
    }

    #[test]
    fn action_fields_keep_their_declaration_order() {
        let artifact = built();
        let store = artifact
            .routes()
            .iter()
            .find(|r| r.name == "links.store")
            .expect("the fixture declares links.store");
        let action = store.action.as_ref().expect("links.store is an action");
        assert_eq!(action.type_name, "StoreLinkRequest");
        assert_eq!(
            action
                .fields
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            vec!["url", "description"]
        );
        assert_eq!(action.fields[0].validates, vec!["url".to_owned()]);
    }

    #[test]
    fn a_route_with_no_typed_payload_carries_none() {
        let artifact = built();
        let show = artifact
            .routes()
            .iter()
            .find(|r| r.name == "links.show")
            .expect("the fixture declares links.show");
        assert!(show.action.is_none());
        assert!(show.query.is_none());
        assert!(show.query_string.is_none());
    }

    #[test]
    fn path_parameters_come_out_in_path_order() {
        assert_eq!(path_params("/a/{one}/b/{two}"), vec!["one", "two"]);
    }

    #[test]
    fn a_wildcard_parameter_loses_its_marker() {
        assert_eq!(path_params("/assets/{*rest}"), vec!["rest"]);
    }

    #[test]
    fn a_path_without_parameters_yields_none() {
        assert!(path_params("/links").is_empty());
    }

    #[test]
    fn an_unclosed_brace_does_not_hang_or_panic() {
        assert!(path_params("/links/{link").is_empty());
    }

    #[test]
    fn module_metadata_is_carried_across() {
        let artifact = built();
        let module = &artifact.modules()["Links"];
        assert!(module.policies.contains("LinkPolicy"));
        assert!(module.services.contains("LinksService"));
        assert!(module.pages.contains("links/Show"));
        assert_eq!(module.controllers["LinksController"].len(), 2);
    }

    #[test]
    fn building_the_same_graph_twice_yields_the_same_bytes() {
        assert_eq!(
            built().to_json().unwrap(),
            built().to_json().unwrap(),
            "the artifact is a function of the source alone"
        );
    }
}
