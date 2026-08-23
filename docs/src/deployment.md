# Deployment

An Arcature application is one binary that listens on one TCP port. There is
no separate Node process, no PHP-FPM pool, no asset server. What you deploy is
the binary, the built frontend assets, and whatever the application reads out
of the environment.

## The pipeline

Every request travels through a fixed, ordered stack. The order is a contract
written down in `src/application/pipeline.rs` and asserted by the test suite;
it does not depend on the order the builder methods were called in.

Outermost first:

| # | Stage | Note |
|---|---|---|
| 1 | `DevProxy` | Forwards Vite requests in development. Pass-through unless `arc dev` set an IPC endpoint. |
| 2 | `Proxy` | Pre-routing URI rewriting. |
| 3 | `Health` | Merged beside the router, not layered over it. |
| 4 | `UagEndpoint` | Merged beside the router too. Debug builds only, after an explicit `.uag_endpoint(..)`. |
| 5 | `Compression` | Sees the final body, whoever produced it. |
| 6 | `SecurityHeaders` | Outside the body limit and timeout, so a `413` and a `408` carry them too. Mints the per-request CSP nonce on the way down. |
| 7 | `CORS` | Answers a preflight without waking anything below. |
| 8 | `RequestId` | Every response carries `x-request-id`. |
| 9 | `AccessLog` | Outside the panic catcher, so a `500` is logged. |
| 10 | `CatchPanic` | A panic becomes a `500`, not a dropped connection. |
| 11 | `ErrorMapping` | RFC 9457 bodies for bodiless errors; release redaction. |
| 12 | `BodyLimit` | Rejects an oversized upload before buffering it. |
| 13 | `Timeout` | A slow handler cannot hold a connection open. |
| 14 | `Maintenance` | Outside session and CSRF. |
| 15 | `RateLimit` | Inside maintenance, outside the session. |
| 16 | `Session` | Loaded before CSRF needs the token. |
| 17 | `CSRF` | An unsafe request is rejected before it can act. |
| 18 | `Inertia` | Innermost framework layer, so a CSRF rejection is not dressed up as a page. |
| 19 | `PageContracts` | Data, not behaviour. |
| 20 | `RedirectMapper` | Finishes `redirect().route(..)` and `.with(..)` against the route table and the session. |
| 21 | user `.layer()`s | Applied in call order, innermost. |
| 22 | Router | Route matching and the handler. |
| 23 | `StaticFiles` | The router's fallback. |

Stages 5 through 21 are **off unless asked for**, `RedirectMapper` excepted --
it is installed by default, because a `redirect().route(..)` that silently
`400`s is not a default anybody wants. An application that calls nothing but
`.routes()` gets a bare router plus the health endpoints. That is deliberate:
each entry in the list above is a decision someone made, which is what makes
the list readable.

