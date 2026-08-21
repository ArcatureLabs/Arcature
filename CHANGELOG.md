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
can be tagged, `test_kit`, `uag` and `oauth` have to be exercised by real
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

### Added

- **CI runs the suite against all three SQL dialects.** A new `Database`
  matrix job builds `jobs,test-kit` once per driver and points
  `ARCATURE_TEST_DB_URL` at a live PostgreSQL 17, MySQL 8, or SQLite file.
  Until now the only database CI ever started was PostgreSQL, so the
  MySQL and SQLite statement text was proven to compile and never proven
  to parse. The job is required by the `CI success` gate.
- **The database tests run instead of being skipped by default.** The two
  tests that need a live server carried `#[ignore]`, which meant no
  ordinary `cargo test` anywhere -- laptop or CI -- ever ran them. They now
  ask `TestDatabase::optional()` for a database and return early when none
  is configured, so a machine with no server stays green, while
  `ARCATURE_REQUIRE_TEST_DB=1` turns that skip into a failure. CI sets it
  on every leg that starts a database, so a leg whose service failed to
  come up can no longer skip its way to a pass. `just db-test` runs the
  same thing locally, one build per driver.
- **The PostgreSQL claim is proven to hand each job to one worker.**
  `src/jobs/dialect/postgres.rs` had no tests at all, so `FOR UPDATE SKIP
  LOCKED` and `RETURNING` were only ever checked by reading them. Eight
  claimers now race over forty jobs against a live server, in batches of
  five and again one at a time, and every job must come back owned by
  exactly one worker with `attempts` at 1. A shared fixture,
  `src/jobs/test_support.rs`, migrates and empties the table and hands out
  an exclusive lock, since the claim is blind to `kind` and two tests
  running at once would take each other's jobs.
- **The MySQL pick-then-mark claim is proven exclusive.** MySQL 8 has no
  `RETURNING`, so its claim reads a set of rows and then marks them one at
  a time -- a window the other two dialects do not have, and one that only
  opens under contention. `src/jobs/dialect/mysql.rs` now runs the same
  eight-claimer race as PostgreSQL against a live server, plus a direct
  test that `CLAIM_MARK`'s `AND status = 'pending'` refuses a row another
  claimer already took. Drop `FOR UPDATE SKIP LOCKED` from the pick and
  these fail with two workers holding one job.
- **The SQLite `BEGIN IMMEDIATE` claim is proven exclusive, and proven to
  wait.** SQLite has neither `SKIP LOCKED` nor `RETURNING`: it excludes
  claimers instead of letting them past each other, which is only correct
  if the write lock is taken before the pick rather than at the first
  write. `src/jobs/dialect/sqlite.rs` now runs the same race as the other
  two dialects, with a batch of one so all eight connections contend for
  every row. Because the fixture treats a claim error as a failure rather
  than an empty batch, a `SESSION_SETUP` that stopped reaching the
  connection now fails the suite with `SQLITE_BUSY` instead of quietly
  losing throughput.
- **The fencing token is proven to reject a zombie worker.** Every
  completion mutation in `src/jobs/complete.rs` fences on `id = ? AND
  status = 'running' AND claim_token = ?`, and whether that clause matches
  nothing is a question only the server can answer. The new tests build a
  zombie the way production does -- let the lease expire, sweep, let a
  second worker reclaim -- and then require the first worker's success,
  retry, death and heartbeat all to be refused while the second worker's
  are accepted. The claim token is per claim, so the reclaim's token
  differs and the stale writes land on nothing.
- **The job migrations are run against all three dialects.** The existing
  tests split the migration files into statements; nothing ever handed
  those statements to a server, so three grammars were being trusted on
  the strength of one. `src/jobs/migrate.rs` now applies the bundled
  migration for real, checks the history table records it, enqueues into
  the table it created, applies twice more to prove idempotence and that
  the dialect's advisory lock is released, and applies inside a
  transaction the caller then rolls back. Which dialect is exercised is
  the build's choice of driver, so CI's `Database` matrix covers all
  three.
- **`AppConfig` consumes `APP_URL` and `APP_NAME`.** Both were parsed from the
  environment, stored, and read by nothing; the changelog entry below promising
  that a `1.0` cannot ship until they are consumed was the only place either
  had an effect. Two accessors make `url` reachable rather than merely
  present: `AppConfig::base_url` returns it with trailing slashes trimmed, and
  `AppConfig::absolute_url` joins a path onto it with exactly one separator.
  `absolute_url` *joins*, it never substitutes -- a path of `//evil.test`,
  `///evil.test` or `https://evil.test` lands as a path segment under
  `APP_URL`, because the leading slashes are stripped before the join. That
  property is what makes the accessor safe to reach for from the signed-URL
  and mail-link code that will call it, and it is pinned by a test. `name` and
  `base_url` are also now what the process announces at startup: the
  `listening` line carries both, so the application says which application it
  is and at which public address, instead of only the socket it happened to
  bind. Nothing was removed and no signature changed; `AppConfig::url` and
  `AppConfig::name` remain the fields they were. Closes #9.

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

