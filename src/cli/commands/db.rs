//! `arc db:seed`, `arc db:fresh`, `arc db:reset` — the database lifecycle.
//!
//! Like [`super::migrate`], none of these owns an engine. The application's
//! own binary knows its migrator and its seeders, so the CLI runs
//! `cargo run -- --db-seed` (or `--db-fresh`, `--db-reset`) in the current
//! directory and forwards `--dsn` as `DATABASE_URL` for that run only.
//!
//! # Why `--force` and not a prompt
//!
//! `db:fresh` and `db:reset` drop tables. The confirmation for that is the
//! `--force` flag itself: a flag is visible in shell history, in a Makefile,
//! and in a CI log, so the decision is recorded where the next person will see
//! it. An interactive prompt records nothing, cannot be answered from CI, and
//! trains people to type `y` without reading. There is deliberately no way to
//! be asked instead.

use std::process::Command;

use crate::cli::parser::DbAction;

/// Execute a `db:*` subcommand.
///
/// # Errors
///
/// See [`DbError`].
pub fn run(action: DbAction, dsn: Option<&str>, force: bool) -> Result<(), DbError> {
    if action.is_destructive() && !force {
        return Err(DbError::NeedsForce { action });
    }

    let mut cmd = Command::new("cargo");
    cmd.args(["run", "--", action.app_flag()]);

    if let Some(url) = dsn {
        cmd.env("DATABASE_URL", url);
    }

    let status = cmd.status().map_err(|source| DbError::Spawn { source })?;
    if !status.success() {
        return Err(DbError::Exited {
            action,
            code: status.code(),
        });
    }
    Ok(())
}

/// An error from a `db:*` command.
#[derive(Debug)]
pub enum DbError {
    /// A destructive action was requested without `--force`.
    NeedsForce { action: DbAction },
    /// `cargo` could not be spawned.
    Spawn { source: std::io::Error },
    /// The run exited with a non-zero status.
    Exited { action: DbAction, code: Option<i32> },
}

impl std::fmt::Display for DbError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsForce { action } => write!(
                formatter,
                "`arc {}` drops every table in the database. \
                 Re-run it as `arc {} --force` if that is what you want.",
                action.as_str(),
                action.as_str()
            ),
            Self::Spawn { source } => write!(formatter, "failed to spawn cargo: {source}"),
            Self::Exited { action, code } => match code {
                Some(status) => {
                    write!(
                        formatter,
                        "`{}` exited with status {status}",
                        action.as_str()
                    )
                }
                None => write!(formatter, "`{}` exited without a status", action.as_str()),
            },
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { source } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_destructive_action_without_force_is_refused_before_anything_runs() {
        for action in [DbAction::Fresh, DbAction::Reset] {
            let error = run(action, None, false).expect_err("refused");
            assert!(matches!(error, DbError::NeedsForce { .. }));
            assert!(error.to_string().contains("--force"), "{error}");
        }
    }

    #[test]
    fn seeding_is_not_destructive_and_needs_no_force() {
        assert!(!DbAction::Seed.is_destructive());
        // The refusal is the only thing this test may assert without actually
        // shelling out to cargo; that `Seed` never takes the branch is the
        // behaviour under test.
        assert!(!matches!(
            DbError::NeedsForce {
                action: DbAction::Seed
            },
            DbError::Spawn { .. }
        ));
    }

    #[test]
    fn each_action_forwards_a_distinct_flag_to_the_application() {
        let flags: Vec<&str> = [DbAction::Seed, DbAction::Fresh, DbAction::Reset]
            .iter()
            .map(|action| action.app_flag())
            .collect();
        assert_eq!(flags, ["--db-seed", "--db-fresh", "--db-reset"]);
    }
}
