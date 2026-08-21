# Changelog

All notable changes to Arcature are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Arcature is in `0.x`, where SemVer shifts one field left and Cargo follows it:
a **minor** bump (`0.1` -> `0.2`) is the breaking bump, and a **patch** bump
(`0.1.0` -> `0.1.1`) is backwards compatible. `arcature = "0.1"` therefore
accepts patches and refuses `0.2`, which is the protection a caret requirement
gives at `1.x` and above. No extra pinning is needed.

**The public API is not frozen.** `0.x` says so, and it is meant literally:
any release before `1.0` may remove or reshape a public item. Before `1.0`
can be tagged, `AppConfig` has to actually consume the environment variables
it parses, `test_kit`, `uag` and `oauth` have to be exercised by real
applications rather than only by their own tests, and the "Not yet
implemented" list below has to be empty or deliberately closed out.

**`0.1.0` restarts the version number.** The `arcature` name on crates.io
already carries an earlier line, `2026.0.0` through `2026.2.1`, published from
a predecessor repository that spread the framework over thirteen crates and
was then abandoned. This repository is a rewrite down to the crate graph
rather than an upgrade of it: no item, no feature name and no crate boundary
survives unchanged, so there is no migration path from `2026.x` and none is
offered. Those four versions are yanked when `0.1.0` publishes -- a yank
removes a version from new resolution and leaves every existing lockfile
resolving exactly as before, so nothing that already depends on `2026.x`
breaks. `arcature-macros` has no prior release, and `0.1.0` below is
therefore its first.

## [Unreleased]

### Deprecated

- **The auth extractors have a module that names them.** `Auth<U>`,
  `OptionalAuth<U>`, `Current<U>`, `OptionalCurrent<U>`, `AuthManager<U>`,
  `LoginBuilder`, `AuthError` and `UserLoader<S>` now live in
  `arcature::auth::extract`. `dx` abbreviates "developer experience", which
  names a goal rather than a thing, so `auth::dx` told a reader nothing about
  what was inside it -- four unrelated concerns in one ~900-line file.
  `arcature::auth::extract` says what it holds. The crate-root and
  `arcature::auth` re-exports are unchanged, and `arcature::auth::dx` still
  resolves.
- **The handler-facing session API has a module that names it.** `Session`
  and `SessionError` now live in `arcature::auth::session_api`, next to the
  `arcature::auth::session` module that configures the cookie and the
  middleware layer. The two halves were previously a file apart for no
  reason other than which one had ended up in `dx`. The re-exports are
  unchanged and `arcature::auth::dx::Session` still resolves.
- **The flash messages have a module that names them.** `Flash`,
  `FlashMessage`, `FlashLevel` and `FlashError` now live in
  `arcature::auth::flash`, together with the two session keys the extractor
  and the redirect mapper have to agree on. The re-exports are unchanged and
  `arcature::auth::dx::Flash` still resolves.