- **The examples in the documentation are compiled again.** 36 of the
  crate's 49 doctests were tagged ```` ```ignore ````. That tag does not
  mean "this example is illustrative"; it means rustdoc never compiles it,
  so the example is free to drift away from the API it documents -- and
  several had drifted far enough to be wrong. They are being un-ignored a
  cluster at a time, and the ones that turned out not to compile are
  corrected rather than re-tagged:
  - **`src/dx/` -- the eight extension-point contracts.** All eight now
    compile. Two were wrong: `Inject<T>` is a newtype with no `Deref`, so
    the `Inject` example's `svc.recent_for(..)` never could have resolved
    and now reaches through `.0`; and the `Resolve` example opened with an
    `impl<S> Resolve<S> for Db` that no reader could have written, because
    both halves are foreign and the orphan rule reserves it for Arcature --
    the prose now says so and the example shows the `#[service]`-generated
    impl, which a reader *can* write.
  - **The crate-root quick start and the application builder.** The first
    example a reader meets did not compile: it called `.run()` on the
    builder, where `run` is on `Application`, and gave `main` the framework
    `Result` where `run` returns `EngineResult`. The builder module's own
    example claimed `pub fn app() -> Application<AppState>` for a function
    that returns an `ApplicationBuilder<AppState>`. Both are corrected, and
    the `layer`, `security_headers` and `cors` examples are now `no_run`
    programs rather than floating expressions.
  - **`auth` -- `AuthUser`, `Auth::authorize`, `AuthManager::login`,
    `Policy`.** The `Policy` example called
    `auth.authorize::<LinkPolicy>("view", &link)`, which cannot compile:
    `authorize` takes the resource type *and* the policy type, and Rust has
    no partial turbofish. It also passed a `&Bound<Link>` where a `&Link` was
    wanted, and swallowed the result with `?` in a handler returning the
    framework `Result` -- but `AuthzError` has no `From` into `Error`, so
    that `?` never compiled either. The corrected example maps it through
    `forbidden(..)` and says in a comment that the choice is the handler's.
  - **`routing` -- the module example and the `Middleware` contract.** The
    `Middleware` example was written as `async fn handle(..) -> Result<Response>`.
    The trait method is not async: it returns
    `Pin<Box<dyn Future<Output = Result<Response>> + Send>>`, because the
    trait is object-safe by hand. The example also returned
    `next.run(request).await` directly, but `Next::run` yields a `Response`,
    not a `Result<Response>` -- a continuation cannot fail. Both are fixed,
    with the reason for each stated above the block, and the missing
    `#[derive(Clone)]` that `Middleware: Clone` requires is now there.
  - **`config` and `database`.** The `database` module example wrote
    `index(db: Db)` and then called `inertia!(..)`, a macro that expands to
    `Inertia::render(&inertia, ..)` and therefore needs an `Inertia` in the
    handler. It also assumed a `User` model that the example never declared.
    The corrected version declares one with `#[model]` and renders through
    `Inertia::render` rather than the `inertia!()` shorthand, which does not
    compile from any call site (see the `inertia` bullet). `Model`'s example
    filtered on `UserColumn::Role` over a struct that has no `role` field.
  - **`inertia` -- the module example, the root document and the props
    schema.** Four of the six now compile. The two that do not are the
    `inertia!()` examples, and they stay `ignore` because the macro they
    show does not compile from *any* call site: its expansion names
    `inertia` without taking it as a macro argument, so `macro_rules`
    mixed-site hygiene resolves that identifier at the definition site,
    finds the `arcature::inertia` module, and every call fails with
    `error[E0423]: expected value, found module 'inertia'`. Both blocks now
    say so inline, and the macro has a "Known limitation" section pointing
    at `Inertia::render`, which is the call the expansion was reaching for.
    Of the rest: the module example dropped its `?`, and the
    `ScriptBody::nonce_attribute` example was a floating `format!` with no
    `ScriptBody` in reach -- a downstream crate cannot construct one, so it
    is now shown the way it is actually met, inside a root-document
    function.
  - **`mail`, `cache` and `storage` -- the three resource facades.** All
    four examples were fragments referring to bindings that were never
    introduced. `Mailable` is now a whole impl; the `Mail` example builds
    on `Mailer::capture_ok`, the transport that accepts everything and
    sends nothing, so the documented send actually runs during
    `cargo test --doc`. `Cache::remember` and the `Storage` builder need a
    Valkey server and a filesystem root, so they are `no_run`: compiled
    against the real signatures, not executed. The storage example also
    dropped its second disk, which called `StorageConfig::s3` -- a method
    that does not exist unless the non-default `storage-s3` feature is on.
  - **`jobs` and `events`.** The `jobs` module example used a `pool` that
    appeared from nowhere and left the one interesting line -- the handler
    registration -- commented out; it is a `no_run` function over a
    `JobPool` parameter now, with the handler registered and the closure's
    `Result<(), JobError>` spelled out, because that bound is the one a
    reader gets wrong. `JobModel`'s example is a runnable `const` with its
    accessors asserted. The `events` example ran `.await` at the top level
    of a block that had no runtime, and imported `Event` from
    `arcature::events`, which is the *trait*: `#[derive(Event)]` needs the
    derive macro of the same name from the crate root. Both are fixed and
    the dispatch now runs.
  - **`validation` -- the `Validated<T>` extractor.** The example was
    wrong in four places at once: it forgot the `#[derive(Deserialize)]`
    that `#[request]` deliberately does not add, put `required` on two
    `String` fields (`validator`'s `required` is for `Option<T>`; a
    `String` serde could not fill is already a deserialization failure),
    returned a bare `Redirect` type that is not the framework's
    `RedirectResponse`, and finished with `redirect!(route::links::index())`
    -- a macro this crate does not define, over a module the example never
    declared. It compiles now, and `redirect()` is shown as what it is, a
    builder.
  - **`oauth` -- the PKCE flow.** Behind a non-default feature, so
    `cargo test --doc` alone never sees it; verified with
    `cargo test --doc --features oauth`. The example wrote
    `oauth::GITHUB` from inside the `oauth` module itself, called a
    `session` that was never introduced, and returned a `redirect(..)`
    from a snippet with no function around it. It is `no_run` -- the token
    exchange would reach GitHub -- and now shows both halves of the flow
    as one compiled function, which is the point of the example: the state
    and the verifier that leave step one are the same two values step two
    reads back.
  - **`arcature-macros` -- the 25 that stay ignored, and why.** Every
    example in the proc-macro crate's module docs shows code naming
    `::arcature::` items, and `arcature-macros` must not depend on
    `arcature` -- that dependency is the cycle. There is nothing in that
    crate for those blocks to compile against, so all 25 keep `ignore` and
    each now carries the reason on its first line rather than leaving a
    reader to guess whether the tag was laziness. `lib.rs` states it once
    at length and points at `arcature`, where the compiled examples for
    the items these macros generate impls for actually live.

