//! Jobs schema migrations.
//!
//! One embedded migration per dialect, applied under a lock where the dialect
//! has one, with an `arcature_jobs_schema_migrations` history table. Applying
//! twice is a no-op.
//!
//! The migration text is not handed to the driver as one blob. MySQL rejects
//! multiple statements in a single prepared query unless the connection opted
//! in, so every dialect's file is split on a `--;;` sentinel and executed one
//! statement at a time. That keeps one code path instead of one per driver.

use sqlx::{Executor, Row};

use super::dialect::{JobDb, JobPool, sql};
use super::error::MigrateError;

/// The connection type of the dialect this build speaks.
type Conn = <JobDb as sqlx::Database>::Connection;

/// The statement separator inside a migration file. A bare `;` cannot be used:
/// it also ends statements *inside* a `CREATE TABLE` body and inside function
/// bodies, so the files mark their real boundaries explicitly.
const STATEMENT_SEPARATOR: &str = "--;;";

/// All migrations in order, as `(version, sql)`.
///
/// The SQL arrives via `include_str!`, so both halves are `&'static str` and
/// pass SQLx's `SqlSafeStr` gate without an escape hatch.
const MIGRATIONS: &[(&str, &str)] = &[("0001_jobs", sql::SCHEMA)];

/// Apply all pending migrations over the pool.
///
/// # Errors
///
/// Returns [`MigrateError`] if the database rejects a statement or the
/// connection fails.
pub async fn apply(pool: &JobPool) -> Result<(), MigrateError> {
    // One connection for the whole run, not one per statement. PostgreSQL's
    // `pg_advisory_lock` and MySQL's `GET_LOCK` are held by the *session*, so
    // a lock taken on a pooled connection protects nothing if the following
    // DDL goes out on a different one.
    let mut conn = pool.acquire().await?;
    apply_on(&mut conn).await
}

/// Apply migrations within a caller's transaction.
///
/// # Errors
///
/// Returns [`MigrateError`] if the database rejects a statement.
pub async fn apply_tx(tx: &mut sqlx::Transaction<'_, JobDb>) -> Result<(), MigrateError> {
    apply_on(&mut *tx).await
}

/// Apply migrations over one connection, taking the dialect's lock around
/// them if it has one.
async fn apply_on(conn: &mut Conn) -> Result<(), MigrateError> {
    conn.execute(sql::CREATE_HISTORY).await?;

    let Some(lock) = sql::LOCK else {
        // SQLite has no advisory lock and needs none: it serialises writers
        // itself, and every statement in the migration is `IF NOT EXISTS`.
        return apply_pending(conn).await;
    };

    sqlx::query(lock).execute(&mut *conn).await?;
    let result = apply_pending(&mut *conn).await;
    // The unlock is best-effort on purpose. If it fails the session is already
    // broken, and reporting that instead of the migration's own error would
    // hide the reason the caller actually needs.
    if let Some(unlock) = sql::UNLOCK {
        let _ = sqlx::query(unlock).execute(&mut *conn).await;
    }
    result
}