- **Authorization has a module that names it.** The `Policy<M>` trait,
  `AuthzError` and the `Auth::authorize` seam now live in
  `arcature::auth::policy` -- the last of the four concerns to leave
  `auth::dx`, which is now nothing but re-exports. Authentication ("who is
  this?") and authorization ("may they do this?") are separate steps by
  design, and they are now separate files. The re-exports are unchanged and
  `arcature::auth::dx::Policy` still resolves.
- **`arcature::auth::dx` is deprecated and scheduled for removal in `0.2.0`.**
  It is now nothing but re-exports of the four modules above, so every path
  that compiled at `0.1.0` still compiles and the fix is to delete the `dx`
  segment. Most of the names warn when used; `Auth`, `OptionalAuth`,
  `SessionError`, `UserLoader` and `Policy` do not, because rustc ignores
  `#[deprecated]` on a re-export and the alias form that would carry the
  attribute cannot be used as a tuple-struct constructor or pattern -- so
  aliasing them would have broken `Auth(user)` to deliver a warning, which is
  the wrong trade. The module documentation is the notice for those five.

### Fixed

- **`arcature-macros` ships its licence text.** The crate declares
  `license = "Apache-2.0"` but the published `0.1.0` tarball contained no
  `LICENSE` file, because Cargo only picks one up from the package
  directory and the licence lived at the workspace root. Apache-2.0 4(a)
  requires the text to travel with the distributed work, so the omission
  was a licensing defect rather than an inconvenience. `macros/LICENSE` is
  now a copy of the root file and appears in `cargo package --list`.

### Documentation

- **`arcature::dx` says what `dx` means, once.** The module now opens by
  stating that the name covers exactly one thing -- the runtime contract
  layer the macro DSL generates code against, which is also precisely what
  the `dx` Cargo feature switches on -- and that no other module may take the
  name for a second meaning. It was worth writing down because a second
  meaning had already appeared: `auth::dx` read `dx` as "developer
  experience" in general, which names a goal rather than a thing and so
  admitted four unrelated concerns into one file. A module named `dx` that is
  not this one is now documented as a defect.
- **`arcature-macros` has a README.** The crates.io page for `0.1.0` is
  blank, which for a proc-macro crate that nobody should depend on
  directly is the wrong first impression: the page now opens by saying so
  and pointing at `arcature`. It also lists all 23 entry points, the two
  properties the expansions guarantee -- no hidden registry, no panics on
  ordinary mistakes -- and the full `ARC-M001`..`ARC-M014` diagnostic
  table. A README is part of the published tarball, so this reaches
  crates.io with the next release rather than retroactively.
- **The community-health files moved into `.github/`.**
  `CODE_OF_CONDUCT.md`, `CONTRIBUTING.md`, `GOVERNANCE.md`, `SECURITY.md`
  and `SUPPORT.md` are no longer at the repository root, which leaves
  `README.md`, `CHANGELOG.md` and `LICENSE` there. GitHub reads all five
  from `.github/`, so nothing it surfaces changes, but a direct link to an
  old path now 404s and the paths inside the published tarball changed
  with them. All five also gained an entry point: the README lists them
  with the question each answers, and the guide has a `The project` page
  in its back matter. Before this, nothing anywhere linked to
  `GOVERNANCE.md` or `SUPPORT.md`.
- **The guide is published.** It is at
  <https://arcaturelabs.github.io/Arcature/>, rebuilt from `main` whenever
  `docs/` changes. CI had been building the book on every pull request and
  discarding it, so reading the guide meant cloning the repository and
  installing mdBook. `homepage` in both manifests moves from the
  repository URL -- where `repository` already pointed -- to the guide, so
  the crates.io sidebar links two different things instead of the same
  thing twice.

## [0.1.0]

### Added

- **The kernel.** The error model, HTTP routing, the application lifecycle and
  typed configuration. Routes are ordinary Rust values --
  `Routes::new([Route::get("/", index).name("home")])` -- rather than a string
  DSL, and named routes generate URLs through `Routes::url_for`.
- **Native Inertia v3.** The server half of the protocol, implemented directly,
  so a stock `@inertiajs/react`, `@inertiajs/vue3` or `@inertiajs/svelte`
  client talks to an Arcature application with no Arcature-supplied JavaScript
  package. The `Inertia` extractor, the `inertia!()` macro, the prop strategies
  (eager, always, lazy, optional, deferred, merge), partial reloads and asset
  versioning.
- **Database.** One PostgreSQL pool shared by SeaORM and SQLx, the `Db` handle,
  the `Query` facade (`where_eq`, `where_in`, `latest`, `paginate`, `count`,
  and the rest), transactions that span both paths, and migrations. The
  `db-postgres` / `db-sqlite` / `db-mysql` split keeps a SQLite user from
  compiling the Postgres protocol.
- **Auth.** Argon2id hashing with rehash detection, tower-sessions cookie
  sessions, double-submit CSRF, the `Auth<U>` / `OptionalAuth<U>` /
  `AuthManager<U>` extractors, `Session` and `Flash`, and the `Policy`
  authorization seam. Logging in rotates the session id without being asked.
- **Validation.** `Validated<T>` and the `ValidatedJson` / `ValidatedForm` /
  `ValidatedQuery` / `ValidatedPath` extractors over the `validator` crate,
  with every rejection mapped to an RFC 9457 problem response.
- **API problems.** `Problem` and `ProblemKind` (RFC 9457), compiled in
  unconditionally because validation depends on them, served as
  `application/problem+json`.
- **Cache.** One multiplexed Valkey/Redis connection, typed operations, key
  namespacing and the `remember` cache-aside helper. A miss is `Ok(None)`; a
  backend failure is an error and never quietly becomes a miss.
- **Storage.** OpenDAL-backed named disks -- `fs` always, S3 behind
  `storage-s3` -- with `StoragePath` rejecting traversal, absolute paths,
  backslashes and control characters before any I/O runs.
- **Mail.** lettre SMTP over rustls, the `Email` builder, the `Mailable` trait,
  the `Mail` facade, and an in-memory capture transport so a test can assert on
  what would have been sent.
- **Jobs.** A PostgreSQL queue on the application's existing pool, claiming
  with `FOR UPDATE SKIP LOCKED` and fencing each claim with a UUID so a stale
  worker cannot commit over a live one's work. A worker run loop with a
  concurrency semaphore, heartbeats, lease sweeping and graceful shutdown;
  exponential backoff with jitter; a scheduler; an observer seam.
- **Events.** In-process typed dispatch, sequential in registration order,
  erased through `serde_json::Value` rather than through `TypeId` and `Any`.
- **Realtime.** Thin WebSocket and SSE wrappers over axum, a bounded broadcast
  channel, an origin policy, and a connection registry with a cap and a drain.
- **Observability.** Validated `x-request-id` generation and echo, stable span
  names, and one structured access-log line per request. No global subscriber
  is installed on the production path.
- **Static assets.** `public/` served as the router fallback, with
  `Cache-Control` chosen per response: a hashed bundle is immutable for a year,
  anything else revalidates, a 404 carries none. The root document resolves its
  entry through Vite's `manifest.json`.
- **The pre-routing proxy.** An application-owned request policy --
  `Continue`, `Redirect`, `Rewrite`, `ShortCircuit` -- that runs before route
  selection, with CRLF-injection defence on redirect targets, rewritten URIs
  and header values.
- **The one-port dev proxy.** Vite runs in `middlewareMode` over an IPC
  endpoint and binds no TCP port; the Rust process owns the only listener and
  forwards Vite's requests, HMR WebSocket included. One origin in development,
  as in production.
- **The Client Exposure Firewall.** `ClientData`, `PropsSchema`, `PageContract`
  and `Inertia::render_page`. A type that merely derives `Serialize` cannot
  reach the browser as page props; exposure is an explicit opt-in the compiler
  checks.
- **The DX layer** behind the `dx` feature: `ApplicationGraph` with duplicate,
  unknown-import and cycle validation; `ModuleDescriptor`; `Resolve<S>` typed
  injection with no runtime container; `Service`, `Provider`, `Command`,
  `RouteModel`, `Bound<T>`, `DbFromState`.
- **The unified DSL macros:** `module!`, `application!`, `routes!`, `redirect!`,
  `page_macro!`, the attributes `#[model]`, `#[request]`, `#[controller]`,
  `#[service]`, `#[provider]`, `#[policy]`, `#[middleware]`, `#[command]`,
  `#[job_handler]`, `#[route_model]`, `#[request_cache]`, `#[resource]`,
  `#[page]`, `#[listener]`, and the derives `Job`, `Event`, `DxComponent`.
  Every macro reports a mistake as a `compile_error!` carrying an `ARC-M<NNN>`
  code; none panics on ordinary bad syntax.
- **Production pipeline stages,** each off unless asked for: compression,
  security headers, CORS, request id, access log, panic catching, error
  mapping, body limit, timeout, maintenance mode, session, CSRF, Inertia and
  user layers. The order they compose in is fixed in
  `src/application/pipeline.rs` rather than following builder call order.
- **Error mapping.** Every bodiless error a layer produced — the bare `404`,
  `405`, `408` and `413` that axum and `tower-http` emit — gets an RFC 9457
  body, and a `text/plain` 5xx is redacted in release builds, because in
  practice that shape is a stringified internal error carrying a connection
  URL or a build-machine path.
- **Health endpoints.** `/up/live` and `/up/ready` are merged beside the
  application router rather than layered over it, so an orchestrator probing
  every few seconds pays no session load, no maintenance check and no log
  line.
- **Security headers.** `nosniff`, `DENY` framing and a strict referrer policy
  always; HSTS and CSP opt-in. An existing header wins, so the layer is a floor
  and not a ceiling.
- **Zero-config CSRF for Inertia.** `CsrfConfig::inertia()` uses the
  `XSRF-TOKEN` cookie and `x-xsrf-token` header axios hard-codes, so a stock
  Inertia client posts successfully without a line of application JavaScript.
- **The `arc` CLI:** `new`, `version`, `serve`, `migrate`, `schedule`,
  `queue work|drain|stats`, `db seed|fresh|reset`, `key:generate`,
  `storage:link`, `doctor`, and the `make:<kind>` generator family
  (controller, model, migration, request, resource, policy, service, job,
  event, listener, middleware, command, page, test, factory, seeder). Parsed
  with clap's builder API, shipped from the same package behind the `cli`
  feature so a normal application never compiles it. `dev` supervises the one
  TCP port, `routes` prints the route table (`--json` for tooling), `typegen`
  emits the TypeScript, and `build` runs validate, typegen, `cargo build
  --release` and `vite build` in that order, failing at the first stage that
  fails and naming it.
- **The application scaffold.** `arc new` writes a Laravel-shaped tree: `app/`
  (controllers, models, services, requests, policies, resources), `bootstrap/`,
  `config/`, `database/migrations/`, `routes/`, `resources/js` and
  `resources/css`, `public/`, `storage/`, `.env` and a smoke test.
- **CI.** An MSRV-and-stable matrix, `cargo fmt --check`, clippy with warnings
  denied, a PostgreSQL 17 service, the whole feature surface through
  `cargo hack`, and `cargo publish --dry-run`.

### Fixed

- `#[controller]` validated its impl block and re-emitted it unchanged while
  `module!` referred to `ControllerMetadata::METHODS`, so any real `module!`
  failed to compile. That is why the scaffold used no DSL at all.
- `ApplicationBuilder` had no way to install a `tower::Layer`. `InertiaLayer`,
  `SessionLayer` and `CsrfLayer` were all written and none could be attached,
  so a scaffolded application answered `500 inertia adapter error` on its own
  home page.
- A route's middleware wrapped every route registered before it. `Route` held a
  closure folding the whole `axum::Router` and `Routes::new` folds routes into
  one accumulating router, so a public route silently inherited a guard
  declared later in the same array. `Route` now owns a `MethodRouter`, which
  cannot reach past the one path it serves.
- `ARCATURE_VITE_IPC` was never read. The dev proxy could only be switched on
  by a builder call the scaffold does not make, so Vite requests would have
  404'd. The builder resolves the endpoint at construction now, and
  `dev_proxy_endpoint()` is the override its documentation already claimed to
  be.
- `tower-http` was optional while the pipeline's body limit and timeout used it
  unconditionally, so `--no-default-features` failed to build. `observe` was
  missing `dep:uuid` and `pages` was missing `dep:tokio`; both only ever built
  because some other feature happened to pull the dependency in.
- The WebSocket run loop had a hard-coded 20-second heartbeat. It honours
  `WsLimits` now and closes a connection whose pong does not arrive.
- `AccessLogLayer` had been written and never applied.
- `IntoRouteParams` appeared in the signature of a public method while being
  crate-private, so no outside caller could name the bound.
- The `opentelemetry` feature was declared and unused. Removed.

### Security

- A caught panic returns an RFC 9457 `Problem` and the panic payload is
  discarded rather than reported: a panic message routinely carries a file
  path, a SQL fragment, or the value that caused it. `tower-http` still logs it
  for the operator.
- HSTS is opt-in. A development server that sends it pins `localhost` to HTTPS
  for a year, and the pin outlives the header that set it.
- `DispatchError::Deserialize` carries no message, because a serde error may
  echo the payload it failed on.
- `CacheConfig`, `S3Config`, `SmtpConfig` and `SmtpCredentials` implement
  `Debug` by hand and redact their secrets.
- Redirect targets are checked against open redirects: an absolute or
  scheme-relative URL pointing at another host is rejected.
- `StoragePath` rejects `..`, absolute paths, backslashes and control
  characters before any I/O is attempted.
- Job claims are fenced with a per-claim UUID, so a worker that lost its lease
  cannot commit a result over the worker that took it over.

### Not yet implemented

Gaps documented in the source at the point of use, repeated here so nobody has
to find them at runtime.

- `AppConfig` reads `APP_NAME`, `APP_URL`, `APP_ENV` and `APP_PORT`, and the
  framework consumes exactly one of them. `port` becomes the port the process
  listens on, below `ARCATURE_BACKEND_PORT` and `PORT` in precedence. `name`,
  `url` and `env` are carried so that `Application::config` can hand them
  back, and are read by no framework code: no surface builds an absolute URL
  yet, and `env` is forbidden from gating behaviour rather than merely not
  gating it -- a protection an operator can switch off with an environment
  variable is a protection in name only, so redaction and the dev-only UAG
  endpoint key off `cfg!(debug_assertions)` instead. See the type
  documentation.

[Unreleased]: https://github.com/ArcatureLabs/Arcature/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ArcatureLabs/Arcature/releases/tag/v0.1.0
