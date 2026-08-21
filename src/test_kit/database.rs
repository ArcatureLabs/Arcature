//! Transaction-per-test against a real database.
//!
//! A test opens a transaction, does its work, and drops it. Nothing is
//! committed, so the next test starts from the same state without a truncate
//! pass, and two tests can run at once against the same database.
//!
//! Everything here is written against [`crate::database::Driver`], the alias
//! the `db-*` features select, so the harness speaks whichever of PostgreSQL,
//! SQLite, and MySQL the build was compiled for. The two things that differ
//! between them -- placeholder style and the spelling of a cast to text --
//! come from [`crate::database::dialect`]; nothing in this file names a
//! driver.
//!
//! # Safety
//!
//! Two conditions, both checked before a single byte is written:
//!
//! 1. `ARCATURE_TEST_DB_URL` must be set. A missing variable is a failure,
//!    never a skip. A suite that silently skips its database tests is a suite
//!    that reports green while testing nothing.
//! 2. The database name must start with `arcature_test_`. This is what stops
//!    a stray `DATABASE_URL` in the environment from pointing the suite at
//!    something that matters.

use std::fmt;

use sqlx::Transaction;

use crate::database::dialect::{as_text, placeholder};
use crate::database::{Connection, Driver, Pool};

/// The environment variable naming the test database.
pub const TEST_DB_URL_VAR: &str = "ARCATURE_TEST_DB_URL";

/// The required prefix of the test database name.
pub const TEST_DB_PREFIX: &str = "arcature_test_";

/// The environment variable that turns a missing test database from a skip
/// into a failure.
///
/// [`TestDatabase::optional`] returns `None` when no database is configured,
/// which is what keeps a laptop with no server green. That is also exactly
/// how a suite comes to report success while testing nothing, so CI sets this
/// variable and the skip becomes a panic. Both halves are needed: without the
/// skip nobody can run the suite locally, and without the switch nobody would
/// notice the day CI stopped provisioning a database.
pub const REQUIRE_TEST_DB_VAR: &str = "ARCATURE_REQUIRE_TEST_DB";

/// Why a test database could not be used.
#[derive(Debug)]
pub enum TestDatabaseError {
    /// `ARCATURE_TEST_DB_URL` is not set.
    NotConfigured,
    /// The URL has no database name.
    NoDatabaseName {
        /// The URL as given, with any password removed.
        url: String,
    },
    /// The database name does not start with [`TEST_DB_PREFIX`].
    UnsafeDatabaseName {
        /// The name that was refused.
        name: String,
    },
    /// The connection failed.
    Connect(String),
    /// A statement failed.
    Query(String),
}

impl fmt::Display for TestDatabaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured => write!(
                f,
                "{TEST_DB_URL_VAR} is not set; database tests need a database, and skipping them silently would report success for work that never ran"
            ),
            Self::NoDatabaseName { url } => {
                write!(f, "{TEST_DB_URL_VAR} names no database: {url}")
            }
            Self::UnsafeDatabaseName { name } => write!(
                f,
                "refusing to run against database `{name}`: the name must start with `{TEST_DB_PREFIX}`, because these tests write to it"
            ),
            Self::Connect(error) => write!(f, "could not connect to the test database: {error}"),
            Self::Query(error) => write!(f, "test database query failed: {error}"),
        }
    }
}

impl std::error::Error for TestDatabaseError {}

/// The database name in a connection URL.
///
/// Split by hand rather than with a URL parser: the only part that matters is
/// the path segment, and pulling in a parser to find it would add a
/// dependency for one `rfind`.
///
/// PostgreSQL and MySQL agree on the shape -- `scheme://[user[:pw]@]host[:port]/name`
/// -- so one branch serves both. SQLite does not have a host at all, and is
/// handled separately by [`sqlite_database_name`].
fn database_name(url: &str) -> Option<&str> {
    if is_sqlite_url(url) {
        return sqlite_database_name(url);
    }
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = after_scheme.split_once('/')?.1;
    let name = path
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    if name.is_empty() { None } else { Some(name) }
}

