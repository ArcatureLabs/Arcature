//! The database layer: SeaORM entities live under `app/models`; this module
//! owns the schema migrations run by `arc migrate`.
//!
//! The [`Migrator`](migrations::Migrator) is the single
//! `MigratorTrait` impl the CLI runs. Add new migration files under
//! `migrations/` and append them to `Migrator::migrations()`.

pub mod migrations;

pub use migrations::Migrator;
