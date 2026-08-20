//! The application's SeaORM models.
//!
//! Each model is one file: `#[model(table = "...")]` prepends the SeaORM
//! derives and binds the struct to the query facade.

pub mod user;

pub use user::User;
