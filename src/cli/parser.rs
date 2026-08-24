//! The `arc` command surface, defined with clap.
//!
//! clap owns *parsing* only. Each subcommand still lives in its own
//! `commands/<name>.rs` module, and this file's job is to describe the
//! surface and hand back a [`Subcommand`] the dispatcher can match on. That
//! split is why the builder never calls into a command's executor: a parse
//! failure must be reportable (with a usage message and a suggestion) before
//! anything touches the filesystem or a database.
//!
//! # Why the builder API and not `derive`
//!
//! The crate depends on clap with `default-features = false` and the manifest
//! does not enable clap's `derive` feature, so `#[derive(Parser)]` does not
//! exist in this build. The builder is also the better fit for the `make:*`
//! family: twenty-two near-identical subcommands come out of one loop over
//! [`MakeKind::ALL`] instead of twenty-two hand-written variants that could
//! drift apart.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Arg, ArgAction, ArgMatches, Command};

/// A parsed CLI subcommand.
///
/// The variants mirror the `commands/<name>.rs` modules: each carries the
/// arguments that its command needs to execute. Variants whose command needs
/// a capability the build may not have (`queue`, `doctor`, `key:generate`)
/// are gated on the same features their command modules are.
#[derive(Debug, Clone)]
pub enum Subcommand {
    /// `arc new <name> [--dest <path>] [--stack <s>] [--db <d>]`. Executed in
    /// [`commands::new`](super::commands::new).
    New {
        name: String,
        dest: Option<PathBuf>,
        stack: Stack,
        database: Database,
        install: bool,
    },
    /// `arc install [--ci]`. Executed in
    /// [`commands::install`](super::commands::install).
    Install { ci: bool },
    /// `arc version` (also `--version`, `-V`). Executed in
    /// [`commands::version`](super::commands::version).
    Version,
    /// `arc serve [--bind <addr>] [--port <n>]`. Executed in
    /// [`commands::serve`](super::commands::serve).
    Serve {
        bind: Option<String>,
        port: Option<u16>,
    },
    /// `arc migrate [--dsn <url>]`. Executed in
    /// [`commands::migrate`](super::commands::migrate).
    Migrate { dsn: Option<String> },
    /// `arc schedule [--dsn <url>]`. Executed in
    /// [`commands::schedule`](super::commands::schedule).
    Schedule { dsn: Option<String> },
    /// `arc make:<kind> <name>`. Executed in
    /// [`commands::make`](super::commands::make).
    Make { kind: MakeKind, name: String },
    /// `arc key:generate [--show]`. Executed in
    /// [`commands::key_generate`](super::commands::key_generate). Only
    /// available with the `auth` feature, which owns the key type and the
    /// certified RNG behind it.
    #[cfg(feature = "auth")]
    KeyGenerate { show: bool },
    /// `arc storage:link`. Executed in
    /// [`commands::storage_link`](super::commands::storage_link).
    StorageLink,
    /// `arc db:<seed|fresh|reset> [--dsn <url>] [--force]`. Executed in
    /// [`commands::db`](super::commands::db).
    Db {
        action: DbAction,
        dsn: Option<String>,
        force: bool,
    },
    /// `arc queue [--dsn <url>] <work|drain|stats>`. Executed in
    /// [`commands::queue`](super::commands::queue). Only available with the
    /// `database` + `jobs` features.
    #[cfg(all(feature = "database", feature = "jobs"))]
    Queue {
        action: QueueAction,
        dsn: Option<String>,
    },
    /// `arc doctor`. Executed in [`commands::doctor`](super::commands::doctor).
    /// Only available with the `database` feature.
    #[cfg(feature = "database")]
    Doctor,
    /// `arc dev [--port <n>] [--host <addr>] [--open]`. Executed in
    /// [`commands::dev`](super::commands::dev).
    Dev {
        port: Option<u16>,
        host: Option<String>,
        open: bool,
    },
    /// `arc routes [--json]`. Executed in
    /// [`commands::routes`](super::commands::routes). Reads the application
    /// graph, so it is gated on `uag` the way `queue` and `doctor` are gated
    /// on theirs.
    #[cfg(feature = "uag")]
    Routes { json: bool },
    /// `arc typegen`. Executed in
    /// [`commands::typegen`](super::commands::typegen).
    #[cfg(feature = "uag")]
    Typegen,
    /// `arc build`. Executed in
    /// [`commands::build`](super::commands::build).
    #[cfg(feature = "uag")]
    Build,
}

