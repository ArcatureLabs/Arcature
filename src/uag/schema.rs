//! The UAG artifact types.
//!
//! Every type here is owned data (`String`, `Vec`, `BTreeMap`, `BTreeSet`)
//! rather than the `&'static` metadata it is built from, because the
//! artifact has to survive serialization and be read back by tooling that
//! never linked the application.
//!
//! Three properties are load-bearing and are the reason the shapes look the
//! way they do:
//!
//! * **No timestamps and no absolute paths.** The artifact is a function of
//!   the source alone, so committing it and diffing it in CI shows a change
//!   in the application rather than a change in when or where it was built.
//! * **Ordered containers.** Sets and maps are `BTreeSet`/`BTreeMap` and the
//!   route list is sorted, so two builds of unchanged code produce
//!   byte-identical JSON.
//! * **A schema version.** [`SCHEMA_VERSION`] is bumped when the shape
//!   changes, so a reader can refuse an artifact it does not understand
//!   instead of silently misreading it.
//!
//! The file follows the shape of
//! [`ContractArtifact`](crate::inertia::contracts::ContractArtifact): a
//! `FORMAT` const, private fields with getters, and a `to_json` that writes
//! deterministic pretty JSON.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::inertia::contracts::PageSchema;

/// The UAG schema version.
///
/// Bumped whenever the artifact shape changes in a way a reader could
/// misinterpret. Readers compare against
/// [`UagArtifact::schema_version`] and refuse what they do not know.
pub const SCHEMA_VERSION: u32 = 1;

/// The whole application, as one deterministic document.
///
/// Built by [`from_graph::build`](super::from_graph::build) from the
/// `&'static` metadata the DSL macros emit. Nothing on the request path
/// reads it -- `arc routes`, `arc typegen`, the OpenAPI document and the
/// cross-stack validation do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UagArtifact {
    format: String,
    schema_version: u32,
    modules: BTreeMap<String, UagModule>,
    routes: Vec<UagRoute>,
    pages: BTreeMap<String, PageSchema>,
}

impl UagArtifact {
    /// The stable artifact format identifier.
    pub const FORMAT: &'static str = "arcature.uag.v1";

    /// Assemble an artifact from its already-ordered parts.
    ///
    /// `routes` is sorted here rather than at the call site so the ordering
    /// rule lives with the type that promises determinism.
    #[must_use]
    pub fn new(
        modules: BTreeMap<String, UagModule>,
        mut routes: Vec<UagRoute>,
        pages: BTreeMap<String, PageSchema>,
    ) -> Self {
        routes.sort_by(|a, b| {
            (&a.path, &a.method, &a.name, &a.handler)
                .cmp(&(&b.path, &b.method, &b.name, &b.handler))
        });
        Self {
            format: Self::FORMAT.to_owned(),
            schema_version: SCHEMA_VERSION,
            modules,
            routes,
            pages,
        }
    }

    /// The artifact format identifier this artifact was written with.
    #[must_use]
    pub fn format(&self) -> &str {
        &self.format
    }

    /// The schema version this artifact was written with.
    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// The modules, ordered by module name.
    #[must_use]
    pub fn modules(&self) -> &BTreeMap<String, UagModule> {
        &self.modules
    }

    /// Every route in the application, ordered by path then method.
    ///
    /// Duplicates are kept rather than collapsed: a path+method declared
    /// twice is what [`validate`](super::validate) reports, so the artifact
    /// must still carry both.
    #[must_use]
    pub fn routes(&self) -> &[UagRoute] {
        &self.routes
    }

    /// The registered page contracts, ordered by page identity.
    #[must_use]
    pub fn pages(&self) -> &BTreeMap<String, PageSchema> {
        &self.pages
    }

    /// Serialize this artifact deterministically as pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns the `serde_json` error if serialization fails.
    pub fn to_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec_pretty(self)
    }
}

