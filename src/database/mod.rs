//! Database: one pool, two first-class paths (SeaORM + SQLx).
//!
//! A single `sqlx::Pool` is shared by SeaORM and SQLx via the matching
//! `Sqlx*Connector`. No second pool, no global registry, no thread-local.
//! The [`Db`] handle is `Clone + Send + Sync + 'static` so it works as Axum
//! state.
//!
//! The driver is chosen at compile time by exactly one of the `db-postgres`
//! / `db-sqlite` / `db-mysql` features; [`Driver`] is the resulting SQLx
//! database type and every driver-shaped item in the crate is written
//! against it rather than against `Postgres`.
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

// ---------------------------------------------------------------------------
// The compile-time driver selection.
// ---------------------------------------------------------------------------

// A build speaks exactly one dialect. Two drivers at once would make `Driver`
// ambiguous and, worse, would let a queue built for one dialect be pointed at
// another; refusing here is cheaper than a runtime surprise.
#[cfg(not(any(feature = "db-postgres", feature = "db-sqlite", feature = "db-mysql")))]
compile_error!(
    "the `database` feature needs a driver: enable exactly one of `db-postgres`, `db-sqlite`, `db-mysql`"
);
#[cfg(any(
    all(feature = "db-postgres", feature = "db-sqlite"),
    all(feature = "db-postgres", feature = "db-mysql"),
    all(feature = "db-sqlite", feature = "db-mysql"),
))]
compile_error!(concat!(
    "more than one database driver is enabled: `db-postgres`, `db-sqlite` and `db-mysql` are ",
    "mutually exclusive. If you did not ask for two, you ran `--all-features`, which turns on ",
    "all three -- name the drivers you want instead, or use the `fullstack` alias. For the ",
    "feature matrix: `cargo hack ... --exclude-all-features --exclude-features database`.",
));

/// The SQLx database this build speaks, selected by the `db-*` features.
#[cfg(feature = "db-postgres")]
pub type Driver = sqlx::Postgres;
/// The SQLx database this build speaks, selected by the `db-*` features.
#[cfg(feature = "db-sqlite")]
pub type Driver = sqlx::Sqlite;
/// The SQLx database this build speaks, selected by the `db-*` features.
#[cfg(feature = "db-mysql")]
pub type Driver = sqlx::MySql;

/// The connection pool type for [`Driver`].
pub type Pool = sqlx::Pool<Driver>;

/// The connect-options type for [`Driver`] (what a database URL parses into).
pub type ConnectOptions = <<Driver as sqlx::Database>::Connection as sqlx::Connection>::Options;

/// A single connection to [`Driver`].
///
/// This is what `&mut *transaction` derefs to, so it is the argument type for
/// anything that takes "a connection to run a statement on" without caring
/// whether that connection came from a pool or a transaction.
pub type Connection = <Driver as sqlx::Database>::Connection;

// Re-export the date/time and UUID crates pulled in by the `database`
// feature, so downstream models reference the pinned versions through
// Arcature (e.g. `arcature::database::chrono::DateTime`).
pub use chrono;
pub use uuid;

/// The query facade, hung on the row type.
///
/// SeaORM splits an entity in two: the `Model` struct holds one row's data,
/// and a separate `Entity` type carries the schema. Queries hang off
/// `Entity`, so the natural spelling would be `UserEntity::query(&db)` --
/// naming a type the application otherwise never mentions.
///
/// This trait moves that entry point onto the row type, so a query reads
/// `User::query(&db)`. It is blanket-implemented for every SeaORM model, so
/// nothing opts in and `#[model]` generates no impl for it.
///
/// # Example
///
/// ```ignore
/// #[model(table = "users")]
/// pub struct User {
///     #[sea_orm(primary_key)]
///     pub id: i64,
///     pub email: String,
/// }
///
/// let admins = User::query(&db)
///     .where_eq(UserColumn::Role, "admin")
///     .all()
///     .await?;
/// ```
pub trait Model: sea_orm::ModelTrait {
    /// Start a typed query over this model's table, bound to `db`.
    #[must_use]
    fn query(db: &Db) -> query::Query<'_, <Self as sea_orm::ModelTrait>::Entity> {
        query::Query::new(db)
    }
}

impl<M: sea_orm::ModelTrait> Model for M {}