/// The frontend stack `arc new` scaffolds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stack {
    /// React + Inertia (the certified default).
    #[default]
    React,
    /// Vue + Inertia.
    Vue,
    /// Svelte + Inertia.
    Svelte,
}

impl Stack {
    /// Every accepted value, in help order.
    pub const ALL: &'static [Self] = &[Self::React, Self::Vue, Self::Svelte];

    /// The value as typed on the command line.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::React => "react",
            Self::Vue => "vue",
            Self::Svelte => "svelte",
        }
    }

    /// Parse a `--stack` value. clap's value parser has already restricted
    /// the input, so a miss means [`Stack::ALL`] and the parser disagree.
    fn parse(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.as_str() == value)
    }
}

/// The database driver `arc new` configures.
///
/// SQLite is the default, and the reason is the first five minutes rather
/// than the next five years. `sqlite://storage/<name>.sqlite?mode=rwc` is
/// created by the driver on first connect, so a generated project runs with
/// no server installed, no container started and no credentials to match.
/// PostgreSQL is one flag away and is what a deployment should use; making it
/// the default meant every new project's first act was to fail at
/// `stage: "connect"`, which teaches nothing about Arcature.
///
/// The library's own default feature is still `db-postgres` -- that is a
/// different question, about what `cargo add arcature` compiles, and it is
/// answered in `Cargo.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Database {
    /// PostgreSQL.
    Postgres,
    /// SQLite, created on first connect. The default.
    #[default]
    Sqlite,
    /// MySQL.
    Mysql,
}

impl Database {
    /// Every accepted value, in help order.
    pub const ALL: &'static [Self] = &[Self::Postgres, Self::Sqlite, Self::Mysql];

    /// The value as typed on the command line.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Sqlite => "sqlite",
            Self::Mysql => "mysql",
        }
    }

    /// The Arcature feature that selects this driver, which is what a
    /// generated `Cargo.toml` has to name.
    #[must_use]
    pub fn feature(self) -> &'static str {
        match self {
            Self::Postgres => "db-postgres",
            Self::Sqlite => "db-sqlite",
            Self::Mysql => "db-mysql",
        }
    }

    /// Parse a `--db` value. See [`Stack::parse`] for why a miss is a bug.
    fn parse(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|d| d.as_str() == value)
    }
}

/// The queue action selected on the command line for `arc queue`.
///
/// Gated with the [`Subcommand::Queue`] variant that carries it: without a
/// database and a queue there is no `arc queue`, and an enum nothing can
/// reach is dead weight in the build.
#[cfg(all(feature = "database", feature = "jobs"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueAction {
    /// Claim and run jobs until Ctrl-C.
    Work,
    /// Requeue dead jobs back to pending.
    Drain,
    /// Print pending / running / dead / cancelled counts.
    Stats,
}

/// The database action selected by the `arc db:*` family.
///
/// [`DbAction::Fresh`] and [`DbAction::Reset`] drop data, so dispatch refuses
/// them without `--force`. The flag *is* the confirmation: `arc` never opens
/// an interactive prompt, because these commands are as likely to run in CI
/// as at a terminal, and a prompt there is a hang rather than a safeguard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbAction {
    /// Run the application's seeders.
    Seed,
    /// Drop every table, re-run migrations, then seed.
    Fresh,
    /// Roll every migration back.
    Reset,
}

impl DbAction {
    /// The flag this action forwards to the application's own binary.
    #[must_use]
    pub fn app_flag(self) -> &'static str {
        match self {
            Self::Seed => "--db-seed",
            Self::Fresh => "--db-fresh",
            Self::Reset => "--db-reset",
        }
    }

    /// Whether the action destroys data and therefore requires `--force`.
    #[must_use]
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::Fresh | Self::Reset)
    }

    /// The subcommand name this action was parsed from, for error messages.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Seed => "db:seed",
            Self::Fresh => "db:fresh",
            Self::Reset => "db:reset",
        }
    }
}