- **`arcature-macros` ships its licence text.** The crate declares
  `license = "Apache-2.0"` but the published `0.1.0` tarball contained no
  `LICENSE` file, because Cargo only picks one up from the package
  directory and the licence lived at the workspace root. Apache-2.0 4(a)
  requires the text to travel with the distributed work, so the omission
  was a licensing defect rather than an inconvenience. `macros/LICENSE` is
  now a copy of the root file and appears in `cargo package --list`.
- **CI's test database name now clears the harness's own safety gate.**
  `src/test_kit/database.rs` refuses any database whose name does not start
  with `arcature_test_`, and it reads the URL from `ARCATURE_TEST_DB_URL`.
  CI provisioned `arcature_test` -- one underscore short of the prefix --
  and exported it only as `DATABASE_URL`, so the database service looked
  wired up while every test that asked for it would have been refused
  twice over. The service is now `arcature_test_ci`
  (`arcature_test_release` in the release workflow) and both variables are
  set.

### Security

- **`SECURITY.md` says that the largest memory-safety surface here is not
  Rust.** `cargo geiger` counts `unsafe` in Rust, so the baseline it produces
  is silent about C and assembly reached through FFI -- and the biggest such
  body in this tree is AWS-LC, vendored by `aws-lc-sys`, which `aws-lc-rs`
  builds. The manifest selects it deliberately at two sites (`sqlx` with
  `tls-rustls-aws-lc-rs`, `lettre` with `aws-lc-rs`); both crates also offer
  `ring`. Version `0.44.0` carries 414 C files, 270 headers and 941 assembly
  files across all targets, and on `x86_64-pc-windows-msvc` compiles to 254
  objects in a 16 MB static library that is linked into every binary reaching
  a database over TLS, sending mail, or making an outbound HTTPS request --
  which is the default feature set. The new section states plainly that
  "Arcature is pure Rust" is therefore false, gives the reasoning for
  choosing AWS-LC over `ring` so a future reader can disagree with an
  argument instead of guessing there was one, and records the consequence: a
  CVE in AWS-LC is an Arcature security release.