/// Whether `url` addresses SQLite.
///
/// SQLite is the one dialect whose URL has no authority component, so it has
/// to be recognised before the host-and-path split rather than after.
fn is_sqlite_url(url: &str) -> bool {
    let scheme = url.split_once(':').map_or("", |(scheme, _)| scheme);
    scheme.eq_ignore_ascii_case("sqlite")
}

/// The database "name" in a SQLite URL: the file stem.
///
/// SQLite names a file rather than a database on a server, so the safety
/// check applies to the file's own name. `sqlite:memory.db`,
/// `sqlite://./tmp/memory.db` and a bare path all reduce to `memory`.
///
/// An in-memory database is its own answer. `:memory:` is created empty, is
/// visible to nothing else, and is gone when the connection closes, so there
/// is no data it could destroy and no reason to make a caller name it
/// `arcature_test_`. It reports the literal [`TEST_DB_PREFIX`] so the caller
/// sees an accept rather than a special case.
fn sqlite_database_name(url: &str) -> Option<&str> {
    let after_scheme = url.split_once("://").map_or_else(
        || url.split_once(':').map_or(url, |(_, rest)| rest),
        |(_, rest)| rest,
    );
    let path = after_scheme.split(['?', '#']).next().unwrap_or_default();
    if path.trim_end_matches('/') == ":memory:" || path.is_empty() {
        // An empty path is SQLite's own spelling of a private temporary
        // database, which is as safe as `:memory:` for the same reasons.
        return Some(TEST_DB_PREFIX);
    }
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let stem = file.split_once('.').map_or(file, |(stem, _)| stem);
    if stem.is_empty() { None } else { Some(stem) }
}

/// Hide the password in a URL so a failure message can be pasted anywhere.
fn redact(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let Some((credentials, host)) = rest.split_once('@') else {
        return url.to_owned();
    };
    let user = credentials.split_once(':').map_or(credentials, |(u, _)| u);
    format!("{scheme}://{user}:***@{host}")
}

/// Check a URL against the two safety conditions.
///
/// Separate from connecting so the rules can be tested without a server.
///
/// # Errors
///
/// Returns [`TestDatabaseError::NoDatabaseName`] when the URL names no
/// database and [`TestDatabaseError::UnsafeDatabaseName`] when the name does
/// not start with [`TEST_DB_PREFIX`].
pub fn check_url(url: &str) -> Result<(), TestDatabaseError> {
    let Some(name) = database_name(url) else {
        return Err(TestDatabaseError::NoDatabaseName { url: redact(url) });
    };
    if name.starts_with(TEST_DB_PREFIX) {
        Ok(())
    } else {
        Err(TestDatabaseError::UnsafeDatabaseName {
            name: name.to_owned(),
        })
    }
}

/// The configured test database URL.
///
/// # Errors
///
/// Returns [`TestDatabaseError::NotConfigured`] when the variable is unset,
/// and whatever [`check_url`] refuses.
pub fn test_database_url() -> Result<String, TestDatabaseError> {
    let url = std::env::var(TEST_DB_URL_VAR).map_err(|_| TestDatabaseError::NotConfigured)?;
    check_url(&url)?;
    Ok(url)
}

/// Whether a missing test database must fail rather than skip.
///
/// True when [`REQUIRE_TEST_DB_VAR`] is set to anything other than the empty
/// string or `0`. `0` is spelled out because a workflow that computes the
/// value will write `0` for false long before it thinks to unset the
/// variable.
#[must_use]
pub fn test_database_required() -> bool {
    std::env::var(REQUIRE_TEST_DB_VAR).is_ok_and(|value| !value.is_empty() && value != "0")
}

/// A pool onto the test database, opened only after the safety check.
#[derive(Debug, Clone)]
pub struct TestDatabase {
    pool: Pool,
}