/// The kind of artifact `arc make:<kind>` writes.
///
/// One enum rather than twenty-two subcommand variants: every `make:*` command
/// takes the same single `<name>` argument and differs only in which
/// blueprint it renders, so the difference belongs in data, not in the shape
/// of the dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakeKind {
    /// A feature module -- its own directory under `app/modules/`, holding a
    /// controller, a service and a routes block declared in one `module!`.
    Module,
    /// An Axum controller under `app/controllers/`.
    Controller,
    /// A SeaORM model under `app/models/`.
    Model,
    /// A timestamped SeaORM migration under `database/migrations/`.
    Migration,
    /// A validated request payload under `app/requests/`.
    Request,
    /// A browser-safe response resource under `app/resources/`.
    Resource,
    /// An authorization policy under `app/policies/`.
    Policy,
    /// A service under `app/services/`.
    Service,
    /// A background job under `app/jobs/`.
    Job,
    /// An in-process event under `app/events/`.
    Event,
    /// An event listener under `app/listeners/`.
    Listener,
    /// A middleware function under `app/middleware/`.
    Middleware,
    /// An application command under `app/commands/`.
    Command,
    /// An Inertia page props struct under `app/pages/`.
    Page,
    /// An integration test under `tests/`.
    Test,
    /// A model factory under `database/factories/`.
    Factory,
    /// A database seeder under `database/seeders/`.
    Seeder,
    /// A multi-channel notification under `app/notifications/`.
    Notification,
    /// A mailable under `app/mail/`.
    Mail,
    /// A server-rendered view under `app/views/`, and its template.
    View,
    /// An upload endpoint under `app/controllers/`.
    Upload,
    /// A sign-in flow under `app/auth/`, and the table behind it.
    Auth,
}

impl MakeKind {
    /// Every kind, in the order they appear in `arc --help`.
    pub const ALL: &'static [Self] = &[
        Self::Module,
        Self::Controller,
        Self::Model,
        Self::Migration,
        Self::Request,
        Self::Resource,
        Self::Policy,
        Self::Service,
        Self::Job,
        Self::Event,
        Self::Listener,
        Self::Middleware,
        Self::Command,
        Self::Page,
        Self::Test,
        Self::Factory,
        Self::Seeder,
        Self::Notification,
        Self::Mail,
        Self::View,
        Self::Upload,
        Self::Auth,
    ];

    /// The bare kind name, as it appears after `make:`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Controller => "controller",
            Self::Model => "model",
            Self::Migration => "migration",
            Self::Request => "request",
            Self::Resource => "resource",
            Self::Policy => "policy",
            Self::Service => "service",
            Self::Job => "job",
            Self::Event => "event",
            Self::Listener => "listener",
            Self::Middleware => "middleware",
            Self::Command => "command",
            Self::Page => "page",
            Self::Test => "test",
            Self::Factory => "factory",
            Self::Seeder => "seeder",
            Self::Notification => "notification",
            Self::Mail => "mail",
            Self::View => "view",
            Self::Upload => "upload",
            Self::Auth => "auth",
        }
    }

    /// The full subcommand name (`make:controller`).
    ///
    /// Spelled out rather than `format!("make:{}", ..)` because clap's
    /// `Command::new` takes a `&'static str` in this build: the manifest does
    /// not enable clap's `string` feature, so an owned `String` cannot become
    /// a command name without leaking it. The `as_str` / `subcommand_name`
    /// pair is checked against each other by a test rather than by the
    /// compiler.
    #[must_use]
    pub fn subcommand_name(self) -> &'static str {
        match self {
            Self::Module => "make:module",
            Self::Controller => "make:controller",
            Self::Model => "make:model",
            Self::Migration => "make:migration",
            Self::Request => "make:request",
            Self::Resource => "make:resource",
            Self::Policy => "make:policy",
            Self::Service => "make:service",
            Self::Job => "make:job",
            Self::Event => "make:event",
            Self::Listener => "make:listener",
            Self::Middleware => "make:middleware",
            Self::Command => "make:command",
            Self::Page => "make:page",
            Self::Test => "make:test",
            Self::Factory => "make:factory",
            Self::Seeder => "make:seeder",
            Self::Notification => "make:notification",
            Self::Mail => "make:mail",
            Self::View => "make:view",
            Self::Upload => "make:upload",
            Self::Auth => "make:auth",
        }
    }

    /// The one-line `--help` summary. Static for the same reason as
    /// [`MakeKind::subcommand_name`].
    #[must_use]
    pub fn about(self) -> &'static str {
        match self {
            Self::Module => "Generate a feature module in app/modules",
            Self::Controller => "Generate a controller in app/controllers",
            Self::Model => "Generate a model in app/models",
            Self::Migration => "Generate a timestamped migration in database/migrations",
            Self::Request => "Generate a validated request in app/requests",
            Self::Resource => "Generate a response resource in app/resources",
            Self::Policy => "Generate an authorization policy in app/policies",
            Self::Service => "Generate a service in app/services",
            Self::Job => "Generate a background job in app/jobs",
            Self::Event => "Generate an event in app/events",
            Self::Listener => "Generate an event listener in app/listeners",
            Self::Middleware => "Generate a middleware in app/middleware",
            Self::Command => "Generate an application command in app/commands",
            Self::Page => "Generate an Inertia page props struct in app/pages",
            Self::Test => "Generate an integration test in tests",
            Self::Factory => "Generate a model factory in database/factories",
            Self::Seeder => "Generate a database seeder in database/seeders",
            Self::Notification => "Generate a notification in app/notifications",
            Self::Mail => "Generate a mailable in app/mail",
            Self::View => "Generate a view in app/views and its template",
            Self::Upload => "Generate an upload controller in app/controllers",
            Self::Auth => "Generate a sign-in flow in app/auth and its migration",
        }
    }

    /// Resolve a `make:<kind>` subcommand name back to its kind.
    fn from_subcommand_name(name: &str) -> Option<Self> {
        let kind = name.strip_prefix("make:")?;
        Self::ALL.iter().copied().find(|k| k.as_str() == kind)
    }
}

