//! Session store schema migrations.
//!
//! One embedded migration per dialect, applied under a lock where the dialect
//! has one, with an `arcature_sessions_schema_migrations` history table.
//! Applying twice is a no-op.
//!
//! The migration text is not handed to the driver as one blob. MySQL rejects
//! multiple statements in a single prepared query unless the connection opted
//! in, so every dialect's file is split on a `--;;` sentinel and executed one
//! statement at a time. That keeps one code path instead of one per driver.
//!
//! This mirrors [`crate::jobs`]'s migrator rather than sharing code with it.
//! The two are twenty lines of near-identical plumbing, and the alternative --
//! a generic migrator parameterised over table name, history table, and lock
//! -- would couple two subsystems that have no reason to change together, so
//! that a schema change in one could break the other's migration.

use sqlx::{Executor, Row};

use super::dialect::{SessionDb, SessionPool, sql};
use super::error::SessionStoreError;

/// The connection type of the dialect this build speaks.
type Conn = <SessionDb as sqlx::Database>::Connection;

/// The statement separator inside a migration file. A bare `;` cannot be used:
/// it also ends statements *inside* a `CREATE TABLE` body, so the files mark
/// their real boundaries explicitly.
const STATEMENT_SEPARATOR: &str = "--;;";

/// All migrations in order, as `(version, sql)`.
///
/// The SQL arrives via `include_str!`, so both halves are `&'static str` and
/// pass SQLx's `SqlSafeStr` gate without an escape hatch.
const MIGRATIONS: &[(&str, &str)] = &[("0001_sessions", sql::SCHEMA)];

/// Apply all pending migrations over the pool.
///
/// # Errors
///
/// Returns [`SessionStoreError::Database`] if the database rejects a statement
/// or the connection fails.
pub(super) async fn apply(pool: &SessionPool) -> Result<(), SessionStoreError> {
    // One connection for the whole run, not one per statement. PostgreSQL's
    // `pg_advisory_lock` and MySQL's `GET_LOCK` are held by the *session*, so
    // a lock taken on a pooled connection protects nothing if the following
    // DDL goes out on a different one.
    let mut conn = pool.acquire().await?;
    apply_on(&mut conn).await
}

/// Apply migrations over one connection, taking the dialect's lock around
/// them if it has one.
async fn apply_on(conn: &mut Conn) -> Result<(), SessionStoreError> {
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
async fn apply_pending(conn: &mut Conn) -> Result<(), SessionStoreError> {
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
/// convention, that comment necessarily contains the sentinel, and a substring
/// split would therefore cut the file in half inside the comment and hand the
/// driver a fragment starting with a backtick.
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
    fn the_bundled_migration_creates_the_sessions_table() {
        // The table name is not incidental: every statement in `dialect`
        // names it literally, so a rename in the migration alone would
        // compile and then fail at the first query.
        for &(version, migration) in MIGRATIONS {
            assert!(
                statements(migration)
                    .any(|statement| statement
                        .contains("CREATE TABLE IF NOT EXISTS arcature_sessions")),
                "{version} does not create arcature_sessions"
            );
        }
    }
}
