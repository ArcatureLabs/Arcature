//! The application's SeaORM models.
//!
//! Add one file per model here: `#[model(table = "...")]` prepends the SeaORM
//! derives and binds the struct to the query facade. Example:
//!
//! ```ignore
//! #[model(table = "users")]
//! pub struct User {
//!     #[sea_orm(primary_key)]
//!     pub id: i64,
//!     pub email: String,
//! }
//! ```
