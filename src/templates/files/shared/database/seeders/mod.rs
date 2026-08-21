//! Database seeders: the data a fresh database needs to be usable.
//!
//! [`run`] is what `__RUST_NAME__ --db-seed` calls. It is empty by design --
//! seed data is application-specific, and a scaffold that invented a demo
//! user would put an account with a known password in every database built
//! from it.
//!
//! A seeder is an `async fn(&Db) -> Result<()>`; call each one from [`run`]
//! in dependency order. Write them to be idempotent (insert-or-ignore rather
//! than insert), because `--db-seed` is run more than once against the same
//! database far more often than it is run against an empty one.

use arcature::prelude::*;

/// Run every seeder in order.
///
/// # Errors
///
/// Returns the first seeder failure, leaving the rest unrun.
pub async fn run(db: &Db) -> Result<()> {
    // Seeding nothing must still prove the connection is usable, or an
    // unreachable database would report success.
    db.ping().await?;
    Ok(())
}
