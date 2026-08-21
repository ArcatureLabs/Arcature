//! The database layer.
//!
//! SeaORM entities live under `app/models`; this module owns the schema
//! migrations `arc migrate` runs and the seeders `--db-seed` runs.
//!
//! The [`Migrator`](migrations::Migrator) is the single `MigratorTrait` impl.
//! Add new migration files under `migrations/` and append them to
//! `Migrator::migrations()`.

pub mod migrations;
pub mod seeders;

pub use migrations::Migrator;
