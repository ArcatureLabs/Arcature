//! Password-reset schema migrations.
//!
//! One embedded migration per dialect, applied under a lock where the dialect
//! has one, with an `arcature_password_resets_schema_migrations` history
//! table. Applying twice is a no-op.
//!
//! The migration text is not handed to the driver as one blob. MySQL rejects
//! multiple statements in a single prepared query unless the connection opted
//! in, so every dialect's file is split on a `--;;` sentinel and executed one
//! statement at a time. That keeps one code path instead of one per driver.
//!
//! This mirrors [`crate::jobs`]'s, [`crate::auth::session_store`]'s and
//! [`crate::tokens`]'s migrators rather than sharing code with them, for the
//! reason stated in the second: a generic migrator parameterised over table
//! name, history table, and lock would couple subsystems that have no reason
//! to change together, so that a schema change in one could break another's
//! migration.

use sqlx::{Executor, Row};

use super::dialect::{ResetDb, ResetPool, sql};
use super::error::PasswordResetError;

/// The connection type of the dialect this build speaks.
type Conn = <ResetDb as sqlx::Database>::Connection;

/// The statement separator inside a migration file. A bare `;` cannot be
/// used: it also ends statements *inside* a `CREATE TABLE` body, so the files
/// mark their real boundaries explicitly.
const STATEMENT_SEPARATOR: &str = "--;;";

/// All migrations in order, as `(version, sql)`.
///
/// The SQL arrives via `include_str!`, so both halves are `&'static str` and
/// pass SQLx's `SqlSafeStr` gate without an escape hatch.
const MIGRATIONS: &[(&str, &str)] = &[("0001_password_resets", sql::SCHEMA)];

/// Apply all pending migrations over the pool.
///
/// # Errors
///
/// Returns [`PasswordResetError::Database`] if the database rejects a
/// statement or the connection fails.
pub(super) async fn apply(pool: &ResetPool) -> Result<(), PasswordResetError> {
    // One connection for the whole run, not one per statement. PostgreSQL's
    // `pg_advisory_lock` and MySQL's `GET_LOCK` are held by the *session*, so
    // a lock taken on a pooled connection protects nothing if the following
    // DDL goes out on a different one.
    let mut conn = pool.acquire().await?;
    apply_on(&mut conn).await
}

/// Apply migrations over one connection, taking the dialect's lock around
/// them if it has one.
async fn apply_on(conn: &mut Conn) -> Result<(), PasswordResetError> {
    conn.execute(sql::CREATE_HISTORY).await?;

    let Some(lock) = sql::LOCK else {
        // SQLite has no advisory lock and needs none: it serialises writers
        // itself, and every statement in the migration is `IF NOT EXISTS`.
        return apply_pending(conn).await;
    };

    sqlx::query(lock).execute(&mut *conn).await?;
    let result = apply_pending(&mut *conn).await;
    // The unlock is best-effort on purpose. If it fails the session is
    // already broken, and reporting that instead of the migration's own error
    // would hide the reason the caller actually needs.
    if let Some(unlock) = sql::UNLOCK {
        let _ = sqlx::query(unlock).execute(&mut *conn).await;
    }
    result
}

/// Run every migration that the history table does not already record.
async fn apply_pending(conn: &mut Conn) -> Result<(), PasswordResetError> {
    for &(version, migration) in MIGRATIONS {
        // A count rather than `SELECT EXISTS(...)`: PostgreSQL would hand
        // back a real boolean there, but SQLite and MySQL hand back an
        // integer, and decoding is typed. A count decodes as `i64`
        // everywhere.
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
/// substring split would therefore cut the file in half inside the comment.
fn statements(migration: &str) -> impl Iterator<Item = &str> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    // `split_inclusive` keeps the line terminator, so the running offset
    // stays a valid index into `migration` and every piece is a borrow of it.
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
    fn the_bundled_migration_creates_the_password_resets_table() {
        // The table name is not incidental: every statement in `dialect`
        // names it literally, so a rename in the migration alone would
        // compile and then fail at the first query.
        for &(version, migration) in MIGRATIONS {
            assert!(
                statements(migration).any(|statement| statement
                    .contains("CREATE TABLE IF NOT EXISTS arcature_password_resets")),
                "{version} does not create arcature_password_resets"
            );
        }
    }

    #[test]
    fn the_bundled_migration_never_stores_a_plaintext_token() {
        // The whole property of this schema in one assertion: there is a
        // digest column and no column that could hold the secret itself. A
        // future migration that adds one has to delete this test first.
        for &(version, migration) in MIGRATIONS {
            assert!(
                migration.contains("secret_digest"),
                "{version} has no secret_digest column"
            );
            assert!(
                !migration.contains("secret_plaintext") && !migration.contains("token TEXT"),
                "{version} appears to store a token in the clear"
            );
        }
    }

    #[test]
    fn the_expiry_column_has_no_null_state() {
        // "Never expires" is not representable on purpose; see the header of
        // the PostgreSQL migration. A reset link with no deadline is a
        // password change sitting in an inbox forever.
        for &(version, migration) in MIGRATIONS {
            let expiry_line = migration
                .lines()
                .find(|line| line.trim_start().starts_with("expires_at"))
                .unwrap_or_else(|| panic!("{version} declares no expires_at column"));
            assert!(
                expiry_line.contains("NOT NULL"),
                "{version} allows a null expiry: {expiry_line}"
            );
        }
    }

    #[test]
    fn the_subject_is_indexed_so_revocation_is_not_a_scan() {
        // Both `issue` and a completed reset delete by subject. Without this
        // index those are full scans, and the table is exactly the one that
        // grows under a password-reset flood.
        for &(version, migration) in MIGRATIONS {
            assert!(
                migration.contains("arcature_password_resets_subject_idx"),
                "{version} does not index subject"
            );
        }
    }
}