impl TestDatabase {
    /// Connect to the database named by `ARCATURE_TEST_DB_URL`.
    ///
    /// # Errors
    ///
    /// Returns an error when the variable is unset, when the database name is
    /// not a test name, or when the connection fails.
    pub async fn connect() -> Result<Self, TestDatabaseError> {
        let url = test_database_url()?;
        let pool = Pool::connect(&url)
            .await
            .map_err(|error| TestDatabaseError::Connect(error.to_string()))?;
        Ok(Self { pool })
    }

    /// Connect if a test database is configured, or report that none is.
    ///
    /// [`connect`](Self::connect) treats a missing `ARCATURE_TEST_DB_URL` as
    /// a failure, which is the right answer for a suite that must never
    /// report success for work it did not do. It is the wrong answer for a
    /// developer with no server running, who would then be unable to run the
    /// suite at all. This is the other answer: unconfigured means
    /// unconfigured, and the test returns without asserting anything.
    ///
    /// The dangerous half of that is a test suite quietly testing nothing, so
    /// the skip is switchable: with [`REQUIRE_TEST_DB_VAR`] set -- as CI sets
    /// it -- an unconfigured database panics instead. Every other failure
    /// (an unsafe database name, a refused connection) panics either way; a
    /// database that is present but wrong is never a reason to skip.
    ///
    /// ```
    /// # use arcature::test_kit::TestDatabase;
    /// # async fn example() {
    /// let Some(database) = TestDatabase::optional().await else {
    ///     // No test database here. Assert nothing rather than assert
    ///     // against nothing.
    ///     return;
    /// };
    /// let mut transaction = database.begin().await.expect("begin");
    /// # let _ = transaction.connection();
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics when the database is misconfigured or unreachable, and when it
    /// is unconfigured while [`test_database_required`] holds.
    pub async fn optional() -> Option<Self> {
        match Self::connect().await {
            Ok(database) => Some(database),
            Err(error @ TestDatabaseError::NotConfigured) => {
                assert!(
                    !test_database_required(),
                    "{REQUIRE_TEST_DB_VAR} is set, so {TEST_DB_URL_VAR} has to be too: {error}"
                );
                None
            }
            Err(error) => panic!("{error}"),
        }
    }

    /// Wrap a pool that the caller opened.
    ///
    /// The safety check still applies -- it is applied to `url`, which must be
    /// the URL the pool was opened with.
    ///
    /// # Errors
    ///
    /// Returns an error when `url` does not name a test database.
    pub fn from_pool(pool: Pool, url: &str) -> Result<Self, TestDatabaseError> {
        check_url(url)?;
        Ok(Self { pool })
    }

    /// The underlying pool.
    #[must_use]
    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// A framework database handle over the same pool.
    #[must_use]
    pub fn db(&self) -> crate::database::Db {
        crate::database::Db::from_pool(self.pool.clone())
    }

    /// Begin a transaction that rolls back when it is dropped.
    ///
    /// # Errors
    ///
    /// Returns an error when the transaction cannot be started.
    pub async fn begin(&self) -> Result<TestTransaction, TestDatabaseError> {
        let transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| TestDatabaseError::Query(error.to_string()))?;
        Ok(TestTransaction { transaction })
    }
}

/// A transaction that is never committed.
///
/// `sqlx` issues the rollback when the transaction is dropped, so a test that
/// panics leaves nothing behind either.
#[derive(Debug)]
pub struct TestTransaction {
    transaction: Transaction<'static, Driver>,
}

impl TestTransaction {
    /// The connection inside the transaction, for `sqlx::query`.
    pub fn connection(&mut self) -> &mut Connection {
        &mut self.transaction
    }

    /// Roll back now rather than at drop.
    ///
    /// # Errors
    ///
    /// Returns an error when the rollback statement fails.
    pub async fn rollback(self) -> Result<(), TestDatabaseError> {
        self.transaction
            .rollback()
            .await
            .map_err(|error| TestDatabaseError::Query(error.to_string()))
    }
}

