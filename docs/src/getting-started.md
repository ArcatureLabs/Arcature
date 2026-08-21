# Getting started

## Requirements

- Rust **1.97.1** or newer (edition 2024). `rust-toolchain.toml` pins `stable`.
- **PostgreSQL 17** for anything using the database or the job queue.
- Node.js, only if you are building a frontend with Vite. Arcature itself
  publishes no npm package.

## Installing

Arcature is not on crates.io yet, so depend on the repository:

```toml
[dependencies]
arcature = { git = "https://github.com/ArcatureLabs/Arcature" }
```

Once it is published, `cargo add arcature` will be the whole install.

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

[Routing](routing.md) for how requests reach handlers, or
[Inertia](inertia.md) if you are building a page-driven frontend.
