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
