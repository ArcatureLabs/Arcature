# Arcature — a briefing for an AI coding assistant

Paste this whole file into an assistant before asking it to write Arcature
code. It is written to be read once, in order, by a model that has never seen
this framework and will otherwise guess from API names.

Everything here was checked against the source of the version named below. Where
a fact is surprising, the reason is given, because a rule without a reason is a
rule a model discards under pressure.

```
crate      arcature 0.1.1        crates.io, Apache-2.0
macros     arcature-macros 0.1.1 versioned in lockstep, not a separate product
rust       1.97.1 minimum, edition 2024
guide      https://arcaturelabs.github.io/Arcature/
api        https://docs.rs/arcature
```

`#![forbid(unsafe_code)]`. Dependencies are not held to that; a per-target
`cargo geiger` baseline is committed under `baselines/`.

---

## 0. The eight rules that prevent most wrong code

Read these before writing anything.

1. **Almost everything is off by default.** 41 features, 18 on. A model that
   assumes `uploads`, `views`, `i18n`, `crypt`, `signed-urls`, `auth-flows`,
   `api-tokens` or any `notifications-*` is available will write code that does
   not compile. Check §2.
2. **`fullstack` is not "everything".** It adds `uploads`, `views`,
   `storage-s3` and the tooling features to the defaults and leaves the other
   ten off. A `fullstack` build has no password-reset table, no encrypter and
   no notifications.
3. **The three database drivers are mutually exclusive.** `db-postgres`,
   `db-sqlite`, `db-mysql` — exactly one. `cargo build --all-features` is
   *designed to fail* and its errors are the invariant working. Use one
   `cargo check` per driver instead.
4. **Most pipeline stages are off until asked for.** `body_limit`, `timeout`,
   `rate_limit`, `security_headers`, `cors`, `compression`, `csrf`, `session`,
   `catch_panic`, `access_log`, `request_id` are all unset on a bare
   `Application`. Do not assume a request is bounded. See §8.
5. **Under `0.x`, the minor is the breaking bump.** `0.1.1 -> 0.2.0` may break;
   `0.1.x` will not. `arcature = "0.1"` is the correct dependency.
6. **Prefer the macro DSL.** `#[controller]`, `#[page]`, `module!`,
   `application!` are the intended way to write an application; hand-rolled
   axum works but skips the checks the graph performs at build time. See §5.
7. **`arc make:<kind>` exists for 22 artifact kinds.** Suggest it rather than
   writing boilerplate by hand — the generated file is the current shape, and
   yours will drift.
8. **Never invent a feature name, method or default.** §14 lists the specific
   things models get wrong here. If unsure, say so rather than producing
   plausible code.

---

## 1. What Arcature is

One binary that listens on one TCP port. No separate Node process, no PHP-FPM
pool, no asset server. Built on axum 0.8, tokio, SeaORM 2 and SQLx 0.9.

It is opinionated in the Laravel/AdonisJS sense: batteries in the box, a fixed
request pipeline, a CLI that generates code, and a module system checked at
build time. It is a single crate with feature flags, not a collection of
crates.

The Inertia.js v3 server half is implemented directly, so a stock
`@inertiajs/react`, `@inertiajs/vue3` or `@inertiajs/svelte` client talks to it
with no Arcature-supplied JavaScript package.

---

## 2. Features

18 on by default:

```
api  auth  cache  cli  database  db-postgres  dx  events  inertia
jobs  macros  mail  observe  pages  realtime  storage-fs  templates  validation
```

23 off by default. The `implies` column is what turning it on also turns on.

