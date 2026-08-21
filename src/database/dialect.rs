//! The two places a statement built at runtime still has to name its dialect.
//!
//! Almost all SQL Arcature writes is either fixed text living behind
//! [`crate::jobs`]'s own per-dialect seam or a SeaORM query that renders
//! itself. What is left is the handful of statements assembled from a table
//! name and a list of columns -- the test kit's `assert_database_has` is the
//! whole of it today -- and those hit exactly two portability walls:
//!
//! 1. **Placeholders.** PostgreSQL numbers them `$1`, `$2`; MySQL and SQLite
//!    both write `?`. Getting this wrong is not a subtle failure -- the
//!    statement does not parse -- but it does mean the same `format!` cannot
//!    serve all three.
//! 2. **Casting to text.** Comparing a bound `&str` against a column of any
//!    type needs the column cast, and `CAST(x AS TEXT)` is not universal:
//!    MySQL's `CAST` has no `TEXT` target and wants `CHAR`. PostgreSQL and
//!    SQLite both accept `TEXT`, so this is two arms rather than three.
//!
//! Both helpers render text that is then interpolated into a statement, so
//! neither may ever be handed caller data. They take an index and an
//! already-validated identifier for that reason.

/// Render the placeholder for the `index`-th bound parameter, counting from 1.
///
/// The index is ignored on MySQL and SQLite, which have only positional `?`.
/// It is still required, because a caller that does not track its own
/// parameter numbering will bind them in the wrong order on PostgreSQL and
/// pass every test run against the other two.
#[cfg(feature = "db-postgres")]
pub(crate) fn placeholder(index: usize) -> String {
    format!("${index}")
}

/// Render the placeholder for the `index`-th bound parameter, counting from 1.
/// See the PostgreSQL arm for why `index` is required but unused here.
#[cfg(any(feature = "db-sqlite", feature = "db-mysql"))]
pub(crate) fn placeholder(index: usize) -> String {
    let _ = index;
    "?".to_owned()
}

/// Wrap `column` in a cast to the dialect's text type.
///
/// `column` is interpolated into the statement, so it must already have been
/// validated as a plain identifier by the caller.
#[cfg(any(feature = "db-postgres", feature = "db-sqlite"))]
pub(crate) fn as_text(column: &str) -> String {
    format!("CAST({column} AS TEXT)")
}

/// Wrap `column` in a cast to the dialect's text type. MySQL's `CAST` has no
/// `TEXT` target; `CHAR` is the spelling that works there.
#[cfg(feature = "db-mysql")]
pub(crate) fn as_text(column: &str) -> String {
    format!("CAST({column} AS CHAR)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_distinct_where_the_dialect_numbers_them() {
        // On PostgreSQL these must differ; on the other two they must not,
        // and either way the caller's numbering has to survive the call.
        let first = placeholder(1);
        let second = placeholder(2);
        if cfg!(feature = "db-postgres") {
            assert_eq!(first, "$1");
            assert_eq!(second, "$2");
        } else {
            assert_eq!(first, "?");
            assert_eq!(second, "?");
        }
    }

    #[test]
    fn a_text_cast_names_a_type_the_dialect_accepts() {
        let rendered = as_text("email");
        assert!(rendered.starts_with("CAST(email AS "), "{rendered}");
        if cfg!(feature = "db-mysql") {
            assert!(
                rendered.ends_with("CHAR)"),
                "MySQL has no CAST target `TEXT`: {rendered}"
            );
        } else {
            assert!(rendered.ends_with("TEXT)"), "{rendered}");
        }
    }
}