The reasoning behind each position is in
[ADR 0004](https://github.com/ArcatureLabs/Arcature/blob/main/docs/decisions/0004-layer-order-contract.md) and in the module
documentation itself.

## Health, liveness and readiness

Three endpoints, mounted under `/up` by default (`.health_prefix("/healthz")`
moves them, `.health(false)` removes them):

| Path | Question | Answered from |
|---|---|---|
| `GET /up/live` | Is this process alive? | The lifecycle alone. Never touches a database, cache, or the network. |
| `GET /up/ready` | Should this process receive traffic? | The lifecycle **and** every started subsystem's probe. |
| `GET /up` | The same, as JSON for a human. | Both. |

Wiring a restart policy to readiness is the classic outage: a database blip
restarts the pod, and the restart does not bring the database back. Point
liveness probes at `/up/live` and load-balancer health checks at `/up/ready`.

These three bypass maintenance mode and Inertia, and always answer
`application/json` with `Cache-Control: no-store`. A cached readiness answer
is a wrong answer.

Readiness is `false` before startup finishes: the health handle holds the
subsystem set in a `OnceLock` that startup fills in, so a process that has not
booted reports that it has not booted.

## Startup and shutdown order

`run_with_state` starts subsystems in a fixed order and tears them down in
reverse:

```text
start:  database -> jobs -> cache -> storage -> mail
serve:  mark ready, accept connections
drain:  begin_drain (readiness turns 503, requests in flight continue)
stop:   mail -> storage -> cache -> jobs -> database
```

The drain step is why the readiness endpoint exists in the form it does.
`begin_drain` runs *before* the listener stops, so `/up/ready` answers `503`
while in-flight requests are still being served. That window is exactly what a
load balancer needs to take the instance out of rotation without dropping
anything.

`SIGTERM` and Ctrl-C both trigger graceful shutdown on Unix; on Windows only
Ctrl-C is wired.

`serve(listener)` is the escape hatch: it takes an already-bound listener and
skips ordered startup entirely. Health endpoints still work, but they report
on an empty resource set, because on that path there are no subsystems.

## Running more than one instance

Most of the framework is indifferent to how many processes you run. Three
subsystems are not, and the difference between them matters more than the
list suggests: two have a cross-instance mode you switch on, and one does
not.

**Sessions are shared, if you configure a store that shares them.** The
`session-store-db` feature puts sessions in the same database the
application already uses, so a request may land on any instance and a
deploy does not log everyone out. `MemoryStore` does neither. This is a
configuration choice with a correct answer, not a limit.

**Rate limiting is per-process until you point it at Redis.** The default
backend is an in-process `HashMap` of token buckets, so with *n* instances
behind a load balancer a client gets roughly *n* times the nominal quota.
`RateLimit::redis(cache)` (needs the `cache` feature) moves the buckets to
Redis/Valkey and the quota becomes global. Decide this deliberately:
`OnBackendError` controls what happens when Redis is unreachable, and it
defaults to `Refuse` -- the limiter fails closed rather than silently
becoming no limiter at all.

**A per-hour quota keyed by address is the one combination that costs
throughput.** The in-memory backend sweeps its bucket table past 8192
entries and drops every bucket that has refilled to capacity, which is what
keeps one-bucket-per-IP from growing without bound. It only works while
buckets refill faster than new addresses arrive. Under a per-hour quota a
bucket stays ineligible for six minutes, so the sweep drops nothing, the
table keeps growing, and every subsequent request rescans it while holding a
blocking mutex. Measured at 128 connections: a fresh key on every request
costs nothing under a per-second quota and **5.2x throughput** under a
per-hour one (`tests/load_profile.rs` has the four-row table, one variable
per row).

That combination is exactly the shape of a login or password-reset throttle,
so it is worth choosing on purpose. `RateLimit::redis(cache)` avoids it
entirely -- there is no client-side map to scan, only a per-key expiry the
server honours. Failing that, prefer the faster-refilling spelling of the
same rate: `per_minute(600)` and `per_hour(10)` allow nearly the same
traffic over an hour, but the first refills a spent bucket in a tenth of a
second and only the second accumulates.

**Realtime fan-out is per-process, and there is no switch.** `Broadcast`
wraps a `tokio::sync::broadcast` channel, which is a channel between tasks
inside one process. A message published on instance A reaches only the
WebSocket and SSE subscribers connected to instance A. Nothing errors and
nothing warns: subscribers on instance B simply never see it, which is why
this is worth stating plainly rather than leaving to be discovered. With
two instances and clients spread evenly, roughly half of each broadcast is
lost from any given client's point of view.

Until a cross-instance bridge exists, there are three honest ways to live
with this:

- **Run one instance.** Vertical scale goes a long way, and this is the
  only option that needs no extra reasoning.
- **Pin realtime connections to one instance.** A load balancer routing
  WebSocket and SSE upgrades to a single backend keeps fan-out correct
  while ordinary HTTP scales out. Whether that instance's failure is
  acceptable is an availability question, not a correctness one.
- **Publish from a shared source.** If every message originates from a job
  worker or an external system, have each instance subscribe to that
  source and re-publish locally. This is the bridge, written by hand.

A Redis pub/sub bridge is the obvious general answer and `redis` is already
in the tree behind the `cache` feature, but it is not written: it would
need delivery semantics, ordering and back-pressure decided on purpose
rather than inherited, and no traffic has yet asked the question.

## Maintenance mode

`Maintenance` is an `Arc`-backed handle, not a global and not a file on disk.
Flip it from an admin route, a signal handler, or a test. Everything except
the health endpoints and any path passed to `Maintenance::allow` gets a `503`
with a `Retry-After` header and an RFC 9457 body -- so a browser, a `fetch`,
and a CLI client all get an answer they can act on.

Because nothing looks the handle up in a registry, an application that does
not keep the handle cannot engage maintenance mode. That is the intended
trade: no ambient switch that some other part of the process can flip.

## Errors in release

`ErrorMapping::new().redact_errors(true)` replaces `text/plain` 5xx bodies
with a generic problem document. The layer sits at stage 10, inside the panic
catcher and outside everything that runs application code, so it catches both
handler errors and the bodiless `404`, `405`, `408` and `413` that axum and
`tower-http` emit on their own.

Redaction is a builder flag, not an automatic consequence of a release build.
Set it explicitly.

## Security headers

`SecurityHeaders::new()` sets `X-Content-Type-Options: nosniff`,
`X-Frame-Options: DENY`, and `Referrer-Policy: strict-origin-when-cross-origin`.
`.with_hsts()` adds `Strict-Transport-Security: max-age=31536000; includeSubDomains`,
and `.with_csp(policy)` sets a Content-Security-Policy from a string you
supply.

Add HSTS only once TLS is actually terminated in front of the process and you
are prepared for the one-year commitment, subdomains included.

### CSP nonces

`.with_csp_nonce(template)` is the other way to set the policy. It takes a
template containing `{nonce}` and substitutes a fresh 144-bit random value on
every request:

```rust,ignore
SecurityHeaders::new()
    .with_hsts()
    .with_csp_nonce("default-src 'self'; script-src 'self' 'nonce-{nonce}'")?
```

A template with no `{nonce}` in it is refused at construction rather than
quietly sent without one, and `.with_csp(..)` and `.with_csp_nonce(..)`
replace each other -- the last one called wins.

The nonce goes into the request extensions before the request reaches
anything else, and the framework stamps it onto every element it emits
itself: the Inertia `data-page` payload script, the module script and
stylesheet links resolved from the Vite manifest, and the Vite HMR client in
development. Read it in a handler by extracting `CspNonce` (or
`Option<CspNonce>`), and in a hand-written root document by calling
`body.nonce_attribute()`.

What the framework cannot stamp is anything the application writes itself: an
inline `<script>` in your own root document, an analytics snippet, a
third-party widget that injects scripts. Those either carry the nonce or stop
running.

Three details worth getting right before you turn this on:

- A nonce constrains only the directive that carries it. `script-src
  'nonce-X'` says nothing about `style-src` or `frame-src`.
- A CSP Level 2 or later browser *ignores* `'unsafe-inline'` in a directive
  that also carries a nonce, which is why `script-src 'nonce-X'
  'unsafe-inline'` is the documented fallback for old browsers rather than a
  contradiction. But `'unsafe-inline'` in a directive with no nonce in it --
  `style-src`, usually -- is not ignored by anything.
- Without `'strict-dynamic'` a nonce does not propagate to scripts that a
  nonce'd script goes on to insert, so a code-split bundle needs `'self'` (or
  `'strict-dynamic'`) in `script-src` alongside the nonce.