/// One feature module's contribution to the application.
///
/// The module's routes are not repeated here -- they live in
/// [`UagArtifact::routes`] with a [`UagRoute::module`] back-reference, so a
/// route appears exactly once in the document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UagModule {
    /// Modules this module imports, by name.
    pub imports: BTreeSet<String>,
    /// Capabilities this module exports, by name.
    pub exports: BTreeSet<String>,
    /// Controllers registered in this module, with their method metadata.
    pub controllers: BTreeMap<String, Vec<UagControllerMethod>>,
    /// Service type names registered in this module.
    pub services: BTreeSet<String>,
    /// Policy type names declared in this module.
    pub policies: BTreeSet<String>,
    /// Frontend page identities owned by this module.
    pub pages: BTreeSet<String>,
    /// Event -> listener bindings declared in this module.
    pub listeners: Vec<UagListener>,
    /// Job handler bindings declared in this module.
    pub jobs: Vec<UagJob>,
    /// Application command bindings declared in this module.
    pub commands: Vec<UagCommand>,
    /// Recurring schedule bindings declared in this module.
    pub schedules: Vec<UagSchedule>,
}

/// One controller method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UagControllerMethod {
    /// The method name (e.g. `"show"`).
    pub name: String,
    /// The parameter names, in declaration order.
    pub params: Vec<String>,
    /// The page identity the method's `Page<T>` return type renders, if it
    /// returns one.
    pub page: Option<String>,
}

/// An event -> listener binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UagListener {
    /// The event type name.
    pub event: String,
    /// The listener function name.
    pub listener: String,
}

/// A job kind bound to a handler function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UagJob {
    /// The job kind string.
    pub kind: String,
    /// The job payload schema version.
    pub version: i16,
    /// The handler function name.
    pub handler: String,
}

/// An application command bound to a function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UagCommand {
    /// The command name (e.g. `"users:prune"`).
    pub name: String,
    /// The command function name.
    pub function: String,
}

/// A job scheduled to run on a cadence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UagSchedule {
    /// The job kind string.
    pub job: String,
    /// The job payload schema version.
    pub version: i16,
    /// When the job fires.
    pub cadence: UagCadence,
}

/// How often a scheduled job fires.
///
/// Mirrors [`ScheduleCadence`](crate::jobs::ScheduleCadence) as owned data.
/// The tag names match, so the two serialize identically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UagCadence {
    /// Every `seconds` seconds.
    Every {
        /// The interval in seconds.
        seconds: u64,
    },
    /// Once a day at the given UTC time.
    Daily {
        /// The hour, 0-23.
        hour: u8,
        /// The minute, 0-59.
        minute: u8,
    },
}

/// One route, with everything the codegen and the OpenAPI document need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UagRoute {
    /// The module that declared this route.
    pub module: String,
    /// The uppercase HTTP method (e.g. `"GET"`).
    pub method: String,
    /// The full path pattern, group prefixes already applied (e.g.
    /// `"/links/{link}"`).
    pub path: String,
    /// The dotted route name (e.g. `"links.show"`), or empty for an unnamed
    /// route. Only named routes reach the generated TypeScript, because an
    /// unnamed route has nothing to be referenced by.
    pub name: String,
    /// The handler path (e.g. `"LinksController::show"`).
    pub handler: String,
    /// The path parameter names, in path order, with the axum wildcard
    /// marker stripped (`{*rest}` is recorded as `rest`).
    pub params: Vec<String>,
    /// The page identities this route renders. Either declared on the route
    /// via `page:`/`pages:`, or inferred from the handler's controller
    /// method return type when the route declared none.
    pub pages: BTreeSet<String>,
    /// The typed request body, for an Action route.
    pub action: Option<UagPayload>,
    /// The typed response, for a Query route.
    pub query: Option<UagQuery>,
    /// The typed query string, for a Query route that declares one.
    pub query_string: Option<UagPayload>,
    /// The policies that guard this route, by type name, as declared with
    /// the `policy:`/`policies:` route option. Sorted rather than kept in
    /// declaration order: a policy set has no meaningful order, and sorting
    /// is what keeps the artifact diffable.
    ///
    /// A declaration, not enforcement -- see
    /// [`RouteDescriptor::policies`](crate::dx::RouteDescriptor::policies).
    pub policies: BTreeSet<String>,
}

