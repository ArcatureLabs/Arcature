# Getting started

## Requirements

- Rust **1.97.1** or newer (edition 2024). `rust-toolchain.toml` pins `stable`.
- **Node.js**, for the frontend a generated project ships with.

That is all. A generated project uses SQLite by default, and the driver
creates the file on first connect — there is no server to install, no
container to start and no credentials to match. PostgreSQL 17 or MySQL 8 are
needed only if you ask for one with `arc new --db postgres`.

## Installing the `arc` CLI

Three routes to the same binary. The first is the one to use.

**Download it.** Take the archive for your platform from the
[releases page](https://github.com/ArcatureLabs/Arcature/releases/latest),
unpack it, and put `arc` on your `PATH`. Linux, macOS and Windows, x86-64 and
arm64, each with a `.sha256`. No toolchain, no compile, seconds.

**Compile it.**

```sh
cargo install arcature --features cli --locked
```

A release build of the whole framework, sea-orm and sqlx included; minutes.

`--locked` is not optional. Without it Cargo ignores the published lockfile and
re-resolves, which currently selects a `sea-schema` that fails to build with
`can't find crate for async_trait`.

**With [`cargo binstall`](https://github.com/cargo-bins/cargo-binstall), if you
already have it.** `cargo binstall arcature` fetches the same prebuilt binary
the releases page serves. It is worth having if you install Rust binaries
often, and it is not worth acquiring for this one: `cargo install
cargo-binstall` compiles several hundred crates, which is more than compiling
Arcature would have cost.

Check it:

```console
$ arc version
arcature 0.1.3
```

## Using Arcature as a library

To add the framework to a crate you already have, rather than generating one:

```sh
cargo add arcature
```

To follow `main` ahead of a release instead, depend on the repository and pin a
revision — a branch reference will move under you.

```toml
[dependencies]
arcature = { git = "https://github.com/ArcatureLabs/Arcature", rev = "..." }
```

## The smallest application

```rust,ignore
use arcature::application::EngineResult;
use arcature::prelude::*;

#[arcature::main]
async fn main() -> EngineResult<()> {
    Application::new()
        .routes(Routes::new([Route::get("/", index).name("home")]))
        .build()
        .run()
        .await
}

async fn index() -> Result<Response> {
    Ok(text(StatusCode::OK, "hello"))
}
```

Three things to notice.

`.build()` is required. `Application::new()` returns an `ApplicationBuilder`;
`.run()` lives on `Application`. Forgetting `.build()` is a type error, not a
runtime surprise.

`run()` returns `EngineResult<()>`, not the framework's `Result<()>`, and
`EngineResult` is not in the prelude — it lives at
`arcature::application::EngineResult`. Engine failures (a port already bound, a
database that will not connect) are a different kind of failure from a
handler's, and they deliberately do not share an error type.

Handlers return `Result<Response>`, where `Result` is Arcature's. `text`,
`json`, `redirect` and `no_content` build the common shapes.

## The generated application

```console
$ arc new blog --stack react --db postgres
Created blog at blog
  installing frontend dependencies (npm install)...

Next:
  cd blog
  arc key:generate     # writes APP_KEY into .env
  arc dev              # one port, backend and Vite together
```

A project is two halves, and only one of them is Rust. `cargo` resolves the
crates on the first build; the frontend needs `npm install` run once, and
`arc new` runs it. Pass `--no-install` to skip that and run `arc install`
yourself later — `arc dev` refuses to start without it and
says so, rather than letting Node fail on a missing `vite`.

`arc new` writes a Laravel-shaped project rather than a single file:

```text
app/
  controllers/   models/     services/
  requests/      policies/   resources/
bootstrap/
  app.rs         state.rs
config/
database/migrations/
routes/mod.rs
resources/js/    resources/css/
public/
storage/
src/main.rs      src/lib.rs
tests/smoke.rs
.env
```

`bootstrap/app.rs` is the composition root. It loads `.env`, reads typed
configuration, and wires the subsystems:

```rust,ignore
pub fn app() -> Result<Application<crate::bootstrap::AppState>> {
    dotenvy::dotenv().ok();
    let config = crate::config::load()?;
    Ok(Application::new()
        .routes(crate::routes::routes())
        .bind(&config.bind_addr)
        .port(config.port)
        .database(config.database)
        .cache(config.cache)
        .storage(config.storage)
        .mail(config.mail)
        .jobs(jobs_registry())
        .build())
}
```

`bootstrap/state.rs` defines `AppState`, the cloneable bundle every handler
reaches through `State<AppState>`. Each field is an `Option`, because a
subsystem that was never configured contributes `None` rather than a panic:

```rust,ignore
#[derive(Clone)]
pub struct AppState {
    pub db: Option<Db>,
    pub jobs: Option<Jobs>,
    pub cache: Option<Cache>,
    pub storage: Option<Storage>,
    pub mail: Option<Mailer>,
}
```

The state is produced *after* startup, from the started `Resources`, which is
why it is a closure rather than a value:

```rust,ignore
pub fn state_fn() -> Arc<dyn Fn(&Resources, &Lifecycle) -> AppState + Send + Sync> {
    Arc::new(|res, _lc| AppState {
        db: res.db().cloned(),
        jobs: res.jobs().cloned(),
        cache: res.cache().cloned(),
        storage: res.storage().cloned(),
        mail: res.mail().cloned(),
    })
}
```

`src/lib.rs` puts the two together with `run_with_state`.

## Features

Arcature's features reduce the compile surface; they are not a self-assembly
kit. `default` is a working full-stack application. Turn features *off* to
compile less, not on to reach a usable state.

```toml
# The whole framework.
arcature = { git = "...", features = ["fullstack"] }

# An API server: no Inertia, no static assets pipeline.
arcature = { git = "...", default-features = false, features = ["api", "database", "auth", "validation"] }
```

The database driver is split three ways — `db-postgres`, `db-sqlite`,
`db-mysql` — so a SQLite application does not compile the PostgreSQL protocol.
The job queue requires PostgreSQL.

## Next

[Your first module](modules.md) to add a feature to the application you just
generated, [Routing](routing.md) for how requests reach handlers, or
[Inertia](inertia.md) if you are building a page-driven frontend.