/// Build the `arc` command surface.
///
/// Public so tests can assert against the surface itself. There is exactly
/// one definition, so `arc --help` and the parser cannot drift apart.
#[must_use]
pub fn command() -> Command {
    let mut cmd = Command::new("arc")
        .about("Arcature: an opinionated full-stack Rust web framework")
        // clap's own version flag would print `arc <version>`, but `arc` has
        // always reported the *framework* version. The flag is handled by
        // hand and routed to the `version` command so the two agree.
        .disable_version_flag(true)
        .arg_required_else_help(true)
        .arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::SetTrue)
                .help("Print the framework version"),
        )
        .subcommand(new_subcommand())
        .subcommand(
            Command::new("install")
                .about("Install the frontend's npm dependencies")
                .arg(
                    Arg::new("ci")
                        .long("ci")
                        .action(ArgAction::SetTrue)
                        .help("Use `npm ci`, which enforces package-lock.json"),
                ),
        )
        .subcommand(Command::new("version").about("Print the framework version"))
        .subcommand(
            Command::new("serve")
                .about("Run the current application")
                .arg(
                    Arg::new("bind")
                        .long("bind")
                        .value_name("ADDR")
                        .help("Address to bind (forwarded as ARCATURE_BACKEND_BIND)"),
                )
                .arg(
                    Arg::new("port")
                        .long("port")
                        .value_name("PORT")
                        .value_parser(clap::value_parser!(u16))
                        .help("Port to bind (forwarded as ARCATURE_BACKEND_PORT)"),
                ),
        )
        .subcommand(
            Command::new("migrate")
                .about("Run pending migrations")
                .arg(dsn_arg()),
        )
        .subcommand(
            Command::new("schedule")
                .about("Run the job scheduler")
                .arg(dsn_arg()),
        )
        .subcommand(
            Command::new("storage:link").about("Link storage/app/public into public/storage"),
        )
        .subcommand(db_subcommand(
            DbAction::Seed,
            "Run the application's seeders",
        ))
        .subcommand(db_subcommand(
            DbAction::Fresh,
            "Drop every table, re-migrate, then seed (destructive)",
        ))
        .subcommand(db_subcommand(
            DbAction::Reset,
            "Roll every migration back (destructive)",
        ))
        .subcommand(
            Command::new("dev")
                .about("Run the development server: one port, Vite and the app behind it")
                .arg(
                    Arg::new("port")
                        .long("port")
                        .value_name("PORT")
                        .value_parser(clap::value_parser!(u16))
                        // Named the way it is because there is exactly one:
                        // Vite does not get a second port under `arc dev`.
                        .help("The one TCP port to serve on (default 3000)"),
                )
                .arg(
                    Arg::new("host")
                        .long("host")
                        .value_name("ADDR")
                        .help("Address to bind (default 127.0.0.1)"),
                )
                .arg(
                    Arg::new("open")
                        .long("open")
                        .action(ArgAction::SetTrue)
                        .help("Open a browser once the first build is serving"),
                ),
        );

    #[cfg(feature = "uag")]
    {
        cmd = cmd
            .subcommand(
                Command::new("routes")
                    .about("List every route the application declares")
                    .arg(
                        Arg::new("json")
                            .long("json")
                            .action(ArgAction::SetTrue)
                            .help("Emit the route list as JSON instead of a table"),
                    ),
            )
            .subcommand(Command::new("typegen").about("Emit TypeScript from the application graph"))
            .subcommand(
                Command::new("build").about("Build for production: typegen, cargo, then Vite"),
            );
    }

    for kind in MakeKind::ALL {
        cmd = cmd.subcommand(
            Command::new(kind.subcommand_name())
                .about(kind.about())
                .arg(
                    Arg::new("name")
                        .required(true)
                        .help("The artifact name (e.g. `user`, `User`, `users/show`)"),
                ),
        );
    }

    #[cfg(feature = "auth")]
    {
        cmd = cmd.subcommand(
            Command::new("key:generate")
                .about("Generate a 64-byte application key")
                .arg(
                    Arg::new("show")
                        .long("show")
                        .action(ArgAction::SetTrue)
                        .help("Print the key instead of writing it to .env"),
                ),
        );
    }

    #[cfg(all(feature = "database", feature = "jobs"))]
    {
        cmd = cmd.subcommand(
            Command::new("queue")
                .about("Operate on the job queue")
                .arg(
                    Arg::new("action")
                        .required(true)
                        .value_parser(["work", "drain", "stats"])
                        .help("The queue action"),
                )
                .arg(dsn_arg()),
        );
    }

    #[cfg(feature = "database")]
    {
        cmd = cmd.subcommand(Command::new("doctor").about("Diagnose the local environment"));
    }

    cmd
}

