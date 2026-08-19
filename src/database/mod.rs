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
pub mod query;
pub mod transaction;
pub mod migration;

pub use config::{DatabaseConfig, PoolConfig, SessionConfig};
pub use connection::Db;
pub use query::{delete, find_by_pk, insert, update, Query, QueryModel};
pub use transaction::Transaction;

// Re-export the certified SeaORM and SQLx crates so downstream code targets
// the pinned versions through Arcature.
pub use sea_orm;
pub use sea_orm_migration;
pub use sqlx;
