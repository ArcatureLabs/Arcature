//! Resource types: the browser-facing projection of domain data.
//!
//! A resource is an explicit exposure boundary. Entities are never sent to a
//! client directly; a controller converts through a `#[resource]` type, so
//! adding a column to a table cannot widen a response by itself. Add one
//! with `arc make:resource`.
