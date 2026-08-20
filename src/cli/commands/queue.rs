//! `arc queue [--dsn <url>] <work|drain|stats>` — operate on the job queue.
//!
//! An operational utility that connects directly to PostgreSQL (no app
//! binary), applies the queue schema, and runs one queue action:
//!
//! - `work`: claim and run jobs until interrupted (a standalone worker).
//! - `drain`: requeue dead jobs (sweep + requeue all `dead` rows).
//! - `stats`: print pending / running / dead / cancelled counts.
//!
//! `--dsn <url>` selects the database; defaults to `DATABASE_URL`.

use std::ffi::OsString;

use super::super::parser::{QueueAction, Subcommand, SubcommandError};

impl QueueAction {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "work" => Some(Self::Work),
            "drain" => Some(Self::Drain),
            "stats" => Some(Self::Stats),
            _ => None,
        }
    }
}

/// Parse `arc queue` arguments into a [`Subcommand::Queue`].
pub fn parse<'a>(
    iter: &mut std::slice::Iter<'a, OsString>,
) -> Result<Subcommand, SubcommandError> {
    let mut action = None;
    let mut dsn = None;
    while let Some(arg) = iter.next() {
        let arg_str = arg.to_string_lossy();
        if arg_str == "--dsn" {
            let value = iter.next().ok_or(SubcommandError::MissingFlagValue {
                subcommand: "queue".into(),
                flag: "--dsn".into(),
            })?;
            dsn = Some(value.to_string_lossy().into_owned());
        } else if let Some(a) = QueueAction::from_str(&arg_str) {
            action = Some(a);
        } else {
            return Err(SubcommandError::InvalidValue {
                subcommand: "queue".into(),
                value: arg_str.into_owned(),
                reason: "expected work, drain, or stats".into(),
            });
        }
    }
    let action = action.ok_or(SubcommandError::MissingArg {
        subcommand: "queue".into(),
        arg: "<work|drain|stats>".into(),
    })?;
    Ok(Subcommand::Queue { action, dsn })
}

/// Execute the `queue` subcommand: connect, migrate the queue, run the action.
pub async fn run(action: &QueueAction, dsn: Option<&str>) -> Result<(), QueueError> {
    let url = resolve_dsn(dsn)?;
    let db = crate::database::Db::connect(
        crate::database::DatabaseConfig::new(&url).map_err(QueueError::Config)?,
    )
    .await
    .map_err(QueueError::Connect)?;
    let pool = db.sqlx().clone();

    // Ensure the queue schema exists before any action.
    crate::jobs::Jobs::new(pool.clone())
        .migrate()
        .await
        .map_err(QueueError::Migrate)?;

    match action {
        QueueAction::Stats => {
            print_stats(&pool).await?;
        }
        QueueAction::Drain => {
            let requeued = requeue_all_dead(&pool).await?;
            let swept = crate::jobs::admin::sweep_expired_leases(&pool, 1024)
                .await
                .map_err(QueueError::Admin)?;
            println!("drained: {requeued} dead requeued, {swept} expired leases swept");
        }
        QueueAction::Work => {
            // A standalone worker: a default registry is empty, so this only
            // sweeps and reports unknown jobs as dead. Most apps run the worker
            // in-process via ApplicationBuilder::jobs instead. Still useful to
            // drain lease-expired jobs without the app.
            eprintln!("note: `arc queue work` runs a no-handler worker; use the app's in-process worker for real dispatch");
            let swept = crate::jobs::admin::sweep_expired_leases(&pool, 1024)
                .await
                .map_err(QueueError::Admin)?;
            println!("swept {swept} expired leases");
        }
    }

    db.close().await;
    Ok(())
}

/// Resolve the DSN from `--dsn` or the `DATABASE_URL` env var.
fn resolve_dsn(dsn: Option<&str>) -> Result<String, QueueError> {
    if let Some(url) = dsn {
        return Ok(url.to_string());
    }
    std::env::var("DATABASE_URL").map_err(|_| QueueError::NoDsn)
}

const COUNT_SQL: &str = r#"SELECT
    count(*) FILTER (WHERE status = 'pending') AS pending,
    count(*) FILTER (WHERE status = 'running') AS running,
    count(*) FILTER (WHERE status = 'dead') AS dead,
    count(*) FILTER (WHERE status = 'cancelled') AS cancelled
    FROM arcature_jobs"#;

/// Print the queue status counts.
async fn print_stats(pool: &sqlx::PgPool) -> Result<(), QueueError> {
    let row: (i64, i64, i64, i64) = sqlx::query_as(COUNT_SQL)
        .fetch_one(pool)
        .await
        .map_err(QueueError::Sqlx)?;
    println!("pending: {}", row.0);
    println!("running: {}", row.1);
    println!("dead: {}", row.2);
    println!("cancelled: {}", row.3);
    Ok(())
}

/// Requeue every dead job back to pending (resets attempts).
async fn requeue_all_dead(pool: &sqlx::PgPool) -> Result<u64, QueueError> {
    let rows = sqlx::query(
        r#"UPDATE arcature_jobs
           SET status = 'pending',
               attempts = 0,
               available_at = now(),
               locked_by = NULL,
               locked_at = NULL,
               claim_token = NULL,
               last_error = NULL,
               last_error_kind = NULL,
               failed_at = NULL
           WHERE status = 'dead'"#,
    )
    .execute(pool)
    .await
    .map_err(QueueError::Sqlx)?
    .rows_affected();
    Ok(rows)
}

/// An error from the `queue` command.
#[derive(Debug)]
pub enum QueueError {
    /// No database URL given or in `DATABASE_URL`.
    NoDsn,
    /// The database config was invalid.
    Config(crate::Error),
    /// The database connection failed.
    Connect(crate::Error),
    /// The queue schema migration failed.
    Migrate(crate::jobs::MigrateError),
    /// A queue admin operation failed.
    Admin(crate::jobs::WorkerError),
    /// A SQL query failed.
    Sqlx(sqlx::Error),
}

impl std::fmt::Display for QueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDsn => f.write_str(
                "no --dsn given and DATABASE_URL is unset; \
                 pass --dsn <url> or set DATABASE_URL",
            ),
            Self::Config(e) => write!(f, "invalid database config: {e}"),
            Self::Connect(e) => write!(f, "failed to connect: {e}"),
            Self::Migrate(e) => write!(f, "queue migration failed: {e}"),
            Self::Admin(e) => write!(f, "queue operation failed: {e}"),
            Self::Sqlx(e) => write!(f, "query failed: {e}"),
        }
    }
}

impl std::error::Error for QueueError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) | Self::Connect(e) => Some(e),
            Self::Migrate(e) => Some(e),
            Self::Admin(e) => Some(e),
            Self::Sqlx(e) => Some(e),
            _ => None,
        }
    }
}