Do not let a shared cache store nonce'd HTML. The document and the header are
cached together so they stay consistent, but every visitor then gets a nonce
that every other visitor already knows, which is the one property it had.
Arcature sets no `Cache-Control` on the initial document; excluding it is the
CDN configuration's job.

## Ports and the environment

The listen port is resolved at startup in this order, highest first:

1. `ARCATURE_BACKEND_PORT`
2. `PORT`
3. `APP_PORT`
4. whatever `.config(..)` or `.port(..)` last set, defaulting to `3000`

The first that parses as a `u16` wins; one that is present but malformed --
`PORT=` in a compose file, say -- is skipped rather than fatal, so an empty
variable does not stop the process booting.

`ARCATURE_BACKEND_PORT` is first, ahead of the platform's `PORT`, because
`arc dev` sets it and its supervisor owns the process's only TCP listener. If
`PORT` outranked it, a stale `PORT` in a developer's `.env` would aim the
child at the address the supervisor already holds, and the one-port topology
would fail with a message about the port being in use.

`AppConfig::from_env()` reads `APP_NAME`, `APP_URL`, `APP_ENV` and `APP_PORT`.
Hand the result to `.config(..)` and `port` becomes the port the server binds.

`name` and `url` appear on the startup line -- the one record a booting
application emits unprompted -- so a process that believes it is reachable at
an address nobody expected says so immediately rather than three days later in
a broken emailed link. `url` is otherwise spent through
`AppConfig::absolute_url(path)`, which roots a path at `APP_URL` with the
trailing slash normalised away; that is the accessor to reach for whenever a
link has to be built with no request in scope, which is every link that
matters -- password resets, `redirect_uri`, anything signed. `path` is joined
and never substituted, so passing something that looks like a URL of its own
produces a path segment under the configured host rather than a link to
another one.