/// Run every migration that the history table does not already record.
async fn apply_pending(conn: &mut Conn) -> Result<(), MigrateError> {
    for &(version, migration) in MIGRATIONS {
        // A count rather than `SELECT EXISTS(...)`: PostgreSQL would hand back
        // a real boolean there, but SQLite and MySQL hand back an integer, and
        // decoding is typed. A count decodes as `i64` everywhere.
        let applied: i64 = sqlx::query(sql::COUNT_APPLIED)
            .bind(version)
            .fetch_one(&mut *conn)
            .await?
            .try_get::<i64, _>(0)?;

        if applied > 0 {
            continue;
        }

        for statement in statements(migration) {
            conn.execute(statement).await?;
        }

        sqlx::query(sql::RECORD_APPLIED)
            .bind(version)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Split a migration file into its individual statements, dropping the empty
/// tail a trailing separator leaves behind.
///
/// A separator is a *line* whose entire content is [`STATEMENT_SEPARATOR`],
/// not every occurrence of those four characters. The distinction is not
/// pedantry: the bundled files each carry a header comment explaining the
/// convention, that comment necessarily contains the sentinel, and a
/// substring split therefore cut the SQLite migration in half at
/// "separated by a line reading `--;;". SQLite got a fragment starting with a
/// backtick and the whole application refused to start.
fn statements(migration: &str) -> impl Iterator<Item = &str> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    // `split_inclusive` keeps the line terminator, so the running offset stays
    // a valid index into `migration` and every piece is a borrow of it. That
    // also makes the function agnostic about `\n` versus `\r\n`, since the
    // trim below removes either.
    for line in migration.split_inclusive('\n') {
        if line.trim() == STATEMENT_SEPARATOR {
            pieces.push(&migration[start..offset]);
            start = offset + line.len();
        }
        offset += line.len();
    }
    pieces.push(&migration[start..]);
    pieces
        .into_iter()
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_migration_splits_into_at_least_one_statement() {
        for &(version, migration) in MIGRATIONS {
            let count = statements(migration).count();
            assert!(count > 0, "{version} produced no statements");
        }
    }

    #[test]
    fn a_comment_that_mentions_the_separator_does_not_split_the_file() {
        let sql = "-- Statements are separated by a line reading `--;;`.\n\
                   CREATE TABLE a (i INT)";
        let split: Vec<_> = statements(sql).collect();
        assert_eq!(split.len(), 1, "split into {split:?}");
        assert!(split[0].starts_with("-- Statements"));
    }

    #[test]
    fn no_bundled_statement_begins_mid_comment() {
        // What a substring split produced: a fragment whose first character is
        // the tail of the backtick-quoted sentinel in the header comment.
        for &(version, migration) in MIGRATIONS {
            for statement in statements(migration) {
                assert!(
                    !statement.starts_with('`'),
                    "{version} produced a fragment, not a statement: {statement:.40}"
                );
            }
        }
    }

    #[test]
    fn splitting_drops_blank_fragments_and_trims_each_statement() {
        let split: Vec<_> =
            statements("  CREATE TABLE a (i INT)  \n--;;\n\n--;;\nSELECT 1\n--;;\n").collect();
        assert_eq!(split, vec!["CREATE TABLE a (i INT)", "SELECT 1"]);
    }
}

// The tests above split text. Whether the text they split is SQL this
// dialect's server accepts is a different question, and only the server can
// answer it: three files, three grammars, and a `CREATE TABLE` that parses on
// PostgreSQL says nothing about MySQL's refusal of `CREATE INDEX IF NOT
// EXISTS`. These tests need a server; see `crate::jobs::test_support` for how
// they skip without one. Which dialect is exercised is the build's choice of
// driver, so the three are covered by running the suite three times rather
// than by three tests.
#[cfg(all(test, feature = "test-kit"))]
mod live_tests {
    use super::*;
    use crate::jobs::test_support::{enqueue, queue, rows};

    /// Read the applied versions out of the history table.
    async fn applied(pool: &JobPool) -> Vec<String> {
        sqlx::query("SELECT version FROM arcature_jobs_schema_migrations")
            .fetch_all(pool)
            .await
            .expect("read the migration history")
            .iter()
            .map(|row| row.try_get::<String, _>("version").expect("version"))
            .collect()
    }

    /// Every statement in this dialect's file is accepted, and the table it
    /// creates is the one the queue writes to.
    ///
    /// The fixture has already applied the migration by the time the test
    /// body runs -- that is what makes an empty `arcature_jobs` available at
    /// all -- so the assertion is about what the migration produced: a table
    /// that takes an enqueue, and a history row naming the version.
    #[tokio::test]
    async fn the_bundled_migration_runs_and_leaves_a_usable_table() {
        let Some(fixture) = queue().await else {
            return;
        };
        let pool = fixture.pool();

        let versions = applied(pool).await;
        for &(version, _) in MIGRATIONS {
            assert!(
                versions.iter().any(|applied| applied == version),
                "{version} is not recorded in the history table: {versions:?}"
            );
        }

        let enqueued = enqueue(pool, 1).await;
        assert_eq!(
            rows(pool).await,
            vec![(enqueued[0], "pending".to_owned(), 0)],
            "the migrated table did not accept an ordinary enqueue"
        );
    }

    /// Applying twice is a no-op rather than an error.
    ///
    /// This is not a theoretical concern: every process that boots calls
    /// `migrate`, so the second application is the normal case and the first
    /// is the exception. It also exercises the dialect's lock -- PostgreSQL's
    /// `pg_advisory_lock`, MySQL's `GET_LOCK`, SQLite's deliberate absence of
    /// one -- including the release, since a session that failed to release
    /// would hang the next `apply` on the same pool rather than fail it.
    #[tokio::test]
    async fn applying_again_changes_nothing() {
        let Some(fixture) = queue().await else {
            return;
        };
        let pool = fixture.pool();
        let before = applied(pool).await;

        apply(pool).await.expect("apply a second time");
        apply(pool).await.expect("apply a third time");

        let after = applied(pool).await;
        assert_eq!(
            after.len(),
            before.len(),
            "re-applying added history rows: {before:?} then {after:?}"
        );
    }

    /// The caller's transaction is honoured: rolling it back unapplies the
    /// migration's bookkeeping.
    ///
    /// `apply_tx` exists so an application can migrate inside a transaction it
    /// already has. What that must not do is take a session-scoped lock the
    /// caller's transaction then outlives, or record a version the rollback
    /// discards. Applying inside a transaction that is rolled back, then
    /// applying normally, would deadlock or double-record if either were
    /// wrong.
    ///
    /// PostgreSQL and SQLite roll DDL back; MySQL commits it implicitly, so
    /// the assertion is deliberately only that both calls succeed and the
    /// history ends up with one row per migration -- true either way.
    #[tokio::test]
    async fn applying_inside_a_rolled_back_transaction_leaves_the_history_consistent() {
        let Some(fixture) = queue().await else {
            return;
        };
        let pool = fixture.pool();

        let mut tx = pool.begin().await.expect("begin");
        apply_tx(&mut tx).await.expect("apply inside a transaction");
        tx.rollback().await.expect("roll back");

        apply(pool).await.expect("apply after the rollback");

        let versions = applied(pool).await;
        assert_eq!(
            versions.len(),
            MIGRATIONS.len(),
            "the history holds {versions:?} for {} migration(s)",
            MIGRATIONS.len()
        );
    }
}