/// Assert a row exists in `table` matching every column in `conditions`.
///
/// Values are compared as text (`CAST(column AS TEXT) = ?`) and are always
/// bound, never interpolated. That makes one assertion work for integers,
/// UUIDs, and strings alike, at the cost of depending on the database's text
/// form for exotic types -- for those, write the query.
///
/// The match is a `count(*)`, not `SELECT EXISTS(..)`. `EXISTS` yields a
/// boolean on PostgreSQL but an integer on MySQL and SQLite, so decoding it
/// as `bool` would compile everywhere and fail at runtime on two of the
/// three; `count(*)` is an integer in all of them.
///
/// Table and column names cannot be bound by the protocol, so they are
/// validated as plain identifiers and rejected otherwise.
///
/// # Panics
///
/// Panics when no matching row exists, reporting how many rows the table
/// holds and which individual conditions matched -- so a failure names the
/// column that is wrong rather than just saying "no row".
pub async fn assert_database_has(
    connection: &mut Connection,
    table: &str,
    conditions: &[(&str, &str)],
) {
    assert!(
        !conditions.is_empty(),
        "assert_database_has needs at least one condition; `any row at all` is not an assertion"
    );
    let table = checked_table(table);
    let matched = count(&mut *connection, &table, conditions)
        .await
        .unwrap_or_else(|error| panic!("assert_database_has could not query `{table}`: {error}"));
    if matched > 0 {
        return;
    }
    panic!(
        "no row in `{table}` matches {}\n{}",
        describe(conditions),
        diagnose(connection, &table, conditions).await
    );
}

/// Validate one SQL identifier.
///
/// Identifiers reach the statement by interpolation because the protocol has
/// no placeholder for them, so anything that is not a bare identifier is
/// refused rather than quoted -- a test that needs a quoted mixed-case name
/// can write its own query.
fn checked_identifier(name: &str) -> &str {
    let valid = !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    assert!(
        valid,
        "`{name}` is not a plain SQL identifier; assert_database_has interpolates table and column names, so it accepts only [A-Za-z_][A-Za-z0-9_]*"
    );
    name
}

/// Validate a table name, allowing one level of schema qualification.
fn checked_table(table: &str) -> String {
    match table.split_once('.') {
        Some((schema, name)) => {
            format!(
                "{}.{}",
                checked_identifier(schema),
                checked_identifier(name)
            )
        }
        None => checked_identifier(table).to_owned(),
    }
}

/// Render the conditions for a failure message.
fn describe(conditions: &[(&str, &str)]) -> String {
    let rendered: Vec<String> = conditions
        .iter()
        .map(|(column, value)| format!("{column} = `{value}`"))
        .collect();
    rendered.join(" and ")
}

/// Explain why nothing matched: the table size, and each condition on its own.
///
/// One condition matching zero rows names the mistake. All of them matching
/// separately but none together means the row is split across records.
async fn diagnose(connection: &mut Connection, table: &str, conditions: &[(&str, &str)]) -> String {
    let mut lines = Vec::with_capacity(conditions.len() + 1);
    match count(&mut *connection, table, &[]).await {
        Ok(total) => lines.push(format!("  `{table}` holds {total} rows")),
        Err(error) => lines.push(format!("  `{table}` could not be counted: {error}")),
    }
    for condition in conditions {
        let (column, value) = *condition;
        match count(&mut *connection, table, std::slice::from_ref(condition)).await {
            Ok(matched) => {
                lines.push(format!("  {column} = `{value}` matches {matched} rows"));
            }
            Err(error) => lines.push(format!(
                "  {column} = `{value}` could not be counted: {error}"
            )),
        }
    }
    lines.join("\n")
}

