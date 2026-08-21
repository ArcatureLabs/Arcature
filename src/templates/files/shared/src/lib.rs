#![forbid(unsafe_code)]

//! The generated application crate root.
//!
//! Modules outside `src/` (`app/`, `bootstrap/`, `config/`, `database/`,
//! `routes/`) are pulled in with `#[path]` so the on-disk layout matches the
//! Laravel-style structure rather than Cargo's default.
//!
//! [`run`] is the whole entry point: it installs logging, reads the process
//! arguments, and either performs a one-shot database command or serves the
//! application.

#[path = "../app/mod.rs"]
pub mod app;
#[path = "../bootstrap/mod.rs"]
pub mod bootstrap;
#[path = "../config/mod.rs"]
pub mod config;
#[path = "../database/mod.rs"]
pub mod database;
#[path = "../routes/mod.rs"]
pub mod routes;

use arcature::prelude::*;

/// What the process was asked to do.
///
/// One flag, one mode. Combining `--migrate` with serving would make the
/// deploy order implicit -- every replica racing to migrate on boot -- so the
/// migration is its own invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Bind the port and serve.
    Serve,
    /// Serve and run the scheduler in the same process.
    Schedule,
    /// Apply pending migrations, then exit.
    Migrate,
    /// Run the seeders, then exit.
    Seed,
    /// Drop every table, re-run all migrations, then exit.
    Fresh,
    /// Roll every migration back, then exit.
    Reset,
}

/// Run the application.
///
/// # Errors
///
/// Returns the failure as an [`std::io::Error`] so `main` can propagate it
/// with the process exit code Rust already assigns to a `Result`-returning
/// `main`.
pub async fn run<I>(args: I) -> std::io::Result<()>
where
    I: IntoIterator<Item = String>,
{
    // First, before anything that might have something to say. `tracing`
    // events go nowhere until a subscriber exists, so without this line the
    // access log, the startup errors and every framework warning are emitted
    // into nothing -- the process runs, and says nothing at all.
    //
    // Debug builds get a human-readable line on stderr; release builds get
    // one redacted JSON object per line. `RUST_LOG` overrides the filter
    // below without touching this file.
    arcature::observe::install_logging(LOG_FILTER).map_err(std::io::Error::other)?;

    let mode = parse_mode(args).map_err(std::io::Error::other)?;
    match mode {
        Mode::Serve => serve(false).await,
        Mode::Schedule => serve(true).await,
        Mode::Migrate | Mode::Seed | Mode::Fresh | Mode::Reset => {
            database_command(mode).await.map_err(std::io::Error::other)
        }
    }
}

/// Bind and serve, optionally with the scheduler in the same process.
async fn serve(scheduler: bool) -> std::io::Result<()> {
    let bootstrapped =
        bootstrap::app(bootstrap::BootOptions { scheduler }).map_err(std::io::Error::other)?;
    bootstrapped
        .application
        .run_with_state(bootstrapped.state_fn)
        .await
        .map_err(std::io::Error::other)
}

/// Perform a one-shot database command against a connection built from the
/// same configuration the server uses.
async fn database_command(mode: Mode) -> Result<()> {
    use arcature::database::sea_orm_migration::MigratorTrait as _;

    dotenvy::dotenv().ok();
    let config = config::load()?;
    let db = Db::connect(config.database).await?;

    match mode {
        Mode::Migrate => {
            arcature::database::migration::up::<database::Migrator>(&db).await?;
            println!("Migrations applied.");
        }
        Mode::Fresh => {
            arcature::database::migration::fresh::<database::Migrator>(&db).await?;
            println!("Database rebuilt from scratch.");
        }
        Mode::Reset => {
            // `down` takes a count, and rolling back "everything" is the
            // number of migrations the migrator knows about.
            let steps = u32::try_from(database::Migrator::migrations().len())
                .map_err(|_| Error::Config("too many migrations to roll back".to_string()))?;
            arcature::database::migration::down::<database::Migrator>(&db, steps).await?;
            println!("All migrations rolled back.");
        }
        Mode::Seed => {
            database::seeders::run(&db).await?;
            println!("Seeders finished.");
        }
        Mode::Serve | Mode::Schedule => unreachable!("serve modes never reach here"),
    }
    Ok(())
}

/// Map the process arguments onto a [`Mode`].
///
/// Unknown arguments are an error rather than being ignored: a typo in a
/// deploy script that silently starts a web server instead of migrating is
/// the failure this guards against.
fn parse_mode<I>(args: I) -> std::result::Result<Mode, String>
where
    I: IntoIterator<Item = String>,
{
    let mut mode = Mode::Serve;
    for arg in args {
        let next = match arg.as_str() {
            "--migrate" => Mode::Migrate,
            "--schedule" => Mode::Schedule,
            "--db-seed" => Mode::Seed,
            "--db-fresh" => Mode::Fresh,
            "--db-reset" => Mode::Reset,
            other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
        };
        if mode != Mode::Serve {
            return Err(format!("`{arg}` conflicts with an earlier flag\n{USAGE}"));
        }
        mode = next;
    }
    Ok(mode)
}

/// The default log filter, used when `RUST_LOG` is unset.
///
/// Two filters, chosen the way the rest of the application chooses: by build
/// profile, not by an environment variable. While developing, this crate and
/// the framework are the two things worth hearing from in detail; in a
/// release build `info` is the level an operator wants and the framework's
/// per-request detail is noise they pay for in log volume.
const LOG_FILTER: &str = if cfg!(debug_assertions) {
    "info,__RUST_NAME__=debug,arcature=debug"
} else {
    "info"
};

/// The usage text shown with an argument error.
const USAGE: &str = "\
usage: __RUST_NAME__ [FLAG]

  (no flag)    bind the port and serve
  --schedule   serve and run the scheduler in the same process
  --migrate    apply pending migrations, then exit
  --db-seed    run the seeders, then exit
  --db-fresh   drop every table, re-run all migrations, then exit
  --db-reset   roll every migration back, then exit";

#[cfg(test)]
mod tests {
    use super::{Mode, parse_mode};

    fn parse(args: &[&str]) -> std::result::Result<Mode, String> {
        parse_mode(args.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn no_arguments_means_serve() {
        assert_eq!(parse(&[]).unwrap(), Mode::Serve);
    }

    #[test]
    fn each_database_flag_selects_its_own_mode() {
        assert_eq!(parse(&["--migrate"]).unwrap(), Mode::Migrate);
        assert_eq!(parse(&["--db-seed"]).unwrap(), Mode::Seed);
        assert_eq!(parse(&["--db-fresh"]).unwrap(), Mode::Fresh);
        assert_eq!(parse(&["--db-reset"]).unwrap(), Mode::Reset);
        assert_eq!(parse(&["--schedule"]).unwrap(), Mode::Schedule);
    }

    #[test]
    fn two_mode_flags_are_refused_rather_than_silently_ordered() {
        let error = parse(&["--migrate", "--db-seed"]).unwrap_err();
        assert!(error.contains("conflicts"), "{error}");
    }

    #[test]
    fn an_unknown_argument_prints_the_usage() {
        let error = parse(&["--migrat"]).unwrap_err();
        assert!(error.contains("usage:"), "{error}");
    }
}