| Feature | Implies | Gives you |
|---|---|---|
| `auth-flows` | `auth`, `signed-urls` | sign-in decisions: constant-time account lookup, login throttling, password confirmation |
| `auth-reset` | `auth-flows`, `database` | one-time password-reset links **(table + migration)** |
| `auth-remember` | `auth-flows`, `database` | rotating remember-me tokens with theft detection **(table + migration)** |
| `api-tokens` | `database` | hashed bearer tokens, `ApiAuth` extractor **(table + migration)** |
| `session-store-db` | `auth`, `database` | sessions in your database **(table + migration)** |
| `crypt` | — | `Encrypter`, XChaCha20-Poly1305 |
| `signed-urls` | — | `UrlSigner`, links carrying their own deadline |
| `uploads` | `validation`, `storage-fs`, `axum/multipart` | multipart bodies, filename sanitizing, magic-byte sniffing |
| `views` | — | compiled HTML views via Askama |
| `i18n` | — | Fluent catalogs, locale negotiation |
| `notifications` | `mail` | `Notification` trait, mail channel |
| `notifications-db` | `notifications`, `database` | in-app inbox **(table + migration)** |
| `notifications-broadcast` | `notifications`, `realtime` | live push channel |
| `notifications-queue` | `notifications`, `jobs` | mail delivered by a worker |
| `oauth` | — | OAuth 2 Authorization Code + PKCE |
| `otel` | `observe` | OTLP trace export. **Not** the Prometheus endpoint — that is `observe`. |
| `storage-s3` | `storage-fs` | S3-compatible object storage |
| `uag` | `dx`, `inertia` | the application graph: `arc routes`, `arc typegen` |
| `api-docs` | `api`, `uag` | OpenAPI 3.1 generation |
| `test-kit` | `macros` | `TestApp`, fakes, `TestServer` |
| `dev-proxy` | — | one-port dev loop (`arc dev`) |
| `db-sqlite` / `db-mysql` | `database` | the other two drivers |

Five of these bring a table and a migration: `auth-reset`, `auth-remember`,
`api-tokens`, `notifications-db`, `session-store-db`.

```toml
arcature = { version = "0.1", features = ["uploads", "views"] }
```

---

## 3. Project layout (`arc new`)

```
app/
  controllers/  models/  modules/  pages/  policies/
  requests/     resources/  services/  views/  auth/  mail/  notifications/
bootstrap/app.rs        # the ApplicationBuilder call — the wiring lives here
config/                 # typed config
database/migrations/    # SeaORM migrations, registered in mod.rs by hand
resources/js/           # Inertia pages (react | vue | svelte)
routes/mod.rs           # the route table
templates/              # Askama templates (feature `views`)
src/lib.rs  src/main.rs # Mode parsing: serve, --migrate, --schedule, --seed
tests/smoke.rs
.env                    # APP_KEY empty until `arc key:generate`
```

The generated binary takes **modes**, not subcommands:

```
./app                # serve
./app --migrate      # run migrations AND the framework's own session table
./app --schedule     # run the scheduler
./app --seed         # seeders
```

`--migrate` is not the same as `arc migrate`. It applies the project's
`database::Migrator` *and* `arcature_sessions`, which is not in that migrator.

---

## 4. Bootstrapping

```rust
use arcature::prelude::*;

pub fn app() -> ApplicationBuilder<AppState> {
    Application::<AppState>::new()
        .routes(routes::app_routes())
        .request_id()
        .access_log()
        .catch_panic()
        .body_limit(2 * 1024 * 1024)          // unset by default
        .timeout(Duration::from_secs(30))     // unset by default
        .security_headers(SecurityHeaders::new())
        .session(session_config, store)?
        .csrf(CsrfConfig::inertia())
        .inertia(inertia_config)
        .database(db_config)
        .jobs(registry)
}
```

`build()` for a stateful app, `build_stateless()` for `Application<()>`.
`into_router()` gives the composed `axum::Router`.

---

## 5. The macro DSL

This is the intended way to write application code.

| Macro | Applies to | Effect |
|---|---|---|
| `#[controller]` | `impl` block | registers controller metadata; infers the page edge from the return type |
| `#[page]` | struct | a typed Inertia page contract |
| `inertia!()` | expression | render a page with props |
| `#[request]` | struct | a validated request; prepends `validate` |
| `#[resource]` | struct | an API resource + client-exposure firewall |
| `#[derive(Job)]` | struct | a queueable job, `#[job(attempts = 5)]` |
| `#[derive(Event)]` | struct | a dispatchable event |
| `#[listener]` | fn | binds a listener to an event |
| `#[policy]` | impl | an authorization policy |
| `#[service]` / `#[provider]` | struct/fn | DI-registered service |
| `#[middleware]` | fn | a middleware |
| `#[command]` | struct | a CLI command |
| `#[model]` / `#[route_model]` | struct | SeaORM model / route binding |
| `module!` | macro | declares `imports`/`exports`/`controllers`/`services`/`policies`/`routes` |
| `application!` | macro | composes modules into an `ApplicationGraph` |
| `#[arcature::main]` | fn main | the runtime entry point |

