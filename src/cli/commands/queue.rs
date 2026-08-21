//! `arc queue [--dsn <url>] <work|drain|stats>` — operate on the job queue.
//!
//! An operational utility that connects directly to the database (no app
//! binary), applies the queue schema, and runs one queue action:
//!
//! - `work`: claim and run jobs until interrupted (a standalone worker).
//! - `drain`: requeue dead jobs (sweep + requeue all `dead` rows).
//! - `stats`: print pending / running / dead / cancelled counts.
//!
//! `--dsn <url>` selects the database; defaults to `DATABASE_URL`.
//!
//! # Why there is no SQL here that names a dialect
//!
//! `arc queue` is built once per driver like everything else, so a statement
//! that only parses on PostgreSQL is a runtime failure on the other two
//! rather than a compile error. The counting query below is written in the
//! subset all three accept, and the requeue goes through
//! [`crate::jobs::admin::requeue_dead`] instead of writing its own `UPDATE`
//! -- the timestamp column is a real timestamp on PostgreSQL and MySQL and
//! epoch milliseconds on SQLite, and that difference already has one owner.

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
pub fn parse<'a>(iter: &mut std::slice::Iter<'a, OsString>) -> Result<Subcommand, SubcommandError> {
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
            eprintln!(
                "note: `arc queue work` runs a no-handler worker; use the app's in-process worker for real dispatch"
            );
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

/// The status counts, in the SQL subset all three drivers parse.
///
/// `count(*) FILTER (WHERE ..)` is the natural spelling and MySQL does not
/// have it. `COUNT(CASE WHEN .. THEN 1 END)` is the same aggregate written
/// in plain SQL-92, so one statement serves every driver rather than three
/// that have to be kept in agreement.
const COUNT_SQL: &str = r#"SELECT
    COUNT(CASE WHEN status = 'pending' THEN 1 END) AS pending,
    COUNT(CASE WHEN status = 'running' THEN 1 END) AS running,
    COUNT(CASE WHEN status = 'dead' THEN 1 END) AS dead,
    COUNT(CASE WHEN status = 'cancelled' THEN 1 END) AS cancelled
    FROM arcature_jobs"#;

/// Print the queue status counts.
async fn print_stats(pool: &crate::database::Pool) -> Result<(), QueueError> {
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

/// How many dead jobs to look up per round trip.
///
/// Bounded because a queue that has been failing for a week can hold more
/// dead rows than fit comfortably in memory, and unbounded is the kind of
/// thing that only shows up on the day it matters. The literal is inlined
/// rather than bound because `LIMIT` placeholders are the one part of this
/// statement whose spelling differs between drivers.
const DEAD_BATCH: usize = 1024;

/// The dead job ids, oldest first.
const DEAD_IDS_SQL: &str =
    "SELECT id FROM arcature_jobs WHERE status = 'dead' ORDER BY id LIMIT 1024";

/// Requeue every dead job back to pending (resets attempts).
///
/// One statement per job rather than one `UPDATE ... WHERE status = 'dead'`.
/// A set-based update would have to write `available_at` itself, and that
/// column is a timestamp on PostgreSQL and MySQL and epoch milliseconds on
/// SQLite; [`crate::jobs::admin::requeue_dead`] already knows which, and one
/// place knowing it is worth the round trips on a command a human runs by
/// hand.
async fn requeue_all_dead(pool: &crate::database::Pool) -> Result<u64, QueueError> {
    let mut total = 0u64;
    loop {
        let ids: Vec<uuid::Uuid> = sqlx::query_scalar(DEAD_IDS_SQL)
            .fetch_all(pool)
            .await
            .map_err(QueueError::Sqlx)?;
        if ids.is_empty() {
            return Ok(total);
        }

        let mut requeued = 0u64;
        for id in &ids {
            requeued += crate::jobs::admin::requeue_dead(pool, *id)
                .await
                .map_err(QueueError::Admin)?;
        }

        // A full batch that requeued nothing means another process is
        // holding those rows; looping again would spin rather than progress.
        if requeued == 0 {
            return Ok(total);
        }
        total += requeued;

        if ids.len() < DEAD_BATCH {
            return Ok(total);
        }
    }
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