/// A named group of fields crossing the wire in one direction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UagPayload {
    /// The Rust type name (e.g. `"StoreLinkRequest"`), or empty when the
    /// route carried field shapes without naming their type.
    pub type_name: String,
    /// The fields, in declaration order. Declaration order is kept rather
    /// than sorted because a form renders in the order the struct declares.
    pub fields: Vec<UagField>,
}

/// A Query route's typed response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UagQuery {
    /// The response element type name (e.g. `"LinkResource"`).
    pub type_name: String,
    /// Whether the response is a collection of the element type.
    pub array: bool,
    /// The element's fields, in declaration order.
    pub fields: Vec<UagField>,
}

/// One field of a request input or a resource output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UagField {
    /// The field name.
    pub name: String,
    /// The Rust type as a clean string (e.g. `"Option<String>"`). The
    /// Rust -> TypeScript mapping is applied by
    /// [`codegen::type_map`](super::codegen::type_map), not stored here, so
    /// the artifact stays a faithful record of the Rust side.
    pub ty: String,
    /// The `#[validate(...)]` rule strings, in source order.
    pub validates: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inertia::contracts::{ContractType, PropsSchema};

    fn route(path: &str, method: &str) -> UagRoute {
        UagRoute {
            module: "Links".to_owned(),
            method: method.to_owned(),
            path: path.to_owned(),
            name: String::new(),
            handler: "LinksController::index".to_owned(),
            params: Vec::new(),
            pages: BTreeSet::new(),
            action: None,
            query: None,
            query_string: None,
            policies: BTreeSet::new(),
        }
    }

    fn artifact() -> UagArtifact {
        let mut pages = BTreeMap::new();
        pages.insert(
            "Home".to_owned(),
            PageSchema::new(PropsSchema::new().required("name", ContractType::string())),
        );
        UagArtifact::new(
            BTreeMap::from([("Links".to_owned(), UagModule::default())]),
            vec![route("/links/{link}", "GET"), route("/links", "GET")],
            pages,
        )
    }

    #[test]
    fn carries_the_stable_format_identifier_and_schema_version() {
        assert_eq!(artifact().format(), UagArtifact::FORMAT);
        assert_eq!(artifact().schema_version(), SCHEMA_VERSION);
    }

    #[test]
    fn routes_are_ordered_by_path_regardless_of_insertion_order() {
        let artifact = artifact();
        let paths: Vec<&str> = artifact.routes().iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec!["/links", "/links/{link}"]);
    }

    #[test]
    fn a_duplicate_path_and_method_survives_into_the_artifact() {
        let both = UagArtifact::new(
            BTreeMap::new(),
            vec![route("/links", "GET"), route("/links", "GET")],
            BTreeMap::new(),
        );
        assert_eq!(both.routes().len(), 2, "validate reports it, so keep both");
    }

    #[test]
    fn json_is_deterministic() {
        assert_eq!(artifact().to_json().unwrap(), artifact().to_json().unwrap());
    }

    #[test]
    fn json_carries_no_timestamp_and_no_absolute_path() {
        let json = String::from_utf8(artifact().to_json().unwrap()).unwrap();
        assert!(!json.contains("C:\\"), "{json}");
        assert!(!json.contains("generated_at"), "{json}");
    }

    #[test]
    fn round_trips_through_json() {
        let json = artifact().to_json().unwrap();
        let parsed: UagArtifact = serde_json::from_slice(&json).unwrap();
        assert_eq!(parsed, artifact());
    }

    #[test]
    fn the_cadence_tag_matches_the_runtime_cadence() {
        let uag = serde_json::to_string(&UagCadence::Every { seconds: 300 }).unwrap();
        let runtime =
            serde_json::to_string(&crate::jobs::ScheduleCadence::Every { seconds: 300 }).unwrap();
        assert_eq!(uag, runtime);
    }
}