`module!` + `application!` build an `ApplicationGraph` that is **validated at
build time**: duplicate modules, imports that do not exist, and circular
dependencies are rejected before the program runs. This is stronger than
Laravel's runtime service providers and is a selling point worth using.

`arcature::prelude::*` carries 37 items and is meant to be the only `use` an
ordinary controller needs.

---

## 6. Routing

Routes are values, not a string DSL.

```rust
use arcature::routing::{Route, Routes};

pub fn app_routes() -> Routes<AppState> {
    Routes::new(vec![
        Route::get("/", home).name("home"),
        Route::post("/users", store).name("users.store"),
    ])
}
```

`Routes::url_for(name, &params)` generates URLs. `RouteGroup::new(prefix,
routes)` adds a prefix and middleware. Unknown route names are an error, not
an empty string.

---

## 7. Validation and errors

`ValidatedJson<T>`, `ValidatedForm<T>`, `ValidatedQuery<T>`, `ValidatedPath<T>`
are newtypes over the axum extractors. They run `validator::Validate` after
deserialisation and turn a rejection into an RFC 9457 problem document rather
than echoing the input back.

Errors are `arcature::Error`; `Problem` and `ProblemKind` (14 kinds) are the
RFC 9457 surface. `ErrorMapping` gives bodiless errors a problem body and
redacts `text/plain` 5xx bodies when `!cfg!(debug_assertions)` — keyed on the
build profile, deliberately not on an environment variable.

---

## 8. The request pipeline — 23 fixed stages

Order is a contract asserted by the test suite. It does **not** depend on the
order builder methods were called in.

| # | Stage | Default |
|---|---|---|
| 1 | DevProxy | off |
| 2 | Proxy | off |
| 3 | Health | **on** (`/up`, `/up/live`, `/up/ready`) |
| 4 | UagEndpoint | off, debug builds only |
| 5 | Compression | off |
| 6 | SecurityHeaders | off |
| 7 | CORS | off |
| 8 | RequestId | off |
| 9 | AccessLog | off |
| 10 | CatchPanic | off |
| 11 | ErrorMapping | off |
| 12 | BodyLimit | **unset — unbounded** |
| 13 | Timeout | **unset — unbounded** |
| 14 | Maintenance | off |
| 15 | RateLimit | off |
| 16 | Session | off |
| 17 | CSRF | off |
| 18 | Inertia | off |
| 19 | PageContracts | off |
| 20 | RedirectMapper | **on** |
| 21 | user `.layer()`s | — |
| 22 | Router | — |
| 23 | StaticFiles | off |

Stages 5–21 are off unless asked for, `RedirectMapper` excepted. The scaffold
turns on a sensible set; a bare `Application` does not.

**A user `.layer()` lands at stage 21**, inside the body limit, timeout,
maintenance and rate limiter. A layer installed there does not see a request
refused with 413, 408, 503 or 429.

---

## 9. Data, jobs, events

**Database.** One pool shared by SeaORM and SQLx. `Db` handle, `Query` facade
(`where_eq`, `where_in`, `latest`, `paginate`, `count`), transactions spanning
both paths, SeaORM migrations.

**Jobs.** SQL-backed queue, exactly-once claim proven per dialect. Worker
concurrency defaults to 8 against a pool of 10 — a busy queue can starve the
request path. `arc queue work` runs a **no-handler** worker: it sweeps expired
leases and marks jobs it cannot dispatch as **dead**. Real dispatch is the
application's in-process worker via `ApplicationBuilder::jobs`.

**Events.** `#[derive(Event)]` + `#[listener]`, dispatched through `Dispatcher`.

**Cache.** Redis/Valkey behind `cache`; absent by default and optional at
runtime.