`env` is carried, readable back through `Application::config()`, and
**deliberately barred from gating behaviour**. Every protection that could plausibly key off an
environment -- the security headers, HSTS, release redaction of error
messages, the UAG endpoint -- keys off `cfg!(debug_assertions)` instead, so it
is decided when the binary is built. An `APP_ENV` that could switch them off
would let anyone who can set an environment variable downgrade a production
binary without redeploying it.

`ARCATURE_VITE_IPC` is set by `arc dev` and consulted by both the dev proxy
and the asset resolver. In production it is unset, and both subsystems fall
back to hashed build output. See
[ADR 0003](https://github.com/ArcatureLabs/Arcature/blob/main/docs/decisions/0003-one-tcp-port.md).

## Building for release

```sh
cargo build --release
```

Feature selection is how you control what ends up in the binary. The default
feature set is batteries-included and compiles the generated application with
no extra flags; `fullstack` adds the operator-adjacent extras (`storage-s3`,
`dev-proxy`, `uag`). Operator opt-ins -- `otel`, `api-docs`, `oauth` -- stay
off unless you name them; `api-docs` in particular publishes a map of your
attack surface.

Database drivers are separate features -- `db-postgres`, `db-sqlite`,
`db-mysql` -- and exactly one belongs in a build. Enabling `database` alone
gives a build that cannot connect to anything, which is deliberate: it is the
only way one crate serves all three without a SQLite user compiling the
Postgres protocol.

`#![forbid(unsafe_code)]` applies to the whole crate.

## Continuous integration

CI runs on both the MSRV (`1.97.1`) and `stable`, with `RUSTFLAGS: -D warnings`
and a `postgres:17` service on
`postgres://postgres:postgres@localhost:5432/arcature_test`. The gates, in
order:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets
cargo build
cargo test
cargo build --no-default-features
cargo build --features fullstack
cargo hack check --each-feature --no-dev-deps
cargo hack build --feature-powerset --skip database --keep-going
```

The feature-matrix jobs are there because feature gating is a compile-surface
decision: a feature that only builds when another one happens to be on is a
bug, and `cargo hack` is the only thing that finds it. A separate job runs
`cargo publish --dry-run --no-verify`.

The `justfile` at the repository root wraps these as `just check`, `just fmt`,
`just lint`, `just test`, `just features` and `just docs`.

## Releasing

Tagging a version triggers `.github/workflows/release.yml`, which publishes
`arcature-macros` first and then `arcature`, and builds the `arc` binary for
Linux, macOS and Windows.

There is no npm step, and there will not be one. See
[ADR 0001](https://github.com/ArcatureLabs/Arcature/blob/main/docs/decisions/0001-no-npm-package.md).