/// The `arc new` subcommand. Split out because its arguments would otherwise
/// bury the shape of [`command`].
fn new_subcommand() -> Command {
    let stacks: Vec<&'static str> = Stack::ALL.iter().map(|s| s.as_str()).collect();
    let drivers: Vec<&'static str> = Database::ALL.iter().map(|d| d.as_str()).collect();

    Command::new("new")
        .about("Generate a new application")
        .arg(Arg::new("name").required(true).help("The project name"))
        .arg(
            Arg::new("dest")
                .long("dest")
                .value_name("PATH")
                .value_parser(clap::value_parser!(PathBuf))
                .help("Where to write the project (defaults to ./<name>)"),
        )
        .arg(
            Arg::new("stack")
                .long("stack")
                .value_name("STACK")
                .value_parser(stacks)
                .default_value(Stack::default().as_str())
                .help("The frontend stack"),
        )
        .arg(
            Arg::new("no-install")
                .long("no-install")
                .action(ArgAction::SetTrue)
                .help("Skip `npm install`; run `arc install` yourself later"),
        )
        .arg(
            Arg::new("db")
                .long("db")
                .value_name("DRIVER")
                .value_parser(drivers)
                .default_value(Database::default().as_str())
                .help("The database driver"),
        )
}

/// The `--dsn <url>` option, shared by every command that talks to a database.
fn dsn_arg() -> Arg {
    Arg::new("dsn")
        .long("dsn")
        .value_name("URL")
        .help("Database URL (defaults to DATABASE_URL)")
}

