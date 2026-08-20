//! Database: one PostgreSQL pool, two first-class paths (SeaORM + SQLx).
//!
//! A single `PgPool` is shared by SeaORM and SQLx via
//! `SqlxPostgresConnector::from_sqlx_postgres_pool`. No second pool, no global
//! registry, no thread-local. The [`Db`] handle is `Clone + Send + Sync +
//! 'static` so it works as Axum state.
//!
//! ```ignore
//! use arcature::prelude::*;
//!
//! pub async fn index(db: Db) -> Result<Response> {
//!     let users = User::query(&db).all().await?;
//!     inertia!("users/index", { users })
//! }
//! ```

pub mod config;
pub mod connection;
pub mod migration;
pub mod query;
pub mod transaction;

pub use config::{DatabaseConfig, PoolConfig, SessionConfig};
pub use connection::Db;
pub use query::{Query, QueryModel, delete, find_by_pk, insert, update};
pub use transaction::Transaction;

// Re-export the certified SeaORM and SQLx crates so downstream code targets
// the pinned versions through Arcature.
pub use sea_orm;
pub use sea_orm_migration;
pub use sqlx;

// Re-export the date/time and UUID crates pulled in by the `database`
// feature, so downstream models reference the pinned versions through
// Arcature (e.g. `arcature::database::chrono::DateTime`).
pub use chrono;
pub use uuid;

/// The marker trait for a SeaORM entity model.
///
/// The `#[model(table = "...")]` macro generates `impl Model for T` where
/// `type Entity = T::Entity`. This binds the user's struct to the
/// [`QueryModel`] query facade so `T::query(&db).where_eq(...).all()` works.
pub trait Model: sea_orm::EntityTrait {
    /// The SeaORM `Entity` for this model (always `<Self as EntityTrait>::Entity`).
    type Entity: sea_orm::EntityTrait<Model = Self>;
}
