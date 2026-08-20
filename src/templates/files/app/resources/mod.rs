//! The application's resources: DTOs returned to the frontend.
//!
//! A resource converts a model into the JSON shape the client sees, so the
//! database schema can evolve without breaking the API. One resource per
//! model; the controller maps models to resources before rendering.

pub mod user_resource;

pub use user_resource::UserResource;