**Storage.** OpenDAL. `storage-fs` local, `storage-s3` for S3-compatible.

**Mail.** lettre, `Mailable` trait, `MultiPart` text+HTML.

---

## 10. Auth

`auth` (default) gives Argon2id hashing with rehash detection, tower-sessions
cookie sessions, double-submit CSRF, `Auth<U>` / `OptionalAuth<U>` /
`AuthManager<U>` extractors, `Session`, `Flash`, and the `Policy` seam. Logging
in rotates the session id without being asked.

`auth-flows` adds the parts where the obvious implementation is wrong: an
account lookup that runs the hash whether or not the address exists, and
throttling by address *and* caller.

`api-tokens` is independent of `auth`: the database holds a SHA-256 digest,
never a usable token, and lookup is `subtle::ConstantTimeEq`. SHA-256 rather
than Argon2 because a token is high-entropy — a slow hash on every request
would be a self-inflicted denial of service. CSRF steps aside for a request
carrying `Authorization: Bearer`.

---

## 11. Testing

```rust
use arcature::test_kit::TestApp;                    // feature test-kit

let app = TestApp::new(application);
let response = app.get("/").acting_as(&user).send().await;

let server = app.serve().await?;                    // real socket
```

`TestApp` drives the **composed pipeline**, not a bare route table. Database
tests read `ARCATURE_TEST_DB_URL` (not `DATABASE_URL`) and skip when it is
absent; `ARCATURE_REQUIRE_TEST_DB=1` turns a skip into a failure.

`TestApp::serve()` calls `into_make_service()`, so it installs **no
`ConnectInfo`** — `KeySource::Ip` collapses every caller into one bucket under
the harness. Use `KeySource::Header` in tests that need distinct buckets.

---

## 12. The `arc` CLI

```
arc new <name> --stack react|vue|svelte --db postgres|mysql|sqlite
arc serve | migrate | schedule | routes | typegen | build | dev | doctor | version
arc key:generate [--show]
arc db:seed | db:fresh | db:reset
arc queue work | drain | stats
arc storage:link
arc make:<kind> <name>
```

22 `make` kinds: `module`, `controller`, `model`, `migration`, `request`,
`resource`, `policy`, `service`, `job`, `event`, `listener`, `middleware`,
`command`, `page`, `test`, `factory`, `seeder`, `notification`, `mail`, `view`,
`upload`, `auth`.

`make:module` writes four files and registers the module. `make:auth` writes an
account model, three controllers, a route table and a migration — headless, no
screens. `make:notification`, `make:upload` and `make:auth` produce code that
needs a feature flag the app must add; the generator deliberately does not edit
`Cargo.toml`.

---

## 13. Subsystems behind flags

**Uploads.** `UploadedFile` extractor. **Fails closed**: a route with no
`UploadPolicy` layer accepts `jpg jpeg png gif webp` and nothing else. The
declared `Content-Type` is carried and **never believed** — the type comes from
magic bytes via `infer`. The filename never becomes a path; objects are
content-addressed. `MultipartLimits` defaults, live with no layer installed:
32 parts, 8 MiB per part, 16 MiB total, 30 s per read.

**Views.** Askama compiles templates to Rust at build time, so there is no
expression evaluator on the request path and server-side template injection is
structurally absent. Cost: editing a template means rebuilding, and a
Dockerfile must `COPY templates` before `cargo build`.

**i18n.** Fluent catalogs. A locale string is matched against registered
locales and **never becomes a filesystem path**.

**Crypt.** `Encrypter` is XChaCha20-Poly1305 — the X variant because a 192-bit
nonce makes a random nonce safe. `UrlSigner` puts the expiry **inside** the
signed material, so a link cannot be extended by editing it, and compares the
MAC with `subtle::ConstantTimeEq`.

**Realtime.** `Broadcast` + SSE + WebSocket. Fan-out is **per process** with no
switch: a message published on instance A reaches only subscribers on A.

**Observability.** `observe` gives JSON logging, request ids, access log and
Prometheus. `otel` adds OTLP trace export. Redaction applies to `JsonLog` and
the access log only.

