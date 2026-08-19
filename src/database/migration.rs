//! Migrations: the single schema-migration authority (sea-orm-migration).
//!
//! Arcature does not own a second migration engine. The application defines
//! its migrations in `database/migrations/src/lib.rs` as a `MigratorTrait`
//! impl, and the CLI runs them via `arc db migrate`.

use sea_orm_migration::MigratorTrait;

/// Run all pending migrations.
pub async fn up<Schema>(db: &super::connection::Db) -> Result<(), crate::Error>
where
    Schema: MigratorTrait,
{
    Schema::up(db.orm(), None)
        .await
        .map_err(|e| crate::Error::Database(e.to_string()))?;
    Ok(())
}

/// Roll back the last `n` migrations.
pub async fn down<Schema>(db: &super::connection::Db, steps: u32) -> Result<(), crate::Error>
where
    Schema: MigratorTrait,
{
    Schema::down(db.orm(), Some(steps))
        .await
        .map_err(|e| crate::Error::Database(e.to_string()))?;
    Ok(())
}

/// Fresh: drop everything and re-run all migrations.
pub async fn fresh<Schema>(db: &super::connection::Db) -> Result<(), crate::Error>
where
    Schema: MigratorTrait,
{
    Schema::fresh(db.orm())
        .await
        .map_err(|e| crate::Error::Database(e.to_string()))?;
    Ok(())
}

/// Migration status.
pub async fn status<Schema>(db: &super::connection::Db) -> Result<Vec<MigrationStatus>, crate::Error>
where
    Schema: MigratorTrait,
{
    let items = Schema::get_migration_with_status(db.orm())
        .await
        .map_err(|e| crate::Error::Database(e.to_string()))?;
    Ok(items
        .into_iter()
        .map(|m| MigrationStatus {
            name: m.name().to_string(),
            applied: matches!(m.status(), sea_orm_migration::MigrationStatus::Applied),
        })
        .collect())
}

/// The status of one migration.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MigrationStatus {
    pub name: String,
    pub applied: bool,
}