/// Count the rows in `table` matching every one of `conditions`.
///
/// An empty slice counts the whole table, which is what the failure diagnosis
/// wants first. Column names are validated and interpolated because the
/// protocol has no placeholder for an identifier; values are always bound.
async fn count(
    connection: &mut Connection,
    table: &str,
    conditions: &[(&str, &str)],
) -> Result<i64, sqlx::Error> {
    let mut sql = format!("SELECT count(*) FROM {table}");
    if !conditions.is_empty() {
        let clauses: Vec<String> = conditions
            .iter()
            .enumerate()
            .map(|(index, (column, _))| {
                format!(
                    "{} = {}",
                    as_text(checked_identifier(column)),
                    placeholder(index + 1)
                )
            })
            .collect();
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    let mut query = sqlx::query_scalar::<Driver, i64>(sqlx::AssertSqlSafe(sql));
    for (_, value) in conditions {
        query = query.bind(*value);
    }
    query.fetch_one(connection).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_database_name_is_accepted() {
        assert!(check_url("postgres://user:pw@localhost:5432/arcature_test_app").is_ok());
    }

    #[test]
    fn a_production_looking_database_name_is_refused() {
        let error = check_url("postgres://user:pw@localhost/app_production")
            .expect_err("a non-test database must be refused");
        assert!(matches!(
            error,
            TestDatabaseError::UnsafeDatabaseName { .. }
        ));
        assert!(error.to_string().contains("app_production"));
    }

    #[test]
    fn a_url_without_a_database_name_is_refused() {
        let error =
            check_url("postgres://user:pw@localhost:5432").expect_err("no database name is fatal");
        assert!(matches!(error, TestDatabaseError::NoDatabaseName { .. }));
    }

    #[test]
    fn query_parameters_are_not_part_of_the_database_name() {
        assert_eq!(
            database_name("postgres://localhost/arcature_test_app?sslmode=require"),
            Some("arcature_test_app")
        );
    }

    #[test]
    fn a_failure_message_never_carries_the_password() {
        let error =
            check_url("postgres://user:hunter2@localhost").expect_err("no database name is fatal");
        let message = error.to_string();
        assert!(!message.contains("hunter2"), "message leaked: {message}");
        assert!(message.contains("***"), "message: {message}");
    }

    #[test]
    fn an_identifier_with_a_quote_is_refused() {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            checked_identifier("users\"; drop table users--")
        }));
        assert!(
            outcome.is_err(),
            "a hostile identifier must not be accepted"
        );
    }

    #[test]
    fn a_schema_qualified_table_keeps_both_parts() {
        assert_eq!(checked_table("public.users"), "public.users");
    }

    #[test]
    fn a_mysql_url_is_read_the_same_way_as_a_postgres_one() {
        assert_eq!(
            database_name("mysql://user:pw@localhost:3306/arcature_test_app"),
            Some("arcature_test_app")
        );
    }

    #[test]
    fn a_sqlite_file_is_named_by_its_stem() {
        // Every spelling sqlx accepts has to reduce to the same name, or the
        // safety check would pass on one and fail on another.
        for url in [
            "sqlite://arcature_test_app.db",
            "sqlite:arcature_test_app.db",
            "sqlite://./tmp/arcature_test_app.db",
            "sqlite://arcature_test_app.db?mode=rwc",
        ] {
            assert_eq!(database_name(url), Some("arcature_test_app"), "{url}");
            assert!(check_url(url).is_ok(), "{url}");
        }
    }

    #[test]
    fn a_sqlite_file_that_is_not_a_test_database_is_refused() {
        let error = check_url("sqlite://./data/production.db")
            .expect_err("a non-test SQLite file must be refused");
        assert!(matches!(
            error,
            TestDatabaseError::UnsafeDatabaseName { .. }
        ));
        assert!(error.to_string().contains("production"));
    }

    #[test]
    fn an_in_memory_sqlite_database_needs_no_name() {
        // Nothing to destroy: it is created empty and dies with the
        // connection, so demanding a prefix would be ceremony.
        for url in ["sqlite::memory:", "sqlite://:memory:", "sqlite://"] {
            assert!(check_url(url).is_ok(), "{url}");
        }
    }
}
