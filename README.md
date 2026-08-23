# Arcature

[![CI](https://github.com/ArcatureLabs/Arcature/actions/workflows/ci.yml/badge.svg)](https://github.com/ArcatureLabs/Arcature/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust 1.97.1+](https://img.shields.io/badge/rust-1.97.1%2B-orange.svg)](rust-toolchain.toml)

An opinionated full-stack Rust web framework. One package, batteries included.

Arcature integrates proven wheels -- Axum, Tower, Tokio, SeaORM, SQLx,
Inertia.js, OpenDAL, lettre, tracing -- and owns what sits between them: the
application lifecycle, the request pipeline order, the conventions, and a
coherent vocabulary. The raw Axum, Tower, SeaORM and SQLx escape hatches stay
available for when the framework's opinions run out.

**Status: pre-release.** `main` breaks without notice, and `0.x` says so
deliberately. The `2026.x` versions on crates.io are from an abandoned
predecessor repository, are yanked, and share nothing with this one but the
name -- see [Versioning](#versioning). Where a subsystem is
narrower than its name suggests, that is said in
[the guide](https://arcaturelabs.github.io/Arcature/) and summarised under
[What is not built yet](#what-is-not-built-yet).

## Writing Arcature with an AI assistant

[`SKILL.md`](SKILL.md) is one file written to be pasted into a coding
assistant before you ask it for Arcature code. Grab it raw:

```
https://raw.githubusercontent.com/ArcatureLabs/Arcature/main/SKILL.md
```

It is not a tutorial. Most of it is the set of things a model gets wrong from
API names alone: that eighteen of forty-one features are on and the rest are
not, that `fullstack` is not all of them, that `body_limit` and `timeout` are
unset by default, that `arc queue work` dispatches nothing, that redaction does
not reach span attributes. An assistant that has read it stops generating code
that compiles and is wrong.

## Install

```sh
cargo add arcature
```

`0.x` means a minor bump is the breaking one, so `arcature = "0.1"` will not
carry you to `0.2` -- see [Versioning](#versioning). To follow `main` ahead of
a release instead, take a git dependency and pin a revision; a branch reference
will move under you.

Requirements: Rust **1.97.1** or newer (edition 2024). Anything that uses the
database or the job queue needs one of PostgreSQL 17, MySQL 8 or SQLite --
picked at build time, one per build.

### Optional subsystems

`cargo add arcature` gives you routing, Inertia, SeaORM, auth, validation,
cache, storage, mail, jobs, events, observability and realtime. The
capabilities below are **off by default** and named individually, because each
one costs something a build that does not use it should not pay -- a parser on
the request path, a table and a migration, or a cipher somebody then has to
own a rotation story for. The reasoning for each is written above it in
`Cargo.toml`; the short version:

| Feature | What you get |
|---|---|
| `auth-flows` | Sign-in decisions that are wrong in ways nothing reports: constant-time account lookup, login throttling, password confirmation |
| `auth-reset` | One-time password-reset links, stored as a digest |
| `auth-remember` | Rotating remember-me tokens, with theft detection |
| `api-tokens` | Hashed bearer tokens for clients with no cookie -- a CLI, a CI job, another service |
| `crypt` | `Encrypter`: XChaCha20-Poly1305 over a versioned, self-describing token |
| `signed-urls` | `UrlSigner`: links that carry their own origin proof and deadline |
| `uploads` | Multipart bodies, filename sanitizing, content-addressed names, magic-byte sniffing |
| `views` | Compiled HTML views via Askama -- no expression evaluator in the request path |
| `i18n` | Fluent catalogs, locale negotiation against a whitelist |
| `session-store-db` | Sessions in your database, so a deploy is not a mass logout |
| `notifications` | One event, one recipient, many channels |
| `notifications-db` | The in-app inbox channel |
| `notifications-broadcast` | The live-push channel, over WebSocket/SSE |
| `notifications-queue` | Mail delivery handed to the job queue instead of the request |
| `otel`, `storage-s3`, `oauth` | OTLP traces, S3-compatible object storage, OAuth 2 with PKCE |

Five bring a table and a migration: `auth-reset`, `auth-remember`,
`api-tokens`, `notifications-db`, `session-store-db`.

`fullstack` is not "everything", despite the name. It adds `uploads`, `views`,
`storage-s3` and the tooling features (`cli`, `templates`, `dev-proxy`, `uag`)
to the defaults, and leaves every row above it off -- so a `fullstack` build
still has no password-reset table, no encrypter and no notifications. Name
what you want. The [upgrade note](docs/src/upgrade.md) says what each feature
costs and, for `auth-reset`, what it deliberately does not cover.

## Quick start

```rust
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

`.build()` turns the `ApplicationBuilder` into an `Application`; `run()` lives
on the latter. `run()` returns `EngineResult<()>` -- engine failures (a bound
port, a database that will not connect) are a different kind of failure from a
handler's, and deliberately do not share an error type with `Result<Response>`.

To scaffold a whole Laravel-shaped project instead:

```sh
cargo install arcature --features cli
arc new my-app
cd my-app
cargo run
```

## What Arcature is

One package. One release unit. One version. Features exist only to reduce
compile surface, never to turn the framework into a self-assembly kit. The
default feature set compiles the generated application with no extra flags.

`#![forbid(unsafe_code)]` applies to the whole crate.

Two decisions shape everything else:

**No hidden registry.** No `inventory`, no `linkme`, no `TypeId` map, no
thread-locals. All framework metadata is `&'static` const data emitted by
macros and named by code you wrote, so `cargo expand` and "go to definition"
are enough to find out what is wired up.

**No npm package.** Arcature publishes no JavaScript. Applications use the
official `@inertiajs/*` adapters, and everything the Rust side hands the
browser travels as generated `.ts` files in the application's own tree rather
than through a framework runtime behind a virtual module.

## The request pipeline

Layer order is a contract, written down in `src/application/pipeline.rs` and
asserted by the test suite. `.inertia()` before `.csrf()` and `.csrf()` before
`.inertia()` produce the same pipeline. Outermost first:

```text
 1 DevProxy      7 CORS          13 Timeout       19 PageContracts
 2 Proxy         8 RequestId     14 Maintenance   20 RedirectMapper
 3 Health        9 AccessLog     15 RateLimit     21 user .layer()s
 4 UagEndpoint  10 CatchPanic    16 Session       22 Router
 5 Compression  11 ErrorMapping  17 CSRF          23 StaticFiles
 6 SecurityHdrs 12 BodyLimit     18 Inertia
```

Stages 5 through 21 are off unless asked for, with one exception: stage 20 is
on unless refused, because `redirect().route(..)` silently doing nothing is a
worse default than one extension lookup per response. The reasoning for each
position is in the module documentation and in
[ADR 0004](docs/decisions/0004-layer-order-contract.md).

## Architecture

| Subsystem | Built on | Feature |
|---|---|---|
| HTTP routing | Axum 0.8, Tower 0.5, tower-http 0.6 | always on |
| Async runtime, `#[arcature::main]` | Tokio 1.53 | `macros` |
| The DSL and its runtime contracts | -- | `dx` |
| Native Inertia.js v3 (server half) | the protocol, implemented directly | `inertia` |
| Database | SeaORM 2.0 + SQLx 0.9 over one pool | `database` + one `db-*` |
| Auth, sessions, CSRF, policies | argon2, tower-sessions, secrecy | `auth` |
| Validation | validator 0.21 | `validation` |
| Cache | Redis/Valkey (redis 1.5) | `cache` |
| Storage | OpenDAL 0.58 | `storage-fs`, `storage-s3` |
| Mail | lettre 0.11 (rustls) | `mail` |
| Jobs | Database-backed queue, one claim strategy per dialect | `jobs` |
| Events | in-process dispatch | `events` |
| Realtime | WebSocket + SSE over axum, fan-out within one process | `realtime` |
| Problem Details (RFC 9457), OpenAPI | -- | `api` |
| Observability | tracing, request ids, JSON logs, Prometheus text, W3C trace context | `observe` |
| Static pages and assets | tower-http `fs` | `pages` |
| The `arc` CLI and templates | clap 4 | `cli`, `templates` |
| Compiled HTML views | askama 0.16 (no runtime parser, so no SSTI) | `views` |

Operator opt-ins stay off by default: `otel` (OpenTelemetry over OTLP),
`api-docs` (an interactive API reference is a map of the attack surface),
`oauth`, `storage-s3`, `dev-proxy`, `uag`, `test-kit`, `views`.

Database drivers are separate features so a SQLite user does not compile the
PostgreSQL protocol. `database` on its own brings the crates but no driver;
exactly one of `db-postgres` / `db-sqlite` / `db-mysql` belongs in a build.
`default` picks `db-postgres`.

### Versioning

Arcature follows semantic versioning. Current version `0.1.1`, readable as
`arcature::FRAMEWORK_VERSION`.

Being in `0.x` shifts SemVer one field left, and Cargo agrees: the breaking
bump is the minor (`0.1` -> `0.2`), the compatible one is the patch. So
`arcature = "0.1"` takes patches and stops at `0.2`, and no exact pin is
needed to stay safe. The public API is not frozen -- that is what `0.x` is
for -- so read the changelog before a minor bump.

`0.1.0` is the first release of this codebase and it restarts the numbering.
crates.io also serves `arcature 2026.0.0` through `2026.2.1`, published from
the predecessor repository this one replaces; those are yanked and are not an
earlier version of what is documented here. A yank only withdraws a version
from new resolution, so anything already pinned to `2026.x` keeps building.

## A tour

### Routes

```rust
use arcature::prelude::*;

pub fn routes() -> Routes<AppState> {
    Routes::new([
        Route::get("/", index).name("home"),
        Route::get("/links/{id}", show).name("links.show"),
        Route::post("/links", store).name("links.store"),
    ])
}
```

Named routes generate URLs through `Routes::url_for("links.show", &["7"])`,
which returns `Err(Error::NotFound(..))` for a name that is not in the table.
Paths use Axum 0.8 syntax (`{id}`, not `:id`).

### Requests with validation

```rust
use arcature::prelude::*;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[arcature::request]
pub struct StoreLinkRequest {
    #[validate(url)]
    pub url: String,
    #[validate(length(min = 1, max = 120))]
    pub title: String,
}

pub async fn store(input: Validated<StoreLinkRequest>) -> Result<Response> {
    let request = input.into_inner();
    Ok(json(&request.title))
}
```

You write the `Deserialize` derive yourself; `#[arcature::request]` adds the
`Validate` derive and the marker trait, and must come after the derives. The
attribute on each field is `#[validate(...)]` -- validator's, not a
framework-specific one. A failure is a `422` with an RFC 9457 problem document
carrying an `errors` extension.

### Controllers

```rust
use arcature::database::QueryModel;
use arcature::prelude::*;

pub struct LinksController;

#[arcature::controller]
impl LinksController {
    pub async fn index(State(state): State<AppState>) -> Result<Response> {
        let db = state.db.as_ref().ok_or_else(|| not_found("no database"))?;
        let links = link::Entity::query(db).latest().limit(20).all().await?;
        Ok(json(&links))
    }
}
```

Every method must be `pub`, `async`, take no `self`, and declare a return
type; the macro rejects anything else with `error[ARC-M004]`. `json` takes one
argument -- the value -- and always answers `200`. Use `text(status, body)`
when the status matters.

`Db` is not an Axum extractor. It comes out of your state, which is why the
example above reaches through `State<AppState>`.

### Inertia pages

```rust
pub async fn index(inertia: Inertia) -> Result<Response> {
    let links: Vec<LinkResource> = Vec::new();
    inertia!("links/index", { links })
}
```

The `inertia!` macro requires a binding literally named `inertia` in scope.
The Client Exposure Firewall makes browser exposure opt-in: a type reaches the
browser only by being a `ClientData`, which `#[page]` and `#[resource]`
generate. Nesting a non-`ClientData` type inside one fails to compile.

### Auth

```rust
use arcature::prelude::*;

pub async fn login(
    auth: AuthManager<User>,
    input: Validated<LoginRequest>,
) -> Result<Response> {
    let request = input.into_inner();
    let user = find_user(&request.email).await?;
    auth.login(&user).await?;
    Ok(redirect().to("/dashboard").into_response())
}

pub async fn dashboard(Auth(user): Auth<User>) -> Result<Response> {
    Ok(json(&user.email))
}
```

`login` takes the user by reference and rotates the session id before binding
it -- session-fixation defence that is mandatory rather than opt-in. `Auth<U>`
is the extractor for the current user (`OptionalAuth<U>` when absent is fine);
loading the user from its id is your `UserLoader` impl, so the framework never
guesses how your users are stored.

Authorization is a separate step: `auth.authorize::<Link, LinkPolicy>("update", &link)?`.
Both type parameters are required.

### Jobs

```rust
use arcature::Job;
use arcature::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Job)]
#[job(attempts = 5)]
pub struct SendWelcomeEmail {
    pub user_id: i64,
}

// Enqueue.
jobs.enqueue(&JobRequest::new(&SendWelcomeEmail::JOB, &payload)?).await?;

// Register the handler. `add` takes `&mut self`.
let mut registry = Registry::new();
registry.add(&SendWelcomeEmail::JOB, |job: SendWelcomeEmail| async move {
    Ok(())
})?;
```

The derive is `arcature::Job`, imported explicitly: the prelude cannot glob it
in, because the derive and the `Job` trait share a name in the type namespace.

The queue runs over the pool the application already has -- no broker to run.
Delivery is at-least-once, and each claim carries a UUID fencing token so a
worker whose lease expired cannot complete a job another worker has since
taken.

Claiming a job without two workers taking the same one is the part no dialect
does the same way, so `src/jobs/dialect/` has one module each. PostgreSQL
claims with `UPDATE .. RETURNING` over `FOR UPDATE SKIP LOCKED`; MySQL 8 has
`SKIP LOCKED` but no `RETURNING`, so it picks then marks; SQLite has neither
and serialises on `BEGIN IMMEDIATE`. The three are not pretending to be the
same implementation, and SQLite's is a single-writer design by construction --
fine for one process, not a fleet.

### Events

```rust
use arcature::Event;
use arcature::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Event)]
pub struct UserRegistered {
    pub user_id: i64,
    pub email: String,
}

let dispatcher = Dispatcher::new()
    .register(|event: UserRegistered| async move { Ok(()) });

dispatcher
    .dispatch(&UserRegistered { user_id: 1, email: "a@b.com".into() })
    .await?;
```

In-process and not durable. Events cross the listener boundary as
`serde_json::Value` rather than through a `TypeId` map, which is what keeps
the no-hidden-registry rule intact; the cost is a serialise/deserialise round
trip per listener.

### Cache

`Cache` is a value you hold, not a namespace of static functions:

```rust
use std::time::Duration;

let users = cache
    .remember("users:all", Duration::from_secs(300), || async {
        load_users().await
    })
    .await?;
```

A miss is not an error, but a backend failure is -- and it does not run the
loader. There is no silent fail-open. The loader's error type only has to be
`Into<CacheError>`.

### Storage

`disk` is an instance method on a connected `Storage`, and every data-path
method takes a validated `StoragePath`:

```rust
use arcature::prelude::*;

let storage = Storage::connect(StorageConfig::fs("storage/app")?).await?;
let path = StoragePath::new("avatars/1.png")?;

storage.disk("default").put(&path, b"...").await?;
let data = storage.disk("default").get(&path).await?;
```

`StoragePath::new` rejects traversal, absolute paths and empty segments, so a
user-supplied filename cannot escape the disk root. `disk(name)` panics for a
name that was never registered -- a typo is a bug, not a runtime branch; use
`try_disk` when the name is genuinely dynamic.

### Mail

`Mail` is also a value: a `Mailer` plus a `From` address.

```rust
use arcature::mail::lettre::message::Message;
use arcature::mail::{Email, EmailError, Mailable};
use arcature::prelude::*;

pub struct WelcomeEmail;

impl Mailable for WelcomeEmail {
    fn build(&self, email: Email) -> Result<Message, EmailError> {
        email.subject("Welcome").html("<h1>Welcome</h1>")
    }
}

let mail = Mail::from_str(mailer, "noreply@example.com")?;
mail.to("user@example.com").send(&WelcomeEmail).await?;
```

`Mail::send` hands your `Mailable` an `Email` builder with `From` and `To`
already set. On the builder, only the body terminators -- `plain`, `html`,
`alternative`, `plain_with_attachments` and `alternative_with_attachments` --
return a `Result`; everything before them is infallible. SMTP credentials have a
`Debug` that prints the type name and no `Display` at all, so they cannot be
logged by accident.

## The `arc` CLI

| Command | Does |
|---|---|
| `arc new <name>` | Scaffold an application (`--stack`, `--db`, `--dest`). |
| `arc serve` | Run the application (`--bind`, `--port`). |
| `arc migrate` | Run pending migrations. |
| `arc schedule` | Run the scheduler. |
| `arc make:<kind> <name>` | Generate an artifact. 22 kinds: module, controller, model, migration, request, resource, policy, service, job, event, listener, middleware, command, page, test, factory, seeder, notification, mail, view, upload, auth. `module` writes a directory of four files, `auth` writes an account, three controllers, a route table and a migration, and `view` writes a struct plus its template; the rest write one apiece. |
| `arc key:generate` | Generate the session key. |
| `arc storage:link` | Link `public/storage` to the local disk. |
| `arc db:seed`, `db:fresh`, `db:reset` | Database lifecycle. |
| `arc queue work\|drain\|stats` | Drive the job queue. |
| `arc doctor` | Check the environment. |
| `arc version` | Print the version. |
| `arc dev` | Run the dev loop: one TCP port, Vite over IPC, rebuild on change (`--port`, `--host`, `--open`). |
| `arc routes` | Print the route table from the application graph (`--json`). Needs `uag`. |
| `arc typegen` | Write `resources/js/generated/` -- typed routes, page props, form rules. Needs `uag`. |
| `arc build` | The production build: graph, typegen, `cargo build --release`, `npm run build`. Needs `uag`. |

`arc dev` is the largest command in the CLI and the one the rest of the dev
experience hangs off: it supervises the application and Vite as child
processes, watches the source tree, and proxies Vite's requests over IPC so
the browser only ever sees one port -- the decision written up in
[ADR 0003](docs/decisions/0003-one-tcp-port.md). `arc typegen` refuses to
write anything if the graph has a diagnostic, because half a generation is
worse than none, and `arc build` runs typegen before Vite so the bundle is
compiled against the graph that shipped.

`arc routes`, `arc typegen` and `arc build` read the application graph, so
they are gated on the `uag` feature the way `arc queue` is gated on
`database` + `jobs`. `cargo install arcature` gets them only with
`--features uag`; the generated application turns the feature on for itself
through its own `uag` feature, which is what `arc typegen` uses when no dev
server is running.

## Documentation

The guide is at **<https://arcaturelabs.github.io/Arcature/>**, republished
from `main` whenever `docs/` changes. Its source is [`docs/`](docs/), and it
builds locally with mdBook:

```sh
cargo install mdbook
mdbook serve docs
```

Chapters: getting started, routing, controllers, validation, Inertia,
database, cache, storage, auth, jobs, events, mail, testing, deployment,
upgrading.

The decisions that are surprising enough to need a written record are in
[`docs/decisions/`](docs/decisions/), each one page, each stating the decision,
the context, and the cost paid:

- [No npm package](docs/decisions/0001-no-npm-package.md)
- [The CSRF cookie is `XSRF-TOKEN`, not `__Host-csrf`](docs/decisions/0002-xsrf-token-cookie.md)
- [Exactly one TCP port, in development too](docs/decisions/0003-one-tcp-port.md)
- [Layer order is a written contract](docs/decisions/0004-layer-order-contract.md)
- [There is no hidden registry](docs/decisions/0005-no-hidden-registry.md)

## What is not built yet

Nothing on this list. Every surface the guide documents is wired to something
that reads it; where a surface is narrower than its name suggests, the
narrowing is written into its own documentation rather than tracked here.

The nearest thing to an exception is `AppConfig::env`, which is carried and
readable but read by no framework code -- barred by design from gating
behaviour, because a protection an environment variable can switch off is a
protection in name only. `port`, `name` and `url` are all consumed: `port`
binds the listener, `name` and `url` are on the startup line, and `url` is
spent through `AppConfig::absolute_url(path)` wherever a link has to be built
with no request in scope. That is stated on the type, and in
[the deployment chapter](docs/src/deployment.md).

## Contributing

What the project says about itself lives in `.github/`, one file per question:

- [SUPPORT.md](.github/SUPPORT.md) -- where to ask what. A bug, a feature, a
  question and a vulnerability each have a different door, and this names them.
- [CONTRIBUTING.md](.github/CONTRIBUTING.md) -- the build, test and lint gates,
  what a change should look like, and how commits and releases are shaped.
- [SECURITY.md](.github/SECURITY.md) -- reporting a vulnerability privately,
  which versions get fixes, and what a reporter is promised in return.
- [GOVERNANCE.md](.github/GOVERNANCE.md) -- who decides, on what basis, and
  what happens to the project if that person stops.
- [CODE_OF_CONDUCT.md](.github/CODE_OF_CONDUCT.md) -- expected behaviour, and
  the mailbox that enforces it.

Released changes are recorded in [CHANGELOG.md](CHANGELOG.md).

The gates in one line each:

```sh
just check    # cargo check --all-targets
just fmt      # cargo fmt --all
just lint     # fmt --check, then clippy --all-targets -D warnings
just test     # cargo test
just features # the cargo-hack feature matrix CI runs
just docs     # cargo doc --no-deps --features fullstack
just ci       # everything CI runs, in CI's order
```

## License

Apache-2.0. See [LICENSE](LICENSE).