/// One `db:*` subcommand. All three take the same two options; only the
/// destructive pair actually requires `--force`, and that is enforced at
/// dispatch so the refusal can name the command that was refused.
fn db_subcommand(action: DbAction, about: &'static str) -> Command {
    Command::new(action.as_str())
        .about(about)
        .arg(dsn_arg())
        .arg(
            Arg::new("force")
                .long("force")
                .action(ArgAction::SetTrue)
                .help("Confirm a destructive operation (never prompts)"),
        )
}

/// Parse the CLI arguments into a [`Subcommand`].
///
/// # Errors
///
/// Returns the [`clap::Error`] clap produced. Callers must respect
/// [`clap::Error::use_stderr`]: `--help` arrives here as an "error" that is
/// really a successful render to stdout.
pub fn parse(args: &[OsString]) -> Result<Subcommand, clap::Error> {
    let matches = command().try_get_matches_from(args)?;
    from_matches(&matches)
}

/// Map clap's matches onto the dispatcher's enum.
fn from_matches(matches: &ArgMatches) -> Result<Subcommand, clap::Error> {
    if matches.get_flag("version") {
        return Ok(Subcommand::Version);
    }

    let Some((name, sub)) = matches.subcommand() else {
        // `arg_required_else_help` covers a bare `arc`; this is the leftover
        // case of flags without a subcommand.
        return Err(command().error(
            clap::error::ErrorKind::MissingSubcommand,
            "a subcommand is required",
        ));
    };

    if let Some(kind) = MakeKind::from_subcommand_name(name) {
        return Ok(Subcommand::Make {
            kind,
            name: string_of(sub, "name"),
        });
    }
    Ok(match name {
        "new" => Subcommand::New {
            name: string_of(sub, "name"),
            dest: sub.get_one::<PathBuf>("dest").cloned(),
            stack: Stack::parse(&string_of(sub, "stack")).unwrap_or_default(),
            database: Database::parse(&string_of(sub, "db")).unwrap_or_default(),
            install: !sub.get_flag("no-install"),
        },
        "install" => Subcommand::Install {
            ci: sub.get_flag("ci"),
        },
        "version" => Subcommand::Version,
        "serve" => Subcommand::Serve {
            bind: sub.get_one::<String>("bind").cloned(),
            port: sub.get_one::<u16>("port").copied(),
        },
        "dev" => Subcommand::Dev {
            port: sub.get_one::<u16>("port").copied(),
            host: sub.get_one::<String>("host").cloned(),
            open: sub.get_flag("open"),
        },
        #[cfg(feature = "uag")]
        "routes" => Subcommand::Routes {
            json: sub.get_flag("json"),
        },
        #[cfg(feature = "uag")]
        "typegen" => Subcommand::Typegen,
        #[cfg(feature = "uag")]
        "build" => Subcommand::Build,
        "migrate" => Subcommand::Migrate { dsn: dsn_of(sub) },
        "schedule" => Subcommand::Schedule { dsn: dsn_of(sub) },
        "storage:link" => Subcommand::StorageLink,
        "db:seed" => db_of(DbAction::Seed, sub),
        "db:fresh" => db_of(DbAction::Fresh, sub),
        "db:reset" => db_of(DbAction::Reset, sub),
        #[cfg(feature = "auth")]
        "key:generate" => Subcommand::KeyGenerate {
            show: sub.get_flag("show"),
        },
        #[cfg(all(feature = "database", feature = "jobs"))]
        "queue" => Subcommand::Queue {
            action: match string_of(sub, "action").as_str() {
                "work" => QueueAction::Work,
                "drain" => QueueAction::Drain,
                // The value parser already rejected everything else.
                _ => QueueAction::Stats,
            },
            dsn: dsn_of(sub),
        },
        #[cfg(feature = "database")]
        "doctor" => Subcommand::Doctor,
        // clap rejects unknown subcommands before this point and every
        // declared subcommand is handled above, so this arm only fires if
        // `command()` gains a subcommand and this match does not.
        other => {
            return Err(command().error(
                clap::error::ErrorKind::InvalidSubcommand,
                format!("unhandled subcommand: {other}"),
            ));
        }
    })
}

