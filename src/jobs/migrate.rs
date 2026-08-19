//! Jobs schema migrations.
//!
//! Two embedded migrations applied under a session-level advisory lock, with
//! a `arcature_jobs_schema_migrations` history table. The migrations are
//! idempotent: re-running `apply` is a no-op if already applied.

use sqlx::Executor;
use sqlx::Row;
use sqlx::postgres::PgConnection;

use super::error::MigrateError;

/// The first migration: creates the `arcature_jobs` table and its indexes.
const MIGRATION_0001: &str = include_str!("migrations/0001_jobs.sql");

/// The second migration: adds the `claim_token` column.
const MIGRATION_0002: &str = include_str!("migrations/0002_claim_token.sql");

/// All migrations in order. Each entry is `(version, sql)` where both are
/// `&'static str` (the SQL via `include_str!`), so they pass the
/// `SqlSafeStr` gate.
const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_jobs", MIGRATION_0001),
    ("0002_claim_token", MIGRATION_0002),
];

/// Create the history table if it does not exist.
const CREATE_HISTORY_SQL: &str = r#"CREATE TABLE IF NOT EXISTS arcature_jobs_schema_migrations (
    version     TEXT PRIMARY KEY,
    applied_at  TIMESTAMPTZ NOT NULL DEFAULT now()
)"#;

/// Check whether a migration version is already applied. The table name is a
/// const so this is a literal static string (injection-safe).
const CHECK_APPLIED_SQL: &str =
    "SELECT EXISTS(SELECT 1 FROM arcature_jobs_schema_migrations WHERE version = $1)";

/// Record a migration version as applied.
const RECORD_APPLIED_SQL: &str =
    "INSERT INTO arcature_jobs_schema_migrations (version) VALUES ($1)";

/// The advisory lock key (a stable, framework-specific int pair).
const ADVISORY_LOCK_KEY: i64 = 71420001;

/// Apply all pending migrations over the pool.
pub async fn apply(pool: &sqlx::PgPool) -> Result<(), MigrateError> {
    pool.execute(CREATE_HISTORY_SQL).await?;

    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(ADVISORY_LOCK_KEY)
        .execute(pool)
        .await?;
    let result = apply_pending(pool).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(ADVISORY_LOCK_KEY)
        .execute(pool)
        .await;
    result
}

async fn apply_pending(pool: &sqlx::PgPool) -> Result<(), MigrateError> {
    for &(version, sql) in MIGRATIONS {
        let already: bool = sqlx::query(CHECK_APPLIED_SQL)
            .bind(version)
            .fetch_one(pool)
            .await?
            .try_get::<bool, _>(0)?;

        if already {
            continue;
        }

        pool.execute(sql).await?;
        sqlx::query(RECORD_APPLIED_SQL)
            .bind(version)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Apply migrations within a caller's transaction.
pub async fn apply_tx(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<(), MigrateError> {
    let conn: &mut PgConnection = &mut **tx;
    sqlx::Executor::execute(&mut *conn, CREATE_HISTORY_SQL).await?;

    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(ADVISORY_LOCK_KEY)
        .execute(&mut *conn)
        .await?;
    let result = apply_pending_conn(&mut *conn).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(ADVISORY_LOCK_KEY)
        .execute(&mut *conn)
        .await;
    result
}

async fn apply_pending_conn(conn: &mut PgConnection) -> Result<(), MigrateError> {
    for &(version, sql) in MIGRATIONS {
        let already: bool = sqlx::query(CHECK_APPLIED_SQL)
            .bind(version)
            .fetch_one(&mut *conn)
            .await?
            .try_get::<bool, _>(0)?;

        if already {
            continue;
        }

        sqlx::Executor::execute(&mut *conn, sql).await?;
        sqlx::query(RECORD_APPLIED_SQL)
            .bind(version)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}
