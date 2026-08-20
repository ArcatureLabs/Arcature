//! The migration registry.
//!
//! Add one `mod m<date>_<seq>_<name>;` line per migration file, in order, and
//! append `Box::new(m<...>::Migration)` to [`Migrator::migrations`]. The
//! `arc migrate` CLI runs these via the app's own binary. Example:
//!
//! ```ignore
//! mod m20260101_000001_create_users_table;
//!
//! impl MigratorTrait for Migrator {
//!     fn migrations() -> Vec<Box<dyn MigrationTrait>> {
//!         vec![Box::new(m20260101_000001_create_users_table::Migration)]
//!     }
//! }
//! ```

use arcature::database::sea_orm_migration::MigratorTrait;

/// The schema migrator. The CLI runs `Migrator::up(db, None)` to apply pending
/// migrations and `Migrator::down(db, Some(n))` to roll back.
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn arcature::database::sea_orm_migration::MigrationTrait>> {
        // Add migrations here as you create them.
        vec![]
    }
}