/// Read a required string argument. clap guarantees presence for arguments
/// that are `.required(true)` or carry a default, so a miss is a bug in
/// [`command`].
fn string_of(matches: &ArgMatches, id: &str) -> String {
    matches
        .get_one::<String>(id)
        .cloned()
        .unwrap_or_else(|| unreachable!("`{id}` is required or defaulted in `command()`"))
}

/// Read the shared `--dsn` option.
fn dsn_of(matches: &ArgMatches) -> Option<String> {
    matches.get_one::<String>("dsn").cloned()
}

/// Build a [`Subcommand::Db`] from a `db:*` subcommand's matches.
fn db_of(action: DbAction, matches: &ArgMatches) -> Subcommand {
    Subcommand::Db {
        action,
        dsn: dsn_of(matches),
        force: matches.get_flag("force"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(argv: &[&str]) -> Vec<OsString> {
        std::iter::once("arc")
            .chain(argv.iter().copied())
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn the_command_surface_is_internally_consistent() {
        command().debug_assert();
    }

    #[test]
    fn a_bare_version_flag_reports_the_framework_version() {
        assert!(matches!(
            parse(&args(&["--version"])).expect("parses"),
            Subcommand::Version
        ));
        assert!(matches!(
            parse(&args(&["-V"])).expect("parses"),
            Subcommand::Version
        ));
        assert!(matches!(
            parse(&args(&["version"])).expect("parses"),
            Subcommand::Version
        ));
    }

    #[test]
    fn no_install_opts_out_of_the_automatic_npm_install() {
        let Subcommand::New { install, .. } =
            parse(&args(&["new", "blog", "--no-install"])).expect("parses")
        else {
            panic!("expected `new`");
        };
        assert!(!install);
    }

    #[test]
    fn install_defaults_to_npm_install_and_ci_is_opt_in() {
        let Subcommand::Install { ci } = parse(&args(&["install"])).expect("parses") else {
            panic!("expected `install`");
        };
        assert!(!ci);
        let Subcommand::Install { ci } = parse(&args(&["install", "--ci"])).expect("parses") else {
            panic!("expected `install`");
        };
        assert!(ci);
    }

    #[test]
    fn new_defaults_to_a_stack_and_a_driver_that_need_no_server() {
        let Subcommand::New {
            name,
            dest,
            stack,
            database,
            install,
        } = parse(&args(&["new", "blog"])).expect("parses")
        else {
            panic!("expected `new`");
        };
        assert_eq!(name, "blog");
        assert_eq!(dest, None);
        assert_eq!(stack, Stack::React);
        // SQLite, not PostgreSQL. `sqlite://storage/<name>.sqlite?mode=rwc`
        // is created by the driver on first connect, so a generated project
        // boots with nothing installed. PostgreSQL as the default made every
        // new project's first act a failure at `stage: "connect"`.
        assert_eq!(database, Database::Sqlite);
        // A generated project should be runnable without a second command,
        // so the install is the default and --no-install is the opt-out.
        assert!(install);
    }

    #[test]
    fn new_accepts_an_explicit_stack_and_driver() {
        let Subcommand::New {
            stack, database, ..
        } = parse(&args(&[
            "new", "blog", "--stack", "svelte", "--db", "sqlite",
        ]))
        .expect("parses")
        else {
            panic!("expected `new`");
        };
        assert_eq!(stack, Stack::Svelte);
        assert_eq!(database, Database::Sqlite);
    }

    #[test]
    fn new_rejects_a_stack_the_framework_does_not_ship() {
        assert!(parse(&args(&["new", "blog", "--stack", "angular"])).is_err());
    }

    #[test]
    fn serve_rejects_a_port_outside_the_u16_range() {
        assert!(parse(&args(&["serve", "--port", "70000"])).is_err());
        let Subcommand::Serve { port, bind } =
            parse(&args(&["serve", "--port", "8080", "--bind", "0.0.0.0"])).expect("parses")
        else {
            panic!("expected `serve`");
        };
        assert_eq!(port, Some(8080));
        assert_eq!(bind.as_deref(), Some("0.0.0.0"));
    }

    #[test]
    fn every_make_kind_has_a_subcommand() {
        for kind in MakeKind::ALL {
            let parsed = parse(&args(&[kind.subcommand_name(), "widget"])).expect("parses");
            let Subcommand::Make { kind: got, name } = parsed else {
                panic!("expected `make`");
            };
            assert_eq!(got, *kind);
            assert_eq!(name, "widget");
        }
    }

    #[test]
    fn a_make_command_without_a_name_is_a_parse_error() {
        assert!(parse(&args(&["make:controller"])).is_err());
    }

    #[test]
    fn the_destructive_db_commands_carry_their_force_flag() {
        let Subcommand::Db { action, force, .. } =
            parse(&args(&["db:fresh", "--force"])).expect("parses")
        else {
            panic!("expected `db`");
        };
        assert_eq!(action, DbAction::Fresh);
        assert!(force);
        assert!(action.is_destructive());

        let Subcommand::Db { action, force, .. } = parse(&args(&["db:seed"])).expect("parses")
        else {
            panic!("expected `db`");
        };
        assert_eq!(action, DbAction::Seed);
        assert!(!force);
        assert!(!action.is_destructive());
    }

    #[test]
    fn dev_runs_on_one_port_that_defaults_rather_than_being_asked_for() {
        let Subcommand::Dev { port, host, open } = parse(&args(&["dev"])).expect("parses") else {
            panic!("expected `dev`");
        };
        assert_eq!(port, None, "the default belongs to the command, not clap");
        assert_eq!(host, None);
        assert!(!open);
    }

    #[test]
    fn dev_takes_the_port_the_developer_names() {
        let Subcommand::Dev { port, host, open } = parse(&args(&[
            "dev", "--port", "4173", "--host", "0.0.0.0", "--open",
        ]))
        .expect("parses") else {
            panic!("expected `dev`");
        };
        assert_eq!(port, Some(4173));
        assert_eq!(host.as_deref(), Some("0.0.0.0"));
        assert!(open);
    }

    #[cfg(feature = "uag")]
    #[test]
    fn routes_prints_a_table_unless_json_is_asked_for() {
        let Subcommand::Routes { json } = parse(&args(&["routes"])).expect("parses") else {
            panic!("expected `routes`");
        };
        assert!(!json, "the human table is the default");

        let Subcommand::Routes { json } = parse(&args(&["routes", "--json"])).expect("parses")
        else {
            panic!("expected `routes`");
        };
        assert!(json);
    }

    #[cfg(feature = "uag")]
    #[test]
    fn typegen_and_build_take_no_arguments() {
        assert!(matches!(
            parse(&args(&["typegen"])).expect("parses"),
            Subcommand::Typegen
        ));
        assert!(matches!(
            parse(&args(&["build"])).expect("parses"),
            Subcommand::Build
        ));
    }

    #[cfg(all(feature = "database", feature = "jobs"))]
    #[test]
    fn queue_takes_its_action_in_either_position() {
        for argv in [
            vec!["queue", "drain", "--dsn", "postgres://x"],
            vec!["queue", "--dsn", "postgres://x", "drain"],
        ] {
            let Subcommand::Queue { action, dsn } = parse(&args(&argv)).expect("parses") else {
                panic!("expected `queue`");
            };
            assert_eq!(action, QueueAction::Drain);
            assert_eq!(dsn.as_deref(), Some("postgres://x"));
        }
    }

    #[cfg(all(feature = "database", feature = "jobs"))]
    #[test]
    fn queue_rejects_an_action_it_does_not_have() {
        assert!(parse(&args(&["queue", "explode"])).is_err());
    }

    #[test]
    fn help_is_reported_as_a_stdout_render_not_a_failure() {
        let error = parse(&args(&["--help"])).expect_err("help short-circuits");
        assert!(!error.use_stderr());
    }

    #[test]
    fn an_unknown_subcommand_is_a_stderr_failure() {
        let error = parse(&args(&["nope"])).expect_err("unknown subcommand");
        assert!(error.use_stderr());
    }
}