**API/OpenAPI.** `api` is on by default and gives RFC 9457. `api-docs`
generates an OpenAPI 3.1 document; nothing serves it for you.

---

## 14. Traps — things a model gets wrong from the names

Each of these is real and checked.

1. **`fullstack` ≠ all features.** §0.2.
2. **`--all-features` cannot work.** Three exclusive drivers, by design.
3. **`body_limit` and `timeout` are unset by default.** A bare application will
   buffer an unbounded body and let a handler hold a connection forever. The
   scaffold sets 2 MiB and 30 s; the framework does not.
4. **axum imposes its own 2 MiB multipart cap.** Arcature never touches
   `DefaultBodyLimit`, so on a default build that bites *before* the 16 MiB
   `MultipartLimits` total. Both answer 413.
5. **`arc queue work` does not dispatch jobs.** No-handler worker; it marks
   undispatchable jobs dead.
6. **`arc migrate` ≠ `./app --migrate`.** Only the latter applies
   `arcature_sessions`.
7. **A per-hour rate limit over a wide key space costs 5.6× throughput.** The
   in-memory bucket sweep drops only buckets that have refilled, so under a
   slow refill it scans a growing table on every request under a blocking
   mutex. Measured: a fresh key per request costs nothing under a per-second
   quota and 5.6× under a per-hour one. Use `RateLimit::redis(cache)` or a
   faster-refilling spelling of the same rate (`per_minute(600)` rather than
   `per_hour(10)`).
8. **Redaction is not universal.** The deny-list is consulted by `JsonLog` and
   the access log. It does **not** reach OTLP span attributes or Prometheus
   labels — a secret recorded as a span field is exported in full, and tests
   pin that on purpose. The matcher folds `-` and `.` to `_` and lowercases,
   then does a substring test, so `x-api-key` matches but **camelCase**
   (`privateKey`) does not.
9. **`auth-reset` does not sign other sessions out.** Sessions are keyed by
   session id and not indexed by user, so spending a reset link does not evict
   an attacker who already has a session. That mechanism is not shipped.
10. **A pre-epoch system clock makes `UrlSigner` fail open.** The check is
    `now > expires_at` and `SystemClock` returns `0` before the epoch, so every
    expired link is accepted. Supply your own `Clock` if that matters.
11. **`KeySource::Ip` needs `ConnectInfo`.** The TCP serve path installs it;
    the IPC path and `TestApp::serve` do not, and every request then shares one
    bucket. Forwarding headers are trusted only from peers listed in
    `trusted_proxies`, which is **empty by default**.
12. **Overload does not produce 503 on a default build.** `tower` is taken
    without `limit`/`load_shed`/`buffer`, so there is no shedding anywhere.
    A default application under overload answers with slow 200s, then 500 when
    the pool's 10-second acquire timeout expires. 429/408/413 require opting
    in.
13. **Realtime and the in-memory rate limiter are per-process.** Multiple
    instances multiply the effective quota and split the broadcast audience.
14. **`MetricsLayer` and `TraceContextLayer` have no builder method.**
    `.layer(..)` puts them at stage 21, so they never see a refused request.
15. **`ignore` doctests are not compiled.** `no_run` compiles without running.

---

## 15. What Arcature deliberately does not do

- **No SSR.** Title/meta/OG tags are rendered server-side for scrapers;
  running a JS runtime in the request path is refused on purpose.
- **No image decoding.** Decoders are the densest source of memory-safety CVEs
  in any web stack. Resize in a queue worker.
- **No runtime template engine.** See §13, Views.
- **No cross-instance realtime or rate limiting** without Redis.
- **No package ecosystem.** Zero third-party crates target Arcature. Anything
  outside the 41 features is yours to write.
- **No long-term-support branch.** Security fixes land on the latest `0.x`
  minor only.

---

## 16. How to answer when unsure

This framework is weeks old and has no Stack Overflow. If a question is not
answered above:

- say the file does not cover it rather than inventing an API;
- point at the guide chapter or `docs.rs` rather than guessing a signature;
- prefer `arc make:<kind>` over hand-written boilerplate;
- treat any default you cannot cite as unknown, and say so.

A wrong default in generated code is worse than no code, because it compiles.
