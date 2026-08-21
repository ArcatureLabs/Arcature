//! The Unified Application Graph.
//!
//! One deterministic artifact describing the whole application -- modules,
//! routes, controllers, pages, request shapes, jobs, commands -- assembled
//! from the `&'static` metadata the DSL macros already emit. Everything
//! downstream reads the UAG rather than re-deriving the same facts: `arc
//! routes`, `arc typegen`, the OpenAPI document, and the cross-stack
//! validation that catches a route renamed in Rust but not in TypeScript.
//!
//! Nothing on the request path reads this module. It exists for tooling.
//!
//! # Layout
//!
//! * [`schema`] -- the artifact types, and the determinism rules they keep.
//! * [`from_graph`] -- assembling the artifact from the `&'static`
//!   metadata; no new metadata channel is introduced.
//! * [`validate`] -- the cross-stack checks, as typed diagnostics that the
//!   caller formats.
//! * [`codegen`] -- the TypeScript and OpenAPI generators, each a pure
//!   function from the artifact to a string.

pub mod codegen;
pub mod from_graph;
pub mod schema;
pub mod validate;

pub use codegen::openapi::OpenApiOptions;
pub use codegen::type_map::TypeShape;
pub use from_graph::{build, path_params};
pub use schema::{
    SCHEMA_VERSION, UagArtifact, UagCadence, UagCommand, UagControllerMethod, UagField, UagJob,
    UagListener, UagModule, UagPayload, UagQuery, UagRoute, UagSchedule,
};
pub use validate::{PAGE_EXTENSIONS, UagDiagnostic, ValidateOptions, validate};