- **The dependency tree's `unsafe` is counted and recorded.**
  `#![forbid(unsafe_code)]` covered the crate and said nothing about the 359
  crates under it, which is where the `unsafe` actually is.
  `unsafe-baseline.txt` now records a `cargo-geiger` reading, one line per
  crate: 153 of the 360 crates in the default `--all-targets` graph contain
  `unsafe`, 49 forbid it, and the build reaches 342 `unsafe` functions,
  31094 expressions, 823 impls, 76 traits and 992 methods. `just geiger`
  recomputes the reading and diffs it against the file; `just geiger-accept`
  records a new one. The diff is a review prompt rather than a gate, so a
  pull request that changes a dependency has to account for what moved.
  `SECURITY.md` explains how to read the file, including that the reading is
  per host target -- the committed one is `x86_64-pc-windows-msvc` -- and
  `CONTRIBUTING.md` explains when to re-record it.

- **A scaffolded project's release build is pinned by a test.** The
  generated `Cargo.toml` already took `arcature` with
  `default-features = false` and an explicit list, so `cli`, `templates`
  and `dev-proxy` stay out of a production binary -- but the only guard was
  a test asserting those strings do not appear in the manifest, which would
  pass unchanged if `default-features = false` were dropped and every one of
  them came back on. The new test asserts the line itself, asserts the
  application's own `default = []`, and asserts that `dev-proxy` is
  reachable from the `dev` feature and from nowhere else in the file. It
  also checks the dependency section specifically -- not the whole manifest
  -- for `uag`, `otel`, `oauth`, `api-docs`, `storage-s3` and `test-kit`, so
  the `dev` and `uag` feature definitions cannot mask a real entry. No
  template content changed; what changed is that reverting it now fails.

- **`cargo-deny` bans `native-tls`.** The `[bans]` list already refused
  `openssl` and `openssl-sys`, but `native-tls` is the facade that pulls
  them in: it is Schannel on Windows, Secure Transport on macOS, and
  OpenSSL on Linux -- which is every machine an Arcature application is
  deployed on. A dependency offering a `native-tls`/`rustls` choice often
  defaults to the former, so the OpenSSL entries alone would only fail
  after the C library was already in the graph. `native-tls`,
  `tokio-native-tls` and `hyper-tls` are now denied by name, so the check
  points at the crate that made the choice.

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
- **`SECURITY.md` has an attack-surface inventory.** The policy said what
  was in scope; it did not say where the scope is. A new section tables
  every place a byte an attacker chose meets something that interprets it
  -- the request line, the four validated extractors, the session and CSRF
  cookies, the Inertia headers, WebSocket frames, storage paths, job
  payloads read back out of the database -- with the parser, the guard, and
  the guard's **default** named in each row. Defaults that are off or unset
  say so: the framework sets no body limit and no request timeout (the
  scaffold sets 2 MiB and 30 s), `SecurityHeaders` is absent until asked
  for, HSTS and CSP are off inside it, and `OriginPolicy::DenyAll` is the
  realtime default. It also records two properties nothing tests --- every
  `std::process::Command::new` is under `src/cli/`, and no SQL in
  `src/jobs/` is built by interpolation --- and two facts a reader should
  not have to discover: the CSRF token is compared with `==` rather than in
  constant time, and `Pages::serve` does not canonicalise, so a symlink out
  of its root is followed. The point of the table is reviewability: a pull
  request either adds a row, changes a guard, or does neither.
- **The README no longer says three CLI commands are unwired.** It claimed
  `arc dev`, `arc typegen` and `arc build` "parse and report that they are
  not wired yet". All three have been implemented for some time --
  `arc dev` alone is ~4,300 lines under `src/cli/commands/dev/`, and the
  three are dispatched at `src/cli/mod.rs:131,149,151`. The command table
  gains a row for each of them plus `arc routes`, and a paragraph now says
  what `arc dev` does and that `routes`/`typegen`/`build` are gated on the
  `uag` feature.
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
- **The dev loop has a measured baseline.** `The dev loop` in the guide
  records what one save costs in a generated application, how it was
  measured, and on what machine. The finding the numbers settle is that a
  Rust-only change rebuilds exactly two units out of 489 -- nothing
  spurious, not `arcature`, not `arcature-macros`, not the embedded
  scaffold templates -- and that 95% of the trip is code generation and
  linking rather than type-checking. Issue #8 quoted a figure with no
  method behind it; this is the method, so the next change to the loop can
  be shown to have worked rather than asserted to have.

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
